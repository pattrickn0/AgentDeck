//! End-to-end tests of the HTTP bridge and of the headless runner, with `Core`
//! in temporary folders and fake agents in the search PATH (the `fake_agent` binary,
//! see `support/fake_agent.rs`).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::time::Duration;

use agentdeck_core::paths::AppPaths;
use agentdeck_core::FeedKind;
use agentdeck_core::{Config, Core, CoreEvent, StartOptions};
use crossbeam_channel::Receiver;
use serde_json::{json, Value};

struct Env {
    _tmp: tempfile::TempDir,
    core: Core,
    rx: Receiver<CoreEvent>,
    root: std::path::PathBuf,
}

fn start(fake_agents: &[(&str, Value)]) -> Env {
    start_cfg(fake_agents, |_| {})
}

/// Like [`start`], with the configuration tweaked by `tweak` before `Core` starts.
fn start_cfg(fake_agents: &[(&str, Value)], tweak: impl FnOnce(&mut Config)) -> Env {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().to_path_buf();
    let paths = AppPaths {
        app_dir: root.clone(),
        bin_dir: root.join("bin"),
        data_dir: root.join("dados"),
        default_workspace: root.join("workspace"),
        whisper_dir: root.join("whisper"),
        config_file: root.join("cfg").join("config.json"),
    };
    let fake_bin = root.join("fake-bin");
    std::fs::create_dir_all(&fake_bin).expect("mkdir");
    for (name, spec) in fake_agents {
        install_fake(&fake_bin, name, spec);
    }
    let mut config = Config::with_project(paths.default_workspace.clone());
    tweak(&mut config);
    let opts = StartOptions {
        search_path: Some(fake_bin.into_os_string()),
        home_dir: Some(root.join("home")),
        watch_workspace: false,
        config: Some(config),
        system_dirs: false,
    };
    let (core, rx) = Core::start_with(paths, opts).expect("start");
    Env { _tmp: tmp, core, rx, root }
}

/// The app's folders inside `root` (as in `start`).
fn paths_in(root: &Path) -> AppPaths {
    AppPaths {
        app_dir: root.to_path_buf(),
        bin_dir: root.join("bin"),
        data_dir: root.join("dados"),
        default_workspace: root.join("workspace"),
        whisper_dir: root.join("whisper"),
        config_file: root.join("cfg").join("config.json"),
    }
}

/// Token that the tab of instance `id` in the active project receives in the environment.
fn tab_token(env: &Env, id: &str) -> String {
    tab_token_in(env, &env.core.active_project_id(), id)
}

/// Token that the tab of instance `id` in project `project` receives in the environment.
fn tab_token_in(env: &Env, project: &str, id: &str) -> String {
    let ident = agentdeck_core::Ident::parse(id).expect("id");
    env.core.spawn_spec_for(project, &ident).expect("spec").env["AGENTDECK_TOKEN"].clone()
}

/// Path of the fake agent compiled with the tests (the `fake_agent` `[[bin]]`).
const FAKE_AGENT: &str = env!("CARGO_BIN_EXE_fake_agent");

/// Executable file name of an agent, as the system looks for it in the search PATH.
fn exe_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

/// Behaviour of the fake agent that prints `text` (and always reads stdin). The fields are
/// described in `support/fake_agent.rs`.
fn says(text: &str) -> Value {
    json!({ "stdout": text })
}

/// Fake agent that prints the arguments (the instructions become `<sys>`), the environment and echoes
/// the prompt received on stdin.
fn echoing_agent() -> Value {
    json!({ "echo_args": true, "echo_env": ["AGENTDECK_AGENT", "AGENTDECK_DEPTH"], "echo_stdin": true })
}

/// Installs the fake agent `name` in `dir`: this crate's binary under the name the PATH looks for,
/// with `<name>.fake.json` next to it holding the behaviour.
fn install_fake(dir: &Path, name: &str, spec: &Value) {
    std::fs::create_dir_all(dir).expect("mkdir");
    let exe = dir.join(exe_name(name));
    std::fs::copy(FAKE_AGENT, &exe).expect("copy the fake agent");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    std::fs::write(exe.with_extension("fake.json"), spec.to_string()).expect("fake agent behaviour");
}

/// Minimal HTTP client (allows forging the Host header).
fn http(port: u16, method: &str, path: &str, host: &str, token: &str, body: &str) -> (u16, Value) {
    http_with_token_header(port, "x-agentdeck-token", method, path, host, token, body)
}

/// Like [`http`], with the token header name of your choice.
fn http_with_token_header(port: u16, token_header: &str, method: &str, path: &str, host: &str, token: &str, body: &str) -> (u16, Value) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(60))).expect("timeout");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\n{token_header}: {token}\r\ncontent-type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).expect("write");
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).expect("read");
    let text = String::from_utf8_lossy(&raw);
    let status: u16 = text.split_whitespace().nth(1).and_then(|c| c.parse().ok()).expect("status");
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
    (status, serde_json::from_str(body).unwrap_or(Value::Null))
}

/// Bridge call that can go to another thread (only port and token).
fn ask_raw(port: u16, token: &str, body: Value) -> (u16, Value) {
    http(port, "POST", "/ask", &format!("127.0.0.1:{port}"), token, &body.to_string())
}

fn ask(env: &Env, body: Value) -> (u16, Value) {
    let port = env.core.bridge_port();
    http(port, "POST", "/ask", &format!("127.0.0.1:{port}"), env.core.bridge_token(), &body.to_string())
}

/// The old name's header (`x-commanddeck-token`) no longer works: only `x-agentdeck-token` authenticates.
#[test]
fn the_old_token_header_is_refused() {
    let env = start(&[("claude", says("ok"))]);
    let port = env.core.bridge_port();
    let token = env.core.bridge_token().to_owned();
    let host = format!("127.0.0.1:{port}");

    let (st, b) = http_with_token_header(port, "x-commanddeck-token", "GET", "/agents", &host, &token, "");
    assert_eq!((st, b), (403, json!({"error": "invalid token"})));
    assert_eq!(http(port, "GET", "/agents", &host, &token, "").0, 200);
}

/// Waits for `CoreEvent::Activate` (the other events that arrive first are ignored).
fn got_activate(env: &Env) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
        match env.rx.recv_timeout(left) {
            Ok(CoreEvent::Activate) => return true,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
    false
}

/// `/activate` (the app's second launch) only works with the master token: without a token or with an
/// agent's, the user's window does not come to the front.
#[test]
fn activate_needs_the_master_token() {
    let env = start(&[("claude", says("ok"))]);
    let port = env.core.bridge_port();
    let host = format!("127.0.0.1:{port}");

    assert_eq!(http(port, "POST", "/activate", &host, "", "").0, 403, "no token");
    assert_eq!(http(port, "POST", "/activate", &host, "wrong", "").0, 403, "token errado");
    assert_eq!(http(port, "POST", "/activate", &host, &tab_token(&env, "claude"), "").0, 403, "agent token");
    assert!(!got_activate(&env), "none of the refused ones asks for the window");

    assert_eq!(http(port, "POST", "/activate", &host, env.core.bridge_token(), "").0, 200);
    assert!(got_activate(&env));
    assert_eq!(http(port, "GET", "/activate", &host, env.core.bridge_token(), "").0, 404, "POST only");
}

/// The second instance finds the first one's bridge through the file next to the lock and makes it show the window.
#[test]
fn a_second_launch_activates_the_running_instance() {
    let env = start(&[("claude", says("ok"))]);
    let data_dir = paths_in(&env.root).data_dir;
    assert!(agentdeck_core::activation::activate_running(&data_dir));
    assert!(got_activate(&env));

    // Once shut down, it releases the lock and deletes the file: there is no one left to ask.
    env.core.shutdown();
    let Env { core, _tmp, .. } = env;
    drop(core);
    assert!(!agentdeck_core::activation::activate_running(&data_dir));
}

#[test]
fn auth_and_validation() {
    let env = start(&[("claude", says("ok"))]);
    let port = env.core.bridge_port();
    let token = env.core.bridge_token().to_owned();
    assert_eq!(token.len(), 32);
    assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    let host = format!("127.0.0.1:{port}");

    let (st, b) = http(port, "GET", "/agents", &host, "wrong", "");
    assert_eq!((st, b), (403, json!({"error": "invalid token"})));
    let (st, b) = http(port, "GET", "/agents", "evil.example:80", &token, "");
    assert_eq!((st, b), (403, json!({"error": "invalid host"})));
    let (st, _) = http(port, "GET", "/agents", &format!("localhost:{port}"), &token, "");
    assert_eq!(st, 200);
    let (st, b) = http(port, "GET", "/nothing", &host, &token, "");
    assert_eq!((st, b), (404, json!({"error": "unknown route"})));
    let (st, b) = http(port, "POST", "/ask", &host, &token, "{oops");
    assert_eq!((st, b), (400, json!({"error": "invalid JSON"})));

    let (st, b) = http(port, "GET", "/agents", &host, &token, "");
    assert_eq!(st, 200);
    let agents = b["agents"].as_array().expect("list").clone();
    assert_eq!(agents, vec![json!("claude")]);

    let (st, b) = ask(&env, json!({"from": "shell", "to": "bogus", "message": "oi"}));
    assert_eq!(st, 400);
    let err = b["error"].as_str().expect("error");
    assert!(err.starts_with("unknown or not installed agent \"bogus\". Available: "), "{err}");

    let (st, b) = ask(&env, json!({"from": "claude", "to": "ClaudeCode", "message": "oi"}));
    assert_eq!((st, b), (400, json!({"error": "an agent cannot ask itself"})));
    let (st, b) = ask(&env, json!({"from": "shell", "to": "claude", "message": "oi", "depth": 2}));
    assert_eq!((st, b), (400, json!({"error": "maximum conversation depth between agents reached"})));
    env.core.shutdown();
}

