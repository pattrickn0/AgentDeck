//! Testes de ponta a ponta da ponte HTTP, do executor headless e da mesa, com `Core`
//! em pastas temporárias e (no Unix) um "claude" falso no PATH de busca.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::time::Duration;

use commanddeck_core::paths::AppPaths;
use commanddeck_core::{Config, Core, CoreEvent, StartOptions};
#[cfg(unix)]
use commanddeck_core::{FeedKind, TableRequest};
use crossbeam_channel::Receiver;
use serde_json::{json, Value};

struct Env {
    _tmp: tempfile::TempDir,
    core: Core,
    #[cfg_attr(not(unix), allow(dead_code))]
    rx: Receiver<CoreEvent>,
    root: std::path::PathBuf,
}

fn start(fake_agents: &[(&str, &str)]) -> Env {
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
    for (name, script) in fake_agents {
        write_script(&fake_bin.join(name), script);
    }
    let opts = StartOptions {
        search_path: Some(fake_bin.into_os_string()),
        home_dir: Some(root.join("home")),
        ollama_discovery: false,
        watch_workspace: false,
        config: Some(Config::default_for(&paths)),
    };
    let (core, rx) = Core::start_with(paths, opts).expect("start");
    Env { _tmp: tmp, core, rx, root }
}

#[cfg(unix)]
fn write_script(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("write");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

#[cfg(not(unix))]
fn write_script(_path: &Path, _body: &str) {}

/// Cliente HTTP mínimo (permite forjar o cabeçalho Host).
fn http(port: u16, method: &str, path: &str, host: &str, token: &str, body: &str) -> (u16, Value) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(60))).expect("timeout");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nx-commanddeck-token: {token}\r\ncontent-type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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

/// Agentes pedidos à UI (`EnsureAgentTab`) desde a última chamada.
#[cfg(unix)]
fn ensure_events(env: &Env) -> Vec<String> {
    env.rx
        .try_iter()
        .filter_map(|ev| match ev {
            CoreEvent::EnsureAgentTab { agent } => Some(agent),
            _ => None,
        })
        .collect()
}

fn ask(env: &Env, body: Value) -> (u16, Value) {
    let port = env.core.bridge_port();
    http(port, "POST", "/ask", &format!("127.0.0.1:{port}"), env.core.bridge_token(), &body.to_string())
}

#[test]
fn auth_and_validation() {
    let env = start(&[("claude", "cat >/dev/null; echo ok")]);
    let port = env.core.bridge_port();
    let token = env.core.bridge_token().to_owned();
    assert_eq!(token.len(), 32);
    assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    let host = format!("127.0.0.1:{port}");

    let (st, b) = http(port, "GET", "/agents", &host, "errado", "");
    assert_eq!((st, b), (403, json!({"error": "token inválido"})));
    let (st, b) = http(port, "GET", "/agents", "evil.example:80", &token, "");
    assert_eq!((st, b), (403, json!({"error": "host inválido"})));
    let (st, _) = http(port, "GET", "/agents", &format!("localhost:{port}"), &token, "");
    assert_eq!(st, 200);
    let (st, b) = http(port, "GET", "/nada", &host, &token, "");
    assert_eq!((st, b), (404, json!({"error": "rota desconhecida"})));
    let (st, b) = http(port, "POST", "/ask", &host, &token, "{oops");
    assert_eq!((st, b), (400, json!({"error": "JSON inválido"})));

    let (st, b) = http(port, "GET", "/agents", &host, &token, "");
    assert_eq!(st, 200);
    let agents = b["agents"].as_array().expect("lista").clone();
    if cfg!(unix) {
        assert_eq!(agents, vec![json!("claude")]);
    }

    let (st, b) = ask(&env, json!({"from": "shell", "to": "bogus", "message": "oi"}));
    assert_eq!(st, 400);
    let err = b["error"].as_str().expect("erro");
    assert!(err.starts_with("agente desconhecido ou não instalado \"bogus\". Disponíveis: "), "{err}");

    if cfg!(unix) {
        let (st, b) = ask(&env, json!({"from": "claude", "to": "ClaudeCode", "message": "oi"}));
        assert_eq!((st, b), (400, json!({"error": "um agente não pode perguntar a si mesmo"})));
        let (st, b) = ask(&env, json!({"from": "shell", "to": "claude", "message": "oi", "depth": 2}));
        assert_eq!((st, b), (400, json!({"error": "profundidade máxima de conversa entre agentes atingida"})));
    }
    env.core.shutdown();
}

