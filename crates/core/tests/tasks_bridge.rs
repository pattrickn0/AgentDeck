//! The tasks HTTP bridge (`/tasks`) with real git: the whole lifecycle through the master token and the
//! permissions of a task's agent (it queries and delivers its own report; it does not merge, discard or create).
//!
//! A few assertions quote answers that belong to other areas (the bridge's permission and validation errors, the
//! `/learn` and `/outline` texts); they stay as those areas have them.

use std::path::Path;

use agentdeck_core::TaskStatus;
use serde_json::{json, Value};

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
fn the_bridge_serves_the_whole_lifecycle_with_the_master_token() {
    need_git!();
    let env = start();
    let master = env.core.bridge_token().to_owned();
    let call = |method: &str, path: &str, body: Value| http(&env.core, method, path, &master, &body.to_string());

    let (st, b) = call("POST", "/tasks", json!({ "title": "Through the bridge", "brief": "Do it through the bridge.", "model": "sonnet" }));
    assert_eq!(st, 200, "{b}");
    assert_eq!((b["task"]["id"].as_str(), b["task"]["status"].as_str()), (Some("t1"), Some("draft")));
    assert_eq!(
        (b["task"]["kind"].as_str(), b["task"]["model"].as_str(), b["task"]["creator"].as_str()),
        (Some("claude"), Some("sonnet"), Some("usuário"))
    );

    // Validation and route errors.
    assert_eq!(call("POST", "/tasks", json!({ "title": " " })).0, 400);
    assert_eq!(call("POST", "/tasks", json!({ "title": "x", "kind": "does-not-exist" })).0, 400);
    assert_eq!(call("POST", "/tasks", json!({ "title": 5 })).0, 400);
    assert_eq!(call("GET", "/tasks/t9", json!(null)).0, 404);
    assert_eq!(call("GET", "/tasks/xyz", json!(null)).0, 400);
    assert_eq!(call("POST", "/tasks/t1/explode", json!({})).0, 404);
    assert_eq!(call("POST", "/tasks/t1/merge", json!({})).0, 409, "a draft is not merged");
    assert_eq!(call("POST", "/tasks/t9/start", json!({})).0, 404);

    // Create already starting.
    let (st, b) = call("POST", "/tasks", json!({ "title": "Starts right away", "brief": "Go.", "start": true }));
    assert_eq!(st, 200, "{b}");
    assert_eq!(b["task"]["status"], "running");
    assert!(Path::new(b["task"]["worktree"].as_str().expect("worktree")).is_dir());
    // Start the t1 draft through its own path.
    let (st, b) = call("POST", "/tasks/t1/start", json!({}));
    assert_eq!((st, b["task"]["status"].as_str()), (200, Some("running")));

    // Lists.
    let (st, b) = call("GET", "/tasks", json!(null));
    assert_eq!(st, 200);
    let list = b["tasks"].as_array().expect("list");
    assert_eq!(list.iter().map(|t| t["id"].as_str().unwrap_or("")).collect::<Vec<_>>(), ["t1", "t2"]);
    assert!(list[0].get("brief").is_none() && list[0].get("report").is_none(), "the list is slim");
    let (st, b) = call("GET", "/tasks/t1", json!(null));
    assert_eq!(st, 200);
    assert_eq!(b["task"]["brief"], "Do it through the bridge.");
    assert!(b["task"]["events"].as_array().is_some_and(|e| e.len() >= 2));
    let (st, b) = http(&env.core, "GET", &format!("/tasks?project={}", env.project), &master, "");
    assert_eq!((st, b["tasks"].as_array().map(Vec::len)), (200, Some(2)));
    assert_eq!(http(&env.core, "GET", "/tasks?project=pnone", &master, "").0, 400);

    // Report, return, merge and discard.
    let t1 = env.core.task(&env.project, "t1").expect("t1");
    commit_file(&t1.worktree, "bridge.txt", "ok\n", "feat: bridge");
    let (st, b) = call("POST", "/tasks/t1/done", json!({ "report": "Through the bridge, ready." }));
    assert_eq!((st, b["task"]["status"].as_str(), b["task"]["report"].as_str()), (200, Some("review"), Some("Through the bridge, ready.")));
    assert!(b["task"]["diff_stat"].as_str().is_some_and(|s| s.contains("bridge.txt")));
    assert_eq!(call("POST", "/tasks/t1/done", json!({ "report": "again" })).0, 409);
    let (st, b) = call("POST", "/tasks/t1/return", json!({ "feedback": "A test is missing." }));
    assert_eq!((st, b["task"]["status"].as_str()), (200, Some("running")));
    assert_eq!(call("POST", "/tasks/t1/done", json!({ "report": "With a test." })).0, 200);
    let (st, b) = call("POST", "/tasks/t1/merge", json!({}));
    assert_eq!((st, b["task"]["status"].as_str()), (200, Some("validating")), "{b}");
    assert!(env.ws.join("bridge.txt").is_file() && t1.worktree.is_dir(), "merging does not clean up");
    // Reject: without a reason it is a usage error; with a reason it goes back to in progress. Then deliver, merge and validate.
    assert_eq!(call("POST", "/tasks/t1/reject", json!({})).0, 400);
    let (st, b) = call("POST", "/tasks/t1/reject", json!({ "feedback": "It does not open in the app." }));
    assert_eq!((st, b["task"]["status"].as_str()), (200, Some("running")), "{b}");
    assert_eq!(call("POST", "/tasks/t1/validate", json!({})).0, 409, "validating only from awaiting validation");
    assert_eq!(call("POST", "/tasks/t1/reject", json!({ "feedback": "x" })).0, 409);
    commit_file(&t1.worktree, "bridge2.txt", "ok\n", "fix: opens in the app");
    assert_eq!(call("POST", "/tasks/t1/done", json!({ "report": "Fixed." })).0, 200);
    assert_eq!(call("POST", "/tasks/t1/merge", json!({})).0, 200);
    let (st, b) = call("POST", "/tasks/t1/validate", json!({}));
    assert_eq!((st, b["task"]["status"].as_str()), (200, Some("merged")), "{b}");
    assert!(!t1.worktree.exists());
    let (st, b) = call("POST", "/tasks/t2/discard", json!({}));
    assert_eq!((st, b["task"]["status"].as_str()), (200, Some("discarded")));
    assert_eq!(call("POST", "/tasks/t2/discard", json!({})).0, 409);
    env.core.shutdown();
}

