//! The `Deck` of the tasks desk end to end, with the real core and git (skips with a warning without
//! `git`): create and start from the form, the report arriving, merge with confirmation, return,
//! validate, reject and discard. No window: the frame is just `before_frame` (core events and thread answers).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use agentdeck_core::paths::AppPaths;
use agentdeck_core::{Config, Core, MergeBlock, StartOptions, Task, TaskStatus};

use crate::deck::Deck;
use crate::docs::{CenterView, DiffDoc, DocState};
use crate::tasks_ui::{actions_for, can_show_diff, diff_button_label, parse_diff_stat, Confirm, TaskAct, TasksView};

fn git_available() -> bool {
    Command::new("git").arg("--version").output().is_ok_and(|o| o.status.success())
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git").current_dir(dir).args(args).output().expect("git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// Folder of the git executable (for the core's search PATH).
fn git_dir() -> PathBuf {
    // `which` does not come in the CI's Fedora container; `command -v` is the shell's own.
    let out =
        if cfg!(windows) { Command::new("where").arg("git").output() } else { Command::new("sh").args(["-c", "command -v git"]).output() };
    let out = out.expect("which git");
    let first = String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or("").trim().to_owned();
    Path::new(&first).parent().map(Path::to_path_buf).unwrap_or_default()
}

/// A fake `claude` agent installed (the core only checks that the executable exists).
fn install_fake_claude(root: &Path) {
    let bin = if cfg!(windows) { root.join("home").join(".local").join("bin") } else { root.join("fake-bin") };
    std::fs::create_dir_all(&bin).expect("mkdir");
    let exe = bin.join(if cfg!(windows) { "claude.exe" } else { "claude" });
    std::fs::write(&exe, b"").expect("write");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(&exe, "#!/bin/sh\ncat >/dev/null\n").expect("write");
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
}

struct Env {
    _tmp: tempfile::TempDir,
    ws: PathBuf,
    core: Core,
    deck: Deck,
    ctx: egui::Context,
}

impl Env {
    fn start() -> Env {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path().to_path_buf();
        install_fake_claude(&root);
        let ws = root.join("ws");
        std::fs::create_dir_all(&ws).expect("ws");
        git(&ws, &["init", "-q", "-b", "main"]);
        for (k, v) in [("user.name", "Test"), ("user.email", "test@example.com"), ("commit.gpgsign", "false")] {
            git(&ws, &["config", k, v]);
        }
        std::fs::write(ws.join("a.txt"), "one\n").expect("w");
        git(&ws, &["add", "-A"]);
        git(&ws, &["commit", "-q", "-m", "initial"]);
        let paths = AppPaths {
            app_dir: root.clone(),
            bin_dir: root.join("bin"),
            data_dir: root.join("dados"),
            default_workspace: root.join("ws"),
            whisper_dir: root.join("whisper"),
            config_file: root.join("cfg").join("config.json"),
        };
        let opts = StartOptions {
            search_path: Some(std::env::join_paths([root.join("fake-bin"), git_dir()]).expect("PATH")),
            home_dir: Some(root.join("home")),
            watch_workspace: false,
            config: Some(Config::with_project(ws.clone())),
            system_dirs: false,
        };
        let (core, rx) = Core::start_with(paths, opts).expect("start");
        let ctx = egui::Context::default();
        // An empty frame loads the fonts (the `Deck` measures the terminal on the first `before_frame`).
        let mut out = ctx.run_ui(egui::RawInput::default(), |_| {});
        out.textures_delta.clear();
        let deck = Deck::new(core.clone(), rx, std::sync::Arc::new(|| {}));
        Env { _tmp: tmp, ws, core, deck, ctx }
    }

    /// Runs frames until `done` becomes true (or 30 s).
    fn pump(&mut self, what: &str, done: impl Fn(&Deck) -> bool) {
        let limit = Instant::now() + Duration::from_secs(30);
        while !done(&self.deck) {
            assert!(Instant::now() < limit, "timed out waiting for: {what}");
            self.deck.before_frame(&self.ctx);
            std::thread::sleep(Duration::from_millis(15));
        }
    }

    fn task(&self, id: &str) -> Task {
        self.deck.tasks.task(id).cloned().unwrap_or_else(|| panic!("task {id} is not in the list"))
    }

    fn project(&self) -> String {
        self.deck.active_project.clone()
    }

    fn tab_open(&self, id: &str) -> bool {
        self.deck.tasks.task(id).is_some_and(|t| self.deck.tasks_tab_open(t))
    }

    /// Fills in the form, submits and waits for the answer; returns the id of the task opened in the detail.
    fn create_from_form(&mut self, title: &str, start_now: bool) -> String {
        self.deck.tasks_open_new();
        let f = &mut self.deck.tasks.form;
        f.title = title.to_owned();
        f.brief = "Make the request.".into();
        f.start_now = start_now;
        self.deck.tasks_submit_form();
        assert!(self.deck.tasks.form.busy, "criando em segundo plano");
        self.pump("the creation", |d| !d.tasks.form.busy);
        match &self.deck.tasks.view {
            TasksView::Detail(id) => id.clone(),
            other => panic!("expected the new task's detail, got {other:?}"),
        }
    }

    /// The state of document `uid` (which must exist).
    fn doc_state(&self, uid: u64) -> &DocState {
        &self.deck.docs.get(uid).unwrap_or_else(|| panic!("document {uid} is not open")).state
    }

    /// Opens the task's diff in the center and waits for it to arrive (the read runs in a thread). Returns the document's uid
    /// and the diff.
    fn open_task_diff(&mut self, id: &str) -> (u64, DiffDoc) {
        self.deck.docs_open_task_diff(id);
        let uid = self.deck.docs.current_uid(&self.project()).expect("focused document");
        assert!(matches!(self.doc_state(uid), DocState::Loading), "loading");
        assert_eq!(self.deck.docs.view, CenterView::Docs, "opening a document takes the center to Documents");
        self.pump("the diff", |d| !matches!(d.docs.get(uid).map(|t| &t.state), Some(DocState::Loading)));
        match self.doc_state(uid) {
            DocState::Diff(doc) => (uid, doc.clone()),
            other => panic!("expected the diff, got {other:?}"),
        }
    }

    /// The agent makes a commit in the task's worktree.
    fn agent_commits(&self, id: &str, file: &str, content: &str) {
        let t = self.task(id);
        std::fs::write(t.worktree.join(file), content).expect("w");
        git(&t.worktree, &["add", "-A"]);
        git(&t.worktree, &["commit", "-q", "-m", &format!("feat: {file}")]);
    }
}

#[test]
fn the_form_creates_and_starts_a_task_then_review_and_merge_run_off_the_ui_thread() {
    if !git_available() {
        eprintln!("git is not installed: test skipped");
        return;
    }
    let mut env = Env::start();
    env.pump("the agents and the git status", |d| d.initialized && d.git.status().is_some());
    // The form opens with the default agent and the current branch as the base.
    env.deck.tasks_open_new();
    assert_eq!((env.deck.tasks.form.kind.as_str(), env.deck.tasks.form.base.as_str()), ("claude", "main"));
    assert!(env.deck.tasks.form.start_now && env.deck.tasks.form.focus_title);
    // Empty title: nothing happens. A core error shows in the form.
    env.deck.tasks_submit_form();
    assert!(!env.deck.tasks.form.busy);
    env.deck.tasks.form.title = "Task with a bad base".into();
    env.deck.tasks.form.base = "nao-existe".into();
    env.deck.tasks_submit_form();
    env.pump("the validation error", |d| !d.tasks.form.busy);
    let err = env.deck.tasks.form.error.clone().expect("inline error");
    assert!(err.contains("is not a local branch"), "{err}");
    assert_eq!(env.deck.tasks.view, TasksView::New, "the form stays open");
    assert!(env.core.tasks(&env.project()).is_empty());

    // Create and start: the detail opens with the task in progress, the agent's tab appears.
    env.deck.tasks.form.base = "main".into();
    env.deck.tasks.form.error = None;
    env.deck.tasks_submit_form();
    env.pump("the creation", |d| !d.tasks.form.busy);
    assert_eq!(env.deck.tasks.view, TasksView::Detail("t1".into()));
    assert!(env.deck.tasks.form.title.is_empty(), "the form goes back empty");
    let t = env.task("t1");
    assert_eq!((t.status, t.base.as_str(), t.kind.as_str()), (TaskStatus::Running, "main", "claude"));
    env.pump("the agent's tab", |d| d.tasks.task("t1").is_some_and(|t| d.tasks_tab_open(t)));
    assert_eq!(actions_for(&env.task("t1"), true), [TaskAct::GoToTab, TaskAct::Discard]);

    // The agent works and delivers the report (it comes from the core, as through the bridge): the list re-reads by itself.
    env.agent_commits("t1", "new.txt", "two\n");
    env.core.task_done(&env.project(), "t1", "I created new.txt.").expect("done");
    env.pump("the task to reach review", |d| d.tasks.task("t1").is_some_and(|t| t.status == TaskStatus::Review));
    assert_eq!(env.deck.tasks.review_count(), 1);
    assert!(parse_diff_stat(env.task("t1").diff_stat.as_deref().unwrap_or("")).is_some_and(|s| s.files == 1));

    // The full diff comes from the core in a thread and opens in the center (the right bar stays on the detail).
    let (diff_uid, doc) = env.open_task_diff("t1");
    assert!(doc.diff.lines.iter().any(|l| l.text == "+two"), "{:?}", doc.diff.lines);
    assert_eq!(doc.files.iter().map(|f| (f.path.as_str(), f.status, f.added)).collect::<Vec<_>>(), [("new.txt", 'A', 1)]);
    assert_eq!(env.deck.tasks.view, TasksView::Detail("t1".into()));
    // Opening again only focuses the same document.
    env.deck.docs_open_task_diff("t1");
    assert_eq!(env.deck.docs.count(&env.project()), 1);
    assert_eq!(env.deck.docs.current_uid(&env.project()), Some(diff_uid));

    // Return: asks for the feedback, goes to the thread, goes back to in progress.
    env.deck.tasks_act("t1", TaskAct::Return);
    assert!(env.deck.tasks.return_open);
    env.deck.tasks.feedback = "  A test is missing.  ".into();
    env.deck.tasks_send_return("t1");
    assert!(env.deck.tasks.busy.is_some(), "one operation at a time");
    env.pump("the return", |d| d.tasks.busy.is_none());
    assert_eq!(env.task("t1").status, TaskStatus::Running);
    assert!(!env.deck.tasks.return_open && env.deck.tasks.feedback.is_empty());
    env.core.task_done(&env.project(), "t1", "Now it has a test.").expect("done 2");
    env.pump("the second review", |d| d.tasks.task("t1").is_some_and(|t| t.status == TaskStatus::Review));

    // Merge asks for confirmation; the "yes" merges and the task awaits validation: the tab stays open.
    env.deck.tasks_act("t1", TaskAct::Merge);
    assert_eq!(env.deck.tasks.confirm, Some(Confirm::Merge("t1".into())));
    assert_eq!(env.task("t1").status, TaskStatus::Review, "nothing changed before the yes");
    env.deck.tasks_confirm_accepted();
    assert!(env.deck.tasks.confirm.is_none() && env.deck.tasks.busy.is_some());
    env.pump("the merge", |d| d.tasks.busy.is_none());
    assert_eq!(env.task("t1").status, TaskStatus::Validating);
    assert!(env.deck.tasks.notice.is_none());
    assert!(env.ws.join("new.txt").exists(), "the merge brought the file into the base");
    assert!(env.tab_open("t1") && env.task("t1").worktree.is_dir(), "the agent stays alive until validation");
    assert_eq!((env.deck.tasks.review_count(), env.deck.tasks.validating_count(), env.deck.tasks.attention_count()), (0, 1, 1));
    assert_eq!(actions_for(&env.task("t1"), true), [TaskAct::Validate, TaskAct::Reject, TaskAct::GoToTab, TaskAct::Discard]);

    // Awaiting validation the diff button shows what the merge brought (the branch is already the base itself).
    let validating = env.task("t1");
    assert!(can_show_diff(&validating) && validating.merge_commit.is_some());
    assert_eq!(diff_button_label(&validating), "View what was merged");
    let (merged_uid, doc) = env.open_task_diff("t1");
    assert_ne!(merged_uid, diff_uid, "the merge diff is another document, not the branch one from before");
    assert!(doc.diff.lines.iter().any(|l| l.text == "+two"), "{:?}", doc.diff.lines);
    assert_eq!(env.deck.docs.count(&env.project()), 2);
    // Closing the last document takes the center back to the agents.
    env.deck.docs_close(diff_uid);
    assert_eq!(env.deck.docs.view, CenterView::Docs);
    env.deck.docs_close(merged_uid);
    env.deck.docs.sync(&env.project());
    assert_eq!(env.deck.docs.view, CenterView::Agents);

    // Reject: opens the same feedback form; sending sends the task back to the agent, in the same worktree.
    env.deck.tasks_act("t1", TaskAct::Reject);
    assert!(env.deck.tasks.return_open);
    env.deck.tasks_send_return("t1");
    assert!(env.deck.tasks.busy.is_none(), "without a reason it does not send");
    env.deck.tasks.feedback = "  The new file does not show up in the app.  ".into();
    env.deck.tasks_send_return("t1");
    assert!(env.deck.tasks.busy.is_some());
    env.pump("the rejection", |d| d.tasks.busy.is_none());
    let back = env.task("t1");
    assert_eq!(back.status, TaskStatus::Running);
    assert!(back.worktree.is_dir() && env.tab_open("t1"));
    assert!(!env.deck.tasks.return_open && env.deck.tasks.feedback.is_empty());
    assert_eq!(env.deck.tasks.attention_count(), 0);
    // The agent fixes it and the cycle goes on: review, merge, awaiting validation.
    env.agent_commits("t1", "fixed.txt", "ok\n");
    env.core.task_done(&env.project(), "t1", "Fixed.").expect("done 3");
    env.pump("the third review", |d| d.tasks.task("t1").is_some_and(|t| t.status == TaskStatus::Review));
    env.deck.tasks_act("t1", TaskAct::Merge);
    env.deck.tasks_confirm_accepted();
    env.pump("the second merge", |d| d.tasks.busy.is_none());
    assert_eq!(env.task("t1").status, TaskStatus::Validating);

    // Validate does not ask: it completes, closes the tab and deletes the worktree.
    env.deck.tasks_act("t1", TaskAct::Validate);
    assert!(env.deck.tasks.confirm.is_none() && env.deck.tasks.busy.is_some());
    env.pump("the validation", |d| d.tasks.busy.is_none());
    assert_eq!(env.task("t1").status, TaskStatus::Merged);
    assert!(env.deck.tasks.notice.is_none() && !env.task("t1").worktree.exists());
    env.pump("the tab to close", |d| d.tasks.task("t1").is_some_and(|t| !d.tasks_tab_open(t)));
    assert!(actions_for(&env.task("t1"), false).is_empty());
    assert_eq!(env.deck.tasks.attention_count(), 0);
}

#[test]
fn a_task_whose_scope_overlaps_waits_in_the_queue_and_starts_by_itself_when_the_other_is_merged() {
    if !git_available() {
        eprintln!("git is not installed: test skipped");
        return;
    }
    let mut env = Env::start();
    env.pump("the agents and the git status", |d| d.initialized && d.git.status().is_some());
    crate::theme::install_fonts(&env.ctx);
    crate::theme::install_style(&env.ctx);

    // The form's scope (one path per line or separated by commas) goes to the core.
    env.deck.tasks_open_new();
    env.deck.tasks.form.scope = " crates/core \n\nmemory/GERAL.md, crates\\cli ".into();
    env.deck.tasks.form.title = "Touches the core".into();
    env.deck.tasks_submit_form();
    env.pump("the creation", |d| !d.tasks.form.busy);
    assert_eq!(env.task("t1").scope, ["crates/core", "memory/GERAL.md", "crates/cli"]);
    assert_eq!(env.task("t1").status, TaskStatus::Running);
    assert!(env.deck.tasks.form.scope.is_empty(), "the form goes back empty");
    env.pump("the agent's tab", |d| d.tasks.task("t1").is_some_and(|t| d.tasks_tab_open(t)));

    // An invalid scope shows in the form, creating nothing.
    env.deck.tasks_open_new();
    env.deck.tasks.form.title = "Wildcard".into();
    env.deck.tasks.form.scope = "crates/*.rs".into();
    env.deck.tasks_submit_form();
    env.pump("the scope error", |d| !d.tasks.form.busy);
    assert!(env.deck.tasks.form.error.as_deref().is_some_and(|e| e.contains("not patterns (glob)")), "{:?}", env.deck.tasks.form.error);
    assert_eq!(env.core.tasks(&env.project()).len(), 1);

    // The second, with a scope inside the first's, enters the queue: no tab, only the discard button.
    env.deck.tasks.form.title = "Touches git.rs".into();
    env.deck.tasks.form.scope = "crates/core/src/git.rs".into();
    env.deck.tasks_submit_form();
    env.pump("the creation of the second", |d| !d.tasks.form.busy && d.tasks.task("t2").is_some());
    let queued = env.task("t2");
    assert_eq!((queued.status, queued.waiting_for.as_deref()), (TaskStatus::Queued, Some("t1")));
    assert_eq!(env.deck.tasks.view, TasksView::Detail("t2".into()));
    assert!(!env.tab_open("t2") && !queued.worktree.exists());
    assert_eq!(actions_for(&queued, false), [TaskAct::Discard]);
    assert_eq!(crate::tasks_ui::Group::of(queued.status), crate::tasks_ui::Group::Queued);
    // The list and the detail of the queued task draw without trouble.
    draw_frame(&mut env, vec![]);
    env.deck.tasks_back();
    draw_frame(&mut env, vec![]);

    // The first delivers and is merged by the button: the second starts by itself, with the merge in the base.
    env.agent_commits("t1", "core.txt", "two\n");
    env.core.task_done(&env.project(), "t1", "Done.").expect("done");
    env.pump("the review", |d| d.tasks.task("t1").is_some_and(|t| t.status == TaskStatus::Review));
    // The file is outside the declared scope: the review shows the notice (and the merge stays free).
    assert_eq!(env.task("t1").outside_scope, ["core.txt"]);
    env.deck.tasks_open_detail("t1");
    draw_frame(&mut env, vec![]);
    env.deck.tasks_act("t1", TaskAct::Merge);
    env.deck.tasks_confirm_accepted();
    env.pump("the merge", |d| d.tasks.busy.is_none());
    env.pump("the queued task to start", |d| d.tasks.task("t2").is_some_and(|t| t.status == TaskStatus::Running));
    let started = env.task("t2");
    assert!(
        started.waiting_for.is_none() && started.worktree.join("core.txt").is_file(),
        "it branches from the base already with t1's merge"
    );
    env.pump("the tab of the task that left the queue", |d| d.tasks.task("t2").is_some_and(|t| d.tasks_tab_open(t)));
    assert_eq!(actions_for(&started, true), [TaskAct::GoToTab, TaskAct::Discard]);
    draw_frame(&mut env, vec![]);
}

#[test]
fn a_refused_merge_shows_the_core_message_and_discard_asks_first() {
    if !git_available() {
        eprintln!("git is not installed: test skipped");
        return;
    }
    let mut env = Env::start();
    env.pump("os agentes", |d| d.initialized);
    // Draft (without "Start now"): it only shows in the list; starting runs in a thread.
    let id = env.create_from_form("Draft", false);
    assert_eq!(env.task(&id).status, TaskStatus::Draft);
    assert_eq!(actions_for(&env.task(&id), false), [TaskAct::Start, TaskAct::Discard]);
    env.deck.tasks_act(&id, TaskAct::Start);
    env.pump("the start", |d| d.tasks.busy.is_none());
    assert_eq!(env.task(&id).status, TaskStatus::Running);
    env.agent_commits(&id, "x.txt", "x\n");
    env.core.task_done(&env.project(), &id, "Done.").expect("done");
    env.pump("review", |d| d.tasks.task("t1").is_some_and(|t| t.status == TaskStatus::Review));

    // A dirty main repository refuses the merge: the core's message becomes the red notice. Through the
    // card's button (in the list), the refusal opens the detail, which is where the notice shows.
    std::fs::write(env.ws.join("a.txt"), "changed\n").expect("w");
    env.deck.tasks.view = TasksView::List;
    env.deck.tasks_act(&id, TaskAct::Merge);
    env.deck.tasks_confirm_accepted();
    env.pump("the refusal", |d| d.tasks.busy.is_none());
    let notice = env.deck.tasks.notice.clone().expect("notice");
    assert!(notice.contains("uncommitted changes") && notice.contains("a.txt"), "{notice}");
    assert_eq!(env.deck.tasks.view, TasksView::Detail(id.clone()), "a refusal coming from the list opens the detail");
    assert_eq!(env.task(&id).status, TaskStatus::Review, "it stays in review");

    // Discard: confirmation first (cancelling does nothing), then it runs and closes everything.
    env.deck.tasks_act(&id, TaskAct::Discard);
    assert_eq!(env.deck.tasks.confirm, Some(Confirm::Discard(id.clone())));
    env.deck.tasks.confirm = None;
    assert_eq!(env.task(&id).status, TaskStatus::Review);
    env.deck.tasks_act(&id, TaskAct::Discard);
    env.deck.tasks_confirm_accepted();
    env.pump("the discard", |d| d.tasks.busy.is_none());
    let t = env.task(&id);
    assert_eq!(t.status, TaskStatus::Discarded);
    assert!(!t.worktree.exists());
    assert!(env.deck.tasks.notice.is_none(), "the old notice went away with the new operation");
}

#[test]
fn a_refused_merge_offers_the_way_out_in_the_notice() {
    if !git_available() {
        eprintln!("git is not installed: test skipped");
        return;
    }
    let mut env = Env::start();
    env.pump("os agentes", |d| d.initialized);
    let id = env.create_from_form("Ways out of the refusal", true);
    env.pump("the tab", |d| d.tasks.task("t1").is_some_and(|t| d.tasks_tab_open(t)));
    env.agent_commits(&id, "x.txt", "x\n");
    env.core.task_done(&env.project(), &id, "Done.").expect("done");
    env.pump("review", |d| d.tasks.task("t1").is_some_and(|t| t.status == TaskStatus::Review));

    // The user's changes in the main folder: the notice brings the reason and "Stash my changes and merge".
    std::fs::write(env.ws.join("a.txt"), "changed\n").expect("w");
    env.deck.tasks_act(&id, TaskAct::Merge);
    env.deck.tasks_confirm_accepted();
    env.pump("the refusal", |d| d.tasks.busy.is_none());
    assert_eq!(env.deck.tasks.notice_block, Some(MergeBlock::MainDirty(vec!["a.txt".into()])));
    assert!(env.deck.tasks.notice.is_some() && !env.deck.tasks.notice_info);
    // "Ask the agent" does not apply to what belongs to the user.
    env.deck.tasks_ask_agent(&id);
    assert!(env.deck.tasks.busy.is_none() && env.task(&id).status == TaskStatus::Review);
    // The click stashes, merges and gives back: the notice becomes the note (information, no reason), the task goes to validation.
    env.deck.tasks_merge_stash(&id);
    assert!(env.deck.tasks.busy.is_some() && env.deck.tasks.notice.is_none() && env.deck.tasks.notice_block.is_none());
    env.pump("stash and merge", |d| d.tasks.busy.is_none());
    assert_eq!(env.task(&id).status, TaskStatus::Validating);
    assert!(env.ws.join("x.txt").exists(), "the merge brought the file");
    // (the user's git may convert the line breaks when giving back)
    let back = std::fs::read_to_string(env.ws.join("a.txt")).expect("a.txt");
    assert_eq!(back.replace("\r\n", "\n"), "changed\n", "the user's change came back");
    let note = env.deck.tasks.notice.clone().expect("nota");
    assert!(env.deck.tasks.notice_info && env.deck.tasks.notice_block.is_none() && note.contains("already back"), "{note}");

    // Rejected and delivered again, with work forgotten in the task folder: "Ask the agent to commit".
    git(&env.ws, &["checkout", "-q", "--", "a.txt"]);
    env.deck.tasks_act(&id, TaskAct::Reject);
    env.deck.tasks.feedback = "Adjust.".into();
    env.deck.tasks_send_return(&id);
    env.pump("the rejection", |d| d.tasks.busy.is_none());
    env.agent_commits(&id, "y.txt", "y\n");
    std::fs::write(env.task(&id).worktree.join("forgot.txt"), "z\n").expect("w");
    env.core.task_done(&env.project(), &id, "Again.").expect("done");
    env.pump("review 2", |d| d.tasks.task("t1").is_some_and(|t| t.status == TaskStatus::Review));
    env.deck.tasks_act(&id, TaskAct::Merge);
    env.deck.tasks_confirm_accepted();
    env.pump("refusal 2", |d| d.tasks.busy.is_none());
    assert_eq!(env.deck.tasks.notice_block, Some(MergeBlock::TaskDirty(vec!["forgot.txt".into()])));
    assert!(!env.deck.tasks.notice_info);
    env.deck.tasks_ask_agent(&id);
    assert!(env.deck.tasks.busy.is_some() && env.deck.tasks.notice_block.is_none(), "clears the notice and sends it back");
    env.pump("the return", |d| d.tasks.busy.is_none());
    let back = env.task(&id);
    assert_eq!(back.status, TaskStatus::Running);
    assert!(back.events.iter().any(|e| e.text.contains("returned to the agent")), "{:?}", back.events);
    assert!(env.deck.tasks.notice.is_none());
}

#[test]
fn reopening_the_tab_of_a_running_task_registers_the_instance_again() {
    if !git_available() {
        eprintln!("git is not installed: test skipped");
        return;
    }
    let mut env = Env::start();
    env.pump("os agentes", |d| d.initialized);
    let id = env.create_from_form("With a tab", true);
    env.pump("the tab", |d| d.tasks.task("t1").is_some_and(|t| d.tasks_tab_open(t)));
    // The user closes the tab: the task stays in progress, and the button becomes "Reopen tab".
    let instance = env.task(&id).instance;
    let idx = env.deck.tabs.iter().position(|t| t.ident.id() == instance).expect("tab");
    env.deck.close_tab(idx);
    assert!(!env.tab_open(&id));
    assert_eq!(actions_for(&env.task(&id), false), [TaskAct::ReopenTab, TaskAct::Discard]);
    env.deck.tasks_act(&id, TaskAct::ReopenTab);
    assert!(env.tab_open(&id), "the tab came back");
    assert!(env.deck.tasks.notice.is_none());
    // The terminal opens in the task's worktree (the core resolves it from the instance).
    let ident = agentdeck_core::Ident::parse(&instance).expect("id");
    let spec = env.core.spawn_spec_for(&env.project(), &ident).expect("spec");
    assert!(agentdeck_core::util::same_path(&spec.cwd, &env.task(&id).worktree), "{}", spec.cwd.display());
    // Go to tab only activates it; an already resolved task does not reopen the closed tab.
    env.deck.tasks_act(&id, TaskAct::GoToTab);
    assert_eq!(env.deck.active.map(|i| env.deck.tabs[i].ident.id()), Some(instance.clone()));
    let merged = Task { status: TaskStatus::Merged, ..env.task(&id) };
    assert!(env.deck.open_task_tab(&merged).is_ok(), "an open tab is only activated");
    env.deck.close_tab(env.deck.active.expect("ativa"));
    assert!(env.deck.open_task_tab(&merged).is_err());
    // Awaiting validation the agent stays alive: the closed tab can be reopened.
    let validating = Task { status: TaskStatus::Validating, ..env.task(&id) };
    assert!(env.deck.open_task_tab(&validating).is_ok() && env.tab_open(&id));
}

#[test]
fn a_new_task_gets_the_learnings_and_the_current_situation_and_feedbacks_come_back() {
    if !git_available() {
        eprintln!("git is not installed: test skipped");
        return;
    }
    let mut env = Env::start();
    env.pump("os agentes", |d| d.initialized);
    // The project already has learnings in the base: a global one, one for a.txt and one for another area.
    let dir = env.ws.join("memory").join("aprendizados");
    std::fs::create_dir_all(&dir).expect("dir");
    let write = |name: &str, touches: &str, body: &str| {
        let text = format!("---\ntitle: Learning {name}\nsummary: Summary {name}.\ntouches: {touches}\n---\n{body}\n");
        std::fs::write(dir.join(format!("{name}.md")), text).expect("w");
    };
    write("global", "[]", "Applies to everyone.");
    write("of-file", "[a.txt]", "Careful when touching a.txt.");
    write("of-other-area", "[crates/**]", "Only for the code.");
    git(&env.ws, &["add", "-A"]);
    git(&env.ws, &["commit", "-q", "-m", "docs: learnings"]);

    let project = env.project();
    let tasks_dir = env._tmp.path().join("dados").join("tarefas");
    fn create(env: &Env, project: &str, title: &str, brief: &str) -> String {
        let spec =
            agentdeck_core::TaskSpec { title: title.into(), brief: brief.into(), creator: Some("user".into()), ..Default::default() };
        let t = env.core.task_create(project, spec).expect("create");
        env.core.task_start(project, &t.id).expect("start");
        t.id
    }

    // The first task touches a.txt: no other tasks yet, with the base's commits.
    let t1 = create(&env, &project, "First", "Adjust a.txt and nothing else.");
    let first = std::fs::read_to_string(tasks_dir.join(format!("{project}-{t1}-brief.md"))).expect("t1 brief");
    assert!(first.contains("What the project has already learned:") && first.contains("### Learning of-file"), "{first}");
    assert!(first.contains("Careful when touching a.txt.") && first.contains("Applies to everyone."), "{first}");
    assert!(first.contains("Outras tarefas em andamento: nenhuma.") && first.contains("docs: learnings"), "{first}");
    // What does not match becomes only the index line.
    assert!(
        first.contains("- memory/aprendizados/of-other-area.md — Summary of-other-area.") && !first.contains("Only for the code."),
        "{first}"
    );

    // t1 already touched a.txt; t2 also names it, and still sees the warning.
    env.pump("t1 in the list", |d| d.tasks.task("t1").is_some());
    env.agent_commits(&t1, "a.txt", "two\n");
    let t2 = create(&env, &project, "Second", "Also touch `a.txt`.");
    let second = std::fs::read_to_string(tasks_dir.join(format!("{project}-{t2}-brief.md"))).expect("t2 brief");
    assert!(second.contains(&format!("- {t1} \"First\" (claude@{t1}, in progress): mexe em a.txt")), "{second}");
    assert!(second.contains("ATENÇÃO: o seu pedido cita a.txt"), "{second}");

    // The previous feedback comes back in the next return.
    env.pump("t2 in the list", |d| d.tasks.task("t2").is_some());
    env.agent_commits(&t2, "b.txt", "b\n");
    env.core.task_done(&project, &t2, "Done.").expect("done");
    env.core.task_return(&project, &t2, "First adjustment: rename.").expect("return 1");
    let after_first = std::fs::read_to_string(tasks_dir.join(format!("{project}-{t2}-feedback.md"))).expect("feedback 1");
    assert!(!after_first.contains("Earlier feedback"), "{after_first}");
    env.core.task_done(&project, &t2, "Done again.").expect("done 2");
    let back = env.core.task_return(&project, &t2, "Second adjustment: test.").expect("return 2");
    let after_second = std::fs::read_to_string(tasks_dir.join(format!("{project}-{t2}-feedback.md"))).expect("feedback 2");
    assert!(
        after_second.contains("1. [review] First adjustment: rename.") && after_second.contains("Second adjustment: test."),
        "{after_second}"
    );
    assert_eq!(back.feedbacks.len(), 2, "the returned task keeps both");
}

#[test]
fn feedback_brings_the_learnings_of_new_files_and_the_situation_lists_uncommitted_work() {
    if !git_available() {
        eprintln!("git is not installed: test skipped");
        return;
    }
    let mut env = Env::start();
    env.pump("os agentes", |d| d.initialized);
    let dir = env.ws.join("memory").join("aprendizados");
    std::fs::create_dir_all(&dir).expect("dir");
    for (name, touches, body) in
        [("global", "[]", "Applies to everyone."), ("of-a", "[a.txt]", "Careful with a."), ("of-b", "[b.txt]", "Careful with b.")]
    {
        let text = format!("---\ntitle: Learning {name}\nsummary: Summary {name}.\ntouches: {touches}\n---\n{body}\n");
        std::fs::write(dir.join(format!("{name}.md")), text).expect("w");
    }
    git(&env.ws, &["add", "-A"]);
    git(&env.ws, &["commit", "-q", "-m", "docs: learnings"]);

    let project = env.project();
    let tasks_dir = env._tmp.path().join("dados").join("tarefas");
    fn create(env: &Env, project: &str, title: &str, brief: &str) -> String {
        let spec =
            agentdeck_core::TaskSpec { title: title.into(), brief: brief.into(), creator: Some("user".into()), ..Default::default() };
        let t = env.core.task_create(project, spec).expect("create");
        env.core.task_start(project, &t.id).expect("start");
        t.id
    }
    let feedback_file = |id: &str| std::fs::read_to_string(tasks_dir.join(format!("{project}-{id}-feedback.md"))).expect("feedback");

    // t1 names only a.txt and delivers b.txt.
    let t1 = create(&env, &project, "First", "Adjust a.txt.");
    env.pump("t1 in the list", |d| d.tasks.task("t1").is_some());
    env.agent_commits(&t1, "b.txt", "b\n");
    env.core.task_done(&project, &t1, "Done.").expect("done");

    // A return naming a.txt (already in the request) and b.txt (new): only b.txt's learning goes, with no global one or index.
    env.core.task_return(&project, &t1, "Review a.txt and also b.txt.").expect("return");
    let first = feedback_file(&t1);
    assert!(first.contains("What the project has already learned about the cited files:"), "{first}");
    assert!(first.contains("Careful with b.") && !first.contains("Careful with a."), "{first}");
    assert!(!first.contains("Applies to everyone.") && !first.contains("Outros aprendizados"), "{first}");

    // b.txt was already named in a previous feedback: naming it again does not repeat the learning.
    env.core.task_done(&project, &t1, "Done again.").expect("done 2");
    env.core.task_return(&project, &t1, "Still missing in b.txt.").expect("return 2");
    let second = feedback_file(&t1);
    assert!(second.contains("Earlier feedback") && !second.contains("about the cited files"), "{second}");
    assert!(!second.contains("Careful with b."), "{second}");

    // The rejection follows the same rule: after merging, a.txt (request) and b.txt (feedback) were already seen.
    env.core.task_done(&project, &t1, "Ready.").expect("done 3");
    env.core.task_merge(&project, &t1).expect("merge");
    env.core.task_reject(&project, &t1, "It does not work: see a.txt and b.txt.").expect("reject");
    let rejected = feedback_file(&t1);
    assert!(rejected.contains("rejected in validation") && !rejected.contains("about the cited files"), "{rejected}");

    // A new task sees t2's still uncommitted work in the current situation.
    let t2 = create(&env, &project, "Second", "Do something else.");
    env.pump("t2 in the list", |d| d.tasks.task("t2").is_some());
    let work = env.task(&t2).worktree;
    std::fs::write(work.join("new.txt"), "x\n").expect("w");
    std::fs::write(work.join("a.txt"), "changed\n").expect("w");
    let t3 = create(&env, &project, "Third", "Nothing.");
    let brief = std::fs::read_to_string(tasks_dir.join(format!("{project}-{t3}-brief.md"))).expect("t3 brief");
    assert!(brief.contains(&format!("- {t2} \"Second\" (claude@{t2}, in progress): mexe em a.txt, new.txt")), "{brief}");
}

#[test]
fn files_markdown_and_commits_open_in_the_center_and_the_files_reload_when_they_change() {
    if !git_available() {
        eprintln!("git is not installed: test skipped");
        return;
    }
    let mut env = Env::start();
    env.pump("the agents and the git status", |d| d.initialized && d.git.status().is_some());
    let project = env.project();
    std::fs::write(env.ws.join("a.rs"), "fn a() {}\n\tfn b() {}\n").expect("w");
    std::fs::write(env.ws.join("note.md"), "# Title\n\n- [ ] one\n").expect("w");
    std::fs::write(env.ws.join("bin.dat"), b"MZ\0\0").expect("w");
    let wait = |env: &mut Env, uid: u64| {
        env.pump("o documento", |d| !matches!(d.docs.get(uid).map(|t| &t.state), Some(DocState::Loading)));
    };

    // Double click in the explorer: text, markdown and binary.
    env.deck.tree_file_double_click("a.rs");
    let rs = env.deck.docs.current_uid(&project).expect("a.rs");
    wait(&mut env, rs);
    // A small file opens as code (reads, searches and edits), with editing off.
    assert!(matches!(env.doc_state(rs), DocState::Source(s) if s.text == "fn a() {}\n\tfn b() {}\n" && !s.editing && !s.dirty()));
    env.deck.tree_file_double_click("note.md");
    let md = env.deck.docs.current_uid(&project).expect("note.md");
    wait(&mut env, md);
    assert!(matches!(env.doc_state(md), DocState::Source(s) if s.text.starts_with("# Title") && !s.raw));
    env.deck.tree_file_double_click("bin.dat");
    let bin = env.deck.docs.current_uid(&project).expect("bin.dat");
    wait(&mut env, bin);
    assert!(matches!(env.doc_state(bin), DocState::Unreadable(m) if m.contains("binário")));
    env.deck.tree_file_double_click("gone.txt");
    let gone = env.deck.docs.current_uid(&project).expect("gone.txt");
    wait(&mut env, gone);
    assert!(matches!(env.doc_state(gone), DocState::Failed(m) if m.contains("gone.txt")));
    assert_eq!(env.deck.docs.count(&project), 4);
    // Opening the same file only focuses it.
    env.deck.tree_file_double_click("a.rs");
    assert_eq!((env.deck.docs.count(&project), env.deck.docs.current_uid(&project)), (4, Some(rs)));

    // The file changes on disk (the watcher says so): the document is re-read without going through "Loading…".
    std::fs::write(env.ws.join("a.rs"), "fn a() {}\nfn c() {}\n").expect("w");
    env.deck.docs_reload_files();
    assert!(matches!(env.doc_state(rs), DocState::Source(_)), "the old read stays on screen until the new one arrives");
    env.pump(
        "the re-read file",
        |d| matches!(d.docs.get(rs).map(|t| &t.state), Some(DocState::Source(s)) if s.text == "fn a() {}\nfn c() {}\n"),
    );
    // The file that disappeared shows as an error; the one that came back, read again.
    std::fs::write(env.ws.join("gone.txt"), "back\n").expect("w");
    env.deck.docs_reload_files();
    env.pump("the file that came back", |d| matches!(d.docs.get(gone).map(|t| &t.state), Some(DocState::Source(_))));

    // The diff of a commit (History) and the one of a changed file (Git tab).
    git(&env.ws, &["add", "-A"]);
    git(&env.ws, &["commit", "-q", "-m", "feat: new files"]);
    let out = Command::new("git").current_dir(&env.ws).args(["rev-parse", "HEAD"]).output().expect("rev-parse");
    let hash = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    env.deck.docs_open_commit(&hash, &hash[..7], "feat: new files");
    let commit = env.deck.docs.current_uid(&project).expect("commit");
    wait(&mut env, commit);
    let DocState::Diff(doc) = env.doc_state(commit) else { panic!("expected the commit diff") };
    let paths: Vec<&str> = doc.files.iter().map(|f| f.path.as_str()).collect();
    for expected in ["a.rs", "bin.dat", "note.md", "gone.txt"] {
        assert!(paths.contains(&expected), "{expected} is not in {paths:?}");
    }

    std::fs::write(env.ws.join("a.rs"), "fn a() {}\nfn d() {}\n").expect("w");
    let entry = agentdeck_core::git::GitEntry {
        path: "a.rs".into(),
        orig_path: None,
        index: '.',
        worktree: 'M',
        kind: agentdeck_core::git::EntryKind::Ordinary,
    };
    env.deck.docs_open_file_diff(&entry, false);
    let file_diff = env.deck.docs.current_uid(&project).expect("file diff");
    wait(&mut env, file_diff);
    let DocState::Diff(doc) = env.doc_state(file_diff) else { panic!("expected the file diff") };
    assert!(doc.diff.lines.iter().any(|l| l.text == "+fn d() {}"), "{:?}", doc.diff.lines);
    // The staged side of the same file is another document.
    env.deck.docs_open_file_diff(&entry, true);
    assert_ne!(env.deck.docs.current_uid(&project), Some(file_diff));

    // The document belongs to the project: a document closed during the read breaks nothing, and "Reload" re-reads.
    env.deck.docs_reload(rs);
    assert!(matches!(env.doc_state(rs), DocState::Source(_)), "a file opened as code is re-read without going through Loading");
    env.deck.docs_close(rs);
    env.pump("the answer for a closed document", |d| d.docs.get(rs).is_none());
    env.deck.before_frame(&env.ctx);
}

#[test]
fn the_task_request_and_report_open_as_markdown_from_the_loaded_task() {
    if !git_available() {
        eprintln!("git is not installed: test skipped");
        return;
    }
    let mut env = Env::start();
    env.pump("the agents and the git status", |d| d.initialized && d.git.status().is_some());
    let id = env.create_from_form("Request and report", true);
    env.agent_commits(&id, "new.txt", "two\n");
    env.core.task_done(&env.project(), &id, "## Done\n\n- [x] I created new.txt").expect("done");
    env.pump("the review", |d| d.tasks.task("t1").is_some_and(|t| t.status == TaskStatus::Review));
    let project = env.project();

    env.deck.docs_open_task_text(&id, false);
    let brief = env.deck.docs.current_uid(&project).expect("request");
    assert!(
        matches!(env.doc_state(brief), DocState::Markdown(t) if t == "Make the request."),
        "the request already comes from the loaded task"
    );
    env.deck.docs_open_task_text(&id, true);
    let report = env.deck.docs.current_uid(&project).expect("report");
    assert!(matches!(env.doc_state(report), DocState::Markdown(t) if t.starts_with("## Done")));
    assert_ne!(brief, report);
    // They are not re-read from disk: the reload button does not exist for them.
    env.deck.docs_reload(report);
    assert!(matches!(env.doc_state(report), DocState::Markdown(_)));
    assert_eq!(env.deck.docs.view, CenterView::Docs);
}

#[test]
fn reading_documents_keeps_the_terminals_running_unresized_and_marks_finished_turns_as_unseen() {
    if !git_available() {
        eprintln!("git is not installed: test skipped");
        return;
    }
    let mut env = Env::start();
    env.pump("the agents and the git status", |d| d.initialized && d.git.status().is_some());
    // The painting uses the app's fonts (the `ui-semibold` family…).
    crate::theme::install_fonts(&env.ctx);
    crate::theme::install_style(&env.ctx);
    let mut out = env.ctx.run_ui(egui::RawInput::default(), |_| {});
    out.textures_delta.clear();
    let id = env.create_from_form("Agent that works", true);
    env.pump("the agent's tab", |d| d.tasks.task("t1").is_some_and(|t| d.tasks_tab_open(t)));
    let project = env.project();
    let instance = env.task(&id).instance;
    let frame = |env: &mut Env, w: f32, h: f32| {
        let input =
            egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, h))), ..Default::default() };
        let Env { deck, ctx, .. } = env;
        let mut out = ctx.run_ui(input, |ui| crate::ui::draw(deck, ui));
        out.textures_delta.clear();
    };
    let tab = |env: &Env| env.deck.tabs.iter().position(|t| t.project == project && t.ident.id() == instance).expect("tab");

    // With the agents in view, the terminal fits the center and the PTY changes after the size stops.
    frame(&mut env, 1280.0, 860.0);
    let i = tab(&env);
    assert!(env.deck.tabs[i].resize_pending(), "the terminal asks for the center's size");
    env.pump("the PTY to change size", |d| !d.tabs.iter().any(|t| t.resize_pending()));
    let size = env.deck.tabs[i].session.size();

    // Reading documents: the window changes size and the PTY is not touched (neither with the center hidden, nor at zero).
    env.deck.docs_open_file("a.txt");
    env.deck.before_frame(&env.ctx);
    assert_eq!(env.deck.docs.view, CenterView::Docs);
    assert!(!env.deck.term_focus, "the terminal does not get the keyboard while hidden");
    frame(&mut env, 900.0, 600.0);
    frame(&mut env, 1100.0, 700.0);
    let i = tab(&env);
    assert!(!env.deck.tabs[i].resize_pending(), "no resize requested with the terminal hidden");
    assert_eq!(env.deck.tabs[i].session.size(), size);

    // The agent finishes a turn while reading: it counts as unseen, even though it is the active tab.
    assert_eq!(env.deck.agents_badge(), crate::docs::Badge::default());
    env.deck.on_agent_signal(&project, &instance, true);
    assert_eq!(env.deck.agents_badge(), crate::docs::Badge { done: 0, busy: 1 });
    env.deck.on_agent_signal(&project, &instance, false);
    assert!(env.deck.tabs[i].done);
    assert_eq!(env.deck.agents_badge(), crate::docs::Badge { done: 1, busy: 0 });
    // Activating the tab without leaving the documents (e.g. an attachment) does not mark it as seen.
    env.deck.activate(i);
    assert!(env.deck.tabs[i].done && !env.deck.term_focus);

    // Going back to the agents (the switch, the shortcut or clicking an agent): the active tab is seen and the terminal gets the keyboard.
    env.deck.show_agents();
    env.deck.before_frame(&env.ctx);
    assert!(!env.deck.tabs[i].done && env.deck.term_focus);
    frame(&mut env, 1100.0, 700.0);
    assert!(env.deck.tabs[tab(&env)].resize_pending(), "on going back, the terminal fits the new size");

    // The shortcut toggles (only with documents open) and Ctrl+Tab cycles through the documents in Documents mode.
    env.deck.push_action(crate::deck::Action::ToggleCenter);
    env.deck.before_frame(&env.ctx);
    assert_eq!(env.deck.docs.view, CenterView::Docs);
    env.deck.docs_open_file("b.txt");
    let second = env.deck.docs.current_uid(&project);
    env.deck.push_action(crate::deck::Action::CycleTab { back: false });
    env.deck.before_frame(&env.ctx);
    assert_ne!(env.deck.docs.current_uid(&project), second);
    env.deck.push_action(crate::deck::Action::ToggleCenter);
    env.deck.before_frame(&env.ctx);
    assert_eq!(env.deck.docs.view, CenterView::Agents);
}

