//! Chat view of an agent tab: the CLI runs in its structured mode and its events are rendered, instead of its
//! terminal screen being scraped.
//!
//! Claude Code, started with `-p --input-format stream-json --output-format stream-json
//! --include-partial-messages --verbose --permission-prompt-tool stdio`, reads one JSON object per line on stdin
//! (user messages, permission answers, interrupts) and writes one per line on stdout (see [`parse_line`]). This module
//! has three parts, all without interface:
//!
//! 1. [`parse_line`]: a stdout line → typed [`ChatEvent`]s. Checked against real recordings in `tests/data/`.
//! 2. [`Transcript`]: the conversation as a list of [`Item`]s, built by [`Transcript::apply`] from the events (and from
//!    the CLI's own transcript file when a saved conversation is resumed, [`Transcript::from_history`]).
//! 3. [`ChatProcess`]: the child process, its reader/writer threads and the registry/shutdown handling of every tab.
//!
//! Only Claude is supported for now (`supports`); the notes about Codex, OpenCode and ACP are in
//! `memory/ARCHITECTURE.md`.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::{json, Value};

use crate::agents::SpawnSpec;
use crate::t;
use crate::{process, Core};

/// Tool results longer than this are cut when stored (the interface shows a few lines anyway).
const RESULT_MAX_CHARS: usize = 8_000;
/// Items kept in a transcript; the oldest are dropped (a resumed conversation loads the tail only).
const ITEMS_MAX: usize = 1_500;
/// The text the CLI echoes when a turn is interrupted.
const INTERRUPTED: &str = "[Request interrupted by user";

/// CLIs that have a chat view.
pub fn supports(kind: &str) -> bool {
    kind == "claude"
}

/// Slash commands that open a screen of the terminal UI and do nothing useful in the structured mode.
pub const TERMINAL_ONLY_COMMANDS: &[&str] =
    &["config", "login", "logout", "theme", "terminal-setup", "vim", "ide", "resume", "statusline", "output-style", "keybindings"];

/// Flags that switch Claude Code to the structured mode.
pub fn chat_args() -> Vec<String> {
    [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--include-partial-messages",
        "--verbose",
        "--permission-prompt-tool",
        "stdio",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect()
}

// ---------- Events ----------

/// A block of a complete assistant message.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Text(String),
    Thinking(String),
    ToolUse { id: String, name: String, input: Value },
}

/// The outcome of a tool call, from a `tool_result` block.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutcome {
    pub tool_use_id: String,
    pub text: String,
    pub is_error: bool,
}

/// The CLI asks whether a tool may run (`control_request` / `can_use_tool`).
#[derive(Debug, Clone, PartialEq)]
pub struct PermissionRequest {
    pub request_id: String,
    pub tool_use_id: String,
    pub tool: String,
    pub input: Value,
    pub description: String,
    /// What "allow for this session" would add (`permission_suggestions` of the request); empty = the CLI offers none.
    pub suggestions: Vec<Value>,
}

/// End of a turn (`result`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TurnResult {
    pub is_error: bool,
    /// The error text, when `is_error`.
    pub error: String,
    pub cost_usd: f64,
    pub duration_ms: u64,
    pub session: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ChatEvent {
    /// `system` / `init`, sent at the start of every turn.
    Init {
        session: String,
        model: String,
        commands: Vec<String>,
        terminal_commands: Vec<String>,
    },
    /// `system` / `status` `requesting`: the CLI is calling the model.
    Requesting,
    TextDelta {
        message: String,
        text: String,
    },
    /// A complete assistant message (or the part of one that just finished).
    Assistant {
        message: String,
        blocks: Vec<Block>,
    },
    ToolResults(Vec<ToolOutcome>),
    Permission(PermissionRequest),
    /// The CLI withdrew a permission request (`control_cancel_request`).
    PermissionCancelled(String),
    /// The turn was cut by an interrupt.
    Interrupted,
    Result(TurnResult),
    /// A control request this module does not know: it has to be answered, or the CLI waits for it.
    UnknownRequest {
        request_id: String,
    },
    /// A line with nothing to show (rate limits, hooks, partial tool input...).
    Ignored,
}

fn str_of<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array).map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect()).unwrap_or_default()
}

/// The text of a `tool_result` content: a string, or a list of `{type:"text", text}` blocks.
fn result_text(content: &Value) -> String {
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .map(|p| match p.get("type").and_then(Value::as_str) {
                Some("text") => str_of(p, "text").to_owned(),
                Some("image") => "[image]".to_owned(),
                _ => String::new(),
            })
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    };
    clip(&text, RESULT_MAX_CHARS)
}

fn clip(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((at, _)) => format!("{}\n… ({} more characters)", &text[..at], text[at..].chars().count()),
        None => text.to_owned(),
    }
}