#[test]
fn a_task_agent_cannot_merge_discard_or_create() {
    need_git!();
    let env = start();
    let core = &env.core;
    // The orchestrator (claude) creates two tasks through the bridge, with its tab's token.
    core.register_instance(&env.project, "claude");
    let boss = tab_token(&env, "claude");
    let created = |title: &str| {
        let (st, b) = http(core, "POST", "/tasks", &boss, &json!({ "title": title, "brief": "Do it.", "start": true }).to_string());
        assert_eq!(st, 200, "{b}");
        b["task"].clone()
    };
    let (t1, t2) = (created("First"), created("Second"));
    assert_eq!((t1["creator"].as_str(), t1["instance"].as_str()), (Some("claude"), Some("claude@t1")));
    assert_eq!(core.team(&env.project, "claude@t1").boss.as_deref(), Some("claude"), "whoever created it is the boss");

    // The agent of task t1 queries and delivers its own task's report, only.
    let agent = tab_token(&env, "claude@t1");
    let call = |token: &str, method: &str, path: &str, body: Value| http(core, method, path, token, &body.to_string());
    assert_eq!(call(&agent, "GET", "/tasks", json!(null)).0, 200);
    assert_eq!(call(&agent, "GET", "/tasks/t2", json!(null)).0, 200);
    for (path, body) in [
        ("/tasks", json!({ "title": "Subtask", "brief": "x" })),
        ("/tasks/t1/start", json!({})),
        ("/tasks/t1/return", json!({ "feedback": "x" })),
        ("/tasks/t1/merge", json!({})),
        ("/tasks/t1/validate", json!({})),
        ("/tasks/t1/reject", json!({ "feedback": "x" })),
        ("/tasks/t1/discard", json!({})),
        ("/tasks/t2/discard", json!({})),
        ("/tasks/t2/merge", json!({})),
    ] {
        let (st, b) = call(&agent, "POST", path, body);
        assert_eq!(st, 403, "{path}: {b}");
        // The wording of the bridge's error belongs to `bridge.rs` (another area): it stays as that area has it.
        assert!(b["error"].as_str().is_some_and(|e| e.contains("só consulta tarefas")), "{b}");
    }
    // Another task's report neither.
    let (st, b) = call(&agent, "POST", "/tasks/t2/done", json!({ "report": "posing as t2" }));
    assert_eq!(st, 403, "{b}");
    assert_eq!(core.task(&env.project, "t2").map(|t| t.status), Some(TaskStatus::Running));
    // And an agent that is not the task's does not deliver a report for it.
    let (st, _) = call(&boss, "POST", "/tasks/t1/done", json!({ "report": "I deliver for it" }));
    assert_eq!(st, 403);
    // Nothing changed: t1 and t2 are still in progress, the creator's name intact, no t3.
    assert_eq!(
        core.tasks(&env.project).iter().map(|t| (t.id.as_str(), t.status)).collect::<Vec<_>>(),
        [("t1", TaskStatus::Running), ("t2", TaskStatus::Running)]
    );

    // The task's own report is valid.
    commit_file(Path::new(t1["worktree"].as_str().expect("worktree")), "done.txt", "ok\n", "feat: done");
    let (st, b) = call(&agent, "POST", "/tasks/t1/done", json!({ "report": "Done by me." }));
    assert_eq!((st, b["task"]["status"].as_str()), (200, Some("review")), "{b}");
    // Whoever created it (the orchestrator, not the task's agent) merges, validates and discards.
    let (st, b) = call(&boss, "POST", "/tasks/t1/merge", json!({}));
    assert_eq!((st, b["task"]["status"].as_str()), (200, Some("validating")), "{b}");
    // Awaiting validation the agent stays alive (the token is valid), but still cannot validate or reject.
    assert_eq!(call(&agent, "GET", "/tasks", json!(null)).0, 200);
    assert_eq!(call(&agent, "POST", "/tasks/t1/validate", json!({})).0, 403);
    assert_eq!(call(&agent, "POST", "/tasks/t1/reject", json!({ "feedback": "x" })).0, 403);
    let (st, b) = call(&boss, "POST", "/tasks/t1/validate", json!({}));
    assert_eq!((st, b["task"]["status"].as_str()), (200, Some("merged")), "{b}");
    let (st, _) = call(&boss, "POST", "/tasks/t2/discard", json!({}));
    assert_eq!(st, 200);
    // The t1 agent's token ceased to exist with the instance; t2's too.
    assert_eq!(call(&agent, "GET", "/tasks", json!(null)).0, 403);
    assert_eq!(t2["id"], "t2");
    core.shutdown();
}

