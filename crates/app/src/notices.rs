//! System notices: what the core and the interface tell the person ("Could not transcribe…", a download that finished,
//! a task ready for review). Each one shows as a toast in the corner of the window and stays in the history behind the
//! bell in the title bar. The painting lives in `ui/notices.rs`; here are the list, the toasts' clock and the pure rules.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use holt_core::{FeedEntry, FeedKind, Tone};

/// Notices kept in the history (the oldest leave).
pub const MAX_NOTICES: usize = 100;
/// Toasts on screen at once; an older one leaves the corner early (it stays in the history).
pub const MAX_TOASTS: usize = 3;

/// One notice of the history.
#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    pub id: u64,
    pub tone: Tone,
    pub text: String,
    /// Project it came from (`None` = global, like the Whisper notices).
    pub project: Option<String>,
    /// The task it is about: the notice opens it.
    pub task: Option<String>,
    pub at_ms: u64,
    /// The person already saw it (opened the history, closed or clicked its toast, or the toast left by itself).
    pub read: bool,
}

/// A toast on screen and when it leaves (`None` = only when closed).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Toast {
    pub id: u64,
    pub until: Option<Instant>,
}

/// How long a toast of each tone stays (`None` = until closed: an error is not missed while the person looks away).
pub fn toast_life(tone: Tone) -> Option<Duration> {
    match tone {
        Tone::Info | Tone::Success => Some(Duration::from_secs(6)),
        Tone::Warning => Some(Duration::from_secs(10)),
        Tone::Error => None,
    }
}

/// The text a feed entry becomes, or `None` when it is not a notice. Questions and answers between agents live in the
/// task's timeline (and in the pair's history); the "thinking" lines and the person's own messages are not news.
/// An agent error names who failed whom.
pub fn notice_text(e: &FeedEntry) -> Option<String> {
    match e.kind {
        FeedKind::System => Some(e.text.clone()),
        FeedKind::Error => Some(match (e.from.as_deref(), e.to.as_deref()) {
            (Some(from), Some(to)) => format!("{from} → {to}: {}", e.text),
            (Some(from), None) => format!("{from}: {}", e.text),
            _ => e.text.clone(),
        }),
        FeedKind::Ask | FeedKind::Answer | FeedKind::User | FeedKind::Thinking => None,
    }
}

#[derive(Debug, Default)]
pub struct Notices {
    /// Oldest first.
    pub list: VecDeque<Notice>,
    pub toasts: Vec<Toast>,
    /// The history popover is open.
    pub open: bool,
    next_id: u64,
}

