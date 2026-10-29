//! Resuming each tab's conversation after the app closes (or crashes).
//!
//! Every tab of a CLI that can resume has its own CLI session id, saved in `ProjectCfg.tab_sessions` (tab id → session
//! id) **as soon as it is known**, so a crash does not lose it. A launch is one of three things:
//!
//! - **first**: the tab has no id yet. For Claude and OpenCode one is generated here and the CLI is told to use it
//!   (Claude `--session-id <uuid>`; OpenCode `--session <id>` creates the session when it does not exist). Antigravity
//!   and Codex choose their own id: the launch carries no id, and it is learned afterwards (see below);
//! - **resume**: the tab has an id whose conversation exists (Claude `--resume <uuid>`; OpenCode `--session <id>`;
//!   Antigravity `--conversation <id>`; Codex `resume <id>`);
//! - **fallback**: a resume that ended right away (see [`resume_failed`]): the id is dropped and the tab opens again as a
//!   first launch, with a notice in the chat. A new id never counts as a resume, so there is no loop.
//!
//! Never `--continue` (or any "most recent session" flag): with several tabs of the same CLI in one folder it would pick
//! another tab's conversation. A tab always asks for its own id. The CLI reports a new id on its own (Claude after
//! `/clear`) through `ask-agent --signal session` ([`Core::set_tab_session`]). The ids the app does not choose come from:
//!
//! - **Antigravity**: its own log. The tab is launched with `--log-file` (one file per tab, [`agy_log_path`]) and
//!   [`agy_session_in_log`] reads the conversation id from it while the tab runs (`Core::sync_agy_session`);
//! - **Codex**: the `notify` hook it calls at the end of each turn (`ask-agent --signal idle <json>`): the JSON carries
//!   `thread-id` ([`notify_thread_id`]), which is the id `codex resume` takes.
//!
//! Flags and formats checked on this machine are in `memory/learnings/resume-session-flags-per-cli.md` and
//! `memory/learnings/resume-antigravity-and-codex.md`.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::instances::Ident;
use crate::Core;

/// A resumed process that ends within this time, with a failure code, is a resume that did not work.
pub const FAIL_WINDOW: Duration = Duration::from_secs(10);

/// The CLI has a session id of its own that the app can choose or learn (the others open a new conversation each time).
pub fn supports(kind: &str) -> bool {
    matches!(kind, "claude" | "opencode" | "antigravity" | "codex")
}

/// The CLI picks the id of a new conversation itself: the app learns it after the launch (Antigravity's log, Codex's
/// `notify`) instead of choosing it.
pub fn learns_id_later(kind: &str) -> bool {
    matches!(kind, "antigravity" | "codex")
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

/// How a launch asks `kind` for the session `id`; `first` = the conversation does not exist yet. A CLI that picks its own
/// ids has nothing to say on a first launch.
pub fn launch_args(kind: &str, id: &str, first: bool) -> Vec<String> {
    let flag = match (kind, first) {
        ("claude", true) => "--session-id",
        ("claude", false) => "--resume",
        ("opencode", _) => "--session",
        ("antigravity", false) => "--conversation",
        // `resume` is a subcommand: it goes before every other option.
        ("codex", false) => "resume",
        _ => return Vec::new(),
    };
    vec![flag.into(), id.into()]
}

/// The decision of one launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The conversation this launch is bound to. `None` for a CLI that picks its own id and has none to continue.
    pub id: Option<String>,
    pub args: Vec<String>,
    /// The launch continues a stored conversation (a failure right away makes the tab open from scratch).
    pub resumed: bool,
    /// The id was just generated: it has to be saved.
    pub generated: bool,
    /// A stored id was found unusable (its conversation is gone, or it is malformed): it has to be forgotten.
    pub stale: bool,
}

