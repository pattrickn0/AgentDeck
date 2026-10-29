//! End-to-end smoke test: starts a `Core` in temporary folders and calls the `ask-agent` binary
//! against the real bridge.

use std::io::Write;
use std::process::{Command, Output, Stdio};

use agentdeck_core::paths::AppPaths;
use agentdeck_core::{Config, Core, StartOptions};

fn start_core(root: &std::path::Path) -> Core {
    let paths = AppPaths {
        app_dir: root.to_path_buf(),
        bin_dir: root.join("bin"),
        data_dir: root.join("data"),
        default_workspace: root.join("workspace"),
        whisper_dir: root.join("whisper"),
        config_file: root.join("cfg").join("config.json"),
    };
    let fake_bin = root.join("fake-bin");
    std::fs::create_dir_all(&fake_bin).expect("mkdir");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let script = fake_bin.join("claude");
        std::fs::write(&script, "#!/bin/sh\necho \"claude answered (from $AGENTDECK_AGENT)\"; cat >/dev/null\n").expect("write");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    let opts = StartOptions {
        search_path: Some(fake_bin.into_os_string()),
        home_dir: Some(root.join("home")),
        watch_workspace: false,
        config: Some(Config::with_project(paths.default_workspace.clone())),
        system_dirs: false,
    };
    Core::start_with(paths, opts).expect("start").0
}

/// Removes the app's variables from the test's environment (the test may run inside a tab).
fn scrub_app_env(cmd: &mut Command) {
    for name in ["PORT", "TOKEN", "AGENT", "DEPTH", "ASK_WAIT"] {
        cmd.env_remove(format!("AGENTDECK_{name}"));
    }
}

fn ask_agent(core: Option<&Core>, args: &[&str], stdin: Option<&str>) -> Output {
    ask_agent_bytes(core, args, stdin.unwrap_or("").as_bytes())
}

fn ask_agent_bytes(core: Option<&Core>, args: &[&str], stdin: &[u8]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ask-agent"));
    cmd.args(args);
    scrub_app_env(&mut cmd);
    if let Some(core) = core {
        cmd.env("AGENTDECK_PORT", core.bridge_port().to_string())
            .env("AGENTDECK_TOKEN", core.bridge_token())
            .env("AGENTDECK_AGENT", "shell");
    }
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn ask-agent");
    if let Some(mut pipe) = child.stdin.take() {
        let _ = pipe.write_all(stdin);
    }
    child.wait_with_output().expect("output")
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[test]
fn outside_agentdeck_exits_2() {
    let out = ask_agent(None, &["--list"], None);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(text(&out.stderr).trim(), "ask-agent: run this command from inside an AgentDeck terminal.");
}

/// The old name's prefix (`COMMANDDECK_*`) no longer works: without `AGENTDECK_*`, `ask-agent` cannot find the bridge.
#[test]
fn old_commanddeck_variables_are_ignored() {
    let tmp = tempfile::tempdir().expect("tmp");
    let core = start_core(tmp.path());
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ask-agent"));
    scrub_app_env(&mut cmd);
    cmd.arg("--list")
        .env("COMMANDDECK_PORT", core.bridge_port().to_string())
        .env("COMMANDDECK_TOKEN", core.bridge_token())
        .env("COMMANDDECK_AGENT", "shell");
    let out = cmd.output().expect("ask-agent");
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    assert_eq!(text(&out.stderr).trim(), "ask-agent: run this command from inside an AgentDeck terminal.");
}

#[test]
fn list_help_and_errors() {
    let tmp = tempfile::tempdir().expect("tmp");
    let core = start_core(tmp.path());

    let out = ask_agent(Some(&core), &["--list"], None);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    if cfg!(unix) {
        assert_eq!(text(&out.stdout), "claude\n");
    }

    let out = ask_agent(Some(&core), &["--help"], None);
    assert_eq!(out.status.code(), Some(0));
    assert!(text(&out.stdout).starts_with("Usage: ask-agent") && text(&out.stdout).contains("--new"));
    let out = ask_agent(Some(&core), &[], None);
    assert_eq!(out.status.code(), Some(2));

    let out = ask_agent(Some(&core), &["bogus", "hi"], None);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).starts_with("ask-agent: unknown or not installed agent \"bogus\""), "{}", text(&out.stderr));

    let out = ask_agent(Some(&core), &["claude", "-"], Some("   \n"));
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(text(&out.stderr).trim(), "ask-agent: empty message.");

    // Stdin that is not UTF-8 (a cp1252 file): becomes text with replacements, no read error.
    let out = ask_agent_bytes(Some(&core), &["bogus", "-"], b"a\xe7\xe3o");
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("unknown or not installed agent"), "{}", text(&out.stderr));
    // Without a message, stdin (a pipe here) is the message.
    let out = ask_agent(Some(&core), &["bogus"], Some("from stdin"));
    assert!(text(&out.stderr).contains("unknown or not installed agent"), "{}", text(&out.stderr));

    // `--boss` talks to a boss that already exists: `--new` does not combine with it.
    let out = ask_agent(Some(&core), &["--new", "--boss", "hi"], None);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("--new does not combine with --boss"), "{}", text(&out.stderr));

    #[cfg(unix)]
    {
        let out = ask_agent(Some(&core), &["claude", "What", "is", "the", "idea?"], None);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        assert_eq!(text(&out.stdout), "claude answered (from claude)\n");
        let out = ask_agent(Some(&core), &["agy"], Some("from stdin"));
        assert_eq!(out.status.code(), Some(1));
        let out = ask_agent(Some(&core), &["claudecode", "-"], Some("from stdin"));
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    }
    core.shutdown();
}