impl Notices {
    /// Adds a notice and its toast. Returns its id.
    pub fn push(&mut self, tone: Tone, text: String, project: Option<String>, task: Option<String>, at_ms: u64, now: Instant) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.list.push_back(Notice { id, tone, text, project, task, at_ms, read: self.open });
        while self.list.len() > MAX_NOTICES {
            if let Some(old) = self.list.pop_front() {
                self.toasts.retain(|t| t.id != old.id);
            }
        }
        // With the history open the person is already looking at it: no toast on top.
        if !self.open {
            self.toasts.push(Toast { id, until: toast_life(tone).map(|d| now + d) });
            while self.toasts.len() > MAX_TOASTS {
                let gone = self.toasts.remove(0);
                self.expire(gone.id);
            }
        }
        id
    }

    pub fn get(&self, id: u64) -> Option<&Notice> {
        self.list.iter().find(|n| n.id == id)
    }

    /// Unread notices that ask for attention (the bell's count): warnings and errors. Info and success notices
    /// are read when their toast leaves.
    pub fn unread(&self) -> usize {
        self.list.iter().filter(|n| !n.read && matches!(n.tone, Tone::Warning | Tone::Error)).count()
    }

    /// The worst tone among the unread ones (the colour of the bell's count).
    pub fn unread_tone(&self) -> Option<Tone> {
        let unread = self.list.iter().filter(|n| !n.read);
        let mut worst = None;
        for n in unread {
            worst = match (worst, n.tone) {
                (_, Tone::Error) => Some(Tone::Error),
                (Some(Tone::Error), _) => worst,
                (_, Tone::Warning) => Some(Tone::Warning),
                (w, _) => w,
            };
        }
        worst
    }

    /// Removes the toasts whose time is up. A toast that left by itself counts as seen, except warnings and errors.
    /// Returns the next moment something leaves (to schedule the repaint).
    pub fn tick(&mut self, now: Instant) -> Option<Instant> {
        let gone: Vec<u64> = self.toasts.iter().filter(|t| t.until.is_some_and(|u| u <= now)).map(|t| t.id).collect();
        for id in gone {
            self.toasts.retain(|t| t.id != id);
            self.expire(id);
        }
        self.toasts.iter().filter_map(|t| t.until).min()
    }

    /// The pointer is over the toast: it does not leave while the person reads it (at least 2 s more after).
    pub fn hold(&mut self, id: u64, now: Instant) {
        if let Some(t) = self.toasts.iter_mut().find(|t| t.id == id) {
            if let Some(u) = t.until.as_mut() {
                *u = (*u).max(now + Duration::from_secs(2));
            }
        }
    }

    /// The person closed the toast (or clicked it): it leaves and counts as read.
    pub fn dismiss(&mut self, id: u64) {
        self.toasts.retain(|t| t.id != id);
        if let Some(n) = self.list.iter_mut().find(|n| n.id == id) {
            n.read = true;
        }
    }

    /// Opens or closes the history. Opening marks everything as read and clears the corner.
    pub fn toggle(&mut self) {
        self.open = !self.open;
        if self.open {
            self.toasts.clear();
            for n in &mut self.list {
                n.read = true;
            }
        }
    }

    pub fn clear(&mut self) {
        self.list.clear();
        self.toasts.clear();
    }

    /// Forgets the notices of a project that was closed.
    pub fn forget_project(&mut self, project: &str) {
        self.list.retain(|n| n.project.as_deref() != Some(project));
        let ids: Vec<u64> = self.list.iter().map(|n| n.id).collect();
        self.toasts.retain(|t| ids.contains(&t.id));
    }

    fn expire(&mut self, id: u64) {
        if let Some(n) = self.list.iter_mut().find(|n| n.id == id) {
            if matches!(n.tone, Tone::Info | Tone::Success) {
                n.read = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push(n: &mut Notices, tone: Tone, now: Instant) -> u64 {
        n.push(tone, format!("{tone:?}"), None, None, 0, now)
    }

    #[test]
    fn only_system_notices_and_agent_errors_become_notices() {
        let sys = FeedEntry::system("Local transcription: model ready.");
        assert_eq!(notice_text(&sys).as_deref(), Some("Local transcription: model ready."));
        let err = FeedEntry::new(FeedKind::Error, Some("opencode"), Some("claude@t4"), "No quota.");
        assert_eq!(notice_text(&err).as_deref(), Some("opencode → claude@t4: No quota."));
        assert_eq!(err.tone, Tone::Error);
        for kind in [FeedKind::Ask, FeedKind::Answer, FeedKind::User, FeedKind::Thinking] {
            assert_eq!(notice_text(&FeedEntry::new(kind, Some("a"), Some("b"), "x")), None);
        }
    }

    #[test]
    fn toasts_leave_by_tone_and_errors_stay() {
        let t0 = Instant::now();
        let mut n = Notices::default();
        let info = push(&mut n, Tone::Info, t0);
        let warn = push(&mut n, Tone::Warning, t0);
        let err = push(&mut n, Tone::Error, t0);
        assert_eq!(n.tick(t0 + Duration::from_secs(1)), Some(t0 + Duration::from_secs(6)));
        n.tick(t0 + Duration::from_secs(7));
        assert_eq!(n.toasts.iter().map(|t| t.id).collect::<Vec<_>>(), [warn, err]);
        n.tick(t0 + Duration::from_secs(60));
        assert_eq!(n.toasts.iter().map(|t| t.id).collect::<Vec<_>>(), [err]);
        // The info that left by itself is read; the warning is not (it waits in the bell).
        assert!(n.get(info).unwrap().read);
        assert!(!n.get(warn).unwrap().read);
        assert_eq!(n.unread(), 2);
        assert_eq!(n.unread_tone(), Some(Tone::Error));
        n.dismiss(err);
        assert!(n.toasts.is_empty());
        assert_eq!(n.unread(), 1);
        assert_eq!(n.unread_tone(), Some(Tone::Warning));
    }

    #[test]
    fn hovering_holds_a_toast() {
        let t0 = Instant::now();
        let mut n = Notices::default();
        let id = push(&mut n, Tone::Success, t0);
        n.hold(id, t0 + Duration::from_secs(5));
        n.tick(t0 + Duration::from_secs(6));
        assert_eq!(n.toasts.len(), 1);
        n.tick(t0 + Duration::from_secs(8));
        assert!(n.toasts.is_empty());
    }

    #[test]
    fn the_corner_keeps_the_newest_toasts_and_the_history_is_capped() {
        let t0 = Instant::now();
        let mut n = Notices::default();
        let ids: Vec<u64> = (0..5).map(|_| push(&mut n, Tone::Info, t0)).collect();
        assert_eq!(n.toasts.iter().map(|t| t.id).collect::<Vec<_>>(), ids[2..]);
        assert!(n.get(ids[0]).unwrap().read);
        for _ in 0..MAX_NOTICES {
            push(&mut n, Tone::Info, t0);
        }
        assert_eq!(n.list.len(), MAX_NOTICES);
        assert!(n.get(ids[4]).is_none());
    }

    #[test]
    fn opening_the_history_reads_everything_and_clears_the_corner() {
        let t0 = Instant::now();
        let mut n = Notices::default();
        push(&mut n, Tone::Error, t0);
        push(&mut n, Tone::Warning, t0);
        n.toggle();
        assert!(n.toasts.is_empty());
        assert_eq!(n.unread(), 0);
        // While it is open, new notices land there directly.
        push(&mut n, Tone::Error, t0);
        assert!(n.toasts.is_empty());
        assert_eq!(n.unread(), 0);
    }

    #[test]
    fn closing_a_project_forgets_its_notices() {
        let t0 = Instant::now();
        let mut n = Notices::default();
        n.push(Tone::Info, "a".into(), Some("p1".into()), None, 0, t0);
        let keep = n.push(Tone::Info, "b".into(), Some("p2".into()), None, 0, t0);
        n.forget_project("p1");
        assert_eq!(n.list.iter().map(|x| x.id).collect::<Vec<_>>(), [keep]);
        assert_eq!(n.toasts.iter().map(|t| t.id).collect::<Vec<_>>(), [keep]);
    }
}
