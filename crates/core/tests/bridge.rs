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
        system_dirs: false,
    };
    let (core, rx) = Core::start_with(paths, opts).expect("start");
    Env { _tmp: tmp, core, rx, root }
}

/// Pastas do app dentro de `root` (como em `start`).
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

/// Token que a aba da instância `id` recebe no ambiente.
fn tab_token(env: &Env, id: &str) -> String {
    let ident = commanddeck_core::Ident::parse(id).expect("id");
    env.core.spawn_spec_for(&ident).expect("spec").env["COMMANDDECK_TOKEN"].clone()
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

/// Chamada à ponte que pode ir para outra thread (só porta e token).
#[cfg(unix)]
fn ask_raw(port: u16, token: &str, body: Value) -> (u16, Value) {
    http(port, "POST", "/ask", &format!("127.0.0.1:{port}"), token, &body.to_string())
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
    // Cada aba tem o próprio token (o mestre não vai para os agentes), estável enquanto ela existir.
    let token = shell.env["COMMANDDECK_TOKEN"].clone();
    assert_ne!(token, env.core.bridge_token());
    assert_eq!(token.len(), 32);
    assert_eq!(env.core.spawn_spec("shell").expect("de novo").env["COMMANDDECK_TOKEN"], token);
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
        assert!(std::fs::read_to_string(&hooks).expect("hooks gravados").contains("ask-agent --signal idle"));
        assert!(claude.double_enter_on_mention);
        assert_eq!(claude.label, "Claude");
    }

    let list = env.core.list_agents();
    let ids: Vec<&str> = list.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, ["claude", "antigravity", "ollama", "codex", "opencode", "aider", "goose", "copilot", "q", "interpreter", "shell"]);
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
    let env = start(&[("claude", script), ("aider", "echo l1 >&2; echo l2 >&2; exit 3")]);
    assert_eq!(ask(&env, json!({"from": "shell", "to": "claude", "message": "a"})).1["answer"], "novo");
    // Sessão "expira": a ponte descarta e tenta de novo, limpa.
    assert_eq!(ask(&env, json!({"from": "shell", "to": "claude", "message": "b"})).1["answer"], "novo");

    let (st, b) = ask(&env, json!({"from": "shell", "to": "aider", "message": "x"}));
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
    let env = start(&[("claude", "cat >/dev/null; echo 'contribuição'"), ("aider", "exit 1")]);
    env.core.run_table(TableRequest {
        task: "Planeje".into(),
        agents: vec!["claude".into(), "aider".into()],
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
    assert_eq!(feed[3], (FeedKind::Thinking, "aider".into(), "rodada 1/1…".into()));
    assert_eq!((feed[4].0, feed[4].1.as_str(), feed[4].2.as_str()), (FeedKind::Error, "aider", "processo saiu com código 1"));
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

#[test]
fn spawn_spec_for_an_instance_carries_its_identity() {
    let env = start(&[]);
    let ident = commanddeck_core::Ident::parse("shell@qa@ana").expect("id");
    let spec = env.core.spawn_spec_for(&ident).expect("shell sempre existe");
    assert_eq!(spec.kind, "shell");
    assert_eq!(spec.env["COMMANDDECK_AGENT"], "shell@qa@ana");
    // `shell@main@ana` não tem cargo: sem COMMANDDECK_ROLE.
    let main = env.core.spawn_spec_for(&commanddeck_core::Ident::parse("shell@main@bia").expect("id")).expect("spec");
    assert_eq!(main.env["COMMANDDECK_AGENT"], "shell@main@bia");
    assert!(!main.env.contains_key("COMMANDDECK_ROLE"));
    env.core.shutdown();
}

#[test]
fn instances_are_registered_named_and_forgotten() {
    let env = start(&[]);
    assert_eq!(env.core.register_instance("claude").id(), "claude");
    assert_eq!(env.core.register_instance("claude").id(), "claude@main@ana");
    assert_eq!(env.core.restore_instance("claude@dev").map(|i| i.id()).as_deref(), Some("claude@dev"));
    assert!(env.core.restore_instance("claude@Dev!").is_none());
    assert_eq!(env.core.instance_ids(), vec!["claude", "claude@main@ana", "claude@dev"]);
    env.core.close_instance("claude@main@ana");
    assert_eq!(env.core.register_instance("claude").id(), "claude@main@ana", "o nome é reaproveitado");
    env.core.shutdown();
}

/// Próxima entrega pedida à interface (ignora os outros eventos).
#[cfg(unix)]
fn next_delivery(env: &Env) -> commanddeck_core::Delivery {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match env.rx.recv_timeout(left) {
            Ok(CoreEvent::Deliver(d)) => return d,
            Ok(_) => continue,
            Err(_) => panic!("nenhuma entrega pedida à interface"),
        }
    }
}

#[cfg(unix)]
#[test]
fn a_question_is_typed_into_the_tab_and_answered_by_callback() {
    let env = start(&[("claude", "echo headless")]);
    env.core.enable_tab_delivery();
    let (port, token) = (env.core.bridge_port(), env.core.bridge_token().to_owned());

    let asker = std::thread::spawn({
        let token = token.clone();
        move || ask_raw(port, &token, json!({"from": "shell", "to": "claude", "message": "qual a capital da França?"}))
    });
    let d = next_delivery(&env);
    assert_eq!((d.ident.id().as_str(), d.create, d.from.as_str()), ("claude", true, "shell"));
    assert!(d.prompt.contains("qual a capital da França?"));
    assert!(d.prompt.contains(&format!("ask-agent --reply {} ", d.request)), "ensina como responder: {}", d.prompt);
    assert!(!d.answer);

    // A aba (claude) responde chamando `ask-agent shell "Paris"`: isso conclui a pergunta de quem espera.
    let (st, b) = ask_raw(port, &token, json!({"from": "claude", "to": "shell", "message": "Paris"}));
    assert_eq!(st, 200);
    assert!(b["answer"].as_str().expect("texto").contains("entregue"));
    let (st, b) = asker.join().expect("thread");
    assert_eq!((st, b["answer"].as_str()), (200, Some("Paris")));
    env.core.shutdown();
}

#[cfg(unix)]
#[test]
fn a_role_opens_a_new_instance_and_new_forces_another() {
    let env = start(&[("claude", "echo headless")]);
    env.core.enable_tab_delivery();
    let (port, token) = (env.core.bridge_port(), env.core.bridge_token().to_owned());
    let spawn_ask = |to: &'static str, new: bool| {
        let token = token.clone();
        std::thread::spawn(move || ask_raw(port, &token, json!({"from": "shell", "to": to, "message": "tarefa", "new": new})))
    };

    let first = spawn_ask("claude@dev", false);
    let d = next_delivery(&env);
    assert_eq!((d.ident.id().as_str(), d.create), ("claude@dev", true));
    let second = spawn_ask("claude@dev", false);
    let d = next_delivery(&env);
    assert_eq!((d.ident.id().as_str(), d.create), ("claude@dev", false), "mesmo cargo: reaproveita a aba");
    let third = spawn_ask("claude@dev", true);
    let d = next_delivery(&env);
    assert_eq!((d.ident.id().as_str(), d.create), ("claude@dev@ana", true), "--new: outra instância, com nome");

    // Quando as abas fecham, quem esperava recebe o erro (e nenhuma thread fica presa).
    env.core.close_instance("claude@dev");
    env.core.close_instance("claude@dev@ana");
    for t in [first, second, third] {
        let (st, b) = t.join().expect("thread");
        assert_eq!(st, 500);
        assert!(b["error"].as_str().expect("erro").contains("fechada"), "{b}");
    }
    env.core.shutdown();
}

