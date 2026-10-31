//! Codex's chat backend: `codex app-server`, the JSON-RPC protocol the IDE extensions speak, over stdio.
//!
//! One long-lived process per tab. Holt sends `initialize`, then `thread/start` (or `thread/resume` with the saved
//! thread id), then one `turn/start` per message; Codex answers with notifications (`item/agentMessage/delta`,
//! `item/started`, `item/completed`, `turn/completed`...) and, when it needs the user, with *server requests* that
//! Holt answers (`item/commandExecution/requestApproval`, `item/fileChange/requestApproval`...). The recordings the
//! tests read are in `tests/data/codex-appserver-*.jsonl`; `memory/learnings/codex-app-server-protocol.md` has what
//! was checked and why this protocol and not `codex exec --json`.
//!
//! How the items become the common chat events:
//!
//! | Codex item | chat |
//! |---|---|
//! | `agentMessage` (+ `item/agentMessage/delta`) | assistant text, streamed |
//! | `commandExecution` | a `Bash` call, its output as the result |
//! | `fileChange` | one `Edit` call per file, with the diff; the result carries the patch hunks |
//! | `mcpToolCall`, `webSearch`, `dynamicToolCall` | a call named after the tool |
//! | `reasoning` | a thinking block (only when Codex sends its text) |
//!
//! Approvals use the existing permission card (`Bash` for a command, `Edit` for a patch, `Permissions` for extra
//! sandbox permissions). Holt does not choose the approval policy or the sandbox: the user's Codex configuration
//! decides (by default `on-request` with the `workspace-write` sandbox in a trusted folder), so a card appears when Codex
//! itself asks.

use std::collections::{HashMap, VecDeque};
use std::path::Path;

use serde_json::{json, Value};

use super::backend::{Capability, ChatBackend, Interrupt, Parsed, Protocol};
use super::{
    clip, str_of, strings, Block, ChatEvent, Item, PatchHunk, PermissionRequest, ToolOutcome, Transcript, TurnResult, RESULT_MAX_CHARS,
};
use crate::agents::SpawnSpec;
use crate::models::Effort;
use crate::t;

pub(super) struct CodexBackend;

impl ChatBackend for CodexBackend {
    fn kind(&self) -> &'static str {
        "codex"
    }

    fn capability(&self) -> Capability {
        Capability::Chat
    }

    fn leading_args(&self) -> Vec<String> {
        vec!["app-server".into()]
    }

    /// `app-server` talks JSON-RPC on stdio by default: no flag switches it.
    fn mode_args(&self) -> Vec<String> {
        Vec::new()
    }

    /// `-c model=<name>`: the value is TOML and falls back to the raw text when it is not (a model name is not).
    fn model_args(&self, model: &str) -> Vec<String> {
        vec!["-c".into(), format!("model={model}")]
    }

    fn effort_args(&self, effort: Effort) -> Vec<String> {
        vec!["-c".into(), format!("model_reasoning_effort={}", effort.as_str())]
    }

    fn resumes_by_args(&self) -> bool {
        false
    }

    fn protocol(&self, spec: &SpawnSpec) -> Box<dyn Protocol> {
        Box::new(CodexProtocol::new(spec.session.clone(), &spec.cwd))
    }

    /// The history comes from the process (`thread/resume` returns the turns): nothing to read here but whether the
    /// conversation exists, so the chat waits for it instead of showing an empty view.
    fn history(&self, home: &Path, session: &str) -> Option<Transcript> {
        crate::resume::codex_has_session(&crate::resume::codex_home(home), session)
            .then(|| Transcript { history_pending: true, ..Transcript::default() })
    }
}

/// What a request Holt sent is waiting for.
#[derive(Debug, Clone, Copy)]
enum Awaiting {
    Initialize,
    /// `thread/start` or `thread/resume`; `true` = a resume.
    Thread(bool),
    Turn,
    Interrupt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApprovalKind {
    Command,
    FileChange,
    Permissions,
}

struct Approval {
    /// The id of the server request, as Codex wrote it (a number or a string): the answer carries it back.
    rpc_id: Value,
    kind: ApprovalKind,
    /// `Permissions` only: what was asked, granted back as it is.
    permissions: Value,
}

/// One file of a `fileChange` item.
#[derive(Debug, Clone)]
struct Change {
    path: String,
    /// A unified diff of the file (see [`unified_diff`]).
    diff: String,
}

pub(super) struct CodexProtocol {
    resume: Option<String>,
    cwd: String,
    next_id: u64,
    awaiting: HashMap<u64, Awaiting>,
    thread: Option<String>,
    turn: Option<String>,
    /// A `turn/start` went out and its `turn/completed` has not come: Codex steers a message sent meanwhile into the same
    /// turn (one `turn/completed` for both), so the next message waits in `queue` to keep one result per message.
    turn_busy: bool,
    interrupt_asked: bool,
    queue: VecDeque<String>,
    approvals: HashMap<String, Approval>,
    edits: HashMap<String, Vec<Change>>,
    context_tokens: u64,
    context_window: u64,
    last_error: String,
}

impl CodexProtocol {
    pub(super) fn new(resume: Option<String>, cwd: &Path) -> Self {
        Self {
            resume,
            cwd: cwd.to_string_lossy().into_owned(),
            next_id: 0,
            awaiting: HashMap::new(),
            thread: None,
            turn: None,
            turn_busy: false,
            interrupt_asked: false,
            queue: VecDeque::new(),
            approvals: HashMap::new(),
            edits: HashMap::new(),
            context_tokens: 0,
            context_window: 0,
            last_error: String::new(),
        }
    }

