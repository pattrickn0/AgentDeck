//! Headless execution (mode `-p`) of the agents, used by the bridge
//! (equivalent to `runHeadless`, `agyPromptArg`, `parseAgyOutput` and `cleanAgentOutput`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::Value;

use crate::agents::{self, agent_def, resolve_alias};
use crate::context::{self, ContextRequest};
use crate::process::{run_captured, Exit};
use crate::Core;

/// The Windows command line is limited to ~32 thousand characters.
pub const MAX_ARG_PROMPT: usize = 24_000;
/// Maximum timeout of a headless answer.
pub const HEADLESS_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Options of [`Core::run_headless`].
#[derive(Debug, Clone, Default)]
pub struct HeadlessOptions {
    /// Conversation key (`"<from>-><to>"`, with the project scope: see `instances::scoped`);
    /// `None` = no history.
    pub session_key: Option<String>,
    /// Project of whoever asks: the agent's `ask-agent` only sees its own instances.
    pub project: String,
    /// Folder where the agent runs (the project's); empty = the active project's.
    pub cwd: std::path::PathBuf,
    pub depth: i64,
    /// Replaces the bridge instructions in the system prompt (Claude).
    pub extra_system: Option<String>,
    pub attachments: Vec<String>,
    /// LLM model (`--model`; in OpenCode, `run -m`). Ignored in the agents that do not choose a model.
    pub model: Option<String>,
}

/// Cleans ANSI formatting and leftover reasoning tags (`<think>…</think>`).
pub fn clean_agent_output(text: &str) -> String {
    static THINK: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static ANSI: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let think = THINK.get_or_init(|| regex::Regex::new(r"(?is)<think>.*?</think>").expect("valid regex"));
    let ansi = ANSI.get_or_init(|| regex::Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]").expect("valid regex"));
    let t = think.replace_all(text, "");
    let t = ansi.replace_all(&t, "");
    t.replace("\r\n", "\n").replace('\r', "\n").trim().to_owned()
}

/// Text and conversation id extracted from `agy --output-format json`.
#[derive(Debug, Clone, PartialEq)]
pub struct AgyOutput {
    pub text: String,
    pub id: Option<String>,
}

/// Extracts the answer and the conversation id from agy's JSON output. Error when the status is
/// `ERROR`/`INVALID`/`CANCELED`.
pub fn parse_agy_output(out: &str) -> Result<AgyOutput, String> {
    let json: Option<Value> = serde_json::from_str(out)
        .ok()
        .or_else(|| out.trim().lines().rev().find(|l| l.trim().starts_with('{')).and_then(|l| serde_json::from_str(l).ok()));
    let Some(json) = json.filter(Value::is_object) else {
        return Ok(AgyOutput { text: out.trim().to_owned(), id: None });
    };
    let mut text: Option<String> = None;
    let mut id: Option<String> = None;
    walk_agy(&json, &mut text, &mut id);
    if let Some(status) = json.get("status").and_then(Value::as_str) {
        if ["ERROR", "INVALID", "CANCELED"].iter().any(|s| s.eq_ignore_ascii_case(status)) {
            let err = json.get("error");
            let msg = err
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| err.and_then(Value::as_str).map(str::to_owned))
                .or_else(|| err.filter(|e| !e.is_null()).map(|e| e.to_string()))
                .or_else(|| text.clone().filter(|t| !t.is_empty()))
                .unwrap_or_else(|| format!("agy returned status {status}"));
            return Err(msg);
        }
    }
    Ok(AgyOutput { text: text.unwrap_or_else(|| out.to_owned()).trim().to_owned(), id })
}

