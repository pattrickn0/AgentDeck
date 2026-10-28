//! The tasks desk with real git (skips with a notice if `git` is not installed): create, start,
//! report, return, merge, validate, reject, discard and persistence. The bridge is in `tasks_bridge.rs`.
//!
//! The tests run in English (the default language). A few assertions quote texts that belong to other areas
//! (`capability.rs`, `agents.rs`) and stay as those areas have them.
use std::process::Command;

use agentdeck_core::models::Effort;
use agentdeck_core::util::same_path;
use agentdeck_core::{CoreEvent, FeedEntry, FeedKind, Ident, MergeBlock, TaskError, TaskSpec, TaskStatus};

#[path = "support/tasks_env.rs"]
mod env;
use env::*;

macro_rules! need_git {
    () => {
        if git_exe().is_none() {
            eprintln!("git not installed: test skipped");
            return;
        }
    };
}

#[test]
fn create_validates_and_records_the_task() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = core
        .task_create(
            &env.project,
            TaskSpec { model: Some("opus".into()), creator: Some("usuário".into()), ..spec("  Fix the button\n of Action! ") },
        )
        .expect("create");
    assert_eq!((t.id.as_str(), t.status), ("t1", TaskStatus::Draft));
    assert_eq!(t.title, "Fix the button of Action!");
    assert_eq!((t.base.as_str(), t.kind.as_str(), t.model.as_deref()), ("main", "claude", Some("opus")));
    assert_eq!(t.branch, "tarefa/t1-fix-the-button-of-action");
    assert_eq!(t.instance, "claude@t1");
    assert!(same_path(&t.worktree, &env.ws.join(".agentdeck").join("worktrees").join("t1")), "{}", t.worktree.display());
    assert!(t.created_ms > 0 && t.started_ms.is_none() && t.events.len() == 1);
    // Draft: nothing was created in git.
    assert!(!t.worktree.exists());
    assert_eq!(branches(&env.ws), ["main"]);
    assert!(evs_tasks_changed(&events(&env)), "TasksChanged on every write");
    // The next number; reading returns the same.
    let t2 = core.task_create(&env.project, spec("Another")).expect("create 2");
    assert_eq!(t2.id, "t2");
    assert_eq!(core.tasks(&env.project).iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["t1", "t2"]);
    assert_eq!(core.task(&env.project, "T1").map(|t| t.title), Some("Fix the button of Action!".to_owned()));
    assert!(core.task(&env.project, "t9").is_none() && core.task(&env.project, "claude").is_none());
    // The requested base must be a local branch.
    git(&env.ws, &["branch", "stable"]);
    let on_base = core.task_create(&env.project, TaskSpec { base: Some("stable".into()), ..spec("On stable") }).expect("requested base");
    assert_eq!(on_base.base, "stable");

    let invalid = |s: TaskSpec| match core.task_create(&env.project, s) {
        Err(TaskError::Invalid(m)) => m,
        other => panic!("expected Invalid, got {other:?}"),
    };
    assert!(invalid(spec("  \n ")).contains("title"));
    assert!(invalid(TaskSpec { kind: "shell".into(), ..spec("x") }).contains("unknown or not installed agent"));
    assert!(invalid(TaskSpec { kind: "opencode".into(), ..spec("x") }).contains("not installed"));
    assert!(invalid(TaskSpec { kind: "claude@dev".into(), ..spec("x") }).contains("give only the model"));
    assert!(invalid(TaskSpec { model: Some("a b;c".into()), ..spec("x") }).contains("invalid model"));
    assert!(invalid(TaskSpec { kind: "aider".into(), model: Some("gpt".into()), ..spec("x") })
        .contains("the agent aider does not accept choosing a model"));
    assert!(invalid(TaskSpec { base: Some("does-not-exist".into()), ..spec("x") }).contains("is not a local branch"));
    assert!(invalid(TaskSpec { base: Some("--exec".into()), ..spec("x") }).contains("is not a local branch"));
    // None of that spent a number: the next one is t4.
    assert_eq!(core.task_create(&env.project, spec("Fourth")).expect("create").id, "t4");
    core.shutdown();
}

fn evs_tasks_changed(evs: &[CoreEvent]) -> bool {
    evs.iter().any(|e| matches!(e, CoreEvent::TasksChanged { .. }))
}

#[test]
fn create_needs_a_repository_with_a_commit_and_a_branch() {
    need_git!();
    // A project that is not a repository.
    let env = start_with(true, false);
    match env.core.task_create(&env.project, spec("x")) {
        Err(TaskError::Invalid(m)) => assert!(m.contains("is not a git repository"), "{m}"),
        other => panic!("{other:?}"),
    }
    // A repository without any commit.
    git(&env.ws, &["init", "-q"]);
    match env.core.task_create(&env.project, spec("x")) {
        Err(TaskError::Invalid(m)) => assert!(m.contains("no commits yet"), "{m}"),
        other => panic!("{other:?}"),
    }
    // Detached HEAD, with no base given.
    std::fs::write(env.ws.join("a.txt"), "x").expect("w");
    git(&env.ws, &["add", "-A"]);
    git(&env.ws, &["commit", "-q", "-m", "initial"]);
    git(&env.ws, &["checkout", "-q", "--detach"]);
    match env.core.task_create(&env.project, spec("x")) {
        Err(TaskError::Invalid(m)) => assert!(m.contains("detached HEAD"), "{m}"),
        other => panic!("{other:?}"),
    }
    let ok = env.core.task_create(&env.project, TaskSpec { base: Some("main".into()), ..spec("x") });
    assert!(ok.is_ok(), "{ok:?}");
    // Unknown project.
    assert!(matches!(env.core.task_create("pxxxxxxxx", spec("x")), Err(TaskError::NotFound(_))));
    env.core.shutdown();
}

#[test]
fn start_makes_the_worktree_the_instance_and_delivers_the_brief() {
    need_git!();
    let env = start();
    let core = &env.core;
    // The requester is the open `claude` agent: it becomes the boss of the task's instance.
    let boss = core.register_instance(&env.project, "claude");
    let t = core
        .task_create(&env.project, TaskSpec { model: Some("opus".into()), creator: Some(boss.id()), ..spec("Fix the login") })
        .expect("create");
    events(&env);
    let t = core.task_start(&env.project, &t.id).expect("start");
    assert_eq!(t.status, TaskStatus::Running);
    assert!(t.started_ms.is_some() && t.error.is_none());

    // The worktree and the branch exist; the main repository stays clean (the folder is excluded by git).
    assert!(t.worktree.join(".git").exists() && t.worktree.join("a.txt").is_file());
    assert!(branches(&env.ws).contains(&t.branch), "{:?}", branches(&env.ws));
    assert_eq!(git(&t.worktree, &["symbolic-ref", "--short", "HEAD"]).trim(), t.branch);
    assert_eq!(worktree_count(&env.ws), 2);
    assert_eq!(git(&env.ws, &["status", "--porcelain"]).trim(), "", "the main repository stays clean");
    let exclude = std::fs::read_to_string(env.ws.join(".git").join("info").join("exclude")).expect("exclude");
    assert_eq!(exclude.lines().filter(|l| l.trim() == "/.agentdeck/").count(), 1, "{exclude}");
    assert!(!env.ws.join(".gitignore").exists(), "the user's .gitignore is not touched");

    // The instance: id, saved model, boss and the terminal in the worktree.
    assert!(core.project_instance_ids(&env.project).contains(&"claude@t1".to_owned()));
    let ident = Ident::parse("claude@t1").expect("id");
    assert_eq!(core.instance_model(&env.project, &ident).as_deref(), Some("opus"));
    assert_eq!(core.team(&env.project, "claude@t1").boss.as_deref(), Some("claude"));
    let spec_for = core.spawn_spec_for(&env.project, &ident).expect("spec");
    assert!(same_path(&spec_for.cwd, &t.worktree), "{}", spec_for.cwd.display());
    assert!(spec_for.args.windows(2).any(|w| w == ["--model", "opus"]), "{:?}", spec_for.args);
    assert_eq!(spec_for.env["AGENTDECK_AGENT"], "claude@t1");
    // Other instances of the project stay in the project folder.
    assert!(same_path(&core.spawn_spec_for(&env.project, &boss).expect("spec").cwd, &env.ws));

    // The brief went to the tab through the messages path (request 0), with the task rules.
    let evs = events(&env);
    let sent = deliveries(&evs);
    assert_eq!(sent.len(), 1, "{sent:?}");
    let (to, prompt) = &sent[0];
    assert_eq!(to, "claude@t1");
    assert!(prompt.contains("Do: Fix the login\n\nCarefully."), "{prompt}");
    assert!(prompt.contains(&t.worktree.display().to_string()) && prompt.contains(&t.branch), "{prompt}");
    assert!(prompt.contains("ask-agent --task done t1 --file") && prompt.contains("ask-agent --boss"), "{prompt}");
    assert!(!prompt.contains('\x1b'));
    assert!(evs.iter().any(|e| matches!(e, CoreEvent::Deliver(d) if d.create && d.from == "claude" && d.project == env.project)));

    // Starting again is not valid; the second start does not repeat the exclusion.
    assert!(matches!(core.task_start(&env.project, "t1"), Err(TaskError::Conflict(_))));
    let t2 = core.task_create(&env.project, spec("Second")).expect("create");
    core.task_start(&env.project, &t2.id).expect("start 2");
    let exclude = std::fs::read_to_string(env.ws.join(".git").join("info").join("exclude")).expect("exclude");
    assert_eq!(exclude.lines().filter(|l| l.trim() == "/.agentdeck/").count(), 1, "{exclude}");
    assert_eq!(worktree_count(&env.ws), 3);
    core.shutdown();
}

