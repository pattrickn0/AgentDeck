//! Resuming each tab's conversation after the app closes (or crashes).
//!
//! Every tab of a CLI that can resume has its own CLI session id, saved in `ProjectCfg.tab_sessions` (tab id → session
//! id) **as soon as it is known**, so a crash does not lose it. A launch is one of three things:
//!
//! - **first**: the tab has no id yet. One is generated here and the CLI is told to use it
//!   (Claude `--session-id <uuid>`; OpenCode `--session <id>` creates the session when it does not exist);
//! - **resume**: the tab has an id whose conversation exists (Claude `--resume <uuid>`; OpenCode `--session <id>`);
//! - **fallback**: a resume that ended right away (see [`resume_failed`]): the id is dropped and the tab opens again as a
//!   first launch, with a notice in the chat. A new id never counts as a resume, so there is no loop.
//!
//! Never `--continue` (or any "most recent session" flag): with several tabs of the same CLI in one folder it would pick
//! another tab's conversation. A tab always asks for its own id. The CLI reports a new id on its own (Claude after
//! `/clear`) through `ask-agent --signal session` ([`Core::set_tab_session`]). Flags checked on this machine are in
//! `memory/learnings/resume-session-flags-per-cli.md`.

use std::path::Path;
use std::time::{Duration, Instant};

use crate::instances::Ident;
use crate::Core;

/// A resumed process that ends within this time, with a failure code, is a resume that did not work.
pub const FAIL_WINDOW: Duration = Duration::from_secs(10);

/// The CLI has a session id of its own that the app can choose or learn (the others open a new conversation each time).
pub fn supports(kind: &str) -> bool {
    matches!(kind, "claude" | "opencode")
}

/// Session id that is safe for the command line, the JSON and the file name: letters, digits, `-` and `_`.
pub fn valid_session_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// New session id in the format `kind` accepts: a UUID for Claude (`--session-id` demands one) and `ses_` plus 26
/// lowercase hex digits for OpenCode (`--session` demands the `ses` prefix; checked with 2.0.32).
pub fn new_session_id(kind: &str) -> Option<String> {
    match kind {
        "claude" => Some(uuid::Uuid::new_v4().to_string()),
        "opencode" => Some(format!("ses_{}", &uuid::Uuid::new_v4().simple().to_string()[..26])),
        _ => None,
    }
}

/// How a launch asks `kind` for the session `id`; `first` = the conversation does not exist yet.
pub fn launch_args(kind: &str, id: &str, first: bool) -> Vec<String> {
    let flag = match (kind, first) {
        ("claude", true) => "--session-id",
        ("claude", false) => "--resume",
        ("opencode", _) => "--session",
        _ => return Vec::new(),
    };
    vec![flag.into(), id.into()]
}

/// The decision of one launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub id: String,
    pub args: Vec<String>,
    /// The launch continues a stored conversation (a failure right away makes the tab open from scratch).
    pub resumed: bool,
    /// The id was just generated: it has to be saved.
    pub generated: bool,
}

/// What to launch for a tab of `kind` whose stored id is `stored`. `exists(id)` says whether the conversation is already on
/// disk: Claude refuses `--resume` for one that never got a message (`No conversation found`), so then the same id is
/// asked for again with `--session-id`. `None` = the CLI does not resume.
pub fn plan(kind: &str, stored: Option<&str>, exists: impl Fn(&str) -> bool) -> Option<Plan> {
    if !supports(kind) {
        return None;
    }
    let (id, generated) = match stored.filter(|s| valid_session_id(s)) {
        Some(id) => (id.to_owned(), false),
        None => (new_session_id(kind)?, true),
    };
    let resumed = !generated && exists(&id);
    let args = launch_args(kind, &id, !resumed);
    Some(Plan { id, args, resumed, generated })
}

/// A launch that resumed a conversation ended too soon with a failure: the stored id does not work (the CLI says the session
/// does not exist, or refuses it). A clean exit does not count: the user may have quit on purpose.
pub fn resume_failed(resumed: bool, spawned_at: Option<Instant>, now: Instant, code: i64) -> bool {
    resumed && code != 0 && spawned_at.is_some_and(|at| now.saturating_duration_since(at) < FAIL_WINDOW)
}

/// Whether Claude has the conversation `id` saved: its transcript is `<config>/projects/<folder>/<id>.jsonl`.
pub fn claude_has_session(config_dir: &Path, id: &str) -> bool {
    let Ok(dirs) = std::fs::read_dir(config_dir.join("projects")) else { return false };
    dirs.flatten().any(|d| d.path().join(format!("{id}.jsonl")).is_file())
}

/// Claude's configuration folder (`CLAUDE_CONFIG_DIR`, else `<home>/.claude`).
pub(crate) fn claude_config_dir(home: &Path) -> std::path::PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR").filter(|v| !v.is_empty()).map(std::path::PathBuf::from).unwrap_or_else(|| home.join(".claude"))
}

impl Core {
    /// The CLI session id saved for the tab, if any.
    pub fn tab_session(&self, project: &str, id: &str) -> Option<String> {
        self.inner.config.read().project(project).and_then(|p| p.tab_sessions.get(id).cloned())
    }

    /// Decides the launch of the tab `ident` (see [`plan`]) and saves a newly generated id right away.
    pub(crate) fn plan_launch(&self, project: &str, ident: &Ident) -> Option<Plan> {
        let id = ident.id();
        let stored = self.tab_session(project, &id);
        let config_dir = claude_config_dir(&self.inner.home);
        let plan = plan(&ident.kind, stored.as_deref(), |session| match ident.kind.as_str() {
            "claude" => claude_has_session(&config_dir, session),
            _ => true,
        })?;
        if stored.as_deref() != Some(plan.id.as_str()) {
            self.update_project(project, |p| {
                p.tab_sessions.insert(id, plan.id.clone());
            });
        }
        Some(plan)
    }