#[cfg(unix)]
#[test]
fn a_failed_delivery_unblocks_the_asker() {
    let env = start(&[("claude", "echo headless")]);
    env.core.enable_tab_delivery();
    let (port, token) = (env.core.bridge_port(), env.core.bridge_token().to_owned());
    let asker = std::thread::spawn(move || ask_raw(port, &token, json!({"from": "shell", "to": "claude", "message": "oi"})));
    let d = next_delivery(&env);
    env.core.fail_delivery(d.request, "a aba não subiu");
    let (st, b) = asker.join().expect("thread");
    assert_eq!((st, b["error"].as_str()), (500, Some("a aba não subiu")));
    env.core.shutdown();
}

#[cfg(unix)]
#[test]
fn wait_cycles_and_unknown_or_ambiguous_targets_are_refused() {
    let env = start(&[("claude", "echo x"), ("agy", "echo x")]);
    env.core.enable_tab_delivery();
    env.core.register_instance("claude"); // a aba principal do Claude
    let (port, token) = (env.core.bridge_port(), env.core.bridge_token().to_owned());
    let call = |from: &'static str, to: &'static str| {
        let token = token.clone();
        std::thread::spawn(move || ask_raw(port, &token, json!({"from": from, "to": to, "message": "oi"})))
    };

    // claude espera antigravity; antigravity espera claude@dev; claude@dev esperar claude fecharia o ciclo.
    let a = call("claude", "antigravity");
    next_delivery(&env);
    let b = call("antigravity", "claude@dev");
    next_delivery(&env);
    let (st, body) = ask_raw(port, &token, json!({"from": "claude@dev", "to": "claude", "message": "oi"}));
    assert_eq!(st, 409, "{body}");
    assert!(body["error"].as_str().expect("erro").contains("ciclo"));

    // Alvo desconhecido e instância ambígua.
    assert_eq!(ask_raw(port, &token, json!({"from": "shell", "to": "bogus", "message": "oi"})).0, 400);
    env.core.restore_instance("claude@qa@ana");
    env.core.restore_instance("claude@qa@bia");
    let (st, body) = ask_raw(port, &token, json!({"from": "shell", "to": "claude@qa", "message": "oi"}));
    assert_eq!(st, 400);
    assert!(body["error"].as_str().expect("erro").contains("ambíguo"), "{body}");

    env.core.shutdown();
    for t in [a, b] {
        let _ = t.join();
    }
}