fn blocks_of(content: &Value) -> Vec<Block> {
    let Some(list) = content.as_array() else { return Vec::new() };
    list.iter()
        .filter_map(|b| match b.get("type").and_then(Value::as_str)? {
            "text" => Some(Block::Text(str_of(b, "text").to_owned())),
            "thinking" => Some(Block::Thinking(str_of(b, "thinking").to_owned())),
            "tool_use" => Some(Block::ToolUse {
                id: str_of(b, "id").to_owned(),
                name: str_of(b, "name").to_owned(),
                input: b.get("input").cloned().unwrap_or(Value::Null),
            }),
            _ => None,
        })
        .collect()
}

/// Turns one line of the CLI's stdout into events. A line that is not JSON (a stray warning) yields nothing.
pub fn parse_line(line: &str) -> Vec<ChatEvent> {
    let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { return Vec::new() };
    let event = match str_of(&v, "type") {
        "system" => match str_of(&v, "subtype") {
            "init" => ChatEvent::Init {
                session: str_of(&v, "session_id").to_owned(),
                model: str_of(&v, "model").to_owned(),
                commands: strings(v.get("slash_commands")),
                terminal_commands: strings(v.get("terminal_slash_commands")),
            },
            "status" if str_of(&v, "status") == "requesting" => ChatEvent::Requesting,
            _ => ChatEvent::Ignored,
        },
        "stream_event" => {
            let ev = v.get("event").unwrap_or(&Value::Null);
            let delta = ev.get("delta").unwrap_or(&Value::Null);
            // A sub-agent's stream is not the conversation's.
            if !v.get("parent_tool_use_id").is_none_or(Value::is_null) {
                return Vec::new();
            }
            if str_of(ev, "type") == "content_block_delta" && str_of(delta, "type") == "text_delta" {
                ChatEvent::TextDelta { message: str_of(&v, "api_message_id").to_owned(), text: str_of(delta, "text").to_owned() }
            } else {
                ChatEvent::Ignored
            }
        }
        "assistant" => {
            if !v.get("parent_tool_use_id").is_none_or(Value::is_null) {
                return Vec::new();
            }
            let msg = v.get("message").unwrap_or(&Value::Null);
            ChatEvent::Assistant { message: str_of(msg, "id").to_owned(), blocks: blocks_of(msg.get("content").unwrap_or(&Value::Null)) }
        }
        "user" => {
            if !v.get("parent_tool_use_id").is_none_or(Value::is_null) {
                return Vec::new();
            }
            let content = v.get("message").and_then(|m| m.get("content")).unwrap_or(&Value::Null);
            let results: Vec<ToolOutcome> = content
                .as_array()
                .into_iter()
                .flatten()
                .filter(|b| str_of(b, "type") == "tool_result")
                .map(|b| ToolOutcome {
                    tool_use_id: str_of(b, "tool_use_id").to_owned(),
                    text: result_text(b.get("content").unwrap_or(&Value::Null)),
                    is_error: b.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                })
                .collect();
            if !results.is_empty() {
                ChatEvent::ToolResults(results)
            } else if user_text(content).starts_with(INTERRUPTED) {
                ChatEvent::Interrupted
            } else {
                ChatEvent::Ignored
            }
        }
        "control_request" => {
            let req = v.get("request").unwrap_or(&Value::Null);
            let request_id = str_of(&v, "request_id").to_owned();
            if str_of(req, "subtype") == "can_use_tool" {
                ChatEvent::Permission(PermissionRequest {
                    request_id,
                    tool_use_id: str_of(req, "tool_use_id").to_owned(),
                    tool: str_of(req, "tool_name").to_owned(),
                    input: req.get("input").cloned().unwrap_or(Value::Null),
                    description: str_of(req, "description").to_owned(),
                    suggestions: req.get("permission_suggestions").and_then(Value::as_array).cloned().unwrap_or_default(),
                })
            } else {
                ChatEvent::UnknownRequest { request_id }
            }
        }
        "control_cancel_request" => ChatEvent::PermissionCancelled(str_of(&v, "request_id").to_owned()),
        "result" => {
            let is_error = v.get("is_error").and_then(Value::as_bool).unwrap_or(false);
            ChatEvent::Result(TurnResult {
                is_error,
                error: if is_error { str_of(&v, "result").to_owned() } else { String::new() },
                cost_usd: v.get("total_cost_usd").and_then(Value::as_f64).unwrap_or(0.0),
                duration_ms: v.get("duration_ms").and_then(Value::as_u64).unwrap_or(0),
                session: str_of(&v, "session_id").to_owned(),
            })
        }
        _ => ChatEvent::Ignored,
    };
    if event == ChatEvent::Ignored {
        Vec::new()
    } else {
        vec![event]
    }
}

/// The text of a user message's content (a string, or the text blocks of a list).
fn user_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => {
            parts.iter().filter(|p| str_of(p, "type") == "text").map(|p| str_of(p, "text")).collect::<Vec<_>>().join("\n")
        }
        _ => String::new(),
    }
}

