//! Chat view of a tab: the state of the conversation, the process behind it and what goes in and out.
//! The data model and the parsing are in `holt_core::chat`; painting is in [`crate::ui::chat`].

use holt_core::chat::{self, ChatEvent, ChatMsg, ChatProcess, Item, Transcript};

use super::*;

/// What a chat tab holds besides the tab itself. A tab has one while its view is Chat (the process may be gone).
pub struct ChatTab {
    pub proc: Option<ChatProcess>,
    pub transcript: Transcript,
    pub composer: String,
    /// What was sent from this composer, oldest first, and the position while browsing it with ↑/↓ (`draft` keeps what
    /// was being typed).
    pub history: Vec<String>,
    pub history_pos: Option<usize>,
    pub draft: String,
    /// Tool calls the user opened or closed by hand (tool id → open), over the default for their kind.
    pub opened: HashMap<String, bool>,
    pub markdown: egui_commonmark::CommonMarkCache,
    /// Laid-out height of each item on its last frame, and the width it was measured at (see `ui::chat`).
    pub heights: Vec<f32>,
    pub heights_width: f32,
    /// The composer takes the keyboard on the next frame.
    pub focus_composer: bool,
    /// The last thing the process wrote to stderr (shown when it dies without explanation).
    pub stderr: Option<String>,
    /// When the current turn started (for the "working 00:14" line).
    pub turn_started: Option<Instant>,
    /// When the last event arrived (the delivery rule waits for the stream to settle).
    pub last_event: Instant,
}

impl ChatTab {
    pub fn new(transcript: Transcript) -> Self {
        Self {
            proc: None,
            transcript,
            composer: String::new(),
            history: Vec::new(),
            history_pos: None,
            draft: String::new(),
            opened: HashMap::new(),
            markdown: egui_commonmark::CommonMarkCache::default(),
            heights: Vec::new(),
            heights_width: 0.0,
            focus_composer: true,
            stderr: None,
            turn_started: None,
            last_event: Instant::now(),
        }
    }
}

/// A message from another agent as the chat shows it: who sent it and the message itself, without the instructions on
/// how to answer that Holt adds for the model (`bridge::tab_prompt`). Anything that does not have that shape is shown whole.
pub(crate) fn agent_message_display(prompt: &str, from: &str) -> (Option<String>, String) {
    const HEAD: &str = "[Message from the agent \"";
    if let Some(rest) = prompt.strip_prefix(HEAD) {
        if let Some((who, rest)) = rest.split_once("\" received by Holt]") {
            let body = rest.trim_start_matches('\n');
            let body = body.split("\n\nAnswer by running in the shell:").next().unwrap_or(body);
            return (Some(who.to_owned()), body.trim().to_owned());
        }
    }
    ((!from.is_empty()).then(|| from.to_owned()), prompt.trim().to_owned())
}

impl Deck {
    /// The chat state for a new tab, if its view is Chat: with what the CLI saved of the conversation, when resumed.
    pub(super) fn new_chat_tab(&self, project: &str, ident: &Ident) -> Option<Box<ChatTab>> {
        self.core.tab_is_chat(project, ident).then(|| Box::new(ChatTab::new(self.core.chat_history(project, &ident.id()))))
    }

    /// Starts (or restarts) the process of a chat tab.
    pub(super) fn start_chat(&mut self, idx: usize) {
        let ident = self.tabs[idx].ident.clone();
        let project = self.tabs[idx].project.clone();
        let spec = self.core.spawn_spec_chat(&project, &ident);
        let wake = self.wake.clone();
        let spawned = spec.and_then(|s| self.core.spawn_chat(&s, wake).map(|p| (s, p)).map_err(|e| e.to_string()));
        let tab = &mut self.tabs[idx];
        tab.hooked = true;
        tab.turn_confirmed = false;
        tab.busy = false;
        tab.done = false;
        tab.idle_deadline = None;
        let Some(chat) = tab.chat.as_mut() else { return };
        // A previous process may have been killed mid-turn.
        chat.transcript.process_ended();
        chat.turn_started = None;
        chat.stderr = None;
        chat.last_event = Instant::now();
        match spawned {
            Ok((spec, proc)) => {
                chat.proc = Some(proc);
                chat.focus_composer = true;
                tab.alive = true;
                tab.spawned_at = Some(Instant::now());
                tab.model = spec.model.clone();
                let first = spec.first_message.clone();
                tab.spec = Some(spec);
                self.core.mail_tab_started(&project, &ident.id());
                if let Some(text) = first {
                    self.chat_send(idx, &text, &text, None);
                }
            }
            Err(msg) => {
                chat.proc = None;
                chat.transcript.push_notice(msg, true);
                tab.alive = false;
                tab.spec = None;
            }
        }
    }