#[test]
fn signal_never_fails_and_reply_needs_a_number() {
    let tmp = tempfile::tempdir().expect("tmp");
    let core = start_core(tmp.path());
    // A hook that fails must not get in the agent's way (in Claude, exit 2 on Stop would block the end
    // of the turn): `--signal` exits with 0 even when the bridge refuses it (here, master token) or outside it.
    let out = ask_agent(Some(&core), &["--signal", "idle", "{\"type\":\"agent-turn-complete\"}"], None);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(ask_agent(None, &["--signal", "busy"], None).status.code(), Some(0), "not even outside AgentDeck");

    let out = ask_agent(Some(&core), &["--reply", "x", "hi"], None);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("--reply needs the question number"), "{}", text(&out.stderr));
    let out = ask_agent(Some(&core), &["--reply", "#41", "hi"], None);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("question #41 is not waiting for an answer"), "{}", text(&out.stderr));
    core.shutdown();
}

/// `ask-agent --usage` the way Claude Code calls it, with the `config` settings folder.
fn statusline(core: Option<&Core>, token: Option<&str>, config: &std::path::Path, stdin: &str) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ask-agent"));
    scrub_app_env(&mut cmd);
    cmd.arg("--usage").env("CLAUDE_CONFIG_DIR", config).current_dir(config);
    if let (Some(core), Some(token)) = (core, token) {
        cmd.env("AGENTDECK_PORT", core.bridge_port().to_string()).env("AGENTDECK_TOKEN", token);
    }
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn ask-agent");
    let _ = child.stdin.take().expect("stdin").write_all(stdin.as_bytes());
    child.wait_with_output().expect("output")
}

#[test]
fn usage_statusline_posts_and_prints() {
    let tmp = tempfile::tempdir().expect("tmp");
    let core = start_core(tmp.path());
    let config = tmp.path().join("claude-config");
    std::fs::create_dir_all(&config).expect("mkdir");
    let input = serde_json::json!({
        "workspace": { "project_dir": config },
        "rate_limits": {
            "five_hour": { "used_percentage": 34.4, "resets_at": 1_760_000_000 },
            "seven_day": { "used_percentage": 12, "resets_at": 1_760_500_000 }
        }
    })
    .to_string();

    // Without the bridge (outside AgentDeck), it only prints; with an API key (no limits), nothing.
    let out = statusline(None, None, &config, &input);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), "session 34% · week 12%\n");
    let out = statusline(None, None, &config, r#"{"rate_limits":null}"#);
    assert_eq!((out.status.code(), text(&out.stdout)), (Some(0), String::new()));
    assert_eq!(statusline(None, None, &config, "junk").status.code(), Some(0));

    // In a Claude tab, the usage reaches the bridge.
    if cfg!(windows) {
        let bin = tmp.path().join("home").join(".local").join("bin");
        std::fs::create_dir_all(&bin).expect("mkdir");
        std::fs::write(bin.join("claude.exe"), b"").expect("write");
    }
    let ident = agentdeck_core::Ident::parse("claude@dev").expect("id");
    let token = core.spawn_spec_for(&core.active_project_id(), &ident).expect("spec").env["AGENTDECK_TOKEN"].clone();
    let out = statusline(Some(&core), Some(&token), &config, &input);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let usage = core.usage_snapshot();
    let claude = usage.get("claude").expect("claude usage");
    assert_eq!(claude.session.map(|w| (w.used, w.resets_at_ms)), Some((0.344, Some(1_760_000_000_000))));
    assert_eq!(claude.weekly.map(|w| w.used), Some(0.12));
    // Token refused by the bridge: it still exits with 0.
    assert_eq!(statusline(Some(&core), Some("wrong"), &config, &input).status.code(), Some(0));

    // The user's statusline (here, the project's) receives the same stdin and its output is the one shown.
    let dot = config.join(".claude");
    std::fs::create_dir_all(&dot).expect("mkdir");
    let user_cmd = if cfg!(windows) { "echo my line" } else { "read x; echo \"my line ${#x}\"" };
    std::fs::write(dot.join("settings.json"), serde_json::json!({ "statusLine": { "type": "command", "command": user_cmd } }).to_string())
        .expect("write");
    let out = statusline(None, None, &config, &input);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    if cfg!(unix) {
        assert_eq!(text(&out.stdout), format!("my line {}\n", input.len()));
    } else {
        assert_eq!(text(&out.stdout).trim(), "my line");
    }
    // Ours, if it is in the settings, does not run itself.
    std::fs::write(dot.join("settings.json"), r#"{"statusLine":{"type":"command","command":"ask-agent --usage"}}"#).expect("write");
    assert_eq!(text(&statusline(None, None, &config, &input).stdout), "session 34% · week 12%\n");
    core.shutdown();
}

#[test]
fn the_model_option_is_checked_and_does_not_mix_with_boss_or_reply() {
    let tmp = tempfile::tempdir().expect("tmp");
    let core = start_core(tmp.path());

    let out = ask_agent(Some(&core), &["--model"], None);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("--model needs the model name"), "{}", text(&out.stderr));
    let out = ask_agent(Some(&core), &["--model", "sonnet", "--boss", "hi"], None);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("--model does not combine with --boss"), "{}", text(&out.stderr));
    let out = ask_agent(Some(&core), &["--model=sonnet", "--reply", "3", "hi"], None);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("--model does not combine with --reply"), "{}", text(&out.stderr));

    // Name out of the pattern: the bridge refuses it before anything else (400 → exit 1).
    let out = ask_agent(Some(&core), &["--new", "--model", "a b", "claude", "hi"], None);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("invalid model: \"a b\""), "{}", text(&out.stderr));
    // The help mentions the option.
    let out = ask_agent(Some(&core), &["--help"], None);
    assert!(text(&out.stdout).contains("[--model <model>]"), "{}", text(&out.stdout));

    #[cfg(unix)]
    {
        // With no tabs, the question is answered in headless mode and the model reaches the fake claude without errors.
        let out = ask_agent(Some(&core), &["--model", "sonnet", "claude", "hi"], None);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    }
    core.shutdown();
}