// ---------- What goes to stdin ----------

/// A user message.
pub fn user_message_json(text: &str) -> String {
    json!({ "type": "user", "message": { "role": "user", "content": text } }).to_string()
}

/// The answer to a [`PermissionRequest`]: `input` is sent back unchanged when allowing, with `rules` (the request's
/// suggestions) when the user allowed it for the rest of the session.
pub fn permission_response_json(request_id: &str, allow: bool, input: &Value, deny_message: &str, rules: &[Value]) -> String {
    let response = if allow && !rules.is_empty() {
        json!({ "behavior": "allow", "updatedInput": input, "updatedPermissions": rules })
    } else if allow {
        json!({ "behavior": "allow", "updatedInput": input })
    } else {
        json!({ "behavior": "deny", "message": deny_message })
    };
    json!({ "type": "control_response", "response": { "subtype": "success", "request_id": request_id, "response": response } }).to_string()
}

/// Cuts the current turn.
pub fn interrupt_json() -> String {
    json!({ "type": "control_request", "request_id": format!("holt-int-{}", crate::util::now_ms()), "request": { "subtype": "interrupt" } })
        .to_string()
}

fn error_response_json(request_id: &str) -> String {
    json!({ "type": "control_response", "response": { "subtype": "error", "request_id": request_id, "error": "not supported by Holt" } })
        .to_string()
}

// ---------- Transcript ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermState {
    Pending,
    Allowed,
    Denied,
    /// The turn ended (or the CLI withdrew the request) before an answer.
    Cancelled,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
    pub outcome: Option<ToolOutcome>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PermissionCard {
    pub request_id: String,
    pub tool_use_id: String,
    pub tool: String,
    pub input: Value,
    pub description: String,
    pub suggestions: Vec<Value>,
    pub state: PermState,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// What the user typed, or a message another agent sent to this tab (`from` = the agent's id).
    User {
        text: String,
        from: Option<String>,
    },
    Assistant {
        message: String,
        text: String,
        streaming: bool,
    },
    Thinking(String),
    Tool(ToolCall),
    Permission(PermissionCard),
    /// Something Holt says in the conversation (a terminal-only command, an interrupt, an error).
    Notice {
        text: String,
        error: bool,
    },
}

/// The conversation of one chat tab.
#[derive(Debug, Clone, Default)]
pub struct Transcript {
    pub items: Vec<Item>,
    /// Turns sent and not yet finished; the tab is busy while there are any.
    pending_turns: u32,
    pub session: Option<String>,
    pub model: String,
    /// Slash commands the CLI offers (names without the slash).
    pub commands: Vec<String>,
    /// Commands the CLI itself says only work in the terminal.
    pub terminal_commands: Vec<String>,
    pub cost_usd: f64,
    /// Bumped on every change, for the interface to know when to lay the text out again.
    pub version: u64,
}

impl Transcript {
    pub fn busy(&self) -> bool {
        self.pending_turns > 0
    }

    /// A permission card is waiting for the user.
    pub fn waiting_for_permission(&self) -> bool {
        self.items.iter().any(|i| matches!(i, Item::Permission(p) if p.state == PermState::Pending))
    }

    fn push(&mut self, item: Item) {
        self.items.push(item);
        if self.items.len() > ITEMS_MAX {
            let drop = self.items.len() - ITEMS_MAX;
            self.items.drain(..drop);
        }
    }

    /// A message was sent to the CLI (it starts a turn).
    pub fn push_user(&mut self, text: &str, from: Option<&str>) {
        self.version += 1;
        self.finish_streaming();
        self.pending_turns += 1;
        self.push(Item::User { text: text.to_owned(), from: from.map(str::to_owned) });
    }

    pub fn push_notice(&mut self, text: impl Into<String>, error: bool) {
        self.version += 1;
        self.push(Item::Notice { text: text.into(), error });
    }

    fn finish_streaming(&mut self) {
        for item in &mut self.items {
            if let Item::Assistant { streaming, .. } = item {
                *streaming = false;
            }
        }
    }

    /// The turn the process was running is gone (it exited or was killed).
    pub fn process_ended(&mut self) {
        self.version += 1;
        self.pending_turns = 0;
        self.finish_streaming();
        self.cancel_permissions();
    }

    fn cancel_permissions(&mut self) {
        for item in &mut self.items {
            if let Item::Permission(p) = item {
                if p.state == PermState::Pending {
                    p.state = PermState::Cancelled;
                }
            }
        }
    }

    /// The user answered a card.
    pub fn answer_permission(&mut self, request_id: &str, allow: bool) {
        self.version += 1;
        for item in &mut self.items {
            if let Item::Permission(p) = item {
                if p.request_id == request_id && p.state == PermState::Pending {
                    p.state = if allow { PermState::Allowed } else { PermState::Denied };
                }
            }
        }
    }

