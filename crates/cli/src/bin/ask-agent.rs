//! `ask-agent` — asks something to another agent through the Holt bridge (port of bin/ask-agent.js).
//!
//! Usage: ask-agent <agent> "message"     (or "-" to read the message from stdin; "--file <file>" from a file)
//!      ask-agent --new <agent> "message"   (opens another instance, even if one already exists)
//!      ask-agent --model <model> <agent> "message"   (chooses the instance's LLM when opening it; combines with --new)
//!      ask-agent --reply <n> "answer"       (answers question number n)
//!      ask-agent --list
//!      ask-agent --team                       (your boss and your crew)
//!      ask-agent --models [agent]            (installed agents, the models --model accepts and the quota)
//!      ask-agent --boss "message"            (asks/reports to your boss)
//!      ask-agent --signal busy|idle [<json>]  (agent hooks: start/end of turn; Codex's notify JSON carries its thread id)
//!      ask-agent --signal session [<id>]      (agent hook: the CLI's conversation id; without <id>, `session_id` in the JSON on stdin)
//!      ask-agent --usage                      (Claude Code statusline: plan usage)
//!      ask-agent --learn [<file>...]   (what the project has already learned, in general or about those files)
//!      ask-agent --outline <file>...   (each file's signatures with the line, without the bodies)
//!      ask-agent --inbox [--all]          (answers to your questions and mail not read yet; --all: the last 20)
//!      ask-agent --read <n>               (the full text of mail #n, a long message that was not typed into your terminal)
//!      ask-agent --task new "title" [--agent <model>] [--model <m>] [--effort low|medium|high] [--level simple|medium|complex] [--base <branch>] [--start] -|"request"|--file <file> [--scope <path>...]
//!      ask-agent --task list | show <id> | start <id> | merge <id> | validate <id> | discard <id>
//!      ask-agent --task done <id> -|"report"|--file <file>   (only the agent of the task itself)
//!      ask-agent --task return <id> "feedback"|--file <file>   (sends a task under review back)
//!      ask-agent --task reject <id> "reason"|--file <file>|-   (rejects a task waiting for validation)
//!
//! `<agent>` is `model[@role[@name]]`: `claude`, `claude@dev`, `claude@dev@ana`.
//!
//! `--file` reads the text from a file (UTF-8, or UTF-16 with BOM, as PowerShell 5.1 writes it). It is the
//! recommended path for long texts or ones with accents: in Windows PowerShell, piping through the shell
//! turns accents and dashes into `?` (the `$OutputEncoding` is US-ASCII).
//!
//! In a tab, `ask-agent` waits for the answer for up to `HOLT_ASK_WAIT` seconds (default 90, below
//! Claude Code's default Bash timeout; `0` = until the end). If the other agent takes longer, it ends with a
//! notice, and the answer is typed later into the asker's terminal.
//!
//! Exit codes: 0 = success; 1 = operational error; 2 = argument error or outside Holt.
//! `--signal` and `--usage` always exit with 0 (a hook that fails must not get in the agent's way).

use std::collections::{BTreeMap, HashMap};
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

use holt_core::usage::{from_claude_statusline, quota_advisory, usage_to_json};
use holt_core::util::decode_text;
use holt_core::{AgentUsage, Ident};
use serde_json::{json, Value};

const USAGE: &str =
    "Usage: ask-agent [--new] [--model <model>] <agent[@role[@name]]> \"message\"   |   ask-agent --reply <n> \"answer\"   |   ask-agent --list   |   ask-agent --team   |   ask-agent --models [agent]   |   ask-agent --boss \"message\"   |   ask-agent --learn [<file>...]   |   ask-agent --outline <file>...   |   ask-agent --inbox [--all]   |   ask-agent --read <n>   |   ask-agent --task new|list|show|start|merge|validate|reject|discard|done|return ... (tasks: ask-agent --task; a task's effort: --task new \"title\" --effort low|medium|high; the size of the work, which decides which models can handle it: --level simple|medium|complex (default medium); the write scope, with a queue: --task new \"title\" \"request\" --scope <path>...)";

/// Seconds `ask-agent` waits for the answer before leaving it to be typed later.
const DEFAULT_WAIT: u64 = 90;
/// `POST /usage` limit: the statusline runs on every refresh of Claude's screen.
const USAGE_POST_TIMEOUT: Duration = Duration::from_millis(1500);
/// Limit of the user's own statusline (passed through by `--usage`).
const USER_STATUSLINE_TIMEOUT: Duration = Duration::from_secs(3);

struct Bridge {
    port: String,
    token: String,
}

impl Bridge {
    /// Makes the request (`timeout` = `None`: no limit, agent answers may take minutes).
    fn request(
        &self,
        method: &str,
        route: &str,
        body: Option<&Value>,
        timeout: Option<std::time::Duration>,
    ) -> Result<(u16, Value), String> {
        let agent: ureq::Agent =
            ureq::Agent::config_builder().proxy(None).http_status_as_error(false).timeout_global(timeout).build().into();
        let url = format!("http://127.0.0.1:{}{route}", self.port);
        let resp = match (method, body) {
            ("GET", _) => agent.get(&url).header("x-holt-token", &self.token).header("content-type", "application/json").call(),
            (_, Some(b)) => agent.post(&url).header("x-holt-token", &self.token).send_json(b),
            (_, None) => agent.post(&url).header("x-holt-token", &self.token).send_empty(),
        };
        let mut resp = resp.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let text = resp.body_mut().with_config().limit(1 << 30).read_to_string().map_err(|e| e.to_string())?;
        let json: Value = serde_json::from_str(&text).map_err(|e| format!("invalid response from the bridge: {e}"))?;
        Ok((status, json))
    }
}

fn fail(msg: impl std::fmt::Display, code: u8) -> ExitCode {
    eprintln!("ask-agent: {msg}");
    ExitCode::from(code)
}

/// Body of `POST /signal`. `session` carries the CLI's conversation id: the argument when it is one (`--signal session
/// <id>`), else the `session_id` of the JSON a hook receives on stdin (`stdin` is only read in that case).
fn signal_body(state: &str, arg: Option<&str>, stdin: impl FnOnce() -> String) -> Value {
    if state == "idle" {
        // Codex's `notify` appends its JSON to the command: the thread id in it is the conversation `codex resume` takes.
        let thread = arg.and_then(holt_core::resume::notify_thread_id);
        return thread.map_or_else(|| json!({ "state": state }), |t| json!({ "state": state, "session": t }));
    }
    if state != "session" {
        return json!({ "state": state });
    }
    let from_json = |text: &str| serde_json::from_str::<Value>(text).ok().and_then(|v| v["session_id"].as_str().map(str::to_owned));
    let session = match arg.map(str::trim).filter(|a| !a.is_empty()) {
        Some(a) if !a.starts_with('{') => Some(a.to_owned()),
        Some(json_arg) => from_json(json_arg),
        None => from_json(&stdin()),
    };
    json!({ "state": state, "session": session.unwrap_or_default() })
}