#[test]
fn workspace_agents_md_and_spawn_specs() {
    let env = start(&[("claude", says("ok"))]);
    let p = env.core.active_project_id();
    let ws = env.core.workspace();
    assert_eq!(ws, env.root.join("workspace"));
    let md = std::fs::read_to_string(ws.join("AGENTS.md")).expect("AGENTS.md created in the app's workspace");
    assert!(md.starts_with("<!-- agentdeck:start -->\nYou are running inside AgentDeck"));
    assert!(md.contains("Your name and role are in the AGENTDECK_AGENT and AGENTDECK_ROLE variables"));
    assert!(md.contains("do NOT use your CLI's internal subagents"));

    let shell = env.core.spawn_spec(&p, "shell").expect("shell");
    assert_eq!(shell.label, "Terminal");
    assert_eq!(shell.cwd, ws);
    assert_eq!(shell.env["AGENTDECK_AGENT"], "shell");
    assert_eq!(shell.env["AGENTDECK_PORT"], env.core.bridge_port().to_string());
    // Each tab has its own token (the master one does not go to the agents), stable while it exists.
    let token = shell.env["AGENTDECK_TOKEN"].clone();
    assert_ne!(token, env.core.bridge_token());
    assert_eq!(token.len(), 32);
    assert_eq!(env.core.spawn_spec(&p, "shell").expect("again").env["AGENTDECK_TOKEN"], token);
    assert!(shell.env["PATH"].starts_with(&env.root.join("bin").display().to_string()));
    assert!(!shell.double_enter_on_mention);

    assert_eq!(env.core.spawn_spec(&p, "nada").expect_err("unknown"), "Unknown agent: nada");
    let missing = env.core.spawn_spec(&p, "antigravity").expect_err("not installed");
    assert!(missing.starts_with("Antigravity CLI (agy) not found."));
    assert_eq!(env.core.spawn_spec(&p, "ollama").expect_err("the Ollama agent was removed"), "Unknown agent: ollama");
    assert_eq!(env.core.spawn_spec("pdeadbeef", "shell").expect_err("no project"), "Unknown project: pdeadbeef");

    // Another project (not active): the terminal opens in its folder, which gets the AGENTS.md on the first tab.
    let other_dir = env.root.join("other");
    let other = env.core.create_project(&other_dir, None);
    assert_eq!(env.core.active_project_id(), p, "creating a project does not switch the active one");
    let spec = env.core.spawn_spec(&other.id, "shell").expect("shell");
    assert_eq!(spec.cwd, other_dir);
    assert_ne!(spec.env["AGENTDECK_TOKEN"], token, "each project's shell has its own token");
    assert!(std::fs::read_to_string(other_dir.join("AGENTS.md")).expect("AGENTS.md of the other project").contains("AGENTDECK_AGENT"));
    assert_eq!(env.core.workspace(), ws);

    let claude = env.core.spawn_spec(&p, "claude").expect("claude");
    assert_eq!(claude.program, env.root.join("fake-bin").join(exe_name("claude")));
    assert_eq!(claude.args[0], "--append-system-prompt");
    assert!(claude.args[1].contains("Your name here is \"claude\"."));
    let hooks = env.root.join("dados").join("hooks").join("claude-settings.json");
    assert_eq!(
        &claude.args[2..],
        [
            "--allowedTools".to_owned(),
            "Bash(ask-agent:*)".to_owned(),
            "--settings".to_owned(),
            hooks.display().to_string(),
            "--add-dir".to_owned(),
            env.root.join("dados").display().to_string()
        ]
    );
    assert!(std::fs::read_to_string(&hooks).expect("hooks written").contains("ask-agent --signal idle"));
    assert!(claude.double_enter_on_mention);
    assert_eq!(claude.label, "Claude");

    let list = env.core.list_agents();
    let ids: Vec<&str> = list.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, ["claude", "antigravity", "codex", "opencode", "aider", "goose", "copilot", "q", "interpreter", "shell"]);
    let shell = list.last().expect("shell");
    assert!(shell.installed && !shell.peer);
    env.core.shutdown();
}

#[test]
fn ask_runs_headless_with_sessions() {
    let env = start(&[("claude", echoing_agent())]);
    let (st, b) = ask(&env, json!({"from": "shell", "to": "claude", "message": "What is the capital?"}));
    assert_eq!(st, 200, "{b}");
    let answer = b["answer"].as_str().expect("answer");
    assert!(answer.starts_with("ARGS: [-p] [--output-format] [text] [--session-id] ["), "{answer}");
    assert!(answer.contains("[--append-system-prompt] [<sys>] [--add-dir]"), "{answer}");
    assert!(answer.contains("ENV: claude 1"), "{answer}");
    assert!(answer.contains(
        "[Message from the agent \"shell\" received by AgentDeck]\n\nWhat is the capital?\n\nAnswer the agent \"shell\" directly. The user follows the conversation in the Conversation tab in real time."
    ));
    let sid = answer.split("[--session-id] [").nth(1).and_then(|s| s.split(']').next()).expect("sid").to_owned();

    // A second message from the same pair resumes the session.
    let (st, b) = ask(&env, json!({"from": "shell", "to": "claude", "message": "And now?"}));
    assert_eq!(st, 200);
    assert!(b["answer"].as_str().expect("answer").contains(&format!("[--resume] [{sid}]")));

    // Feed: ask/answer.
    let feed: Vec<_> = env
        .rx
        .try_iter()
        .filter_map(|e| match e {
            CoreEvent::Feed(f) => Some(f),
            _ => None,
        })
        .collect();
    assert_eq!(feed[0].kind, FeedKind::Ask);
    assert_eq!(
        (feed[0].from.as_deref(), feed[0].to.as_deref(), feed[0].text.as_str()),
        (Some("shell"), Some("claude"), "What is the capital?")
    );
    assert_eq!(feed[1].kind, FeedKind::Answer);
    assert_eq!((feed[1].from.as_deref(), feed[1].to.as_deref()), (Some("claude"), Some("shell")));
    let project = env.core.active_project_id();
    assert!(
        feed.iter().all(|f| f.project.as_deref() == Some(project.as_str())),
        "the conversation belongs to the project of whoever asked"
    );
    env.core.shutdown();
}

#[test]
fn failing_resumed_session_retries_and_errors_report_tail() {
    // Always fails when it receives --resume; otherwise it answers. Errors go to stderr.
    let claude = json!({"fail_on_arg": "--resume", "stderr": "session expired", "stdout": "new"});
    let aider = json!({"stderr": "l1\nl2", "exit": 3});
    let env = start(&[("claude", claude), ("aider", aider)]);
    assert_eq!(ask(&env, json!({"from": "shell", "to": "claude", "message": "a"})).1["answer"], "new");
    // The session "expires": the bridge discards it and retries cleanly.
    assert_eq!(ask(&env, json!({"from": "shell", "to": "claude", "message": "b"})).1["answer"], "new");

    let (st, b) = ask(&env, json!({"from": "shell", "to": "aider", "message": "x"}));
    assert_eq!((st, b), (500, json!({"error": "l1\nl2"})));
    env.core.shutdown();
}

#[test]
fn same_pair_requests_are_serialized() {
    let tmp_log = tempfile::NamedTempFile::new().expect("tmp");
    let log = tmp_log.path().to_string_lossy().into_owned();
    let script = json!({"log": log, "log_first": "start", "sleep_ms": 300, "log_last": "end", "stdout": "ok"});
    let env = std::sync::Arc::new(start(&[("claude", script)]));
    let mut hs = Vec::new();
    for i in 0..3 {
        let env = env.clone();
        hs.push(std::thread::spawn(move || ask(&env, json!({"from": "shell", "to": "claude", "message": format!("m{i}")})).0));
    }
    for h in hs {
        assert_eq!(h.join().expect("thread"), 200);
    }
    let text = std::fs::read_to_string(tmp_log.path()).expect("log");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines, ["start", "end", "start", "end", "start", "end"], "overlapping runs");
    env.core.shutdown();
}

#[test]
fn attachments_and_config_persistence() {
    let env = start(&[]);
    let r = env.core.attach_bytes("note é.txt", b"x").expect("attachment");
    assert!(r.contains(&format!("dados/anexos/s_{}_", env.core.session_id())), "{r}");
    assert!(r.ends_with("-note_e.txt"));
    env.core.update_config(|c| c.whisper_lang = "en".into());
    let saved = std::fs::read_to_string(env.root.join("cfg").join("config.json")).expect("saved config");
    assert!(saved.contains("\"whisperLang\": \"en\""));
    // Session recorded.
    let sessions = std::fs::read_to_string(env.root.join("dados").join("sessions.json")).expect("sessions.json");
    assert!(sessions.contains(env.core.session_id()));
    assert_eq!(env.core.transcribe_engine(), "unavailable");
    agentdeck_core::i18n::with_lang(agentdeck_core::i18n::Lang::PtBr, || {
        assert_eq!(env.core.transcribe_engine(), "indisponível");
    });
    assert!(env.core.transcribe_pcm16k(&[0.0; 160]).is_err());
    env.core.shutdown();
}

#[test]
fn spawn_spec_for_an_instance_carries_its_identity() {
    let env = start(&[]);
    let p = env.core.active_project_id();
    let ident = agentdeck_core::Ident::parse("shell@qa@ana").expect("id");
    let spec = env.core.spawn_spec_for(&p, &ident).expect("shell always exists");
    assert_eq!(spec.kind, "shell");
    assert_eq!(spec.env["AGENTDECK_AGENT"], "shell@qa@ana");
    assert_eq!(spec.cwd, env.root.join("workspace"));
    // `shell@main@ana` has no role: no AGENTDECK_ROLE.
    let main = env.core.spawn_spec_for(&p, &agentdeck_core::Ident::parse("shell@main@bia").expect("id")).expect("spec");
    assert_eq!(main.env["AGENTDECK_AGENT"], "shell@main@bia");
    assert!(!main.env.contains_key("AGENTDECK_ROLE"));
    env.core.shutdown();
}

#[test]
fn instances_are_registered_named_and_forgotten() {
    let env = start(&[]);
    let p = env.core.active_project_id();
    assert_eq!(env.core.register_instance(&p, "claude").id(), "claude");
    assert_eq!(env.core.register_instance(&p, "claude").id(), "claude@main@ana");
    assert_eq!(env.core.restore_instance(&p, "claude@dev").map(|i| i.id()).as_deref(), Some("claude@dev"));
    assert!(env.core.restore_instance(&p, "claude@Dev!").is_none());
    assert_eq!(env.core.project_instance_ids(&p), vec!["claude", "claude@main@ana", "claude@dev"]);
    env.core.close_instance(&p, "claude@main@ana");
    assert_eq!(env.core.register_instance(&p, "claude").id(), "claude@main@ana", "the name is reused");
    env.core.shutdown();
}

#[test]
fn two_projects_each_have_their_own_claude() {
    let env = start(&[]);
    fake_claude(&env);
    let a = env.core.active_project_id();
    let b = env.core.create_project(&env.root.join("b"), Some("Project B")).id;
    assert_ne!(a, b);
    assert_eq!(env.core.create_project(&env.root.join("b"), None).id, b, "the same folder is the same project");
    // Each project has its own `claude`, without an extra name.
    assert_eq!(env.core.register_instance(&a, "claude").id(), "claude");
    assert_eq!(env.core.register_instance(&b, "claude").id(), "claude");
    env.core.restore_instance(&a, "claude@qa");
    let (ta, tb) = (tab_token_in(&env, &a, "claude"), tab_token_in(&env, &b, "claude"));
    assert_ne!(ta, tb, "one token per project");
    assert_eq!(tab_token_in(&env, &b, "claude"), tb, "stable while the instance exists");
    // `instance_ids` joins every project, without repeats (usage only looks at the model).
    assert_eq!(env.core.instance_ids(), ["claude", "claude@qa"]);
    assert!(env.core.peers(&a).contains(&"claude@qa".to_owned()));
    assert!(!env.core.peers(&b).contains(&"claude@qa".to_owned()), "B does not see A's instances");

    let port = env.core.bridge_port();
    let host = format!("127.0.0.1:{port}");
    // /agents and /signal in the token's project.
    let agents = |token: &str| http(port, "GET", "/agents", &host, token, "").1["agents"].clone();
    assert!(agents(&ta).as_array().expect("list").contains(&json!("claude@qa")));
    assert!(!agents(&tb).as_array().expect("list").contains(&json!("claude@qa")));
    let _ = env.rx.try_iter().count();
    assert_eq!(http(port, "POST", "/signal", &host, &tb, &json!({"state": "busy"}).to_string()).0, 200);
    let signals: Vec<(String, String)> = env
        .rx
        .try_iter()
        .filter_map(|e| match e {
            CoreEvent::AgentSignal { project, id, .. } => Some((project, id)),
            _ => None,
        })
        .collect();
    assert_eq!(signals, [(b.clone(), "claude".to_owned())]);

    // Closing A's claude does not touch B's.
    env.core.close_instance(&a, "claude");
    assert_eq!(http(port, "GET", "/agents", &host, &ta, "").0, 403);
    assert_eq!(http(port, "GET", "/agents", &host, &tb, "").0, 200);
    assert_eq!(env.core.project_instance_ids(&b), ["claude"]);
    env.core.shutdown();
}