    pub fn pending_permission(&self) -> Option<&PermissionCard> {
        self.items.iter().find_map(|i| match i {
            Item::Permission(p) if p.state == PermState::Pending => Some(p),
            _ => None,
        })
    }

    pub fn apply(&mut self, ev: &ChatEvent) {
        self.version += 1;
        match ev {
            ChatEvent::Init { session, model, commands, terminal_commands } => {
                if !session.is_empty() {
                    self.session = Some(session.clone());
                }
                if !model.is_empty() {
                    self.model = model.clone();
                }
                self.commands = commands.clone();
                self.terminal_commands = terminal_commands.clone();
            }
            ChatEvent::Requesting | ChatEvent::UnknownRequest { .. } | ChatEvent::Ignored => {}
            ChatEvent::TextDelta { message, text } => match self.items.last_mut() {
                Some(Item::Assistant { message: m, text: t, streaming: true }) if m == message => t.push_str(text),
                _ => self.push(Item::Assistant { message: message.clone(), text: text.clone(), streaming: true }),
            },
            ChatEvent::Assistant { message, blocks } => {
                for block in blocks {
                    self.apply_block(message, block);
                }
            }
            ChatEvent::ToolResults(results) => {
                for r in results {
                    let call = self.items.iter_mut().rev().find_map(|i| match i {
                        Item::Tool(c) if c.id == r.tool_use_id => Some(c),
                        _ => None,
                    });
                    if let Some(call) = call {
                        call.outcome = Some(r.clone());
                    }
                }
            }
            ChatEvent::Permission(p) => {
                self.finish_streaming();
                self.push(Item::Permission(PermissionCard {
                    request_id: p.request_id.clone(),
                    tool_use_id: p.tool_use_id.clone(),
                    tool: p.tool.clone(),
                    input: p.input.clone(),
                    description: p.description.clone(),
                    suggestions: p.suggestions.clone(),
                    state: PermState::Pending,
                }));
            }
            ChatEvent::PermissionCancelled(id) => {
                for item in &mut self.items {
                    if let Item::Permission(p) = item {
                        if p.request_id == *id && p.state == PermState::Pending {
                            p.state = PermState::Cancelled;
                        }
                    }
                }
            }
            ChatEvent::Interrupted => self.push(Item::Notice { text: t!("Interrupted.").to_owned(), error: false }),
            ChatEvent::Result(r) => {
                self.pending_turns = self.pending_turns.saturating_sub(1);
                self.finish_streaming();
                self.cancel_permissions();
                self.cost_usd = r.cost_usd;
                if !r.session.is_empty() {
                    self.session = Some(r.session.clone());
                }
                if r.is_error && !r.error.is_empty() {
                    self.push(Item::Notice { text: r.error.clone(), error: true });
                }
            }
        }
    }

    fn apply_block(&mut self, message: &str, block: &Block) {
        match block {
            Block::Text(text) => {
                // The final text of a block that was streaming replaces what the deltas built.
                let streamed = self.items.iter_mut().rev().find_map(|i| match i {
                    Item::Assistant { message: m, text: t, streaming: true } if m == message => Some(t),
                    _ => None,
                });
                if let Some(t) = streamed {
                    t.clone_from(text);
                    self.finish_streaming();
                } else if !text.trim().is_empty() {
                    self.push(Item::Assistant { message: message.to_owned(), text: text.clone(), streaming: false });
                }
            }
            Block::Thinking(text) => {
                if !text.trim().is_empty() {
                    self.push(Item::Thinking(text.clone()));
                }
            }
            Block::ToolUse { id, name, input } => {
                if !self.items.iter().any(|i| matches!(i, Item::Tool(c) if c.id == *id)) {
                    self.finish_streaming();
                    self.push(Item::Tool(ToolCall { id: id.clone(), name: name.clone(), input: input.clone(), outcome: None }));
                }
            }
        }
    }