#[test]
fn workspace_agents_md_and_spawn_specs() {
    let env = start(&[("claude", "echo ok")]);
    let ws = env.core.workspace();
    assert_eq!(ws, env.root.join("workspace"));
    let md = std::fs::read_to_string(ws.join("AGENTS.md")).expect("AGENTS.md criado no workspace do app");
    assert!(md.starts_with("<!-- commanddeck:start -->\nVocê está rodando dentro do CommandDeck Terminal"));
    assert!(md.contains("Seu nome aqui está na variável de ambiente COMMANDDECK_AGENT"));

    let shell = env.core.spawn_spec("shell").expect("shell");
    assert_eq!(shell.label, "Terminal");
    assert_eq!(shell.cwd, ws);
    assert_eq!(shell.env["COMMANDDECK_AGENT"], "shell");
    assert_eq!(shell.env["COMMANDDECK_PORT"], env.core.bridge_port().to_string());
    assert!(shell.env["PATH"].starts_with(&env.root.join("bin").display().to_string()));
    assert!(!shell.double_enter_on_mention);

    assert_eq!(env.core.spawn_spec("nada").expect_err("desconhecido"), "Agente desconhecido: nada");
    let missing = env.core.spawn_spec("antigravity").expect_err("não instalado");
    assert!(missing.starts_with("Antigravity CLI (agy) não encontrado."));
    assert!(env.core.spawn_spec("ollama").expect_err("sem ollama").starts_with("Ollama CLI não encontrado."));

    #[cfg(unix)]
    {
        let claude = env.core.spawn_spec("claude").expect("claude");
        assert_eq!(claude.program, env.root.join("fake-bin").join("claude"));
        assert_eq!(claude.args[0], "--append-system-prompt");
        assert!(claude.args[1].contains("Seu nome aqui é \"claude\"."));
        assert_eq!(&claude.args[2..], ["--add-dir".to_owned(), env.root.join("dados").display().to_string()]);
        assert!(claude.double_enter_on_mention);
        assert_eq!(claude.label, "Claude");
    }

    let list = env.core.list_agents();
    let ids: Vec<&str> = list.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, ["claude", "antigravity", "ollama", "gemini", "aider", "goose", "copilot", "q", "interpreter", "shell"]);
    let ollama = &list[2];
    assert!(!ollama.installed && !ollama.peer && ollama.label == "Ollama");
    let shell = list.last().expect("shell");
    assert!(shell.installed && !shell.peer);
    env.core.shutdown();
}

#[cfg(unix)]
const ECHO_AGENT: &str = r#"printf 'ARGS:'; for a in "$@"; do case "$a" in *CommandDeck*) printf ' [<sys>]';; *) printf ' [%s]' "$a";; esac; done; echo; echo "ENV: $COMMANDDECK_AGENT $COMMANDDECK_DEPTH"; cat"#;

#[cfg(unix)]
#[test]
fn ask_runs_headless_with_sessions() {
    let env = start(&[("claude", ECHO_AGENT)]);
    let (st, b) = ask(&env, json!({"from": "shell", "to": "claude", "message": "Qual a capital?"}));
    assert_eq!(st, 200, "{b}");
    let answer = b["answer"].as_str().expect("answer");
    assert!(answer.starts_with("ARGS: [-p] [--output-format] [text] [--session-id] ["), "{answer}");
    assert!(answer.contains("[--append-system-prompt] [<sys>] [--add-dir]"), "{answer}");
    assert!(answer.contains("ENV: claude 1"), "{answer}");
    assert!(answer.contains(
        "[Mensagem do agente \"shell\" recebida pelo CommandDeck]\n\nQual a capital?\n\nResponda diretamente ao agente \"shell\". O usuário acompanha a conversa integrada em tempo real."
    ));
    let sid = answer.split("[--session-id] [").nth(1).and_then(|s| s.split(']').next()).expect("sid").to_owned();

    // Segunda mensagem do mesmo par retoma a sessão.
    let (st, b) = ask(&env, json!({"from": "shell", "to": "claude", "message": "E agora?"}));
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
    assert_eq!((feed[0].from.as_deref(), feed[0].to.as_deref(), feed[0].text.as_str()), (Some("shell"), Some("claude"), "Qual a capital?"));
    assert_eq!(feed[1].kind, FeedKind::Answer);
    assert_eq!((feed[1].from.as_deref(), feed[1].to.as_deref()), (Some("claude"), Some("shell")));
    env.core.shutdown();
}

#[cfg(unix)]
#[test]
fn failing_resumed_session_retries_and_errors_report_tail() {
    // Falha sempre que receber --resume; senão responde. Erros vão para o stderr.
    let script = r#"for a in "$@"; do [ "$a" = "--resume" ] && { echo "sessão expirada" >&2; exit 1; }; done; cat >/dev/null; echo novo"#;
    let env = start(&[("claude", script), ("gemini", "echo l1 >&2; echo l2 >&2; exit 3")]);
    assert_eq!(ask(&env, json!({"from": "shell", "to": "claude", "message": "a"})).1["answer"], "novo");
    // Sessão "expira": a ponte descarta e tenta de novo, limpa.
    assert_eq!(ask(&env, json!({"from": "shell", "to": "claude", "message": "b"})).1["answer"], "novo");

    let (st, b) = ask(&env, json!({"from": "shell", "to": "gemini", "message": "x"}));
    assert_eq!((st, b), (500, json!({"error": "l1\nl2"})));
    env.core.shutdown();
}