    /// Sends `prompt` to the CLI of chat tab `idx`; the conversation shows `shown` (and who sent it, for another agent).
    pub(crate) fn chat_send(&mut self, idx: usize, prompt: &str, shown: &str, from: Option<&str>) {
        let tab = &mut self.tabs[idx];
        let Some(chat) = tab.chat.as_mut() else { return };
        let Some(proc) = chat.proc.as_ref() else { return };
        proc.send_user(prompt);
        chat.transcript.push_user(shown, from);
        chat.turn_started.get_or_insert_with(Instant::now);
        let (project, id) = (tab.project.clone(), tab.ident.id());
        self.on_agent_signal(&project, &id, true);
        if let Some(tab) = self.tabs.get_mut(idx) {
            // The stream tells when the turn ends: the "no output" release of the terminal does not apply.
            tab.idle_deadline = None;
        }
    }

    /// Types what the user wrote in the composer. A slash command that only works in the terminal is not sent.
    pub(crate) fn chat_submit(&mut self, idx: usize) {
        let Some(chat) = self.tabs[idx].chat.as_mut() else { return };
        let text = chat.composer.trim().to_owned();
        if text.is_empty() || chat.proc.is_none() {
            return;
        }
        chat.composer.clear();
        chat.history_pos = None;
        chat.draft.clear();
        if chat.history.last() != Some(&text) {
            chat.history.push(text.clone());
        }
        if let Some(name) = chat::terminal_only_command(&text, &chat.transcript.terminal_commands) {
            chat.transcript.push_user(&text, None);
            // Not a turn: the line is not sent, so nothing answers it.
            chat.transcript
                .push_notice(t!("/{name} opens a screen of the terminal. Use the Terminal view of this tab for it.", name), false);
            return;
        }
        self.chat_send(idx, &text, &text, None);
    }

    /// The user answered a permission card.
    pub(crate) fn chat_answer(&mut self, idx: usize, request_id: &str, allow: bool, for_session: bool) {
        let Some(chat) = self.tabs[idx].chat.as_mut() else { return };
        let card = chat.transcript.items.iter().find_map(|i| match i {
            Item::Permission(p) if p.request_id == request_id => Some(p.clone()),
            _ => None,
        });
        let (Some(card), Some(proc)) = (card, chat.proc.as_ref()) else { return };
        let rules: &[serde_json::Value] = if for_session { &card.suggestions } else { &[] };
        proc.answer_permission(request_id, allow, &card.input, "The user denied this action in Holt.", rules);
        chat.transcript.answer_permission(request_id, allow);
    }

    /// Cuts the running turn (Esc, the stop button).
    pub(crate) fn chat_interrupt(&mut self, idx: usize) {
        if let Some(proc) = self.tabs[idx].chat.as_ref().and_then(|c| c.proc.as_ref()) {
            proc.interrupt();
        }
    }

    /// Text that was typed or dictated for the tab (a dictation, attached files, a file clicked in the tree): in a chat tab
    /// it goes to the composer, so the user can read it before sending.
    pub(crate) fn chat_insert(&mut self, idx: usize, text: &str) -> bool {
        let Some(chat) = self.tabs.get_mut(idx).and_then(|t| t.chat.as_mut()) else { return false };
        if !chat.composer.is_empty() && !chat.composer.ends_with([' ', '\n']) {
            chat.composer.push(' ');
        }
        chat.composer.push_str(text);
        chat.focus_composer = true;
        true
    }

    /// Delivers a message from another agent (or from the interface) to a chat tab: it becomes a message of the
    /// conversation, with no typing and no Enter.
    pub(crate) fn chat_deliver(&mut self, idx: usize, prompt: &str, from: &str) {
        let (who, shown) = agent_message_display(prompt, from);
        self.chat_send(idx, prompt, &shown, who.as_deref());
    }

