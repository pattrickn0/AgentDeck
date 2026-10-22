//! Smoke test de ponta a ponta: sobe um `Core` em pastas temporárias e chama o binário
//! `ask-agent` contra a ponte real.

use std::io::Write;
use std::process::{Command, Output, Stdio};

use agentdeck_core::paths::AppPaths;
use agentdeck_core::{Config, Core, StartOptions};

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
        std::fs::write(&script, "#!/bin/sh\necho \"claude respondeu (de $AGENTDECK_AGENT)\"; cat >/dev/null\n").expect("write");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    let opts = StartOptions {
        search_path: Some(fake_bin.into_os_string()),
        home_dir: Some(root.join("home")),
        ollama_discovery: false,
        watch_workspace: false,
        config: Some(Config::with_project(paths.default_workspace.clone())),
        system_dirs: false,
    };
    Core::start_with(paths, opts).expect("start").0
}

/// Tira do ambiente do teste as variáveis do app, dos dois nomes (o teste pode rodar dentro de uma aba).
fn scrub_app_env(cmd: &mut Command) {
    for name in ["PORT", "TOKEN", "AGENT", "DEPTH", "ASK_WAIT"] {
        cmd.env_remove(format!("AGENTDECK_{name}")).env_remove(format!("COMMANDDECK_{name}"));
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
    assert_eq!(text(&out.stderr).trim(), "ask-agent: rode este comando de dentro de um terminal do AgentDeck.");
}

/// Uma aba aberta pela versão antiga tem só `COMMANDDECK_*` no ambiente: o `ask-agent` continua
/// falando com a ponte.
#[test]
fn old_commanddeck_variables_still_work() {
    let tmp = tempfile::tempdir().expect("tmp");
    let core = start_core(tmp.path());
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ask-agent"));
    scrub_app_env(&mut cmd);
    cmd.arg("--list")
        .env("COMMANDDECK_PORT", core.bridge_port().to_string())
        .env("COMMANDDECK_TOKEN", core.bridge_token())
        .env("COMMANDDECK_AGENT", "shell");
    let out = cmd.output().expect("ask-agent");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    if cfg!(unix) {
        assert_eq!(text(&out.stdout), "claude
");
    }

    // A nova vence quando as duas existem: com a antiga errada, a nova ainda funciona.
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ask-agent"));
    scrub_app_env(&mut cmd);
    cmd.arg("--list")
        .env("COMMANDDECK_PORT", "1")
        .env("COMMANDDECK_TOKEN", "velho")
        .env("AGENTDECK_PORT", core.bridge_port().to_string())
        .env("AGENTDECK_TOKEN", core.bridge_token());
    assert_eq!(cmd.output().expect("ask-agent").status.code(), Some(0));
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

    // Stdin que não é UTF-8 (arquivo em cp1252): vira texto com substituições, sem erro de leitura.
    let out = ask_agent_bytes(Some(&core), &["bogus", "-"], b"a\xe7\xe3o");
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("agente desconhecido"), "{}", text(&out.stderr));
    // Sem mensagem, o stdin (aqui um pipe) é a mensagem.
    let out = ask_agent(Some(&core), &["bogus"], Some("pelo stdin"));
    assert!(text(&out.stderr).contains("agente desconhecido"), "{}", text(&out.stderr));

    // `--boss` fala com o chefe que já existe: `--new` não combina com ele.
    let out = ask_agent(Some(&core), &["--new", "--boss", "oi"], None);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("--new não combina com --boss"), "{}", text(&out.stderr));

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

#[test]
fn signal_never_fails_and_reply_needs_a_number() {
    let tmp = tempfile::tempdir().expect("tmp");
    let core = start_core(tmp.path());
    // Um hook que falha não pode atrapalhar o agente (no Claude, saída 2 no Stop bloquearia o fim
    // do turno): `--signal` sai com 0 mesmo recusado pela ponte (aqui, token mestre) ou fora dele.
    let out = ask_agent(Some(&core), &["--signal", "idle", "{\"type\":\"agent-turn-complete\"}"], None);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(ask_agent(None, &["--signal", "busy"], None).status.code(), Some(0), "nem fora do AgentDeck");

    let out = ask_agent(Some(&core), &["--reply", "x", "oi"], None);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("--reply precisa do número da pergunta"), "{}", text(&out.stderr));
    let out = ask_agent(Some(&core), &["--reply", "#41", "oi"], None);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("a pergunta #41 não está esperando resposta"), "{}", text(&out.stderr));
    core.shutdown();
}