#[test]
fn a_role_asked_in_one_project_opens_there_and_cycles_do_not_cross_projects() {
    let env = start(&[]);
    fake_claude(&env);
    env.core.enable_tab_delivery();
    let a = env.core.active_project_id();
    let b = env.core.create_project(&env.root.join("b"), None).id;
    env.core.register_instance(&a, "claude"); // the main Claude tab in A
    let port = env.core.bridge_port();
    let ask_as = |token: String, to: &'static str| std::thread::spawn(move || ask_raw(port, &token, json!({"to": to, "message": "oi"})));

    // A: claude → claude@dev (creates claude@dev in A) → claude@qa.
    let first = ask_as(tab_token_in(&env, &a, "claude"), "claude@dev");
    let d = next_delivery(&env);
    assert_eq!((d.project.as_str(), d.ident.id().as_str(), d.create, d.from.as_str()), (a.as_str(), "claude@dev", true, "claude"));
    assert!(env.core.project_instance_ids(&a).contains(&"claude@dev".to_owned()));
    assert!(!env.core.project_instance_ids(&b).contains(&"claude@dev".to_owned()), "B does not see A's claude@dev");
    assert!(!env.core.peers(&b).contains(&"claude@dev".to_owned()));
    let second = ask_as(tab_token_in(&env, &a, "claude@dev"), "claude@qa");
    assert_eq!(next_delivery(&env).ident.id(), "claude@qa");

    // B's claude@dev does not answer in place of A's.
    let host = format!("127.0.0.1:{port}");
    let forged = json!({"to": "", "message": "x", "reply_to": d.request}).to_string();
    assert_eq!(http(port, "POST", "/ask", &host, &tab_token_in(&env, &b, "claude@dev"), &forged).0, 404);

    // In A, claude@qa asking claude would close the cycle (409)…
    let (st, body) = ask_raw(port, &tab_token_in(&env, &a, "claude@qa"), json!({"to": "claude", "message": "oi"}));
    assert_eq!(st, 409, "{body}");
    assert_eq!(body["error"], "\"claude\" is already waiting for an answer from \"claude@qa\" (wait cycle)");
    // …and asking another already exceeds the maximum depth.
    let (st, body) = ask_raw(port, &tab_token_in(&env, &a, "claude@qa"), json!({"to": "claude@x", "message": "oi"}));
    assert_eq!((st, body), (400, json!({"error": "maximum conversation depth between agents reached"})));
    // In B, the same question is neither a cycle nor too deep: it goes to B's claude (created there).
    let third = ask_as(tab_token_in(&env, &b, "claude@qa"), "claude");
    let d = next_delivery(&env);
    assert_eq!((d.project.as_str(), d.ident.id().as_str(), d.create), (b.as_str(), "claude", true));

    env.core.shutdown();
    for t in [first, second, third] {
        let _ = t.join();
    }
}

#[test]
fn the_master_token_picks_the_project_from_the_body() {
    let env = start(&[]);
    fake_claude(&env);
    env.core.enable_tab_delivery();
    let b = env.core.create_project(&env.root.join("b"), None).id;
    let port = env.core.bridge_port();
    let master = env.core.bridge_token().to_owned();
    let (st, body) = ask_raw(port, &master, json!({"from": "shell", "to": "claude", "message": "oi", "project": "pdeadbeef"}));
    assert_eq!((st, body), (400, json!({"error": "unknown project: \"pdeadbeef\""})));
    let asking = std::thread::spawn({
        let b = b.clone();
        move || ask_raw(port, &master, json!({"from": "shell", "to": "claude", "message": "oi", "project": b}))
    });
    let d = next_delivery(&env);
    assert_eq!((d.project.as_str(), d.ident.id().as_str()), (b.as_str(), "claude"));
    env.core.fail_delivery(d.request, "test");
    assert_eq!(asking.join().expect("thread").0, 500);
    env.core.shutdown();
}

#[test]
fn removing_a_project_closes_its_instances() {
    let env = start(&[]);
    let a = env.core.active_project_id();
    let b = env.core.create_project(&env.root.join("b"), None).id;
    let tb = tab_token_in(&env, &b, "shell");
    env.core.restore_instance(&b, "shell");
    let port = env.core.bridge_port();
    let host = format!("127.0.0.1:{port}");
    assert!(env.core.rename_project(&b, "New name"));
    assert!(!env.core.rename_project(&b, "  "));
    assert_eq!(env.core.projects().iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["workspace", "New name"]);
    assert!(env.core.update_project(&b, |p| {
        p.open_tabs = vec!["shell".into()];
        p.id = "other".into();
    }));
    assert_eq!(env.core.projects()[1].open_tabs, ["shell"]);
    assert_eq!(env.core.projects()[1].id, b, "the id does not change");

    assert!(env.core.remove_project(&b));
    assert!(!env.core.remove_project(&b));
    assert_eq!(http(port, "GET", "/agents", &host, &tb, "").0, 403, "the tabs' token of the removed project dies");
    assert!(env.core.project_instance_ids(&b).is_empty());
    assert_eq!(env.core.active_project_id(), a);
    assert!(env.root.join("b").is_dir(), "the folder is not deleted");

    // With none active, none is left: empty workspace.
    assert!(env.core.remove_project(&a));
    assert!(env.core.active_project().is_none());
    assert_eq!(env.core.workspace(), std::path::PathBuf::new());
    assert!(env.core.read_dir("").is_empty());
    // Creating a project with none active activates it.
    let c = env.core.create_project(&env.root.join("c"), None);
    assert_eq!(env.core.active_project_id(), c.id);
    assert_eq!(env.core.workspace(), env.root.join("c"));
    // set_workspace is a shortcut: it creates the folder's project and activates it.
    env.core.set_workspace(env.root.join("d")).expect("set_workspace");
    assert_eq!(env.core.active_project().expect("ativo").path, env.root.join("d"));
    assert_eq!(env.core.projects().len(), 2);
    env.core.shutdown();
}

/// Next delivery requested from the interface (ignores the other events).
fn next_delivery(env: &Env) -> agentdeck_core::Delivery {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match env.rx.recv_timeout(left) {
            Ok(CoreEvent::Deliver(d)) => return d,
            Ok(_) => continue,
            Err(_) => panic!("no delivery requested from the interface"),
        }
    }
}

#[test]
fn a_question_is_typed_into_the_tab_and_answered_by_callback() {
    let env = start(&[("claude", says("headless"))]);
    env.core.enable_tab_delivery();
    let (port, token) = (env.core.bridge_port(), env.core.bridge_token().to_owned());

    let asker = std::thread::spawn({
        let token = token.clone();
        move || ask_raw(port, &token, json!({"from": "shell", "to": "claude", "message": "what is the capital of France?"}))
    });
    let d = next_delivery(&env);
    assert_eq!((d.ident.id().as_str(), d.create, d.from.as_str()), ("claude", true, "shell"));
    assert!(d.prompt.contains("what is the capital of France?"));
    assert!(d.prompt.contains(&format!("ask-agent --reply {} ", d.request)), "teaches how to answer: {}", d.prompt);
    assert!(!d.answer);

    // The tab (claude) answers by calling `ask-agent shell "Paris"`: that completes the asker's question.
    let (st, b) = ask_raw(port, &token, json!({"from": "claude", "to": "shell", "message": "Paris"}));
    assert_eq!(st, 200);
    assert!(b["answer"].as_str().expect("text").contains("delivered"));
    let (st, b) = asker.join().expect("thread");
    assert_eq!((st, b["answer"].as_str()), (200, Some("Paris")));
    env.core.shutdown();
}

#[test]
fn a_role_opens_a_new_instance_and_new_forces_another() {
    let env = start(&[("claude", says("headless"))]);
    let p = env.core.active_project_id();
    env.core.enable_tab_delivery();
    let (port, token) = (env.core.bridge_port(), env.core.bridge_token().to_owned());
    let spawn_ask = |to: &'static str, new: bool| {
        let token = token.clone();
        std::thread::spawn(move || ask_raw(port, &token, json!({"from": "shell", "to": to, "message": "task", "new": new})))
    };

    let first = spawn_ask("claude@dev", false);
    let d = next_delivery(&env);
    assert_eq!((d.ident.id().as_str(), d.create), ("claude@dev", true));
    let second = spawn_ask("claude@dev", false);
    let d = next_delivery(&env);
    assert_eq!((d.ident.id().as_str(), d.create), ("claude@dev", false), "same role: reuses the tab");
    let third = spawn_ask("claude@dev", true);
    let d = next_delivery(&env);
    assert_eq!((d.ident.id().as_str(), d.create), ("claude@dev@ana", true), "--new: another instance, with a name");

    // When the tabs close, whoever was waiting gets the error (and no thread is left stuck).
    env.core.close_instance(&p, "claude@dev");
    env.core.close_instance(&p, "claude@dev@ana");
    for t in [first, second, third] {
        let (st, b) = t.join().expect("thread");
        assert_eq!(st, 500);
        assert!(b["error"].as_str().expect("error").contains("was closed"), "{b}");
    }
    env.core.shutdown();
}

#[test]
fn a_failed_delivery_unblocks_the_asker() {
    let env = start(&[("claude", says("headless"))]);
    env.core.enable_tab_delivery();
    let (port, token) = (env.core.bridge_port(), env.core.bridge_token().to_owned());
    let asker = std::thread::spawn(move || ask_raw(port, &token, json!({"from": "shell", "to": "claude", "message": "oi"})));
    let d = next_delivery(&env);
    env.core.fail_delivery(d.request, "the tab did not come up");
    let (st, b) = asker.join().expect("thread");
    assert_eq!((st, b["error"].as_str()), (500, Some("the tab did not come up")));
    env.core.shutdown();
}