    /// Everything the processes of chat tabs said since the last frame. Returns the tabs whose process ended, with its code.
    pub(super) fn pump_chat(&mut self) -> Vec<(usize, i64)> {
        if self.debug_chat_demo && self.active.and_then(|i| self.tabs.get(i)).is_some_and(|t| t.chat.is_some()) {
            self.debug_chat_demo = false;
            self.fill_chat_demo();
        }
        let mut exits = Vec::new();
        for idx in 0..self.tabs.len() {
            let mut batch = Vec::new();
            if let Some(proc) = self.tabs[idx].chat.as_ref().and_then(|c| c.proc.as_ref()) {
                while let Some(msg) = proc.try_recv() {
                    batch.push(msg);
                }
            }
            for msg in batch {
                match msg {
                    ChatMsg::Event(ev) => self.on_chat_event(idx, ev),
                    ChatMsg::Stderr(line) => {
                        if let Some(chat) = self.tabs[idx].chat.as_mut() {
                            chat.stderr = Some(line);
                        }
                    }
                    ChatMsg::Exited(code) => {
                        if let Some(chat) = self.tabs[idx].chat.as_mut() {
                            chat.proc = None;
                        }
                        exits.push((idx, code.map_or(-1, i64::from)));
                    }
                }
            }
        }
        exits
    }

    fn on_chat_event(&mut self, idx: usize, ev: ChatEvent) {
        let (project, id) = (self.tabs[idx].project.clone(), self.tabs[idx].ident.id());
        let Some(chat) = self.tabs[idx].chat.as_mut() else { return };
        chat.last_event = Instant::now();
        match &ev {
            // `/clear` starts a new conversation: the saved id follows.
            ChatEvent::Init { session, .. } if !session.is_empty() => {
                if self.core.tab_session(&project, &id).as_deref() != Some(session.as_str()) {
                    self.core.set_tab_session(&project, &id, session);
                }
            }
            // The chat has no screen for the questions of `AskUserQuestion`: tell the model to ask in plain text.
            ChatEvent::Permission(p) if p.tool == "AskUserQuestion" => {
                if let Some(proc) = chat.proc.as_ref() {
                    let msg = "The user cannot answer multiple-choice questions in this view. Ask the question as plain text instead.";
                    proc.answer_permission(&p.request_id, false, &p.input, msg, &[]);
                }
                return;
            }
            ChatEvent::Permission(_) => {
                // The agent is waiting for the user: same marker as a finished turn, and the chime.
                if self.active != Some(idx) || self.docs.view == CenterView::Docs {
                    self.tabs[idx].done = true;
                }
                self.play_sound(false);
            }
            _ => {}
        }
        let Some(chat) = self.tabs[idx].chat.as_mut() else { return };
        chat.transcript.apply(&ev);
        if let ChatEvent::Result(_) = ev {
            if !chat.transcript.busy() {
                chat.turn_started = None;
                self.on_agent_signal(&project, &id, false);
                self.core.mail_turn_ended(&project, &id);
                if let Some(tab) = self.tabs.get_mut(idx) {
                    tab.idle_deadline = None;
                }
            }
        }
    }

    /// The tab's process ended with `code`: the conversation says so, and the composer offers to start it again.
    pub(super) fn chat_ended(&mut self, idx: usize, code: i64) {
        let Some(chat) = self.tabs[idx].chat.as_mut() else { return };
        chat.transcript.process_ended();
        chat.turn_started = None;
        let why = chat.stderr.take().map(|s| format!(": {s}")).unwrap_or_default();
        chat.transcript.push_notice(t!("The process ended (code {code}){why}.", code, why), code != 0);
    }

    /// Switches the tab between the chat and the terminal view; the conversation continues in the other (same session).
    pub fn set_tab_view(&mut self, idx: usize, chat: bool) {
        let tab = &self.tabs[idx];
        if tab.chat.is_some() == chat || !chat::supports(&tab.kind) {
            return;
        }
        let (project, ident) = (tab.project.clone(), tab.ident.clone());
        self.core.set_tab_chat(&project, &ident.id(), chat);
        let tab = &mut self.tabs[idx];
        tab.session.kill();
        // Questions still in the queue wait for the new process; the one that was killed is not "ended" for the core.
        tab.chat = None;
        if chat {
            let transcript = self.core.chat_history(&project, &ident.id());
            self.tabs[idx].chat = Some(Box::new(ChatTab::new(transcript)));
        }
        self.start(idx);
    }