/// What to launch for a tab of `kind` whose stored id is `stored`. `exists(id)` says whether the conversation is already on
/// disk: Claude refuses `--resume` for one that never got a message (`No conversation found`), so then the same id is
/// asked for again with `--session-id`. A CLI that picks its own ids (Antigravity, Codex) opens a new conversation when
/// the stored one is gone, and the new id replaces it once known. `None` = the CLI does not resume.
pub fn plan(kind: &str, stored: Option<&str>, exists: impl Fn(&str) -> bool) -> Option<Plan> {
    if !supports(kind) {
        return None;
    }
    let usable = stored.filter(|s| valid_session_id(s));
    if learns_id_later(kind) {
        let id = usable.filter(|id| exists(id)).map(str::to_owned);
        let args = id.as_deref().map(|id| launch_args(kind, id, false)).unwrap_or_default();
        return Some(Plan { resumed: id.is_some(), stale: stored.is_some() && id.is_none(), id, args, generated: false });
    }
    let (id, generated) = match usable {
        Some(id) => (id.to_owned(), false),
        None => (new_session_id(kind)?, true),
    };
    let resumed = !generated && exists(&id);
    let args = launch_args(kind, &id, !resumed);
    Some(Plan { id: Some(id), args, resumed, generated, stale: stored.is_some() && generated })
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

/// Codex's home (`CODEX_HOME`, else `<home>/.codex`).
pub(crate) fn codex_home(home: &Path) -> PathBuf {
    std::env::var_os("CODEX_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(".codex"))
}

/// Whether Codex has the thread `id` saved: `<codex home>/sessions/<year>/<month>/<day>/rollout-<time>-<id>.jsonl`. It
/// is also what tells the thread of a conversation from the hidden helper threads (the generated title of a chat), which
/// call `notify` too but leave no rollout.
pub fn codex_has_session(codex_home: &Path, id: &str) -> bool {
    if !valid_session_id(id) {
        return false;
    }
    let suffix = format!("-{id}.jsonl");
    let subdirs = |dir: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()
    };
    subdirs(&codex_home.join("sessions")).iter().flat_map(|year| subdirs(year)).flat_map(|month| subdirs(&month)).any(|day| {
        std::fs::read_dir(day).into_iter().flatten().flatten().any(|f| {
            let name = f.file_name();
            let name = name.to_string_lossy();
            name.starts_with("rollout-") && name.ends_with(&suffix)
        })
    })
}

/// The thread id in the JSON Codex passes to `notify` (`{"type":"agent-turn-complete","thread-id":"...",...}`).
pub fn notify_thread_id(json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let id = v.get("thread-id")?.as_str()?;
    valid_session_id(id).then(|| id.to_owned())
}

/// Whether Antigravity has the conversation `id` saved (`<home>/.gemini/antigravity-cli/conversations/<id>.db`). Without it
/// `--conversation` only warns ("conversation not found") and opens an empty one, which would count as a resume.
pub fn agy_has_conversation(home: &Path, id: &str) -> bool {
    valid_session_id(id) && home.join(".gemini").join("antigravity-cli").join("conversations").join(format!("{id}.db")).is_file()
}

/// The `--log-file` of an Antigravity tab: `<data>/agy-logs/<tab id>.log`. `agy` truncates it at each start and creates the
/// folders.
pub fn agy_log_path(data_dir: &Path, tab_id: &str) -> PathBuf {
    data_dir.join("agy-logs").join(format!("{}.log", tab_id.replace('@', "_")))
}

/// The conversation `agy` is on, from the text of its log: the last `Created conversation <uuid>` (first message of a new
/// conversation, also after `/new`), `Resuming conversation <uuid>` (`--conversation`, `/resume`) or `Streaming conversation
/// <uuid>` (it is shown) line. Checked with `agy` 1.4.7 and 1.4.10.
pub fn agy_session_in_log(text: &str) -> Option<String> {
    const MARKS: [&str; 3] = ["Created conversation ", "Resuming conversation ", "Streaming conversation "];
    text.lines().rev().find_map(|line| {
        let (_, rest) = MARKS.iter().find_map(|m| line.split_once(m))?;
        let id = rest.split_whitespace().next()?;
        uuid::Uuid::parse_str(id).ok().map(|u| u.to_string())
    })
}

/// Reads what `agy` appended to its log since `pos` (complete lines only) and returns the conversation id in it, if any.
/// `pos` moves forward; a log that shrank (a new process truncated it) is read from the start.
pub fn read_agy_log_session(log: &Path, pos: &mut u64) -> Option<String> {
    let mut file = std::fs::File::open(log).ok()?;
    let len = file.metadata().ok()?.len();
    if len < *pos {
        *pos = 0;
    }
    file.seek(SeekFrom::Start(*pos)).ok()?;
    let mut buf = Vec::new();
    file.take(len - *pos).read_to_end(&mut buf).ok()?;
    let complete = buf.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    *pos += complete as u64;
    agy_session_in_log(&String::from_utf8_lossy(&buf[..complete]))
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
        let home = &self.inner.home;
        let plan = plan(&ident.kind, stored.as_deref(), |session| match ident.kind.as_str() {
            "claude" => claude_has_session(&claude_config_dir(home), session),
            "codex" => codex_has_session(&codex_home(home), session),
            "antigravity" => agy_has_conversation(home, session),
            _ => true,
        })?;
        match &plan.id {
            Some(new) if stored.as_deref() != Some(new.as_str()) => {
                self.update_project(project, |p| {
                    p.tab_sessions.insert(id, new.clone());
                });
            }
            // The stored id points to nothing (the CLI forgot it): the new conversation will report its own.
            None if plan.stale => self.forget_tab_session(project, &id),
            _ => {}
        }
        Some(plan)
    }

    /// The CLI of the tab reported its session id (Claude's `SessionStart` hook, after `/clear` for instance; Codex's
    /// `notify`): it replaces the saved one. `false` = refused (invalid id, a CLI that does not resume, or a Codex thread
    /// that is not a saved conversation).
    pub fn set_tab_session(&self, project: &str, id: &str, session: &str) -> bool {
        let Some(ident) = Ident::parse(id).filter(|i| supports(&i.kind)) else { return false };
        if !valid_session_id(session) || (ident.kind == "codex" && !codex_has_session(&codex_home(&self.inner.home), session)) {
            return false;
        }
        if self.tab_session(project, id).as_deref() != Some(session) {
            self.update_project(project, |p| {
                p.tab_sessions.insert(id.to_owned(), session.to_owned());
            });
        }
        true
    }

    /// Antigravity's tab `id` has a log at `log` (its `--log-file`): saves the conversation it is on, if the log names one
    /// that differs from the saved id. `pos` is how much of the log was read already. Returns whether the id changed.
    pub fn sync_agy_session(&self, project: &str, id: &str, log: &Path, pos: &mut u64) -> bool {
        let Some(session) = read_agy_log_session(log, pos) else { return false };
        let changed = self.tab_session(project, id).as_deref() != Some(session.as_str());
        changed && self.set_tab_session(project, id, &session)
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
        assert!(["codex", "antigravity", "aider", "shell"].iter().all(|k| new_session_id(k).is_none()));
        assert!(["codex", "antigravity"].iter().all(|k| supports(k) && learns_id_later(k)));
        assert!(["aider", "shell"].iter().all(|k| !supports(k)));
        assert!(valid_session_id(&claude) && valid_session_id(&oc));
        assert!(!valid_session_id("") && !valid_session_id("a b") && !valid_session_id("x;y") && !valid_session_id(&"a".repeat(129)));
    }

    #[test]
    fn first_launch_generates_and_asks_for_that_id() {
        let claude = plan("claude", None, |_| false).expect("plan");
        assert!(claude.generated && !claude.resumed);
        assert_eq!(claude.args, ["--session-id", claude.id.as_deref().expect("id")]);
        let oc = plan("opencode", None, |_| false).expect("plan");
        assert!(oc.generated && !oc.resumed);
        assert_eq!(oc.args, ["--session", oc.id.as_deref().expect("id")]);
    }

    #[test]
    fn a_stored_conversation_is_resumed_by_its_own_id_never_by_continue() {
        let claude = plan("claude", Some(UUID), |_| true).expect("plan");
        assert_eq!((claude.generated, claude.resumed, claude.id.as_deref()), (false, true, Some(UUID)));
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
        assert!(p.generated && !p.resumed && p.stale && p.id.as_deref() != Some("not valid; rm"));
    }

    #[test]
    fn clis_without_a_session_id_get_nothing() {
        for kind in ["shell", "aider"] {
            assert_eq!(plan(kind, Some(UUID), |_| true), None, "{kind}");
            assert!(launch_args(kind, UUID, false).is_empty());
        }
    }

    #[test]
    fn antigravity_and_codex_resume_by_the_id_they_chose_and_have_nothing_to_say_before() {
        // First launch: no id exists yet, and none is invented (the CLI picks it; it is learned afterwards).
        for kind in ["antigravity", "codex"] {
            let first = plan(kind, None, |_| panic!("nothing to look up")).expect("plan");
            assert_eq!(first, Plan { id: None, args: vec![], resumed: false, generated: false, stale: false }, "{kind}");
            assert!(launch_args(kind, UUID, true).is_empty());
        }
        let agy = plan("antigravity", Some(UUID), |_| true).expect("plan");
        assert_eq!((agy.resumed, agy.id.as_deref()), (true, Some(UUID)));
        assert_eq!(agy.args, ["--conversation", UUID]);
        // `resume` is a subcommand: the id follows it directly.
        let codex = plan("codex", Some(UUID), |_| true).expect("plan");
        assert_eq!((codex.resumed, codex.id.as_deref()), (true, Some(UUID)));
        assert_eq!(codex.args, ["resume", UUID]);
        for p in [&agy, &codex] {
            assert!(p.args.iter().all(|a| !matches!(a.as_str(), "--continue" | "-c" | "--last")), "{:?}", p.args);
        }
    }

    #[test]
    fn a_conversation_that_is_gone_opens_a_new_one_and_the_stale_id_is_dropped() {
        for kind in ["antigravity", "codex"] {
            let p = plan(kind, Some(UUID), |_| false).expect("plan");
            assert_eq!((p.id, p.resumed, p.stale), (None, false, true), "{kind}");
            assert!(p.args.is_empty());
            let bad = plan(kind, Some("x y"), |_| true).expect("plan");
            assert!(bad.id.is_none() && bad.stale && bad.args.is_empty());
        }
    }

    const AGY_LOG: &str = "\
I1029 10:37:40.551442     337 manager.go:1015] Full redraw completed (RerenderAll) for conversation  (epoch 0, items 1) in 1ms
I1029 10:37:48.859129     764 conversation_manager.go:672] Starting new conversation (agent=false)
I1029 10:37:48.878026     764 server.go:1235] Created conversation 36ee4ddf-9114-4939-ba85-72a3cf8bda98 (backend=BACKEND_TYPE_UNSPECIFIED)
I1029 10:37:48.879054     764 conversation_manager.go:1080] Streaming conversation 36ee4ddf-9114-4939-ba85-72a3cf8bda98
I1029 10:37:48.880104     764 server.go:1253] Starting conversation update stream for 36ee4ddf-9114-4939-ba85-72a3cf8bda98
I1029 10:37:48.880679     764 server.go:1826] Sending user message to conversation 36ee4ddf-9114-4939-ba85-72a3cf8bda98 (items=1, media=0)
";

    #[test]
    fn the_antigravity_conversation_is_read_from_its_log_and_the_last_one_wins() {
        assert_eq!(agy_session_in_log(AGY_LOG).as_deref(), Some("36ee4ddf-9114-4939-ba85-72a3cf8bda98"));
        // A tab without a message yet: the log names no conversation (the TUI creates it with the first message).
        assert_eq!(agy_session_in_log(AGY_LOG.lines().next().expect("line")), None);
        // `--conversation`: "Resuming" and "Streaming" name it. `/new` later: the new one is the tab's conversation.
        let resumed = "I1014 09:41:58.727457       1 common.go:422] Resuming conversation d6ddf2a5-1df9-40d0-a8d5-156ff3e16853\n\
I1014 09:41:58.727457     323 conversation_manager.go:969] Streaming conversation d6ddf2a5-1df9-40d0-a8d5-156ff3e16853\n";
        assert_eq!(agy_session_in_log(resumed).as_deref(), Some("d6ddf2a5-1df9-40d0-a8d5-156ff3e16853"));
        assert_eq!(agy_session_in_log(&format!("{resumed}{AGY_LOG}")).as_deref(), Some("36ee4ddf-9114-4939-ba85-72a3cf8bda98"));
        // Not a UUID: ignored.
        assert_eq!(agy_session_in_log("x Created conversation not-a-uuid (backend=1)\n"), None);
    }

    #[test]
    fn the_log_is_read_incrementally_and_only_complete_lines_count() {
        let tmp = tempfile::tempdir().expect("tmp");
        let log = tmp.path().join("agy-logs").join("antigravity_dev.log");
        std::fs::create_dir_all(log.parent().expect("dir")).expect("mkdir");
        let mut pos = 0;
        assert_eq!(read_agy_log_session(&log, &mut pos), None, "no log yet");
        let line = "I1029 server.go:1235] Created conversation 36ee4ddf-9114-4939-ba85-72a3cf8bda98 (backend=X)\n";
        let (head, tail) = line.split_at(50);
        std::fs::write(&log, head).expect("w");
        assert_eq!(read_agy_log_session(&log, &mut pos), None, "half a line");
        assert_eq!(pos, 0);
        std::fs::write(&log, format!("{head}{tail}")).expect("w");
        assert_eq!(read_agy_log_session(&log, &mut pos).as_deref(), Some("36ee4ddf-9114-4939-ba85-72a3cf8bda98"));
        assert_eq!(pos, line.len() as u64);
        assert_eq!(read_agy_log_session(&log, &mut pos), None, "nothing new");
        // A new process truncates the log: it is read again from the start.
        std::fs::write(&log, "short\n").expect("w");
        assert_eq!(read_agy_log_session(&log, &mut pos), None);
        assert_eq!(pos, 6);
        assert_eq!(agy_log_path(Path::new("/d"), "antigravity@dev@ana"), Path::new("/d/agy-logs/antigravity_dev_ana.log"));
    }

    #[test]
    fn the_codex_thread_comes_from_the_notify_json_and_only_a_saved_thread_counts() {
        // As Codex 0.164 sends it (the last argument of the notify command).
        let json = r#"{"type":"agent-turn-complete","thread-id":"01a187e4-5d25-7c61-9416-9143d2b8a5e5","turn-id":"01a187e4-9619-7770-b9c2-0b48ebcb5f26","cwd":"C:\\Users\\ana\\project","client":"codex-tui","input-messages":["say banana"],"last-assistant-message":"banana"}"#;
        assert_eq!(notify_thread_id(json).as_deref(), Some("01a187e4-5d25-7c61-9416-9143d2b8a5e5"));
        assert_eq!(notify_thread_id(r#"{"type":"agent-turn-complete"}"#), None);
        assert_eq!(notify_thread_id(r#"{"thread-id":"a b;c"}"#), None);
        assert_eq!(notify_thread_id("not json"), None);

        let tmp = tempfile::tempdir().expect("tmp");
        let id = "01a187e4-5d25-7c61-9416-9143d2b8a5e5";
        assert!(!codex_has_session(tmp.path(), id), "no sessions folder");
        let day = tmp.path().join("sessions").join("2026").join("10").join("29");
        std::fs::create_dir_all(&day).expect("mkdir");
        std::fs::write(day.join(format!("rollout-2026-10-29T10-40-52-{id}.jsonl")), "{}\n").expect("w");
        assert!(codex_has_session(tmp.path(), id));
        // The hidden title thread calls notify too, but never leaves a rollout.
        assert!(!codex_has_session(tmp.path(), "01a187e4-9778-7e20-8302-a5c03b0d4cb1"));
        assert!(!codex_has_session(tmp.path(), "../x"));
    }

    #[test]
    fn antigravity_conversations_are_the_db_files_of_its_folder() {
        let tmp = tempfile::tempdir().expect("tmp");
        assert!(!agy_has_conversation(tmp.path(), UUID));
        let dir = tmp.path().join(".gemini").join("antigravity-cli").join("conversations");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join(format!("{UUID}.db")), "").expect("w");
        assert!(agy_has_conversation(tmp.path(), UUID));
        assert!(!agy_has_conversation(tmp.path(), "other"));
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