#[test]
fn learn_answers_from_the_worktree_of_the_asking_agent() {
    need_git!();
    let env = start();
    let core = &env.core;
    // A learning versioned in the base, before the task exists (the worktree departs from it).
    let text = "---\ntitle: Careful with src\nsummary: Rule for src.\ntouches: [src/**]\n---\nDo not touch src without reading this.\n";
    std::fs::create_dir_all(env.ws.join("memory").join("aprendizados")).expect("folder");
    commit_file(&env.ws, "memory/aprendizados/src.md", text, "docs: learning");
    core.register_instance(&env.project, "claude");
    let boss = tab_token(&env, "claude");
    let (st, b) = http(core, "POST", "/tasks", &boss, &json!({ "title": "With memory", "brief": "Do it.", "start": true }).to_string());
    assert_eq!(st, 200, "{b}");
    let worktree = b["task"]["worktree"].as_str().expect("worktree").to_owned();
    let agent = tab_token(&env, "claude@t1");
    let ask = |token: &str, paths: Value| http(core, "POST", "/learn", token, &json!({ "paths": paths }).to_string());

    // A relative path and an absolute path inside the worktree give the same answer.
    let (st, rel) = ask(&agent, json!(["src/lib.rs"]));
    assert_eq!(st, 200, "{rel}");
    let rel = rel["text"].as_str().expect("text").to_owned();
    assert!(rel.contains("Do not touch src without reading this.") && rel.contains("### Careful with src"), "{rel}");
    let absolute = Path::new(&worktree).join("src").join("lib.rs").to_string_lossy().into_owned();
    let (st, abs) = ask(&agent, json!([absolute]));
    assert_eq!((st, abs["text"].as_str()), (200, Some(rel.as_str())));
    // A file without a learning: says nothing matches, and shows the index. (The wording belongs to `learnings.rs`.)
    let (st, none) = ask(&agent, json!(["docs/x.md"]));
    assert_eq!(st, 200);
    assert!(
        none["text"]
            .as_str()
            .is_some_and(|t| t.contains("Nenhum aprendizado específico") && t.contains("memory/aprendizados/src.md — Rule for src.")),
        "{none}"
    );
    // Without paths: the globals and the index of the specific ones. The orchestrator (no task) reads from the project's repository.
    let (st, b) = ask(&agent, json!([]));
    assert!(
        st == 200
            && b["text"]
                .as_str()
                .is_some_and(|t| t.starts_with("Aprendizados do projeto:") && t.contains("memory/aprendizados/src.md — Rule for src.")),
        "{st} {b}"
    );
    let (st, b) = ask(&boss, json!(["src/lib.rs"]));
    assert!(st == 200 && b["text"].as_str().is_some_and(|t| t.contains("Do not touch src")), "{st} {b}");
    // POST only.
    assert_eq!(http(core, "GET", "/learn", &boss, "").0, 404);
}