#[test]
fn wait_cycles_and_unknown_or_ambiguous_targets_are_refused() {
    let env = start(&[("claude", says("x")), ("agy", says("x"))]);
    let p = env.core.active_project_id();
    env.core.enable_tab_delivery();
    env.core.register_instance(&p, "claude"); // the main Claude tab
    let (port, token) = (env.core.bridge_port(), env.core.bridge_token().to_owned());
    let call = |from: &'static str, to: &'static str| {
        let token = token.clone();
        std::thread::spawn(move || ask_raw(port, &token, json!({"from": from, "to": to, "message": "oi"})))
    };

    // claude waits for antigravity; antigravity waits for claude@dev; claude@dev waiting for claude would close the cycle.
    let a = call("claude", "antigravity");
    next_delivery(&env);
    let b = call("antigravity", "claude@dev");
    next_delivery(&env);
    let (st, body) = ask_raw(port, &token, json!({"from": "claude@dev", "to": "claude", "message": "oi"}));
    assert_eq!(st, 409, "{body}");
    assert!(body["error"].as_str().expect("error").contains("cycle"));

    // Unknown target and ambiguous instance.
    assert_eq!(ask_raw(port, &token, json!({"from": "shell", "to": "bogus", "message": "oi"})).0, 400);
    env.core.restore_instance(&p, "claude@qa@ana");
    env.core.restore_instance(&p, "claude@qa@bia");
    let (st, body) = ask_raw(port, &token, json!({"from": "shell", "to": "claude@qa", "message": "oi"}));
    assert_eq!(st, 400);
    assert!(body["error"].as_str().expect("error").contains("ambiguous"), "{body}");

    env.core.shutdown();
    for t in [a, b] {
        let _ = t.join();
    }
}

#[test]
fn without_tab_delivery_the_bridge_still_answers_headless() {
    // The interface did not turn delivery on (CLI/tests) or the user turned the option off: headless mode.
    let env = start(&[("claude", says("headless-answer"))]);
    let (st, b) = ask(&env, json!({"from": "shell", "to": "claude", "message": "oi"}));
    assert_eq!((st, b["answer"].as_str()), (200, Some("headless-answer")));

    env.core.enable_tab_delivery();
    env.core.update_config(|c| c.auto_open_agent_tabs = false);
    let (st, b) = ask(&env, json!({"from": "shell", "to": "claude", "message": "again"}));
    assert_eq!((st, b["answer"].as_str()), (200, Some("headless-answer")));
    assert!(env.rx.try_iter().all(|e| !matches!(e, CoreEvent::Deliver(_))));
    env.core.shutdown();
}

#[test]
fn reply_route_only_completes_pending_questions() {
    let env = start(&[("claude", says("x"))]);
    env.core.enable_tab_delivery();
    let (port, token) = (env.core.bridge_port(), env.core.bridge_token().to_owned());
    let host = format!("127.0.0.1:{port}");

    // Without a pending question, /reply creates nothing: 404.
    let body = json!({"from": "claude", "to": "shell", "message": "alone"}).to_string();
    assert_eq!(http(port, "POST", "/reply", &host, &token, &body).0, 404);
    assert!(env.rx.try_iter().all(|e| !matches!(e, CoreEvent::Deliver(_))));

    let asker = std::thread::spawn({
        let token = token.clone();
        move || ask_raw(port, &token, json!({"from": "shell", "to": "claude", "message": "oi"}))
    });
    next_delivery(&env);
    let (st, b) = http(port, "POST", "/reply", &host, &token, &body);
    assert_eq!((st, b["delivered"].as_bool()), (200, Some(true)));
    let (st, b) = asker.join().expect("thread");
    assert_eq!((st, b["answer"].as_str()), (200, Some("alone")));
    // The second attempt (the model had already answered) does not deliver again.
    assert_eq!(http(port, "POST", "/reply", &host, &token, &body).0, 404);
    env.core.shutdown();
}

#[test]
fn the_token_names_the_caller_and_the_body_cannot_forge_it() {
    let env = start(&[]);
    let p = env.core.active_project_id();
    let port = env.core.bridge_port();
    let host = format!("127.0.0.1:{port}");
    let qa = tab_token(&env, "shell@qa");
    // With a tab's token, the body's `from` is ignored: the tab is the one speaking.
    let _ = env.rx.try_iter().count();
    let body = json!({"from": "claude", "state": "busy"}).to_string();
    assert_eq!(http(port, "POST", "/signal", &host, &qa, &body).0, 200);
    let who: Vec<String> = env
        .rx
        .try_iter()
        .filter_map(|e| match e {
            CoreEvent::AgentSignal { id, .. } => Some(id),
            _ => None,
        })
        .collect();
    assert_eq!(who, ["shell@qa"]);
    // With the master token, `from` must be a valid id.
    let body = json!({"from": "x\u{1b}[201~", "to": "claude", "message": "oi"}).to_string();
    let (st, b) = http(port, "POST", "/ask", &host, env.core.bridge_token(), &body);
    assert_eq!(st, 400);
    assert!(b["error"].as_str().expect("error").starts_with("invalid sender"), "{b}");
    // Closing the tab invalidates its token.
    env.core.close_instance(&p, "shell@qa");
    assert_eq!(http(port, "GET", "/agents", &host, &qa, "").0, 403);
    env.core.shutdown();
}

#[test]
fn requests_without_host_or_too_big_are_refused() {
    let env = start(&[]);
    let port = env.core.bridge_port();
    let token = env.core.bridge_token().to_owned();
    assert_eq!(http(port, "GET", "/agents", "", &token, ""), (403, json!({"error": "invalid host"})));
    let big = "x".repeat((16 << 20) + 1);
    let (st, _) = http(port, "POST", "/ask", &format!("127.0.0.1:{port}"), &token, &big);
    assert_eq!(st, 413);
    env.core.shutdown();
}

#[test]
fn hooks_signal_turns_of_a_tab() {
    let env = start(&[]);
    let port = env.core.bridge_port();
    let host = format!("127.0.0.1:{port}");
    let token = tab_token(&env, "shell@dev");
    let _ = env.rx.try_iter().count();
    let (st, _) = http(port, "POST", "/signal", &host, &token, &json!({"state": "busy"}).to_string());
    assert_eq!(st, 200);
    let (st, _) = http(port, "POST", "/signal", &host, &token, &json!({"state": "idle"}).to_string());
    assert_eq!(st, 200);
    let signals: Vec<(String, bool)> = env
        .rx
        .try_iter()
        .filter_map(|e| match e {
            CoreEvent::AgentSignal { id, busy, .. } => Some((id, busy)),
            _ => None,
        })
        .collect();
    assert_eq!(signals, [("shell@dev".to_owned(), true), ("shell@dev".to_owned(), false)]);
    // Invalid state, or not a tab (master token): refused.
    assert_eq!(http(port, "POST", "/signal", &host, &token, &json!({"state": "x"}).to_string()).0, 400);
    assert_eq!(http(port, "POST", "/signal", &host, env.core.bridge_token(), &json!({"state": "idle"}).to_string()).0, 400);
    env.core.shutdown();
}

/// Makes `claude` "installed" for the core (the file only needs to exist; it never runs).
fn fake_claude(env: &Env) {
    fake_cli(env, "claude");
}

/// Makes the agent `name` (`claude`, `codex`, `opencode`, `agy`…) "installed" for the core. The
/// behaviour is empty: these tests never run it.
fn fake_cli(env: &Env, name: &str) {
    install_fake(&env.root.join("fake-bin"), name, &json!({}));
}

#[test]
fn statusline_reports_usage_of_the_tab_model() {
    let env = start(&[]);
    fake_claude(&env);
    let port = env.core.bridge_port();
    let host = format!("127.0.0.1:{port}");
    let token = tab_token(&env, "claude@dev");
    let _ = env.rx.try_iter().count();
    let body = json!({"session": {"used": 0.34, "resets_at_ms": 1_760_000_000_000u64}, "weekly": {"used": 1.0, "resets_at_ms": null}});
    assert_eq!(http(port, "POST", "/usage", &host, &token, &body.to_string()), (200, json!({"ok": true})));
    // The model comes from the token (`claude@dev` → `claude`), not from the body.
    let usage = env.core.usage_snapshot();
    let claude = &usage["claude"];
    assert_eq!(claude.session, Some(agentdeck_core::UsageWindow { used: 0.34, resets_at_ms: Some(1_760_000_000_000) }));
    assert_eq!(claude.weekly, Some(agentdeck_core::UsageWindow { used: 1.0, resets_at_ms: None }));
    assert!(claude.updated_ms > 0 && !claude.local);
    assert!(env.rx.try_iter().any(|e| matches!(e, CoreEvent::UsageChanged)));
    // The same usage again does not wake the UI.
    assert_eq!(http(port, "POST", "/usage", &host, &token, &body.to_string()).0, 200);
    assert!(!env.rx.try_iter().any(|e| matches!(e, CoreEvent::UsageChanged)));

    // Out of 0..=1, not a tab (master token) or in a tab without a plan (shell): refused.
    let bad = json!({"session": {"used": 34}}).to_string();
    assert_eq!(http(port, "POST", "/usage", &host, &token, &bad).0, 400);
    assert_eq!(http(port, "POST", "/usage", &host, env.core.bridge_token(), &body.to_string()).0, 400);
    assert_eq!(http(port, "POST", "/usage", &host, &tab_token(&env, "shell@dev"), &body.to_string()).0, 400);
    assert_eq!(env.core.usage_snapshot().len(), 1);
    env.core.shutdown();
}

#[test]
fn one_app_per_data_dir() {
    let env = start(&[]);
    let second = Core::start_with(paths_in(&env.root), StartOptions { watch_workspace: false, system_dirs: false, ..Default::default() });
    let err = second.err().expect("the second instance in the same data folder is refused").to_string();
    assert!(err.contains("is already using the data folder"), "{err}");
    env.core.shutdown();
}

#[test]
fn headless_runs_get_their_own_short_lived_token() {
    let env = start(&[("claude", json!({ "print_env": "AGENTDECK_TOKEN" }))]);
    let (st, b) = ask(&env, json!({"from": "shell", "to": "claude", "message": "oi"}));
    assert_eq!(st, 200, "{b}");
    let token = b["answer"].as_str().expect("token").trim().to_owned();
    assert_eq!(token.len(), 32);
    assert_ne!(token, env.core.bridge_token());
    let port = env.core.bridge_port();
    assert_eq!(http(port, "GET", "/agents", &format!("127.0.0.1:{port}"), &token, "").0, 403, "token forgotten when finished");
    env.core.shutdown();
}

#[test]
fn only_the_asked_agent_can_answer_a_question() {
    let env = start(&[("claude", says("x")), ("agy", says("x"))]);
    env.core.enable_tab_delivery();
    let port = env.core.bridge_port();
    let host = format!("127.0.0.1:{port}");
    let master = env.core.bridge_token().to_owned();
    let asker = std::thread::spawn(move || ask_raw(port, &master, json!({"from": "shell", "to": "claude", "message": "oi"})));
    let d = next_delivery(&env);
    // Another tab trying to answer in place of claude (by the number or by the pair): it does not deliver.
    let agy = tab_token(&env, "antigravity");
    let forged = json!({"to": "shell", "message": "forged", "reply_to": d.request, "from": "claude"}).to_string();
    let (st, b) = http(port, "POST", "/ask", &host, &agy, &forged);
    assert_eq!(st, 404, "{b}");
    assert_eq!(http(port, "POST", "/reply", &host, &agy, &forged).0, 404);
    // claude answers by the question number.
    let claude = tab_token(&env, "claude");
    let body = json!({"to": "", "message": "Paris", "reply_to": d.request}).to_string();
    let (st, b) = http(port, "POST", "/ask", &host, &claude, &body);
    assert_eq!((st, b), (200, json!({"answer": "Answer delivered to \"shell\"."})));
    let (st, b) = asker.join().expect("thread");
    assert_eq!((st, b["answer"].as_str()), (200, Some("Paris")));
    env.core.shutdown();
}

