//! The backend layer of the chat view: what each CLI contributes so that its structured mode can be shown as a chat.
//!
//! The view itself (`Transcript`, `ChatProcess`, the interface) knows nothing about a CLI. A backend ([`ChatBackend`])
//! says which CLIs are supported ([`Capability`]), which arguments switch the CLI to its structured mode (and pass the
//! model, the effort and the conversation to resume), where the saved conversation is read from, and gives one
//! [`Protocol`] per running process: the state machine that turns the CLI's stdout lines into [`ChatEvent`]s and
//! the messages, answers and interrupts of the user into the lines (or the signal) the CLI expects.
//!
//! Adding a CLI: see "How to add a chat backend" in `memory/ARCHITECTURE.md`.

use std::path::Path;

use serde_json::Value;

use super::{parse_agy_line, parse_claude_line, ChatEvent, Transcript};
use crate::agents::SpawnSpec;
use crate::models::Effort;

/// Whether a CLI has a chat view or only the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    Chat,
    TerminalOnly,
}

/// What a [`Protocol`] makes of one stdout line.
#[derive(Debug, Default)]
pub struct Parsed {
    pub events: Vec<ChatEvent>,
    /// Lines to write back right away: the answers to requests Holt does not serve (they would wait for them forever).
    pub reply: Vec<String>,
}

impl Parsed {
    pub fn events(events: Vec<ChatEvent>) -> Self {
        Self { events, reply: Vec::new() }
    }
}

/// How a CLI cuts the running turn.
#[derive(Debug)]
pub enum Interrupt {
    /// Lines to write to stdin.
    Lines(Vec<String>),
    /// `SIGINT` to the process group (a Ctrl-C on Windows): the CLI stays alive and ends the turn by itself.
    Signal,
}

/// The conversation with one running process of a CLI's structured mode. One per process; the reader thread calls
/// [`Protocol::parse`] and the interface the other methods, always under one lock, so the lines it returns are written
/// in the order the protocol produced them.
pub trait Protocol: Send {
    /// Lines to write as soon as the process starts (a handshake). Most CLIs need none.
    fn open(&mut self) -> Vec<String> {
        Vec::new()
    }
    /// One line of stdout. A line that is not the CLI's JSON (a stray warning) yields nothing.
    fn parse(&mut self, line: &str) -> Parsed;
    /// The lines that send a message of the user (or of another agent) as a new turn. A protocol that needs a handshake
    /// to finish first keeps the message and sends it later, from `parse`.
    fn user_message(&mut self, text: &str) -> Vec<String>;
    /// The lines that answer a permission request: `input` goes back unchanged when allowing, `rules` are the
    /// request's suggestions when the user allowed it for the rest of the session.
    fn answer_permission(&mut self, request_id: &str, allow: bool, input: &Value, deny_message: &str, rules: &[Value]) -> Vec<String>;
    fn interrupt(&mut self) -> Interrupt;
}

/// What a launch asks of the CLI, for the arguments (see [`ChatBackend`]).
pub struct Launch<'a> {
    pub model: Option<&'a str>,
    pub effort: Option<Effort>,
}

/// One CLI's structured mode.
pub trait ChatBackend: Sync {
    /// The agent id (`claude`, `antigravity`, `codex`).
    fn kind(&self) -> &'static str;
    fn capability(&self) -> Capability {
        Capability::Chat
    }
    /// Arguments that go before every other one (a subcommand).
    fn leading_args(&self) -> Vec<String> {
        Vec::new()
    }
    /// Arguments that switch the CLI to its structured mode (they go after the others).
    fn mode_args(&self) -> Vec<String>;
    /// How the model is chosen on the command line.
    fn model_args(&self, model: &str) -> Vec<String>;
    /// How the effort is chosen on the command line (empty when the CLI has no such option).
    fn effort_args(&self, _effort: Effort) -> Vec<String> {
        Vec::new()
    }
    /// The conversation to resume is passed as arguments (`--resume <id>`). A CLI whose protocol resumes by itself
    /// (Codex: `thread/resume`) answers `false` and reads [`SpawnSpec::session`] in [`ChatBackend::protocol`].
    fn resumes_by_args(&self) -> bool {
        true
    }
    /// A new protocol for a process started from `spec`.
    fn protocol(&self, spec: &SpawnSpec) -> Box<dyn Protocol>;
    /// What the CLI saved of the conversation `session`, for a resumed chat to show it. `None` = nothing readable.
    fn history(&self, home: &Path, session: &str) -> Option<Transcript>;

    /// The arguments this backend adds after [`ChatBackend::leading_args`], in the order they must be passed. The model
    /// and the effort are what a restart with a new value changes: spawn again with the same conversation to resume.
    fn spawn_args(&self, launch: &Launch) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(model) = launch.model {
            args.extend(self.model_args(model));
        }
        if let Some(effort) = launch.effort {
            args.extend(self.effort_args(effort));
        }
        args.extend(self.mode_args());
        args
    }
}