fn main() -> ExitCode {
    let env = |k: &str| holt_core::env_var(k);
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--usage") {
        let bridge = env("PORT").zip(env("TOKEN")).map(|(port, token)| Bridge { port, token });
        return statusline(bridge);
    }
    let (Some(port), Some(token)) = (env("PORT"), env("TOKEN")) else {
        if args.first().is_some_and(|a| a == "--signal") {
            return ExitCode::SUCCESS;
        }
        return fail("run this command from inside a Holt terminal.", 2);
    };
    let from = env("AGENT").unwrap_or_else(|| "shell".into());
    let depth: i64 = env("DEPTH").and_then(|d| d.trim().parse::<f64>().ok()).map(|d| d as i64).unwrap_or(0);
    let bridge = Bridge { port, token };

    if args.first().is_some_and(|a| a == "--signal") {
        // Agent hook (Codex still appends some JSON, ignored). Failures are silent.
        let state = args.get(1).map(String::as_str).unwrap_or("");
        let body = signal_body(state, args.get(2).map(String::as_str), || {
            // The hook gets its JSON on stdin; in a terminal there is none (reading would block).
            let mut raw = String::new();
            if !std::io::stdin().is_terminal() {
                let _ = std::io::stdin().read_to_string(&mut raw);
            }
            raw
        });
        let _ = bridge.request("POST", "/signal", Some(&body), Some(std::time::Duration::from_secs(3)));
        return ExitCode::SUCCESS;
    }
    if args.first().is_some_and(|a| a == "--task") {
        return task_command(&bridge, &args[1..]);
    }
    if args.first().is_some_and(|a| a == "--learn") {
        return learn_command(&bridge, &args[1..]);
    }
    if args.first().is_some_and(|a| a == "--outline") {
        return outline_command(&bridge, &args[1..]);
    }
    if args.first().is_some_and(|a| a == "--inbox") {
        return inbox_command(&bridge, &args[1..]);
    }
    if args.first().is_some_and(|a| a == "--read") {
        return read_command(&bridge, &args[1..]);
    }
    if args.first().is_some_and(|a| a == "--models") {
        return models_command(&bridge, args.get(1).map(String::as_str));
    }
    let (force_new, model) = match take_flags(&mut args) {
        Ok(v) => v,
        Err(e) => return fail(format!("{e}\n{USAGE}"), 2),
    };
    let file_body = match take_body_file(&mut args) {
        Ok(p) => p,
        Err(e) => return fail(format!("{e}\n{USAGE}"), 2),
    };
    let mut reply_to: Option<u64> = None;
    if args.first().is_some_and(|a| a == "--reply") {
        match args.get(1).and_then(|n| n.trim_start_matches('#').parse().ok()) {
            Some(n) => reply_to = Some(n),
            None => return fail(format!("--reply needs the question number.\n{USAGE}"), 2),
        }
        // From here on, `args[0]` plays the target (ignored) and the rest is the message.
        args.drain(..2);
        args.insert(0, String::new());
        if model.is_some() {
            return fail(format!("--model does not combine with --reply (the model applies when opening an instance).\n{USAGE}"), 2);
        }
    }
    if args.first().is_some_and(|a| a == "--team") {
        return match team(&bridge) {
            Ok(t) => {
                println!("You: {}", with_model(&t.id, &t.models));
                match &t.boss {
                    Some(b) => println!("Boss: {} (report to it with: ask-agent --boss \"message\")", with_model(b, &t.models)),
                    None => println!("Boss: none (you answer directly to the user)"),
                }
                if t.crew.is_empty() {
                    println!("Crew: no agents");
                } else {
                    println!("Crew: {}", t.crew.iter().map(|c| with_model(c, &t.models)).collect::<Vec<_>>().join(", "));
                }
                ExitCode::SUCCESS
            }
            Err(e) => fail(e, 1),
        };
    }
    if args.first().is_some_and(|a| a == "--boss") {
        // The boss is an instance that already exists: `--new` would open another one instead of talking to it.
        // Refusing is clearer than ignoring it silently.
        if force_new {
            return fail(format!("--new does not combine with --boss (the boss is an instance that already exists).\n{USAGE}"), 2);
        }
        if model.is_some() {
            return fail(format!("--model does not combine with --boss (the boss is an instance that already exists).\n{USAGE}"), 2);
        }
        match team(&bridge) {
            Ok(Team { boss: Some(boss), .. }) => args[0] = boss,
            Ok(_) => return fail("you have no boss: you answer directly to the user (write the answer in your terminal).", 1),
            Err(e) => return fail(e, 1),
        }
    }
    let to = args.first().map(String::as_str).unwrap_or("");
    if reply_to.is_none() && (to.is_empty() || to == "-h" || to == "--help") {
        println!("{USAGE}");
        return ExitCode::from(if to.is_empty() { 2 } else { 0 });
    }
    if to == "--list" {
        return match bridge.request("GET", "/agents", None, None) {
            Ok((_, json)) => match json.get("agents").and_then(Value::as_array) {
                Some(list) => {
                    let models = models_of(&json);
                    let names: Vec<String> = list.iter().filter_map(Value::as_str).map(|n| with_model(n, &models)).collect();
                    println!("{}", names.join("\n"));
                    ExitCode::SUCCESS
                }
                None => fail(json.get("error").and_then(Value::as_str).unwrap_or("invalid response from the bridge"), 1),
            },
            Err(e) => fail(e, 1),
        };
    }

    let mut message = match &file_body {
        Some(path) => match read_text_file(path) {
            Ok(text) => text,
            Err(e) => return fail(e, 2),
        },
        None => args.get(1..).map(|a| a.join(" ")).unwrap_or_default(),
    };
    // stdin is only read with "-" or when it comes from a pipe/file: in a terminal, with no message, reading
    // would hang without a notice. `decode_text`: UTF-8 (or UTF-16 with BOM) without error, and cp1252 files
    // become replacement characters instead of failing.
    if file_body.is_none() && (message == "-" || (message.is_empty() && !std::io::stdin().is_terminal())) {
        let mut raw = Vec::new();
        if let Err(e) = std::io::stdin().read_to_end(&mut raw) {
            return fail(format!("failed to read stdin: {e}"), 1);
        }
        message = decode_text(&raw);
    }
    if message.trim().is_empty() {
        return fail("empty message.", 2);
    }

    if reply_to.is_none() {
        if let Some(note) = quota_advisory_for_target(&bridge, to) {
            println!("Quota warning: {note}. The message will be sent.");
        }
    }

    let wait = env("ASK_WAIT").and_then(|w| w.trim().parse::<u64>().ok()).unwrap_or(DEFAULT_WAIT);
    // `ack`: the bridge only counts the answer as delivered when this process confirms it (see `confirm_delivered`).
    let mut body = json!({ "from": from, "to": to, "message": message, "depth": depth, "new": force_new, "wait": wait, "ack": true });
    if let Some(n) = reply_to {
        body["reply_to"] = json!(n);
    }
    if let Some(m) = &model {
        body["model"] = json!(m);
    }
    match bridge.request("POST", "/ask", Some(&body), None) {
        // 202: the answer has not arrived yet and will be typed later into this terminal (the text explains).
        Ok((200 | 202, json)) => {
            let answer = match json.get("answer") {
                Some(Value::String(s)) => s.clone(),
                Some(other) => other.to_string(),
                None => "undefined".into(),
            };
            println!("{answer}");
            confirm_delivered(&bridge, &json);
            ExitCode::SUCCESS
        }
        Ok((_, json)) => {
            let code = fail(json.get("error").and_then(Value::as_str).unwrap_or("unknown error"), 1);
            confirm_delivered(&bridge, &json);
            code
        }
        Err(e) => fail(e, 1),
    }
}

/// Tells the bridge that the question's answer (the body's `request`) was printed, so that it leaves the
/// inbox. Without this (this process died first), the answer stays in `ask-agent --inbox`. A failure here
/// only makes the answer show up again in `--inbox`.
fn confirm_delivered(bridge: &Bridge, json: &Value) {
    if let Some(request) = json.get("request").and_then(Value::as_u64) {
        let _ = bridge.request("POST", "/inbox/delivered", Some(&json!({ "request": request })), Some(std::time::Duration::from_secs(5)));
    }
}

/// Options at the start of the command line, in any order: `--new`, `--model <m>` and `--model=<m>`.
/// Returns `(--new, model)` and leaves the rest (target and message) in `args`.
fn take_flags(args: &mut Vec<String>) -> Result<(bool, Option<String>), String> {
    let (mut force_new, mut model) = (false, None);
    loop {
        match args.first().map(String::as_str) {
            Some("--new") => force_new = true,
            Some("--model") => {
                if args.len() < 2 || args[1].is_empty() || args[1].starts_with("--") {
                    return Err("--model needs the model name (e.g. --model sonnet).".into());
                }
                model = Some(args.remove(1));
            }
            Some(a) if a.starts_with("--model=") => {
                let m = a["--model=".len()..].to_owned();
                if m.is_empty() {
                    return Err("--model needs the model name (e.g. --model=sonnet).".into());
                }
                model = Some(m);
            }
            _ => return Ok((force_new, model)),
        }
        args.remove(0);
    }
}

/// `--models [agent]`: the agents installed on this computer and the models `--model` accepts.
fn models_command(bridge: &Bridge, only: Option<&str>) -> ExitCode {
    let route = match only {
        Some(a) => format!("/models?agent={a}"),
        None => "/models".to_owned(),
    };
    // The CLIs may take up to 15 s on the first query (afterwards it comes from the bridge's cache).
    match bridge.request("GET", &route, None, Some(Duration::from_secs(40))) {
        Ok((200, json)) => {
            println!("{}", json.get("text").and_then(Value::as_str).unwrap_or_default());
            ExitCode::SUCCESS
        }
        Ok((_, json)) => fail(json.get("error").and_then(Value::as_str).unwrap_or("unknown error"), 1),
        Err(e) => fail(e, 1),
    }
}

/// Reads the quota line of the inventory (`--models`) without blocking the send if the read fails.
fn quota_advisory_for_target(bridge: &Bridge, target: &str) -> Option<String> {
    let target = Ident::parse(target)?;
    let kind = holt_core::agents::resolve_alias(&target.kind);
    if !["claude", "antigravity", "codex"].contains(&kind) {
        return None;
    }
    let (status, json) = bridge.request("GET", "/models", None, Some(Duration::from_secs(40))).ok()?;
    if status != 200 {
        return None;
    }
    let text = json.get("text")?.as_str()?;
    let (choices, used) = quota_usage_from_inventory(text);
    quota_advisory(kind, &choices, &used)
}

