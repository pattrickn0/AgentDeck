//! The chat view's plumbing with the real core and a fake `claude`: the launch (structured mode, the tab's own
//! conversation), the saved conversation read back, and a process that gets a message, asks for a permission, gets the
//! answer and ends.

use std::sync::Arc;
use std::time::{Duration, Instant};

use holt_core::chat::{self, ChatEvent, ChatMsg, Item};
use holt_core::Ident;
use serde_json::{json, Value};

#[path = "support/tasks_env.rs"]
mod env;
use env::*;

fn arg_after(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

#[test]
fn the_chat_launch_uses_the_structured_mode_and_the_tabs_own_conversation() {
    let env = start_with(false, false);
    let ident = Ident::parse("claude@dev").expect("ident");
    let id = ident.id();

    let first = env.core.spawn_spec_chat(&env.project, &ident).expect("spec");
    for flag in ["-p", "--include-partial-messages", "--verbose"] {
        assert!(first.args.iter().any(|a| a == flag), "{flag} in {:?}", first.args);
    }
    assert_eq!(arg_after(&first.args, "--input-format").as_deref(), Some("stream-json"));
    assert_eq!(arg_after(&first.args, "--output-format").as_deref(), Some("stream-json"));
    assert_eq!(arg_after(&first.args, "--permission-prompt-tool").as_deref(), Some("stdio"));
    // The end of a turn is an event of the stream: no hooks that call back.
    assert!(!first.args.iter().any(|a| a == "--settings"), "{:?}", first.args);
    // The answer to another agent still needs `ask-agent`.
    assert!(first.args.iter().any(|a| a == "Bash(ask-agent:*)"));
    let session = arg_after(&first.args, "--session-id").expect("a new conversation gets an id");
    assert!(!first.resumed);
    assert_eq!(env.core.tab_session(&env.project, &id).as_deref(), Some(session.as_str()));

    // The CLI saved the conversation: the next launch resumes it and the chat shows what was said.
    let dir = env.root.join("home").join(".claude").join("projects").join("-ws");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let lines = [
        json!({ "type": "user", "message": { "content": "Add a menu entry" } }),
        json!({ "type": "assistant", "message": { "id": "m1", "content": [{ "type": "text", "text": "Done." }] } }),
    ];
    let text = lines.iter().map(Value::to_string).collect::<Vec<_>>().join("\n");
    std::fs::write(dir.join(format!("{session}.jsonl")), text).expect("transcript");
    let again = env.core.spawn_spec_chat(&env.project, &ident).expect("spec");
    assert_eq!(arg_after(&again.args, "--resume").as_deref(), Some(session.as_str()));
    assert!(again.resumed);
    let history = env.core.chat_history(&env.project, &id);
    assert_eq!(history.items.len(), 2, "{:?}", history.items);
    assert!(matches!(&history.items[1], Item::Assistant { text, .. } if text == "Done."));
    assert_eq!(history.session.as_deref(), Some(session.as_str()));

    // The terminal launch of the same tab keeps resuming the same conversation, with its hooks.
    let term = env.core.spawn_spec_for(&env.project, &ident).expect("spec");
    assert_eq!(arg_after(&term.args, "--resume").as_deref(), Some(session.as_str()));
    assert!(term.args.iter().any(|a| a == "--settings"));
    assert!(!term.args.iter().any(|a| a == "-p"));
    env.core.shutdown();
}

#[test]
fn a_task_prompt_becomes_the_first_message_instead_of_an_argument() {
    let env = start_with(false, false);
    let ident = Ident::parse("claude@t1").expect("ident");
    env.core.set_boot_prompt(&env.project, &ident.id(), "Read the brief in data/tasks/t1.md and start.".into());
    let spec = env.core.spawn_spec_chat(&env.project, &ident).expect("spec");
    assert_eq!(spec.first_message.as_deref(), Some("Read the brief in data/tasks/t1.md and start."));
    assert!(!spec.args.iter().any(|a| a.contains("brief in data")), "{:?}", spec.args);
    env.core.shutdown();
}

#[test]
fn claude_antigravity_and_codex_have_a_chat_view_and_the_choice_is_remembered_per_tab() {
    let env = start_with(false, false);
    let claude = Ident::parse("claude@dev").expect("ident");
    let agy = Ident::parse("antigravity@dev").expect("ident");
    let codex = Ident::parse("codex@dev").expect("ident");
    assert!(env.core.tab_is_chat(&env.project, &claude), "chat is the default for Claude");
    assert!(env.core.tab_is_chat(&env.project, &agy), "chat is the default for Antigravity");
    assert!(env.core.tab_is_chat(&env.project, &codex), "chat is the default for Codex");
    assert!(!env.core.tab_is_chat(&env.project, &Ident::main("aider")));
    assert!(env.core.spawn_spec_chat(&env.project, &Ident::main("aider")).is_err());

    env.core.set_tab_chat(&env.project, &claude.id(), false);
    assert!(!env.core.tab_is_chat(&env.project, &claude));
    assert!(env.core.tab_is_chat(&env.project, &Ident::parse("claude@other").expect("ident")));
    env.core.set_tab_chat(&env.project, &claude.id(), true);
    assert!(env.core.tab_is_chat(&env.project, &claude));

    env.core.set_tab_chat(&env.project, &agy.id(), false);
    assert!(!env.core.tab_is_chat(&env.project, &agy));
    assert!(env.core.tab_is_chat(&env.project, &Ident::parse("antigravity@other").expect("ident")));
    env.core.set_tab_chat(&env.project, &agy.id(), true);
    assert!(env.core.tab_is_chat(&env.project, &agy));

    env.core.set_tab_chat(&env.project, &codex.id(), false);
    assert!(!env.core.tab_is_chat(&env.project, &codex));
    assert!(env.core.tab_is_chat(&env.project, &Ident::parse("codex@other").expect("ident")));
    env.core.set_tab_chat(&env.project, &codex.id(), true);
    assert!(env.core.tab_is_chat(&env.project, &codex));
    env.core.shutdown();
}

#[test]
fn the_codex_chat_launch_uses_the_structured_mode_and_the_tabs_own_conversation() {
    let env = start_with(false, false);
    install_fake_agent(&env.root, "codex");
    let ident = Ident::parse("codex@dev").expect("ident");
    let id = ident.id();

    let first = env.core.spawn_spec_chat(&env.project, &ident).expect("spec");
    assert_eq!(first.args.first().map(String::as_str), Some("app-server"), "{:?}", first.args);
    assert!(!first.resumed);

    let session = "01a18f37-5976-7bc1-9616-3ab2e181dab3";
    let codex_dir = env.root.join("home").join(".codex").join("sessions").join("2026").join("10").join("30");
    std::fs::create_dir_all(&codex_dir).expect("mkdir");
    std::fs::write(codex_dir.join(format!("rollout-2026-10-30T12-00-00-{session}.jsonl")), "").expect("rollout");
    assert!(env.core.set_tab_session(&env.project, &id, session));

    let again = env.core.spawn_spec_chat(&env.project, &ident).expect("spec");
    assert!(again.resumed);
    assert_eq!(again.session.as_deref(), Some(session));
    assert_eq!(again.args.first().map(String::as_str), Some("app-server"));
    assert!(!again.args.contains(&"resume".to_owned()));

    let history = env.core.chat_history(&env.project, &id);
    assert!(history.history_pending);
    assert_eq!(history.session.as_deref(), Some(session));

    env.core.shutdown();
}

#[test]
fn the_antigravity_chat_launch_uses_the_structured_mode_and_the_tabs_own_conversation() {
    let env = start_with(false, false);
    install_fake_agent(&env.root, "agy");
    let ident = Ident::parse("antigravity@dev").expect("ident");
    let id = ident.id();

    let first = env.core.spawn_spec_chat(&env.project, &ident).expect("spec");
    assert_eq!(arg_after(&first.args, "--input-format").as_deref(), Some("stream-json"));
    assert_eq!(arg_after(&first.args, "--output-format").as_deref(), Some("stream-json"));
    assert_eq!(arg_after(&first.args, "--mode").as_deref(), Some("auto"));
    assert!(!first.args.iter().any(|a| a == "--log-file"), "chat mode does not use log file: {:?}", first.args);
    assert!(!first.resumed);

    // Simulate session saved in config and on disk:
    let session = "3ee16dad-e26c-4460-8dd5-99d773098a00";
    assert!(env.core.set_tab_session(&env.project, &id, session));
    let conv_dir = env.root.join("home").join(".gemini").join("antigravity-cli").join("conversations");
    std::fs::create_dir_all(&conv_dir).expect("mkdir");
    std::fs::write(conv_dir.join(format!("{session}.db")), "").expect("db");

    let brain_dir =
        env.root.join("home").join(".gemini").join("antigravity-cli").join("brain").join(session).join(".system_generated").join("logs");
    std::fs::create_dir_all(&brain_dir).expect("mkdir");
    let transcript = r#"{"step_index":0,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","created_at":"2026-10-30T01:00:00Z","content":"<USER_REQUEST>\nHello Antigravity\n</USER_REQUEST>"}
{"step_index":1,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-10-30T01:00:01Z","content":"Hello from Antigravity!"}"#;
    std::fs::write(brain_dir.join("transcript.jsonl"), transcript).expect("transcript");

    let again = env.core.spawn_spec_chat(&env.project, &ident).expect("spec");
    assert_eq!(arg_after(&again.args, "--conversation").as_deref(), Some(session));
    assert!(again.resumed);

    let history = env.core.chat_history(&env.project, &id);
    assert_eq!(history.items.len(), 2, "{:?}", history.items);
    assert!(matches!(&history.items[0], Item::User { text, .. } if text == "Hello Antigravity"));
    assert!(matches!(&history.items[1], Item::Assistant { text, .. } if text == "Hello from Antigravity!"));
    assert_eq!(history.session.as_deref(), Some(session));

    env.core.shutdown();
}

#[cfg(unix)]
fn install_chat_agent(root: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    // Answers every line; on "need-permission" it asks first and writes the answer it got to `answer.txt`.
    let script = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *need-permission*)
      echo '{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Write","input":{"file_path":"/x"},"tool_use_id":"t1","permission_suggestions":[]}}'
      IFS= read -r answer
      printf '%s\n' "$answer" > answer.txt
      ;;
    *) echo '{"type":"assistant","message":{"id":"m1","content":[{"type":"text","text":"hello"}]}}' ;;
  esac
  echo '{"type":"result","is_error":false,"total_cost_usd":0.5,"session_id":"s"}'
done
"#;
    let path = root.join("fake-bin").join("claude");
    std::fs::write(&path, script).expect("write");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

#[cfg(unix)]
fn wait_for(proc: &chat::ChatProcess, what: &str, mut done: impl FnMut(&ChatMsg) -> bool) -> Vec<ChatMsg> {
    let limit = Instant::now() + Duration::from_secs(15);
    let mut seen = Vec::new();
    loop {
        assert!(Instant::now() < limit, "timed out waiting for {what}; got {seen:?}");
        match proc.try_recv() {
            Some(msg) => {
                let stop = done(&msg);
                seen.push(msg);
                if stop {
                    return seen;
                }
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

#[cfg(unix)]
#[test]
fn a_message_goes_in_the_events_come_back_and_the_permission_answer_reaches_the_process() {
    let env = start_with(false, false);
    install_chat_agent(&env.root);
    let ident = Ident::parse("claude@dev").expect("ident");
    let spec = env.core.spawn_spec_chat(&env.project, &ident).expect("spec");
    let woken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let w = woken.clone();
    let proc = env
        .core
        .spawn_chat(
            &spec,
            Arc::new(move || {
                w.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }),
        )
        .expect("spawn");

    proc.send_user("hi");
    let seen = wait_for(&proc, "the first turn", |m| matches!(m, ChatMsg::Event(ChatEvent::Result(_))));
    assert!(seen.iter().any(|m| matches!(m, ChatMsg::Event(ChatEvent::Assistant { .. }))));
    assert!(woken.load(std::sync::atomic::Ordering::SeqCst) > 0, "the interface is woken");

    proc.send_user("need-permission");
    let seen = wait_for(&proc, "the permission request", |m| matches!(m, ChatMsg::Event(ChatEvent::Permission(_))));
    let Some(ChatMsg::Event(ChatEvent::Permission(request))) = seen.last() else { unreachable!() };
    assert_eq!((request.request_id.as_str(), request.tool.as_str()), ("r1", "Write"));
    proc.answer_permission("r1", true, &request.input, "", &[]);
    wait_for(&proc, "the end of that turn", |m| matches!(m, ChatMsg::Event(ChatEvent::Result(_))));
    let answer: Value = serde_json::from_str(&std::fs::read_to_string(spec.cwd.join("answer.txt")).expect("answer")).expect("json");
    assert_eq!(answer["response"]["request_id"], "r1");
    assert_eq!(answer["response"]["response"]["behavior"], "allow");
    assert_eq!(answer["response"]["response"]["updatedInput"]["file_path"], "/x");

    // Killing the process is reported as its end.
    proc.kill();
    wait_for(&proc, "the exit", |m| matches!(m, ChatMsg::Exited(_)));
    env.core.shutdown();
}

#[cfg(unix)]
#[test]
fn a_process_that_cannot_start_is_an_error_not_a_hang() {
    let env = start_with(false, false);
    let ident = Ident::parse("claude@dev").expect("ident");
    let mut spec = env.core.spawn_spec_chat(&env.project, &ident).expect("spec");
    spec.program = env.root.join("does-not-exist");
    assert!(env.core.spawn_chat(&spec, Arc::new(|| {})).is_err());
    env.core.shutdown();
}

/// Against the real Claude Code (costs a few tokens, needs the user's login): `cargo test -p holt-core --test chat -- --ignored`.
/// Checks that the arguments the core builds are accepted by the installed CLI, that a message gets an answer, and that a
/// process started again with `--resume` knows the conversation.
#[cfg(unix)]
#[test]
#[ignore = "talks to the real claude CLI"]
fn the_real_claude_answers_in_the_chat_mode_and_resumes_the_conversation() {
    isolated_config();
    let tmp = tempfile::tempdir().expect("tmp");
    let root = holt_core::util::strip_verbatim(&std::fs::canonicalize(tmp.path()).expect("canonicalize"));
    let ws = root.join("ws");
    std::fs::create_dir_all(&ws).expect("ws");
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).expect("HOME");
    let opts = holt_core::StartOptions {
        search_path: std::env::var_os("PATH"),
        home_dir: Some(home),
        watch_workspace: false,
        config: Some(holt_core::Config::with_project(ws.clone())),
        system_dirs: false,
    };
    let (core, _rx) = holt_core::Core::start_with(paths_in(&root), opts).expect("start");
    let project = core.active_project_id();
    let ident = Ident::parse("claude@dev").expect("ident");

    let run = |text: &str| -> String {
        let spec = core.spawn_spec_chat(&project, &ident).expect("spec");
        let proc = core.spawn_chat(&spec, Arc::new(|| {})).expect("spawn");
        proc.send_user(text);
        let limit = Instant::now() + Duration::from_secs(120);
        let mut transcript = chat::Transcript::default();
        transcript.push_user(text, None);
        loop {
            assert!(Instant::now() < limit, "no answer: {:?}", transcript.items);
            match proc.try_recv() {
                Some(ChatMsg::Event(ev)) => {
                    transcript.apply(&ev);
                    if matches!(ev, ChatEvent::Result(_)) {
                        break;
                    }
                }
                Some(ChatMsg::Stderr(line)) => eprintln!("stderr: {line}"),
                Some(ChatMsg::Exited(code)) => panic!("exited {code:?}: {:?}", transcript.items),
                None => std::thread::sleep(Duration::from_millis(20)),
            }
        }
        transcript
            .items
            .iter()
            .filter_map(|i| if let Item::Assistant { text, .. } = i { Some(text.clone()) } else { None })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let first = run("Remember the word BANANA7. Reply with just: ok");
    assert!(!first.is_empty());
    // A new process for the same tab resumes the conversation (the second launch uses `--resume`).
    let second = run("What was the word I told you to remember? Reply with just the word.");
    assert!(second.contains("BANANA7"), "{second}");
    core.shutdown();
}