fn walk_agy(v: &Value, text: &mut Option<String>, id: &mut Option<String>) {
    let is_id = |k: &str| {
        let k = k.to_ascii_lowercase();
        ["conversation", "session"].iter().any(|p| k.strip_prefix(p).is_some_and(|rest| matches!(rest, "id" | "_id" | "-id")))
    };
    let is_text = |k: &str| ["response", "result", "text", "output", "answer"].iter().any(|t| t.eq_ignore_ascii_case(k));
    match v {
        Value::Object(map) => {
            for (k, val) in map {
                if let Value::String(s) = val {
                    // `!id` / `!text` in JS: an empty string also counts as "not found yet".
                    if id.as_deref().is_none_or(str::is_empty) && is_id(k) {
                        *id = Some(s.clone());
                    }
                    if text.as_deref().is_none_or(str::is_empty) && is_text(k) {
                        *text = Some(s.clone());
                    }
                } else {
                    walk_agy(val, text, id);
                }
            }
        }
        Value::Array(arr) => {
            for val in arr {
                // Loose strings in arrays have no key; the JS only looks at values with a key.
                if !val.is_string() {
                    walk_agy(val, text, id);
                }
            }
        }
        _ => {}
    }
}

impl Core {
    /// Runs `agent` in headless mode and returns the cleaned answer. BLOCKING.
    /// Equivalent to `runHeadless`.
    /// The answer and its errors go to an agent: always English (see `i18n::english`), whatever thread calls it.
    pub fn run_headless(&self, agent: &str, prompt: &str, opts: HeadlessOptions) -> anyhow::Result<String> {
        crate::i18n::english(|| self.run_headless_with(agent, prompt, opts, &self.inner.state.shutdown))
    }

    pub(crate) fn run_headless_with(
        &self,
        agent: &str,
        prompt: &str,
        opts: HeadlessOptions,
        cancel: &AtomicBool,
    ) -> anyhow::Result<String> {
        let base = resolve_alias(agent);
        let def = agent_def(base).ok_or_else(|| anyhow::anyhow!("Unknown agent: {agent}"))?;
        let reg = &self.inner.agents;
        let workspace = if opts.cwd.as_os_str().is_empty() { self.workspace() } else { opts.cwd.clone() };
        let project = opts.project.clone();

        let resolved = reg.resolve(base).ok_or_else(|| anyhow::anyhow!("{}", def.missing))?;
        let data_dir = self.paths().data_dir.to_string_lossy().into_owned();

        // Extension point (future RAG): no provider registered today.
        let chunks = context::gather(&self.inner.providers, &ContextRequest { agent, prompt, workspace: &workspace });
        let extra_ctx = context::render(&chunks);
        let prompt_owned;
        let prompt = if extra_ctx.is_empty() {
            prompt
        } else {
            prompt_owned = format!("{prompt}\n\n{extra_ctx}");
            prompt_owned.as_str()
        };

        let (program, mut args) = resolved.command();
        // `.cmd` shim (Windows): free text does not go on the command line; the prompt goes by file.
        let via_cmd = resolved.shim.is_some();
        let mut sid = opts.session_key.as_ref().and_then(|k| self.inner.headless_sessions.lock().get(k).cloned());
        let is_new = opts.session_key.is_some() && sid.is_none();
        let mut stdin = prompt.to_owned();
        let model =
            opts.model.as_deref().filter(|m| crate::models::valid_model(m) && crate::models::accepts_model(base)).map(str::to_owned);
        match base {
            "claude" => {
                if is_new {
                    sid = Some(uuid::Uuid::new_v4().to_string());
                }
                args.extend(["-p", "--output-format", "text"].map(String::from));
                args.extend(model.iter().flat_map(|m| crate::models::headless_args(base, m)));
                if let Some(s) = &sid {
                    args.push(if is_new { "--session-id" } else { "--resume" }.into());
                    args.push(s.clone());
                }
                let sys = opts.extra_system.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| reg.bridge_instructions(Some("claude")));
                args.extend(["--append-system-prompt".into(), sys, "--add-dir".into(), data_dir.clone()]);
            }
            "antigravity" => {
                args.extend([
                    "-p".into(),
                    self.prompt_arg(&workspace, prompt, via_cmd),
                    "--output-format".into(),
                    "json".into(),
                    "--print-timeout".into(),
                    "10m".into(),
                    "--add-dir".into(),
                    data_dir.clone(),
                ]);
                args.extend(model.iter().flat_map(|m| crate::models::headless_args(base, m)));
                if let Some(s) = &sid {
                    args.extend(["--conversation".into(), s.clone()]);
                }
                stdin.clear();
            }
            "aider" => {
                match self.prompt_file_if(prompt, via_cmd) {
                    Some(file) => args.extend(["--message-file".into(), file]),
                    None => args.extend(["--message".into(), prompt.to_owned()]),
                }
                args.extend(["--no-auto-commits", "--yes-always"].map(String::from));
                stdin.clear();
            }
            "goose" => {
                match self.prompt_file_if(prompt, via_cmd) {
                    Some(file) => args.extend(["run".into(), "--instructions".into(), file]),
                    None => args.extend(["run".into(), "--text".into(), prompt.to_owned()]),
                }
                stdin.clear();
            }
            "copilot" => {
                args.extend(["explain".into(), self.prompt_arg(&workspace, prompt, via_cmd)]);
                stdin.clear();
            }
            "q" => {
                args.extend(["chat".into(), "--message".into(), self.prompt_arg(&workspace, prompt, via_cmd)]);
                stdin.clear();
            }
            "interpreter" => {
                args.extend(["-y".into(), "--prompt".into(), self.prompt_arg(&workspace, prompt, via_cmd)]);
                stdin.clear();
            }
            // `codex exec -`: the prompt comes through standard input; only the final answer goes to stdout.
            "codex" => {
                args.push("exec".into());
                args.extend(model.iter().flat_map(|m| crate::models::headless_args(base, m)));
                args.extend(["--skip-git-repo-check", "-"].map(String::from));
            }
            "opencode" => {
                args.push("run".into());
                args.extend(model.iter().flat_map(|m| crate::models::headless_args(base, m)));
                args.push(self.prompt_arg(&workspace, prompt, via_cmd));
                stdin.clear();
            }
            _ => {}
        }