/// Extracts the biggest relevant group of each quota line in the `--models` text inventory.
/// The inventory is written by another area: it may come with `cota:`/`quota:` and `ESGOTADA`/`EXHAUSTED`,
/// so both spellings are read here.
fn quota_usage_from_inventory(text: &str) -> (Vec<String>, HashMap<String, f32>) {
    let official = ["claude", "antigravity", "codex"];
    let mut choices = Vec::new();
    let mut used = HashMap::new();
    let mut current: Option<&str> = None;
    for line in text.lines() {
        if line.starts_with(' ') {
            if let (Some(kind), Some(note)) = (current, quota_note(line.trim_start())) {
                if let Some(value) = quota_used_from_note(note) {
                    used.insert(kind.to_owned(), value);
                }
            }
            continue;
        }
        let heading = line.split(" --model").next().or_else(|| line.split(':').next()).unwrap_or(line).trim();
        current = official.iter().copied().find(|kind| *kind == heading);
        if let Some(kind) = current {
            if !choices.contains(&kind.to_owned()) {
                choices.push(kind.to_owned());
            }
        }
    }
    (choices, used)
}

/// The text after the quota label of an indented inventory line, in either language.
fn quota_note(line: &str) -> Option<&str> {
    line.strip_prefix("cota:").or_else(|| line.strip_prefix("quota:")).map(str::trim_start)
}

fn quota_used_from_note(note: &str) -> Option<f32> {
    let groups: Vec<&str> = note.split(',').map(str::trim).collect();
    let has_gemini = groups.iter().any(|group| group.to_ascii_lowercase().contains("gemini"));
    groups
        .iter()
        .filter(|group| !has_gemini || group.to_ascii_lowercase().contains("gemini"))
        .filter_map(|group| {
            let upper = group.to_ascii_uppercase();
            if upper.contains("ESGOTADA") || upper.contains("EXHAUSTED") {
                return Some(1.0);
            }
            group.split_whitespace().find_map(|part| {
                let percent = part.strip_suffix('%')?.parse::<f32>().ok()?;
                Some((percent / 100.0).clamp(0.0, 1.0))
            })
        })
        .max_by(f32::total_cmp)
}

/// `claude@dev (sonnet)` when the instance has a model.
fn with_model(id: &str, models: &BTreeMap<String, String>) -> String {
    match models.get(id) {
        Some(m) => format!("{id} ({m})"),
        None => id.to_owned(),
    }
}

/// The `models` map (id → model) of a bridge response, with the quota note (`quota`) of whoever is out or
/// near the end: "gemini-4-argon-high; Gemini 4 Argon quota exhausted, renews in 9 h".
fn models_of(json: &Value) -> BTreeMap<String, String> {
    let map = |key: &str| -> BTreeMap<String, String> {
        json.get(key)
            .and_then(Value::as_object)
            .map(|o| o.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned()))).collect())
            .unwrap_or_default()
    };
    let quota = map("quota");
    let mut models = map("models");
    for (id, model) in models.iter_mut() {
        if let Some(note) = quota.get(id) {
            *model = format!("{model}; {note}");
        }
    }
    models
}

/// `GET /team` response.
struct Team {
    id: String,
    boss: Option<String>,
    crew: Vec<String>,
    /// Model of whoever appears above (id → model).
    models: BTreeMap<String, String>,
}

/// `GET /team`: me, boss, crew and the models.
fn team(bridge: &Bridge) -> Result<Team, String> {
    let (status, json) = bridge.request("GET", "/team", None, Some(Duration::from_secs(10)))?;
    if status != 200 {
        return Err(json.get("error").and_then(Value::as_str).unwrap_or("unknown error").to_owned());
    }
    let id = json.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
    let boss = json.get("boss").and_then(Value::as_str).map(str::to_owned);
    let crew = json.get("crew").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect());
    Ok(Team { id, boss, crew: crew.unwrap_or_default(), models: models_of(&json) })
}

// ---------- Learnings (`--learn`) ----------

const LEARN_USAGE: &str = "Usage: ask-agent --learn [<file>...]   (what the project has already learned, in general or about those files)";

/// `ask-agent --learn [<path>...]`: prints the learnings (`memory/learnings/`) that apply to the files;
/// with no paths, the global ones and the index of the others.
fn learn_command(bridge: &Bridge, paths: &[String]) -> ExitCode {
    if paths.iter().any(|p| p == "-h" || p == "--help") {
        println!("{LEARN_USAGE}");
        return ExitCode::SUCCESS;
    }
    match bridge.request("POST", "/learn", Some(&json!({ "paths": paths })), Some(Duration::from_secs(15))) {
        Ok((200, json)) => {
            println!("{}", json.get("text").and_then(Value::as_str).unwrap_or(""));
            ExitCode::SUCCESS
        }
        Ok((_, json)) => fail(json.get("error").and_then(Value::as_str).unwrap_or("unknown error"), 1),
        Err(e) => fail(e, 1),
    }
}

// ---------- Outline of a file (`--outline`) ----------

const OUTLINE_USAGE: &str =
    "Usage: ask-agent --outline <file>...   (each file's signatures with the line number, without the bodies: read only the part you need)";

/// `ask-agent --outline <path>...`: prints the skeleton of each file (items with signature and line).
fn outline_command(bridge: &Bridge, paths: &[String]) -> ExitCode {
    if paths.iter().any(|p| p == "-h" || p == "--help") {
        println!("{OUTLINE_USAGE}");
        return ExitCode::SUCCESS;
    }
    if paths.is_empty() {
        return fail(OUTLINE_USAGE, 2);
    }
    match bridge.request("POST", "/outline", Some(&json!({ "paths": paths })), Some(Duration::from_secs(30))) {
        Ok((200, json)) => {
            println!("{}", json.get("text").and_then(Value::as_str).unwrap_or(""));
            ExitCode::SUCCESS
        }
        Ok((_, json)) => fail(json.get("error").and_then(Value::as_str).unwrap_or("unknown error"), 1),
        Err(e) => fail(e, 1),
    }
}

// ---------- Inbox (`--inbox`) ----------

const INBOX_USAGE: &str =
    "Usage: ask-agent --inbox [--all]   (the answers to your questions that were not delivered yet and the mail you have not read; --all: the last 20)";

/// How long ago `ts_ms` was (epoch in ms), in short text.
fn age_text(ts_ms: u64, now_ms: u64) -> String {
    let secs = now_ms.saturating_sub(ts_ms) / 1000;
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86_399 => format!("{} h ago", secs / 3600),
        _ => format!("{} d ago", secs / 86_400),
    }
}

/// An inbox answer, as text.
fn inbox_entry_text(entry: &Value, now_ms: u64) -> String {
    let text = |k: &str| entry.get(k).and_then(Value::as_str).unwrap_or("");
    let request = entry.get("request").and_then(Value::as_u64).unwrap_or(0);
    let failed = entry.get("failed").and_then(Value::as_bool).unwrap_or(false);
    let delivered = entry.get("delivered").and_then(Value::as_bool).unwrap_or(false);
    let kind = if failed { "No answer" } else { "Answer" };
    let age = age_text(entry.get("ts").and_then(Value::as_u64).unwrap_or(0), now_ms);
    let state = if delivered { "already delivered in the terminal" } else { "not delivered" };
    format!("[{kind} from the agent \"{}\" to your question #{request}, {age}, {state}]\n\n{}", text("from"), text("text"))
}

/// A mail in the `--inbox` listing, on one line.
fn mail_line(mail: &Value, now_ms: u64) -> String {
    let n = mail.get("mail").and_then(Value::as_u64).unwrap_or(0);
    let text = |k: &str| mail.get(k).and_then(Value::as_str).unwrap_or("");
    let age = age_text(mail.get("ts").and_then(Value::as_u64).unwrap_or(0), now_ms);
    let state = if mail.get("read").and_then(Value::as_bool).unwrap_or(false) { "read" } else { "not read" };
    format!("[Mail #{n} from \"{}\", {age}, {state}] \"{}\" — read with: ask-agent --read {n}", text("from"), text("preview"))
}

/// `ask-agent --inbox [--all]`: prints the answers to this agent's questions that have not reached it yet
/// (or the last 20, with `--all`), then the mail it has not read. The printed answers become delivered.
fn inbox_command(bridge: &Bridge, flags: &[String]) -> ExitCode {
    if flags.iter().any(|f| f == "-h" || f == "--help") {
        println!("{INBOX_USAGE}");
        return ExitCode::SUCCESS;
    }
    if let Some(bad) = flags.iter().find(|f| *f != "--all") {
        return fail(format!("unknown option \"{bad}\".\n{INBOX_USAGE}"), 2);
    }
    let all = !flags.is_empty();
    let route = if all { "/inbox?all=1" } else { "/inbox" };
    match bridge.request("GET", route, None, Some(Duration::from_secs(15))) {
        Ok((200, json)) => {
            let list = |k: &str| json.get(k).and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
            let (answers, mail) = (list("answers"), list("mail"));
            if answers.is_empty() && mail.is_empty() {
                println!("{}", if all { "The inbox is empty." } else { "No pending answer or mail." });
                return ExitCode::SUCCESS;
            }
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
            let mut parts: Vec<String> = answers.iter().map(|e| inbox_entry_text(e, now)).collect();
            if !mail.is_empty() {
                parts.push(mail.iter().map(|m| mail_line(m, now)).collect::<Vec<_>>().join("\n"));
            }
            println!("{}", parts.join("\n\n---\n\n"));
            ExitCode::SUCCESS
        }
        Ok((_, json)) => fail(json.get("error").and_then(Value::as_str).unwrap_or("unknown error"), 1),
        Err(e) => fail(e, 1),
    }
}