/// "Quit" from the tray: with an agent in the middle of a turn the app asks first (without the question the turn would be lost), and the
/// second launch's request arrives as `CoreEvent::Activate`.
#[test]
fn quitting_asks_while_an_agent_is_in_the_middle_of_a_turn_and_the_second_launch_asks_for_the_window() {
    if !git_available() {
        eprintln!("git is not installed: test skipped");
        return;
    }
    let mut env = Env::start();
    env.pump("the agents and the git status", |d| d.initialized && d.git.status().is_some());
    let id = env.create_from_form("Agent that works", true);
    env.pump("the agent's tab", |d| d.tasks.task("t1").is_some_and(|t| d.tasks_tab_open(t)));
    let (project, instance) = (env.project(), env.task(&id).instance);

    env.deck.on_agent_signal(&project, &instance, true);
    assert_eq!(env.deck.working_tabs().len(), 1);
    env.deck.request_quit();
    assert!(!env.deck.quit_requested, "in the middle of a turn the app asks before quitting");
    assert_eq!(env.deck.docs.confirm, Some(crate::docs::Unsaved::Busy));
    env.deck.docs_answer(crate::docs::Answer::Cancel);
    assert!(!env.deck.quit_requested && env.deck.docs.confirm.is_none());
    env.deck.request_quit();
    env.deck.docs_answer(crate::docs::Answer::Discard);
    assert!(env.deck.quit_requested, "\"Quit anyway\"");

    env.deck.quit_requested = false;
    env.deck.on_agent_signal(&project, &instance, false);
    env.deck.request_quit();
    assert!(env.deck.quit_requested, "with no turn in progress there is nothing to ask");

    assert!(!env.deck.activate_requested);
    env.deck.on_core_event(agentdeck_core::CoreEvent::Activate);
    assert!(env.deck.activate_requested);
}

