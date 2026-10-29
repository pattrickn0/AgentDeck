//! Questions between tabs that wait for an answer.
//!
//! With interactive tabs, `ask-agent A "msg"` types the message into instance A's terminal and the
//! `ask-agent` of whoever asked stays **waiting**. The answer arrives when A runs
//! `ask-agent <who-asked> "answer"`: that call is recognized as an answer (there is a pending
//! question in the opposite direction) and delivered to whoever is waiting. No screen scraping.

use std::collections::HashMap;

use crossbeam_channel::{bounded, Receiver, Sender};
use parking_lot::Mutex;

/// What whoever waits receives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Answer(String),
    Failed(String),
}

struct Pending {
    id: u64,
    asker: String,
    target: String,
    /// How many pending questions come before this one in the chain (0 = whoever asked was not
    /// answering anyone).
    depth: i64,
    /// When the question was asked (epoch ms).
    since_ms: u64,
    /// The asker's `ask-agent` has already returned (HTTP 202): the answer will be typed later into its
    /// terminal. Whoever asked is not blocked, so the question does not enter cycle detection (but it
    /// still counts in the depth).
    detached: bool,
    tx: Sender<Reply>,
}

/// Snapshot of a pending question between instances (project, who asked, to whom).
///
/// The internal keys carry the project scope (`<project>#<id>`, see [`crate::instances::scoped`]);
/// here they come separated: `project` and the ids without the project. A key without scope gives an
/// empty `project`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingInfo {
    pub id: u64,
    pub project: String,
    pub asker: String,
    pub target: String,
    pub depth: i64,
    pub since_ms: u64,
    pub detached: bool,
}

#[derive(Default)]
struct State {
    next: u64,
    list: Vec<Pending>,
    /// Questions already concluded (answered or failed) whose `ask-agent` has not fetched the
    /// result yet: number → (who asked, target), with the current names (they follow renames).
    /// They leave in [`Exchange::remove`].
    finished: HashMap<u64, (String, String)>,
}

impl State {
    /// Takes the question at position `pos` out of the list and sends `reply` to whoever waits.
    fn conclude(&mut self, pos: usize, reply: Reply) -> Pending {
        let p = self.list.remove(pos);
        self.finished.insert(p.id, (p.asker.clone(), p.target.clone()));
        let _ = p.tx.send(reply);
        p
    }

    /// Concludes with `reason` every question that satisfies `pred`.
    fn fail_where(&mut self, pred: impl Fn(&Pending) -> bool, reason: &str) -> usize {
        let mut n = 0;
        while let Some(pos) = self.list.iter().position(&pred) {
            self.conclude(pos, Reply::Failed(reason.to_owned()));
            n += 1;
        }
        n
    }
}

#[derive(Default)]
pub struct Exchange {
    state: Mutex<State>,
}

/// Is there a path `from → … → to` following the pending questions (who waits → who answers)?
fn reaches(list: &[Pending], from: &str, to: &str) -> bool {
    let mut seen = vec![from.to_owned()];
    let mut stack = vec![from.to_owned()];
    while let Some(node) = stack.pop() {
        for p in list.iter().filter(|p| p.asker == node && !p.detached) {
            if p.target == to {
                return true;
            }
            if !seen.contains(&p.target) {
                seen.push(p.target.clone());
                stack.push(p.target.clone());
            }
        }
    }
    false
}

impl Exchange {
    /// Records that `asker` waits for an answer from `target`. Refuses if that would close a wait
    /// cycle (A waits for B, B waits for C, C waits for A): nobody would ever answer.
    pub fn register(&self, asker: &str, target: &str) -> Result<(u64, Receiver<Reply>), String> {
        self.register_at(asker, target, 0)
    }

    /// Like [`Exchange::register`], storing the question's depth in the chain (see
    /// [`Exchange::chain_depth`]).
    pub fn register_at(&self, asker: &str, target: &str, depth: i64) -> Result<(u64, Receiver<Reply>), String> {
        let mut st = self.state.lock();
        if asker == target || reaches(&st.list, target, asker) {
            return Err(format!("\"{target}\" is already waiting for an answer from \"{asker}\" (wait cycle)"));
        }
        let id = st.next;
        st.next += 1;
        let (tx, rx) = bounded(1);
        let since_ms = crate::util::now_ms();
        st.list.push(Pending { id, asker: asker.to_owned(), target: target.to_owned(), depth, since_ms, detached: false, tx });
        Ok((id, rx))
    }