#[cfg(unix)]
#[test]
fn same_pair_requests_are_serialized() {
    let tmp_log = tempfile::NamedTempFile::new().expect("tmp");
    let log = tmp_log.path().display().to_string();
    let script = format!("echo start >> '{log}'; sleep 0.3; echo end >> '{log}'; cat >/dev/null; echo ok");
    let env = std::sync::Arc::new(start(&[("claude", &script)]));
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
    assert_eq!(lines, ["start", "end", "start", "end", "start", "end"], "execuções sobrepostas");
    env.core.shutdown();
}

#[cfg(unix)]
#[test]
fn table_runs_rounds_and_finishes() {
    let env = start(&[("claude", "cat >/dev/null; echo 'contribuição'"), ("gemini", "exit 1")]);
    env.core.run_table(TableRequest {
        task: "Planeje".into(),
        agents: vec!["claude".into(), "gemini".into()],
        rounds: 1,
        attachments: vec!["a.png".into()],
    });
    let events: Vec<CoreEvent> = env.rx.try_iter().collect();
    let feed: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            CoreEvent::Feed(f) => Some((f.kind, f.from.clone().unwrap_or_default(), f.text.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(feed[0], (FeedKind::User, "você".into(), "Planeje\n@a.png".into()));
    assert_eq!(feed[1], (FeedKind::Thinking, "claude".into(), "rodada 1/1…".into()));
    assert_eq!(feed[2], (FeedKind::Table, "claude".into(), "contribuição".into()));
    assert_eq!(feed[3], (FeedKind::Thinking, "gemini".into(), "rodada 1/1…".into()));
    assert_eq!((feed[4].0, feed[4].1.as_str(), feed[4].2.as_str()), (FeedKind::Error, "gemini", "processo saiu com código 1"));
    assert_eq!(feed[5], (FeedKind::System, String::new(), "Mesa encerrada.".into()));
    assert!(matches!(events.last(), Some(CoreEvent::TableFinished)));
    env.core.shutdown();
}

#[cfg(unix)]
#[test]
fn stop_table_kills_running_agent() {
    let env = std::sync::Arc::new(start(&[("claude", "sleep 30; echo tarde")]));
    let e2 = env.clone();
    let h = std::thread::spawn(move || {
        e2.core.run_table(TableRequest { task: "t".into(), agents: vec!["claude".into()], rounds: 3, attachments: vec![] })
    });
    std::thread::sleep(Duration::from_millis(500));
    let t = std::time::Instant::now();
    env.core.stop_table();
    h.join().expect("thread");
    assert!(t.elapsed() < Duration::from_secs(10));
    let texts: Vec<String> = env
        .rx
        .try_iter()
        .filter_map(|e| match e {
            CoreEvent::Feed(f) if f.kind == FeedKind::System => Some(f.text),
            _ => None,
        })
        .collect();
    assert_eq!(texts, ["Mesa interrompida."]);
    env.core.shutdown();
}

#[test]
fn attachments_and_config_persistence() {
    let env = start(&[]);
    let r = env.core.attach_bytes("nota é.txt", b"x").expect("anexo");
    assert!(r.contains(&format!("dados/anexos/s_{}_", env.core.session_id())), "{r}");
    assert!(r.ends_with("-nota_e.txt"));
    env.core.update_config(|c| c.whisper_lang = "en".into());
    let saved = std::fs::read_to_string(env.root.join("cfg").join("config.json")).expect("config salva");
    assert!(saved.contains("\"whisperLang\": \"en\""));
    // Sessão registrada.
    let sessions = std::fs::read_to_string(env.root.join("dados").join("sessions.json")).expect("sessions.json");
    assert!(sessions.contains(env.core.session_id()));
    assert_eq!(env.core.transcribe_engine(), "indisponível");
    assert!(env.core.transcribe_pcm16k(&[0.0; 160]).is_err());
    env.core.shutdown();
}

#[cfg(unix)]
#[test]
fn ask_asks_the_ui_to_open_the_agent_tab() {
    let env = start(&[("claude", "cat >/dev/null; echo resposta")]);
    let (st, b) = ask(&env, json!({"from": "shell", "to": "claude", "message": "oi"}));
    assert_eq!((st, b["answer"].as_str()), (200, Some("resposta")));
    assert_eq!(ensure_events(&env), vec!["claude".to_owned()]);

    // Com a opção desligada nada é pedido à UI (a resposta continua saindo em headless).
    env.core.update_config(|c| c.auto_open_agent_tabs = false);
    let (st, _) = ask(&env, json!({"from": "shell", "to": "claude", "message": "de novo"}));
    assert_eq!(st, 200);
    assert!(ensure_events(&env).is_empty());

    // Pedido inválido (agente desconhecido) também não abre aba.
    env.core.update_config(|c| c.auto_open_agent_tabs = true);
    let (st, _) = ask(&env, json!({"from": "shell", "to": "bogus", "message": "oi"}));
    assert_eq!(st, 400);
    assert!(ensure_events(&env).is_empty());
    env.core.shutdown();
}