    /// Rebuilds the conversation from the CLI's own transcript file (`<claude>/projects/<folder>/<session>.jsonl`), so a
    /// resumed tab shows what was said before. Only the last `max_items` items are kept.
    pub fn from_history(jsonl: &str, max_items: usize) -> Transcript {
        let mut t = Transcript::default();
        for line in jsonl.lines() {
            let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
            if v.get("isSidechain").and_then(Value::as_bool).unwrap_or(false) || v.get("isMeta").and_then(Value::as_bool).unwrap_or(false) {
                continue;
            }
            let content = v.get("message").and_then(|m| m.get("content")).unwrap_or(&Value::Null);
            match str_of(&v, "type") {
                "user" => {
                    let results: Vec<ToolOutcome> = content
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|b| str_of(b, "type") == "tool_result")
                        .map(|b| ToolOutcome {
                            tool_use_id: str_of(b, "tool_use_id").to_owned(),
                            text: result_text(b.get("content").unwrap_or(&Value::Null)),
                            is_error: b.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                        })
                        .collect();
                    if !results.is_empty() {
                        t.apply(&ChatEvent::ToolResults(results));
                        continue;
                    }
                    let text = user_text(content);
                    let text = history_user_text(&text);
                    if let Some(text) = text {
                        t.push(Item::User { text, from: None });
                    }
                }
                "assistant" => {
                    let msg = v.get("message").unwrap_or(&Value::Null);
                    if str_of(msg, "model") == "<synthetic>" && !t.items.iter().any(|i| matches!(i, Item::User { .. })) {
                        continue;
                    }
                    t.apply(&ChatEvent::Assistant { message: str_of(msg, "id").to_owned(), blocks: blocks_of(content) });
                }
                _ => {}
            }
        }
        let keep_from = t.items.len().saturating_sub(max_items);
        t.items.drain(..keep_from);
        t.pending_turns = 0;
        t
    }
}

/// What a user line of the CLI's transcript shows: the text, `/command args` for a slash command, nothing for the
/// reminders and local-command noise the CLI stores there.
fn history_user_text(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() || text.starts_with("<local-command") || text.starts_with("<system-reminder") || text.starts_with(INTERRUPTED) {
        return None;
    }
    if text.starts_with("<command-name>") {
        let tag = |name: &str| -> String {
            let open = format!("<{name}>");
            let close = format!("</{name}>");
            text.split_once(&open)
                .and_then(|(_, rest)| rest.split_once(&close))
                .map(|(inner, _)| inner.trim().to_owned())
                .unwrap_or_default()
        };
        let (name, args) = (tag("command-name"), tag("command-args"));
        return Some(if args.is_empty() { name } else { format!("{name} {args}") });
    }
    Some(text.to_owned())
}

/// The path of the CLI's transcript for `session`, if the CLI has it saved.
pub fn history_file(config_dir: &Path, session: &str) -> Option<std::path::PathBuf> {
    if !crate::resume::valid_session_id(session) {
        return None;
    }
    std::fs::read_dir(config_dir.join("projects")).ok()?.flatten().map(|d| d.path().join(format!("{session}.jsonl"))).find(|p| p.is_file())
}

// ---------- Slash commands ----------

/// What the composer does with a line that starts with `/`: the name of the command, if it is a terminal-only one.
pub fn terminal_only_command(text: &str, from_cli: &[String]) -> Option<String> {
    let name = text.trim().strip_prefix('/')?.split_whitespace().next()?.to_lowercase();
    (TERMINAL_ONLY_COMMANDS.contains(&name.as_str()) || from_cli.contains(&name)).then_some(name)
}

// ---------- View of a tab ----------

impl Core {
    /// Whether the tab shows the chat view: the user's choice for this tab, else the default of its CLI.
    pub fn tab_is_chat(&self, project: &str, ident: &crate::Ident) -> bool {
        if !supports(&ident.kind) {
            return false;
        }
        let cfg = self.inner.config.read();
        match cfg.project(project).and_then(|p| p.tab_views.get(&ident.id())) {
            Some(choice) => choice != "terminal",
            None => cfg.chat_by_default,
        }
    }

    /// Saves the user's choice of view for the tab.
    pub fn set_tab_chat(&self, project: &str, id: &str, chat: bool) {
        self.update_project(project, |p| {
            p.tab_views.insert(id.to_owned(), if chat { "chat" } else { "terminal" }.to_owned());
        });
    }

    /// Reads the saved conversation of a tab from the CLI's own transcript, for a resumed chat to show it.
    pub fn chat_history(&self, project: &str, id: &str) -> Transcript {
        let Some(session) = self.tab_session(project, id) else { return Transcript::default() };
        let config_dir = crate::resume::claude_config_dir(&self.inner.home);
        let Some(text) = history_file(&config_dir, &session).and_then(|p| std::fs::read_to_string(p).ok()) else {
            return Transcript::default();
        };
        let mut t = Transcript::from_history(&text, ITEMS_MAX);
        t.session = Some(session);
        t
    }
}

// ---------- Process ----------

/// What the reader threads hand to the interface.
#[derive(Debug)]
pub enum ChatMsg {
    Event(ChatEvent),
    /// The last line the process wrote to stderr (kept for the error shown when it dies).
    Stderr(String),
    /// stdout closed: the process ended with this code.
    Exited(Option<i32>),
}

/// A running chat process. Dropping it kills the process.
pub struct ChatProcess {
    child: Arc<Mutex<Child>>,
    registry_id: u64,
    core: Core,
    stdin: Sender<String>,
    rx: Receiver<ChatMsg>,
}