#[test]
fn start_refuses_without_tabs_and_recovers_from_a_failure() {
    need_git!();
    // Without the agents' tabs (headless): refuses with the explanation and leaves the draft as it is.
    let env = start_with(false, true);
    let t = env.core.task_create(&env.project, spec("No tabs")).expect("create");
    match env.core.task_start(&env.project, &t.id) {
        Err(TaskError::Conflict(m)) => assert!(m.contains("agents' tabs"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(env.core.task(&env.project, &t.id).map(|t| t.status), Some(TaskStatus::Draft));
    env.core.shutdown();

    // Failure on start: the base vanished. The task stays Failed with the message; when the base comes back, starting again works.
    let env = start();
    git(&env.ws, &["branch", "temporary"]);
    let t = env.core.task_create(&env.project, TaskSpec { base: Some("temporary".into()), ..spec("Will fail") }).expect("create");
    git(&env.ws, &["branch", "-q", "-D", "temporary"]);
    match env.core.task_start(&env.project, &t.id) {
        Err(TaskError::Failed(m)) => assert!(m.contains("no longer exists"), "{m}"),
        other => panic!("{other:?}"),
    }
    let failed = env.core.task(&env.project, &t.id).expect("task");
    assert_eq!(failed.status, TaskStatus::Failed);
    assert!(failed.error.as_deref().is_some_and(|e| e.contains("temporary")), "{failed:?}");
    assert!(failed.events.iter().any(|e| e.text.starts_with("error:")));
    assert!(deliveries(&events(&env)).is_empty(), "nothing was delivered");
    git(&env.ws, &["branch", "temporary", "main"]);
    let again = env.core.task_start(&env.project, &t.id).expect("new attempt");
    assert_eq!((again.status, again.error), (TaskStatus::Running, None));
    assert!(again.worktree.is_dir());
    env.core.shutdown();
}

#[test]
fn done_stores_the_report_and_the_diff_stat() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = core.task_create(&env.project, spec("Report")).expect("create");
    // Only a task in progress receives a report.
    assert!(matches!(core.task_done(&env.project, &t.id, "x"), Err(TaskError::Conflict(_))));
    let t = core.task_start(&env.project, &t.id).expect("start");
    assert!(matches!(core.task_done(&env.project, &t.id, " \n "), Err(TaskError::Invalid(_))));
    assert!(matches!(core.task_done(&env.project, "t9", "x"), Err(TaskError::NotFound(_))));
    commit_file(&t.worktree, "new.rs", "fn main() {}\n", "feat: new");
    std::fs::write(t.worktree.join("dirty.txt"), "forgot to commit").expect("w");
    let done = core.task_done(&env.project, &t.id, "I made new.rs.\nTests: ok.").expect("done");
    assert_eq!(done.status, TaskStatus::Review);
    assert_eq!(done.report.as_deref(), Some("I made new.rs.\nTests: ok."));
    let stat = done.diff_stat.expect("diff stat");
    assert!(stat.contains("new.rs") && stat.contains("1 file changed"), "{stat}");
    assert!(done.finished_ms.is_some());
    assert!(done.events.iter().any(|e| e.text.contains("uncommitted") && e.text.contains("dirty.txt")), "{:?}", done.events);
    assert!(done.events.iter().any(|e| e.text.contains("report received")));
    // In review, delivering again is not valid.
    assert!(matches!(core.task_done(&env.project, &t.id, "once more"), Err(TaskError::Conflict(_))));
    core.shutdown();
}

#[test]
fn task_diff_shows_only_what_the_branch_brought() {
    use agentdeck_core::git::DiffKind;
    need_git!();
    let env = start();
    let core = &env.core;
    let t = core.task_create(&env.project, spec("Diff")).expect("create");
    // Draft: the branch does not exist yet.
    assert!(matches!(core.task_diff(&env.project, &t.id), Err(TaskError::Conflict(_))));
    assert!(matches!(core.task_diff(&env.project, "t9"), Err(TaskError::NotFound(_))));
    let t = core.task_start(&env.project, &t.id).expect("start");
    commit_file(&t.worktree, "new.rs", "fn main() {}\n", "feat: new");
    // The base moves after the task left: the `base...branch` diff does not show what the base gained.
    commit_file(&env.ws, "from_base.txt", "only in the base\n", "chore: base");
    let diff = core.task_diff(&env.project, &t.id).expect("diff");
    assert!(!diff.truncated && !diff.binary);
    let added: Vec<&str> = diff.lines.iter().filter(|l| l.kind == DiffKind::Add).map(|l| l.text.as_str()).collect();
    assert_eq!(added, ["+fn main() {}"], "{:?}", diff.lines);
    let meta = diff.lines.iter().filter(|l| l.kind == DiffKind::Meta).map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");
    assert!(meta.contains("new.rs") && !meta.contains("from_base.txt"), "{meta}");
    // Discarded: the branch is gone, and so is the diff.
    core.task_discard(&env.project, &t.id).expect("discard");
    assert!(matches!(core.task_diff(&env.project, &t.id), Err(TaskError::Conflict(_))));
    core.shutdown();
}

#[test]
fn return_sends_the_feedback_and_goes_back_to_running() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = core.task_create(&env.project, spec("Return")).expect("create");
    assert!(matches!(core.task_return(&env.project, &t.id, "x"), Err(TaskError::Conflict(_))));
    let t = core.task_start(&env.project, &t.id).expect("start");
    assert!(matches!(core.task_return(&env.project, &t.id, "x"), Err(TaskError::Conflict(_))), "a task in progress is not returned");
    commit_file(&t.worktree, "a.rs", "1\n", "feat: a");
    core.task_done(&env.project, &t.id, "Ready.").expect("done");
    events(&env);

    assert!(matches!(core.task_return(&env.project, &t.id, "  "), Err(TaskError::Invalid(_))));
    let back = core.task_return(&env.project, &t.id, "The test for the empty case is missing.").expect("return");
    assert_eq!((back.status, back.finished_ms), (TaskStatus::Running, None));
    assert_eq!(back.report.as_deref(), Some("Ready."), "the previous report stays until the next one");
    let sent = deliveries(&events(&env));
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].0, "claude@t1");
    assert!(
        sent[0].1.contains("The test for the empty case is missing.") && sent[0].1.contains("ask-agent --task done t1 --file"),
        "{}",
        sent[0].1
    );
    // The cycle goes on: new commit, new report, and the tab (closed by hand in the middle) is registered again.
    core.close_instance(&env.project, "claude@t1");
    commit_file(&t.worktree, "b.rs", "2\n", "fix: b");
    let again = core.task_done(&env.project, &t.id, "Now it is right.").expect("done 2");
    assert_eq!((again.status, again.report.as_deref()), (TaskStatus::Review, Some("Now it is right.")));
    assert!(again.diff_stat.as_deref().is_some_and(|s| s.contains("a.rs") && s.contains("b.rs")));
    core.task_return(&env.project, &t.id, "One more tweak.").expect("return 2");
    assert!(core.project_instance_ids(&env.project).contains(&"claude@t1".to_owned()), "the instance is back in the core");
    core.shutdown();
}

#[test]
fn merge_makes_a_no_ff_commit_and_waits_for_validation() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = task_in_review(&env, "Merge this", "feature.txt");
    events(&env);
    // The app rewrites the AGENTS.md block in the project: that does not prevent the merge.
    let agents_md = env.ws.join("AGENTS.md");
    let text = std::fs::read_to_string(&agents_md).expect("AGENTS.md");
    std::fs::write(&agents_md, format!("{text}\n<!-- rewritten by the app -->\n")).expect("w");
    let head_before = git(&env.ws, &["rev-parse", "HEAD"]);

    let merged = core.task_merge(&env.project, &t.id).expect("merge");
    assert_eq!(merged.status, TaskStatus::Validating);
    assert!(merged.finished_ms.is_some() && merged.error.is_none());
    // Merge commit (two parents) with the task's message, on the base. (The subject stays `Merge tarefa`: see merge.rs.)
    assert_eq!(git(&env.ws, &["log", "-1", "--format=%s"]).trim(), "Merge tarefa t1: Merge this");
    assert_eq!(git(&env.ws, &["log", "-1", "--format=%P"]).split_whitespace().count(), 2, "--no-ff");
    assert_eq!(git(&env.ws, &["symbolic-ref", "--short", "HEAD"]).trim(), "main");
    assert!(env.ws.join("feature.txt").is_file());
    assert_ne!(git(&env.ws, &["rev-parse", "HEAD"]), head_before);
    // Nothing was cleaned up: worktree, folder, branch and instance remain; the tab was not closed.
    assert!(t.worktree.is_dir());
    assert_eq!(worktree_count(&env.ws), 2);
    assert_eq!(branches(&env.ws), ["main", t.branch.as_str()]);
    assert!(!events(&env).iter().any(|e| matches!(e, CoreEvent::CloseInstance { .. })));
    assert!(core.project_instance_ids(&env.project).contains(&"claude@t1".to_owned()));
    assert_eq!(core.task_of_instance(&env.project, "claude@t1").map(|t| t.status), Some(TaskStatus::Validating));
    assert!(
        merged.events.iter().any(|e| e.text.contains("merged into main") && e.text.contains("awaiting validation")),
        "{:?}",
        merged.events
    );
    // The task's branch was aligned with the base (fast-forward): the same commit, and the worktree already has the merge.
    assert_eq!(git(&t.worktree, &["rev-parse", "HEAD"]), git(&env.ws, &["rev-parse", "main"]));
    assert_eq!(git(&env.ws, &["rev-parse", &t.branch]), git(&env.ws, &["rev-parse", "main"]));
    // The terminal of a reopened tab stays in the worktree.
    let ident = Ident::parse("claude@t1").expect("id");
    assert!(same_path(&core.spawn_spec_for(&env.project, &ident).expect("spec").cwd, &t.worktree));
    // The AGENTS.md change stayed intact in the working directory.
    assert!(std::fs::read_to_string(&agents_md).expect("AGENTS.md").contains("rewritten by the app"));
    // Merging again is not valid; discarding is (and leaves the merge in the base).
    assert!(matches!(core.task_merge(&env.project, &t.id), Err(TaskError::Conflict(_))));
    core.shutdown();
}

#[test]
fn validate_cleans_up_and_completes() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = task_in_review(&env, "Validate this", "feature.txt");
    // Before merging it cannot be validated.
    match core.task_validate(&env.project, &t.id) {
        Err(TaskError::Conflict(m)) => assert!(m.contains("only a task awaiting validation can be validated"), "{m}"),
        other => panic!("{other:?}"),
    }
    core.task_merge(&env.project, &t.id).expect("merge");
    events(&env);

    let done = core.task_validate(&env.project, &t.id).expect("validate");
    assert_eq!(done.status, TaskStatus::Merged);
    assert!(done.finished_ms.is_some() && done.error.is_none());
    assert!(done.events.iter().any(|e| e.text.contains("validated")), "{:?}", done.events);
    // Cleanup: worktree, folder and branch gone; the tab was closed and the instance forgotten.
    assert!(!t.worktree.exists());
    assert_eq!(worktree_count(&env.ws), 1);
    assert_eq!(branches(&env.ws), ["main"]);
    assert!(events(&env).iter().any(|e| matches!(e, CoreEvent::CloseInstance { id, .. } if id == "claude@t1")));
    assert!(!core.project_instance_ids(&env.project).contains(&"claude@t1".to_owned()));
    assert!(env.ws.join("feature.txt").is_file(), "the merge stays in the base");
    // Completed is final: nothing else is valid, and the terminal of a new tab with that id goes back to the project folder.
    for r in [core.task_validate(&env.project, &t.id), core.task_merge(&env.project, &t.id), core.task_discard(&env.project, &t.id)] {
        assert!(matches!(r, Err(TaskError::Conflict(_))), "{r:?}");
    }
    assert!(matches!(core.task_reject(&env.project, &t.id, "x"), Err(TaskError::Conflict(_))));
    assert!(core.task_of_instance(&env.project, "claude@t1").is_none());
    let ident = Ident::parse("claude@t1").expect("id");
    assert!(same_path(&core.spawn_spec_for(&env.project, &ident).expect("spec").cwd, &env.ws));
    core.shutdown();
}