    fn request(&mut self, method: &str, params: Value, awaiting: Awaiting) -> String {
        self.next_id += 1;
        self.awaiting.insert(self.next_id, awaiting);
        json!({ "id": self.next_id, "method": method, "params": params }).to_string()
    }

    fn thread_request(&mut self, resume: Option<String>) -> String {
        match resume {
            Some(id) => self.request("thread/resume", json!({ "threadId": id, "cwd": self.cwd }), Awaiting::Thread(true)),
            None => self.request("thread/start", json!({ "cwd": self.cwd }), Awaiting::Thread(false)),
        }
    }

    /// Starts the next waiting message, if the thread is ready and no turn is running.
    fn pump(&mut self) -> Vec<String> {
        let Some(thread) = self.thread.clone() else { return Vec::new() };
        if self.turn_busy {
            return Vec::new();
        }
        let Some(text) = self.queue.pop_front() else { return Vec::new() };
        self.turn_busy = true;
        self.interrupt_asked = false;
        let params = json!({ "threadId": thread, "input": [{ "type": "text", "text": text, "text_elements": [] }] });
        vec![self.request("turn/start", params, Awaiting::Turn)]
    }

    fn interrupt_line(&mut self) -> Option<String> {
        let (thread, turn) = (self.thread.clone()?, self.turn.clone()?);
        Some(self.request("turn/interrupt", json!({ "threadId": thread, "turnId": turn }), Awaiting::Interrupt))
    }

    // ----- Responses to Holt's requests -----

    fn response(&mut self, id: u64, v: &Value) -> Parsed {
        let Some(awaiting) = self.awaiting.remove(&id) else { return Parsed::default() };
        let error = v.get("error").map(|e| str_of(e, "message").to_owned());
        let result = v.get("result").unwrap_or(&Value::Null);
        let mut out = Parsed::default();
        match awaiting {
            Awaiting::Initialize => match error {
                Some(msg) => out.events.push(ChatEvent::Notice { text: msg, error: true }),
                None => {
                    out.reply.push(json!({ "method": "initialized" }).to_string());
                    let resume = self.resume.take();
                    let line = self.thread_request(resume);
                    out.reply.push(line);
                }
            },
            Awaiting::Thread(resume) => match error {
                // The conversation is gone (or refused): open a new one instead of a dead tab, and say so.
                Some(msg) if resume => {
                    out.events.push(ChatEvent::Notice {
                        text: t!("Could not resume the Codex conversation ({reason}). A new one was started.", reason = msg),
                        error: false,
                    });
                    let line = self.thread_request(None);
                    out.reply.push(line);
                }
                Some(msg) => {
                    out.events.push(ChatEvent::Notice { text: msg.clone(), error: true });
                    // Nothing will answer the messages that waited: end each one as a failed turn.
                    for _ in self.queue.drain(..) {
                        out.events.push(ChatEvent::Result(TurnResult { is_error: true, error: msg.clone(), ..TurnResult::default() }));
                    }
                }
                None => {
                    let thread = result.get("thread").unwrap_or(&Value::Null);
                    let session = str_of(thread, "id").to_owned();
                    self.thread = Some(session.clone()).filter(|s| !s.is_empty());
                    out.events.push(ChatEvent::Init {
                        session,
                        model: str_of(result, "model").to_owned(),
                        commands: Vec::new(),
                        terminal_commands: Vec::new(),
                    });
                    if resume {
                        out.events.push(ChatEvent::Replay(replay_items(thread.get("turns"))));
                    }
                    out.reply = self.pump();
                }
            },
            Awaiting::Turn => match error {
                Some(msg) => {
                    self.turn_busy = false;
                    out.events.push(ChatEvent::Result(TurnResult { is_error: true, error: msg, ..TurnResult::default() }));
                    out.reply = self.pump();
                }
                None => {
                    if self.turn.is_none() {
                        self.turn = result.get("turn").map(|t| str_of(t, "id").to_owned()).filter(|s| !s.is_empty());
                    }
                    if std::mem::take(&mut self.interrupt_asked) {
                        out.reply.extend(self.interrupt_line());
                    }
                }
            },
            Awaiting::Interrupt => {}
        }
        out
    }

    // ----- Requests from Codex -----