    /// Would `asker` waiting for `target` close a wait cycle?
    pub fn would_cycle(&self, asker: &str, target: &str) -> bool {
        asker == target || reaches(&self.state.lock().list, target, asker)
    }

    /// Renames an instance in the pending questions (and in the concluded ones not read yet).
    pub fn rename(&self, old_id: &str, new_id: &str) {
        let mut st = self.state.lock();
        let st = &mut *st;
        let pairs = st.list.iter_mut().map(|p| (&mut p.asker, &mut p.target)).chain(st.finished.values_mut().map(|(a, t)| (a, t)));
        for (asker, target) in pairs {
            for who in [asker, target] {
                if who.as_str() == old_id {
                    *who = new_id.to_owned();
                }
            }
        }
    }

    /// Who made question `id` and to whom, with the current names (a rename in the middle of the
    /// question counts). Valid until [`Exchange::remove`], even after it is answered.
    pub fn parties(&self, id: u64) -> Option<(String, String)> {
        let st = self.state.lock();
        st.list.iter().find(|p| p.id == id).map(|p| (p.asker.clone(), p.target.clone())).or_else(|| st.finished.get(&id).cloned())
    }

    /// Depth of a new question asked by `asker`: 0 if nobody waits for its answer; otherwise one
    /// more than the deepest question it is answering. With tabs, the depth does not travel through
    /// the environment (the question is typed), so it is measured here.
    pub fn chain_depth(&self, asker: &str) -> i64 {
        self.state.lock().list.iter().filter(|p| p.target == asker).map(|p| p.depth + 1).max().unwrap_or(0)
    }

    /// `from` answered `to`: delivers to the oldest pending request from `to` to `from`.
    ///
    /// A new question from `from` to `to` while `to` waits for `from` would close a wait cycle
    /// (refused in [`Exchange::register`]); that is why such a message can only be an answer.
    pub fn reply(&self, from: &str, to: &str, text: &str) -> bool {
        self.reply_where(from, to, text, |_| true)
    }

    /// Like [`Exchange::reply`], but only for whoever is **still waiting** (its `ask-agent` did not
    /// return with 202). After the 202 the asker is free, so `ask-agent <it> "…"` may be a new
    /// question; the answer to a detached question goes by number (`--reply N`).
    pub fn reply_waiting(&self, from: &str, to: &str, text: &str) -> bool {
        self.reply_where(from, to, text, |p| !p.detached)
    }

    fn reply_where(&self, from: &str, to: &str, text: &str, ok: impl Fn(&Pending) -> bool) -> bool {
        let mut st = self.state.lock();
        let Some(pos) = st.list.iter().position(|p| p.asker == to && p.target == from && ok(p)) else { return false };
        st.conclude(pos, Reply::Answer(text.to_owned()));
        true
    }

    /// `from` answered question number `id`. Only valid if the question was asked to `from`.
    /// Returns who asked.
    pub fn reply_id(&self, id: u64, from: &str, text: &str) -> Option<String> {
        let mut st = self.state.lock();
        let pos = st.list.iter().position(|p| p.id == id && p.target == from)?;
        Some(st.conclude(pos, Reply::Answer(text.to_owned())).asker)
    }

    /// Is someone waiting for an answer from `from` to `to`? (`ask-agent` from `from` to `to` would be an answer.)
    pub fn is_waiting(&self, asker: &str, target: &str) -> bool {
        self.state.lock().list.iter().any(|p| p.asker == asker && p.target == target)
    }

    /// The request `id` still waits for an answer (it was not answered, cancelled or expired).
    pub fn is_pending(&self, id: u64) -> bool {
        self.state.lock().list.iter().any(|p| p.id == id)
    }

    /// Ends a request without an answer (timeout or failure). Idempotent.
    pub fn fail(&self, id: u64, reason: &str) {
        self.state.lock().fail_where(|p| p.id == id, reason);
    }