static CLAUDE: ClaudeBackend = ClaudeBackend;
static ANTIGRAVITY: AntigravityBackend = AntigravityBackend;
static CODEX: super::codex::CodexBackend = super::codex::CodexBackend;

/// The backend of `kind`; `None` for a CLI that only has its terminal.
pub fn for_kind(kind: &str) -> Option<&'static dyn ChatBackend> {
    let backend: &'static dyn ChatBackend = match kind {
        "claude" => &CLAUDE,
        "antigravity" => &ANTIGRAVITY,
        "codex" => &CODEX,
        _ => return None,
    };
    Some(backend)
}

/// What `kind` offers: [`Capability::TerminalOnly`] for a CLI without a backend.
pub fn capability(kind: &str) -> Capability {
    for_kind(kind).map_or(Capability::TerminalOnly, ChatBackend::capability)
}

// ---------- Claude Code ----------

struct ClaudeBackend;

impl ChatBackend for ClaudeBackend {
    fn kind(&self) -> &'static str {
        "claude"
    }

    fn mode_args(&self) -> Vec<String> {
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

    fn model_args(&self, model: &str) -> Vec<String> {
        vec!["--model".into(), model.into()]
    }

    fn effort_args(&self, effort: Effort) -> Vec<String> {
        vec!["--effort".into(), effort.as_str().into()]
    }

    fn protocol(&self, _spec: &SpawnSpec) -> Box<dyn Protocol> {
        Box::new(ClaudeProtocol)
    }

    fn history(&self, home: &Path, session: &str) -> Option<Transcript> {
        let config_dir = crate::resume::claude_config_dir(home);
        let text = super::history_file(&config_dir, session).and_then(|p| std::fs::read_to_string(p).ok())?;
        Some(Transcript::from_history(&text, super::ITEMS_MAX))
    }
}

struct ClaudeProtocol;

impl Protocol for ClaudeProtocol {
    fn parse(&mut self, line: &str) -> Parsed {
        let mut parsed = Parsed::default();
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { return parsed };
        for ev in parse_claude_line(&v) {
            // A request this module does not know has to be answered, or the CLI waits for it.
            if let ChatEvent::UnknownRequest { request_id } = &ev {
                parsed.reply.push(super::error_response_json(request_id));
            }
            parsed.events.push(ev);
        }
        parsed
    }

    fn user_message(&mut self, text: &str) -> Vec<String> {
        vec![super::user_message_json(text)]
    }

    fn answer_permission(&mut self, request_id: &str, allow: bool, input: &Value, deny_message: &str, rules: &[Value]) -> Vec<String> {
        vec![super::permission_response_json(request_id, allow, input, deny_message, rules)]
    }

    fn interrupt(&mut self) -> Interrupt {
        Interrupt::Lines(vec![super::interrupt_json()])
    }
}

// ---------- Antigravity ----------

struct AntigravityBackend;

impl ChatBackend for AntigravityBackend {
    fn kind(&self) -> &'static str {
        "antigravity"
    }

    fn mode_args(&self) -> Vec<String> {
        ["--input-format", "stream-json", "--output-format", "stream-json", "--mode", "auto"].iter().map(|s| (*s).to_owned()).collect()
    }

    fn model_args(&self, model: &str) -> Vec<String> {
        vec!["--model".into(), model.into()]
    }

    // Antigravity's effort is a suffix of the model name (see `models::effort_model`): nothing on the command line.

    fn protocol(&self, _spec: &SpawnSpec) -> Box<dyn Protocol> {
        Box::new(AntigravityProtocol)
    }

    fn history(&self, home: &Path, session: &str) -> Option<Transcript> {
        let text = super::agy_history_file(home, session).and_then(|p| std::fs::read_to_string(p).ok())?;
        Some(super::from_agy_history(&text, super::ITEMS_MAX))
    }
}

struct AntigravityProtocol;

impl Protocol for AntigravityProtocol {
    fn parse(&mut self, line: &str) -> Parsed {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { return Parsed::default() };
        Parsed::events(parse_agy_line(&v))
    }

    fn user_message(&mut self, text: &str) -> Vec<String> {
        vec![super::agy_user_message_json(text)]
    }

    /// `agy` runs with `permission_mode: "always-proceed"`: it asks nothing.
    fn answer_permission(&mut self, _request_id: &str, _allow: bool, _input: &Value, _deny_message: &str, _rules: &[Value]) -> Vec<String> {
        Vec::new()
    }

    fn interrupt(&mut self) -> Interrupt {
        Interrupt::Signal
    }
}