        if via_cmd {
            // Batch files do not receive line breaks in the arguments (e.g. the system prompt).
            args = args.into_iter().map(|a| a.replace("\r\n", " ").replace(['\r', '\n'], " ")).collect();
        }
        let guard = self.headless_token(&project, agent);
        let mut cmd = std::process::Command::new(&program);
        cmd.args(&args).current_dir(&workspace).env_clear().envs(agents::agent_env(self, agent, opts.depth, &guard.token));
        let captured = run_captured(cmd, Some(stdin.into_bytes()), HEADLESS_TIMEOUT, Some(cancel), Some(&self.inner.processes))
            .map_err(|e| anyhow::anyhow!("failed to start \"{}\": {e}", program.display()))?;
        drop(guard);
        if captured.exit == Exit::TimedOut {
            anyhow::bail!("agent \"{agent}\" exceeded the answer time limit (10 minute timeout)");
        }
        if captured.exit == Exit::Cancelled {
            anyhow::bail!("execution interrupted");
        }

        let resumed = opts.session_key.is_some() && !is_new;
        if captured.exit == Exit::Code(Some(0)) && !captured.stdout.trim().is_empty() {
            if base == "antigravity" {
                match parse_agy_output(&captured.stdout) {
                    Ok(parsed) => {
                        if let (Some(key), Some(id)) = (&opts.session_key, &parsed.id) {
                            self.inner.headless_sessions.lock().insert(key.clone(), id.clone());
                        }
                        return Ok(clean_agent_output(&parsed.text));
                    }
                    Err(e) if !resumed => anyhow::bail!("{e}"),
                    Err(_) => {}
                }
            } else {
                if let (true, Some(key), Some(s)) = (is_new, &opts.session_key, &sid) {
                    self.inner.headless_sessions.lock().insert(key.clone(), s.clone());
                }
                return Ok(clean_agent_output(&captured.stdout));
            }
        }
        if resumed {
            // Shutting down (or cancelled): retrying would open an orphan process after `kill_all`.
            if self.stopping(cancel) {
                anyhow::bail!("execution interrupted");
            }
            // Expired/invalid session: drop it and try again, clean.
            if let Some(key) = &opts.session_key {
                self.inner.headless_sessions.lock().remove(key);
            }
            let retry = HeadlessOptions { attachments: Vec::new(), ..opts };
            return self.run_headless_with(agent, prompt, retry, cancel);
        }
        let source = if captured.stderr.is_empty() { &captured.stdout } else { &captured.stderr };
        let msg = crate::util::last_meaningful_lines(source, 6);
        if msg.is_empty() {
            let code = match captured.exit {
                Exit::Code(Some(c)) => c.to_string(),
                _ => "null".into(),
            };
            anyhow::bail!("process exited with code {code}");
        }
        anyhow::bail!("{msg}")
    }

    /// Cancelled or the app shutting down (`Core::shutdown`): no more processes are opened.
    fn stopping(&self, cancel: &AtomicBool) -> bool {
        cancel.load(Ordering::SeqCst) || self.inner.state.shutdown.load(Ordering::SeqCst)
    }

    /// Writes the prompt to `data/prompts` when it cannot go on the command line: too long
    /// (the Windows one has ~32 thousand characters) or `force` (it would go through a `.cmd` shim).
    /// Returns the file path; `None` = it goes as an argument anyway.
    fn prompt_file_if(&self, prompt: &str, force: bool) -> Option<String> {
        if !force && prompt.chars().count() <= MAX_ARG_PROMPT {
            return None;
        }
        let dir = self.paths().data_dir.join("prompts");
        let file = dir.join(format!("s_{}_prompt-{}.md", self.inner.state.session_id, uuid::Uuid::new_v4().simple()));
        match std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&file, prompt)) {
            Ok(()) => Some(file.to_string_lossy().into_owned()),
            Err(e) => {
                log::warn!("could not write the prompt to {}: {e}", file.display());
                None
            }
        }
    }

    /// The prompt as an argument, or the instruction to read its file (see [`Core::prompt_file_if`]).
    /// Equivalent to `agyPromptArg`.
    fn prompt_arg(&self, workspace: &std::path::Path, prompt: &str, force: bool) -> String {
        match self.prompt_file_if(prompt, force) {
            Some(file) => format!(
                "The full message is in the file {}. Read the whole file and answer following what it asks.",
                crate::util::ref_path(workspace, std::path::Path::new(&file))
            ),
            None => prompt.to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_output() {
        assert_eq!(clean_agent_output("<think>\nhmm\n</think>\x1b[1;32mHi\x1b[0m\r\nworld\r!  "), "Hi\nworld\n!");
        assert_eq!(clean_agent_output("<THINK>a</THINK>b<think>c</think>"), "b");
        assert_eq!(clean_agent_output(""), "");
    }

    #[test]
    fn agy_parsing() {
        let p = parse_agy_output(r#"{"status":"OK","response":"  answer  ","conversation_id":"c-1"}"#).expect("ok");
        assert_eq!(p, AgyOutput { text: "answer".into(), id: Some("c-1".into()) });

        // Nested and with noise before the last JSON line.
        let out = "progress log\n{\"data\":{\"result\":\"r\",\"meta\":{\"sessionId\":\"s9\"}}}\n";
        let p = parse_agy_output(out).expect("ok");
        assert_eq!(p, AgyOutput { text: "r".into(), id: Some("s9".into()) });

        // Without JSON: raw text.
        assert_eq!(parse_agy_output(" only text \n").expect("ok"), AgyOutput { text: "only text".into(), id: None });

        // JSON without a text field: returns the whole output.
        let p = parse_agy_output(r#"{"x":1}"#).expect("ok");
        assert_eq!(p.text, r#"{"x":1}"#);

        // Error status.
        assert_eq!(parse_agy_output(r#"{"status":"ERROR","error":{"message":"failed"}}"#).expect_err("error"), "failed");
        assert_eq!(parse_agy_output(r#"{"status":"canceled"}"#).expect_err("error"), "agy returned status canceled");
        assert_eq!(parse_agy_output(r#"{"status":"INVALID","error":"bad"}"#).expect_err("error"), "bad");
    }
}
