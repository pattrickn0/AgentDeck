//! Smoke test de ponta a ponta: sobe um `Core` em pastas temporárias e chama o binário
//! `ask-agent` contra a ponte real.

use std::io::Write;
use std::process::{Command, Output, Stdio};

use commanddeck_core::paths::AppPaths;
use commanddeck_core::{Config, Core, StartOptions};

fn start_core(root: &std::path::Path) -> Core {
    let paths = AppPaths {
        app_dir: root.to_path_buf(),
        bin_dir: root.join("bin"),
        data_dir: root.join("dados"),
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
        std::fs::write(&script, "#!/bin/sh\necho \"claude respondeu (de $COMMANDDECK_AGENT)\"; cat >/dev/null\n").expect("write");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    let opts = StartOptions {
        search_path: Some(fake_bin.into_os_string()),
        home_dir: Some(root.join("home")),
        ollama_discovery: false,
        watch_workspace: false,
        config: Some(Config::default_for(&paths)),
    };
    Core::start_with(paths, opts).expect("start").0
}

fn ask_agent(core: Option<&Core>, args: &[&str], stdin: Option<&str>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ask-agent"));
    cmd.args(args)
        .env_remove("COMMANDDECK_PORT")
        .env_remove("COMMANDDECK_TOKEN")
        .env_remove("COMMANDDECK_AGENT")
        .env_remove("COMMANDDECK_DEPTH");
    if let Some(core) = core {
        cmd.env("COMMANDDECK_PORT", core.bridge_port().to_string())
            .env("COMMANDDECK_TOKEN", core.bridge_token())
            .env("COMMANDDECK_AGENT", "shell");
    }
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn ask-agent");
    if let Some(mut pipe) = child.stdin.take() {
        let _ = pipe.write_all(stdin.unwrap_or("").as_bytes());
    }
    child.wait_with_output().expect("output")
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[test]
fn outside_commanddeck_exits_2() {
    let out = ask_agent(None, &["--list"], None);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(text(&out.stderr).trim(), "ask-agent: rode este comando de dentro de um terminal do CommandDeck.");
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
    assert!(text(&out.stdout).starts_with("Uso: ask-agent") && text(&out.stdout).contains("--new"));
    let out = ask_agent(Some(&core), &[], None);
    assert_eq!(out.status.code(), Some(2));

    let out = ask_agent(Some(&core), &["bogus", "oi"], None);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).starts_with("ask-agent: agente desconhecido ou não instalado \"bogus\""), "{}", text(&out.stderr));

    let out = ask_agent(Some(&core), &["claude", "-"], Some("   \n"));
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(text(&out.stderr).trim(), "ask-agent: mensagem vazia.");

    #[cfg(unix)]
    {
        let out = ask_agent(Some(&core), &["claude", "Qual", "a", "ideia?"], None);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        assert_eq!(text(&out.stdout), "claude respondeu (de claude)\n");
        let out = ask_agent(Some(&core), &["agy"], Some("pelo stdin"));
        assert_eq!(out.status.code(), Some(1));
        let out = ask_agent(Some(&core), &["claudecode", "-"], Some("pelo stdin"));
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    }
    core.shutdown();
}