#[test]
fn the_depth_limit_also_holds_between_tabs() {
    let env = start(&[("claude", says("x")), ("agy", says("x"))]);
    env.core.enable_tab_delivery();
    let port = env.core.bridge_port();
    let host = format!("127.0.0.1:{port}");
    let master = env.core.bridge_token().to_owned();
    // shell → claude (depth 0), claude → claude@dev (1), claude@dev → antigravity (2: refused).
    let first = std::thread::spawn(move || ask_raw(port, &master, json!({"from": "shell", "to": "claude", "message": "a"})));
    next_delivery(&env);
    let claude = tab_token(&env, "claude");
    let second = std::thread::spawn({
        let claude = claude.clone();
        move || http(port, "POST", "/ask", &format!("127.0.0.1:{port}"), &claude, &json!({"to": "claude@dev", "message": "b"}).to_string())
    });
    next_delivery(&env);
    let dev = tab_token(&env, "claude@dev");
    let (st, b) = http(port, "POST", "/ask", &host, &dev, &json!({"to": "antigravity", "message": "c"}).to_string());
    assert_eq!((st, b), (400, json!({"error": "maximum conversation depth between agents reached"})));
    env.core.shutdown();
    let _ = (first.join(), second.join());
}

#[test]
fn a_slow_answer_is_typed_later_into_the_askers_tab() {
    let env = start(&[("claude", says("x"))]);
    env.core.enable_tab_delivery();
    let port = env.core.bridge_port();
    let host = format!("127.0.0.1:{port}");
    let claude = tab_token(&env, "claude");
    // claude's `ask-agent` accepts waiting 1 s: after that it returns (202) and the answer comes later.
    let t = std::time::Instant::now();
    let body = json!({"to": "claude@dev", "message": "tarefa longa", "wait": 1}).to_string();
    let asking = std::thread::spawn({
        let claude = claude.clone();
        let host = host.clone();
        move || http(port, "POST", "/ask", &host, &claude, &body)
    });
    let question = next_delivery(&env);
    let (st, b) = asking.join().expect("thread");
    assert!(t.elapsed() >= Duration::from_secs(1));
    assert_eq!(st, 202, "{b}");
    assert_eq!(b["pending"].as_u64(), Some(question.request));
    assert!(b["answer"].as_str().expect("warning").contains("typed into this terminal"), "{b}");

    let dev = tab_token(&env, "claude@dev");
    let reply = json!({"to": "", "message": "done\u{1b}[201~", "reply_to": question.request}).to_string();
    assert_eq!(http(port, "POST", "/ask", &host, &dev, &reply).0, 200);
    let late = next_delivery(&env);
    assert!(late.answer);
    assert_eq!((late.ident.id().as_str(), late.create, late.from.as_str()), ("claude", false, "claude@dev"));
    assert!(
        late.prompt.starts_with(&format!("[Answer from the agent \"claude@dev\" to your question #{}", question.request)),
        "{}",
        late.prompt
    );
    assert!(late.prompt.contains("done[201~") && !late.prompt.contains('\u{1b}'), "{}", late.prompt);
    env.core.shutdown();
}

#[test]
fn closing_the_askers_tab_drops_its_questions() {
    let env = start(&[("claude", says("x"))]);
    let p = env.core.active_project_id();
    env.core.enable_tab_delivery();
    let port = env.core.bridge_port();
    let host = format!("127.0.0.1:{port}");
    let dev = tab_token(&env, "claude@dev");
    let asking = std::thread::spawn(move || http(port, "POST", "/ask", &host, &dev, &json!({"to": "claude", "message": "oi"}).to_string()));
    let d = next_delivery(&env);
    env.core.close_instance(&p, "claude@dev");
    let (st, b) = asking.join().expect("thread");
    assert_eq!(st, 500);
    assert!(b["error"].as_str().expect("error").contains("was closed"), "{b}");
    // Without the old question, claude can ask claude@dev (no ghost "cycle").
    let claude = tab_token(&env, "claude");
    let port2 = env.core.bridge_port();
    let again = std::thread::spawn(move || {
        http(
            port2,
            "POST",
            "/ask",
            &format!("127.0.0.1:{port2}"),
            &claude,
            &json!({"to": "claude@dev", "message": "what now?"}).to_string(),
        )
    });
    let d2 = next_delivery(&env);
    assert_ne!(d2.request, d.request);
    assert_eq!(d2.ident.id(), "claude@dev");
    env.core.shutdown();
    let _ = again.join();
}

#[test]
fn a_plain_terminal_waits_until_the_end_instead_of_getting_typed_answers() {
    // In a "shell" tab, an answer typed later would run as a command.
    let env = start(&[("claude", says("x"))]);
    env.core.enable_tab_delivery();
    let port = env.core.bridge_port();
    let host = format!("127.0.0.1:{port}");
    let shell = tab_token(&env, "shell");
    let asking = std::thread::spawn(move || {
        http(port, "POST", "/ask", &host, &shell, &json!({"to": "claude", "message": "oi", "wait": 1}).to_string())
    });
    let d = next_delivery(&env);
    std::thread::sleep(Duration::from_millis(1500));
    assert!(!asking.is_finished(), "it cannot return with 202 after 1 s");
    let claude = tab_token(&env, "claude");
    let body = json!({"to": "", "message": "answer", "reply_to": d.request}).to_string();
    assert_eq!(http(port, "POST", "/ask", &format!("127.0.0.1:{port}"), &claude, &body).0, 200);
    let (st, b) = asking.join().expect("thread");
    assert_eq!((st, b["answer"].as_str()), (200, Some("answer")));
    env.core.shutdown();
}

#[test]
fn the_team_can_be_reorganized_and_survives_renames_and_closes() {
    let env = start(&[]);
    let p = env.core.active_project_id();
    let port = env.core.bridge_port();
    let host = format!("127.0.0.1:{port}");
    for id in ["shell", "shell@dev", "shell@qa"] {
        env.core.restore_instance(&p, id);
    }
    env.core.set_boss(&p, "shell@dev", Some("shell")).expect("reorganiza");
    env.core.set_boss(&p, "shell@qa", Some("shell@dev")).expect("reorganiza");

    // Each tab checks its own position through the token.
    let (st, b) = http(port, "GET", "/team", &host, &tab_token(&env, "shell@dev"), "");
    assert_eq!((st, b), (200, json!({"id": "shell@dev", "boss": "shell", "crew": ["shell@qa"], "quota": {}, "models": {}})));
    let (st, _) = http(port, "GET", "/team", &host, env.core.bridge_token(), "");
    assert_eq!(st, 400, "the master token is not an agent");

    // Cycles and closed instances are refused.
    assert!(env.core.set_boss(&p, "shell", Some("shell@qa")).is_err(), "the boss cannot end up under its own crew");
    assert!(env.core.set_boss(&p, "shell@qa", Some("shell@nada")).is_err());

    // Renaming takes the team along; closing passes the crew to the grandparent; releasing at the root removes the boss.
    let ana = agentdeck_core::Ident::parse("shell@dev@ana").expect("id");
    env.core.rename_instance(&p, "shell@dev", &ana).expect("renames");
    assert_eq!(env.core.team(&p, "shell@qa").boss.as_deref(), Some("shell@dev@ana"));
    env.core.close_instance(&p, "shell@dev@ana");
    assert_eq!(env.core.team(&p, "shell@qa").boss.as_deref(), Some("shell"));
    assert_eq!(env.core.team(&p, "shell").crew, ["shell@qa"]);
    env.core.set_boss(&p, "shell@qa", None).expect("root");
    assert_eq!(env.core.team(&p, "shell"), agentdeck_core::Team::default());
    assert!(env.core.projects()[0].tab_parents.is_empty(), "saved in the project's config");
    env.core.shutdown();
}

#[test]
fn an_agent_becomes_the_boss_of_the_role_it_opens_and_the_new_one_is_told() {
    let env = start(&[("claude", says("headless"))]);
    let p = env.core.active_project_id();
    env.core.enable_tab_delivery();
    let port = env.core.bridge_port();
    env.core.restore_instance(&p, "claude");
    let boss = tab_token(&env, "claude");
    let asker = std::thread::spawn(move || ask_raw(port, &boss, json!({"to": "claude@dev", "message": "task"})));
    let d = next_delivery(&env);
    assert_eq!((d.ident.id().as_str(), d.create), ("claude@dev", true));
    // The notice goes when the interface types the message.
    let notice = env.core.take_team_notice(&p, "claude@dev").expect("notice");
    assert!(notice.contains("you answer to \"claude\""), "{notice}");
    assert_eq!(env.core.team(&p, "claude").crew, ["claude@dev"]);
    env.core.close_instance(&p, "claude@dev");
    let _ = asker.join();
    env.core.shutdown();
}

/// Active project with `claude` "installed", tab delivery on and the `claude` tab open.
fn start_with_claude_tab() -> (Env, String) {
    let env = start(&[]);
    fake_claude(&env);
    env.core.enable_tab_delivery();
    let p = env.core.active_project_id();
    env.core.restore_instance(&p, "claude");
    (env, p)
}

/// `POST /ask` with a tab's token, in a thread.
fn ask_in_thread(env: &Env, token: String, body: Value) -> std::thread::JoinHandle<(u16, Value)> {
    let port = env.core.bridge_port();
    std::thread::spawn(move || ask_raw(port, &token, body))
}

#[test]
fn renaming_both_tabs_mid_question_still_delivers_the_late_answer() {
    let (env, p) = start_with_claude_tab();
    let asking = ask_in_thread(&env, tab_token(&env, "claude"), json!({"to": "claude@dev", "message": "task", "wait": 1}));
    let q = next_delivery(&env);
    assert_eq!(asking.join().expect("thread").0, 202);
    // Both tabs change names while the answer has not arrived.
    let ident = |s: &str| agentdeck_core::Ident::parse(s).expect("id");
    env.core.rename_instance(&p, "claude", &ident("claude@lead")).expect("renames");
    env.core.rename_instance(&p, "claude@dev", &ident("claude@dev@ana")).expect("renames");
    // The tab's token follows the instance; the answer goes by the number and reaches the new name.
    let reply = json!({"to": "", "message": "done", "reply_to": q.request});
    assert_eq!(ask_raw(env.core.bridge_port(), &tab_token(&env, "claude@dev@ana"), reply).0, 200);
    let late = next_delivery(&env);
    assert!(late.answer);
    assert_eq!((late.ident.id().as_str(), late.from.as_str()), ("claude@lead", "claude@dev@ana"));
    env.core.shutdown();
}

#[test]
fn after_a_202_asking_the_asker_back_is_a_new_question() {
    let (env, _p) = start_with_claude_tab();
    let asking = ask_in_thread(&env, tab_token(&env, "claude"), json!({"to": "claude@dev", "message": "task", "wait": 1}));
    let q = next_delivery(&env);
    assert_eq!(asking.join().expect("thread").0, 202);
    // claude no longer waits: `ask-agent claude "…"` from claude@dev is a new question, typed into its tab.
    let back = ask_in_thread(&env, tab_token(&env, "claude@dev"), json!({"to": "claude", "message": "which branch?"}));
    let d = next_delivery(&env);
    assert_eq!((d.ident.id().as_str(), d.from.as_str(), d.answer), ("claude", "claude@dev", false));
    assert_ne!(d.request, q.request);
    assert!(env.core.question_pending(q.request), "the original question still waits for --reply");
    env.core.fail_delivery(d.request, "test");
    assert_eq!(back.join().expect("thread").0, 500);
    assert!(env.core.question_pending(q.request), "releasing the new question does not cancel the original");
    env.core.shutdown();
}

