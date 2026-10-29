//! When to type another agent's message into a tab's terminal.
//!
//! The `ask-agent` question is typed into the instance's terminal (see `bridge.rs`). Typing at the wrong
//! moment mixes the text with what the user is typing or with the answer the agent is still writing. This is
//! the rule, without any interface, for the UI to consult on every frame.
//!
//! Signals used, from the most reliable to the least:
//! 1. **The user's keyboard**: nothing is typed right after a key, and a much longer wait applies when there
//!    is started text not yet sent.
//! 2. **Agent hooks** (`ask-agent --signal busy|idle`, wired into Claude and Codex): with them, the tab is
//!    only free after the end-of-turn notice.
//! 3. **Output heuristic**: without hooks, the tab is free when the terminal has been still for
//!    [`READY_IDLE`]. If it never settles, the message goes after [`READY_TIMEOUT`] in the queue
//!    (counted from when it *entered the queue*, not from when the tab opened).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Without hooks, the terminal must stay still for this long.
pub const READY_IDLE: Duration = Duration::from_millis(1500);
/// With hooks, it is enough for the output to settle after the end-of-turn notice.
pub const HOOK_SETTLE: Duration = Duration::from_millis(300);
/// Without hooks, if the terminal never settles, the message is typed after this long in the queue.
pub const READY_TIMEOUT: Duration = Duration::from_secs(30);
/// After a user key (with no pending text: Enter, Ctrl+C, arrows…).
pub const USER_QUIET: Duration = Duration::from_secs(2);
/// After a user key that left started text on the line (no Enter).
pub const TYPING_QUIET: Duration = Duration::from_secs(20);
/// A freshly opened tab (without hooks): nothing is typed before this, even if the output has already
/// settled. A TUI may stay quiet for more than [`READY_IDLE`] between the first drawing and the input field
/// being up (loading a session, MCP, model); text pasted in that window is lost.
pub const BOOT_GRACE: Duration = Duration::from_secs(5);
/// Interval to check again while the tab is busy.
pub const POLL: Duration = Duration::from_millis(300);
/// On Windows Codex, how much longer the Enter waits per pasted character (see [`enter_delay`]).
pub const CODEX_ENTER_PER_CHAR: Duration = Duration::from_millis(1);
/// Cap on the Enter wait: an enormous text does not hold the tab forever.
pub const MAX_ENTER_DELAY: Duration = Duration::from_secs(10);

/// How long the Enter waits after pasting `text` into the tab of CLI `kind` (`base` for almost all).
///
/// On Windows Codex does not receive the paste as a paste: it reads it key by key and treats an Enter that
/// arrives before it finishes as a line break, and the message stays stuck in the field ("[Pasted Content N
/// chars]"). Measured on 2026-10-25 (Codex 0.164.0): 1,735 characters lose the Enter at 600 ms and catch it
/// at 1,200 ms; 3,593 lose it at 1,200 and catch it at 2,400. The wait grows 1 ms per character, with ~1.5×
/// of margin.
pub fn enter_delay(kind: &str, text: &str, base: Duration) -> Duration {
    if kind != "codex" || !cfg!(windows) {
        return base;
    }
    let chars = u32::try_from(text.chars().count()).unwrap_or(u32::MAX);
    (base + CODEX_ENTER_PER_CHAR.saturating_mul(chars)).min(MAX_ENTER_DELAY)
}

/// What the UI knows about the tab right now.
#[derive(Debug, Clone, Copy)]
pub struct TabState {
    /// The process is running.
    pub alive: bool,
    /// The agent is in the middle of a turn (the user's Enter, a sent message or the `busy` hook).
    pub busy: bool,
    /// The tab has already received some hook notice: `busy` comes from the agent, not from the heuristic.
    pub hooked: bool,
    /// When the tab's process was started.
    pub spawned_at: Option<Instant>,
    /// Last output from the process.
    pub last_output: Option<Instant>,
    /// Last user key in this tab.
    pub last_key: Option<Instant>,
    /// The user started typing something and has not sent it yet (nor erased it with Ctrl+C/Ctrl+U/Esc).
    pub typing: bool,
}

/// Decision for the first message in the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// It can be typed now.
    Ready,
    /// Check again at this instant.
    WaitUntil(Instant),
    /// There is no way to deliver (the tab is not running).
    Fail,
}