#[test]
fn the_highlight_follows_the_active_tab_and_the_pty_resizes_once_per_switch() {
    if !git_available() {
        eprintln!("git is not installed: test skipped");
        return;
    }
    use agentdeck_core::Organization;
    let mut env = Env::start();
    env.pump("the agents and the git status", |d| d.initialized && d.git.status().is_some());
    crate::theme::install_fonts(&env.ctx);
    crate::theme::install_style(&env.ctx);
    let mut out = env.ctx.run_ui(egui::RawInput::default(), |_| {});
    out.textures_delta.clear();
    let frame = |env: &mut Env| {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 860.0))),
            ..Default::default()
        };
        let Env { deck, ctx, .. } = env;
        let mut out = ctx.run_ui(input, |ui| crate::ui::draw(deck, ui));
        out.textures_delta.clear();
    };
    let tab_of = |env: &Env, kind: &str| env.deck.tabs.iter().position(|t| t.kind == kind).expect("tab");
    // Two CLIs (two groups), the last one opened is the selected one.
    env.deck.open_tab("claude", false);
    env.deck.open_tab("shell", false);
    env.deck.set_organization(Organization::Highlight);
    frame(&mut env);
    let big_group = |env: &Env| env.deck.panes.first().and_then(|p| p.groups.first()).cloned();
    assert_eq!(env.deck.panes.len(), 2);
    assert_eq!(big_group(&env).as_deref(), Some("shell"));
    let (big, small) = (env.deck.panes[0].rect, env.deck.panes[1].rect);
    assert!(big.width() > small.width() * 1.8, "the selected one's window is the big one: {big:?} × {small:?}");
    env.pump("each window's PTY to get its size", |d| !d.tabs.iter().any(|t| t.resize_pending()) && d.metrics.is_some());
    frame(&mut env);
    env.pump("the PTY to adjust", |d| !d.tabs.iter().any(|t| t.resize_pending()));

    // Still frames do not ask for a resize.
    for _ in 0..5 {
        frame(&mut env);
    }
    assert!(!env.deck.tabs.iter().any(|t| t.resize_pending()), "a frame with no change does not touch the PTY");

    // Selecting the other CLI swaps the windows: its own becomes the big one and the PTY changes once, with the size stopped.
    let (claude, shell) = (tab_of(&env, "claude"), tab_of(&env, "shell"));
    let (size_claude, size_shell) = (env.deck.tabs[claude].session.size(), env.deck.tabs[shell].session.size());
    env.deck.activate(claude);
    frame(&mut env);
    assert_eq!(big_group(&env).as_deref(), Some("claude"));
    assert_eq!(env.deck.panes[0].rect, big, "the highlight switches CLI, the big window stays in the same place");
    assert!(env.deck.tabs[claude].resize_pending() && env.deck.tabs[shell].resize_pending(), "both windows changed size");
    // Nothing is resized while the size is changing (still within the 150 ms): the PTY's size is the one from before.
    assert_eq!((env.deck.tabs[claude].session.size(), env.deck.tabs[shell].session.size()), (size_claude, size_shell));
    env.pump("the PTY to change size", |d| !d.tabs.iter().any(|t| t.resize_pending()));
    assert_ne!(env.deck.tabs[claude].session.size(), size_claude, "Claude is now in the big window");
    for _ in 0..5 {
        frame(&mut env);
    }
    assert!(!env.deck.tabs.iter().any(|t| t.resize_pending()), "after the switch, no resize per frame");

    // Changing the organization is saved and the right segment comes back on reopening (the choice goes to the Config).
    env.deck.persist_layout_now();
    assert_eq!(env.core.config().organization(), Organization::Highlight);
    env.deck.set_organization(Organization::Tabs);
    frame(&mut env);
    assert_eq!(env.deck.panes.len(), 1, "in Tabs there is a single window");
}