// ---------- Mail (`--read`) ----------

const READ_USAGE: &str = "Usage: ask-agent --read <n>   (the full text of mail #n: a long message, or the late answer to a question, that Holt did not type into your terminal; only its addressee can read it)";

/// Mail `mail` as the reader sees it: the header, the text and what to do next.
fn mail_text(mail: &Value, now_ms: u64) -> String {
    let text = |k: &str| mail.get(k).and_then(Value::as_str).unwrap_or("");
    let n = mail.get("mail").and_then(Value::as_u64).unwrap_or(0);
    let request = mail.get("request").and_then(Value::as_u64).unwrap_or(0);
    let from = text("from");
    let age = age_text(mail.get("ts").and_then(Value::as_u64).unwrap_or(0), now_ms);
    let next = if text("kind") == "answer" {
        "Continue the task based on this answer.".to_owned()
    } else if mail.get("answerable").and_then(Value::as_bool).unwrap_or(false) {
        format!(
            "Answer by running in the shell: ask-agent --reply {request} \"your answer\" (the answer only reaches \"{from}\" through that command; \
for long answers or ones with accents, write them to a file and use: ask-agent --reply {request} --file <file>)."
        )
    } else {
        format!(
            "Question #{request} is no longer waiting for an answer (it was answered, cancelled, expired or Holt was restarted). If a reply \
is still useful, send it as a new message: ask-agent {from} \"message\"."
        )
    };
    let header = if text("kind") == "answer" {
        format!("[Mail #{n}: answer from the agent \"{from}\" to your question #{request}, {age}]")
    } else {
        format!("[Mail #{n} from \"{from}\", {age}]")
    };
    format!("{header}\n\n{}\n\n{next}", text("text"))
}

/// `ask-agent --read <n>`: prints mail #n (and marks it as read).
fn read_command(bridge: &Bridge, args: &[String]) -> ExitCode {
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{READ_USAGE}");
        return ExitCode::SUCCESS;
    }
    let [number] = args else {
        return fail(format!("--read needs the mail number.\n{READ_USAGE}"), 2);
    };
    let Ok(n) = number.trim_start_matches('#').parse::<u64>() else {
        return fail(format!("\"{number}\" is not a mail number.\n{READ_USAGE}"), 2);
    };
    match bridge.request("GET", &format!("/mail/{n}"), None, Some(Duration::from_secs(30))) {
        Ok((200, json)) => {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
            println!("{}", mail_text(&json, now));
            ExitCode::SUCCESS
        }
        Ok((_, json)) => fail(json.get("error").and_then(Value::as_str).unwrap_or("unknown error"), 1),
        Err(e) => fail(e, 1),
    }
}

// ---------- Tasks (`--task`) ----------

const TASK_USAGE: &str = "Usage: ask-agent --task new \"title\" [--agent <model>] [--model <m>] [--effort low|medium|high] [--level simple|medium|complex] [--base <branch>] [--start] -|\"request\"|--file <file> [--scope <path> [<path>...]]   |   ask-agent --task list   |   ask-agent --task show <id>   |   ask-agent --task start|merge|validate|discard <id>   |   ask-agent --task done <id> -|\"report\"|--file <file>   |   ask-agent --task return|reject <id> \"feedback\"|--file <file>|-   |   --scope lists the paths (files or folders, relative to the repository root, no wildcards) the task will touch and lasts until the next option: put the request before it or use --file; a task whose scope overlaps another one in progress waits in the queue and starts by itself (memory/ and Cargo.lock do not count)";

/// Where a long text (request, report, feedback) comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Body {
    /// Written on the command line.
    Inline(String),
    /// `-`: standard input.
    Stdin,
    /// `--file <path>`: read from the file (UTF-8, or UTF-16 with BOM).
    File(String),
    /// Not given: standard input counts if it is a pipe/file.
    Missing,
}

/// A `--task` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TaskCmd {
    New {
        title: String,
        brief: Body,
        agent: Option<String>,
        model: Option<String>,
        effort: Option<String>,
        level: Option<String>,
        base: Option<String>,
        start: bool,
        scope: Vec<String>,
    },
    List,
    Show(String),
    Start(String),
    Merge(String),
    Validate(String),
    Discard(String),
    Done {
        id: String,
        report: Body,
    },
    Return {
        id: String,
        feedback: Body,
    },
    Reject {
        id: String,
        feedback: Body,
    },
}

/// A lone `-` = standard input; `--file <path>` = file; text = command line; nothing = `Missing`.
fn body_of(words: &[String]) -> Result<Body, String> {
    match words {
        [] => Ok(Body::Missing),
        [dash] if dash == "-" => Ok(Body::Stdin),
        [flag, path] if flag == "--file" => Ok(Body::File(path.clone())),
        [one] if one.starts_with("--file=") => Ok(Body::File(one["--file=".len()..].to_owned())),
        [flag, ..] if flag == "--file" => Err("--file needs the file path.".into()),
        words => Ok(Body::Inline(words.join(" "))),
    }
}

/// Takes a `--file <path>` (or `--file=<path>`) out of the command line and returns the path.
/// It applies to the generic message (`ask-agent <agent> --file <file>`, `--reply ... --file <file>`).
fn take_body_file(args: &mut Vec<String>) -> Result<Option<String>, String> {
    let mut path = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--file" {
            let value = args.get(i + 1).cloned().filter(|v| !v.is_empty() && !v.starts_with("--"));
            path = Some(value.ok_or("--file needs the file path.")?);
            args.drain(i..=i + 1);
        } else if let Some(p) = args[i].strip_prefix("--file=") {
            path = Some(p.to_owned());
            args.remove(i);
        } else {
            i += 1;
        }
    }
    Ok(path)
}

/// The task id (`t3`, `3`) the way the bridge expects it.
fn task_id(arg: Option<&String>) -> Result<String, String> {
    let arg = arg.ok_or("missing the task id (e.g. t3).")?;
    holt_core::tasks::parse_task_id(arg).ok_or_else(|| format!("invalid task id: \"{arg}\" (use t3 or 3)."))
}

/// Reads what comes after `--task`.
fn parse_task(args: &[String]) -> Result<TaskCmd, String> {
    let Some(sub) = args.first().map(String::as_str) else { return Err("missing the --task subcommand.".into()) };
    let rest = &args[1..];
    match sub {
        "list" | "ls" => match rest {
            [] => Ok(TaskCmd::List),
            _ => Err("--task list takes no arguments.".into()),
        },
        "show" | "start" | "merge" | "validate" | "discard" => {
            if rest.len() > 1 {
                return Err(format!("--task {sub} takes only the task id."));
            }
            let id = task_id(rest.first())?;
            Ok(match sub {
                "show" => TaskCmd::Show(id),
                "start" => TaskCmd::Start(id),
                "merge" => TaskCmd::Merge(id),
                "validate" => TaskCmd::Validate(id),
                _ => TaskCmd::Discard(id),
            })
        }
        "done" => Ok(TaskCmd::Done { id: task_id(rest.first())?, report: body_of(&rest[1..])? }),
        "return" => Ok(TaskCmd::Return { id: task_id(rest.first())?, feedback: body_of(&rest[1..])? }),
        "reject" => Ok(TaskCmd::Reject { id: task_id(rest.first())?, feedback: body_of(&rest[1..])? }),
        "new" => {
            let (mut agent, mut model, mut effort, mut level, mut base, mut start) = (None, None, None, None, None, false);
            let mut brief_file: Option<String> = None;
            let mut scope: Vec<String> = Vec::new();
            let mut words: Vec<String> = Vec::new();
            let mut it = rest.iter().peekable();
            while let Some(a) = it.next() {
                // `--scope` takes all paths up to the next option (or a `-`, the request's standard input).
                if a == "--scope" || a.starts_with("--scope=") {
                    let before = scope.len();
                    scope.extend(a.strip_prefix("--scope=").filter(|p| !p.is_empty()).map(str::to_owned));
                    if a == "--scope" {
                        while let Some(p) = it.next_if(|n| !n.starts_with("--") && n.as_str() != "-") {
                            scope.push(p.clone());
                        }
                    }
                    if scope.len() == before {
                        return Err("--scope needs at least one path.".into());
                    }
                    continue;
                }
                let mut value = |name: &str, inline: Option<&str>| -> Result<String, String> {
                    let v = match inline {
                        Some(v) => Some(v.to_owned()),
                        None => it.next().cloned(),
                    };
                    v.filter(|v| !v.is_empty() && !v.starts_with("--")).ok_or_else(|| format!("{name} needs a value."))
                };
                match a.as_str() {
                    "--start" => start = true,
                    "--agent" => agent = Some(value("--agent", None)?),
                    "--model" => model = Some(value("--model", None)?),
                    "--effort" => effort = Some(value("--effort", None)?),
                    "--level" => level = Some(value("--level", None)?),
                    "--base" => base = Some(value("--base", None)?),
                    "--file" => brief_file = Some(value("--file", None)?),
                    s if s.starts_with("--agent=") => agent = Some(value("--agent", Some(&s["--agent=".len()..]))?),
                    s if s.starts_with("--model=") => model = Some(value("--model", Some(&s["--model=".len()..]))?),
                    s if s.starts_with("--effort=") => effort = Some(value("--effort", Some(&s["--effort=".len()..]))?),
                    s if s.starts_with("--level=") => level = Some(value("--level", Some(&s["--level=".len()..]))?),
                    s if s.starts_with("--base=") => base = Some(value("--base", Some(&s["--base=".len()..]))?),
                    s if s.starts_with("--file=") => brief_file = Some(value("--file", Some(&s["--file=".len()..]))?),
                    s if s.starts_with("--") && s.len() > 2 => return Err(format!("unknown option of --task new: {s}")),
                    _ => words.push(a.clone()),
                }
            }
            if words.is_empty() || words[0].trim().is_empty() || words[0] == "-" {
                return Err("missing the task title: ask-agent --task new \"title\" ... -".into());
            }
            let title = words.remove(0);
            let brief = match brief_file {
                Some(path) => Body::File(path),
                None => body_of(&words)?,
            };
            Ok(TaskCmd::New { title, brief, agent, model, effort, level, base, start, scope })
        }
        other => Err(format!("unknown --task subcommand: {other}")),
    }
}