/// Can a message that has been in the queue since `queued_at` be typed into the tab's terminal?
pub fn gate(tab: &TabState, queued_at: Instant, now: Instant) -> Gate {
    if !tab.alive {
        return Gate::Fail;
    }
    if let Some(key) = tab.last_key {
        let quiet = if tab.typing { TYPING_QUIET } else { USER_QUIET };
        if now < key + quiet {
            return Gate::WaitUntil(key + quiet);
        }
    }
    if tab.busy {
        return Gate::WaitUntil(now + POLL);
    }
    // With hooks the agent has already proved it is up; without them, give the TUI time to come up.
    if let Some(boot) = tab.spawned_at.filter(|_| !tab.hooked).map(|at| at + BOOT_GRACE) {
        if now < boot {
            return Gate::WaitUntil(boot);
        }
    }
    let settle = if tab.hooked { HOOK_SETTLE } else { READY_IDLE };
    match tab.last_output {
        Some(out) if now >= out + settle => Gate::Ready,
        _ if !tab.hooked && now >= queued_at + READY_TIMEOUT => Gate::Ready,
        Some(out) if tab.hooked => Gate::WaitUntil(out + settle),
        Some(out) => Gate::WaitUntil((out + settle).min(queued_at + READY_TIMEOUT)),
        // With hooks and no output yet: the TUI has drawn nothing, wait.
        None if tab.hooked => Gate::WaitUntil(now + POLL),
        None => Gate::WaitUntil(queued_at + READY_TIMEOUT),
    }
}

/// A tab's message queue, with the instant each message entered it.
#[derive(Debug)]
pub struct Mailbox<T> {
    items: VecDeque<(Instant, T)>,
}

impl<T> Default for Mailbox<T> {
    fn default() -> Self {
        Self { items: VecDeque::new() }
    }
}

impl<T> Mailbox<T> {
    pub fn push(&mut self, now: Instant, item: T) {
        self.items.push_back((now, item));
    }

    /// When the first message entered the queue.
    pub fn front_since(&self) -> Option<Instant> {
        self.items.front().map(|(t, _)| *t)
    }

    pub fn pop(&mut self) -> Option<T> {
        self.items.pop_front().map(|(_, item)| item)
    }

    pub fn drain(&mut self) -> impl Iterator<Item = T> + '_ {
        self.items.drain(..).map(|(_, item)| item)
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The messages in queue order.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.items.iter().map(|(_, item)| item)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }
}