    fn server_request(&mut self, id: &Value, method: &str, params: &Value) -> Parsed {
        let request_id = match id {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let item = str_of(params, "itemId").to_owned();
        let reason = params.get("reason").and_then(Value::as_str).unwrap_or("").to_owned();
        let (kind, tool, input, suggestions) = match method {
            "item/commandExecution/requestApproval" => {
                let command = match params.get("command").and_then(Value::as_str) {
                    Some(c) => display_command(c),
                    None => match params.get("networkApprovalContext") {
                        Some(ctx) => t!("Network access: {host}", host = str_of(ctx, "host")),
                        None => String::new(),
                    },
                };
                // The decisions Codex offers for this request; "for this session" only when it lists it.
                let session =
                    params.get("availableDecisions").and_then(Value::as_array).is_none_or(|d| d.iter().any(|x| x == "acceptForSession"));
                let suggestions = if session { vec![json!("acceptForSession")] } else { Vec::new() };
                (ApprovalKind::Command, "Bash", json!({ "command": command, "cwd": str_of(params, "cwd") }), suggestions)
            }
            "item/fileChange/requestApproval" => {
                let changes = self.edits.get(&item).cloned().unwrap_or_default();
                (ApprovalKind::FileChange, "Edit", edit_input(&changes), vec![json!("acceptForSession")])
            }
            "item/permissions/requestApproval" => {
                let permissions = params.get("permissions").cloned().unwrap_or(Value::Null);
                self.approvals.insert(
                    request_id.clone(),
                    Approval { rpc_id: id.clone(), kind: ApprovalKind::Permissions, permissions: permissions.clone() },
                );
                let event = ChatEvent::Permission(PermissionRequest {
                    request_id,
                    tool_use_id: item,
                    tool: "Permissions".into(),
                    input: json!({ "reason": reason, "permissions": permissions }),
                    description: reason,
                    suggestions: vec![json!("session")],
                });
                return Parsed::events(vec![event]);
            }
            // Questions to the user, login refreshes, dynamic tools...: Holt has no screen for them. An error answer lets the
            // turn go on (the tool fails) instead of waiting forever.
            _ => {
                let line = json!({ "id": id, "error": { "code": -32601, "message": "not supported by Holt" } }).to_string();
                return Parsed { events: Vec::new(), reply: vec![line] };
            }
        };
        self.approvals.insert(request_id.clone(), Approval { rpc_id: id.clone(), kind, permissions: Value::Null });
        Parsed::events(vec![ChatEvent::Permission(PermissionRequest {
            request_id,
            tool_use_id: item,
            tool: tool.into(),
            input,
            description: reason,
            suggestions,
        })])
    }

    // ----- Notifications -----

    fn notification(&mut self, method: &str, p: &Value) -> Parsed {
        let mut out = Parsed::default();
        match method {
            "turn/started" => {
                if let Some(turn) = p.get("turn") {
                    self.turn = Some(str_of(turn, "id").to_owned()).filter(|s| !s.is_empty());
                }
                self.turn_busy = true;
            }
            "turn/completed" => {
                let turn = p.get("turn").unwrap_or(&Value::Null);
                let status = str_of(turn, "status");
                self.turn = None;
                self.turn_busy = false;
                self.interrupt_asked = false;
                if status == "interrupted" {
                    out.events.push(ChatEvent::Interrupted);
                }
                let failed = status == "failed";
                let error = if failed {
                    let msg = turn.get("error").map(|e| str_of(e, "message")).filter(|m| !m.is_empty()).unwrap_or(&self.last_error);
                    if msg.is_empty() {
                        t!("The turn failed.").to_owned()
                    } else {
                        msg.to_owned()
                    }
                } else {
                    String::new()
                };
                self.last_error.clear();
                out.events.push(ChatEvent::Result(TurnResult {
                    is_error: failed,
                    error,
                    cost_usd: 0.0,
                    duration_ms: turn.get("durationMs").and_then(Value::as_u64).unwrap_or(0),
                    session: self.thread.clone().unwrap_or_default(),
                    context_window: self.context_window,
                }));
                out.reply = self.pump();
            }
            "item/agentMessage/delta" => {
                out.events.push(ChatEvent::TextDelta { message: str_of(p, "itemId").to_owned(), text: str_of(p, "delta").to_owned() });
            }
            "item/started" | "item/completed" => {
                if let Some(item) = p.get("item") {
                    out.events = self.item_events(item, method == "item/completed");
                }
            }
            "thread/tokenUsage/updated" => {
                let usage = p.get("tokenUsage").unwrap_or(&Value::Null);
                self.context_tokens =
                    usage.get("last").and_then(|l| l.get("inputTokens")).and_then(Value::as_u64).unwrap_or(self.context_tokens);
                self.context_window = usage.get("modelContextWindow").and_then(Value::as_u64).unwrap_or(self.context_window);
                out.events.push(ChatEvent::Assistant { message: String::new(), blocks: Vec::new(), context_tokens: self.context_tokens });
            }
            // Codex stops waiting for an answer (it was given, or the turn was cut): the card is no longer pending.
            "serverRequest/resolved" => {
                let id = match p.get("requestId") {
                    Some(Value::String(s)) => s.clone(),
                    Some(other) => other.to_string(),
                    None => String::new(),
                };
                self.approvals.remove(&id);
                out.events.push(ChatEvent::PermissionCancelled(id));
            }
            "error" => {
                if let Some(msg) = p.get("error").map(|e| str_of(e, "message")).filter(|m| !m.is_empty()) {
                    if !p.get("willRetry").and_then(Value::as_bool).unwrap_or(false) {
                        self.last_error = msg.to_owned();
                    }
                }
            }
            "warning" => {
                let message = str_of(p, "message");
                if !message.is_empty() {
                    out.events.push(ChatEvent::Notice { text: message.to_owned(), error: false });
                }
            }
            _ => {}
        }
        out
    }

    /// The chat events of one `item/started` or `item/completed`.
    fn item_events(&mut self, item: &Value, completed: bool) -> Vec<ChatEvent> {
        let id = str_of(item, "id").to_owned();
        let call = |id: &str, name: &str, input: Value| ChatEvent::Assistant {
            message: id.to_owned(),
            blocks: vec![Block::ToolUse { id: id.to_owned(), name: name.to_owned(), input }],
            context_tokens: 0,
        };
        let finished = |id: &str, text: String, is_error: bool, patch: Vec<PatchHunk>| {
            ChatEvent::ToolResults(vec![ToolOutcome { tool_use_id: id.to_owned(), text, is_error, patch }])
        };
        let mut events = Vec::new();
        match str_of(item, "type") {
            "agentMessage" if completed => {
                events.push(ChatEvent::Assistant {
                    message: id,
                    blocks: vec![Block::Text(str_of(item, "text").to_owned())],
                    context_tokens: 0,
                });
            }
            "reasoning" if completed => {
                let text = reasoning_text(item);
                if !text.is_empty() {
                    events.push(ChatEvent::Assistant { message: id, blocks: vec![Block::Thinking(text)], context_tokens: 0 });
                }
            }
            "commandExecution" => {
                events.push(call(&id, "Bash", json!({ "command": display_command(str_of(item, "command")) })));
                if completed {
                    let status = str_of(item, "status");
                    let code = item.get("exitCode").and_then(Value::as_i64);
                    let is_error = matches!(status, "failed" | "declined") || code.is_some_and(|c| c != 0);
                    let mut text = clip(str_of(item, "aggregatedOutput"), RESULT_MAX_CHARS);
                    if status == "declined" {
                        text = t!("Declined.").to_owned();
                    } else if text.trim().is_empty() && is_error {
                        text = code.map_or_else(String::new, |c| format!("exit code {c}"));
                    }
                    events.push(finished(&id, text, is_error, Vec::new()));
                }
            }
            "fileChange" => {
                let changes = changes_of(item);
                if !completed {
                    self.edits.insert(id.clone(), changes.clone());
                }
                let status = str_of(item, "status");
                for (n, change) in changes.iter().enumerate() {
                    let call_id = change_call_id(&id, n);
                    events.push(call(&call_id, "Edit", json!({ "file_path": change.path, "diff": change.diff })));
                    if completed {
                        let failed = matches!(status, "failed" | "declined");
                        let text = if status == "declined" { t!("Declined.").to_owned() } else { String::new() };
                        events.push(finished(&call_id, text, failed, patch_from_diff(&change.diff)));
                    }
                }
                if completed {
                    self.edits.remove(&id);
                }
            }
            "mcpToolCall" => {
                let name = format!("mcp__{}__{}", str_of(item, "server"), str_of(item, "tool"));
                events.push(call(&id, &name, item.get("arguments").cloned().unwrap_or(Value::Null)));
                if completed {
                    let error = item.get("error").map(|e| str_of(e, "message").to_owned()).filter(|m| !m.is_empty());
                    let is_error = error.is_some() || matches!(str_of(item, "status"), "failed" | "declined");
                    let text = error.unwrap_or_else(|| mcp_result_text(item.get("result")));
                    events.push(finished(&id, clip(&text, RESULT_MAX_CHARS), is_error, Vec::new()));
                }
            }
            "dynamicToolCall" => {
                events.push(call(&id, str_of(item, "tool"), item.get("arguments").cloned().unwrap_or(Value::Null)));
                if completed {
                    let text = item
                        .get("contentItems")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().map(|c| str_of(c, "text")).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n"))
                        .unwrap_or_default();
                    let is_error = item.get("success").and_then(Value::as_bool) == Some(false);
                    events.push(finished(&id, clip(&text, RESULT_MAX_CHARS), is_error, Vec::new()));
                }
            }
            "webSearch" => {
                let query = Some(str_of(item, "query"))
                    .filter(|q| !q.is_empty())
                    .or_else(|| item.get("action").and_then(|a| a.get("query")).and_then(Value::as_str));
                events.push(call(&id, "WebSearch", json!({ "query": query.unwrap_or("") })));
                if completed {
                    events.push(finished(&id, String::new(), false, Vec::new()));
                }
            }
            _ => {}
        }
        events
    }
}

impl Protocol for CodexProtocol {
    fn open(&mut self) -> Vec<String> {
        let params = json!({ "clientInfo": { "name": "holt", "title": "Holt", "version": env!("CARGO_PKG_VERSION") } });
        vec![self.request("initialize", params, Awaiting::Initialize)]
    }