/// `ask-agent --signal session` the way Claude's `SessionStart` hook calls it: with the tab's token and the JSON on stdin.
fn session_hook(core: &Core, token: &str, args: &[&str], stdin: &str) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ask-agent"));
    scrub_app_env(&mut cmd);
    cmd.args(["--signal", "session"]).args(args);
    cmd.env("AGENTDECK_PORT", core.bridge_port().to_string()).env("AGENTDECK_TOKEN", token);
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn ask-agent");
    let _ = child.stdin.take().expect("stdin").write_all(stdin.as_bytes());
    child.wait_with_output().expect("output")
}

#[test]
fn the_session_hook_saves_the_new_conversation_id_of_the_tab() {
    let tmp = tempfile::tempdir().expect("tmp");
    let core = start_core(tmp.path());
    if cfg!(windows) {
        let bin = tmp.path().join("home").join(".local").join("bin");
        std::fs::create_dir_all(&bin).expect("mkdir");
        std::fs::write(bin.join("claude.exe"), b"").expect("write");
    }
    let project = core.active_project_id();
    let ident = agentdeck_core::Ident::parse("claude@dev").expect("id");
    let token = core.spawn_spec_for(&project, &ident).expect("spec").env["AGENTDECK_TOKEN"].clone();
    let first = core.tab_session(&project, "claude@dev").expect("saved by the first launch");

    // `/clear`: Claude calls the hook with the new id in the JSON on stdin.
    let json = r#"{"session_id":"7f0c1a52-1111-4222-8333-944455556666","hook_event_name":"SessionStart","source":"clear"}"#;
    let out = session_hook(&core, &token, &[], json);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(core.tab_session(&project, "claude@dev").as_deref(), Some("7f0c1a52-1111-4222-8333-944455556666"));
    assert_ne!(first, "7f0c1a52-1111-4222-8333-944455556666");
    // Junk on stdin, or the master token: the hook still exits with 0 and nothing changes.
    assert_eq!(session_hook(&core, &token, &[], "junk").status.code(), Some(0));
    assert_eq!(session_hook(&core, core.bridge_token(), &[], json).status.code(), Some(0));
    assert_eq!(core.tab_session(&project, "claude@dev").as_deref(), Some("7f0c1a52-1111-4222-8333-944455556666"));
    // The id can also come as an argument (for CLIs whose hook does not use stdin).
    assert_eq!(session_hook(&core, &token, &["abc-123"], "").status.code(), Some(0));
    assert_eq!(core.tab_session(&project, "claude@dev").as_deref(), Some("abc-123"));
    core.shutdown();
}