#[test]
fn switching_there_and_back_inside_the_delay_resizes_nothing() {
    if !git_available() {
        eprintln!("git is not installed: test skipped");
        return;
    }
    use agentdeck_core::Organization;
    let mut env = Env::start();
    env.pump("the agents and the git status", |d| d.initialized && d.git.status().is_some());
    crate::theme::install_fonts(&env.ctx);
    crate::theme::install_style(&env.ctx);
    let mut out = env.ctx.run_ui(egui::RawInput::default(), |_| {});
    out.textures_delta.clear();
    let frame = |env: &mut Env| {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 860.0))),
            ..Default::default()
        };
        let Env { deck, ctx, .. } = env;
        let mut out = ctx.run_ui(input, |ui| crate::ui::draw(deck, ui));
        out.textures_delta.clear();
    };
    let tab_of = |env: &Env, kind: &str| env.deck.tabs.iter().position(|t| t.kind == kind).expect("tab");
    env.deck.open_tab("claude", false);
    env.deck.open_tab("shell", false);
    env.deck.set_organization(Organization::Highlight);
    frame(&mut env);
    env.pump("each window's PTY to get its size", |d| !d.tabs.iter().any(|t| t.resize_pending()));
    let (claude, shell) = (tab_of(&env, "claude"), tab_of(&env, "shell"));
    let before = (env.deck.tabs[claude].session.size(), env.deck.tabs[shell].session.size());

    // Tab switched and switched back: both windows changed and came back within the deadline, and the PTY
    // stays at the size it already had (the pending one from the middle is cancelled).
    env.deck.activate(claude);
    frame(&mut env);
    assert!(env.deck.tabs[claude].resize_pending() && env.deck.tabs[shell].resize_pending(), "the windows swapped sizes");
    env.deck.activate(shell);
    frame(&mut env);
    assert!(!env.deck.tabs.iter().any(|t| t.resize_pending()), "going back to the current size cancels the resize");
    std::thread::sleep(Duration::from_millis(800));
    frame(&mut env);
    frame(&mut env);
    assert_eq!(
        (env.deck.tabs[claude].session.size(), env.deck.tabs[shell].session.size()),
        before,
        "going there and back reprints nothing"
    );
}