    fn parse(&mut self, line: &str) -> Parsed {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { return Parsed::default() };
        let method = v.get("method").and_then(Value::as_str);
        let params = v.get("params").unwrap_or(&Value::Null);
        match (v.get("id"), method) {
            (Some(id), Some(method)) => self.server_request(id, method, params),
            (Some(id), None) => id.as_u64().map_or_else(Parsed::default, |id| self.response(id, &v)),
            (None, Some(method)) => self.notification(method, params),
            (None, None) => Parsed::default(),
        }
    }

    fn user_message(&mut self, text: &str) -> Vec<String> {
        self.queue.push_back(text.to_owned());
        self.pump()
    }

    fn answer_permission(&mut self, request_id: &str, allow: bool, _input: &Value, _deny_message: &str, rules: &[Value]) -> Vec<String> {
        let Some(approval) = self.approvals.remove(request_id) else { return Vec::new() };
        let result = match approval.kind {
            // `decline` lets the agent go on and tell the user it did not run; `cancel` would also cut the turn.
            ApprovalKind::Command | ApprovalKind::FileChange => {
                let decision = if allow { rules.first().cloned().unwrap_or_else(|| json!("accept")) } else { json!("decline") };
                json!({ "decision": decision })
            }
            ApprovalKind::Permissions if allow => {
                json!({ "permissions": approval.permissions, "scope": if rules.is_empty() { "turn" } else { "session" } })
            }
            ApprovalKind::Permissions => json!({ "permissions": {}, "scope": "turn" }),
        };
        vec![json!({ "id": approval.rpc_id, "result": result }).to_string()]
    }

