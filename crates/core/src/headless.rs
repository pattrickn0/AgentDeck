//! Execução headless (modo `-p`) dos agentes, usada pela ponte e pelas mesas
//! (equivalente a `runHeadless`, `agyPromptArg`, `parseAgyOutput` e `cleanAgentOutput`).

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use serde_json::Value;

use crate::agents::{self, agent_def, resolve_alias, DEFAULT_OLLAMA_MODEL};
use crate::context::{self, ContextRequest};
use crate::events::{FeedEntry, FeedKind};
use crate::ollama::{AgentEvent, RunOptions};
use crate::process::{run_captured, Exit};
use crate::Core;

/// A linha de comando do Windows tem limite de ~32 mil caracteres.
pub const MAX_ARG_PROMPT: usize = 24_000;
/// Timeout máximo de uma resposta headless.
pub const HEADLESS_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Opções de [`Core::run_headless`].
#[derive(Debug, Clone, Default)]
pub struct HeadlessOptions {
    /// Chave da conversa (`"<de>-><para>"`); `None` = sem histórico.
    pub session_key: Option<String>,
    pub depth: i64,
    /// Substitui as instruções da ponte no prompt de sistema (Claude/Ollama).
    pub extra_system: Option<String>,
    pub attachments: Vec<String>,
}

/// Limpa formatação ANSI e tags residuais de raciocínio (`<think>…</think>`).
pub fn clean_agent_output(text: &str) -> String {
    static THINK: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static ANSI: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let think = THINK.get_or_init(|| regex::Regex::new(r"(?is)<think>.*?</think>").expect("regex válida"));
    let ansi = ANSI.get_or_init(|| regex::Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]").expect("regex válida"));
    let t = think.replace_all(text, "");
    let t = ansi.replace_all(&t, "");
    t.replace("\r\n", "\n").replace('\r', "\n").trim().to_owned()
}

/// Texto e id da conversa extraídos de `agy --output-format json`.
#[derive(Debug, Clone, PartialEq)]
pub struct AgyOutput {
    pub text: String,
    pub id: Option<String>,
}

/// Extrai a resposta e o id da conversa da saída JSON do agy. Erro quando o status é
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
                .unwrap_or_else(|| format!("agy retornou status {status}"));
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
                    // `!id` / `!text` no JS: string vazia também conta como "ainda não achou".
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
                // Strings soltas em arrays não têm chave; o JS só olha valores com chave.
                if !val.is_string() {
                    walk_agy(val, text, id);
                }
            }
        }
        _ => {}
    }
}

impl Core {
    /// Executa `agent` em modo headless e devolve a resposta limpa. BLOQUEANTE.
    /// Equivalente a `runHeadless`; o Ollama usa o motor interno com ferramentas.
    pub fn run_headless(&self, agent: &str, prompt: &str, opts: HeadlessOptions) -> anyhow::Result<String> {
        self.run_headless_with(agent, prompt, opts, &self.inner.state.shutdown)
    }