#[test]
fn validate_is_refused_while_there_is_unmerged_work() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = task_in_review(&env, "Loose work", "feature.txt");
    core.task_merge(&env.project, &t.id).expect("merge");
    let refused = |expect: &str| match core.task_validate(&env.project, &t.id) {
        Err(TaskError::Conflict(m)) => assert!(m.contains(expect), "{m}"),
        other => panic!("expected a refusal with \"{expect}\", got {other:?}"),
    };
    // The agent kept working: an uncommitted file...
    std::fs::write(t.worktree.join("loose.txt"), "x").expect("w");
    refused("uncommitted work (loose.txt)");
    // ...and a commit that the base does not have.
    git(&t.worktree, &["add", "-A"]);
    git(&t.worktree, &["commit", "-q", "-m", "feat: one more"]);
    refused("commits that were not merged");
    let now = core.task(&env.project, &t.id).expect("task");
    assert_eq!(now.status, TaskStatus::Validating, "{now:?}");
    assert!(now.worktree.is_dir() && branches(&env.ws).contains(&now.branch), "nothing was removed");
    assert!(now.events.iter().any(|e| e.text.contains("validation refused")), "{:?}", now.events);
    // Rejecting brings that work back into the normal cycle.
    core.task_reject(&env.project, &t.id, "Deliver what is missing.").expect("reject");
    core.task_done(&env.project, &t.id, "Now it is right.").expect("done");
    core.task_merge(&env.project, &t.id).expect("merge 2");
    assert_eq!(core.task_validate(&env.project, &t.id).expect("validate").status, TaskStatus::Merged);
    assert!(env.ws.join("loose.txt").is_file());
    core.shutdown();
}

#[test]
fn reject_returns_the_task_to_the_same_agent_in_the_same_worktree() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = task_in_review(&env, "Reject this", "feature.txt");
    // Outside awaiting validation it cannot be rejected.
    assert!(matches!(core.task_reject(&env.project, &t.id, "x"), Err(TaskError::Conflict(_))), "in review");
    core.task_merge(&env.project, &t.id).expect("merge");
    let base_head = git(&env.ws, &["rev-parse", "main"]);
    assert!(matches!(core.task_reject(&env.project, &t.id, " \n "), Err(TaskError::Invalid(_))), "no reason");
    assert_eq!(core.task(&env.project, &t.id).map(|t| t.status), Some(TaskStatus::Validating), "the refusal changes nothing");
    events(&env);

    let back = core.task_reject(&env.project, &t.id, "  When saving, the app closes.  ").expect("reject");
    assert_eq!((back.status, back.finished_ms), (TaskStatus::Running, None));
    assert_eq!(back.worktree, t.worktree);
    assert!(back.events.iter().any(|e| e.text.contains("rejected")), "{:?}", back.events);
    // The reason reaches the same agent, saying the work was already merged.
    let evs = events(&env);
    let sent = deliveries(&evs);
    assert_eq!(sent.len(), 1, "{sent:?}");
    let (to, prompt) = &sent[0];
    assert_eq!(to, "claude@t1");
    for needle in [
        "rejected in validation",
        "already merged into main",
        "rejected it for the following reason",
        "When saving, the app closes.",
        "same branch tarefa/t1-reject-this",
        "ask-agent --task done t1 --file",
    ] {
        assert!(prompt.contains(needle), "{needle}: {prompt}");
    }
    assert!(!prompt.contains("could not align"), "the branch was aligned: {prompt}");
    // The worktree stays the same, with the base's code (the merge included), without touching the base.
    assert!(t.worktree.join("feature.txt").is_file());
    assert_eq!(git(&env.ws, &["rev-parse", "main"]), base_head);
    // The cycle goes on: fix, deliver, review, merge, awaiting validation, validate.
    commit_file(&t.worktree, "fix.txt", "ok\n", "fix: the app does not close");
    let again = core.task_done(&env.project, &t.id, "Fixed.").expect("done");
    assert_eq!(again.status, TaskStatus::Review);
    assert!(again.diff_stat.as_deref().is_some_and(|s| s.contains("fix.txt") && !s.contains("feature.txt")), "{:?}", again.diff_stat);
    assert_eq!(core.task_merge(&env.project, &t.id).expect("merge 2").status, TaskStatus::Validating);
    assert!(env.ws.join("fix.txt").is_file());
    assert_eq!(core.task_validate(&env.project, &t.id).expect("validate").status, TaskStatus::Merged);
    assert!(!t.worktree.exists());
    core.shutdown();
}

#[test]
fn reject_reopens_the_tab_and_recreates_a_missing_worktree() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = task_in_review(&env, "Closed tab", "feature.txt");
    core.task_merge(&env.project, &t.id).expect("merge");
    // The user closed the tab (the core forgot the instance) and the folder was deleted by hand.
    core.close_instance(&env.project, &t.instance);
    std::fs::remove_dir_all(&t.worktree).expect("rm");
    events(&env);

    let back = core.task_reject(&env.project, &t.id, "It does not work.").expect("reject");
    assert_eq!(back.status, TaskStatus::Running);
    // The folder is back, on the task's branch and with the base's code; the delivery asks for the tab to open (`create`).
    assert!(t.worktree.is_dir() && t.worktree.join("feature.txt").is_file());
    assert_eq!(git(&t.worktree, &["symbolic-ref", "--short", "HEAD"]).trim(), t.branch);
    assert!(core.project_instance_ids(&env.project).contains(&"claude@t1".to_owned()), "the instance is back in the core");
    let evs = events(&env);
    assert!(evs.iter().any(|e| matches!(e, CoreEvent::Deliver(d) if d.request == 0 && d.create && d.ident.id() == "claude@t1")), "{evs:?}");
    assert!(same_path(&core.spawn_spec_for(&env.project, &Ident::parse("claude@t1").expect("id")).expect("spec").cwd, &t.worktree));
    // Without the folder AND without the branch (deleted from outside): recreates from the base.
    commit_file(&t.worktree, "x.txt", "x\n", "feat: x");
    core.task_done(&env.project, &t.id, "Again.").expect("done");
    core.task_merge(&env.project, &t.id).expect("merge 2");
    git(&env.ws, &["worktree", "remove", "--force", &t.worktree.to_string_lossy()]);
    git(&env.ws, &["branch", "-D", &t.branch]);
    core.task_reject(&env.project, &t.id, "Not yet.").expect("reject 2");
    assert!(t.worktree.join("x.txt").is_file());
    assert_eq!(git(&env.ws, &["rev-parse", &t.branch]), git(&env.ws, &["rev-parse", "main"]));
    core.shutdown();
}