#[test]
fn five_quick_switches_resize_each_tab_at_most_once_to_the_final_size() {
    if !git_available() {
        eprintln!("git is not installed: test skipped");
        return;
    }
    use agentdeck_core::Organization;
    let mut env = Env::start();
    env.pump("the agents and the git status", |d| d.initialized && d.git.status().is_some());
    crate::theme::install_fonts(&env.ctx);
    crate::theme::install_style(&env.ctx);
    let mut out = env.ctx.run_ui(egui::RawInput::default(), |_| {});
    out.textures_delta.clear();
    let frame = |env: &mut Env| {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 860.0))),
            ..Default::default()
        };
        let Env { deck, ctx, .. } = env;
        let mut out = ctx.run_ui(input, |ui| crate::ui::draw(deck, ui));
        out.textures_delta.clear();
    };
    let tab_of = |env: &Env, kind: &str| env.deck.tabs.iter().position(|t| t.kind == kind).expect("tab");
    env.deck.open_tab("claude", false);
    env.deck.open_tab("shell", false);
    env.deck.set_organization(Organization::Highlight);
    frame(&mut env);
    env.pump("each window's PTY to get its size", |d| !d.tabs.iter().any(|t| t.resize_pending()));
    let (claude, shell) = (tab_of(&env, "claude"), tab_of(&env, "shell"));

    // Five switches in a row (each within the resize deadline): the pending one is always
    // replaced by the new size, so each tab changes size only once, to the final size.
    let started = Instant::now();
    for _ in 0..5 {
        let next = if env.deck.active == Some(claude) { shell } else { claude };
        env.deck.activate(next);
        frame(&mut env);
    }
    assert!(started.elapsed() < Duration::from_millis(600), "the five switches must fit in the deadline of one");
    let mut changes = [0u32; 2];
    let mut last: Vec<_> = [claude, shell].iter().map(|&i| env.deck.tabs[i].session.size()).collect();
    let limit = Instant::now() + Duration::from_secs(30);
    while env.deck.tabs.iter().any(|t| t.resize_pending()) {
        assert!(Instant::now() < limit, "timed out waiting for the PTY to adjust");
        env.deck.before_frame(&env.ctx);
        for (n, &i) in [claude, shell].iter().enumerate() {
            let size = env.deck.tabs[i].session.size();
            if size != last[n] {
                changes[n] += 1;
                last[n] = size;
            }
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    assert!(changes.iter().all(|&n| n <= 1), "each tab changes size at most once, not {changes:?}");
    assert!(changes.contains(&1), "the five switches end at a size different from the initial one: {changes:?}");
    let metrics = env.deck.metrics.clone().expect("the installed fonts");
    for pane in &env.deck.panes {
        let idx = env.deck.tabs.iter().position(|t| t.uid == pane.shown_uid).expect("the window's tab");
        assert_eq!(env.deck.tabs[idx].session.size(), metrics.grid_size_for(pane.body), "the PTY stays at the window's final size");
    }
}

#[test]
fn a_tab_that_never_stops_printing_gets_resized_at_the_three_second_ceiling() {
    if !git_available() {
        eprintln!("git is not installed: test skipped");
        return;
    }
    let mut env = Env::start();
    env.pump("the agents and the git status", |d| d.initialized && d.git.status().is_some());
    crate::theme::install_fonts(&env.ctx);
    crate::theme::install_style(&env.ctx);
    let mut out = env.ctx.run_ui(egui::RawInput::default(), |_| {});
    out.textures_delta.clear();
    let frame = |env: &mut Env, w: f32, h: f32| {
        let input =
            egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, h))), ..Default::default() };
        let Env { deck, ctx, .. } = env;
        let mut out = ctx.run_ui(input, |ui| crate::ui::draw(deck, ui));
        out.textures_delta.clear();
    };
    env.deck.open_tab("shell", false);
    frame(&mut env, 1400.0, 860.0);
    env.pump("the PTY to get its size", |d| !d.tabs.iter().any(|t| t.resize_pending()));
    let tab = env.deck.tabs.len() - 1;

    // The window changes size and the tab never stops writing: the next resize waits for the
    // output to stop for 300 ms, but an agent that prints without stopping cannot hold it forever: it goes out at the 3 s ceiling.
    frame(&mut env, 1000.0, 700.0);
    assert!(env.deck.tabs[tab].resize_pending(), "the window changed size and the PTY still waits");
    let started = Instant::now();
    while env.deck.tabs[tab].resize_pending() {
        assert!(started.elapsed() < Duration::from_secs(10), "the resize did not go out even at the 3 s ceiling");
        *env.deck.tabs[tab].session.shared.last_output.lock() = Some(Instant::now());
        env.deck.before_frame(&env.ctx);
        std::thread::sleep(Duration::from_millis(10));
    }
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(2500) && elapsed <= Duration::from_secs(4),
        "with output that never stops the resize goes out at the 3 s ceiling, not in {elapsed:?}"
    );
}