#[test]
fn outline_answers_from_the_worktree_of_the_asking_agent() {
    need_git!();
    let env = start();
    let core = &env.core;
    std::fs::create_dir_all(env.ws.join("src")).expect("folder");
    commit_file(
        &env.ws,
        "src/lib.rs",
        "pub struct S {
    a: u8,
}

impl S {
    pub fn go(&self) -> u8 {
        self.a
    }
}
",
        "src",
    );
    core.register_instance(&env.project, "claude");
    let boss = tab_token(&env, "claude");
    let (st, b) = http(core, "POST", "/tasks", &boss, &json!({ "title": "With outline", "brief": "Do it.", "start": true }).to_string());
    assert_eq!(st, 200, "{b}");
    let worktree = b["task"]["worktree"].as_str().expect("worktree").to_owned();
    // What the task changes in the worktree shows up in its outline, not in the repository's.
    std::fs::write(
        Path::new(&worktree).join("src").join("lib.rs"),
        "pub fn only_in_the_task() {}
",
    )
    .expect("write");
    let agent = tab_token(&env, "claude@t1");
    let ask = |token: &str, paths: Value| http(core, "POST", "/outline", token, &json!({ "paths": paths }).to_string());

    // The outline wording (`linha`, `não existe`, `recusado`...) belongs to `outline.rs` (another area): it stays as that area has it.
    let (st, rel) = ask(&agent, json!(["src/lib.rs"]));
    assert_eq!(st, 200, "{rel}");
    let rel = rel["text"].as_str().expect("text").to_owned();
    assert!(rel.contains("src/lib.rs — 1 linha, Rust") && rel.contains("    1  pub fn only_in_the_task()"), "{rel}");
    // An absolute path inside the worktree gives the same answer.
    let absolute = Path::new(&worktree).join("src").join("lib.rs").to_string_lossy().into_owned();
    let (st, abs) = ask(&agent, json!([absolute]));
    assert_eq!((st, abs["text"].as_str()), (200, Some(rel.as_str())));
    // Without a task (the orchestrator) it reads the project's repository.
    let (st, b) = ask(&boss, json!(["src/lib.rs"]));
    let text = b["text"].as_str().unwrap_or("");
    assert!(st == 200 && text.contains("pub struct S { a }") && text.contains("  pub fn go(&self) -> u8"), "{st} {b}");
    // A file that does not exist, `..` and a folder become a notice, not an error; the rest of the list goes on.
    let (st, b) = ask(&agent, json!(["does-not-exist.rs", "../outside.rs", "src", "src/lib.rs"]));
    let text = b["text"].as_str().unwrap_or("");
    assert!(
        st == 200
            && text.contains("does-not-exist.rs — não existe")
            && text.contains("../outside.rs — recusado")
            && text.contains("src — é uma pasta")
            && text.contains("only_in_the_task"),
        "{st} {b}"
    );
    // Without files: a usage error. POST only.
    assert_eq!(ask(&agent, json!([])).0, 400);
    assert_eq!(http(core, "GET", "/outline", &boss, "").0, 404);
    core.shutdown();
}

