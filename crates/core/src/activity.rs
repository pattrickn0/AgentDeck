//! Last activity of each agent tab (terminal output or a turn in progress).
//!
//! The UI reports here while a tab is working. The bridge uses it to count the waiting time of a
//! question from the end of the tab's work, not from when the question was made: an agent busy
//! with a long task does not run out of time.

use std::collections::HashMap;
use std::time::Instant;

use parking_lot::Mutex;

#[derive(Default)]
pub struct Activity {
    last: Mutex<HashMap<String, Instant>>,
}

impl Activity {
    /// The tab `id` did something now (produced output or is in the middle of a turn).
    pub fn touch(&self, id: &str, at: Instant) {
        let mut last = self.last.lock();
        match last.get_mut(id) {
            Some(prev) => *prev = (*prev).max(at),
            None => {
                last.insert(id.to_owned(), at);
            }
        }
    }

    /// Last activity signal of the tab `id`, if there was one.
    pub fn last(&self, id: &str) -> Option<Instant> {
        self.last.lock().get(id).copied()
    }

    /// The tab was closed.
    pub fn forget(&self, id: &str) {
        self.last.lock().remove(id);
    }

    /// Forgets the tabs whose keys start with `prefix` (those of a removed project).
    pub fn forget_prefix(&self, prefix: &str) {
        self.last.lock().retain(|k, _| !k.starts_with(prefix));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn keeps_the_most_recent_signal() {
        let a = Activity::default();
        let t0 = Instant::now();
        assert_eq!(a.last("x"), None);
        a.touch("x", t0 + Duration::from_secs(5));
        a.touch("x", t0);
        assert_eq!(a.last("x"), Some(t0 + Duration::from_secs(5)));
        a.forget("x");
        assert_eq!(a.last("x"), None);
    }

    #[test]
    fn forgets_a_whole_project() {
        let a = Activity::default();
        let t0 = Instant::now();
        for k in ["p1#claude", "p1#claude@dev", "p2#claude"] {
            a.touch(k, t0);
        }
        a.forget_prefix("p1#");
        assert_eq!((a.last("p1#claude"), a.last("p1#claude@dev"), a.last("p2#claude")), (None, None, Some(t0)));
    }
}