#[test]
fn editing_a_file_saves_it_as_it_was_and_asks_before_losing_what_was_typed() {
    if !git_available() {
        eprintln!("git is not installed: test skipped");
        return;
    }
    let mut env = Env::start();
    env.pump("the agents and the git status", |d| d.initialized && d.git.status().is_some());
    let project = env.project();
    let path = env.ws.join("edit.rs");
    std::fs::write(&path, "fn a() {}\r\nfn b() {}\r\n").expect("w");
    env.deck.docs_open_file("edit.rs");
    let uid = env.deck.docs.current_uid(&project).expect("edit.rs");
    env.pump("the file", |d| matches!(d.docs.get(uid).map(|t| &t.state), Some(DocState::Source(_))));
    let src = |env: &mut Env| match &mut env.deck.docs.get_mut(uid).expect("aberto").state {
        DocState::Source(s) => s.clone(),
        other => panic!("expected code, got {other:?}"),
    };
    let edit = |env: &mut Env, f: &dyn Fn(&mut crate::docs::Source)| {
        if let DocState::Source(s) = &mut env.deck.docs.get_mut(uid).expect("aberto").state {
            f(s);
        }
    };
    assert_eq!(src(&mut env).text, "fn a() {}\nfn b() {}\n", "the screen works with \\n");

    // Edit and save: the disk gets the new text with the same line ending (CRLF), no BOM, and the edit stops being dirty.
    edit(&mut env, &|s| {
        s.set_editing(true);
        s.text = "fn a() {}\nfn c() {}\n".into();
        s.touched();
    });
    assert!(env.deck.docs.get(uid).expect("aberto").dirty());
    env.deck.push_action(crate::deck::Action::SaveDoc);
    env.deck.before_frame(&env.ctx);
    assert!(src(&mut env).saving, "grava numa thread");
    env.pump("the save", |d| !d.docs.get(uid).is_some_and(|t| t.dirty() || matches!(&t.state, DocState::Source(s) if s.saving)));
    assert_eq!(std::fs::read(&path).expect("r"), b"fn a() {}\r\nfn c() {}\r\n");
    // The watcher's re-read of what was just saved warns of nothing.
    env.deck.docs_reload_files();
    env.pump("the re-read", |d| d.docs.get(uid).is_some_and(|t| matches!(&t.state, DocState::Source(s) if s.external.is_none())));
    assert!(!env.deck.docs.get(uid).expect("aberto").dirty());

    // An agent changes the file while there is an unsaved edit: it asks (keep mine / reload) and nothing is lost.
    edit(&mut env, &|s| {
        s.text = "fn a() {}\nfn mine() {}\n".into();
        s.touched();
    });
    std::fs::write(&path, "fn a() {}\r\nfn from_agent() {}\r\n").expect("w");
    env.deck.docs_reload_files();
    env.pump("the notice", |d| d.docs.get(uid).is_some_and(|t| matches!(&t.state, DocState::Source(s) if s.external.is_some())));
    assert_eq!(src(&mut env).text, "fn a() {}\nfn mine() {}\n", "what the user typed stays on screen");
    // Without choosing, Ctrl+S does not save over what the agent did.
    env.deck.docs_save(uid, crate::docs::After::Nothing);
    assert!(!src(&mut env).saving);
    assert_eq!(std::fs::read(&path).expect("r"), b"fn a() {}\r\nfn from_agent() {}\r\n");
    edit(&mut env, &|s| s.keep_mine());
    env.deck.docs_save(uid, crate::docs::After::Nothing);
    env.pump("the overwriting save", |d| {
        !d.docs.get(uid).is_some_and(|t| t.dirty() || matches!(&t.state, DocState::Source(s) if s.saving))
    });
    assert_eq!(std::fs::read(&path).expect("r"), b"fn a() {}\r\nfn mine() {}\r\n");

    // Closing the tab with an edit asks; cancelling keeps everything; "save" saves and closes; "don't save" closes without saving.
    edit(&mut env, &|s| {
        s.text.push_str("// more\n");
        s.touched();
    });
    env.deck.docs_request_close(uid);
    assert_eq!(env.deck.docs.confirm, Some(crate::docs::Unsaved::Tab(uid)));
    assert!(env.deck.docs.get(uid).is_some(), "the tab stays open while it asks");
    env.deck.docs_answer(crate::docs::Answer::Cancel);
    assert!(env.deck.docs.confirm.is_none() && env.deck.docs.get(uid).is_some() && env.deck.docs.get(uid).expect("aberto").dirty());
    env.deck.docs_request_close(uid);
    env.deck.docs_answer(crate::docs::Answer::Save);
    env.pump("save and close", |d| d.docs.get(uid).is_none());
    assert_eq!(std::fs::read(&path).expect("r"), b"fn a() {}\r\nfn mine() {}\r\n// more\r\n");
    env.deck.docs_open_file("edit.rs");
    let again = env.deck.docs.current_uid(&project).expect("edit.rs");
    env.pump("the file again", |d| matches!(d.docs.get(again).map(|t| &t.state), Some(DocState::Source(_))));
    if let Some(DocState::Source(s)) = env.deck.docs.get_mut(again).map(|t| &mut t.state) {
        s.text.push_str("// discarded\n");
    }
    env.deck.docs_request_close(again);
    env.deck.docs_answer(crate::docs::Answer::Discard);
    assert!(env.deck.docs.get(again).is_none());
    assert_eq!(std::fs::read(&path).expect("r"), b"fn a() {}\r\nfn mine() {}\r\n// more\r\n", "discarding does not save");

    // Closing the app: with no edit, it quits straight; with an edit, it asks (and "quit without saving" quits).
    env.deck.docs_open_file("edit.rs");
    let third = env.deck.docs.current_uid(&project).expect("edit.rs");
    env.pump("the file once more", |d| matches!(d.docs.get(third).map(|t| &t.state), Some(DocState::Source(_))));
    if let Some(DocState::Source(s)) = env.deck.docs.get_mut(third).map(|t| &mut t.state) {
        s.text.push_str("// not saved\n");
    }
    env.deck.request_quit();
    assert!(!env.deck.quit_requested, "with an unsaved edit the app asks before quitting");
    assert_eq!(env.deck.docs.confirm, Some(crate::docs::Unsaved::Quit));
    env.deck.docs_answer(crate::docs::Answer::Cancel);
    assert!(!env.deck.quit_requested);
    env.deck.request_quit();
    env.deck.docs_answer(crate::docs::Answer::Save);
    env.pump("save everything and quit", |d| d.quit_requested);
    assert!(std::fs::read(&path).expect("r").ends_with(b"// not saved\r\n"));
    env.deck.quit_requested = false;
    env.deck.request_quit();
    assert!(env.deck.quit_requested, "with no edit there is nothing to ask");
}