/// The text of a `Body`: the command line, standard input or a file. With nothing and in a terminal,
/// an error (reading would hang without a notice).
fn read_body(body: Body, what: &str) -> Result<String, String> {
    let text = match body {
        Body::Inline(text) => text,
        Body::Stdin => read_stdin()?,
        Body::File(path) => read_text_file(&path)?,
        Body::Missing if !std::io::stdin().is_terminal() => read_stdin()?,
        Body::Missing => {
            return Err(format!("missing {what}: pass the text as an argument, through standard input (-) or in --file <file>."))
        }
    };
    Ok(text)
}

fn read_stdin() -> Result<String, String> {
    let mut raw = Vec::new();
    std::io::stdin().read_to_end(&mut raw).map_err(|e| format!("failed to read stdin: {e}"))?;
    Ok(decode_text(&raw))
}

/// Reads a file as text (UTF-8 or UTF-16 with BOM; see `decode_text`). It is the path that keeps the
/// accents: the agent's shell (PowerShell) would turn them into `?` when piping through `stdin`.
fn read_text_file(path: &str) -> Result<String, String> {
    let raw = std::fs::read(path).map_err(|e| format!("failed to read the file {path}: {e}"))?;
    Ok(decode_text(&raw))
}

/// The text of a task field in the bridge response.
fn field<'a>(task: &'a Value, key: &str) -> &'a str {
    task.get(key).and_then(Value::as_str).unwrap_or("")
}

fn status_label(status: &str) -> &str {
    match status {
        "draft" => "draft",
        "queued" => "queued",
        "running" => "running",
        "review" => "review",
        "validating" => "validating",
        "merged" => "done",
        "discarded" => "discarded",
        "failed" => "error",
        other => other,
    }
}

/// `claude@t3 (sonnet, effort high)` or just the instance.
fn who(task: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(m) = task.get("model").and_then(Value::as_str) {
        parts.push(m.to_owned());
    }
    if let Some(e) = task.get("effort").and_then(Value::as_str) {
        parts.push(format!("effort {e}"));
    }
    match parts.is_empty() {
        true => field(task, "instance").to_owned(),
        false => format!("{} ({})", field(task, "instance"), parts.join(", ")),
    }
}

/// The task's level (`medium` when the server does not send it; it is the CLI value).
fn level_text(task: &Value) -> &str {
    match field(task, "level") {
        "" => "medium",
        level => level,
    }
}

/// One `--task list` line: id, state, agent with model and title.
fn task_line(task: &Value) -> String {
    let waiting = match field(task, "waiting_for") {
        "" => String::new(),
        id => format!(" (waits for {id})"),
    };
    format!("{:<5} {:<10} {:<26} {}{waiting}", field(task, "id"), status_label(field(task, "status")), who(task), field(task, "title"))
}

