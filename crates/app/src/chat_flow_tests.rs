//! The chat view of a Claude tab end to end, with the real core and a fake `claude` that speaks the structured mode: a
//! message from another agent becomes part of the conversation (no typing, no Enter), a permission request waits for the
//! user and its answer reaches the CLI, and the tab goes back and forth between chat and terminal on the same
//! conversation. No window: the frame is just `before_frame`.

use std::os::unix::fs::PermissionsExt;

use holt_core::chat::Item;
use holt_core::Ident;
use serde_json::{json, Value};

use crate::tasks_flow_tests::Env;

/// Answers every line with a message and a result; on "need-permission" it asks first and writes the answer it got to
/// `answer.txt` (in the project folder).
const FAKE_CLAUDE: &str = r#"#!/bin/sh
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

fn env_with_chat_agent() -> Env {
    let mut env = Env::start_with(true);
    let script = env.ws.parent().expect("root").join("fake-bin").join("claude");
    std::fs::write(&script, FAKE_CLAUDE).expect("write");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    env.pump("the agent list", |d| d.agents.knows("claude"));
    env.deck.open_tab("claude", false);
    env.pump("the chat tab to be running", |d| d.tabs.first().is_some_and(|t| t.alive));
    env
}

fn items(env: &Env) -> Vec<Item> {
    env.deck.tabs[0].chat.as_ref().expect("a chat tab").transcript.items.clone()
}

#[test]
fn a_claude_tab_opens_in_the_chat_view_with_its_own_conversation() {
    let env = env_with_chat_agent();
    let tab = &env.deck.tabs[0];
    assert!(tab.chat.is_some(), "chat is the default view of a Claude tab");
    let spec = tab.spec.as_ref().expect("spec");
    assert!(spec.args.iter().any(|a| a == "stream-json"), "{:?}", spec.args);
    let id = tab.ident.id();
    assert!(env.core.tab_session(&env.project(), &id).is_some(), "the conversation id is saved as soon as the tab starts");
}

#[test]
fn a_question_from_another_agent_becomes_a_message_of_the_conversation_and_ends_with_the_turn() {
    let mut env = env_with_chat_agent();
    let prompt = "[Message from the agent \"claude@dev\" received by Holt]\n\nWhich key did you pick?\n\nAnswer by running in the shell: ask-agent --reply 7 \"your answer\"";
    env.deck.send_to_instance(&Ident::main("claude"), false, prompt.to_owned()).expect("queued");
    env.pump("the answer", |d| {
        d.tabs[0]
            .chat
            .as_ref()
            .is_some_and(|c| c.transcript.items.iter().any(|i| matches!(i, Item::Assistant { text, .. } if text == "hello")))
    });
    let items = items(&env);
    // The conversation shows the message, not the instructions that Holt adds for the model.
    assert!(
        matches!(&items[0], Item::User { text, from: Some(who) } if text == "Which key did you pick?" && who == "claude@dev"),
        "{items:?}"
    );
    env.pump("the turn to end", |d| !d.tabs[0].chat.as_ref().is_some_and(|c| c.transcript.busy()) && !d.tabs[0].busy);
}

#[test]
fn a_permission_request_waits_for_the_user_and_the_answer_reaches_the_cli() {
    let mut env = env_with_chat_agent();
    env.deck.tabs[0].chat.as_mut().expect("chat").composer = "need-permission please".into();
    env.deck.chat_submit(0);
    assert!(env.deck.tabs[0].busy, "sending a message starts a turn");
    env.pump("the permission card", |d| d.tabs[0].chat.as_ref().is_some_and(|c| c.transcript.waiting_for_permission()));
    assert!(env.deck.tabs[0].chat.as_ref().expect("chat").transcript.busy(), "the turn waits for the answer");
    assert!(env.deck.tabs[0].done || env.deck.active == Some(0));

    env.deck.chat_answer(0, "r1", true, false);
    env.pump("the end of the turn", |d| !d.tabs[0].busy);
    let answer: Value = serde_json::from_str(&std::fs::read_to_string(env.ws.join("answer.txt")).expect("answer")).expect("json");
    assert_eq!(answer["response"]["response"]["behavior"], "allow");
    assert_eq!(answer["response"]["request_id"], "r1");
    assert!(!env.deck.tabs[0].chat.as_ref().expect("chat").transcript.waiting_for_permission());
}

#[test]
fn a_terminal_only_command_is_not_sent() {
    let mut env = env_with_chat_agent();
    env.deck.tabs[0].chat.as_mut().expect("chat").composer = "/config".into();
    env.deck.chat_submit(0);
    let items = items(&env);
    assert!(matches!(items.last(), Some(Item::Notice { text, .. }) if text.contains("/config")), "{items:?}");
    assert!(!env.deck.tabs[0].busy, "nothing was sent, so no turn started");
    assert!(env.deck.tabs[0].chat.as_ref().expect("chat").composer.is_empty());
}

#[test]
fn the_tab_goes_to_the_terminal_and_back_on_the_same_conversation() {
    let mut env = env_with_chat_agent();
    let id = env.deck.tabs[0].ident.id();
    let session = env.core.tab_session(&env.project(), &id).expect("session");
    // What the CLI saved of the conversation so far.
    let dir = env.ws.parent().expect("root").join("home").join(".claude").join("projects").join("-ws");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let lines = [
        json!({ "type": "user", "message": { "content": "first question" } }),
        json!({ "type": "assistant", "message": { "id": "m1", "content": [{ "type": "text", "text": "first answer" }] } }),
    ];
    std::fs::write(dir.join(format!("{session}.jsonl")), lines.iter().map(Value::to_string).collect::<Vec<_>>().join("\n"))
        .expect("transcript");

    env.deck.set_tab_view(0, false);
    assert!(env.deck.tabs[0].chat.is_none() && env.deck.tabs[0].alive);
    let spec = env.deck.tabs[0].spec.as_ref().expect("spec");
    assert!(spec.resumed && spec.args.contains(&session) && !spec.args.iter().any(|a| a == "stream-json"), "{:?}", spec.args);
    assert!(!env.core.tab_is_chat(&env.project(), &env.deck.tabs[0].ident), "the choice is remembered");

    env.deck.set_tab_view(0, true);
    let spec = env.deck.tabs[0].spec.as_ref().expect("spec");
    assert!(spec.resumed && spec.args.iter().any(|a| a == "stream-json"));
    let items = items(&env);
    assert_eq!(items.len(), 2, "the conversation comes back from the CLI's own transcript: {items:?}");
    assert!(matches!(&items[0], Item::User { text, .. } if text == "first question"));
    assert_eq!(env.core.tab_session(&env.project(), &id).as_deref(), Some(session.as_str()), "same conversation");
}
