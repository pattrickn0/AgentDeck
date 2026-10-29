//! Durable mail between agents: a long or multi-line message to a tab is saved on disk and only a one-line
//! notice is typed; the addressee reads it with `GET /mail/<n>` (`ask-agent --read`).
//!
//! The interface is played by the test: it reads the `Deliver` events and tells the core when it "typed" a notice.

use std::time::Duration;

use crossbeam_channel::Receiver;
use holt_core::{Core, CoreEvent, Delivery};
use serde_json::{json, Value};

#[path = "support/tasks_env.rs"]
mod env;
use env::*;

const LONG: &str = "Please review the whole change in the branch, check the tests and tell me what is missing: this is a \
message that is clearly longer than two hundred characters, so it cannot be typed into a terminal in one go without risking a split paste.";

fn start() -> Env {
    start_with(true, false)
}

fn next_delivery(rx: &Receiver<CoreEvent>) -> Delivery {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match rx.recv_timeout(left) {
            Ok(CoreEvent::Deliver(d)) => return d,
            Ok(_) => continue,
            Err(_) => panic!("no delivery requested from the interface"),
        }
    }
}

fn no_delivery(rx: &Receiver<CoreEvent>) {
    std::thread::sleep(Duration::from_millis(150));
    let extra: Vec<Delivery> = rx
        .try_iter()
        .filter_map(|e| match e {
            CoreEvent::Deliver(d) => Some(d),
            _ => None,
        })
        .collect();
    assert!(extra.is_empty(), "unexpected deliveries: {extra:?}");
}

/// `POST /ask` from `shell` (the master token) to `claude@dev`, in a thread.
fn ask_in_thread(core: &Core, message: &str, wait: Option<u64>) -> std::thread::JoinHandle<(u16, Value)> {
    let core = core.clone();
    let mut body = json!({"from": "shell", "to": "claude@dev", "message": message});
    if let Some(w) = wait {
        body["wait"] = json!(w);
    }
    std::thread::spawn(move || http(&core, "POST", "/ask", core.bridge_token(), &body.to_string()))
}

fn read_mail(env: &Env, token: &str, n: u64) -> (u16, Value) {
    http(&env.core, "GET", &format!("/mail/{n}"), token, "")
}

fn mail_dir(env: &Env) -> std::path::PathBuf {
    env.root.join("data").join("mail").join(&env.project)
}

#[test]
fn a_short_message_is_typed_as_before() {
    let env = start();
    let asking = ask_in_thread(&env.core, "what is the capital of France?", None);
    let d = next_delivery(&env.rx);
    assert_eq!((d.ident.id().as_str(), d.mail), ("claude@dev", None));
    assert!(d.prompt.contains("what is the capital of France?"));
    assert!(d.prompt.contains(&format!("ask-agent --reply {} ", d.request)), "{}", d.prompt);
    assert!(!mail_dir(&env).exists(), "a short message leaves nothing on disk");
    env.core.fail_delivery(d.request, "end of test");
    asking.join().expect("thread");
    env.core.shutdown();
}