/// A frame of the whole window (1280 × 860) with the `events` events; returns what was copied to the clipboard.
fn draw_frame(env: &mut Env, events: Vec<egui::Event>) -> Vec<String> {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 860.0))),
        events,
        ..Default::default()
    };
    let Env { deck, ctx, .. } = env;
    let mut out = ctx.run_ui(input, |ui| crate::ui::draw(deck, ui));
    out.textures_delta.clear();
    out.platform_output
        .commands
        .iter()
        .filter_map(|c| match c {
            egui::OutputCommand::CopyText(t) => Some(t.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_editor_takes_typing_the_search_and_the_diff_selects_and_copies() {
    if !git_available() {
        eprintln!("git is not installed: test skipped");
        return;
    }
    let mut env = Env::start();
    env.pump("the agents and the git status", |d| d.initialized && d.git.status().is_some());
    crate::theme::install_fonts(&env.ctx);
    crate::theme::install_style(&env.ctx);
    let mut out = env.ctx.run_ui(egui::RawInput::default(), |_| {});
    out.textures_delta.clear();
    let project = env.project();
    std::fs::write(env.ws.join("e.rs"), "fn a() {}\nfn b() {}\n").expect("w");
    env.deck.docs_open_file("e.rs");
    let uid = env.deck.docs.current_uid(&project).expect("e.rs");
    env.pump("the file", |d| matches!(d.docs.get(uid).map(|t| &t.state), Some(DocState::Source(_))));
    let text = |env: &Env| match &env.deck.docs.get(uid).expect("aberto").state {
        DocState::Source(s) => (s.text.clone(), s.dirty(), s.version),
        other => panic!("expected code, got {other:?}"),
    };
    draw_frame(&mut env, vec![]);
    // In read-only mode the keyboard changes nothing.
    let editor = egui::Id::new(("doc-src", uid));
    env.ctx.memory_mut(|m| m.request_focus(editor));
    draw_frame(&mut env, vec![]);
    draw_frame(&mut env, vec![egui::Event::Text("Z".into())]);
    assert_eq!(text(&env).0, "fn a() {}\nfn b() {}\n", "read-only does not accept typing");

    // In edit mode, what is typed goes into the text, marks it as unsaved and counts a new version.
    if let Some(DocState::Source(s)) = env.deck.docs.get_mut(uid).map(|t| &mut t.state) {
        s.set_editing(true);
    }
    draw_frame(&mut env, vec![]);
    env.ctx.memory_mut(|m| m.request_focus(editor));
    draw_frame(&mut env, vec![]);
    let before = text(&env).2;
    draw_frame(&mut env, vec![egui::Event::Text("Z".into())]);
    let (typed, dirty, version) = text(&env);
    assert!(typed.contains('Z') && dirty && version > before, "{typed:?}");
    draw_frame(&mut env, vec![]);

    // Ctrl+F opens the search and the field gets the text; the galley marks the occurrences and the count shows.
    env.deck.push_action(crate::deck::Action::FindInDoc);
    env.deck.before_frame(&env.ctx);
    draw_frame(&mut env, vec![]);
    let find_field = egui::Id::new(("doc-find", uid));
    env.ctx.memory_mut(|m| m.request_focus(find_field));
    draw_frame(&mut env, vec![egui::Event::Text("fn".into())]);
    draw_frame(&mut env, vec![]);
    let tab = env.deck.docs.get(uid).expect("aberto");
    assert!(tab.find.open && tab.find.query == "fn" && tab.find.count() == 2, "{:?}", tab.find);

    // Diff: selects by dragging and copies with Ctrl+C only the code of the lines (no number, no + or −).
    let id = env.create_from_form("For the diff", true);
    env.agent_commits(&id, "novo.rs", "// one\nfn two() {}\n");
    let (diff_uid, _) = env.open_task_diff(&id);
    draw_frame(&mut env, vec![]);
    draw_frame(&mut env, vec![]);
    let at = |x: f32, y: f32| egui::Pos2::new(x, y);
    let click = |pos| vec![egui::Event::PointerMoved(pos)];
    let button =
        |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
    let (start, end) = (at(520.0, 125.0), at(820.0, 240.0));
    draw_frame(&mut env, click(start));
    draw_frame(&mut env, vec![button(start, true)]);
    draw_frame(&mut env, click(at(500.0, 160.0)));
    draw_frame(&mut env, click(end));
    draw_frame(&mut env, vec![button(end, false)]);
    let copied = draw_frame(&mut env, vec![egui::Event::Copy]);
    assert_eq!(copied.len(), 1, "one Ctrl+C copies once: {copied:?}");
    assert!(!copied[0].is_empty() && !copied[0].contains("+//") && !copied[0].starts_with('+'), "{:?}", copied[0]);
    assert!(copied[0].contains("fn two() {}"), "{:?}", copied[0]);
    let _ = diff_uid;
}

#[test]
fn every_document_state_draws_without_trouble() {
    if !git_available() {
        eprintln!("git is not installed: test skipped");
        return;
    }
    let mut env = Env::start();
    env.pump("the agents and the git status", |d| d.initialized && d.git.status().is_some());
    crate::theme::install_fonts(&env.ctx);
    crate::theme::install_style(&env.ctx);
    let mut out = env.ctx.run_ui(egui::RawInput::default(), |_| {});
    out.textures_delta.clear();
    for item in [
        "docs-demo",
        "docs-demo-file",
        "docs-demo-md",
        "docs-demo-report",
        "docs-demo-unreadable",
        "docs-demo-many",
        "docs-demo-edit",
        "docs-demo-find",
        "docs-demo-diff-find",
        "docs-demo-external",
        "docs-demo-confirm",
        "docs-demo-large",
        "docs-demo-md-code",
    ] {
        env.deck.docs = crate::docs::Docs::default();
        crate::docs_demo::apply(&mut env.deck, item);
        draw_frame(&mut env, vec![]);
        draw_frame(&mut env, vec![]);
    }
}

/// Runs only the frameless work (`Deck::background`, what the loop does with the window hidden or minimized) until `done`.
/// Returns the last deadline requested.
fn background_until(env: &mut Env, what: &str, done: impl Fn(&Deck) -> bool) -> Option<Instant> {
    let limit = Instant::now() + Duration::from_secs(30);
    loop {
        let next = env.deck.background();
        if done(&env.deck) {
            return next;
        }
        assert!(Instant::now() < limit, "timed out waiting for: {what}");
        std::thread::sleep(Duration::from_millis(15));
    }
}

fn screen_has(deck: &Deck, tab: usize, needle: &str) -> bool {
    crate::terminal::record::grid_text(&deck.tabs[tab].session.shared.emu.lock()).contains(needle)
}

#[test]
fn a_hidden_window_still_types_the_queued_messages_and_the_late_answers_and_never_resizes_the_terminals() {
    if !git_available() {
        eprintln!("git is not installed: test skipped");
        return;
    }
    use agentdeck_core::Delivery;
    let mut env = Env::start();
    env.pump("the agents and the git status", |d| d.initialized && d.git.status().is_some());
    crate::theme::install_fonts(&env.ctx);
    crate::theme::install_style(&env.ctx);
    let mut out = env.ctx.run_ui(egui::RawInput::default(), |_| {});
    out.textures_delta.clear();
    // A real terminal (the PTY, the output and the echo are the system's) with the window still in view: one frame fits it.
    env.deck.open_tab("shell", false);
    let input =
        egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 860.0))), ..Default::default() };
    let Env { deck, ctx, .. } = &mut env;
    let mut out = ctx.run_ui(input, |ui| crate::ui::draw(deck, ui));
    out.textures_delta.clear();
    env.pump("the PTY to get its size", |d| !d.tabs.iter().any(|t| t.resize_pending()));
    let tab = env.deck.tabs.len() - 1;
    let (project, ident) = (env.project(), env.deck.tabs[tab].ident.clone());
    let size = env.deck.tabs[tab].session.size();
    // The hook signal (as Claude and Codex do) takes the "idle terminal" wait out of the way.
    env.deck.on_agent_signal(&project, &ident.id(), true);
    env.deck.on_agent_signal(&project, &ident.id(), false);
    let delivery = |prompt: &str, answer: bool| Delivery {
        request: 0,
        project: project.clone(),
        ident: ident.clone(),
        create: false,
        from: "other".into(),
        prompt: prompt.into(),
        answer,
        boot: None,
    };

    // Minimized window: no frame, only `background`. Another agent's message is typed when the tab becomes free.
    background_until(&mut env, "the terminal to speak", |d| {
        d.tabs[tab].session.shared.output_seq.load(std::sync::atomic::Ordering::Relaxed) > 0
    });
    env.deck.on_deliver(delivery("echo mensagem-minimizada-1", false));
    background_until(&mut env, "the message to be typed", |d| screen_has(d, tab, "mensagem-minimizada-1"));

    // The "typed later" answer goes out the same way. (The tab is a shell's, which the rule refuses for safety: it is made to
    // pass for a CLI just for the rule; the PTY is still a shell's.)
    env.deck.tabs[tab].kind = "claude".into();
    background_until(&mut env, "the tab to become free", |d| !d.tabs[tab].busy);
    env.deck.on_deliver(delivery("echo resposta-tardia-2", true));
    background_until(&mut env, "the answer to be typed", |d| screen_has(d, tab, "resposta-tardia-2"));

    // With no frame at all, the terminal neither changes size nor asks to.
    assert!(!env.deck.tabs[tab].resize_pending(), "background does not fit terminals to the window size");
    assert_eq!(env.deck.tabs[tab].session.size(), size);

    // With no activity the work ends: no deadline, and the loop sleeps until a core event (no wasted CPU).
    let limit = Instant::now() + Duration::from_secs(30);
    while env.deck.background().is_some() {
        assert!(Instant::now() < limit, "there are still deadlines scheduled with the tab idle");
        std::thread::sleep(Duration::from_millis(50));
    }
}