    fn interrupt(&mut self) -> Interrupt {
        if self.turn.is_none() && self.turn_busy {
            // `turn/start` has not answered yet: the interrupt goes out with its answer.
            self.interrupt_asked = true;
        }
        Interrupt::Lines(self.interrupt_line().into_iter().collect())
    }
}

// ---------- Items ----------

fn change_call_id(item: &str, index: usize) -> String {
    if index == 0 {
        item.to_owned()
    } else {
        format!("{item}#{index}")
    }
}

fn changes_of(item: &Value) -> Vec<Change> {
    item.get("changes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|c| {
            let kind = c.get("kind").map(|k| str_of(k, "type")).unwrap_or("update");
            Change { path: str_of(c, "path").to_owned(), diff: unified_diff(kind, str_of(c, "diff")) }
        })
        .collect()
}

/// What the permission card of a patch shows: the file (and how many more) and the diff of every file.
fn edit_input(changes: &[Change]) -> Value {
    let first = changes.first().map(|c| c.path.as_str()).unwrap_or("");
    let path = if changes.len() > 1 { format!("{first} (+{} more)", changes.len() - 1) } else { first.to_owned() };
    json!({ "file_path": path, "diff": changes.iter().map(|c| c.diff.as_str()).collect::<Vec<_>>().join("\n") })
}

/// The diff of a `fileChange` as a unified diff: an update already is one; a new file carries its whole content and a
/// deleted one the content it had, which become one hunk of added or removed lines.
fn unified_diff(kind: &str, diff: &str) -> String {
    let lines: Vec<&str> = diff.lines().collect();
    match kind {
        "add" => format!("@@ -0,0 +1,{} @@\n{}", lines.len(), lines.iter().map(|l| format!("+{l}")).collect::<Vec<_>>().join("\n")),
        "delete" => format!("@@ -1,{} +0,0 @@\n{}", lines.len(), lines.iter().map(|l| format!("-{l}")).collect::<Vec<_>>().join("\n")),
        _ => diff.to_owned(),
    }
}

/// The hunks of a unified diff (`@@ -a,b +c,d @@` and the lines after it). Anything before the first hunk header (`diff
/// --git`, `---`, `+++`) and the `\ No newline` markers are skipped.
pub fn patch_from_diff(diff: &str) -> Vec<PatchHunk> {
    let mut hunks: Vec<PatchHunk> = Vec::new();
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("@@ ") {
            let start = |sign: char| -> u32 {
                rest.split_whitespace()
                    .find_map(|part| part.strip_prefix(sign))
                    .and_then(|r| r.split(',').next())
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(1)
            };
            hunks.push(PatchHunk { old_start: start('-'), new_start: start('+'), lines: Vec::new() });
        } else if let Some(hunk) = hunks.last_mut() {
            if matches!(line.chars().next(), Some(' ' | '+' | '-')) {
                hunk.lines.push(line.to_owned());
            } else if line.is_empty() {
                // A context line that lost its leading space.
                hunk.lines.push(" ".to_owned());
            }
        }
    }
    hunks
}

fn reasoning_text(item: &Value) -> String {
    let content = strings(item.get("content")).join("\n\n");
    if content.trim().is_empty() {
        strings(item.get("summary")).join("\n\n")
    } else {
        content
    }
}