impl Core {
    /// Starts `spec` (a Claude launch from [`Core::spawn_spec_chat`]) with its pipes. `wake` is called whenever
    /// something arrives, to make the interface look at [`ChatProcess::try_recv`].
    pub fn spawn_chat(&self, spec: &SpawnSpec, wake: Arc<dyn Fn() + Send + Sync>) -> std::io::Result<ChatProcess> {
        let mut cmd = Command::new(&spec.program);
        cmd.args(&spec.args).current_dir(&spec.cwd).env_clear().envs(&spec.env);
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        process::platform_flags(&mut cmd);
        let mut child = cmd.spawn()?;
        process::die_with_app(&child);
        let stdout = child.stdout.take().ok_or_else(|| std::io::Error::other("no stdout"))?;
        let stderr = child.stderr.take();
        let mut pipe = child.stdin.take().ok_or_else(|| std::io::Error::other("no stdin"))?;
        let child = Arc::new(Mutex::new(child));
        let registry_id = self.inner.processes.add(child.clone());

        let (tx, rx) = channel();
        let (stdin, lines) = channel::<String>();
        // Writer: a stalled process must never block the interface.
        std::thread::Builder::new().name("chat-write".into()).spawn(move || {
            for line in lines {
                if pipe.write_all(line.as_bytes()).and_then(|()| pipe.write_all(b"\n")).and_then(|()| pipe.flush()).is_err() {
                    break;
                }
            }
        })?;
        if let Some(mut err) = stderr {
            let (tx, wake) = (tx.clone(), wake.clone());
            std::thread::Builder::new().name("chat-stderr".into()).spawn(move || {
                let mut text = String::new();
                let _ = err.read_to_string(&mut text);
                if let Some(last) = text.lines().rev().find(|l| !l.trim().is_empty()) {
                    let _ = tx.send(ChatMsg::Stderr(last.trim().to_owned()));
                    wake();
                }
            })?;
        }
        let (reply, child_for_reader) = (stdin.clone(), child.clone());
        std::thread::Builder::new().name("chat-read".into()).spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let mut any = false;
                for ev in parse_line(&line) {
                    if let ChatEvent::UnknownRequest { request_id } = &ev {
                        let _ = reply.send(error_response_json(request_id));
                    }
                    any = true;
                    let _ = tx.send(ChatMsg::Event(ev));
                }
                if any {
                    wake();
                }
            }
            let code = child_for_reader.lock().wait().ok().and_then(|s| s.code());
            let _ = tx.send(ChatMsg::Exited(code));
            wake();
        })?;
        Ok(ChatProcess { child, registry_id, core: self.clone(), stdin, rx })
    }
}

impl ChatProcess {
    pub fn try_recv(&self) -> Option<ChatMsg> {
        self.rx.try_recv().ok()
    }

    fn send_line(&self, line: String) {
        let _ = self.stdin.send(line);
    }

    pub fn send_user(&self, text: &str) {
        self.send_line(user_message_json(text));
    }

    pub fn answer_permission(&self, request_id: &str, allow: bool, input: &Value, deny_message: &str, rules: &[Value]) {
        self.send_line(permission_response_json(request_id, allow, input, deny_message, rules));
    }

    pub fn interrupt(&self) {
        self.send_line(interrupt_json());
    }

    pub fn kill(&self) {
        process::kill_tree(&mut self.child.lock());
    }
}