#[test]
fn reject_needs_the_agent_tabs() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = task_in_review(&env, "No tabs", "feature.txt");
    core.task_merge(&env.project, &t.id).expect("merge");
    core.update_config(|c| c.auto_open_agent_tabs = false);
    match core.task_reject(&env.project, &t.id, "x") {
        Err(TaskError::Conflict(m)) => assert!(m.contains("Open agent tabs"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(core.task(&env.project, &t.id).map(|t| t.status), Some(TaskStatus::Validating));
    core.shutdown();
}

#[test]
fn a_task_awaiting_validation_can_be_discarded_and_the_merge_stays() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = task_in_review(&env, "Discard later", "feature.txt");
    core.task_merge(&env.project, &t.id).expect("merge");
    events(&env);
    let gone = core.task_discard(&env.project, &t.id).expect("discard");
    assert_eq!((gone.status, gone.worktree.exists()), (TaskStatus::Discarded, false));
    assert_eq!(branches(&env.ws), ["main"]);
    assert_eq!(worktree_count(&env.ws), 1);
    assert!(gone.events.iter().any(|e| e.text.contains("what was already merged stays in main")), "{:?}", gone.events);
    assert!(events(&env).iter().any(|e| matches!(e, CoreEvent::CloseInstance { id, .. } if id == "claude@t1")));
    assert!(env.ws.join("feature.txt").is_file(), "discarding does not undo the merge");
    core.shutdown();
}

#[test]
fn a_task_awaiting_validation_survives_a_restart() {
    need_git!();
    let env = start();
    let t = task_in_review(&env, "Restart", "feature.txt");
    env.core.task_merge(&env.project, &t.id).expect("merge");
    let text = std::fs::read_to_string(env.root.join("dados").join("tarefas").join(format!("{}.json", env.project))).expect("json");
    assert!(text.contains("\"status\": \"validating\""), "{text}");
    let (root, project, config) = (env.root.clone(), env.project.clone(), env.config.clone());
    let Env { _tmp, core: first, rx, .. } = env;
    first.shutdown();
    drop((first, rx));
    let (core, _rx) = start_core(&root, config, true);
    let (env_project, back) = (project.as_str(), core.task(&project, "t1").expect("t1"));
    assert_eq!(back.status, TaskStatus::Validating);
    // The agent is still the task's and the terminal opens in the worktree.
    assert_eq!(core.task_of_instance(env_project, "claude@t1").map(|t| t.id), Some("t1".to_owned()));
    assert!(same_path(&core.spawn_spec_for(env_project, &Ident::parse("claude@t1").expect("id")).expect("spec").cwd, &t.worktree));
    assert_eq!(core.task_validate(env_project, "t1").expect("validate").status, TaskStatus::Merged);
    core.shutdown();
    drop(_tmp);
}

#[test]
fn the_timeline_of_a_task_survives_a_restart() {
    need_git!();
    let env = start();
    let t = task_in_review(&env, "Timeline", "feature.txt");
    let ask = |kind, from: &str, to: &str, text: &str| {
        env.core.feed(FeedEntry::new(kind, Some(from), Some(to), text).in_project(&env.project));
    };
    ask(FeedKind::Ask, "claude@t1", "antigravity", "how do I do it?");
    ask(FeedKind::Answer, "antigravity", "claude@t1", "like this");
    ask(FeedKind::Ask, "claude", "antigravity", "from another tab");
    // In another project it does not count, even with the same instance id.
    env.core.feed(FeedEntry::new(FeedKind::Ask, Some("claude@t1"), Some("x"), "someone else's").in_project("pOther"));
    let texts = |task: agentdeck_core::Task| task.messages.into_iter().map(|m| m.text).collect::<Vec<_>>();
    assert_eq!(texts(env.core.task(&env.project, &t.id).expect("t1")), ["how do I do it?", "like this"]);
    let (root, project, config) = (env.root.clone(), env.project.clone(), env.config.clone());
    let Env { _tmp, core: first, rx, .. } = env;
    first.shutdown();
    drop((first, rx));
    let (core, _rx) = start_core(&root, config, true);
    assert_eq!(texts(core.task(&project, "t1").expect("t1")), ["how do I do it?", "like this"]);
    core.shutdown();
    drop(_tmp);
}

#[test]
fn a_failed_merge_keeps_the_agent_terminal_in_the_worktree_after_a_restart() {
    need_git!();
    let env = start();
    let t = task_in_review(&env, "Merge failed", "feature.txt");
    // Simulates the infrastructure error in the merge: the file records `Failed` with the report already delivered.
    let file = env.root.join("dados").join("tarefas").join(format!("{}.json", env.project));
    let (root, project, config) = (env.root.clone(), env.project.clone(), env.config.clone());
    let Env { _tmp, core: first, rx, .. } = env;
    first.shutdown();
    drop((first, rx));
    let text = std::fs::read_to_string(&file).expect("json");
    assert!(text.contains("\"status\": \"review\""), "{text}");
    std::fs::write(&file, text.replace("\"status\": \"review\"", "\"status\": \"failed\"")).expect("w");
    let (core, _rx) = start_core(&root, config, true);
    let failed = core.task(&project, "t1").expect("t1");
    assert_eq!((failed.status, failed.report.is_some()), (TaskStatus::Failed, true));
    let ident = Ident::parse("claude@t1").expect("id");
    assert!(
        same_path(&core.spawn_spec_for(&project, &ident).expect("spec").cwd, &t.worktree),
        "in the worktree, not in the main repository"
    );
    // A task left in Failed with a report (earlier version) is not a dead end: it can be returned to the agent.
    let back = core.task_return(&project, "t1", "Adjust this.").expect("return");
    assert_eq!(back.status, TaskStatus::Running);
    core.shutdown();
    drop(_tmp);
}

#[test]
fn a_merge_that_fails_for_an_external_reason_leaves_the_task_in_review() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = task_in_review(&env, "External merge", "feature.txt");
    // Git cannot take the main folder's index lock (a failure that is not a conflict).
    let lock = env.ws.join(".git").join("index.lock");
    std::fs::write(&lock, "").expect("w");
    match core.task_merge(&env.project, &t.id) {
        Err(TaskError::Failed(m)) => assert!(m.contains("the merge failed"), "{m}"),
        other => panic!("expected the merge to fail, got {other:?}"),
    }
    let now = core.task(&env.project, &t.id).expect("task");
    assert_eq!(now.status, TaskStatus::Review, "{now:?}");
    assert!(now.events.iter().any(|e| e.text.contains("error: the merge failed")), "{:?}", now.events);
    assert!(now.worktree.is_dir() && branches(&env.ws).contains(&now.branch));
    // It leaves the dead end from both sides: returning to the agent...
    let back = core.task_return(&env.project, &t.id, "Try again.").expect("return");
    assert_eq!(back.status, TaskStatus::Running);
    core.task_done(&env.project, &t.id, "Done again.").expect("done");
    // ...or merging again, after removing the obstacle ("Try again").
    std::fs::remove_file(&lock).expect("rm");
    assert_eq!(core.task_merge(&env.project, &t.id).expect("merge").status, TaskStatus::Validating);
    core.shutdown();
}

/// Inserts a line inside the app's block in AGENTS.md (what the app does when the instructions change).
fn rewrite_app_block(agents_md: &std::path::Path) -> String {
    let text = std::fs::read_to_string(agents_md).expect("AGENTS.md");
    assert!(text.contains(agentdeck_core::workspace::MARKER_START), "{text}");
    let new = text.replacen(
        agentdeck_core::workspace::MARKER_START,
        &format!("{}\nnew line from the app", agentdeck_core::workspace::MARKER_START),
        1,
    );
    std::fs::write(agents_md, &new).expect("w");
    new
}

#[test]
fn merge_works_when_only_the_app_block_of_agents_md_changed_and_the_branch_touches_it() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = core.task_create(&env.project, spec("Touches AGENTS")).expect("create");
    let t = core.task_start(&env.project, &t.id).expect("start");
    let theirs = std::fs::read_to_string(t.worktree.join("AGENTS.md")).expect("the worktree's AGENTS.md");
    commit_file(&t.worktree, "AGENTS.md", &format!("# Task rule\n\n{theirs}"), "docs: rule in AGENTS.md");
    core.task_done(&env.project, &t.id, "I touched AGENTS.md.").expect("done");
    // Meanwhile the app rewrote its block in the main folder, uncommitted.
    let agents_md = env.ws.join("AGENTS.md");
    rewrite_app_block(&agents_md);

    let merged = core.task_merge(&env.project, &t.id).expect("merge");
    assert_eq!(merged.status, TaskStatus::Validating);
    let after = std::fs::read_to_string(&agents_md).expect("AGENTS.md");
    assert!(after.starts_with("# Task rule"), "the branch's change came in: {after}");
    assert!(after.contains("new line from the app"), "the app's block remains: {after}");
    assert_eq!(after.matches(agentdeck_core::workspace::MARKER_START).count(), 1, "{after}");
    core.shutdown();
}