    pub(crate) fn run_headless_with(
        &self,
        agent: &str,
        prompt: &str,
        opts: HeadlessOptions,
        cancel: &AtomicBool,
    ) -> anyhow::Result<String> {
        let sub_model = agent.strip_prefix("ollama:");
        let base = if sub_model.is_some() { "ollama" } else { resolve_alias(agent) };
        let def = agent_def(base).ok_or_else(|| anyhow::anyhow!("Agente desconhecido: {agent}"))?;
        let reg = &self.inner.agents;

        if base == "ollama" {
            let resolved = reg.resolve_ollama(sub_model).ok_or_else(|| anyhow::anyhow!("{}", def.missing))?;
            let model = sub_model.map(str::to_owned).unwrap_or(resolved.model);
            let model = if model.is_empty() { DEFAULT_OLLAMA_MODEL.to_owned() } else { model };
            let mut ro = RunOptions::new(model, prompt, self.workspace());
            ro.session_key = opts.session_key;
            ro.extra_system = opts.extra_system;
            ro.attachments = opts.attachments;
            // `ask-agent` chamado pelo modelo enxerga a ponte (com a profundidade correta).
            ro.command_env = Some(agents::agent_env(self, agent, opts.depth));
            let core = self.clone();
            let out = self.inner.ollama.run(ro, cancel, &mut |ev| {
                if let AgentEvent::ToolCall { name, summary } = ev {
                    core.feed(FeedEntry::new(FeedKind::Thinking, Some("ollama"), None, format!("⚙️ [{name}] {summary}")));
                }
            })?;
            if out.cancelled {
                anyhow::bail!("execução interrompida");
            }
            return Ok(out.text);
        }

        let resolved = reg.resolve(base).ok_or_else(|| anyhow::anyhow!("{}", def.missing))?;
        let data_dir = self.paths().data_dir.to_string_lossy().into_owned();
        let workspace = self.workspace();

        // Ponto de extensão (RAG futuro): nenhum provedor registrado hoje.
        let chunks = context::gather(&self.inner.providers, &ContextRequest { agent, prompt, workspace: &workspace });
        let extra_ctx = context::render(&chunks);
        let prompt_owned;
        let prompt = if extra_ctx.is_empty() {
            prompt
        } else {
            prompt_owned = format!("{prompt}\n\n{extra_ctx}");
            prompt_owned.as_str()
        };

        let mut args = resolved.pre.clone();
        let mut sid = opts.session_key.as_ref().and_then(|k| self.inner.headless_sessions.lock().get(k).cloned());
        let is_new = opts.session_key.is_some() && sid.is_none();
        let mut stdin = prompt.to_owned();
        match base {
            "claude" => {
                if is_new {
                    sid = Some(uuid::Uuid::new_v4().to_string());
                }
                args.extend(["-p", "--output-format", "text"].map(String::from));
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
                    self.agy_prompt_arg(prompt),
                    "--output-format".into(),
                    "json".into(),
                    "--print-timeout".into(),
                    "10m".into(),
                    "--add-dir".into(),
                    data_dir.clone(),
                ]);
                if let Some(s) = &sid {
                    args.extend(["--conversation".into(), s.clone()]);
                }
                stdin.clear();
            }
            "gemini" => {
                if is_new {
                    sid = Some(uuid::Uuid::new_v4().to_string());
                }
                args.extend(["-p", prompt, "-o", "text", "--approval-mode", "yolo", "--skip-trust"].map(String::from));
                if let Some(s) = &sid {
                    args.push(if is_new { "--session-id" } else { "--resume" }.into());
                    args.push(s.clone());
                }
                stdin.clear();
            }
            "aider" => {
                args.extend(["--message", prompt, "--no-auto-commits", "--yes-always"].map(String::from));
                stdin.clear();
            }
            "goose" => {
                args.extend(["run", "--text", prompt].map(String::from));
                stdin.clear();
            }
            "copilot" => {
                args.extend(["explain", prompt].map(String::from));
                stdin.clear();
            }
            "q" => {
                args.extend(["chat", "--message", prompt].map(String::from));
                stdin.clear();
            }
            "interpreter" => {
                args.extend(["-y", "--prompt", prompt].map(String::from));
                stdin.clear();
            }
            _ => {}
        }

        let mut cmd = std::process::Command::new(&resolved.program);
        cmd.args(&args).current_dir(&workspace).env_clear().envs(agents::agent_env(self, agent, opts.depth));
        let captured = run_captured(cmd, Some(stdin.into_bytes()), HEADLESS_TIMEOUT, None, Some(&self.inner.processes))
            .map_err(|e| anyhow::anyhow!("falha ao iniciar \"{}\": {e}", resolved.program.display()))?;
        if captured.exit == Exit::TimedOut {
            anyhow::bail!("agente \"{agent}\" excedeu o tempo limite de resposta (timeout de 10 minutos)");
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
            // Sessão expirada/inválida: descarta e tenta de novo, limpa.
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
            anyhow::bail!("processo saiu com código {code}");
        }
        anyhow::bail!("{msg}")
    }

    /// O agy -p recebe o prompt pela linha de comando; prompts longos vão para um arquivo
    /// em `dados/prompts`. Equivalente a `agyPromptArg`.
    fn agy_prompt_arg(&self, prompt: &str) -> String {
        if prompt.chars().count() <= MAX_ARG_PROMPT {
            return prompt.to_owned();
        }
        let dir = self.paths().data_dir.join("prompts");
        let file = dir.join(format!("s_{}_prompt-{}.md", self.inner.state.session_id, crate::util::now_ms()));
        if std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&file, prompt)).is_err() {
            return prompt.to_owned();
        }
        format!(
            "A mensagem completa está no arquivo {}. Leia o arquivo inteiro e responda seguindo o que ele pede.",
            crate::util::ref_path(&self.workspace(), &file)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_output() {
        assert_eq!(clean_agent_output("<think>\nhmm\n</think>\x1b[1;32mOlá\x1b[0m\r\nmundo\r!  "), "Olá\nmundo\n!");
        assert_eq!(clean_agent_output("<THINK>a</THINK>b<think>c</think>"), "b");
        assert_eq!(clean_agent_output(""), "");
    }

    #[test]
    fn agy_parsing() {
        let p = parse_agy_output(r#"{"status":"OK","response":"  resposta  ","conversation_id":"c-1"}"#).expect("ok");
        assert_eq!(p, AgyOutput { text: "resposta".into(), id: Some("c-1".into()) });

        // Aninhado e com ruído antes da última linha JSON.
        let out = "log de progresso\n{\"data\":{\"result\":\"r\",\"meta\":{\"sessionId\":\"s9\"}}}\n";
        let p = parse_agy_output(out).expect("ok");
        assert_eq!(p, AgyOutput { text: "r".into(), id: Some("s9".into()) });

        // Sem JSON: texto bruto.
        assert_eq!(parse_agy_output(" só texto \n").expect("ok"), AgyOutput { text: "só texto".into(), id: None });

        // JSON sem campo de texto: devolve a saída inteira.
        let p = parse_agy_output(r#"{"x":1}"#).expect("ok");
        assert_eq!(p.text, r#"{"x":1}"#);

        // Status de erro.
        assert_eq!(parse_agy_output(r#"{"status":"ERROR","error":{"message":"falhou"}}"#).expect_err("erro"), "falhou");
        assert_eq!(parse_agy_output(r#"{"status":"canceled"}"#).expect_err("erro"), "agy retornou status canceled");
        assert_eq!(parse_agy_output(r#"{"status":"INVALID","error":"ruim"}"#).expect_err("erro"), "ruim");
    }
}