/// The paths of a list of texts in the JSON (`scope`, `outside_scope`), separated by commas.
fn paths_of(task: &Value, key: &str) -> String {
    let list = task.get(key).and_then(Value::as_array);
    list.map(|l| l.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default()
}

/// `--task show`: everything the person (or the agent) needs to review it.
fn task_detail(task: &Value) -> String {
    let mut out = format!("Task {}: {}\n", field(task, "id"), field(task, "title"));
    out.push_str(&format!("State: {}\n", status_label(field(task, "status"))));
    if let Some(id) = task.get("waiting_for").and_then(Value::as_str) {
        out.push_str(&format!("Queued: waits for task {id} (the scope overlaps); it starts by itself when that one leaves\n"));
    }
    out.push_str(&format!("Agent: {} · level: {}\n", who(task), level_text(task)));
    let scope = paths_of(task, "scope");
    if !scope.is_empty() {
        out.push_str(&format!("Scope: {scope}\n"));
    }
    let outside = paths_of(task, "outside_scope");
    if !outside.is_empty() {
        out.push_str(&format!("Warning: touched files outside the scope: {outside}\n"));
    }
    out.push_str(&format!("Base: {} · branch: {}\n", field(task, "base"), field(task, "branch")));
    let wt = field(task, "worktree");
    if !wt.is_empty() {
        out.push_str(&format!("Worktree: {wt}\n"));
    }
    if let Some(c) = task.get("creator").and_then(Value::as_str) {
        out.push_str(&format!("Created by: {c}\n"));
    }
    if let Some(e) = task.get("error").and_then(Value::as_str) {
        out.push_str(&format!("Error: {e}\n"));
    }
    if let Some(stat) = task.get("diff_stat").and_then(Value::as_str) {
        out.push_str(&format!("\nChanges (git diff --stat {}...{}):\n{stat}\n", field(task, "base"), field(task, "branch")));
    }
    if let Some(report) = task.get("report").and_then(Value::as_str) {
        out.push_str(&format!("\nReport:\n{report}\n"));
    }
    if let Some(events) = task.get("events").and_then(Value::as_array) {
        out.push_str("\nHistory:\n");
        let skip = events.len().saturating_sub(10);
        for e in events.iter().skip(skip) {
            out.push_str(&format!("  - {}\n", field(e, "text")));
        }
    }
    out
}

/// `ask-agent --task ...`: talks to the bridge's `/tasks`.
fn task_command(bridge: &Bridge, args: &[String]) -> ExitCode {
    let cmd = match parse_task(args) {
        Ok(c) => c,
        Err(e) => return fail(format!("{e}\n{TASK_USAGE}"), 2),
    };
    // Body already read (standard input before talking to the bridge) and the HTTP request.
    let (method, route, body) = match cmd {
        TaskCmd::List => ("GET", "/tasks".to_owned(), None),
        TaskCmd::Show(id) => ("GET", format!("/tasks/{id}"), None),
        TaskCmd::Start(id) => ("POST", format!("/tasks/{id}/start"), Some(json!({}))),
        TaskCmd::Merge(id) => ("POST", format!("/tasks/{id}/merge"), Some(json!({}))),
        TaskCmd::Validate(id) => ("POST", format!("/tasks/{id}/validate"), Some(json!({}))),
        TaskCmd::Discard(id) => ("POST", format!("/tasks/{id}/discard"), Some(json!({}))),
        TaskCmd::Done { id, report } => match read_body(report, "the report") {
            Ok(r) if r.trim().is_empty() => return fail("empty report: say what was done, the files, the tests and what is missing.", 2),
            Ok(r) => ("POST", format!("/tasks/{id}/done"), Some(json!({ "report": r }))),
            Err(e) => return fail(e, 2),
        },
        TaskCmd::Return { id, feedback } => match read_body(feedback, "the feedback") {
            Ok(f) if f.trim().is_empty() => return fail("empty feedback: say what has to change.", 2),
            Ok(f) => ("POST", format!("/tasks/{id}/return"), Some(json!({ "feedback": f }))),
            Err(e) => return fail(e, 2),
        },
        TaskCmd::Reject { id, feedback } => match read_body(feedback, "the reason") {
            Ok(f) if f.trim().is_empty() => return fail("empty reason: say what did not work in the test.", 2),
            Ok(f) => ("POST", format!("/tasks/{id}/reject"), Some(json!({ "feedback": f }))),
            Err(e) => return fail(e, 2),
        },
        TaskCmd::New { title, brief, agent, model, effort, level, base, start, scope } => match read_body(brief, "the task request") {
            Ok(b) => {
                let mut body = json!({ "title": title, "brief": b, "start": start });
                if !scope.is_empty() {
                    body["scope"] = json!(scope);
                }
                for (key, value) in [("kind", agent), ("model", model), ("effort", effort), ("level", level), ("base", base)] {
                    if let Some(v) = value {
                        body[key] = json!(v);
                    }
                }
                ("POST", "/tasks".to_owned(), Some(body))
            }
            Err(e) => return fail(e, 2),
        },
    };
    let new = route == "/tasks" && method == "POST";
    let verb = route.rsplit('/').next().unwrap_or("").to_owned();
    // Starting or merging runs git (it may take a while): the limit is generous.
    let (status, json) = match bridge.request(method, &route, body.as_ref(), Some(Duration::from_secs(600))) {
        Ok(v) => v,
        Err(e) => return fail(e, 1),
    };
    if status != 200 {
        return fail(json.get("error").and_then(Value::as_str).unwrap_or("unknown error"), 1);
    }
    if let Some(list) = json.get("tasks").and_then(Value::as_array) {
        if list.is_empty() {
            println!("No tasks in this project.");
        }
        for task in list {
            println!("{}", task_line(task));
        }
        return ExitCode::SUCCESS;
    }
    let task = json.get("task").cloned().unwrap_or(Value::Null);
    let id = field(&task, "id").to_owned();
    match (new, verb.as_str()) {
        (true, _) => {
            println!("Task {id} created: {}", field(&task, "title"));
            println!(
                "  agent: {} · level: {} · base: {} · branch: {}",
                who(&task),
                level_text(&task),
                field(&task, "base"),
                field(&task, "branch")
            );
            if let Some(events) = task.get("events").and_then(Value::as_array) {
                for event in events {
                    if let Some(text) = event.get("text").and_then(Value::as_str) {
                        // Event texts are written by the tasks area, which may be in either language.
                        let lowered = text.to_lowercase();
                        if lowered.starts_with("agente escolhido automaticamente")
                            || lowered.starts_with("agent chosen automatically")
                            || lowered.starts_with("aviso de cota:")
                            || lowered.starts_with("quota warning:")
                            || lowered.starts_with("aviso de capacidade:")
                            || lowered.starts_with("capacity warning:")
                        {
                            println!("  {text}");
                        }
                    }
                }
            }
            let scope = paths_of(&task, "scope");
            if !scope.is_empty() {
                println!("  scope: {scope}");
            }
            if field(&task, "status") == "queued" {
                println!(
                    "  queued: the scope overlaps that of {}; it starts by itself when that one is merged, discarded or fails",
                    field(&task, "waiting_for")
                );
            } else if field(&task, "status") == "running" {
                println!("  started in the worktree {}", field(&task, "worktree"));
                println!(
                    "  the agent reports with: ask-agent --task done {id} --file <file>  (the report is in: ask-agent --task show {id})"
                );
            } else {
                println!("  start it with: ask-agent --task start {id}");
            }
        }
        (_, "start") if field(&task, "status") == "queued" => println!(
            "Task {id} queued: the scope overlaps that of {}; it starts by itself when that one is merged, discarded or fails.",
            field(&task, "waiting_for")
        ),
        (_, "start") => println!("Task {id} started: {} works in the worktree {}", who(&task), field(&task, "worktree")),
        (_, "merge") => {
            println!(
                "Task {id} merged into {}; waiting for validation (the agent, the worktree and the branch stay).",
                field(&task, "base")
            )
        }
        (_, "validate") => println!("Task {id} validated and done; worktree and branch removed."),
        (_, "reject") => println!("Task {id} rejected; {} received the reason and goes back to work.", field(&task, "instance")),
        (_, "discard") => println!("Task {id} discarded; worktree and branch removed."),
        (_, "done") => println!("Report of task {id} delivered; it is in review."),
        (_, "return") => println!("Task {id} sent back to {} with your feedback.", field(&task, "instance")),
        _ => print!("{}", task_detail(&task)),
    }
    ExitCode::SUCCESS
}

// ---------- Claude Code statusline (`--usage`) ----------

/// `--usage`: reads the JSON Claude Code passes to the statusline, sends the plan usage to the bridge
/// (`POST /usage`) and prints the line: the user's own statusline, if they have one (our `--settings`
/// replaces it), or "session 34% · week 12%". Without data, it prints nothing.
fn statusline(bridge: Option<Bridge>) -> ExitCode {
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let status: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
    let usage = from_claude_statusline(&status);
    // The request runs alongside the user's statusline.
    let post = bridge.filter(|_| usage.session.is_some() || usage.weekly.is_some()).map(|bridge| {
        let body = usage_to_json(&usage);
        std::thread::spawn(move || {
            let _ = bridge.request("POST", "/usage", Some(&body), Some(USAGE_POST_TIMEOUT));
        })
    });
    let line = user_statusline(&status).and_then(|cmd| run_user_statusline(&cmd, &input)).unwrap_or_else(|| usage_line(&usage));
    if let Some(post) = post {
        let _ = post.join();
    }
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(line.as_bytes());
    let _ = out.flush();
    ExitCode::SUCCESS
}

/// "session 34% · week 12%", with a line break (empty without data).
fn usage_line(u: &AgentUsage) -> String {
    let pct = |w: Option<holt_core::UsageWindow>| w.map(|w| format!("{:.0}%", w.used * 100.0));
    let parts: Vec<String> =
        [("session", pct(u.session)), ("week", pct(u.weekly))].into_iter().filter_map(|(k, v)| Some(format!("{k} {}", v?))).collect();
    if parts.is_empty() {
        String::new()
    } else {
        parts.join(" · ") + "\n"
    }
}

/// The user's statusline command, in Claude Code's order of precedence: local project, project and user
/// (`CLAUDE_CONFIG_DIR` or `~/.claude`). Our own is ignored.
fn user_statusline(status: &Value) -> Option<String> {
    let project = ["project_dir", "current_dir"]
        .iter()
        .find_map(|k| status.get("workspace")?.get(*k)?.as_str().filter(|s| !s.is_empty()).map(PathBuf::from))
        .or_else(|| std::env::current_dir().ok());
    let config_dir = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| holt_core::util::home_dir().join(".claude"));
    let mut files: Vec<PathBuf> = Vec::new();
    if let Some(p) = &project {
        files.push(p.join(".claude").join("settings.local.json"));
        files.push(p.join(".claude").join("settings.json"));
    }
    files.push(config_dir.join("settings.json"));
    files.iter().find_map(|f| statusline_in(f))
}

fn statusline_in(file: &Path) -> Option<String> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()?;
    let sl = v.get("statusLine")?;
    if sl.get("type").and_then(Value::as_str).is_some_and(|t| t != "command") {
        return None;
    }
    let cmd = sl.get("command")?.as_str()?.trim();
    (!cmd.is_empty() && !cmd.contains("ask-agent --usage")).then(|| cmd.to_owned())
}

/// Runs the user's statusline with the same stdin; `None` if it does not run or takes too long.
fn run_user_statusline(cmd: &str, input: &str) -> Option<String> {
    let mut child = shell(cmd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        let input = input.to_owned();
        std::thread::spawn(move || {
            let _ = stdin.write_all(input.as_bytes());
        });
    }
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        let _ = tx.send(out);
    });
    match rx.recv_timeout(USER_STATUSLINE_TIMEOUT) {
        Ok(out) => {
            let _ = child.wait();
            Some(String::from_utf8_lossy(&out).into_owned())
        }
        Err(_) => {
            let _ = child.kill();
            None
        }
    }
}

/// The shell Claude Code runs the statusline in: `sh` on Unix; on Windows, Git Bash
/// (`CLAUDE_CODE_GIT_BASH_PATH` or the `bash.exe` on PATH, without the WSL one in `System32`), or `cmd`.
fn shell(cmd: &str) -> Command {
    if cfg!(windows) {
        let windir = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let bash = std::env::var_os("CLAUDE_CODE_GIT_BASH_PATH").filter(|v| !v.is_empty()).map(PathBuf::from).or_else(|| {
            std::env::split_paths(&std::env::var_os("PATH")?)
                .map(|d| d.join("bash.exe"))
                .find(|p| p.is_file() && !p.to_string_lossy().to_lowercase().starts_with(&windir.to_string_lossy().to_lowercase()))
        });
        match bash {
            Some(bash) => {
                let mut c = Command::new(bash);
                c.args(["-c", cmd]);
                c
            }
            None => cmd_shell(cmd),
        }
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", cmd]);
        c
    }
}

/// `cmd /D /S /C "<command>"` with the raw line: Rust's quoting would turn `"` into `\"`, which `cmd`
/// does not understand. With `/S`, `cmd` strips only the outer quotes.
#[cfg(windows)]
fn cmd_shell(cmd: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut c = Command::new("cmd");
    c.raw_arg(format!("/D /S /C \"{cmd}\""));
    c
}