#[test]
fn merge_refuses_to_discard_user_edits_in_agents_md_that_the_branch_touches() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = core.task_create(&env.project, spec("Touches AGENTS")).expect("create");
    let t = core.task_start(&env.project, &t.id).expect("start");
    let theirs = std::fs::read_to_string(t.worktree.join("AGENTS.md")).expect("the worktree's AGENTS.md");
    commit_file(&t.worktree, "AGENTS.md", &format!("# Task rule\n\n{theirs}"), "docs: rule in AGENTS.md");
    core.task_done(&env.project, &t.id, "I touched AGENTS.md.").expect("done");
    // The user edited the text outside the app's block.
    let agents_md = env.ws.join("AGENTS.md");
    let mine = format!("# My rule\n\n{}", rewrite_app_block(&agents_md));
    std::fs::write(&agents_md, &mine).expect("w");

    match core.task_merge(&env.project, &t.id) {
        Err(TaskError::Conflict(m)) => assert!(m.contains("AGENTS.md") && m.contains("uncommitted"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(std::fs::read_to_string(&agents_md).expect("AGENTS.md"), mine, "nothing was discarded");
    assert_eq!(core.task(&env.project, &t.id).expect("task").status, TaskStatus::Review);
    core.shutdown();
}

#[test]
fn merge_is_refused_until_the_main_worktree_is_ready() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = task_in_review(&env, "Refusals", "feature.txt");
    let still_in_review = || {
        let now = core.task(&env.project, &t.id).expect("task");
        assert_eq!(now.status, TaskStatus::Review, "{now:?}");
        assert!(now.worktree.is_dir(), "nothing was removed");
    };
    let refused = |expect: &str| match core.task_merge(&env.project, &t.id) {
        Err(TaskError::Conflict(m)) => assert!(m.contains(expect), "{m}"),
        other => panic!("expected a refusal with \"{expect}\", got {other:?}"),
    };

    // The main repository on another branch.
    git(&env.ws, &["checkout", "-q", "-b", "other"]);
    refused("is on the branch \"other\", not \"main\"");
    still_in_review();
    git(&env.ws, &["checkout", "-q", "main"]);

    // Uncommitted change in a tracked file.
    std::fs::write(env.ws.join("a.txt"), "I changed it\n").expect("w");
    refused("uncommitted changes (a.txt)");
    still_in_review();
    git(&env.ws, &["checkout", "-q", "--", "a.txt"]);

    // Uncommitted work in the task's worktree.
    std::fs::write(t.worktree.join("forgot.txt"), "x").expect("w");
    refused("uncommitted work (forgot.txt)");
    still_in_review();
    std::fs::remove_file(t.worktree.join("forgot.txt")).expect("rm");

    // All ready: now it goes (the refusal left no trace).
    assert!(t.events.len() < core.task(&env.project, &t.id).expect("task").events.len(), "the refusals stay in the history");
    assert_eq!(core.task_merge(&env.project, &t.id).expect("merge").status, TaskStatus::Validating);
    core.shutdown();
}

#[test]
fn merge_needs_new_commits() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = core.task_create(&env.project, spec("Nothing")).expect("create");
    let t = core.task_start(&env.project, &t.id).expect("start");
    let done = core.task_done(&env.project, &t.id, "I did not need to change anything.").expect("done");
    assert!(done.diff_stat.as_deref().is_some_and(|s| s.contains("no changes")), "{:?}", done.diff_stat);
    match core.task_merge(&env.project, &t.id) {
        Err(TaskError::Conflict(m)) => assert!(m.contains("has no new commits"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert!(core.task_discard(&env.project, &t.id).is_ok());
    core.shutdown();
}

#[test]
fn a_conflicting_merge_is_aborted_and_the_task_stays_in_review() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = core.task_create(&env.project, spec("Conflict")).expect("create");
    let t = core.task_start(&env.project, &t.id).expect("start");
    commit_file(&t.worktree, "a.txt", "task version\ntwo\n", "feat: change a.txt in the task");
    core.task_done(&env.project, &t.id, "I changed a.txt.").expect("done");
    // Meanwhile the base changed the same line.
    commit_file(&env.ws, "a.txt", "base version\ntwo\n", "fix: change a.txt in the base");
    let head = git(&env.ws, &["rev-parse", "HEAD"]);

    match core.task_merge(&env.project, &t.id) {
        Err(TaskError::Conflict(m)) => assert!(m.contains("conflict") && m.contains("a.txt") && m.contains("undone"), "{m}"),
        other => panic!("{other:?}"),
    }
    // The merge was undone: no MERGE_HEAD, no changes, same HEAD, the base's file.
    assert!(!env.ws.join(".git").join("MERGE_HEAD").exists());
    assert_eq!(git(&env.ws, &["status", "--porcelain", "--untracked-files=no"]).trim(), "");
    assert_eq!(git(&env.ws, &["rev-parse", "HEAD"]), head);
    assert_eq!(std::fs::read_to_string(env.ws.join("a.txt")).expect("a.txt"), "base version\ntwo\n");
    let now = core.task(&env.project, &t.id).expect("task");
    assert_eq!(now.status, TaskStatus::Review);
    assert!(now.worktree.is_dir() && branches(&env.ws).contains(&now.branch));
    assert!(now.events.iter().any(|e| e.text.contains("conflict in the merge")), "{:?}", now.events);

    // Returned, the task brings the base into the branch (resolved by hand here) and now it merges.
    core.task_return(&env.project, &t.id, "Update with main and resolve the conflict.").expect("return");
    let out = Command::new(git_exe().expect("git")).current_dir(&t.worktree).args(["merge", "main"]).output().expect("git");
    assert!(!out.status.success(), "the conflict happens on the task's branch");
    std::fs::write(t.worktree.join("a.txt"), "resolved\ntwo\n").expect("w");
    git(&t.worktree, &["add", "-A"]);
    git(&t.worktree, &["commit", "-q", "-m", "merge: resolve"]);
    core.task_done(&env.project, &t.id, "Resolved.").expect("done 2");
    assert_eq!(core.task_merge(&env.project, &t.id).expect("merge").status, TaskStatus::Validating);
    assert_eq!(std::fs::read_to_string(env.ws.join("a.txt")).expect("a.txt"), "resolved\ntwo\n");
    core.shutdown();
}

fn stash_list(dir: &std::path::Path) -> Vec<String> {
    git(dir, &["stash", "list"]).lines().map(str::to_owned).collect()
}

#[test]
fn a_refused_merge_names_the_way_out_and_the_command_line_never_stashes() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = task_in_review(&env, "Ways out", "feature.txt");

    // 1. The user's changes in the main repository: the refusal brings the reason, and the plain merge stashes nothing.
    std::fs::write(env.ws.join("a.txt"), "I changed it\n").expect("w");
    let refusal = core.task_merge_assisted(&env.project, &t.id, false).expect_err("refusal");
    assert!(matches!(&refusal.error, TaskError::Conflict(m) if m.contains("uncommitted changes (a.txt)")), "{refusal:?}");
    assert_eq!(refusal.block, Some(MergeBlock::MainDirty(vec!["a.txt".into()])));
    let block = refusal.block.expect("reason");
    assert!(block.explain(&t).contains("set them aside") && block.return_feedback(&t).is_none(), "the agent does not resolve this");
    assert!(matches!(core.task_merge(&env.project, &t.id), Err(TaskError::Conflict(_))));
    assert!(stash_list(&env.ws).is_empty(), "the command line never stashes anything");
    assert_eq!(std::fs::read_to_string(env.ws.join("a.txt")).expect("a.txt"), "I changed it\n");
    git(&env.ws, &["checkout", "-q", "--", "a.txt"]);

    // 2. Uncommitted work in the task's folder: the ready-made request cites the files and the folder, and returns the task.
    std::fs::write(t.worktree.join("forgot.txt"), "x").expect("w");
    let refusal = core.task_merge_assisted(&env.project, &t.id, true).expect_err("refusal");
    let block = refusal.block.expect("reason");
    assert_eq!(block, MergeBlock::TaskDirty(vec!["forgot.txt".into()]));
    let ask = block.return_feedback(&t).expect("request");
    assert!(ask.contains("- forgot.txt") && ask.contains(&t.worktree.display().to_string()) && ask.contains("git commit"), "{ask}");
    assert_eq!(core.task_return(&env.project, &t.id, &ask).expect("return").status, TaskStatus::Running);
    git(&t.worktree, &["add", "-A"]);
    git(&t.worktree, &["commit", "-q", "-m", "feat: forgot.txt"]);
    core.task_done(&env.project, &t.id, "Committed.").expect("done");
    assert_eq!(core.task_merge(&env.project, &t.id).expect("merge").status, TaskStatus::Validating);
    core.shutdown();
}

#[test]
fn a_conflict_refusal_lists_the_files_and_asks_the_agent_to_merge_the_base() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = core.task_create(&env.project, spec("Conflict")).expect("create");
    let t = core.task_start(&env.project, &t.id).expect("start");
    commit_file(&t.worktree, "a.txt", "task version\ntwo\n", "feat: change a.txt in the task");
    core.task_done(&env.project, &t.id, "I changed a.txt.").expect("done");
    commit_file(&env.ws, "a.txt", "base version\ntwo\n", "fix: change a.txt in the base");
    // The user's change in another file: the "stash" comes back even when the merge is undone.
    commit_file(&env.ws, "b.txt", "b\n", "docs: b");
    std::fs::write(env.ws.join("b.txt"), "user's b\n").expect("w");

    let refusal = core.task_merge_assisted(&env.project, &t.id, true).expect_err("conflict");
    let block = refusal.block.clone().expect("reason");
    assert_eq!(block, MergeBlock::Conflict(vec!["a.txt".into()]), "{refusal:?}");
    assert!(matches!(&refusal.error, TaskError::Conflict(m) if m.contains("conflict") && m.contains("already back")), "{refusal:?}");
    let ask = block.return_feedback(&t).expect("request");
    assert!(ask.contains("- a.txt") && ask.contains("git merge main") && ask.contains(&t.worktree.display().to_string()), "{ask}");
    assert!(block.explain(&t).contains("\"main\"") && block.explain(&t).contains("a.txt"));
    // Nothing was left on the way: no merge in progress, no stash, the user's change in place.
    assert!(!env.ws.join(".git").join("MERGE_HEAD").exists() && stash_list(&env.ws).is_empty());
    assert_eq!(std::fs::read_to_string(env.ws.join("b.txt")).expect("b.txt"), "user's b\n");
    assert_eq!(core.task(&env.project, &t.id).expect("task").status, TaskStatus::Review);

    // Returned with that request, the agent merges the base into its branch and the merge goes through.
    core.task_return(&env.project, &t.id, &ask).expect("return");
    let out = Command::new(git_exe().expect("git")).current_dir(&t.worktree).args(["merge", "main"]).output().expect("git");
    assert!(!out.status.success(), "the conflict happens on the task's branch");
    commit_file(&t.worktree, "a.txt", "resolved\ntwo\n", "merge: resolve");
    core.task_done(&env.project, &t.id, "Resolved.").expect("done 2");
    let merged = core.task_merge_assisted(&env.project, &t.id, true).expect("merge");
    assert_eq!(merged.task.status, TaskStatus::Validating);
    assert_eq!(std::fs::read_to_string(env.ws.join("b.txt")).expect("b.txt"), "user's b\n");
    core.shutdown();
}

#[test]
fn stash_and_merge_gives_the_changes_back_cleanly() {
    need_git!();
    let env = start();
    let core = &env.core;
    // An old entry on the stash stack (from another worktree, for example) must not be touched.
    commit_file(&env.ws, "old.txt", "v\n", "docs: old");
    std::fs::write(env.ws.join("old.txt"), "stashed by someone else\n").expect("w");
    git(&env.ws, &["stash", "push", "-q", "-m", "from another worktree"]);
    let t = task_in_review(&env, "Stash and merge", "feature.txt");

    // The user has: a changed file, a new one already in the index and a new untracked one (which stays where it is).
    std::fs::write(env.ws.join("a.txt"), "one\ntwo\nthree by the user\n").expect("w");
    std::fs::write(env.ws.join("staged.txt"), "new in the index\n").expect("w");
    git(&env.ws, &["add", "staged.txt"]);
    std::fs::write(env.ws.join("scratch.txt"), "loose\n").expect("w");

    let out = core.task_merge_assisted(&env.project, &t.id, true).expect("stash and merge");
    assert_eq!(out.task.status, TaskStatus::Validating);
    let note = out.note.expect("notice");
    assert!(note.contains("already back"), "{note}");
    // The merge brought the task's file; each of the user's things is back; only the old entry remains in the stash.
    assert!(env.ws.join("feature.txt").is_file());
    assert_eq!(std::fs::read_to_string(env.ws.join("a.txt")).expect("a.txt"), "one\ntwo\nthree by the user\n");
    assert_eq!(std::fs::read_to_string(env.ws.join("staged.txt")).expect("staged.txt"), "new in the index\n");
    assert_eq!(std::fs::read_to_string(env.ws.join("scratch.txt")).expect("scratch.txt"), "loose\n");
    let list = stash_list(&env.ws);
    assert_eq!(list.len(), 1, "{list:?}");
    assert!(list[0].contains("from another worktree"), "{list:?}");
    assert!(core.task(&env.project, &t.id).expect("task").events.iter().any(|e| e.text.contains("given back")));
    core.shutdown();
}

#[test]
fn stash_and_merge_keeps_the_changes_in_the_stash_when_they_clash_with_the_task() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = core.task_create(&env.project, spec("Same line")).expect("create");
    let t = core.task_start(&env.project, &t.id).expect("start");
    commit_file(&t.worktree, "a.txt", "from the task\ntwo\n", "feat: change a.txt in the task");
    core.task_done(&env.project, &t.id, "I changed a.txt.").expect("done");
    // The user changed, uncommitted, the same line.
    std::fs::write(env.ws.join("a.txt"), "from the user\ntwo\n").expect("w");

    let out = core.task_merge_assisted(&env.project, &t.id, true).expect("the merge goes through");
    assert_eq!(out.task.status, TaskStatus::Validating);
    let note = out.note.expect("notice");
    assert!(note.contains("still kept") && note.contains("a.txt") && note.contains("git stash pop"), "{note}");
    // The folder is clean, with the task's version (no conflict marks), and the user's changes intact in the stash.
    assert_eq!(std::fs::read_to_string(env.ws.join("a.txt")).expect("a.txt"), "from the task\ntwo\n");
    assert_eq!(git(&env.ws, &["status", "--porcelain", "--untracked-files=no"]).trim(), "");
    let list = stash_list(&env.ws);
    assert_eq!(list.len(), 1, "{list:?}");
    assert!(list[0].contains("agentdeck: stashed before merging task t1"), "{list:?}");
    assert!(git(&env.ws, &["stash", "show", "-p", "stash@{0}"]).contains("+from the user"));
    assert!(core.task(&env.project, &t.id).expect("task").events.iter().any(|e| e.text.contains("still in the stash")));
    // Recovering by hand, as the notice teaches, brings the conflict to the user to resolve (the stash was not lost).
    let pop = Command::new(git_exe().expect("git")).current_dir(&env.ws).args(["stash", "pop"]).output().expect("git");
    assert!(!pop.status.success(), "the conflict is real");
    core.shutdown();
}