#[test]
fn the_old_reply_form_resolves_aliases() {
    let (env, _p) = start_with_claude_tab();
    let asking = ask_in_thread(&env, tab_token(&env, "claude"), json!({"to": "claude@dev", "message": "task"}));
    next_delivery(&env);
    // `claudecode` is an alias of `claude`: it is the answer to whoever waits, not a cycle (409).
    let (st, b) = ask_raw(env.core.bridge_port(), &tab_token(&env, "claude@dev"), json!({"to": "claudecode", "message": "done"}));
    assert_eq!((st, b["answer"].as_str()), (200, Some("Answer delivered to \"claude\".")), "{b}");
    let (st, b) = asking.join().expect("thread");
    assert_eq!((st, b["answer"].as_str()), (200, Some("done")));
    env.core.shutdown();
}

/// `GET /inbox` (or `/inbox?all=1`) with a tab's token: the answers in its box.
fn inbox(env: &Env, token: &str, all: bool) -> Vec<Value> {
    let port = env.core.bridge_port();
    let route = if all { "/inbox?all=1" } else { "/inbox" };
    let (st, b) = http(port, "GET", route, &format!("127.0.0.1:{port}"), token, "");
    assert_eq!(st, 200, "{b}");
    b["answers"].as_array().expect("answers").clone()
}

#[test]
fn a_synchronous_answer_lands_in_the_inbox_as_delivered() {
    let (env, _p) = start_with_claude_tab();
    let claude = tab_token(&env, "claude");
    let asking = ask_in_thread(&env, claude.clone(), json!({"to": "claude@dev", "message": "task"}));
    let q = next_delivery(&env);
    assert_eq!(
        ask_raw(env.core.bridge_port(), &tab_token(&env, "claude@dev"), json!({"to": "", "message": "done", "reply_to": q.request})).0,
        200
    );
    let (st, b) = asking.join().expect("thread");
    assert_eq!((st, b["answer"].as_str(), b["request"].as_u64()), (200, Some("done"), Some(q.request)), "{b}");
    // The 200 was written: the answer is in the box, already delivered (the mark comes right after the write).
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !inbox(&env, &claude, true).iter().all(|e| e["delivered"] == true) {
        assert!(std::time::Instant::now() < deadline, "the synchronous answer was not marked as delivered");
        std::thread::sleep(Duration::from_millis(20));
    }
    let all = inbox(&env, &claude, true);
    assert_eq!(all.len(), 1, "{all:?}");
    assert_eq!(
        (all[0]["request"].as_u64(), all[0]["from"].as_str(), all[0]["text"].as_str()),
        (Some(q.request), Some("claude@dev"), Some("done"))
    );
    assert_eq!(all[0]["failed"], false);
    assert!(inbox(&env, &claude, false).is_empty(), "a delivered one does not show up in --inbox");
    // Only the agent itself reads its box: another's is empty and the master token has no box.
    assert!(inbox(&env, &tab_token(&env, "claude@dev"), true).is_empty());
    let port = env.core.bridge_port();
    assert_eq!(http(port, "GET", "/inbox", &format!("127.0.0.1:{port}"), env.core.bridge_token(), "").0, 400);
    env.core.shutdown();
}

#[test]
fn with_ack_the_answer_waits_for_the_client_to_confirm_it() {
    let (env, _p) = start_with_claude_tab();
    let claude = tab_token(&env, "claude");
    let dev = tab_token(&env, "claude@dev");
    let answered = |message: &str| {
        let asking = ask_in_thread(&env, claude.clone(), json!({"to": "claude@dev", "message": message, "ack": true}));
        let q = next_delivery(&env);
        assert_eq!(ask_raw(env.core.bridge_port(), &dev, json!({"to": "", "message": "done", "reply_to": q.request})).0, 200);
        let (st, b) = asking.join().expect("thread");
        assert_eq!((st, b["request"].as_u64()), (200, Some(q.request)), "{b}");
        q.request
    };
    // Writing the 200 does not prove `ask-agent` was still there: without the confirmation it stays pending
    // (reading the box marks it as delivered, which is why each question is read only once).
    let first = answered("first");
    std::thread::sleep(Duration::from_millis(200));
    let pending: Vec<u64> = inbox(&env, &claude, false).iter().filter_map(|e| e["request"].as_u64()).collect();
    assert_eq!(pending, [first]);
    // Confirmed by the client, it leaves `--inbox`; another agent does not confirm the asker's answer.
    let second = answered("second");
    let port = env.core.bridge_port();
    let host = format!("127.0.0.1:{port}");
    let body = json!({ "request": second }).to_string();
    assert_eq!(http(port, "POST", "/inbox/delivered", &host, &dev, &body).0, 404);
    assert_eq!(http(port, "POST", "/inbox/delivered", &host, &claude, &body).0, 200);
    assert!(inbox(&env, &claude, false).is_empty(), "a confirmed one does not show up in --inbox");
    env.core.shutdown();
}

#[test]
fn a_failed_question_is_kept_in_the_inbox_with_its_reason() {
    let (env, _p) = start_with_claude_tab();
    let claude = tab_token(&env, "claude");
    let asking = ask_in_thread(&env, claude.clone(), json!({"to": "claude@dev", "message": "task"}));
    let q = next_delivery(&env);
    env.core.fail_delivery(q.request, "the tab died");
    assert_eq!(asking.join().expect("thread").0, 500);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while inbox(&env, &claude, true).iter().all(|e| e["delivered"] != true) {
        assert!(std::time::Instant::now() < deadline, "the failure did not reach the box");
        std::thread::sleep(Duration::from_millis(20));
    }
    let all = inbox(&env, &claude, true);
    assert_eq!((all[0]["failed"].as_bool(), all[0]["text"].as_str()), (Some(true), Some("the tab died")), "{all:?}");
    env.core.shutdown();
}

#[test]
fn a_late_answer_stays_in_the_inbox_until_it_is_typed_or_read() {
    let (env, p) = start_with_claude_tab();
    let claude = tab_token(&env, "claude");
    let asking = ask_in_thread(&env, claude.clone(), json!({"to": "claude@dev", "message": "task", "wait": 1}));
    let q = next_delivery(&env);
    let (st, b) = asking.join().expect("thread");
    assert_eq!(st, 202);
    assert!(b["answer"].as_str().expect("aviso").contains("ask-agent --inbox"), "{b}");
    assert!(inbox(&env, &claude, true).is_empty(), "no answer yet");

    let dev = tab_token(&env, "claude@dev");
    assert_eq!(ask_raw(env.core.bridge_port(), &dev, json!({"to": "", "message": "done", "reply_to": q.request})).0, 200);
    let late = next_delivery(&env);
    assert!(late.answer);
    // The interface has not typed it yet: the answer is in the box, not delivered.
    let pending = inbox(&env, &claude, false);
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!((pending[0]["text"].as_str(), pending[0]["delivered"].as_bool()), (Some("done"), Some(false)));
    // Read in --inbox, it does not come back; it stays in --inbox --all.
    assert!(inbox(&env, &claude, false).is_empty());
    assert_eq!(inbox(&env, &claude, true).len(), 1);

    // The interface types a second answer: delivered, out of the default list. If it could not type it, it warns in the Conversation tab.
    let asking = ask_in_thread(&env, claude.clone(), json!({"to": "claude@dev", "message": "another", "wait": 1}));
    let q2 = next_delivery(&env);
    assert_eq!(asking.join().expect("thread").0, 202);
    assert_eq!(ask_raw(env.core.bridge_port(), &dev, json!({"to": "", "message": "second", "reply_to": q2.request})).0, 200);
    assert!(next_delivery(&env).answer);
    env.core.answer_typed(q2.request);
    assert!(inbox(&env, &claude, false).is_empty(), "the typed one was already delivered");
    env.core.answer_lost(&p, q2.request, "the tab was not open");
    let warning = loop {
        match env.rx.recv_timeout(Duration::from_secs(5)).expect("warning in the Conversation tab") {
            CoreEvent::Feed(e) if e.kind == FeedKind::Error && e.text.contains("--inbox") => break e,
            _ => continue,
        }
    };
    assert!(warning.text.contains(&format!("#{}", q2.request)) && warning.text.contains("the tab was not open"), "{}", warning.text);
    assert_eq!((warning.to.as_deref(), warning.project.as_deref()), (Some("claude"), Some(p.as_str())));
    env.core.shutdown();
}

#[test]
fn removing_a_project_mid_question_leaves_nothing_behind() {
    let env = start(&[]);
    fake_claude(&env);
    env.core.enable_tab_delivery();
    let b = env.core.create_project(&env.root.join("b"), None).id;
    env.core.restore_instance(&b, "claude");
    let tb = tab_token_in(&env, &b, "claude");
    let asking = ask_in_thread(&env, tb.clone(), json!({"to": "claude@dev", "message": "task"}));
    let q = next_delivery(&env);
    assert_eq!(q.project, b);
    assert!(env.core.remove_project(&b));
    let (st, body) = asking.join().expect("thread");
    assert_eq!(st, 500, "{body}");
    assert_eq!(ask_raw(env.core.bridge_port(), &tb, json!({"to": "claude@qa", "message": "oi"})).0, 403);
    assert!(!env.core.question_pending(q.request), "the question goes away with the project");
    // Nothing recreates the removed project's entry.
    env.core.restore_instance(&b, "claude@qa");
    assert!(env.core.project_instance_ids(&b).is_empty());
    assert!(env.core.instance_ids().is_empty());
    env.core.shutdown();
}

#[test]
fn the_team_notice_waits_for_a_message_that_is_really_typed() {
    let (env, p) = start_with_claude_tab();
    for id in ["claude@dev", "claude@qa"] {
        env.core.restore_instance(&p, id);
    }
    env.core.set_boss(&p, "claude@dev", Some("claude")).expect("reorganiza");
    env.core.set_boss(&p, "claude@qa", Some("claude")).expect("reorganiza");
    let asking = ask_in_thread(&env, tab_token(&env, "claude"), json!({"to": "claude@dev", "message": "task"}));
    let d = next_delivery(&env);
    assert!(!d.prompt.contains("place in the team"), "the interface attaches it when typing: {}", d.prompt);
    // The interface discarded the delivery: the notice stays for the next message.
    env.core.fail_delivery(d.request, "discarded");
    assert_eq!(asking.join().expect("thread").0, 500);
    assert!(env.core.take_team_notice(&p, "claude@dev").expect("notice").contains("you answer to \"claude\""));
    assert_eq!(env.core.take_team_notice(&p, "claude@dev"), None, "only once");
    // The boss that loses a subordinate with no crew is warned too.
    let _ = env.core.take_team_notice(&p, "claude");
    env.core.close_instance(&p, "claude@qa");
    assert!(env.core.take_team_notice(&p, "claude").expect("notice").contains("your crew: claude@dev"));
    env.core.shutdown();
}

