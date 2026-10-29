//! `ask-agent --learn` brings the current situation of the project (without files) and the clash warning with
//! another task (with files), for the agent of any tab, against the real git.

use std::path::Path;
use std::time::Instant;

use serde_json::json;

use agentdeck_core::TaskStatus;

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

/// The text of `POST /learn` for the token's agent, with the given `paths`.
fn learn(env: &Env, token: &str, paths: &[&str]) -> String {
    let (st, b) = http(&env.core, "POST", "/learn", token, &json!({ "paths": paths }).to_string());
    assert_eq!(st, 200, "{b}");
    b["text"].as_str().expect("text").to_owned()
}

/// Creates and starts a task through the orchestrator; returns the folder of the worktree.
fn start_task(env: &Env, boss: &str, title: &str) -> String {
    start_task_with(env, boss, title, "Do it.")
}

fn start_task_with(env: &Env, boss: &str, title: &str, brief: &str) -> String {
    let (st, b) = http(&env.core, "POST", "/tasks", boss, &json!({ "title": title, "brief": brief, "start": true }).to_string());
    assert_eq!(st, 200, "{b}");
    b["task"]["worktree"].as_str().expect("worktree").to_owned()
}

const HISTORY: &str = "memory/history.md";

#[test]
fn learn_brings_the_situation_to_any_tab_and_warns_about_clashes() {
    need_git!();
    let env = start();
    let core = &env.core;
    core.register_instance(&env.project, "claude");
    let boss = tab_token(&env, "claude");
    let (w1, w2) = (start_task(&env, &boss, "First"), start_task(&env, &boss, "Second"));
    // t1 has `app.rs` and the history in the branch; t2 only has `wip.rs`, without a commit.
    commit_file(Path::new(&w1), "app.rs", "um\n", "feat: app");
    std::fs::create_dir_all(Path::new(&w1).join("memory")).expect("folder");
    commit_file(Path::new(&w1), HISTORY, "- t1\n", "docs: historico");
    std::fs::write(Path::new(&w2).join("wip.rs"), "rascunho\n").expect("wip");
    let (a1, a2) = (tab_token(&env, "claude@t1"), tab_token(&env, "claude@t2"));
    let running = TaskStatus::Running.label();

    // Normal tab, without files: what it already brought and the situation, with both tasks and the base commits.
    let started = Instant::now();
    let text = learn(&env, &boss, &[]);
    eprintln!("learn without files with 2 open tasks: {:?}", started.elapsed());
    assert!(text.starts_with("The project has no learnings yet"), "{text}");
    let (_, situation) = text.split_once("Current situation:\n").unwrap_or_else(|| panic!("no situation: {text}"));
    assert!(situation.contains(&format!("- t1 \"First\" (claude@t1, {running}): touches app.rs, memory/history.md")), "{situation}");
    assert!(situation.contains(&format!("- t2 \"Second\" (claude@t2, {running}): touches wip.rs")), "{situation}");
    assert!(situation.contains("Latest commits on main:"), "{situation}");

    // The agent of a task does not see itself in its own situation.
    let own = learn(&env, &a1, &[]);
    assert!(own.contains("Current situation:") && own.contains("- t2 \"Second\"") && !own.contains("- t1 "), "{own}");
    assert!(own.contains("Latest commits on main:"), "{own}");

    // With files: only the clash warning, at the top; its own file and the history do not count.
    assert!(!learn(&env, &a1, &["app.rs"]).contains("WARNING"), "its own file");
    assert!(!learn(&env, &a2, &[HISTORY]).contains("WARNING"), "the history merges by union");
    assert!(!learn(&env, &a2, &["outro.rs"]).contains("WARNING"), "no clash, nothing extra");
    assert!(!learn(&env, &a2, &["app.rs"]).contains("Current situation:"), "with files the situation does not come whole");
    // File only in the branch of t1.
    let on_branch = learn(&env, &a2, &["app.rs"]);
    assert!(
        on_branch.starts_with(&format!(
            "WARNING:\napp.rs: t1 (claude@t1, {running}; already touched) also touches it; changes to the same lines will conflict when merging\n\n"
        )),
        "{on_branch}"
    );
    // File only without a commit in the worktree of t2.
    let dirty = learn(&env, &a1, &["wip.rs"]);
    assert!(dirty.contains(&format!("wip.rs: t2 (claude@t2, {running}; already touched) also touches it")), "{dirty}");
    // The absolute path inside its own worktree counts as the relative one (and still does not warn about its own file).
    let own_abs = Path::new(&w1).join("app.rs").to_string_lossy().into_owned();
    assert!(!learn(&env, &a1, &[own_abs.as_str()]).contains("WARNING"));
    // The normal tab is warned too.
    assert!(learn(&env, &boss, &["app.rs"]).contains(&format!("app.rs: t1 (claude@t1, {running}; already touched) also touches it")));
    core.shutdown();
}