#[test]
fn stash_and_merge_refuses_before_touching_anything_when_a_new_file_of_the_user_clashes() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = task_in_review(&env, "New file", "feature.txt");
    // A new, untracked user file with the name the task also creates; and a change to stash.
    std::fs::write(env.ws.join("feature.txt"), "mine\n").expect("w");
    std::fs::write(env.ws.join("a.txt"), "I changed it\n").expect("w");
    let refusal = core.task_merge_assisted(&env.project, &t.id, true).expect_err("refusal");
    assert!(
        matches!(&refusal.error, TaskError::Conflict(m) if m.contains("new files of yours") && m.contains("feature.txt")),
        "{refusal:?}"
    );
    assert_eq!(refusal.block, None, "no button: the file is the user's and the stash does not take it");
    assert!(stash_list(&env.ws).is_empty(), "nothing was stashed");
    assert_eq!(std::fs::read_to_string(env.ws.join("a.txt")).expect("a.txt"), "I changed it\n");
    assert_eq!(std::fs::read_to_string(env.ws.join("feature.txt")).expect("feature.txt"), "mine\n");
    // Without the tracked changes the refusal is the same (the check applies with or without stashing).
    git(&env.ws, &["checkout", "-q", "--", "a.txt"]);
    let refusal = core.task_merge_assisted(&env.project, &t.id, false).expect_err("refusal");
    assert!(matches!(&refusal.error, TaskError::Conflict(m) if m.contains("new files of yours")), "{refusal:?}");
    // With the obstacle removed, stash and merge goes through.
    std::fs::write(env.ws.join("a.txt"), "I changed it\n").expect("w");
    std::fs::remove_file(env.ws.join("feature.txt")).expect("rm");
    let out = core.task_merge_assisted(&env.project, &t.id, true).expect("merge");
    assert_eq!(out.task.status, TaskStatus::Validating);
    assert_eq!(std::fs::read_to_string(env.ws.join("a.txt")).expect("a.txt"), "I changed it\n");
    core.shutdown();
}

#[test]
fn stash_and_merge_does_not_stash_when_the_task_dir_still_has_work_to_commit() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = task_in_review(&env, "Dirty folder", "feature.txt");
    std::fs::write(env.ws.join("a.txt"), "I changed it\n").expect("w");
    std::fs::write(t.worktree.join("forgot.txt"), "x").expect("w");
    let refusal = core.task_merge_assisted(&env.project, &t.id, true).expect_err("refusal");
    assert!(matches!(refusal.block, Some(MergeBlock::TaskDirty(_))), "{refusal:?}");
    assert!(stash_list(&env.ws).is_empty() && std::fs::read_to_string(env.ws.join("a.txt")).expect("a.txt") == "I changed it\n");
    core.shutdown();
}

#[test]
fn discard_removes_everything_even_without_the_folder() {
    need_git!();
    let env = start();
    let core = &env.core;

    // Draft: only the record.
    let draft = core.task_create(&env.project, spec("Draft")).expect("create");
    events(&env);
    assert_eq!(core.task_discard(&env.project, &draft.id).expect("discard").status, TaskStatus::Discarded);
    assert!(!events(&env).iter().any(|e| matches!(e, CoreEvent::CloseInstance { .. })), "a draft has no tab");

    // In progress, with uncommitted work: everything goes away.
    let t = core.task_create(&env.project, spec("In progress")).expect("create");
    let t = core.task_start(&env.project, &t.id).expect("start");
    std::fs::write(t.worktree.join("junk.txt"), "x").expect("w");
    events(&env);
    let gone = core.task_discard(&env.project, &t.id).expect("discard");
    assert_eq!((gone.status, gone.worktree.exists()), (TaskStatus::Discarded, false));
    assert_eq!(branches(&env.ws), ["main"]);
    assert_eq!(worktree_count(&env.ws), 1);
    assert!(events(&env).iter().any(|e| matches!(e, CoreEvent::CloseInstance { id, .. } if id == "claude@t2")));
    assert!(!core.project_instance_ids(&env.project).contains(&"claude@t2".to_owned()));
    assert!(matches!(core.task_discard(&env.project, &t.id), Err(TaskError::Conflict(_))), "already discarded");

    // In review, with the folder deleted by hand: discards the same way and git keeps no stale record.
    let review = task_in_review(&env, "Deleted folder", "x.txt");
    std::fs::remove_dir_all(&review.worktree).expect("rm");
    assert_eq!(worktree_count(&env.ws), 2, "git still remembers the worktree");
    let gone = core.task_discard(&env.project, &review.id).expect("discard without the folder");
    assert_eq!(gone.status, TaskStatus::Discarded);
    assert_eq!(branches(&env.ws), ["main"]);
    assert_eq!(worktree_count(&env.ws), 1);

    // With the folder AND the branch already deleted from outside.
    let t = core.task_create(&env.project, spec("All from outside")).expect("create");
    let t = core.task_start(&env.project, &t.id).expect("start");
    git(&env.ws, &["worktree", "remove", "--force", &t.worktree.to_string_lossy()]);
    git(&env.ws, &["branch", "-D", &t.branch]);
    assert_eq!(core.task_discard(&env.project, &t.id).expect("discard").status, TaskStatus::Discarded);
    core.shutdown();
}

#[test]
fn the_terminal_of_a_missing_worktree_says_so() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = core.task_create(&env.project, spec("Folder vanishes")).expect("create");
    let t = core.task_start(&env.project, &t.id).expect("start");
    std::fs::remove_dir_all(&t.worktree).expect("rm");
    let ident = Ident::parse("claude@t1").expect("id");
    match core.spawn_spec_for(&env.project, &ident) {
        // The message belongs to `agents.rs` (another area): it stays as that area has it.
        Err(m) => assert!(m.contains("A pasta da tarefa não existe mais"), "{m}"),
        Ok(s) => panic!("opened in {}", s.cwd.display()),
    }
    core.shutdown();
}

#[test]
fn tasks_survive_a_restart_and_numbers_are_never_reused() {
    need_git!();
    let env = start();
    let core = env.core.clone();
    let t1 = core.task_create(&env.project, spec("First")).expect("create");
    let t2 = core.task_create(&env.project, spec("Second")).expect("create");
    core.task_start(&env.project, &t1.id).expect("start");
    core.task_discard(&env.project, &t2.id).expect("discard");
    let before = core.tasks(&env.project);
    let (root, project, config, ws) = (env.root.clone(), env.project.clone(), env.config.clone(), env.ws.clone());

    // The file stays in the data folder, not in the user's repository.
    let file = root.join("dados").join("tarefas").join(format!("{project}.json"));
    let text = std::fs::read_to_string(&file).expect("tasks file");
    assert!(text.contains("\"id\": \"t1\"") && text.contains("\"status\": \"discarded\"") && text.contains("\"next\": 3"), "{text}");
    assert!(!ws.join("tarefas").exists());
    let names: Vec<String> = std::fs::read_dir(file.parent().expect("folder"))
        .expect("dir")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        names.iter().all(|n| !n.ends_with(".tmp")) && names.iter().any(|n| n == &format!("{project}.json")),
        "no temporary files left over: {names:?}"
    );

    // Restarts the app (same data folder and same configuration).
    let Env { _tmp, core: first, rx, .. } = env;
    first.shutdown();
    drop((first, core, rx));
    let (core2, _rx2) = start_core(&root, config, true);
    assert_eq!(core2.tasks(&project), before, "the tasks come back as they were");
    let t3 = core2.task_create(&project, spec("Third")).expect("create");
    assert_eq!(t3.id, "t3", "the discarded task's number is not reused");
    // And the task in progress keeps its terminal in its worktree.
    let ident = Ident::parse("claude@t1").expect("id");
    assert!(same_path(&core2.spawn_spec_for(&project, &ident).expect("spec").cwd, &before[0].worktree));
    core2.shutdown();
    drop(_tmp);
}

#[test]
fn removing_a_project_keeps_its_tasks_file() {
    need_git!();
    let env = start();
    let t = env.core.task_create(&env.project, spec("Stays")).expect("create");
    let file = env.root.join("dados").join("tarefas").join(format!("{}.json", env.project));
    assert!(file.is_file());
    assert!(env.core.remove_project(&env.project));
    assert!(file.is_file(), "the tasks file does not vanish with the project");
    assert!(std::fs::read_to_string(&file).expect("read").contains(&t.id));
    env.core.shutdown();
}

/// Adding the same folder again reuses the id: the project's desk comes back with the old tasks.
#[test]
fn reading_the_project_again_brings_its_tasks_back() {
    need_git!();
    let env = start();
    let t = env.core.task_create(&env.project, spec("Comes back")).expect("create");
    assert!(env.core.remove_project(&env.project));
    let again = env.core.create_project(&env.ws, None);
    assert_eq!(again.id, env.project, "adding the same folder again is the same project");
    assert_eq!(env.core.active_project_id(), again.id, "with no active project, the re-added one takes over");
    assert_eq!(env.core.tasks(&again.id).iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), [t.id.as_str()]);
    env.core.shutdown();
}

/// The latest task `Deliver` (request 0): `(instance, prompt, boot)`.
fn last_task_delivery(evs: &[CoreEvent]) -> (String, String, Option<String>) {
    evs.iter()
        .rev()
        .find_map(|e| match e {
            CoreEvent::Deliver(d) if d.request == 0 => Some((d.ident.id(), d.prompt.clone(), d.boot.clone())),
            _ => None,
        })
        .expect("no task delivery")
}