#[test]
fn a_model_without_quota_is_refused_before_opening_the_tab() {
    use agentdeck_core::{AgentUsage, UsageGroup, UsageWindow};
    let env = start(&[]);
    fake_cli(&env, "agy");
    env.core.enable_tab_delivery();
    let p = env.core.active_project_id();
    let port = env.core.bridge_port();
    let master = env.core.bridge_token().to_owned();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_millis() as u64;
    let w = |used, h: u64| Some(UsageWindow { used, resets_at_ms: Some(now + h * 3_600_000) });
    let group = |name: &str, weekly| UsageGroup { name: name.into(), session: None, weekly };
    let groups = vec![group("Gemini Models", w(0.0, 30)), group("Gemini 4 Argon", w(1.0, 9))];
    env.core.set_usage("antigravity", AgentUsage { weekly: w(1.0, 9), updated_ms: now, groups, ..Default::default() });

    // Argon without quota: a 409 that explains it and suggests another model; no tab is created.
    let (st, body) =
        ask_raw(port, &master, json!({"from": "shell", "to": "antigravity@arq", "message": "oi", "model": "gemini-4-argon-high"}));
    assert_eq!(st, 409, "{body}");
    let error = body["error"].as_str().expect("error");
    assert!(error.starts_with("antigravity@arq uses gemini-4-argon-high and its quota for Gemini 4 Argon exhausted, renews in"), "{error}");
    assert!(error.contains("--new --model <model>"), "{error}");
    assert!(env.core.project_instance_ids(&p).is_empty(), "nothing was created");
    assert!(env.core.projects()[0].tab_models.is_empty());

    // The Flash counts in another group: it delivers normally.
    let asking = master_ask(&env, json!({"from": "shell", "to": "antigravity@arq", "message": "oi", "model": "gemini-3.8-flash-high"}));
    let d = next_delivery(&env);
    assert_eq!((d.ident.id().as_str(), d.create), ("antigravity@arq", true));
    env.core.fail_delivery(d.request, "test");
    assert_eq!(asking.join().expect("thread").0, 500);

    // A stale reading does not refuse: the quota may have come back.
    let stale = vec![group("Gemini 4 Argon", w(1.0, 9))];
    env.core.set_usage("antigravity", AgentUsage { updated_ms: now - 2 * 3_600_000, groups: stale, ..Default::default() });
    let asking = master_ask(&env, json!({"from": "shell", "to": "antigravity@b", "message": "oi", "model": "gemini-4-argon-low"}));
    let d = next_delivery(&env);
    assert_eq!(d.ident.id(), "antigravity@b");
    env.core.fail_delivery(d.request, "test");
    assert_eq!(asking.join().expect("thread").0, 500);
}

/// `POST /ask` with the master token (the body's `from` counts), in a thread.
fn master_ask(env: &Env, body: Value) -> std::thread::JoinHandle<(u16, Value)> {
    let token = env.core.bridge_token().to_owned();
    ask_in_thread(env, token, body)
}

#[test]
fn a_model_conflict_on_an_open_instance_is_an_explicit_409() {
    let env = start(&[]);
    for cli in ["claude", "aider"] {
        fake_cli(&env, cli);
    }
    env.core.enable_tab_delivery();
    let p = env.core.active_project_id();
    let state = |env: &Env| env.core.projects()[0].tab_models.clone();
    let port = env.core.bridge_port();
    let master = env.core.bridge_token().to_owned();

    // Opens `claude@dev` with sonnet: the model is saved before the tab opens.
    let first = master_ask(&env, json!({"from": "shell", "to": "claude@dev", "message": "task", "model": "sonnet"}));
    let d = next_delivery(&env);
    assert_eq!((d.ident.id().as_str(), d.create), ("claude@dev", true));
    assert_eq!(env.core.instance_model(&p, &d.ident).as_deref(), Some("sonnet"));
    assert_eq!(state(&env).get("claude@dev").map(String::as_str), Some("sonnet"), "saved in the project's config");
    assert_eq!(env.core.instance_models(&p).get("claude@dev").map(String::as_str), Some("sonnet"));

    // Another model for the same instance: a 409 with the way out through --new; nothing changes.
    let (st, body) = ask_raw(port, &master, json!({"from": "shell", "to": "claude@dev", "message": "oi", "model": "opus"}));
    assert_eq!(st, 409, "{body}");
    assert_eq!(body["error"], "claude@dev is already open with sonnet; use --new to open another one with opus");
    assert_eq!(env.core.project_instance_ids(&p), ["claude@dev"]);
    assert_eq!(state(&env).len(), 1);
    // Asking for a model on an instance open with the CLI default is a conflict too.
    env.core.restore_instance(&p, "claude@qa");
    let (st, body) = ask_raw(port, &master, json!({"from": "shell", "to": "claude@qa", "message": "oi", "model": "opus"}));
    assert_eq!(
        (st, body["error"].as_str()),
        (409, Some("claude@qa is already open with the default model; use --new to open another one with opus"))
    );

    // The same model, or none, delivers normally into the tab that already exists.
    for model in [Some("sonnet"), None] {
        let mut body = json!({"from": "shell", "to": "claude@dev", "message": "again"});
        if let Some(m) = model {
            body["model"] = json!(m);
        }
        let asking = master_ask(&env, body);
        let d = next_delivery(&env);
        assert_eq!((d.ident.id().as_str(), d.create), ("claude@dev", false), "{model:?}");
        env.core.fail_delivery(d.request, "test");
        assert_eq!(asking.join().expect("thread").0, 500);
    }

    // --new + --model opens another instance of the role with the requested model.
    let second = master_ask(&env, json!({"from": "shell", "to": "claude@dev", "message": "another", "model": "opus", "new": true}));
    let d = next_delivery(&env);
    assert_eq!((d.ident.id().as_str(), d.create), ("claude@dev@ana", true));
    assert_eq!(env.core.instance_model(&p, &d.ident).as_deref(), Some("opus"));
    assert_eq!(state(&env).get("claude@dev").map(String::as_str), Some("sonnet"), "the first one did not change");

    // Model out of the pattern, or an agent that does not choose a model: 400 before creating anything.
    let ids_before = env.core.project_instance_ids(&p);
    let bad = |to: &str, model: Value| ask_raw(port, &master, json!({"from": "shell", "to": to, "message": "oi", "model": model}));
    let (st, body) = bad("claude@x", json!("a b"));
    assert_eq!(st, 400);
    assert!(body["error"].as_str().expect("error").starts_with("invalid model: \"a b\""), "{body}");
    assert_eq!(bad("claude@x", json!("a\"; rm")).0, 400);
    assert_eq!(bad("claude@x", json!("m".repeat(129))).0, 400);
    assert_eq!(bad("claude@x", json!(7)).0, 400);
    assert_eq!(bad("aider", json!("gpt")), (400, json!({"error": "the agent aider does not accept choosing a model"})));
    assert_eq!(env.core.project_instance_ids(&p), ids_before, "nothing was created");
    assert_eq!(state(&env).len(), 2);

    // Closing forgets the model.
    env.core.close_instance(&p, "claude@dev@ana");
    assert_eq!(state(&env).keys().collect::<Vec<_>>(), ["claude@dev"]);
    env.core.shutdown();
    for t in [first, second] {
        let _ = t.join();
    }
}

#[test]
fn a_failed_request_does_not_keep_the_model_of_an_instance_that_never_opened() {
    let env = start(&[]);
    fake_claude(&env);
    env.core.enable_tab_delivery();
    let p = env.core.active_project_id();
    // Depth exceeded after creating: the instance and the model go away together.
    let (st, _) = ask_raw(
        env.core.bridge_port(),
        env.core.bridge_token(),
        json!({"from": "shell", "to": "claude@dev", "message": "oi", "model": "sonnet", "depth": 2}),
    );
    assert_eq!(st, 400);
    assert!(env.core.project_instance_ids(&p).is_empty());
    assert!(env.core.projects()[0].tab_models.is_empty());
    env.core.shutdown();
}

#[test]
fn the_roster_and_the_team_show_the_models() {
    let (env, p) = start_with_claude_tab();
    let port = env.core.bridge_port();
    let host = format!("127.0.0.1:{port}");
    let second = env.core.register_instance_with_model(&p, "claude", Some("opus")).expect("registra");
    assert_eq!(second.id(), "claude@main@ana");
    let dev = agentdeck_core::Ident::parse("claude@dev").expect("id");
    env.core.restore_instance(&p, "claude@dev");
    env.core.assign_model(&p, &dev, Some("sonnet"));
    env.core.set_boss(&p, "claude@dev", Some("claude")).expect("team");

    let (st, body) = http(port, "GET", "/agents", &host, &tab_token(&env, "claude"), "");
    assert_eq!(st, 200);
    assert_eq!(body["models"], json!({"claude@dev": "sonnet", "claude@main@ana": "opus"}));
    assert_eq!(body["quota"], json!({"claude@dev": "quota: no reading", "claude@main@ana": "quota: no reading"}));
    let (st, body) = http(port, "GET", "/team", &host, &tab_token(&env, "claude@dev"), "");
    assert_eq!(st, 200);
    // Only those in the answer (itself; the boss and the crew, if they had a model).
    assert_eq!(body["models"], json!({"claude@dev": "sonnet"}));
    assert_eq!(body["quota"], json!({"claude@dev": "quota: no reading"}));
    assert_eq!(body["boss"], "claude");
    env.core.shutdown();
}

#[test]
fn the_models_inventory_lists_installed_agents_only_and_survives_a_failed_listing() {
    use agentdeck_core::{AgentUsage, UsageGroup, UsageWindow};
    let env = start(&[]);
    // The installed `opencode` lists nothing (the fake exits with no output): one failure line, the rest goes on.
    fake_cli(&env, "claude");
    fake_cli(&env, "opencode");
    let port = env.core.bridge_port();
    let host = format!("127.0.0.1:{port}");
    let get = |path: &str| http(port, "GET", path, &host, env.core.bridge_token(), "");

    let (st, body) = get("/models");
    assert_eq!(st, 200, "{body}");
    let text = body["text"].as_str().expect("text");
    assert!(text.starts_with("claude --model: opus, sonnet, haiku, fable\n  quota: no reading\n"), "{text}");
    assert!(text.contains("opencode: could not list the models"), "{text}");
    assert!(text.contains("\n  quota: no reading\n\ncapability ("), "the capability comes after the agents: {text}");
    assert!(text.contains("Claude Sonnet 5.5 (claude sonnet): complexa, Coding Agent Index 59.3 (2026-10)"), "{text}");
    assert!(!text.contains("antigravity"), "what is not installed does not show up: {text}");

    // A single agent (with an alias); the not-installed and the invalid ones are refused.
    let (st, body) = get("/models?agent=claudecode");
    assert_eq!(st, 200);
    assert!(
        body["text"]
            .as_str()
            .is_some_and(|t| t.starts_with("claude --model: opus, sonnet, haiku, fable\n  quota: no reading\n\ncapability (")),
        "{body}"
    );
    let (st, body) = get("/models?agent=agy");
    assert_eq!(st, 404);
    assert!(body["error"].as_str().expect("error").contains("is not installed"), "{body}");
    assert_eq!(get("/models?agent=a%20b").0, 400);

    // With Antigravity installed and renewed (4%) and Claude at 60%, Antigravity comes before Claude.
    fake_cli(&env, "agy");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_millis() as u64;
    let w = |used, h: u64| Some(UsageWindow { used, resets_at_ms: Some(now + h * 3_600_000) });
    env.core.set_usage("claude", AgentUsage { weekly: w(0.60, 4 * 24), updated_ms: now, ..Default::default() });
    env.core.set_usage(
        "antigravity",
        AgentUsage {
            groups: vec![UsageGroup { name: "Gemini 4 Argon".into(), session: None, weekly: w(0.04, 6 * 24) }],
            updated_ms: now,
            ..Default::default()
        },
    );
    let (st, body) = get("/models");
    assert_eq!(st, 200, "{body}");
    let text = body["text"].as_str().expect("text");
    let agy_pos = text.find("antigravity").expect("agy");
    let claude_pos = text.find("claude --model").expect("claude");
    assert!(agy_pos < claude_pos, "the one with the most room comes first:\n{text}");
    assert!(text.contains("  quota: Gemini 4 Argon 4% used (renews in 6 days)"), "{text}");
    assert!(text.contains("  quota: 60% used (renews in 4 days)"), "{text}");
    env.core.shutdown();
}

