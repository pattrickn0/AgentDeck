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
fn only_claude_has_a_chat_view_and_the_choice_is_remembered_per_tab() {
    let env = start_with(false, false);
    let claude = Ident::parse("claude@dev").expect("ident");
    assert!(env.core.tab_is_chat(&env.project, &claude), "chat is the default for Claude");
    assert!(!env.core.tab_is_chat(&env.project, &Ident::main("aider")));
    assert!(env.core.spawn_spec_chat(&env.project, &Ident::main("aider")).is_err());
    env.core.set_tab_chat(&env.project, &claude.id(), false);
    assert!(!env.core.tab_is_chat(&env.project, &claude));
    assert!(env.core.tab_is_chat(&env.project, &Ident::parse("claude@other").expect("ident")));
    env.core.set_tab_chat(&env.project, &claude.id(), true);
    assert!(env.core.tab_is_chat(&env.project, &claude));
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
    let proc = env.core.spawn_chat(&spec, Arc::new(move || { w.fetch_add(1, std::sync::atomic::Ordering::SeqCst); })).expect("spawn");

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