#[test]
fn the_brief_goes_to_a_file_and_the_boot_line_is_short_ascii() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = core.task_create(&env.project, spec("Fix the café button")).expect("create");
    events(&env);
    let t = core.task_start(&env.project, &t.id).expect("start");
    let (to, prompt, boot) = last_task_delivery(&events(&env));
    assert_eq!(to, "claude@t1");
    // The full text (with accents and line breaks) stays in a file in the data folder...
    let file = env.root.join("dados").join("tarefas").join(format!("{}-t1-brief.md", env.project));
    assert_eq!(std::fs::read_to_string(&file).expect("brief written"), prompt);
    assert!(prompt.contains("Fix the café button") && prompt.contains('\n'));
    // ...and the command line carries only a short, ASCII, single-line sentence that points to it.
    let line = boot.expect("claude accepts an initial prompt");
    assert!(line.is_ascii() && !line.contains(['\n', '"']) && line.len() < 400, "{line}");
    assert!(line.starts_with("Task t1: ") && line.contains("start now"), "{line}");
    assert!(line.contains(&file.to_string_lossy().into_owned()), "{line} / {}", file.display());
    // Returning and rejecting too: the file is the feedback's (overwritten every time).
    core.task_done(&env.project, &t.id, "Done.").expect("done");
    core.task_return(&env.project, &t.id, "The açaí test is missing.").expect("return");
    let (_, prompt, boot) = last_task_delivery(&events(&env));
    let feedback = env.root.join("dados").join("tarefas").join(format!("{}-t1-feedback.md", env.project));
    assert_eq!(std::fs::read_to_string(&feedback).expect("feedback written"), prompt);
    assert!(prompt.contains("The açaí test is missing."));
    let line = boot.expect("boot of the return");
    assert!(line.is_ascii() && line.contains("returned") && line.contains("-t1-feedback.md"), "{line}");
    commit_file(&t.worktree, "x.txt", "x\n", "feat: x");
    core.task_done(&env.project, &t.id, "Again.").expect("done 2");
    core.task_merge(&env.project, &t.id).expect("merge");
    core.task_reject(&env.project, &t.id, "It broke on save.").expect("reject");
    let (_, prompt, boot) = last_task_delivery(&events(&env));
    assert_eq!(std::fs::read_to_string(&feedback).expect("feedback rewritten"), prompt);
    assert!(prompt.contains("rejected in validation") && prompt.contains("It broke on save."));
    assert!(boot.is_some_and(|l| l.is_ascii() && l.contains("failed validation")));
    core.shutdown();
}

#[test]
fn a_cli_without_an_initial_prompt_keeps_the_typed_delivery() {
    need_git!();
    let env = start();
    let t = env.core.task_create(&env.project, TaskSpec { kind: "aider".into(), ..spec("With aider") }).expect("create");
    events(&env);
    env.core.task_start(&env.project, &t.id).expect("start");
    let (to, prompt, boot) = last_task_delivery(&events(&env));
    assert_eq!(to, "aider@t1");
    assert_eq!(boot, None, "aider has no initial prompt: the brief is typed");
    assert!(prompt.contains("Do: With aider"));
    assert!(!env.root.join("dados").join("tarefas").join(format!("{}-t1-brief.md", env.project)).exists());
    env.core.shutdown();
}

#[test]
fn the_effort_of_a_task_is_validated_stored_and_reaches_the_claude_tab() {
    need_git!();
    let env = start();
    let core = &env.core;
    install_fake_agent(&env.root, "agy");
    install_fake_agent(&env.root, "opencode");
    let effort = |e| TaskSpec { effort: Some(e), ..spec("With effort") };

    // Claude: stores the effort and opens the tab with `--effort`; without effort, nothing goes to the CLI.
    let t = core.task_create(&env.project, TaskSpec { model: Some("sonnet".into()), ..effort(Effort::High) }).expect("create");
    assert_eq!(t.effort, Some(Effort::High));
    assert_eq!(core.task(&env.project, &t.id).expect("task").effort, Some(Effort::High));
    let ident = Ident::parse(&t.instance).expect("id");
    let draft = core.spawn_spec_for(&env.project, &ident).expect("spec");
    assert!(!draft.args.iter().any(|a| a == "--effort"), "a draft is not yet the tab's task: {:?}", draft.args);
    core.task_start(&env.project, &t.id).expect("start");
    let args = core.spawn_spec_for(&env.project, &ident).expect("spec").args;
    assert!(args.windows(2).any(|w| w == ["--effort", "high"]), "{args:?}");
    assert!(args.windows(2).any(|w| w == ["--model", "sonnet"]), "{args:?}");
    let plain = core.task_create(&env.project, spec("Without effort")).expect("create");
    assert_eq!(plain.effort, None);
    core.task_start(&env.project, &plain.id).expect("start");
    let args = core.spawn_spec_for(&env.project, &Ident::parse(&plain.instance).expect("id")).expect("spec").args;
    assert!(!args.iter().any(|a| a == "--effort"), "{args:?}");

    // The bridge's JSON carries the effort; a task saved before it keeps loading.
    assert_eq!(t.summary_json()["effort"], "high");
    let old: agentdeck_core::Task = serde_json::from_str(r#"{"id":"t9","title":"old"}"#).expect("old task");
    assert_eq!(old.effort, None);

    // Antigravity: the effort swaps the model suffix, which is what gets saved; without a model (or with a
    // model without per-effort versions) the request is refused, instead of being saved with no effect.
    let agy = |model: Option<&str>| TaskSpec { kind: "antigravity".into(), model: model.map(String::from), ..effort(Effort::Low) };
    let t = core.task_create(&env.project, agy(Some("gemini-4-argon-high"))).expect("agy");
    assert_eq!((t.model.as_deref(), t.effort), (Some("gemini-4-argon-low"), Some(Effort::Low)));
    for bad in [agy(None), agy(Some("gemini-4-flash"))] {
        let e = core.task_create(&env.project, bad).expect_err("no per-effort versions");
        assert!(matches!(&e, TaskError::Invalid(m) if m.contains("effort not applicable to antigravity")), "{e:?}");
    }

    // OpenCode: the tab does not receive the effort, so the request is refused (with the hint).
    let e = core
        .task_create(&env.project, TaskSpec { kind: "opencode".into(), model: Some("opencode/big-pickle".into()), ..effort(Effort::High) })
        .expect_err("opencode");
    assert!(matches!(&e, TaskError::Invalid(m) if m.contains("headless")), "{e:?}");
    // Whoever lacks the option too.
    let e = core.task_create(&env.project, TaskSpec { kind: "aider".into(), ..effort(Effort::High) }).expect_err("aider");
    assert!(matches!(&e, TaskError::Invalid(m) if m.contains("effort not applicable to aider")), "{e:?}");
    core.shutdown();
}

#[test]
fn the_spawn_spec_carries_the_boot_prompt_once_per_cli() {
    need_git!();
    let env = start();
    let core = &env.core;
    install_fake_agent(&env.root, "agy");
    install_fake_agent(&env.root, "opencode");
    install_fake_agent(&env.root, "codex");
    let line = "Task t7: the full request is in the file C:/Users/Ana Bia/dados/tarefas/p1-t7-brief.md. Read the file and start now.";
    let spec_of = |id: &str, boot: bool| {
        let ident = Ident::parse(id).expect("id");
        core.register_instance(&env.project, id);
        if boot {
            core.set_boot_prompt(&env.project, id, line.to_owned());
        }
        core.spawn_spec_for(&env.project, &ident).expect("spec")
    };

    // Claude: positional, BEFORE the options (`--add-dir` and `--allowedTools` accept several values and would swallow it).
    let claude = spec_of("claude@t7", true);
    assert_eq!(claude.args[0], line, "{:?}", claude.args);
    assert_eq!(claude.args[1], "--append-system-prompt");
    assert!(claude.args.iter().filter(|a| a.as_str() == line).count() == 1);
    // Antigravity: `-i "<prompt>"` (--prompt-interactive), at the end.
    let agy = spec_of("antigravity@t7", true);
    assert_eq!(agy.args[agy.args.len() - 2..], ["-i".to_owned(), line.to_owned()], "{:?}", agy.args);
    // OpenCode (TUI): `--prompt "<prompt>"` at the end (with a model, the `--standalone` comes before).
    let opencode = spec_of("opencode@t7", true);
    assert_eq!(opencode.args[opencode.args.len() - 2..], ["--prompt".to_owned(), line.to_owned()], "{:?}", opencode.args);
    // The others ignore it: even with a stored prompt, the spec does not change and the prompt does not leak into another opening.
    let codex = spec_of("codex@t7", true);
    assert!(!codex.args.iter().any(|a| a.contains("Task t7")), "{:?}", codex.args);
    let aider = spec_of("aider@t7", true);
    assert!(!aider.args.iter().any(|a| a.contains("Task t7")), "{:?}", aider.args);

    // Only once: the next opening (restarting the tab, reopening the app) comes up without the prompt.
    for id in ["claude@t7", "antigravity@t7", "opencode@t7"] {
        let again = spec_of(id, false);
        assert!(!again.args.iter().any(|a| a == line || a == "-i" || a == "--prompt"), "{id}: {:?}", again.args);
    }
    // Without a stored prompt, Claude's spec starts as always.
    assert_eq!(spec_of("claude@t8", false).args[0], "--append-system-prompt");
    assert_eq!(core.take_boot_prompt(&env.project, "claude@t7"), None);
    core.shutdown();
}

#[test]
fn a_boot_line_is_refused_when_the_path_is_not_plain_ascii() {
    use agentdeck_core::tasks::boot_line;
    let t = agentdeck_core::Task { id: "t3".into(), ..agentdeck_core::Task::default() };
    let ok = boot_line(&t, "the request is in the file", std::path::Path::new("C:/dados/tarefas/p1-t3-brief.md")).expect("ascii");
    assert!(
        ok.starts_with("Task t3: the request is in the file. Read the file C:/dados") && ok.ends_with("without waiting for confirmation."),
        "{ok}"
    );
    // A path with an accent (or quotes): the command line of a `.cmd` shim would not carry it whole; falls back to the typed delivery.
    assert_eq!(boot_line(&t, "x", std::path::Path::new("C:/Users/José/brief.md")), None);
    assert_eq!(boot_line(&t, "x", std::path::Path::new("C:/a\"b/brief.md")), None);
}

// ---------- After the merge: merge_commit, diff, done and validate with no dead ends ----------

fn diff_adds(diff: &agentdeck_core::git::GitDiff) -> Vec<String> {
    use agentdeck_core::git::DiffKind;
    diff.lines.iter().filter(|l| l.kind == DiffKind::Add).map(|l| l.text.clone()).collect()
}

#[test]
fn merge_stores_the_full_merge_commit_and_a_new_merge_overwrites_it() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = task_in_review(&env, "Keep the commit", "feature.txt");
    assert_eq!(core.task(&env.project, &t.id).and_then(|t| t.merge_commit), None, "before merging there is no commit");
    let merged = core.task_merge(&env.project, &t.id).expect("merge");
    let head = git(&env.ws, &["rev-parse", "HEAD"]).trim().to_owned();
    assert_eq!(head.len(), 40);
    assert_eq!(merged.merge_commit.as_deref(), Some(head.as_str()), "full hash of the merge commit");
    assert!(merged.events.iter().any(|e| e.text.contains(&format!("({})", &head[..7]))), "{:?}", merged.events);
    // Rejected and delivered with a new commit: the new merge overwrites it.
    core.task_reject(&env.project, &t.id, "A tweak is missing.").expect("reject");
    commit_file(&t.worktree, "tweak.txt", "ok\n", "fix: tweak");
    core.task_done(&env.project, &t.id, "Tweaked.").expect("done");
    let again = core.task_merge(&env.project, &t.id).expect("merge 2");
    let head2 = git(&env.ws, &["rev-parse", "HEAD"]).trim().to_owned();
    assert_ne!(head2, head);
    assert_eq!(again.merge_commit.as_deref(), Some(head2.as_str()));
    // It persists: what is read again is the same.
    assert_eq!(core.task(&env.project, &t.id).and_then(|t| t.merge_commit).as_deref(), Some(head2.as_str()));
    core.shutdown();
}