impl Drop for ChatProcess {
    fn drop(&mut self) {
        self.kill();
        self.core.inner.processes.remove(self.registry_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(sample: &str) -> Vec<ChatEvent> {
        sample.lines().flat_map(parse_line).collect()
    }

    const TOOLS: &str = include_str!("../tests/data/claude-stream-tools.jsonl");
    const TEXT: &str = include_str!("../tests/data/claude-stream-text.jsonl");
    const INTERRUPT: &str = include_str!("../tests/data/claude-stream-interrupt.jsonl");

    fn replay(sample: &str) -> Transcript {
        let mut t = Transcript::default();
        t.push_user("go", None);
        for ev in events(sample) {
            t.apply(&ev);
        }
        t
    }

    #[test]
    fn the_init_event_names_the_session_model_and_commands() {
        let evs = events(TOOLS);
        let Some(ChatEvent::Init { session, model, commands, terminal_commands }) = evs.first() else { panic!("{:?}", evs.first()) };
        assert_eq!(session.len(), 36);
        assert_eq!(model, "claude-opus-5-5");
        assert!(!commands.is_empty());
        assert!(terminal_commands.contains(&"doctor".to_owned()));
    }

    #[test]
    fn text_deltas_stream_into_one_assistant_item_that_the_final_message_completes() {
        let evs = events(TEXT);
        let deltas = evs.iter().filter(|e| matches!(e, ChatEvent::TextDelta { .. })).count();
        assert!(deltas > 5, "{deltas}");
        let t = replay(TEXT);
        let texts: Vec<&Item> = t.items.iter().filter(|i| matches!(i, Item::Assistant { .. })).collect();
        assert_eq!(texts.len(), 1, "{:?}", t.items);
        let Item::Assistant { text, streaming, .. } = texts[0] else { unreachable!() };
        assert!(!streaming);
        assert!(text.starts_with("I couldn't create `hello.txt`"), "{text}");
        assert!(text.ends_with("try again?"));
    }

    #[test]
    fn a_tool_call_gets_its_result_and_the_permission_card_is_cancelled_at_the_end_of_the_turn() {
        let t = replay(TEXT);
        let Some(Item::Tool(call)) = t.items.iter().find(|i| matches!(i, Item::Tool(_))) else { panic!("{:?}", t.items) };
        assert_eq!(call.name, "Write");
        assert_eq!(call.input["content"], "hi");
        assert!(call.outcome.as_ref().is_some_and(|o| o.is_error));
        let Some(Item::Permission(card)) = t.items.iter().find(|i| matches!(i, Item::Permission(_))) else { panic!() };
        assert_eq!(card.tool, "Write");
        assert_eq!(card.suggestions.len(), 1);
        assert_eq!(card.state, PermState::Cancelled);
        assert!(!t.waiting_for_permission());
    }

    #[test]
    fn a_permission_request_waits_until_it_is_answered() {
        let mut t = Transcript::default();
        t.push_user("write it", None);
        for ev in events(TEXT).into_iter().take_while(|e| !matches!(e, ChatEvent::ToolResults(_))) {
            t.apply(&ev);
        }
        assert!(t.busy());
        let card = t.pending_permission().expect("a pending card").clone();
        assert_eq!(card.tool, "Write");
        t.answer_permission(&card.request_id, true);
        assert!(t.pending_permission().is_none());
        assert!(
            matches!(t.items.iter().find(|i| matches!(i, Item::Permission(_))), Some(Item::Permission(p)) if p.state == PermState::Allowed)
        );
    }

    #[test]
    fn a_session_with_edits_has_tools_with_results_and_ends_the_turn() {
        let t = replay(TOOLS);
        let names: Vec<&str> = t.items.iter().filter_map(|i| if let Item::Tool(c) = i { Some(c.name.as_str()) } else { None }).collect();
        assert_eq!(names, ["Write", "Edit", "Bash"]);
        for item in &t.items {
            if let Item::Tool(c) = item {
                assert!(c.outcome.as_ref().is_some_and(|o| !o.is_error), "{}", c.name);
            }
        }
        assert!(t.cost_usd > 0.0);
        assert!(t.session.is_some());
    }

    #[test]
    fn two_results_are_two_turns() {
        // The sample has two user turns (a task and `/context`): `replay` sent one message, so it ends with one pending.
        assert_eq!(events(TOOLS).iter().filter(|e| matches!(e, ChatEvent::Result(_))).count(), 2);
        assert!(replay(TOOLS).pending_turns == 0);
    }

    #[test]
    fn a_slash_command_answers_with_a_plain_assistant_message() {
        let t = replay(TOOLS);
        let Some(Item::Assistant { text, .. }) = t.items.last() else { panic!("{:?}", t.items.last()) };
        assert!(text.starts_with("## Context Usage"), "{text}");
    }

    #[test]
    fn an_interrupt_is_noticed_and_the_next_turn_works() {
        let evs = events(INTERRUPT);
        assert!(evs.contains(&ChatEvent::Interrupted));
        let t = replay(INTERRUPT);
        assert!(t.items.iter().any(|i| matches!(i, Item::Notice { text, .. } if text == "Interrupted.")));
        assert!(matches!(t.items.last(), Some(Item::Assistant { text, .. }) if text == "OK"));
    }

    #[test]
    fn what_goes_to_stdin_is_one_json_object() {
        let m: Value = serde_json::from_str(&user_message_json("hi\nthere")).unwrap();
        assert_eq!((m["type"].as_str(), m["message"]["content"].as_str()), (Some("user"), Some("hi\nthere")));
        let input = json!({ "file_path": "/x" });
        let ok: Value = serde_json::from_str(&permission_response_json("r1", true, &input, "", &[])).unwrap();
        assert_eq!(ok["response"]["request_id"], "r1");
        assert_eq!(ok["response"]["response"]["behavior"], "allow");
        assert_eq!(ok["response"]["response"]["updatedInput"], input);
        let rules = [json!({ "type": "setMode", "mode": "acceptEdits", "destination": "session" })];
        let session: Value = serde_json::from_str(&permission_response_json("r3", true, &input, "", &rules)).unwrap();
        assert_eq!(session["response"]["response"]["updatedPermissions"], json!(rules));
        let no: Value = serde_json::from_str(&permission_response_json("r2", false, &input, "No.", &[])).unwrap();
        assert_eq!(
            (no["response"]["response"]["behavior"].as_str(), no["response"]["response"]["message"].as_str()),
            (Some("deny"), Some("No."))
        );
        let i: Value = serde_json::from_str(&interrupt_json()).unwrap();
        assert_eq!(i["request"]["subtype"], "interrupt");
    }

    #[test]
    fn unknown_requests_and_stray_lines() {
        assert!(parse_line("not json").is_empty());
        assert!(parse_line(r#"{"type":"rate_limit_event"}"#).is_empty());
        let ev = parse_line(r#"{"type":"control_request","request_id":"a","request":{"subtype":"hook_callback"}}"#);
        assert_eq!(ev, vec![ChatEvent::UnknownRequest { request_id: "a".into() }]);
        assert_eq!(parse_line(r#"{"type":"control_cancel_request","request_id":"a"}"#), vec![ChatEvent::PermissionCancelled("a".into())]);
    }

    #[test]
    fn a_subagents_messages_are_not_the_conversation() {
        let line = r#"{"type":"assistant","parent_tool_use_id":"toolu_1","message":{"id":"m","content":[{"type":"text","text":"x"}]}}"#;
        assert!(parse_line(line).is_empty());
    }

    #[test]
    fn long_tool_output_is_cut() {
        let big = "x".repeat(RESULT_MAX_CHARS + 50);
        let line = json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "tool_use_id": "t", "content": big }] } });
        let Some(ChatEvent::ToolResults(r)) = parse_line(&line.to_string()).pop() else { panic!() };
        assert!(r[0].text.ends_with("(50 more characters)"));
    }

    #[test]
    fn the_history_of_a_saved_conversation_shows_what_was_said() {
        let jsonl = [
            json!({ "type": "queue-operation" }),
            json!({ "type": "user", "message": { "content": "Add a menu entry" } }),
            json!({ "type": "attachment" }),
            json!({ "type": "assistant", "message": { "id": "m1", "content": [{ "type": "tool_use", "id": "t1", "name": "Edit", "input": { "file_path": "a.rs" } }] } }),
            json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "tool_use_id": "t1", "content": "ok" }] } }),
            json!({ "type": "user", "isMeta": true, "message": { "content": "<local-command-caveat>x</local-command-caveat>" } }),
            json!({ "type": "user", "message": { "content": "<command-name>/context</command-name>\n<command-message>context</command-message>\n<command-args></command-args>" } }),
            json!({ "type": "user", "isSidechain": true, "message": { "content": "sub-agent prompt" } }),
            json!({ "type": "assistant", "message": { "id": "m2", "content": [{ "type": "text", "text": "Done." }] } }),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
        let t = Transcript::from_history(&jsonl, 100);
        assert!(!t.busy());
        assert_eq!(t.items.len(), 4, "{:?}", t.items);
        assert!(matches!(&t.items[0], Item::User { text, .. } if text == "Add a menu entry"));
        assert!(matches!(&t.items[1], Item::Tool(c) if c.name == "Edit" && c.outcome.is_some()));
        assert!(matches!(&t.items[2], Item::User { text, .. } if text == "/context"));
        assert!(matches!(&t.items[3], Item::Assistant { text, .. } if text == "Done."));
        assert_eq!(Transcript::from_history(&jsonl, 2).items.len(), 2);
    }

    #[test]
    fn terminal_only_commands_are_recognised() {
        assert_eq!(terminal_only_command("/config", &[]), Some("config".into()));
        assert_eq!(terminal_only_command("  /Theme dark", &[]), Some("theme".into()));
        assert_eq!(terminal_only_command("/doctor", &["doctor".into()]), Some("doctor".into()));
        assert_eq!(terminal_only_command("/context", &["doctor".into()]), None);
        assert_eq!(terminal_only_command("config", &[]), None);
    }

    #[test]
    fn a_dead_process_frees_the_tab_and_withdraws_the_cards() {
        let mut t = Transcript::default();
        t.push_user("x", None);
        t.apply(&ChatEvent::Permission(PermissionRequest {
            request_id: "r".into(),
            tool_use_id: "t".into(),
            tool: "Bash".into(),
            input: Value::Null,
            description: String::new(),
            suggestions: vec![],
        }));
        assert!(t.busy() && t.waiting_for_permission());
        t.process_ended();
        assert!(!t.busy() && !t.waiting_for_permission());
    }

    #[test]
    fn history_files_are_found_by_session_id() {
        let dir = std::env::temp_dir().join(format!("holt-chat-hist-{}", std::process::id()));
        let proj = dir.join("projects").join("-some-folder");
        std::fs::create_dir_all(&proj).unwrap();
        let id = "11111111-2222-4333-8444-555555555555";
        std::fs::write(proj.join(format!("{id}.jsonl")), "{}").unwrap();
        assert!(history_file(&dir, id).is_some());
        assert!(history_file(&dir, "22222222-2222-4333-8444-555555555555").is_none());
        assert!(history_file(&dir, "../x").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