#[cfg(unix)]
#[test]
fn without_tab_delivery_the_bridge_still_answers_headless() {
    // A interface não ligou a entrega (CLI/testes) ou o usuário desligou a opção: modo headless.
    let env = start(&[("claude", "cat >/dev/null; echo resposta-headless")]);
    let (st, b) = ask(&env, json!({"from": "shell", "to": "claude", "message": "oi"}));
    assert_eq!((st, b["answer"].as_str()), (200, Some("resposta-headless")));

    env.core.enable_tab_delivery();
    env.core.update_config(|c| c.auto_open_agent_tabs = false);
    let (st, b) = ask(&env, json!({"from": "shell", "to": "claude", "message": "de novo"}));
    assert_eq!((st, b["answer"].as_str()), (200, Some("resposta-headless")));
    assert!(env.rx.try_iter().all(|e| !matches!(e, CoreEvent::Deliver(_))));
    env.core.shutdown();
}

#[cfg(unix)]
#[test]
fn reply_route_only_completes_pending_questions() {
    let env = start(&[("claude", "echo x")]);
    env.core.enable_tab_delivery();
    let (port, token) = (env.core.bridge_port(), env.core.bridge_token().to_owned());
    let host = format!("127.0.0.1:{port}");

    // Sem pergunta pendente, /reply não cria nada: 404.
    let body = json!({"from": "claude", "to": "shell", "message": "sozinha"}).to_string();
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
    assert_eq!((st, b["answer"].as_str()), (200, Some("sozinha")));
    // A segunda tentativa (o modelo já tinha respondido) não entrega de novo.
    assert_eq!(http(port, "POST", "/reply", &host, &token, &body).0, 404);
    env.core.shutdown();
}

