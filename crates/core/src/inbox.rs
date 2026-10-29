//! Inbox of answers (`ask-agent --inbox`).
//!
//! Every question from an agent to a tab that finishes (with an answer, on time or later, or with a failure)
//! leaves an entry here, in the box of whoever asked. The entry is born "not delivered" and becomes
//! delivered when the answer actually reaches that agent: the bridge's 200 was written without error, or the
//! interface typed the late answer into its terminal. If none of that happened (the tab closed, the connection
//! had already been abandoned), the answer stays here, and `ask-agent --inbox` shows it. The inbox lives in
//! the app's memory: it keeps the last [`INBOX_LIMIT`] answers of each instance.

use std::collections::{HashMap, VecDeque};

use parking_lot::Mutex;
use serde::Serialize;

/// Answers kept per instance that asked.
pub const INBOX_LIMIT: usize = 20;

/// An answer received by whoever asked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InboxEntry {
    /// Question number.
    pub request: u64,
    /// Who answered (instance id, without the project).
    pub from: String,
    /// The answer text, or the reason for the failure.
    pub text: String,
    /// The question ended without an answer (`text` is the reason).
    pub failed: bool,
    /// Epoch in milliseconds.
    pub ts: u64,
    /// The answer already reached whoever asked.
    pub delivered: bool,
}

/// Boxes per instance that asked (key with the project scope: `instances::scoped`).
#[derive(Default)]
pub struct Inbox {
    boxes: Mutex<HashMap<String, VecDeque<InboxEntry>>>,
}

impl Inbox {
    /// Stores the answer in `asker`'s box. A question that already has an entry there is replaced; going
    /// past [`INBOX_LIMIT`], the oldest one leaves.
    pub fn push(&self, asker: &str, entry: InboxEntry) {
        let mut boxes = self.boxes.lock();
        let list = boxes.entry(asker.to_owned()).or_default();
        list.retain(|e| e.request != entry.request);
        list.push_back(entry);
        while list.len() > INBOX_LIMIT {
            list.pop_front();
        }
    }

    /// Marks the answer to question `request` as delivered. `false` = it is in no box.
    pub fn mark_delivered(&self, request: u64) -> bool {
        let mut boxes = self.boxes.lock();
        let entry = boxes.values_mut().flat_map(|l| l.iter_mut()).find(|e| e.request == request);
        entry.map(|e| e.delivered = true).is_some()
    }

    /// The instance `old` was renamed to `new` (scoped keys): the box goes along, after whatever already
    /// exists in `new`, so that the renamed tab's `--inbox` finds the answers and another tab opened later
    /// with the old name does not read them.
    pub fn rename(&self, old: &str, new: &str) {
        let mut boxes = self.boxes.lock();
        let Some(moved) = boxes.remove(old) else { return };
        let list = boxes.entry(new.to_owned()).or_default();
        list.extend(moved);
        while list.len() > INBOX_LIMIT {
            list.pop_front();
        }
    }

    /// The answer to question `request` and whom it belongs to (`asker` key).
    pub fn find(&self, request: u64) -> Option<(String, InboxEntry)> {
        let boxes = self.boxes.lock();
        boxes.iter().find_map(|(asker, l)| l.iter().find(|e| e.request == request).map(|e| (asker.clone(), e.clone())))
    }

    /// `asker`'s answers, oldest first: only the undelivered ones, or the [`INBOX_LIMIT`] with `all`.
    /// Whoever asks for the undelivered ones receives them here, so they become delivered.
    pub fn read(&self, asker: &str, all: bool) -> Vec<InboxEntry> {
        let mut boxes = self.boxes.lock();
        let Some(list) = boxes.get_mut(asker) else { return Vec::new() };
        let shown: Vec<InboxEntry> = list.iter().filter(|e| all || !e.delivered).cloned().collect();
        for e in list.iter_mut() {
            e.delivered = true;
        }
        shown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(request: u64) -> InboxEntry {
        InboxEntry { request, from: "antigravity".into(), text: format!("answer {request}"), failed: false, ts: 1, delivered: false }
    }

    #[test]
    fn it_keeps_the_last_twenty_answers_of_each_asker() {
        let inbox = Inbox::default();
        for n in 0..25 {
            inbox.push("p::claude", entry(n));
        }
        inbox.push("p::opencode", entry(100));
        let all = inbox.read("p::claude", true);
        assert_eq!(all.len(), INBOX_LIMIT);
        assert_eq!(all.first().map(|e| e.request), Some(5));
        assert_eq!(all.last().map(|e| e.request), Some(24));
        assert_eq!(inbox.read("p::opencode", true).len(), 1);
        assert!(inbox.read("p::ninguem", true).is_empty());
    }

    #[test]
    fn renaming_moves_the_box_to_the_new_name() {
        let inbox = Inbox::default();
        inbox.push("p::claude", entry(1));
        inbox.push("p::claude@dev", entry(2));
        inbox.rename("p::claude", "p::claude@dev");
        let got: Vec<u64> = inbox.read("p::claude@dev", false).iter().map(|e| e.request).collect();
        assert_eq!(got, [2, 1]);
        assert!(inbox.read("p::claude", true).is_empty(), "another tab opened with the old name does not read the answers");
        inbox.rename("p::ninguem", "p::outro");
        assert!(inbox.read("p::outro", true).is_empty());
    }

    #[test]
    fn delivered_answers_leave_the_default_listing_but_stay_in_all() {
        let inbox = Inbox::default();
        inbox.push("p::claude", entry(1));
        inbox.push("p::claude", entry(2));
        assert!(inbox.mark_delivered(1));
        assert!(!inbox.mark_delivered(9));
        let pending: Vec<u64> = inbox.read("p::claude", false).iter().map(|e| e.request).collect();
        assert_eq!(pending, [2]);
        // Reading the pending ones delivers them: the next read does not repeat them.
        assert!(inbox.read("p::claude", false).is_empty());
        let all: Vec<u64> = inbox.read("p::claude", true).iter().map(|e| e.request).collect();
        assert_eq!(all, [1, 2]);
    }

    #[test]
    fn a_repeated_question_replaces_its_entry_and_find_names_the_asker() {
        let inbox = Inbox::default();
        inbox.push("p::claude", entry(3));
        assert!(inbox.mark_delivered(3));
        inbox.push("p::claude", InboxEntry { text: "new".into(), ..entry(3) });
        let (asker, found) = inbox.find(3).unwrap();
        assert_eq!((asker.as_str(), found.text.as_str(), found.delivered), ("p::claude", "new", false));
        assert_eq!(inbox.read("p::claude", true).len(), 1);
        assert!(inbox.find(4).is_none());
    }
}