#[test]
fn a_long_message_is_stored_and_only_a_notice_is_typed() {
    let env = start();
    let asking = ask_in_thread(&env.core, LONG, None);
    let d = next_delivery(&env.rx);
    let n = d.mail.expect("a long message goes through the mail");
    assert_eq!((d.ident.id().as_str(), d.create, d.answer, d.from.as_str()), ("claude@dev", true, false, "shell"));
    assert!(
        d.prompt.starts_with(&format!("[mail #{n} from shell: \"Please review the whole change in the branch, check the test")),
        "{}",
        d.prompt
    );
    assert!(d.prompt.ends_with(&format!("…\" — read with: ask-agent --read {n}]")), "{}", d.prompt);
    assert!(!d.prompt.contains('\n') && !d.prompt.contains("clearly longer"), "{}", d.prompt);
    // On disk: the text in its own file and the index.
    assert_eq!(std::fs::read_to_string(mail_dir(&env).join(format!("{n}.txt"))).expect("body"), LONG);
    let index: Value = serde_json::from_str(&std::fs::read_to_string(mail_dir(&env).join("index.json")).expect("index")).expect("json");
    assert_eq!((index["mail"][0]["to"].as_str(), index["mail"][0]["kind"].as_str()), (Some("claude@dev"), Some("question")));
    assert!(index["mail"][0].get("body").is_none(), "the index holds no text");

    // Multi-line text is mail too, however short.
    let asking2 = ask_in_thread(&env.core, "line one\nline two", None);
    let d2 = next_delivery(&env.rx);
    assert!(d2.mail.is_some_and(|m| m != n), "{d2:?}");
    assert!(!d2.prompt.contains('\n'));
    env.core.fail_delivery(d.request, "end of test");
    env.core.fail_delivery(d2.request, "end of test");
    asking.join().expect("thread");
    asking2.join().expect("thread");
    env.core.shutdown();
}

#[test]
fn only_the_addressee_reads_the_mail_and_reading_marks_it() {
    let env = start();
    let asking = ask_in_thread(&env.core, LONG, None);
    let d = next_delivery(&env.rx);
    let n = d.mail.expect("mail");
    env.core.restore_instance(&env.project, "claude@qa");
    let (dev, qa) = (tab_token(&env, "claude@dev"), tab_token(&env, "claude@qa"));

    // Anyone else: refused, and it stays unread.
    let (st, b) = read_mail(&env, &qa, n);
    assert_eq!(st, 403, "{b}");
    assert!(!b.to_string().contains("clearly longer"), "the refusal does not leak the text: {b}");
    assert_eq!(read_mail(&env, env.core.bridge_token(), n).0, 403, "not even the master token");
    assert!(env.core.mail_unread(&env.project, n));
    // The addressee: the full text, how to answer, and it is read from now on.
    let (st, b) = read_mail(&env, &dev, n);
    assert_eq!(st, 200, "{b}");
    assert_eq!(
        (b["text"].as_str(), b["from"].as_str(), b["request"].as_u64(), b["answerable"].as_bool()),
        (Some(LONG), Some("shell"), Some(d.request), Some(true))
    );
    assert!(!env.core.mail_unread(&env.project, n));
    // Missing, or not a number.
    assert_eq!(read_mail(&env, &dev, 99).0, 404);
    assert_eq!(http(&env.core, "GET", "/mail/abc", &dev, "").0, 400);

    // The reply still works as with any question.
    let reply = json!({"to": "", "message": "all good", "reply_to": d.request});
    assert_eq!(http(&env.core, "POST", "/ask", &dev, &reply.to_string()).0, 200);
    let (st, b) = asking.join().expect("thread");
    assert_eq!((st, b["answer"].as_str()), (200, Some("all good")));
    // The question is over: the mail still reads, but says nobody waits for an answer.
    assert_eq!(read_mail(&env, &dev, n).1["answerable"].as_bool(), Some(false));
    env.core.shutdown();
}

#[test]
fn the_notice_comes_again_when_a_turn_ends_without_reading_at_most_twice() {
    let env = start();
    let asking = ask_in_thread(&env.core, LONG, None);
    let d = next_delivery(&env.rx);
    let n = d.mail.expect("mail");
    let dev = tab_token(&env, "claude@dev");
    let idle = || assert_eq!(http(&env.core, "POST", "/signal", &dev, &json!({"state": "idle"}).to_string()).0, 200);

    // The notice is still in the queue (not typed): a turn that ends does not remind.
    idle();
    no_delivery(&env.rx);
    // Typed, and the turn ends without anyone reading: a reminder, the same notice, for the open tab only.
    env.core.mail_typed(&env.project, n);
    idle();
    let r = next_delivery(&env.rx);
    assert_eq!((r.mail, r.request, r.answer, r.create, r.prompt.as_str()), (Some(n), 0, true, false, d.prompt.as_str()));
    // Not typed yet: nothing more.
    idle();
    no_delivery(&env.rx);
    env.core.mail_typed(&env.project, n);
    idle();
    assert_eq!(next_delivery(&env.rx).mail, Some(n));
    env.core.mail_typed(&env.project, n);
    idle();
    no_delivery(&env.rx);

    // Read mail is not reminded.
    assert_eq!(read_mail(&env, &dev, n).0, 200);
    env.core.mail_typed(&env.project, n);
    idle();
    no_delivery(&env.rx);
    env.core.fail_delivery(d.request, "end of test");
    asking.join().expect("thread");
    env.core.shutdown();
}