fn mcp_result_text(result: Option<&Value>) -> String {
    let Some(result) = result else { return String::new() };
    result
        .get("content")
        .and_then(Value::as_array)
        .map(|parts| parts.iter().map(|p| str_of(p, "text")).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n"))
        .unwrap_or_default()
}

/// The command as the user would type it: Codex runs `/bin/bash -lc "<script>"` (or PowerShell's `-Command`), and
/// the card should show the script.
fn display_command(command: &str) -> String {
    let command = command.trim();
    let mut parts = command.splitn(3, ' ');
    let (Some(shell), Some(flag), Some(script)) = (parts.next(), parts.next(), parts.next()) else { return command.to_owned() };
    let name = shell.rsplit(['/', '\\']).next().unwrap_or(shell).to_lowercase();
    let name = name.strip_suffix(".exe").unwrap_or(&name);
    let runs_script = match name {
        "bash" | "sh" | "zsh" | "fish" | "dash" => matches!(flag, "-c" | "-lc" | "-ic" | "-ilc"),
        "powershell" | "pwsh" => flag.eq_ignore_ascii_case("-command"),
        _ => false,
    };
    if !runs_script {
        return command.to_owned();
    }
    let script = script.trim();
    for quote in ['"', '\''] {
        if script.len() >= 2 && script.starts_with(quote) && script.ends_with(quote) {
            let inner = &script[1..script.len() - 1];
            return if quote == '"' { inner.replace("\\\"", "\"") } else { inner.replace("'\\''", "'") };
        }
    }
    script.to_owned()
}

/// The conversation of a resumed thread (`thread/resume` returns its turns with their items) as chat items.
fn replay_items(turns: Option<&Value>) -> Vec<Item> {
    let mut proto = CodexProtocol::new(None, Path::new(""));
    let mut transcript = Transcript::default();
    for turn in turns.and_then(Value::as_array).into_iter().flatten() {
        for item in turn.get("items").and_then(Value::as_array).into_iter().flatten() {
            if str_of(item, "type") == "userMessage" {
                let text = item
                    .get("content")
                    .and_then(Value::as_array)
                    .map(|parts| {
                        parts
                            .iter()
                            .map(|p| if str_of(p, "type") == "text" { str_of(p, "text") } else { "[image]" })
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();
                if !text.trim().is_empty() {
                    transcript.push(Item::User { text, from: None });
                }
                continue;
            }
            for ev in proto.item_events(item, true) {
                transcript.apply(&ev);
            }
        }
    }
    transcript.items
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOOLS: &str = include_str!("../../tests/data/codex-appserver-tools.jsonl");
    const PATCH: &str = include_str!("../../tests/data/codex-appserver-patch.jsonl");
    const DECLINE: &str = include_str!("../../tests/data/codex-appserver-decline.jsonl");
    const INTERRUPT: &str = include_str!("../../tests/data/codex-appserver-interrupt.jsonl");
    const RESUME: &str = include_str!("../../tests/data/codex-appserver-resume.jsonl");

    /// What the interface would hold after the sample: the protocol is driven with what the sample's *server* said (the
    /// requests of the client were different runs of the same dialogue, so the responses are matched by id when
    /// the handshake is replayed by `feed`).
    fn feed(proto: &mut CodexProtocol, sample: &str) -> (Transcript, Vec<String>) {
        let mut t = Transcript::default();
        let mut replies = Vec::new();
        for line in sample.lines() {
            let parsed = proto.parse(line);
            replies.extend(parsed.reply);
            for ev in &parsed.events {
                t.apply(ev);
            }
        }
        (t, replies)
    }

    /// A protocol that already did the handshake (the recording's responses are matched by id).
    fn ready(resume: Option<&str>) -> CodexProtocol {
        let mut p = CodexProtocol::new(resume.map(str::to_owned), Path::new("/work/project"));
        let _ = p.open();
        p
    }

    fn user(p: &mut CodexProtocol, t: &mut Transcript, text: &str) -> Vec<String> {
        t.push_user(text, None);
        p.user_message(text)
    }

    fn requests(lines: &[String]) -> Vec<Value> {
        lines.iter().map(|l| serde_json::from_str(l).expect("json")).collect()
    }

    #[test]
    fn the_handshake_is_initialize_then_the_thread_then_the_waiting_message() {
        let mut p = ready(None);
        let mut t = Transcript::default();
        // A message typed before the thread exists waits.
        assert!(user(&mut p, &mut t, "hello").is_empty());
        let first = requests(&feed(&mut p, TOOLS.lines().next().expect("initialize response")).1);
        assert_eq!(first[0]["method"], "initialized");
        assert_eq!(first[1]["method"], "thread/start");
        assert_eq!(first[1]["params"]["cwd"], "/work/project");
        // The thread answer starts the turn with the waiting message.
        let thread_line = TOOLS.lines().find(|l| l.starts_with("{\"id\":2,")).expect("thread response");
        let parsed = p.parse(thread_line);
        assert!(
            matches!(&parsed.events[0], ChatEvent::Init { session, model, .. } if session == "01a18f37-5976-7bc1-9616-3ab2e181dab3" && model == "gpt-6-luna")
        );
        let turn = requests(&parsed.reply);
        assert_eq!(turn[0]["method"], "turn/start");
        assert_eq!(turn[0]["params"]["threadId"], "01a18f37-5976-7bc1-9616-3ab2e181dab3");
        assert_eq!(turn[0]["params"]["input"][0]["text"], "hello");
    }

    #[test]
    fn a_message_sent_during_a_turn_waits_for_its_end_so_each_one_has_a_result() {
        let mut p = ready(None);
        let mut t = Transcript::default();
        let _ = feed(&mut p, TOOLS.lines().next().unwrap());
        let _ = p.parse(TOOLS.lines().find(|l| l.starts_with("{\"id\":2,")).unwrap());
        assert_eq!(user(&mut p, &mut t, "one").len(), 1);
        assert!(user(&mut p, &mut t, "two").is_empty(), "Codex would steer it into the running turn");
        assert_eq!(t.items.len(), 2);
        let done = TOOLS.lines().find(|l| l.contains("\"method\":\"turn/completed\"")).unwrap();
        let parsed = p.parse(done);
        assert!(matches!(parsed.events.last(), Some(ChatEvent::Result(r)) if !r.is_error && r.duration_ms == 3645));
        let next = requests(&parsed.reply);
        assert_eq!(next[0]["params"]["input"][0]["text"], "two");
    }

    #[test]
    fn text_streams_and_the_command_becomes_a_bash_call_with_its_output() {
        let mut p = ready(None);
        let (t, _) = feed(&mut p, TOOLS);
        let tool = t.items.iter().find_map(|i| if let Item::Tool(c) = i { Some(c) } else { None }).expect("a tool call");
        assert_eq!(tool.name, "Bash");
        assert_eq!(tool.input["command"], "pwd && printf 'hi' > hello.txt && echo done", "the shell wrapper is not shown");
        let outcome = tool.outcome.as_ref().expect("a result");
        assert_eq!(outcome.text, "/work/project\ndone\n");
        assert!(!outcome.is_error);
        assert!(t.items.iter().any(
            |i| matches!(i, Item::Assistant { text, streaming: false, .. } if text == "Created `hello.txt` with `hi` and ran `echo done`.")
        ));
        assert_eq!(t.context_tokens, 13064);
        assert_eq!(t.context_window, 258400);
    }

    #[test]
    fn the_deltas_build_the_message_and_the_completed_item_has_the_last_word() {
        let mut p = ready(None);
        let mut t = Transcript::default();
        let lines: Vec<&str> = TOOLS.lines().filter(|l| l.contains("item/agentMessage/delta")).collect();
        assert!(lines.len() > 3);
        for l in &lines {
            for ev in p.parse(l).events {
                t.apply(&ev);
            }
        }
        assert!(matches!(t.items.last(), Some(Item::Assistant { streaming: true, text, .. }) if text.starts_with("Created")));
    }

    #[test]
    fn a_command_approval_is_a_permission_card_and_the_answer_goes_back_with_the_request_id() {
        let mut p = ready(None);
        let line = TOOLS.lines().find(|l| l.contains("item/commandExecution/requestApproval")).expect("approval");
        let parsed = p.parse(line);
        let ChatEvent::Permission(req) = &parsed.events[0] else { panic!("{:?}", parsed.events) };
        assert_eq!((req.request_id.as_str(), req.tool.as_str()), ("0", "Bash"));
        assert_eq!(req.tool_use_id, "exec-e858e65e-7668-47b9-8849-43988bdf9a4b");
        assert_eq!(req.input["command"], "pwd && printf 'hi' > hello.txt && echo done");
        // The recording lists no `acceptForSession` for this request: no "allow for this session" button.
        assert!(req.suggestions.is_empty());

        let allow = requests(&p.answer_permission("0", true, &req.input, "", &[]));
        assert_eq!(allow[0], json!({ "id": 0, "result": { "decision": "accept" } }));
        // Answered once: a second answer finds nothing.
        assert!(p.answer_permission("0", true, &req.input, "", &[]).is_empty());

        let parsed = p.parse(line);
        let deny = requests(&p.answer_permission("0", false, &req.input, "no", &[]));
        assert_eq!(deny[0], json!({ "id": 0, "result": { "decision": "decline" } }));
        assert!(matches!(parsed.events[0], ChatEvent::Permission(_)));
    }

    #[test]
    fn a_declined_command_ends_as_an_error_result_and_the_turn_goes_on() {
        let mut p = ready(None);
        let (t, _) = feed(&mut p, DECLINE);
        let tool = t.items.iter().find_map(|i| if let Item::Tool(c) = i { Some(c) } else { None }).expect("tool");
        assert_eq!(tool.input["command"], "rm hello.txt");
        let outcome = tool.outcome.as_ref().expect("result");
        assert!(outcome.is_error);
        assert!(t.items.iter().any(|i| matches!(i, Item::Assistant { text, .. } if text.contains("rejected"))));
        assert!(!t.waiting_for_permission(), "the card was resolved");
    }

    #[test]
    fn a_patch_is_one_edit_call_per_file_with_the_diff_before_and_after_the_approval() {
        let mut p = ready(None);
        let (t, _) = feed(&mut p, PATCH);
        let edits: Vec<&super::super::ToolCall> =
            t.items.iter().filter_map(|i| if let Item::Tool(c) = i { Some(c) } else { None }).collect();
        assert_eq!(edits.len(), 2);
        assert!(edits.iter().all(|c| c.name == "Edit" && c.input["file_path"] == "/work/project/notes.txt"));
        let created = edits[0].outcome.as_ref().expect("result").patch.clone();
        assert_eq!(created, vec![PatchHunk { old_start: 0, new_start: 1, lines: vec!["+one".into(), "+two".into(), "+three".into()] }]);
        let updated = edits[1].outcome.as_ref().expect("result").patch.clone();
        assert_eq!(
            updated,
            vec![PatchHunk { old_start: 1, new_start: 1, lines: vec![" one".into(), "-two".into(), "+TWO".into(), " three".into()] }]
        );
    }

    #[test]
    fn a_patch_approval_shows_the_diff_and_offers_the_session() {
        let mut p = ready(None);
        let mut cards = Vec::new();
        for line in PATCH.lines() {
            for ev in p.parse(line).events {
                if let ChatEvent::Permission(req) = ev {
                    cards.push(req);
                }
            }
        }
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].tool, "Edit");
        assert_eq!(cards[0].input["file_path"], "/work/project/notes.txt");
        assert_eq!(patch_from_diff(cards[1].input["diff"].as_str().unwrap())[0].lines[1], "-two");
        assert_eq!(cards[0].suggestions, vec![json!("acceptForSession")]);

        // When the approval request arrives, answering it with suggestions sends acceptForSession.
        let line = PATCH.lines().find(|l| l.contains("item/fileChange/requestApproval")).unwrap();
        let _ = p.parse(line);
        let answer = requests(&p.answer_permission(&cards[0].request_id, true, &cards[0].input, "", &cards[0].suggestions));
        assert_eq!(answer[0]["result"]["decision"], "acceptForSession");
    }

    #[test]
    fn an_interrupted_turn_says_so_and_ends_with_a_result() {
        let mut p = ready(None);
        let mut t = Transcript::default();
        let _ = feed(&mut p, INTERRUPT.lines().next().unwrap());
        let _ = p.parse(INTERRUPT.lines().find(|l| l.starts_with("{\"id\":2,")).unwrap());
        let sent = user(&mut p, &mut t, "Run the shell command: sleep 8; echo hi   and then say done.");
        assert_eq!(requests(&sent)[0]["method"], "turn/start");
        // Before the turn is known the interrupt waits for its id.
        assert!(matches!(p.interrupt(), Interrupt::Lines(l) if l.is_empty()));
        let (t2, replies) = feed(&mut p, INTERRUPT);
        assert!(requests(&replies)
            .iter()
            .any(|r| r["method"] == "turn/interrupt" && r["params"]["turnId"] == "01a18f3a-0035-7e22-844b-a0205b74295d"));
        assert!(t2.items.iter().any(|i| matches!(i, Item::Notice { text, .. } if text == "Interrupted.")));
        assert!(!t2.busy());
        // Once the turn is known the interrupt goes out at once.
        let mut p = ready(None);
        let _ = p.parse(INTERRUPT.lines().next().unwrap());
        let _ = p.parse(INTERRUPT.lines().find(|l| l.starts_with("{\"id\":2,")).unwrap());
        let _ = p.parse(INTERRUPT.lines().find(|l| l.contains("\"method\":\"turn/started\"")).unwrap());
        let Interrupt::Lines(lines) = p.interrupt() else { panic!("a request, not a signal") };
        assert_eq!(requests(&lines)[0]["params"]["threadId"], "01a18f3a-002a-7133-b357-01cbdd8434bc");
    }

    #[test]
    fn resuming_asks_for_the_thread_and_replays_what_was_said() {
        let mut p = ready(Some("01a18f37-5976-7bc1-9616-3ab2e181dab3"));
        let init = RESUME.lines().next().unwrap();
        let replies = requests(&p.parse(init).reply);
        assert_eq!(replies[1]["method"], "thread/resume");
        assert_eq!(replies[1]["params"]["threadId"], "01a18f37-5976-7bc1-9616-3ab2e181dab3");
        let thread = RESUME.lines().find(|l| l.starts_with("{\"id\":2,")).unwrap();
        let parsed = p.parse(thread);
        let ChatEvent::Replay(items) = parsed.events.iter().find(|e| matches!(e, ChatEvent::Replay(_))).expect("replay") else {
            unreachable!()
        };
        assert!(matches!(&items[0], Item::User { text, .. } if text.starts_with("Create a file hello.txt")));
        assert!(matches!(&items[1], Item::Tool(c) if c.name == "Bash" && c.outcome.is_some()));
        assert!(matches!(&items[2], Item::Assistant { text, .. } if text.starts_with("Created `hello.txt`")));

        // The chat that asked for the history puts it before what was typed meanwhile; a restart does not duplicate it.
        let mut t = Transcript { history_pending: true, ..Transcript::default() };
        t.push_user("typed while starting", None);
        t.apply(&parsed.events[1]);
        assert_eq!(t.items.len(), 4);
        assert!(matches!(&t.items[3], Item::User { text, .. } if text == "typed while starting"));
        t.apply(&parsed.events[1]);
        assert_eq!(t.items.len(), 4);
    }

    #[test]
    fn a_resume_that_fails_starts_a_new_thread_and_says_so() {
        let mut p = ready(Some("gone"));
        let _ = p.parse(RESUME.lines().next().unwrap());
        let failed = p.parse(r#"{"id":2,"error":{"code":-32600,"message":"no rollout found for thread id gone"}}"#);
        assert!(matches!(&failed.events[0], ChatEvent::Notice { text, error: false } if text.contains("no rollout found")));
        assert_eq!(requests(&failed.reply)[0]["method"], "thread/start");
    }

    #[test]
    fn requests_holt_cannot_serve_are_refused_so_the_turn_does_not_wait() {
        let mut p = ready(None);
        let parsed = p.parse(r#"{"id":7,"method":"item/tool/requestUserInput","params":{"itemId":"x","questions":[]}}"#);
        assert!(parsed.events.is_empty());
        let reply = requests(&parsed.reply);
        assert_eq!(reply[0]["id"], 7);
        assert_eq!(reply[0]["error"]["code"], -32601);
    }

    #[test]
    fn a_failed_turn_reports_the_error() {
        let mut p = ready(None);
        let _ =
            p.parse(r#"{"method":"error","params":{"error":{"message":"rate limited"},"willRetry":false,"threadId":"t","turnId":"u"}}"#);
        let parsed = p.parse(
            r#"{"method":"turn/completed","params":{"threadId":"t","turn":{"id":"u","status":"failed","error":null,"durationMs":5}}}"#,
        );
        assert!(matches!(&parsed.events[0], ChatEvent::Result(r) if r.is_error && r.error == "rate limited"));
    }

    #[test]
    fn the_shell_wrapper_is_not_part_of_the_command_shown() {
        assert_eq!(display_command(r#"/bin/bash -lc "pwd && echo \"x\"""#), r#"pwd && echo "x""#);
        assert_eq!(display_command("/bin/bash -lc 'rm hello.txt'"), "rm hello.txt");
        assert_eq!(display_command("/bin/bash -lc 'echo '\\''hi'\\'''"), "echo 'hi'");
        assert_eq!(display_command(r#"powershell.exe -Command "Get-ChildItem""#), "Get-ChildItem");
        assert_eq!(display_command("git status"), "git status");
        assert_eq!(display_command("/usr/bin/python3 -c 'print(1)'"), "/usr/bin/python3 -c 'print(1)'");
    }

    #[test]
    fn a_new_and_a_deleted_file_become_one_hunk() {
        assert_eq!(
            patch_from_diff(&unified_diff("add", "a\nb\n")),
            vec![PatchHunk { old_start: 0, new_start: 1, lines: vec!["+a".into(), "+b".into()] }]
        );
        assert_eq!(
            patch_from_diff(&unified_diff("delete", "a\n")),
            vec![PatchHunk { old_start: 1, new_start: 0, lines: vec!["-a".into()] }]
        );
        let git =
            "diff --git a/x b/x\nindex 1..2 100644\n--- a/x\n+++ b/x\n@@ -3,2 +3,2 @@ fn f()\n a\n-b\n+c\n\\ No newline at end of file\n";
        assert_eq!(
            patch_from_diff(git),
            vec![PatchHunk { old_start: 3, new_start: 3, lines: vec![" a".into(), "-b".into(), "+c".into()] }]
        );
    }

    #[test]
    fn the_backend_takes_the_model_and_the_effort_as_configuration_and_resumes_by_protocol() {
        let backend = super::super::for_kind("codex").expect("backend");
        assert_eq!(backend.leading_args(), ["app-server"]);
        assert!(!backend.resumes_by_args());
        let args = backend.spawn_args(&super::super::Launch { model: Some("gpt-6-luna"), effort: Some(Effort::High) });
        assert_eq!(args, ["-c", "model=gpt-6-luna", "-c", "model_reasoning_effort=high"]);
        assert!(backend.spawn_args(&super::super::Launch { model: None, effort: None }).is_empty());
    }
}