/// Effect of a user key on "is there started text on the line?".
/// `bytes` is what the key sends to the process.
pub fn typing_after(bytes: &[u8], was_typing: bool) -> bool {
    match bytes {
        // Enter sends; Ctrl+C, Ctrl+U and Esc (alone) usually clear the line.
        b"\r" | b"\n" | [0x03] | [0x15] | [0x1b] => false,
        // Control keys and sequences (arrows, Backspace…) do not change the state.
        [b] if *b < 0x20 || *b == 0x7f => was_typing,
        [0x1b, ..] => was_typing,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn the_codex_enter_waits_for_the_paste_on_windows() {
        let base = ms(250);
        let long = "x".repeat(1735);
        assert_eq!(enter_delay("claude", &long, base), base, "only Codex waits longer");
        if cfg!(windows) {
            assert_eq!(enter_delay("codex", "hi", base), ms(252));
            assert!(enter_delay("codex", &long, base) > ms(1200), "1,735 characters caught it at 1,200 ms");
            assert!(enter_delay("codex", &"x".repeat(3593), base) > ms(2400), "3,593 characters caught it at 2,400 ms");
            assert_eq!(enter_delay("codex", &"x".repeat(200_000), base), MAX_ENTER_DELAY);
        } else {
            assert_eq!(enter_delay("codex", &long, base), base, "outside Windows the paste arrives whole");
        }
    }

    fn idle_tab(t0: Instant) -> TabState {
        TabState { alive: true, busy: false, hooked: false, spawned_at: None, last_output: Some(t0), last_key: None, typing: false }
    }

    #[test]
    fn a_dead_tab_fails() {
        let t0 = Instant::now();
        let tab = TabState { alive: false, ..idle_tab(t0) };
        assert_eq!(gate(&tab, t0, t0 + ms(5000)), Gate::Fail);
    }

    #[test]
    fn without_hooks_the_terminal_must_be_quiet() {
        let t0 = Instant::now();
        let tab = idle_tab(t0);
        assert_eq!(gate(&tab, t0, t0 + ms(1000)), Gate::WaitUntil(t0 + READY_IDLE));
        assert_eq!(gate(&tab, t0, t0 + READY_IDLE), Gate::Ready);
    }

    #[test]
    fn the_timeout_counts_from_the_queue_not_from_the_spawn() {
        // The tab has existed for hours and is still writing: before, the message went out right away (the
        // deadline counted from when the tab opened). Now it waits to settle or READY_TIMEOUT in the queue.
        let t0 = Instant::now() + Duration::from_secs(3600);
        let queued = t0;
        let writing = |now: Instant| TabState { last_output: Some(now), ..idle_tab(t0) };
        let now = queued + ms(100);
        assert_eq!(gate(&writing(now), queued, now), Gate::WaitUntil(now + READY_IDLE));
        let now = queued + READY_TIMEOUT;
        assert_eq!(gate(&writing(now), queued, now), Gate::Ready, "never settled: it goes anyway");
    }

    #[test]
    fn a_busy_tab_waits_even_after_the_timeout() {
        let t0 = Instant::now();
        let tab = TabState { busy: true, ..idle_tab(t0) };
        let now = t0 + READY_TIMEOUT * 4;
        assert_eq!(gate(&tab, t0, now), Gate::WaitUntil(now + POLL));
    }

    #[test]
    fn user_keys_hold_the_delivery() {
        let t0 = Instant::now();
        let now = t0 + ms(10_000);
        // Key 1 s ago, no started text: waits for USER_QUIET to complete.
        let tab = TabState { last_key: Some(now - ms(1000)), ..idle_tab(t0) };
        assert_eq!(gate(&tab, t0, now), Gate::WaitUntil(now - ms(1000) + USER_QUIET));
        // Text started and not sent: waits much longer, and the queue timeout does not break that.
        let tab = TabState { last_key: Some(now - ms(1000)), typing: true, ..idle_tab(t0) };
        let late = t0 + READY_TIMEOUT * 2;
        let tab_late = TabState { last_key: Some(late - ms(1000)), ..tab };
        assert_eq!(gate(&tab_late, t0, late), Gate::WaitUntil(late - ms(1000) + TYPING_QUIET));
        assert_eq!(gate(&tab, t0, now - ms(1000) + TYPING_QUIET), Gate::Ready);
    }

    #[test]
    fn with_hooks_only_the_end_of_turn_frees_the_tab() {
        let t0 = Instant::now();
        let hooked = TabState { hooked: true, ..idle_tab(t0) };
        // A long pause without output in the middle of the turn (the hook said busy): keeps waiting.
        let busy = TabState { busy: true, ..hooked };
        assert!(matches!(gate(&busy, t0, t0 + ms(60_000)), Gate::WaitUntil(_)));
        // End of turn: it is enough for the output to settle for HOOK_SETTLE.
        assert_eq!(gate(&hooked, t0, t0 + ms(100)), Gate::WaitUntil(t0 + HOOK_SETTLE));
        assert_eq!(gate(&hooked, t0, t0 + HOOK_SETTLE), Gate::Ready);
        // With hooks there is no "it goes anyway": with no output at all, it waits.
        let blank = TabState { last_output: None, ..hooked };
        assert!(matches!(gate(&blank, t0, t0 + READY_TIMEOUT * 2), Gate::WaitUntil(_)));
    }

    #[test]
    fn a_fresh_tab_waits_for_the_boot_grace_even_when_quiet() {
        let t0 = Instant::now();
        // Output still for 2 s (> READY_IDLE), but the process came up 2 s ago: the TUI may still be loading.
        let tab = TabState { spawned_at: Some(t0), last_output: Some(t0), ..idle_tab(t0) };
        let now = t0 + ms(2000);
        assert_eq!(gate(&tab, t0, now), Gate::WaitUntil(t0 + BOOT_GRACE));
        assert_eq!(gate(&tab, t0, t0 + BOOT_GRACE), Gate::Ready);
        // With hooks the agent has already introduced itself: only the settled output matters.
        let hooked = TabState { hooked: true, ..tab };
        assert_eq!(gate(&hooked, t0, t0 + HOOK_SETTLE), Gate::Ready);
    }

    #[test]
    fn a_tab_that_never_printed_waits_for_the_timeout() {
        let t0 = Instant::now();
        let tab = TabState { last_output: None, ..idle_tab(t0) };
        assert_eq!(gate(&tab, t0, t0 + ms(10)), Gate::WaitUntil(t0 + READY_TIMEOUT));
        assert_eq!(gate(&tab, t0, t0 + READY_TIMEOUT), Gate::Ready);
    }

    #[test]
    fn typing_state_follows_the_keys() {
        assert!(typing_after(b"a", false));
        assert!(typing_after("ç".as_bytes(), false));
        assert!(!typing_after(b"\r", true));
        assert!(!typing_after(&[0x03], true), "Ctrl+C");
        assert!(!typing_after(&[0x15], true), "Ctrl+U");
        assert!(!typing_after(&[0x1b], true), "Esc");
        assert!(typing_after(&[0x7f], true), "Backspace does not know whether it erased everything");
        assert!(typing_after(b"\x1b[D", true), "arrow");
        assert!(!typing_after(b"\x1b[D", false));
    }

    #[test]
    fn the_mailbox_keeps_order_and_arrival_time() {
        let t0 = Instant::now();
        let mut mb = Mailbox::default();
        assert!(mb.front_since().is_none());
        mb.push(t0, "a");
        mb.push(t0 + ms(5), "b");
        assert_eq!((mb.front_since(), mb.len()), (Some(t0), 2));
        assert_eq!(mb.pop(), Some("a"));
        assert_eq!(mb.front_since(), Some(t0 + ms(5)));
        assert_eq!(mb.drain().collect::<Vec<_>>(), ["b"]);
        assert!(mb.is_empty());
    }
}