    /// Fails every request waiting for `target` (its tab was closed or died).
    pub fn fail_target(&self, target: &str, reason: &str) -> usize {
        self.state.lock().fail_where(|p| p.target == target, reason)
    }

    /// Fails every request made by `asker` (its tab was closed: nobody reads the answer).
    pub fn fail_asker(&self, asker: &str, reason: &str) -> usize {
        self.state.lock().fail_where(|p| p.asker == asker, reason)
    }

    /// Forgets the request (whoever waited gave up or already received it). Returns whether it was
    /// still pending: `false` = it was already concluded, and the result is in the waiter's channel
    /// (an answer that arrived at the moment the deadline ended is not lost).
    pub fn remove(&self, id: u64) -> bool {
        let mut st = self.state.lock();
        st.finished.remove(&id);
        let before = st.list.len();
        st.list.retain(|p| p.id != id);
        st.list.len() != before
    }

    /// The asker's `ask-agent` for question `id` returned without the answer (202). Idempotent.
    pub fn detach(&self, id: u64) {
        if let Some(p) = self.state.lock().list.iter_mut().find(|p| p.id == id) {
            p.detached = true;
        }
    }

    /// Pending questions, oldest first.
    pub fn snapshot(&self) -> Vec<PendingInfo> {
        self.state
            .lock()
            .list
            .iter()
            .map(|p| {
                let (project, asker) = crate::instances::unscoped(&p.asker);
                PendingInfo {
                    id: p.id,
                    project: project.to_owned(),
                    asker: asker.to_owned(),
                    target: crate::instances::unscoped(&p.target).1.to_owned(),
                    depth: p.depth,
                    since_ms: p.since_ms,
                    detached: p.detached,
                }
            })
            .collect()
    }