#[test]
fn a_notice_that_was_lost_is_typed_again_when_the_tab_is_back() {
    let env = start();
    let asking = ask_in_thread(&env.core, LONG, None);
    let d = next_delivery(&env.rx);
    let n = d.mail.expect("mail");
    // A tab that starts while its notice is on the way hears nothing new.
    env.core.mail_tab_started(&env.project, "claude@dev");
    no_delivery(&env.rx);
    // The queue was discarded: the next start announces it.
    env.core.mail_dropped(&env.project, n);
    env.core.mail_tab_started(&env.project, "claude@dev");
    let again = next_delivery(&env.rx);
    assert_eq!((again.mail, again.request, again.prompt.as_str()), (Some(n), 0, d.prompt.as_str()));
    env.core.fail_delivery(d.request, "end of test");
    asking.join().expect("thread");
    env.core.shutdown();
}

#[test]
fn pending_mail_survives_a_restart() {
    let env = start();
    let asking = ask_in_thread(&env.core, LONG, None);
    let d = next_delivery(&env.rx);
    let n = d.mail.expect("mail");
    let read = ask_in_thread(&env.core, "second\nmessage", None);
    let d2 = next_delivery(&env.rx);
    let m = d2.mail.expect("mail");
    assert_eq!(read_mail(&env, &tab_token(&env, "claude@dev"), m).0, 200);
    let Env { _tmp, core, root, config, project, .. } = env;
    core.shutdown();
    asking.join().expect("thread");
    read.join().expect("thread");
    drop(core);

    // The app starts again on the same folder: the unread mail is listed and its notice is typed when the tab is back.
    let (core, rx) = start_core(&root, config, true);
    core.restore_instance(&project, "claude@dev");
    let ident = holt_core::Ident::parse("claude@dev").expect("id");
    let token = core.spawn_spec_for(&project, &ident).expect("spec").env["HOLT_TOKEN"].clone();
    let (st, b) = http(&core, "GET", "/inbox", &token, "");
    assert_eq!(st, 200, "{b}");
    let listed: Vec<u64> = b["mail"].as_array().expect("mail").iter().filter_map(|x| x["mail"].as_u64()).collect();
    assert_eq!(listed, [n], "{b}");
    assert!(b["mail"][0]["preview"].as_str().is_some_and(|p| p.starts_with("Please review")), "{b}");
    core.mail_tab_started(&project, "claude@dev");
    let again = next_delivery(&rx);
    assert_eq!((again.mail, again.prompt.as_str()), (Some(n), d.prompt.as_str()));
    // The text is still there, for its addressee; the question is gone, so nobody waits for an answer.
    let (st, b) = http(&core, "GET", &format!("/mail/{n}"), &token, "");
    assert_eq!((st, b["text"].as_str(), b["answerable"].as_bool()), (200, Some(LONG), Some(false)), "{b}");
    // Read: not listed, not announced again.
    assert!(http(&core, "GET", "/inbox", &token, "").1["mail"].as_array().expect("mail").is_empty());
    core.mail_tab_started(&project, "claude@dev");
    no_delivery(&rx);
    // New messages do not reuse old numbers.
    let asking = {
        let core = core.clone();
        std::thread::spawn(move || {
            let body = json!({"from": "shell", "to": "claude@dev", "message": LONG});
            http(&core, "POST", "/ask", core.bridge_token(), &body.to_string())
        })
    };
    let fresh = next_delivery(&rx);
    assert!(fresh.mail.is_some_and(|x| x > m), "{fresh:?}");
    core.fail_delivery(fresh.request, "end of test");
    asking.join().expect("thread");
    core.shutdown();
}