#[test]
fn the_model_resolves_to_request_then_saved_then_role_default_and_reaches_each_cli() {
    let env = start_cfg(&[], |c| {
        c.role_models.insert("claude@dev".into(), "sonnet".into());
        c.role_models.insert("claude".into(), "opus".into());
        c.role_models.insert("opencode".into(), "opencode/glm-5.3-flash-free".into());
        c.role_models.insert("aider".into(), "not-valid".into());
    });
    for cli in ["claude", "codex", "opencode", "agy", "aider"] {
        fake_cli(&env, cli);
    }
    let p = env.core.active_project_id();
    let ident = |s: &str| agentdeck_core::Ident::parse(s).expect("id");
    let saved = |env: &Env| env.core.projects()[0].tab_models.clone();
    let tail = |args: &[String], n: usize| args[args.len() - n..].to_vec();
    let data = env.root.join("dados").display().to_string();

    // c) the role default counts, and is saved when opening.
    let dev = env.core.spawn_spec_for(&p, &ident("claude@dev")).expect("claude@dev");
    assert_eq!(dev.model.as_deref(), Some("sonnet"));
    assert_eq!(tail(&dev.args, 2), ["--model", "sonnet"]);
    assert_eq!(saved(&env).get("claude@dev").map(String::as_str), Some("sonnet"));
    // `model@role` beats `model`; a role without an entry falls back to `model`.
    assert_eq!(env.core.spawn_spec_for(&p, &ident("claude@qa")).expect("qa").model.as_deref(), Some("opus"));

    // b) the saved one beats a default that changed later.
    env.core.update_config(|c| {
        c.role_models.insert("claude@dev".into(), "haiku".into());
    });
    assert_eq!(env.core.spawn_spec_for(&p, &ident("claude@dev")).expect("again").model.as_deref(), Some("sonnet"));
    // a) the request beats the saved one.
    assert_eq!(env.core.assign_model(&p, &ident("claude@dev"), Some("fable")).as_deref(), Some("fable"));
    let spec = env.core.spawn_spec_for(&p, &ident("claude@dev")).expect("fable");
    assert_eq!(tail(&spec.args, 2), ["--model", "fable"]);

    // Codex and Antigravity: --model; Codex keeps the end-of-turn hook.
    env.core.assign_model(&p, &ident("codex@x"), Some("gpt-5-codex"));
    let codex = env.core.spawn_spec_for(&p, &ident("codex@x")).expect("codex");
    assert_eq!(tail(&codex.args, 2), ["--model", "gpt-5-codex"]);
    assert!(codex.args.iter().any(|a| a.starts_with("notify=")));
    let agy = env.core.spawn_spec_for(&p, &ident("antigravity@x")).expect("agy");
    // Last, the instance's instructions folder (`dados/agy/<id>`), after the data folder.
    let own = std::path::Path::new(&data).join("agy").join("antigravity@x").to_string_lossy().into_owned();
    assert_eq!((agy.model, tail(&agy.args, 3)), (None, vec![data, "--add-dir".to_owned(), own]), "no model, no --model");
    env.core.assign_model(&p, &ident("antigravity@x"), Some("claude-sonnet-5-5-medium"));
    let agy = env.core.spawn_spec_for(&p, &ident("antigravity@x")).expect("agy");
    assert_eq!(tail(&agy.args, 2), ["--model", "claude-sonnet-5-5-medium"]);

    // OpenCode: the TUI has no --model; it gets its own server + configuration in the environment.
    let oc = env.core.spawn_spec_for(&p, &ident("opencode")).expect("opencode");
    assert_eq!(oc.args, ["--standalone"]);
    let conf: Value = serde_json::from_str(&oc.env["OPENCODE_CONFIG_CONTENT"]).expect("json");
    let instructions = env.root.join("dados").join("agentdeck-instructions.md");
    assert_eq!(conf, json!({"model": "opencode/glm-5.3-flash-free", "instructions": [instructions.to_string_lossy()]}));
    // Without a model, OpenCode still gets the AgentDeck instructions (own server + file in the configuration).
    env.core.update_config(|c| {
        c.role_models.remove("opencode");
    });
    let plain = env.core.spawn_spec_for(&p, &ident("opencode@qa")).expect("opencode without a model");
    assert_eq!(plain.args, ["--standalone"]);
    let conf: Value = serde_json::from_str(&plain.env["OPENCODE_CONFIG_CONTENT"]).expect("json");
    assert_eq!(conf, json!({"instructions": [instructions.to_string_lossy()]}));

    // Whoever does not choose a model does not get one, neither from the default nor from the request.
    assert_eq!(env.core.spawn_spec_for(&p, &ident("aider")).expect("aider").model, None);
    assert_eq!(env.core.assign_model(&p, &ident("aider"), Some("gpt")), None);
    assert_eq!(env.core.spawn_spec_for(&p, &ident("shell")).expect("shell").model, None);
    assert!(!saved(&env).contains_key("aider") && !saved(&env).contains_key("shell"));
    assert!(env.core.register_instance_with_model(&p, "aider", Some("gpt")).is_err());
    assert!(env.core.register_instance_with_model(&p, "claude", Some("a b")).is_err());

    // Renaming takes the model; closing forgets it.
    env.core.restore_instance(&p, "claude@dev");
    env.core.rename_instance(&p, "claude@dev", &ident("claude@lead")).expect("renames");
    assert_eq!(saved(&env).get("claude@lead").map(String::as_str), Some("fable"));
    assert!(!saved(&env).contains_key("claude@dev"));
    env.core.close_instance(&p, "claude@lead");
    assert!(!saved(&env).contains_key("claude@lead"));
    env.core.shutdown();
}

#[test]
fn a_saved_model_survives_restarting_the_app() {
    let tmp = tempfile::tempdir().expect("tmp");
    let paths = paths_in(tmp.path());
    let opts = |config: Config| StartOptions {
        search_path: None,
        home_dir: Some(tmp.path().join("home")),
        watch_workspace: false,
        config: Some(config),
        system_dirs: false,
    };
    let (core, _rx) = Core::start_with(paths.clone(), opts(Config::with_project(paths.default_workspace.clone()))).expect("start");
    let p = core.active_project_id();
    let ident = core.register_instance_with_model(&p, "claude", Some("sonnet")).expect("registra");
    assert_eq!(ident.id(), "claude");
    let saved = std::fs::read_to_string(&paths.config_file).expect("the config was written");
    assert!(saved.contains("\"tabModels\"") && saved.contains("\"sonnet\""), "{saved}");
    core.shutdown();
    drop(core);
    // Another app launch reads the same config: the model is there, and `restore_instance` respects it.
    let again = Config::load(&paths);
    assert_eq!(again.project(&p).map(|x| x.tab_models.clone()), Some([("claude".to_owned(), "sonnet".to_owned())].into()));
}

#[test]
fn headless_runs_get_the_requested_or_default_model() {
    let env = start_cfg(&[("claude", echoing_agent()), ("aider", says("ok"))], |c| {
        c.role_models.insert("claude".into(), "opus".into());
    });
    let ask_model = |model: Option<&str>| {
        let mut body = json!({"from": "shell", "to": "claude", "message": "oi"});
        if let Some(m) = model {
            body["model"] = json!(m);
        }
        ask(&env, body)
    };
    let (st, b) = ask_model(Some("sonnet"));
    assert_eq!(st, 200, "{b}");
    assert!(b["answer"].as_str().expect("answer").starts_with("ARGS: [-p] [--output-format] [text] [--model] [sonnet] ["), "{b}");
    let (_, b) = ask_model(None);
    assert!(b["answer"].as_str().expect("answer").contains("[--model] [opus]"), "the role default counts without a request: {b}");
    let (st, b) = ask(&env, json!({"from": "shell", "to": "aider", "message": "oi", "model": "gpt"}));
    assert_eq!((st, b), (400, json!({"error": "the agent aider does not accept choosing a model"})));
    env.core.shutdown();
}

/// With the real CLIs installed: `cargo test -p agentdeck-core --test bridge -- --ignored --nocapture`.
#[test]
#[ignore = "uses the agy and opencode installed on the machine"]
fn the_models_inventory_of_this_machine() {
    let tmp = tempfile::tempdir().expect("tmp");
    let paths = paths_in(tmp.path());
    let opts = StartOptions {
        search_path: None,
        home_dir: None,
        watch_workspace: false,
        config: Some(Config::with_project(paths.default_workspace.clone())),
        system_dirs: true,
    };
    let (core, _rx) = Core::start_with(paths, opts).expect("start");
    let started = std::time::Instant::now();
    let items = core.models_inventory(None).expect("inventory");
    println!("{}\n({:?})", agentdeck_core::models::format_inventory(&items, &core.quota_lines()), started.elapsed());
    core.shutdown();
}

/// With the real CLIs installed: `cargo test -p agentdeck-core --test bridge -- --ignored --nocapture`.
#[test]
#[ignore = "uses the agy and opencode installed on the machine"]
fn the_known_models_come_from_the_installed_clis() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().to_path_buf();
    let paths = paths_in(&root);
    let opts = StartOptions {
        search_path: None,
        home_dir: None,
        watch_workspace: false,
        config: Some(Config::with_project(paths.default_workspace.clone())),
        system_dirs: true,
    };
    let (core, _rx) = Core::start_with(paths, opts).expect("start");
    assert_eq!(core.known_models("claude"), ["opus", "sonnet", "haiku", "fable"]);
    assert!(core.known_models("shell").is_empty());
    for kind in ["antigravity", "opencode"] {
        let started = std::time::Instant::now();
        let list = core.known_models(kind);
        println!("{kind}: {} models in {:?}; first ones: {:?}", list.len(), started.elapsed(), &list[..list.len().min(4)]);
        let again = std::time::Instant::now();
        assert_eq!(core.known_models(kind), list);
        assert!(again.elapsed() < Duration::from_millis(50), "the second query comes from the cache");
    }
    core.shutdown();
}