    /// `HOLT_DEBUG_UI=chat-demo`: fills the active chat tab with a made-up conversation, to look at the view.
    fn fill_chat_demo(&mut self) {
        use holt_core::chat::{PermState, PermissionCard, ToolCall, ToolOutcome};
        use serde_json::json;
        let Some(chat) = self.active.and_then(|i| self.tabs.get_mut(i)).and_then(|t| t.chat.as_mut()) else { return };
        let ok = |id: &str, text: &str| Some(ToolOutcome { tool_use_id: id.into(), text: text.into(), is_error: false });
        let tool = |id: &str, name: &str, input: serde_json::Value, outcome| {
            Item::Tool(ToolCall { id: id.into(), name: name.into(), input, outcome })
        };
        let t = &mut chat.transcript;
        t.model = "claude-sonnet-5-5".into();
        t.cost_usd = 0.12;
        t.commands = ["context", "compact", "cost", "clear", "review"].iter().map(|s| (*s).to_owned()).collect();
        t.items = vec![
            Item::User { text: "The tab menu has no \"close others\". Add it, and keep the order of the remaining tabs.".into(), from: None },
            Item::Assistant { message: "m1".into(), text: "I'll look at how the tab context menu is built first.".into(), streaming: false },
            tool("t1", "Grep", json!({ "pattern": "tab_context_menu", "path": "crates/app/src" }), ok("t1", "3 matches")),
            tool("t2", "Read", json!({ "file_path": "crates/app/src/ui/menus.rs" }), ok("t2", "fn tab_context_menu(...)")),
            Item::Assistant {
                message: "m2".into(),
                text: "The menu is a plain list of `MenuItem`s and every action goes through `deck.close_tab(idx)`.\n\n- add one entry that closes every other tab of the pane\n- keep the index of the survivor stable\n\n```rust\nitems.push((TabAction::CloseOthers, t!(\"Close others\"), None));\n```".into(),
                streaming: false,
            },
            tool(
                "t3",
                "Edit",
                json!({ "file_path": "crates/app/src/ui/menus.rs", "old_string": "    items.push((TabAction::Close, t!(\"Close\"), None));", "new_string": "    items.push((TabAction::Close, t!(\"Close\"), None));\n    if siblings > 1 {\n        items.push((TabAction::CloseOthers, t!(\"Close others\"), None));\n    }" }),
                ok("t3", "updated"),
            ),
            Item::User { text: "Which key did you pick for the new menu entry? I'm documenting it in the manual tests.".into(), from: Some("claude@dev".into()) },
            Item::Assistant { message: "m3".into(), text: "There is no shortcut yet. I'll answer claude@dev with that, then run the tests once you allow them.".into(), streaming: false },
            tool("t4", "Bash", json!({ "command": "cargo test -p holt tabs::" }), None),
            Item::Permission(PermissionCard {
                request_id: "r1".into(),
                tool_use_id: "t4".into(),
                tool: "Bash".into(),
                input: json!({ "command": "cargo test -p holt tabs::" }),
                description: String::new(),
                suggestions: vec![json!({ "type": "setMode", "mode": "acceptEdits", "destination": "session" })],
                state: PermState::Pending,
            }),
        ];
        chat.heights.clear();
    }

    /// Chat view tabs of the process list, for `shutdown`.
    pub(super) fn kill_chats(&mut self) {
        for t in &mut self.tabs {
            if let Some(proc) = t.chat.as_ref().and_then(|c| c.proc.as_ref()) {
                proc.kill();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_question_from_another_agent_is_shown_without_the_instructions_for_the_model() {
        let prompt = "[Message from the agent \"claude@dev\" received by Holt]\n\nWhich key did you pick?\n\nAnswer by running in the shell: ask-agent --reply 7 \"your answer\"\n(the answer only reaches...)";
        let (who, text) = agent_message_display(prompt, "claude@dev");
        assert_eq!((who.as_deref(), text.as_str()), (Some("claude@dev"), "Which key did you pick?"));
    }

    #[test]
    fn other_deliveries_are_shown_whole() {
        let (who, text) = agent_message_display("[mail #3 from claude@dev: \"x\" — read with: ask-agent --read 3]", "claude@dev");
        assert_eq!(who.as_deref(), Some("claude@dev"));
        assert!(text.starts_with("[mail #3"));
        let (who, text) = agent_message_display("Review PR 4", "");
        assert_eq!((who, text.as_str()), (None, "Review PR 4"));
    }
}