#[cfg(not(windows))]
fn cmd_shell(cmd: &str) -> Command {
    let mut c = Command::new("cmd");
    c.args(["/D", "/S", "/C", cmd]);
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_session_signal_carries_the_conversation_id_from_the_argument_or_the_hook_json() {
        let no_stdin = || panic!("stdin must not be read");
        assert_eq!(signal_body("busy", None, no_stdin), json!({"state": "busy"}));
        assert_eq!(signal_body("idle", Some("{\"type\":\"x\"}"), no_stdin), json!({"state": "idle"}));
        // Codex's `notify`: the JSON it appends carries the thread id `codex resume` takes (stdin is never read for it).
        let notify = r#"{"type":"agent-turn-complete","thread-id":"01a187e4-5d25-7c61-9416-9143d2b8a5e5","turn-id":"t","cwd":"C:\\p","client":"codex-tui"}"#;
        assert_eq!(
            signal_body("idle", Some(notify), no_stdin),
            json!({"state": "idle", "session": "01a187e4-5d25-7c61-9416-9143d2b8a5e5"})
        );
        assert_eq!(signal_body("idle", Some(r#"{"thread-id":"a b;c"}"#), no_stdin), json!({"state": "idle"}));
        assert_eq!(signal_body("idle", None, no_stdin), json!({"state": "idle"}));
        assert_eq!(signal_body("session", Some("ses_abc"), no_stdin), json!({"state": "session", "session": "ses_abc"}));
        // Claude's SessionStart hook: the JSON arrives on stdin.
        let hook =
            || r#"{"session_id":"429e5864-ad7e-46ed-9f76-b280602c8ff5","hook_event_name":"SessionStart","source":"clear"}"#.to_owned();
        assert_eq!(signal_body("session", None, hook), json!({"state": "session", "session": "429e5864-ad7e-46ed-9f76-b280602c8ff5"}));
        assert_eq!(signal_body("session", Some(r#"{"session_id":"x1"}"#), no_stdin)["session"], "x1");
        // Nothing usable: an empty id (the bridge refuses it).
        assert_eq!(signal_body("session", None, || "not json".to_owned())["session"], "");
    }

    #[test]
    fn mail_reads_with_how_to_go_on() {
        let question = json!({"mail": 12, "from": "claude@dev", "kind": "question", "request": 7, "ts": 0, "text": "line 1\nline 2", "answerable": true});
        let text = mail_text(&question, 120_000);
        assert!(text.starts_with("[Mail #12 from \"claude@dev\", 2 min ago]\n\nline 1\nline 2\n\n"), "{text}");
        assert!(text.contains("ask-agent --reply 7 \"your answer\"") && text.contains("--file <file>"), "{text}");
        let closed = json!({"mail": 12, "from": "claude@dev", "kind": "question", "request": 7, "ts": 0, "text": "x", "answerable": false});
        let text = mail_text(&closed, 0);
        assert!(text.contains("no longer waiting") && text.contains("ask-agent claude@dev \"message\""), "{text}");
        let answer = json!({"mail": 3, "from": "codex", "kind": "answer", "request": 2, "ts": 0, "text": "done", "answerable": false});
        let text = mail_text(&answer, 0);
        assert!(text.starts_with("[Mail #3: answer from the agent \"codex\" to your question #2, just now]\n\ndone\n\n"), "{text}");
        assert!(text.ends_with("Continue the task based on this answer."), "{text}");
        let line = mail_line(&json!({"mail": 4, "from": "a", "preview": "hello…", "ts": 0, "read": false}), 3_600_000);
        assert_eq!(line, "[Mail #4 from \"a\", 1 h ago, not read] \"hello…\" — read with: ask-agent --read 4");
    }

    #[test]
    fn inbox_entries_read_as_a_message_with_age_and_state() {
        let entry = json!({"request": 4, "from": "antigravity", "text": "ready", "failed": false, "ts": 1_000_000, "delivered": false});
        let text = inbox_entry_text(&entry, 1_000_000 + 5 * 60_000);
        assert_eq!(
            text,
            "[Answer from the agent \"antigravity\" to your question #4, 5 min ago, not delivered]

ready"
        );
        let failed = json!({"request": 5, "from": "claude@dev", "text": "did not answer", "failed": true, "ts": 0, "delivered": true});
        assert!(inbox_entry_text(&failed, 3 * 3_600_000)
            .starts_with("[No answer from the agent \"claude@dev\" to your question #5, 3 h ago, already delivered in the terminal]"));
        assert_eq!(age_text(10_000, 20_000), "just now");
    }

    type Parsed = (Result<(bool, Option<String>), String>, Vec<String>);

    fn flags(args: &[&str]) -> Parsed {
        let mut v: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        (take_flags(&mut v), v)
    }

    fn rest(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    fn words(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    fn task(args: &[&str]) -> Result<TaskCmd, String> {
        parse_task(&words(args))
    }

    #[test]
    fn task_new_forms_are_parsed() {
        let new = |title: &str, brief: Body, agent: Option<&str>, model: Option<&str>, base: Option<&str>, start: bool| TaskCmd::New {
            title: title.into(),
            brief,
            agent: agent.map(String::from),
            model: model.map(String::from),
            effort: None,
            level: None,
            base: base.map(String::from),
            start,
            scope: Vec::new(),
        };
        assert_eq!(task(&["new", "Fix login", "-"]), Ok(new("Fix login", Body::Stdin, None, None, None, false)));
        assert_eq!(task(&["new", "Fix login"]), Ok(new("Fix login", Body::Missing, None, None, None, false)));
        assert_eq!(task(&["new", "T", "Do", "this", "now"]), Ok(new("T", Body::Inline("Do this now".into()), None, None, None, false)));
        assert_eq!(
            task(&["new", "T", "--agent", "opencode", "--model", "opencode/glm", "--base", "dev", "--start", "-"]),
            Ok(new("T", Body::Stdin, Some("opencode"), Some("opencode/glm"), Some("dev"), true))
        );
        // The options work in any position and with `=`.
        assert_eq!(
            task(&["new", "--start", "--agent=claude", "T", "--model=sonnet", "--base=main", "request"]),
            Ok(new("T", Body::Inline("request".into()), Some("claude"), Some("sonnet"), Some("main"), true))
        );
        // The request comes from a file (it keeps the accents): `--file <file>` or `--file=<file>`.
        assert_eq!(task(&["new", "T", "--file", "request.md"]), Ok(new("T", Body::File("request.md".into()), None, None, None, false)));
        assert_eq!(
            task(&["new", "T", "--start", "--file=request.md"]),
            Ok(new("T", Body::File("request.md".into()), None, None, None, true))
        );
        // The effort works in any position, with `=`; the bridge checks the value.
        let TaskCmd::New { effort, .. } = task(&["new", "T", "--effort", "high", "request"]).expect("effort") else { panic!("new") };
        assert_eq!(effort.as_deref(), Some("high"));
        let TaskCmd::New { effort, .. } = task(&["new", "--effort=low", "T", "-"]).expect("effort=") else { panic!("new") };
        assert_eq!(effort.as_deref(), Some("low"));
        let TaskCmd::New { level, .. } = task(&["new", "T", "--level", "complex", "request"]).expect("level") else { panic!("new") };
        assert_eq!(level.as_deref(), Some("complex"));
        let TaskCmd::New { level, .. } = task(&["new", "--level=simple", "T", "-"]).expect("level=") else { panic!("new") };
        assert_eq!(level.as_deref(), Some("simple"));
        let TaskCmd::New { level, .. } = task(&["new", "T", "-"]).expect("no level") else { panic!("new") };
        assert_eq!(level, None);
        // `--scope` takes the paths up to the next option or a `-`; it may repeat and works with `=`.
        let scope_of = |args: &[&str]| match task(args).expect("scope") {
            TaskCmd::New { scope, brief, .. } => (scope, brief),
            other => panic!("{other:?}"),
        };
        assert_eq!(
            scope_of(&["new", "T", "request", "--scope", "crates/core", "crates/cli/src/lib.rs", "--start"]),
            (vec!["crates/core".to_owned(), "crates/cli/src/lib.rs".to_owned()], Body::Inline("request".into()))
        );
        assert_eq!(scope_of(&["new", "T", "--scope", "a", "b", "-"]), (vec!["a".to_owned(), "b".to_owned()], Body::Stdin));
        assert_eq!(
            scope_of(&["new", "--scope=a", "T", "--scope", "b", "--file", "p.md"]),
            (vec!["a".to_owned(), "b".to_owned()], Body::File("p.md".into()))
        );
        assert!(scope_of(&["new", "T", "-"]).0.is_empty());
        for bad in [
            &["new"][..],
            &["new", "T", "--scope"],
            &["new", "T", "--scope", "--start"],
            &["new", "T", "--scope="],
            &["new", "T", "--effort"],
            &["new", "T", "--effort", "--start"],
            &["new", "-"],
            &["new", "  "],
            &["new", "T", "--agent"],
            &["new", "T", "--model", "--start"],
            &["new", "T", "--base="],
            &["new", "T", "--agentx", "x"],
            &["new", "T", "--file"],
            &["new", "T", "--file="],
        ] {
            assert!(task(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn task_other_forms_are_parsed() {
        assert_eq!(task(&["list"]), Ok(TaskCmd::List));
        assert_eq!(task(&["ls"]), Ok(TaskCmd::List));
        assert_eq!(task(&["show", "t3"]), Ok(TaskCmd::Show("t3".into())));
        assert_eq!(task(&["show", "3"]), Ok(TaskCmd::Show("t3".into())));
        assert_eq!(task(&["start", "T4"]), Ok(TaskCmd::Start("t4".into())));
        assert_eq!(task(&["merge", "t5"]), Ok(TaskCmd::Merge("t5".into())));
        assert_eq!(task(&["validate", "5"]), Ok(TaskCmd::Validate("t5".into())));
        assert_eq!(task(&["discard", "t6"]), Ok(TaskCmd::Discard("t6".into())));
        assert_eq!(task(&["done", "t3", "-"]), Ok(TaskCmd::Done { id: "t3".into(), report: Body::Stdin }));
        assert_eq!(task(&["done", "t3"]), Ok(TaskCmd::Done { id: "t3".into(), report: Body::Missing }));
        assert_eq!(task(&["done", "t3", "all", "done"]), Ok(TaskCmd::Done { id: "t3".into(), report: Body::Inline("all done".into()) }));
        assert_eq!(
            task(&["return", "t3", "The test is missing."]),
            Ok(TaskCmd::Return { id: "t3".into(), feedback: Body::Inline("The test is missing.".into()) })
        );
        assert_eq!(task(&["return", "t3", "-"]), Ok(TaskCmd::Return { id: "t3".into(), feedback: Body::Stdin }));
        assert_eq!(
            task(&["reject", "t3", "It", "does not save."]),
            Ok(TaskCmd::Reject { id: "t3".into(), feedback: Body::Inline("It does not save.".into()) })
        );
        assert_eq!(task(&["reject", "t3", "-"]), Ok(TaskCmd::Reject { id: "t3".into(), feedback: Body::Stdin }));
        assert_eq!(task(&["reject", "t3", "--file", "m.md"]), Ok(TaskCmd::Reject { id: "t3".into(), feedback: Body::File("m.md".into()) }));
        // Report/feedback from a file (it keeps the accents).
        assert_eq!(task(&["done", "t3", "--file", "r.md"]), Ok(TaskCmd::Done { id: "t3".into(), report: Body::File("r.md".into()) }));
        assert_eq!(task(&["done", "t3", "--file=r.md"]), Ok(TaskCmd::Done { id: "t3".into(), report: Body::File("r.md".into()) }));
        assert_eq!(task(&["return", "t3", "--file", "f.md"]), Ok(TaskCmd::Return { id: "t3".into(), feedback: Body::File("f.md".into()) }));
        for bad in [
            &[][..],
            &["explode"],
            &["list", "x"],
            &["show"],
            &["show", "claude@t3"],
            &["start", "t1", "t2"],
            &["done"],
            &["done", "t3", "--file"],
            &["done", "xyz", "-"],
            &["return"],
            &["reject"],
            &["validate"],
            &["validate", "t1", "t2"],
        ] {
            assert!(task(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn task_output_is_readable() {
        let t = json!({
            "id": "t3", "title": "Fix login", "status": "review", "base": "main", "branch": "task/t3-fix-login",
            "instance": "claude@t3", "model": "sonnet", "creator": "claude", "worktree": "/r/.holt/worktrees/t3",
            "diff_stat": " a.rs | 2 +-", "report": "Done.", "events": [{ "at_ms": 1, "text": "created" }, { "at_ms": 2, "text": "started" }]
        });
        assert_eq!(task_line(&t), "t3    review     claude@t3 (sonnet)         Fix login");
        assert_eq!(who(&json!({ "instance": "claude@t3", "model": "sonnet", "effort": "high" })), "claude@t3 (sonnet, effort high)");
        assert_eq!(who(&json!({ "instance": "claude@t3", "effort": "low" })), "claude@t3 (effort low)");
        let d = task_detail(&t);
        for needle in [
            "Task t3: Fix login",
            "State: review",
            "Agent: claude@t3 (sonnet)",
            "Base: main · branch: task/t3-fix-login",
            "Created by: claude",
            "a.rs | 2 +-",
            "Report:\nDone.",
            "  - started",
        ] {
            assert!(d.contains(needle), "{needle}\n{d}");
        }
        let draft = json!({ "id": "t4", "title": "T", "status": "draft", "instance": "opencode@t4" });
        assert_eq!(task_line(&draft), "t4    draft      opencode@t4                T");
        let queued = json!({
            "id": "t5", "title": "U", "status": "queued", "instance": "claude@t5", "waiting_for": "t3",
            "scope": ["crates/core", "crates/cli"], "outside_scope": ["crates/app/x.rs"]
        });
        assert_eq!(task_line(&queued), "t5    queued     claude@t5                  U (waits for t3)");
        let d = task_detail(&queued);
        for needle in ["State: queued", "Queued: waits for task t3", "Scope: crates/core, crates/cli", "outside the scope: crates/app/x.rs"]
        {
            assert!(
                d.contains(needle),
                "{needle}
{d}"
            );
        }
        assert!(!task_detail(&draft).contains("Scope:"));
    }

    #[test]
    fn model_flag_is_parsed_in_any_order_before_the_target() {
        assert_eq!(flags(&["claude@dev", "hi"]), (Ok((false, None)), rest(&["claude@dev", "hi"])));
        assert_eq!(flags(&["--new", "claude", "hi"]).0, Ok((true, None)));
        assert_eq!(flags(&["--model", "sonnet", "claude@dev", "hi"]), (Ok((false, Some("sonnet".into()))), rest(&["claude@dev", "hi"])));
        assert_eq!(flags(&["--new", "--model", "opencode/glm-5.3", "opencode", "hi"]).0, Ok((true, Some("opencode/glm-5.3".into()))));
        assert_eq!(flags(&["--model", "opus", "--new", "claude", "hi"]).0, Ok((true, Some("opus".into()))));
        assert_eq!(flags(&["--model=haiku", "claude", "hi"]), (Ok((false, Some("haiku".into()))), rest(&["claude", "hi"])));
        // What comes after the target is a message, even if it looks like an option.
        assert_eq!(flags(&["claude", "--model", "x"]), (Ok((false, None)), rest(&["claude", "--model", "x"])));
    }

    #[test]
    fn model_flag_needs_a_name() {
        for bad in [&["--model"][..], &["--model", "--new", "claude"], &["--model", "", "claude"], &["--model="]] {
            assert!(flags(bad).0.is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_message_body_can_come_from_a_file() {
        for (args, path, left) in [
            (&["claude", "hi"][..], None, &["claude", "hi"][..]),
            (&["claude", "--file", "m.md"], Some("m.md"), &["claude"]),
            (&["--reply", "3", "--file=m.md"], Some("m.md"), &["--reply", "3"]),
        ] {
            let mut v = words(args);
            assert_eq!(take_body_file(&mut v), Ok(path.map(String::from)), "{args:?}");
            assert_eq!(v, words(left));
        }
        for bad in [&["claude", "--file"][..], &["claude", "--file", "--new"]] {
            let mut v = words(bad);
            assert!(take_body_file(&mut v).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn models_show_next_to_the_instances() {
        let json = json!({ "agents": ["claude", "claude@dev"], "models": { "claude@dev": "sonnet", "x": 1 } });
        let models = models_of(&json);
        assert_eq!(with_model("claude@dev", &models), "claude@dev (sonnet)");
        assert_eq!(with_model("claude", &models), "claude");
        assert!(models_of(&json!({})).is_empty());
        // The quota note goes next to the model.
        let json = json!({ "models": { "antigravity@arquiteto": "gemini-4-argon-high" }, "quota": { "antigravity@arquiteto": "quota exhausted" } });
        assert_eq!(with_model("antigravity@arquiteto", &models_of(&json)), "antigravity@arquiteto (gemini-4-argon-high; quota exhausted)");
    }

    #[test]
    fn quota_inventory_uses_gemini_groups_and_excludes_opencode() {
        // The inventory is written by another area: both spellings are read (`cota:`/`quota:`, `ESGOTADA`/`EXHAUSTED`).
        let inventory = "claude --model: opus\n  cota: 74% usada\nantigravity --model: gemini-4-argon-high\n  quota: Gemini Models 45% usada, Gemini 4 Argon 48% usada, Claude and GPT models 98% usada\ncodex --model <nome> (este computador não lista os modelos)\n  cota: sem leitura\nopencode --model <provedor>/<modelo>: \n  cota: 0% usada";
        let (choices, used) = quota_usage_from_inventory(inventory);
        assert_eq!(choices, ["claude", "antigravity", "codex"]);
        assert_eq!(used.get("claude"), Some(&0.74));
        assert_eq!(used.get("antigravity"), Some(&0.48));
        assert!(!used.contains_key("codex"));
        assert!(!choices.iter().any(|kind| kind == "opencode"));
        // The exhausted note, in either language, counts as full.
        assert_eq!(quota_used_from_note("Gemini 4 Argon ESGOTADA (renova em 9 h)"), Some(1.0));
        assert_eq!(quota_used_from_note("Gemini 4 Argon EXHAUSTED (renews in 9 h)"), Some(1.0));
    }
}