    /// The CLI of the tab reported its session id (Claude's `SessionStart` hook, after `/clear` for instance): it replaces the
    /// saved one. `false` = refused (invalid id, or a CLI that does not resume).
    pub fn set_tab_session(&self, project: &str, id: &str, session: &str) -> bool {
        let kind_ok = Ident::parse(id).is_some_and(|i| supports(&i.kind));
        if !kind_ok || !valid_session_id(session) {
            return false;
        }
        if self.tab_session(project, id).as_deref() != Some(session) {
            self.update_project(project, |p| {
                p.tab_sessions.insert(id.to_owned(), session.to_owned());
            });
        }
        true
    }

    /// Forgets the tab's session id: the next launch starts a new conversation.
    pub fn forget_tab_session(&self, project: &str, id: &str) {
        if self.tab_session(project, id).is_some() {
            self.update_project(project, |p| {
                p.tab_sessions.remove(id);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UUID: &str = "429e5864-ad7e-46ed-9f76-b280602c8ff5";

    #[test]
    fn ids_come_in_the_format_each_cli_accepts() {
        let claude = new_session_id("claude").expect("claude");
        assert!(uuid::Uuid::parse_str(&claude).is_ok(), "{claude}");
        let oc = new_session_id("opencode").expect("opencode");
        assert!(oc.starts_with("ses_") && oc.len() == 30 && oc[4..].chars().all(|c| c.is_ascii_hexdigit()), "{oc}");
        assert_ne!(new_session_id("claude"), new_session_id("claude"));
        assert!(["codex", "antigravity", "aider", "shell"].iter().all(|k| new_session_id(k).is_none() && !supports(k)));
        assert!(valid_session_id(&claude) && valid_session_id(&oc));
        assert!(!valid_session_id("") && !valid_session_id("a b") && !valid_session_id("x;y") && !valid_session_id(&"a".repeat(129)));
    }

    #[test]
    fn first_launch_generates_and_asks_for_that_id() {
        let claude = plan("claude", None, |_| false).expect("plan");
        assert!(claude.generated && !claude.resumed);
        assert_eq!(claude.args, ["--session-id", claude.id.as_str()]);
        let oc = plan("opencode", None, |_| false).expect("plan");
        assert!(oc.generated && !oc.resumed);
        assert_eq!(oc.args, ["--session", oc.id.as_str()]);
    }

    #[test]
    fn a_stored_conversation_is_resumed_by_its_own_id_never_by_continue() {
        let claude = plan("claude", Some(UUID), |_| true).expect("plan");
        assert_eq!((claude.generated, claude.resumed, claude.id.as_str()), (false, true, UUID));
        assert_eq!(claude.args, ["--resume", UUID]);
        let oc = plan("opencode", Some("ses_abc"), |_| true).expect("plan");
        assert!(oc.resumed && !oc.generated);
        assert_eq!(oc.args, ["--session", "ses_abc"]);
        for p in [&claude, &oc] {
            assert!(p.args.iter().all(|a| a != "--continue" && a != "-c"), "{:?}", p.args);
        }
    }

    #[test]
    fn a_conversation_that_never_got_a_message_is_asked_for_again_with_the_same_id() {
        // Claude only saves the transcript with the first message: `--resume` would fail with "No conversation found".
        let p = plan("claude", Some(UUID), |_| false).expect("plan");
        assert_eq!((p.generated, p.resumed), (false, false));
        assert_eq!(p.args, ["--session-id", UUID]);
    }

    #[test]
    fn an_unusable_stored_id_is_replaced() {
        let p = plan("claude", Some("not valid; rm"), |_| true).expect("plan");
        assert!(p.generated && !p.resumed && p.id != "not valid; rm");
    }

    #[test]
    fn clis_without_a_session_id_get_nothing() {
        for kind in ["codex", "antigravity", "shell", "aider"] {
            assert_eq!(plan(kind, Some(UUID), |_| true), None, "{kind}");
            assert!(launch_args(kind, UUID, false).is_empty());
        }
    }

    #[test]
    fn only_a_quick_failure_of_a_resumed_launch_is_a_failed_resume() {
        let t0 = Instant::now();
        let soon = t0 + Duration::from_secs(2);
        let late = t0 + FAIL_WINDOW + Duration::from_secs(1);
        assert!(resume_failed(true, Some(t0), soon, 1));
        assert!(resume_failed(true, Some(t0), soon, -1));
        assert!(!resume_failed(true, Some(t0), soon, 0), "a clean exit is the user quitting");
        assert!(!resume_failed(true, Some(t0), late, 1), "a conversation that ran for a while is not a refusal");
        assert!(!resume_failed(false, Some(t0), soon, 1), "a first launch has nothing to drop");
        assert!(!resume_failed(true, None, soon, 1));
    }

    #[test]
    fn claude_transcripts_are_found_in_any_project_folder() {
        let tmp = tempfile::tempdir().expect("tmp");
        assert!(!claude_has_session(tmp.path(), UUID), "no projects folder");
        let dir = tmp.path().join("projects").join("C--some-folder");
        std::fs::create_dir_all(&dir).expect("mkdir");
        assert!(!claude_has_session(tmp.path(), UUID));
        std::fs::write(dir.join(format!("{UUID}.jsonl")), "{}\n").expect("w");
        assert!(claude_has_session(tmp.path(), UUID));
        assert!(!claude_has_session(tmp.path(), "other"));
    }
}