#[test]
fn the_token_names_the_caller_and_the_body_cannot_forge_it() {
    let env = start(&[]);
    let port = env.core.bridge_port();
    let host = format!("127.0.0.1:{port}");
    let qa = tab_token(&env, "shell@qa");
    // Com o token de uma aba, o `from` do corpo é ignorado: quem fala é a aba.
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
    // Com o token mestre, o `from` precisa ser um id válido.
    let body = json!({"from": "x\u{1b}[201~", "to": "claude", "message": "oi"}).to_string();
    let (st, b) = http(port, "POST", "/ask", &host, env.core.bridge_token(), &body);
    assert_eq!(st, 400);
    assert!(b["error"].as_str().expect("erro").starts_with("remetente inválido"), "{b}");
    // Fechar a aba invalida o token dela.
    env.core.close_instance("shell@qa");
    assert_eq!(http(port, "GET", "/agents", &host, &qa, "").0, 403);
    env.core.shutdown();
}

#[test]
fn requests_without_host_or_too_big_are_refused() {
    let env = start(&[]);
    let port = env.core.bridge_port();
    let token = env.core.bridge_token().to_owned();
    assert_eq!(http(port, "GET", "/agents", "", &token, ""), (403, json!({"error": "host inválido"})));
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
            CoreEvent::AgentSignal { id, busy } => Some((id, busy)),
            _ => None,
        })
        .collect();
    assert_eq!(signals, [("shell@dev".to_owned(), true), ("shell@dev".to_owned(), false)]);
    // Estado inválido, ou sem ser uma aba (token mestre): recusado.
    assert_eq!(http(port, "POST", "/signal", &host, &token, &json!({"state": "x"}).to_string()).0, 400);
    assert_eq!(http(port, "POST", "/signal", &host, env.core.bridge_token(), &json!({"state": "idle"}).to_string()).0, 400);
    env.core.shutdown();
}

#[test]
fn one_app_per_data_dir() {
    let env = start(&[]);
    let second = Core::start_with(
        paths_in(&env.root),
        StartOptions { ollama_discovery: false, watch_workspace: false, system_dirs: false, ..Default::default() },
    );
    let err = second.err().expect("a segunda instância na mesma pasta de dados é recusada").to_string();
    assert!(err.contains("já está usando a pasta de dados"), "{err}");
    env.core.shutdown();
}