#[test]
fn the_bridge_reads_the_effort_of_a_new_task() {
    need_git!();
    let env = start();
    let token = tab_token(&env, "claude");
    let post = |body: &str| http(&env.core, "POST", "/tasks", &token, body);
    // The request's own words are protocol input: the Portuguese spelling `Alto` is accepted as `high`.
    let (status, body) = post(r#"{"title":"Quick","kind":"claude","effort":"Alto"}"#);
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["task"]["effort"], "high");
    let (status, body) = post(r#"{"title":"Default","kind":"claude"}"#);
    assert_eq!((status, &body["task"]["effort"]), (200, &serde_json::Value::Null));
    let (status, body) = post(r#"{"title":"Maximum","kind":"claude","effort":"max"}"#);
    assert_eq!(status, 400);
    assert!(body["error"].as_str().is_some_and(|e| e.contains("esforço inválido")), "{body}");
    let (status, _) = post(r#"{"title":"Number","kind":"claude","effort":3}"#);
    assert_eq!(status, 400);
    env.core.shutdown();
}

#[test]
fn the_bridge_reads_the_level_of_a_new_task() {
    need_git!();
    let env = start();
    let token = tab_token(&env, "claude");
    let post = |body: &str| http(&env.core, "POST", "/tasks", &token, body);
    let (status, body) = post(r#"{"title":"Big","kind":"claude","level":"complexa"}"#);
    assert_eq!((status, &body["task"]["level"]), (200, &serde_json::json!("complexa")), "{body}");
    let (status, body) = post(r#"{"title":"Default","kind":"claude"}"#);
    assert_eq!((status, &body["task"]["level"]), (200, &serde_json::json!("media")));
    let (status, body) = post(r#"{"title":"Medium","kind":"claude","level":"Média"}"#);
    assert_eq!((status, &body["task"]["level"]), (200, &serde_json::json!("media")));
    let (status, body) = post(r#"{"title":"Huge","kind":"claude","level":"enorme"}"#);
    assert_eq!(status, 400);
    assert!(body["error"].as_str().is_some_and(|e| e.contains("nível inválido")), "{body}");
    env.core.shutdown();
}

#[test]
fn the_bridge_reads_the_scope_of_a_new_task_and_queues_on_start() {
    need_git!();
    let env = start();
    let token = tab_token(&env, "claude");
    let post = |body: &str| http(&env.core, "POST", "/tasks", &token, body);
    // A list of paths or a text with one per line; it arrives normalized and shows up in the list and in the detail.
    let (status, body) = post(r#"{"title":"Core","kind":"claude","scope":["crates/core/","Crates\\Core\\"],"start":true}"#);
    assert_eq!(status, 200, "{body}");
    assert_eq!((body["task"]["status"].as_str(), &body["task"]["scope"]), (Some("running"), &json!(["crates/core"])));
    let (status, body) = post(r#"{"title":"Git","kind":"claude","scope":"crates/core/src/git.rs\n\nmemory/GERAL.md","start":true}"#);
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["task"]["status"], "queued");
    assert_eq!(body["task"]["waiting_for"], "t1");
    assert_eq!(body["task"]["scope"], json!(["crates/core/src/git.rs", "memory/GERAL.md"]));
    let (status, list) = http(&env.core, "GET", "/tasks", &token, "");
    assert_eq!(status, 200, "{list}");
    assert_eq!((list["tasks"][1]["status"].as_str(), list["tasks"][1]["waiting_for"].as_str()), (Some("queued"), Some("t1")));
    assert_eq!(list["tasks"][1]["scope"], json!(["crates/core/src/git.rs", "memory/GERAL.md"]));
    let (status, shown) = http(&env.core, "GET", "/tasks/t2", &token, "");
    assert_eq!((status, shown["task"]["waiting_for"].as_str()), (200, Some("t1")));
    // Invalid scope or of another type: 400, without creating.
    for bad in [r#""scope":["crates/*.rs"]"#, r#""scope":["/abs"]"#, r#""scope":["../x"]"#, r#""scope":[3]"#, r#""scope":7"#] {
        let (status, body) = post(&format!(r#"{{"title":"Bad","kind":"claude",{bad}}}"#));
        assert_eq!(status, 400, "{bad}: {body}");
    }
    assert_eq!(env.core.tasks(&env.project).len(), 2);
    env.core.shutdown();
}