    pub fn len(&self) -> usize {
        self.state.lock().list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const T: Duration = Duration::from_millis(500);

    #[test]
    fn a_reply_reaches_the_one_waiting() {
        let ex = Exchange::default();
        let (_, rx) = ex.register("claude", "claude@dev").unwrap();
        assert!(ex.is_waiting("claude", "claude@dev"));
        assert!(ex.reply("claude@dev", "claude", "ready"));
        assert_eq!(rx.recv_timeout(T), Ok(Reply::Answer("ready".into())));
        assert!(ex.is_empty());
        // With nobody waiting, an "answer" is not an answer.
        assert!(!ex.reply("claude@dev", "claude", "again"));
    }

    #[test]
    fn replies_go_to_the_oldest_request_first() {
        let ex = Exchange::default();
        let (_, first) = ex.register("a", "b").unwrap();
        let (_, second) = ex.register("a", "b").unwrap();
        ex.reply("b", "a", "one");
        ex.reply("b", "a", "two");
        assert_eq!(first.recv_timeout(T), Ok(Reply::Answer("one".into())));
        assert_eq!(second.recv_timeout(T), Ok(Reply::Answer("two".into())));
    }

    #[test]
    fn the_direction_matters() {
        let ex = Exchange::default();
        let (_, rx) = ex.register("a", "b").unwrap();
        // c answering a, or b answering c, is not the expected answer.
        assert!(!ex.reply("c", "a", "x"));
        assert!(!ex.reply("b", "c", "x"));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn wait_cycles_are_refused() {
        let ex = Exchange::default();
        ex.register("a", "b").unwrap();
        ex.register("b", "c").unwrap();
        // c waiting for a would close a → b → c → a.
        let err = ex.register("c", "a").unwrap_err();
        assert!(err.contains("cycle"), "{err}");
        // Without a cycle: d waits for a, and a waiting for d would close it… (a → b → c; d → a)
        ex.register("d", "a").unwrap();
        assert!(ex.register("a", "d").is_err(), "a → d → a");
        // Asking yourself too.
        assert!(ex.register("x", "x").is_err());
    }

    #[test]
    fn failing_a_request_or_a_target_unblocks_the_waiters() {
        let ex = Exchange::default();
        let (id, rx) = ex.register("a", "b").unwrap();
        ex.fail(id, "timed out");
        assert_eq!(rx.recv_timeout(T), Ok(Reply::Failed("timed out".into())));
        ex.fail(id, "again"); // idempotent

        let (_, r1) = ex.register("a", "b").unwrap();
        let (_, r2) = ex.register("c", "b").unwrap();
        let (_, r3) = ex.register("a", "z").unwrap();
        assert_eq!(ex.fail_target("b", "the tab was closed"), 2);
        assert_eq!(r1.recv_timeout(T), Ok(Reply::Failed("the tab was closed".into())));
        assert_eq!(r2.recv_timeout(T), Ok(Reply::Failed("the tab was closed".into())));
        assert!(r3.try_recv().is_err(), "whoever waits for another target keeps waiting");
        assert_eq!(ex.len(), 1);
    }

    #[test]
    fn replies_by_id_only_from_the_target() {
        let ex = Exchange::default();
        let (first, r1) = ex.register("a", "b").unwrap();
        let (second, r2) = ex.register("a", "b").unwrap();
        assert_eq!(ex.reply_id(second, "c", "intruder"), None, "only whoever received the question answers");
        assert_eq!(ex.reply_id(second, "b", "two").as_deref(), Some("a"));
        assert_eq!(r2.recv_timeout(T), Ok(Reply::Answer("two".into())));
        assert!(r1.try_recv().is_err(), "the other question stays pending");
        assert_eq!(ex.reply_id(second, "b", "again"), None);
        assert_eq!(ex.reply_id(first, "b", "one").as_deref(), Some("a"));
    }

    #[test]
    fn chain_depth_follows_the_pending_questions() {
        let ex = Exchange::default();
        assert_eq!(ex.chain_depth("a"), 0);
        ex.register_at("a", "b", 0).unwrap();
        assert_eq!(ex.chain_depth("b"), 1, "b answers a question of depth 0");
        ex.register_at("b", "c", 1).unwrap();
        assert_eq!(ex.chain_depth("c"), 2);
        assert_eq!(ex.chain_depth("a"), 0);
    }

    #[test]
    fn failing_an_asker_drops_its_questions() {
        let ex = Exchange::default();
        let (_, r1) = ex.register("a", "b").unwrap();
        let (_, r2) = ex.register("c", "b").unwrap();
        assert_eq!(ex.fail_asker("a", "closed"), 1);
        assert_eq!(r1.recv_timeout(T), Ok(Reply::Failed("closed".into())));
        assert!(r2.try_recv().is_err());
        assert!(!ex.is_waiting("a", "b"));
        // Without the old question, b can ask a without a "cycle".
        assert!(ex.register("b", "a").is_ok());
    }

    #[test]
    fn the_snapshot_lists_pending_questions_in_order() {
        let ex = Exchange::default();
        let (a, _ra) = ex.register_at("a", "b", 0).unwrap();
        let (b, _rb) = ex.register_at("b", "c", 1).unwrap();
        let snap = ex.snapshot();
        assert_eq!(
            snap.iter().map(|p| (p.id, p.asker.as_str(), p.target.as_str(), p.depth)).collect::<Vec<_>>(),
            [(a, "a", "b", 0), (b, "b", "c", 1)]
        );
        assert!(snap.iter().all(|p| p.since_ms > 0 && !p.detached));
        ex.detach(a);
        ex.detach(a);
        assert!(ex.snapshot()[0].detached);
        ex.reply("b", "a", "ok");
        assert_eq!(ex.snapshot().len(), 1);
        assert_eq!(ex.snapshot()[0].project, "", "key without scope");
        ex.detach(a); // already removed: nothing happens
        assert_eq!(ex.snapshot().len(), 1);
    }

    #[test]
    fn a_detached_question_does_not_block_and_makes_no_cycle() {
        // claude waits for claude@dev, which waits for antigravity: antigravity asking claude would close the cycle…
        let ex = Exchange::default();
        let (first, _r1) = ex.register("claude", "claude@dev").unwrap();
        ex.register("claude@dev", "antigravity").unwrap();
        assert!(ex.would_cycle("antigravity", "claude"));
        // …but claude's ask-agent has already returned (202): claude is free, so there is no cycle.
        ex.detach(first);
        assert!(!ex.would_cycle("antigravity", "claude"));
        assert!(ex.register("antigravity", "claude").is_ok());
        // The depth still counts the detached question.
        assert_eq!(ex.chain_depth("claude@dev"), 1);
    }

    #[test]
    fn scoped_keys_keep_projects_apart_and_the_snapshot_splits_them() {
        use crate::instances::scoped;
        let ex = Exchange::default();
        let (a, b) = ("p00000001", "p00000002");
        ex.register_at(&scoped(a, "claude"), &scoped(a, "claude@dev"), 0).unwrap();
        // The same pair in another project is not a cycle and does not count in its depth.
        assert!(!ex.would_cycle(&scoped(b, "claude@dev"), &scoped(b, "claude")));
        assert!(ex.would_cycle(&scoped(a, "claude@dev"), &scoped(a, "claude")));
        assert_eq!(ex.chain_depth(&scoped(b, "claude@dev")), 0);
        assert_eq!(ex.chain_depth(&scoped(a, "claude@dev")), 1);
        // Answering in the wrong project does not deliver.
        assert!(!ex.reply(&scoped(b, "claude@dev"), &scoped(b, "claude"), "x"));
        let snap = ex.snapshot();
        assert_eq!((snap[0].project.as_str(), snap[0].asker.as_str(), snap[0].target.as_str()), (a, "claude", "claude@dev"));
    }

    #[test]
    fn the_parties_follow_a_rename_until_the_waiter_reads_the_result() {
        let ex = Exchange::default();
        let (id, rx) = ex.register("a", "b").unwrap();
        ex.rename("b", "b@dev");
        assert_eq!(ex.parties(id), Some(("a".into(), "b@dev".into())));
        assert_eq!(ex.reply_id(id, "b@dev", "ok").as_deref(), Some("a"));
        // Answered, but the waiter has not read it yet: the names remain available (and follow renames).
        ex.rename("a", "a@qa");
        assert_eq!(ex.parties(id), Some(("a@qa".into(), "b@dev".into())));
        assert_eq!(rx.recv_timeout(T), Ok(Reply::Answer("ok".into())));
        assert!(!ex.remove(id), "it was no longer pending");
        assert_eq!(ex.parties(id), None);
    }

    #[test]
    fn a_reply_right_at_the_deadline_is_not_lost() {
        let ex = Exchange::default();
        let (id, rx) = ex.register("a", "b").unwrap();
        // The waiter's deadline ended, but the answer arrives before it gives up.
        assert!(ex.reply("b", "a", "on time"));
        assert!(!ex.remove(id), "remove tells that the question was already concluded");
        assert_eq!(rx.try_recv(), Ok(Reply::Answer("on time".into())));
        // Without an answer, remove takes the pending question out.
        let (id, _rx) = ex.register("a", "b").unwrap();
        assert!(ex.remove(id));
    }

    #[test]
    fn after_a_202_only_the_number_or_the_reply_route_answer() {
        let ex = Exchange::default();
        let (id, rx) = ex.register("a", "b").unwrap();
        ex.detach(id);
        // `ask-agent a "…"` from b is no longer an answer: a is free and may receive a new question.
        assert!(!ex.reply_waiting("b", "a", "new question"));
        assert!(!ex.would_cycle("b", "a"));
        assert!(rx.try_recv().is_err());
        // While it waits, the old form still works.
        let (_, rx2) = ex.register("c", "b").unwrap();
        assert!(ex.reply_waiting("b", "c", "answer"));
        assert_eq!(rx2.recv_timeout(T), Ok(Reply::Answer("answer".into())));
        // The /reply route (which never creates a question) and the number keep answering the detached one.
        assert!(ex.reply("b", "a", "through the route"));
        assert_eq!(rx.recv_timeout(T), Ok(Reply::Answer("through the route".into())));
    }

    #[test]
    fn removing_a_request_forgets_it() {
        let ex = Exchange::default();
        let (id, _rx) = ex.register("a", "b").unwrap();
        ex.remove(id);
        assert!(!ex.is_waiting("a", "b"));
        // Once removed, the cycle is gone.
        assert!(ex.register("b", "a").is_ok());
    }
}