#[test]
fn a_long_late_answer_is_mail_for_the_asker() {
    let env = start();
    env.core.restore_instance(&env.project, "claude");
    let (claude, dev) = (tab_token(&env, "claude"), tab_token(&env, "claude@dev"));
    let core = env.core.clone();
    let asking = std::thread::spawn({
        let body = json!({"to": "claude@dev", "message": "task", "wait": 1}).to_string();
        move || http(&core, "POST", "/ask", &claude, &body)
    });
    let q = next_delivery(&env.rx);
    assert_eq!((q.mail, asking.join().expect("thread").0), (None, 202));

    let answer = format!("Result:\n{}", "detail ".repeat(60));
    let reply = json!({"to": "", "message": answer, "reply_to": q.request});
    assert_eq!(http(&env.core, "POST", "/ask", &dev, &reply.to_string()).0, 200);
    let late = next_delivery(&env.rx);
    let n = late.mail.expect("a long answer goes through the mail too");
    assert!(late.answer && late.request == q.request, "{late:?}");
    assert_eq!((late.ident.id().as_str(), late.from.as_str()), ("claude", "claude@dev"));
    assert!(
        late.prompt.starts_with(&format!("[mail #{n} from claude@dev: \"Result: detail detail")) && !late.prompt.contains('\n'),
        "{}",
        late.prompt
    );

    // The asker reads it; it is not the dev tab's mail.
    let claude = tab_token(&env, "claude");
    assert_eq!(read_mail(&env, &dev, n).0, 403);
    let (st, b) = read_mail(&env, &claude, n);
    assert_eq!((st, b["kind"].as_str(), b["text"].as_str()), (200, Some("answer"), Some(answer.as_str())), "{b}");
    // Read in full: it is delivered, and --inbox does not offer it again.
    let (_, inbox) = http(&env.core, "GET", "/inbox", &claude, "");
    assert!(inbox["answers"].as_array().expect("answers").is_empty() && inbox["mail"].as_array().expect("mail").is_empty(), "{inbox}");
    env.core.shutdown();
}

#[test]
fn the_inbox_shows_an_answer_in_full_once_not_as_mail_too() {
    let env = start();
    env.core.restore_instance(&env.project, "claude");
    let (claude, dev) = (tab_token(&env, "claude"), tab_token(&env, "claude@dev"));
    let core = env.core.clone();
    let asking = std::thread::spawn({
        let (claude, core) = (claude.clone(), core.clone());
        let body = json!({"to": "claude@dev", "message": "task", "wait": 1}).to_string();
        move || http(&core, "POST", "/ask", &claude, &body)
    });
    let q = next_delivery(&env.rx);
    asking.join().expect("thread");
    let answer = "x".repeat(300);
    let reply = json!({"to": "", "message": answer, "reply_to": q.request});
    assert_eq!(http(&core, "POST", "/ask", &dev, &reply.to_string()).0, 200);
    let late = next_delivery(&env.rx);
    let n = late.mail.expect("mail");
    // The notice was never typed (tab closed): `--inbox` brings the answer, and that counts as reading the mail.
    let (_, b) = http(&core, "GET", "/inbox", &claude, "");
    assert_eq!(b["answers"].as_array().expect("answers").len(), 1, "{b}");
    assert!(b["mail"].as_array().expect("mail").is_empty(), "{b}");
    assert!(!core.mail_unread(&env.project, n));
    env.core.shutdown();
}