/// `ask-agent --usage` como o Claude Code o chama, com a pasta de configuração `config`.
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

    // Sem ponte (fora do AgentDeck), só imprime; com chave de API (sem limites), nada.
    let out = statusline(None, None, &config, &input);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), "sessão 34% · semana 12%\n");
    let out = statusline(None, None, &config, r#"{"rate_limits":null}"#);
    assert_eq!((out.status.code(), text(&out.stdout)), (Some(0), String::new()));
    assert_eq!(statusline(None, None, &config, "lixo").status.code(), Some(0));

    // Numa aba do Claude, o uso chega à ponte.
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
    let claude = usage.get("claude").expect("uso do claude");
    assert_eq!(claude.session.map(|w| (w.used, w.resets_at_ms)), Some((0.344, Some(1_760_000_000_000))));
    assert_eq!(claude.weekly.map(|w| w.used), Some(0.12));
    // Token recusado pela ponte: sai com 0 do mesmo jeito.
    assert_eq!(statusline(Some(&core), Some("errado"), &config, &input).status.code(), Some(0));

    // A statusline do usuário (aqui, a do projeto) recebe o mesmo stdin e a saída dela é a mostrada.
    let dot = config.join(".claude");
    std::fs::create_dir_all(&dot).expect("mkdir");
    let user_cmd = if cfg!(windows) { "echo minha linha" } else { "read x; echo \"minha linha ${#x}\"" };
    std::fs::write(dot.join("settings.json"), serde_json::json!({ "statusLine": { "type": "command", "command": user_cmd } }).to_string())
        .expect("write");
    let out = statusline(None, None, &config, &input);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    if cfg!(unix) {
        assert_eq!(text(&out.stdout), format!("minha linha {}\n", input.len()));
    } else {
        assert_eq!(text(&out.stdout).trim(), "minha linha");
    }
    // A nossa própria, se estiver nas configurações, não roda a si mesma.
    std::fs::write(dot.join("settings.json"), r#"{"statusLine":{"type":"command","command":"ask-agent --usage"}}"#).expect("write");
    assert_eq!(text(&statusline(None, None, &config, &input).stdout), "sessão 34% · semana 12%\n");
    core.shutdown();
}

#[test]
fn the_model_option_is_checked_and_does_not_mix_with_boss_or_reply() {
    let tmp = tempfile::tempdir().expect("tmp");
    let core = start_core(tmp.path());

    let out = ask_agent(Some(&core), &["--model"], None);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("--model precisa do nome do modelo"), "{}", text(&out.stderr));
    let out = ask_agent(Some(&core), &["--model", "sonnet", "--boss", "oi"], None);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("--model não combina com --boss"), "{}", text(&out.stderr));
    let out = ask_agent(Some(&core), &["--model=sonnet", "--reply", "3", "oi"], None);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("--model não combina com --reply"), "{}", text(&out.stderr));

    // Nome fora do padrão: a ponte recusa antes de qualquer coisa (400 → saída 1).
    let out = ask_agent(Some(&core), &["--new", "--model", "a b", "claude", "oi"], None);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("modelo inválido: \"a b\""), "{}", text(&out.stderr));
    // A ajuda cita a opção.
    let out = ask_agent(Some(&core), &["--help"], None);
    assert!(text(&out.stdout).contains("[--model <modelo>]"), "{}", text(&out.stdout));

    #[cfg(unix)]
    {
        // Sem abas, a pergunta é respondida em headless e o modelo chega ao claude falso sem erro.
        let out = ask_agent(Some(&core), &["--model", "sonnet", "claude", "oi"], None);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    }
    core.shutdown();
}