#[test]
fn a_task_saved_before_merge_commit_existed_still_loads() {
    let old = r#"{"id":"t4","title":"Old","status":"validating","base":"main","branch":"tarefa/t4-old"}"#;
    let t: agentdeck_core::Task = serde_json::from_str(old).expect("old task");
    assert_eq!((t.id.as_str(), t.status, t.merge_commit.as_deref()), ("t4", TaskStatus::Validating, None));
    // And the field goes back and forth through the JSON.
    let t = agentdeck_core::Task { merge_commit: Some("abc".into()), ..t };
    let back: agentdeck_core::Task = serde_json::from_str(&serde_json::to_string(&t).expect("json")).expect("back");
    assert_eq!(back.merge_commit.as_deref(), Some("abc"));
}

#[test]
fn the_diff_of_a_task_awaiting_validation_is_what_the_merge_brought() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = task_in_review(&env, "See the merged", "feature.txt");
    let before = core.task_diff(&env.project, &t.id).expect("diff in review");
    assert_eq!(diff_adds(&before), ["+content"]);
    // The base moves before the merge: the merge commit does not bring that, so the diff does not either.
    commit_file(&env.ws, "from_base.txt", "only in the base\n", "chore: base");
    core.task_merge(&env.project, &t.id).expect("merge");
    // After the alignment, `base...branch` is empty; the diff comes from the merge commit.
    let diff = core.task_diff(&env.project, &t.id).expect("diff in validation");
    assert_eq!(diff_adds(&diff), ["+content"], "{:?}", diff.lines);
    assert!(!diff.truncated && !diff.binary);
    let text: String = diff.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");
    assert!(text.contains("feature.txt") && !text.contains("from_base.txt"), "{text}");
    // Completed: branch and worktree vanish, the diff of what was merged remains.
    core.task_validate(&env.project, &t.id).expect("validate");
    let done = core.task_diff(&env.project, &t.id).expect("diff of the completed one");
    assert_eq!(diff_adds(&done), ["+content"]);
    // Discarded still has no diff (the branch vanishes and there is no merge commit).
    let other = core.task_create(&env.project, spec("Another")).expect("create");
    core.task_discard(&env.project, &other.id).expect("discard");
    assert!(matches!(core.task_diff(&env.project, &other.id), Err(TaskError::Conflict(_))));
    core.shutdown();
}

#[test]
fn reject_then_done_without_commits_goes_back_to_validating_without_a_new_merge() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = task_in_review(&env, "Nothing to change", "feature.txt");
    let merged = core.task_merge(&env.project, &t.id).expect("merge");
    let commit = merged.merge_commit.clone().expect("commit");
    core.task_reject(&env.project, &t.id, "I thought it broke, but maybe not.").expect("reject");
    let done = core.task_done(&env.project, &t.id, "Checked: there is nothing to change.").expect("done");
    assert_eq!(done.status, TaskStatus::Review);
    let head = git(&env.ws, &["rev-parse", "HEAD"]);
    // Without new commits, but already merged before: it does not refuse, it does not merge again.
    let again = core.task_merge(&env.project, &t.id).expect("merge without new commits");
    assert_eq!(again.status, TaskStatus::Validating);
    assert_eq!(again.merge_commit.as_deref(), Some(commit.as_str()), "the merge commit does not change");
    assert_eq!(git(&env.ws, &["rev-parse", "HEAD"]), head, "no new merge commit");
    assert!(
        again.events.iter().any(|e| e.text.contains("no new commits since the last merge")
            && e.text.contains(&commit[..7])
            && e.text.contains("awaiting validation again")),
        "{:?}",
        again.events
    );
    assert!(again.finished_ms.is_some() && again.error.is_none());
    // The worktree remains; the diff shows what the merge brought; validating completes.
    assert!(t.worktree.is_dir());
    assert_eq!(diff_adds(&core.task_diff(&env.project, &t.id).expect("diff")), ["+content"]);
    assert_eq!(core.task_validate(&env.project, &t.id).expect("validate").status, TaskStatus::Merged);
    core.shutdown();
}

#[test]
fn done_while_awaiting_validation_needs_new_commits() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = task_in_review(&env, "Deliver later", "feature.txt");
    core.task_merge(&env.project, &t.id).expect("merge");
    // Without new commits: 409 with the explanation, and nothing changes.
    match core.task_done(&env.project, &t.id, "One more thing.") {
        Err(TaskError::Conflict(m)) => assert_eq!(
            m,
            format!(
                "task {} was already merged and awaits the user's validation; there are no new commits to deliver. If something is still missing, the user decides (Reject)",
                t.id
            )
        ),
        other => panic!("{other:?}"),
    }
    let still = core.task(&env.project, &t.id).expect("task");
    assert_eq!(still.status, TaskStatus::Validating);
    assert!(still.report.as_deref().is_some_and(|r| r.starts_with("Done.")), "the old report stayed");
    // The user fixed something in the task's tab: with a new commit the agent can deliver.
    commit_file(&t.worktree, "fix.txt", "ok\n", "fix: fixed in the tab");
    let done = core.task_done(&env.project, &t.id, "I delivered the fix made in the tab.").expect("done");
    assert_eq!(done.status, TaskStatus::Review);
    assert_eq!(done.report.as_deref(), Some("I delivered the fix made in the tab."));
    assert!(done.diff_stat.as_deref().is_some_and(|s| s.contains("fix.txt") && !s.contains("feature.txt")), "{:?}", done.diff_stat);
    assert!(done.events.iter().any(|e| e.text.contains("the agent delivered more work after the merge; in review")), "{:?}", done.events);
    // Then the normal cycle: merge → awaiting validation → validate.
    assert_eq!(core.task_merge(&env.project, &t.id).expect("merge 2").status, TaskStatus::Validating);
    assert_eq!(core.task_validate(&env.project, &t.id).expect("validate").status, TaskStatus::Merged);
    assert!(env.ws.join("fix.txt").is_file());
    core.shutdown();
}

#[test]
fn validate_completes_with_a_warning_when_the_base_branch_is_gone() {
    need_git!();
    let env = start();
    let core = &env.core;
    git(&env.ws, &["branch", "stable"]);
    git(&env.ws, &["checkout", "-q", "stable"]);
    let t = core.task_create(&env.project, TaskSpec { base: Some("stable".into()), ..spec("Base vanishes") }).expect("create");
    let t = core.task_start(&env.project, &t.id).expect("start");
    commit_file(&t.worktree, "feature.txt", "content\n", "feat: base vanishes");
    core.task_done(&env.project, &t.id, "Done.").expect("done");
    core.task_merge(&env.project, &t.id).expect("merge");
    // The base is deleted by hand (the repository goes back to main) while the task waits for validation.
    git(&env.ws, &["checkout", "-q", "main"]);
    git(&env.ws, &["branch", "-D", "stable"]);
    let done = core.task_validate(&env.project, &t.id).expect("validate without the base");
    assert_eq!(done.status, TaskStatus::Merged);
    assert!(
        done.events.iter().any(|e| e
            .text
            .contains("warning: the base stable no longer exists; I did not check whether the branch had commits outside it")),
        "{:?}",
        done.events
    );
    assert!(!t.worktree.exists());
    assert!(!branches(&env.ws).contains(&t.branch), "the task's branch was removed");
    core.shutdown();
}

#[test]
fn the_task_level_is_recorded_and_the_choice_warns_when_the_model_falls_short() {
    use agentdeck_core::capability::Level;
    need_git!();
    let env = start();
    let log = |t: &agentdeck_core::Task| t.events.iter().map(|e| e.text.clone()).collect::<Vec<_>>().join("\n");
    // Without a level, media holds; the request is saved.
    let plain = env.core.task_create(&env.project, spec("No level")).expect("create");
    assert_eq!(plain.level, Level::Medium);
    assert!(!log(&plain).contains("capacity warning"), "{}", log(&plain));
    let big = env
        .core
        .task_create(&env.project, TaskSpec { level: Level::Complex, model: Some("haiku".into()), ..spec("An engine") })
        .expect("create");
    assert_eq!(big.level, Level::Complex);
    assert_eq!(env.core.task(&env.project, &big.id).map(|t| t.level), Some(Level::Complex));
    // The text after the label comes from `capability.rs` (another area): it stays as that area has it.
    assert!(log(&big).contains("capacity warning: a tarefa é complexa e claude haiku só vai até simples"), "{}", log(&big));
    // Low effort drops a level: sonnet (complexa) becomes media.
    let low = env
        .core
        .task_create(
            &env.project,
            TaskSpec {
                level: Level::Complex,
                model: Some("sonnet".into()),
                effort: Some(agentdeck_core::models::Effort::Low),
                ..spec("Quick")
            },
        )
        .expect("create");
    assert!(log(&low).contains("claude sonnet só vai até media"), "{}", log(&low));
    // Automatic agent: the one capable of the level, with the reason recorded.
    let auto =
        env.core.task_create(&env.project, TaskSpec { kind: String::new(), level: Level::Complex, ..spec("Automatic") }).expect("create");
    assert_eq!(auto.kind, "claude");
    assert!(log(&auto).contains("by capacity and quota: complexa: claude sonnet, capaz"), "{}", log(&auto));
}

#[test]
fn the_models_inventory_lists_the_capacity_of_each_model() {
    need_git!();
    let env = start();
    let lines = env.core.capability_lines(&env.core.models_inventory(None).expect("inventory"));
    let sonnet = lines.iter().find(|l| l.starts_with("Claude Sonnet 5.5")).expect("sonnet");
    // The capacity wording belongs to `capability.rs` (another area): it stays as that area has it.
    assert!(sonnet.contains("complexa, Coding Agent Index 59.3 (2026-10)"), "{sonnet}");
}