#[cfg(unix)]
#[test]
fn headless_runs_get_their_own_short_lived_token() {
    let env = start(&[("claude", r#"cat >/dev/null; echo "$COMMANDDECK_TOKEN""#)]);
    let (st, b) = ask(&env, json!({"from": "shell", "to": "claude", "message": "oi"}));
    assert_eq!(st, 200, "{b}");
    let token = b["answer"].as_str().expect("token").trim().to_owned();
    assert_eq!(token.len(), 32);
    assert_ne!(token, env.core.bridge_token());
    let port = env.core.bridge_port();
    assert_eq!(http(port, "GET", "/agents", &format!("127.0.0.1:{port}"), &token, "").0, 403, "token esquecido ao terminar");
    env.core.shutdown();
}

#[cfg(unix)]
#[test]
fn only_the_asked_agent_can_answer_a_question() {
    let env = start(&[("claude", "echo x"), ("agy", "echo x")]);
    env.core.enable_tab_delivery();
    let port = env.core.bridge_port();
    let host = format!("127.0.0.1:{port}");
    let master = env.core.bridge_token().to_owned();
    let asker = std::thread::spawn(move || ask_raw(port, &master, json!({"from": "shell", "to": "claude", "message": "oi"})));
    let d = next_delivery(&env);
    // Outra aba tentando responder no lugar do claude (pelo número ou pelo par): não entrega.
    let agy = tab_token(&env, "antigravity");
    let forged = json!({"to": "shell", "message": "forjada", "reply_to": d.request, "from": "claude"}).to_string();
    let (st, b) = http(port, "POST", "/ask", &host, &agy, &forged);
    assert_eq!(st, 404, "{b}");
    assert_eq!(http(port, "POST", "/reply", &host, &agy, &forged).0, 404);
    // O claude responde pelo número da pergunta.
    let claude = tab_token(&env, "claude");
    let body = json!({"to": "", "message": "Paris", "reply_to": d.request}).to_string();
    let (st, b) = http(port, "POST", "/ask", &host, &claude, &body);
    assert_eq!((st, b), (200, json!({"answer": "Resposta entregue a \"shell\"."})));
    let (st, b) = asker.join().expect("thread");
    assert_eq!((st, b["answer"].as_str()), (200, Some("Paris")));
    env.core.shutdown();
}

#[cfg(unix)]
#[test]
fn the_depth_limit_also_holds_between_tabs() {
    let env = start(&[("claude", "echo x"), ("agy", "echo x")]);
    env.core.enable_tab_delivery();
    let port = env.core.bridge_port();
    let host = format!("127.0.0.1:{port}");
    let master = env.core.bridge_token().to_owned();
    // shell → claude (profundidade 0), claude → claude@dev (1), claude@dev → antigravity (2: recusada).
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
    assert_eq!((st, b), (400, json!({"error": "profundidade máxima de conversa entre agentes atingida"})));
    env.core.shutdown();
    let _ = (first.join(), second.join());
}

#[cfg(unix)]
#[test]
fn a_slow_answer_is_typed_later_into_the_askers_tab() {
    let env = start(&[("claude", "echo x")]);
    env.core.enable_tab_delivery();
    let port = env.core.bridge_port();
    let host = format!("127.0.0.1:{port}");
    let claude = tab_token(&env, "claude");
    // O `ask-agent` do claude aceita esperar 1 s: depois disso volta (202) e a resposta vem depois.
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
    assert!(b["answer"].as_str().expect("aviso").contains("será digitada neste terminal"), "{b}");

    let dev = tab_token(&env, "claude@dev");
    let reply = json!({"to": "", "message": "pronto\u{1b}[201~", "reply_to": question.request}).to_string();
    assert_eq!(http(port, "POST", "/ask", &host, &dev, &reply).0, 200);
    let late = next_delivery(&env);
    assert!(late.answer);
    assert_eq!((late.ident.id().as_str(), late.create, late.from.as_str()), ("claude", false, "claude@dev"));
    assert!(
        late.prompt.starts_with(&format!("[Resposta do agente \"claude@dev\" à sua pergunta #{}", question.request)),
        "{}",
        late.prompt
    );
    assert!(late.prompt.contains("pronto[201~") && !late.prompt.contains('\u{1b}'), "{}", late.prompt);
    env.core.shutdown();
}

#[cfg(unix)]
#[test]
fn closing_the_askers_tab_drops_its_questions() {
    let env = start(&[("claude", "echo x")]);
    env.core.enable_tab_delivery();
    let port = env.core.bridge_port();
    let host = format!("127.0.0.1:{port}");
    let dev = tab_token(&env, "claude@dev");
    let asking = std::thread::spawn(move || http(port, "POST", "/ask", &host, &dev, &json!({"to": "claude", "message": "oi"}).to_string()));
    let d = next_delivery(&env);
    env.core.close_instance("claude@dev");
    let (st, b) = asking.join().expect("thread");
    assert_eq!(st, 500);
    assert!(b["error"].as_str().expect("erro").contains("fechada"), "{b}");
    // Sem a pergunta antiga, claude pode perguntar a claude@dev (nada de "ciclo" fantasma).
    let claude = tab_token(&env, "claude");
    let port2 = env.core.bridge_port();
    let again = std::thread::spawn(move || {
        http(port2, "POST", "/ask", &format!("127.0.0.1:{port2}"), &claude, &json!({"to": "claude@dev", "message": "e aí?"}).to_string())
    });
    let d2 = next_delivery(&env);
    assert_ne!(d2.request, d.request);
    assert_eq!(d2.ident.id(), "claude@dev");
    env.core.shutdown();
    let _ = again.join();
}

#[cfg(unix)]
#[test]
fn a_plain_terminal_waits_until_the_end_instead_of_getting_typed_answers() {
    // Numa aba "shell", uma resposta digitada depois seria executada como comando.
    let env = start(&[("claude", "echo x")]);
    env.core.enable_tab_delivery();
    let port = env.core.bridge_port();
    let host = format!("127.0.0.1:{port}");
    let shell = tab_token(&env, "shell");
    let asking = std::thread::spawn(move || {
        http(port, "POST", "/ask", &host, &shell, &json!({"to": "claude", "message": "oi", "wait": 1}).to_string())
    });
    let d = next_delivery(&env);
    std::thread::sleep(Duration::from_millis(1500));
    assert!(!asking.is_finished(), "não pode voltar com 202 depois de 1 s");
    let claude = tab_token(&env, "claude");
    let body = json!({"to": "", "message": "resposta", "reply_to": d.request}).to_string();
    assert_eq!(http(port, "POST", "/ask", &format!("127.0.0.1:{port}"), &claude, &body).0, 200);
    let (st, b) = asking.join().expect("thread");
    assert_eq!((st, b["answer"].as_str()), (200, Some("resposta")));
    env.core.shutdown();
}

#[cfg(unix)]
#[test]
fn the_conversation_map_follows_questions_live() {
    let env = start(&[("claude", "echo x")]);
    env.core.enable_tab_delivery();
    let port = env.core.bridge_port();
    let host = format!("127.0.0.1:{port}");
    assert_eq!(env.core.conversation_map().pending, []);
    let claude = tab_token(&env, "claude");
    let asking = std::thread::spawn({
        let host = host.clone();
        move || http(port, "POST", "/ask", &host, &claude, &json!({"to": "claude@dev", "message": "x", "wait": 1}).to_string())
    });
    let d = next_delivery(&env);
    let map = env.core.conversation_map();
    assert_eq!(map.pending.len(), 1);
    let p = &map.pending[0];
    assert_eq!((p.id, p.asker.as_str(), p.target.as_str(), p.detached), (d.request, "claude", "claude@dev", false));
    assert!(map.instances.contains(&"claude@dev".to_owned()));
    // Depois do 202, a pergunta continua no mapa, marcada como resposta atrasada.
    assert_eq!(asking.join().expect("thread").0, 202);
    assert!(env.core.conversation_map().pending[0].detached);
    let dev = tab_token(&env, "claude@dev");
    let reply = json!({"to": "", "message": "ok", "reply_to": d.request}).to_string();
    assert_eq!(http(port, "POST", "/ask", &host, &dev, &reply).0, 200);
    assert_eq!(env.core.conversation_map().pending, []);
    env.core.shutdown();
}

#[cfg(unix)]
#[test]
fn a_headless_question_shows_in_the_map_while_it_runs() {
    let env = std::sync::Arc::new(start(&[("claude", "cat >/dev/null; sleep 1; echo pronto")]));
    let e2 = env.clone();
    let asking = std::thread::spawn(move || ask(&e2, json!({"from": "shell", "to": "claude", "message": "oi"})));
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let seen = loop {
        let h = env.core.conversation_map().headless;
        if !h.is_empty() || std::time::Instant::now() > deadline {
            break h;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(seen.iter().map(|a| (a.asker.as_str(), a.target.as_str())).collect::<Vec<_>>(), [("shell", "claude")]);
    assert_eq!(asking.join().expect("thread").0, 200);
    assert!(env.core.conversation_map().headless.is_empty(), "some quando a resposta chega");
    env.core.shutdown();
}