#[test]
fn learn_without_other_tasks_says_so_and_sends_nothing_extra_for_files() {
    need_git!();
    let env = start();
    env.core.register_instance(&env.project, "claude");
    let boss = tab_token(&env, "claude");
    let w1 = start_task(&env, &boss, "Sozinha");
    commit_file(Path::new(&w1), "app.rs", "um\n", "feat: app");
    let agent = tab_token(&env, "claude@t1");
    let alone = learn(&env, &agent, &[]);
    assert!(alone.contains("Current situation:\nOther tasks in progress: none."), "{alone}");
    assert!(!learn(&env, &agent, &["app.rs"]).contains("WARNING"));
    // The orchestrator (normal tab) is warned about the task that already touched it.
    let running = TaskStatus::Running.label();
    assert!(learn(&env, &boss, &["app.rs"]).contains(&format!("t1 (claude@t1, {running}; already touched)")));
    env.core.shutdown();
}

#[test]
fn learn_stays_quick_with_several_tasks_open() {
    need_git!();
    let env = start();
    env.core.register_instance(&env.project, "claude");
    let boss = tab_token(&env, "claude");
    for n in 0..3 {
        let wt = start_task(&env, &boss, &format!("Task {n}"));
        commit_file(Path::new(&wt), &format!("f{n}.rs"), "x\n", "feat: f");
        std::fs::write(Path::new(&wt).join(format!("unsaved{n}.rs")), "y\n").expect("unsaved");
    }
    let started = Instant::now();
    let text = learn(&env, &boss, &[]);
    let elapsed = started.elapsed();
    eprintln!("learn without files with 3 open tasks: {elapsed:?}");
    let running = TaskStatus::Running.label();
    assert!(text.contains(&format!("- t3 \"Task 2\" (claude@t3, {running}): touches f2.rs, unsaved2.rs")), "{text}");
    assert!(elapsed.as_secs() < 5, "slow: {elapsed:?}");
    env.core.shutdown();
}

#[test]
fn a_fresh_task_clashes_through_its_request_but_a_merged_one_does_not() {
    need_git!();
    let env = start();
    env.core.register_instance(&env.project, "claude");
    let boss = tab_token(&env, "claude");
    commit_file(
        &env.ws,
        "target.rs",
        "base
",
        "feat: alvo",
    );
    commit_file(
        &env.ws,
        "ready.rs",
        "base
",
        "feat: ready",
    );
    // t1 has just been born: clean worktree, no commit; only the request cites `target.rs`.
    start_task_with(&env, &boss, "Just created", "Change target.rs and nothing else.");
    // t2 also touches `ready.rs`, delivers and is merged: it stays waiting for validation, already in the base.
    let w2 = start_task_with(&env, &boss, "Already merged", "Change ready.rs.");
    commit_file(
        Path::new(&w2),
        "ready.rs",
        "novo
",
        "feat: ready",
    );
    let (st, b) = http(&env.core, "POST", "/tasks/t2/done", &tab_token(&env, "claude@t2"), &json!({ "report": "Done." }).to_string());
    assert_eq!(st, 200, "{b}");
    let (st, b) = http(&env.core, "POST", "/tasks/t2/merge", &boss, "{}");
    assert_eq!((st, b["task"]["status"].as_str()), (200, Some("validating")), "{b}");
    let w3 = start_task(&env, &boss, "Third");
    assert!(Path::new(&w3).is_dir());
    let agent = tab_token(&env, "claude@t3");
    let running = TaskStatus::Running.label();

    let cited = learn(&env, &agent, &["target.rs"]);
    assert!(
        cited.starts_with(&format!(
            "WARNING:
target.rs: t1 (claude@t1, {running}; cited by the request) also touches it;"
        )),
        "{cited}"
    );
    // Waiting for validation is already in the base: no warning, but it stays in the situation.
    assert!(!learn(&env, &agent, &["ready.rs"]).contains("WARNING"));
    let situation = learn(&env, &agent, &[]);
    assert!(
        situation
            .contains(&format!("- t1 \"Just created\" (claude@t1, {running}): no files in the branch yet; the request cites target.rs")),
        "{situation}"
    );
    assert!(situation.contains(&format!("- t2 \"Already merged\" (claude@t2, {})", TaskStatus::Validating.label())), "{situation}");
    env.core.shutdown();
}
