//! Testes de ponta a ponta da ponte HTTP, e do executor headless, com `Core`
//! em pastas temporárias e agentes falsos no PATH de busca (o binário `fake_agent`,
//! ver `support/fake_agent.rs`).

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

/// Como [`start`], com a configuração ajustada por `tweak` antes de o `Core` subir.
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

/// Token que a aba da instância `id` do projeto ativo recebe no ambiente.
fn tab_token(env: &Env, id: &str) -> String {
    tab_token_in(env, &env.core.active_project_id(), id)
}

/// Token que a aba da instância `id` do projeto `project` recebe no ambiente.
fn tab_token_in(env: &Env, project: &str, id: &str) -> String {
    let ident = agentdeck_core::Ident::parse(id).expect("id");
    env.core.spawn_spec_for(project, &ident).expect("spec").env["AGENTDECK_TOKEN"].clone()
}

/// Caminho do agente falso compilado junto com os testes (o `[[bin]]` `fake_agent`).
const FAKE_AGENT: &str = env!("CARGO_BIN_EXE_fake_agent");

/// Nome do arquivo executável de um agente, como o sistema procura no PATH de busca.
fn exe_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

/// Comportamento do agente falso que imprime `text` (e sempre lê a entrada padrão). Os campos são
/// descritos em `support/fake_agent.rs`.
fn says(text: &str) -> Value {
    json!({ "stdout": text })
}

/// Agente falso que imprime os argumentos (as instruções viram `<sys>`), o ambiente e devolve o
/// prompt recebido na entrada padrão.
fn echoing_agent() -> Value {
    json!({ "echo_args": true, "echo_env": ["AGENTDECK_AGENT", "AGENTDECK_DEPTH"], "echo_stdin": true })
}

/// Instala o agente falso `name` em `dir`: o binário deste crate com o nome que o PATH procura e, ao
/// lado, `<nome>.fake.json` com o comportamento.
fn install_fake(dir: &Path, name: &str, spec: &Value) {
    std::fs::create_dir_all(dir).expect("mkdir");
    let exe = dir.join(exe_name(name));
    std::fs::copy(FAKE_AGENT, &exe).expect("copiar o agente falso");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    std::fs::write(exe.with_extension("fake.json"), spec.to_string()).expect("comportamento do agente falso");
}

/// Cliente HTTP mínimo (permite forjar o cabeçalho Host).
fn http(port: u16, method: &str, path: &str, host: &str, token: &str, body: &str) -> (u16, Value) {
    http_with_token_header(port, "x-agentdeck-token", method, path, host, token, body)
}

/// Como [`http`], com o nome do cabeçalho do token à escolha.
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

/// Chamada à ponte que pode ir para outra thread (só porta e token).
fn ask_raw(port: u16, token: &str, body: Value) -> (u16, Value) {
    http(port, "POST", "/ask", &format!("127.0.0.1:{port}"), token, &body.to_string())
}

fn ask(env: &Env, body: Value) -> (u16, Value) {
    let port = env.core.bridge_port();
    http(port, "POST", "/ask", &format!("127.0.0.1:{port}"), env.core.bridge_token(), &body.to_string())
}

/// O cabeçalho do nome antigo (`x-commanddeck-token`) não vale mais: só `x-agentdeck-token` autentica.
#[test]
fn the_old_token_header_is_refused() {
    let env = start(&[("claude", says("ok"))]);
    let port = env.core.bridge_port();
    let token = env.core.bridge_token().to_owned();
    let host = format!("127.0.0.1:{port}");

    let (st, b) = http_with_token_header(port, "x-commanddeck-token", "GET", "/agents", &host, &token, "");
    assert_eq!((st, b), (403, json!({"error": "token inválido"})));
    assert_eq!(http(port, "GET", "/agents", &host, &token, "").0, 200);
}

/// Espera o `CoreEvent::Activate` (os outros eventos que chegarem antes são ignorados).
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

/// `/activate` (a segunda abertura do app) só vale com o token mestre: sem token ou com o de um agente a janela
/// do usuário não vai para a frente.
#[test]
fn activate_needs_the_master_token() {
    let env = start(&[("claude", says("ok"))]);
    let port = env.core.bridge_port();
    let host = format!("127.0.0.1:{port}");

    assert_eq!(http(port, "POST", "/activate", &host, "", "").0, 403, "sem token");
    assert_eq!(http(port, "POST", "/activate", &host, "errado", "").0, 403, "token errado");
    assert_eq!(http(port, "POST", "/activate", &host, &tab_token(&env, "claude"), "").0, 403, "token de agente");
    assert!(!got_activate(&env), "nenhuma das recusadas pede a janela");

    assert_eq!(http(port, "POST", "/activate", &host, env.core.bridge_token(), "").0, 200);
    assert!(got_activate(&env));
    assert_eq!(http(port, "GET", "/activate", &host, env.core.bridge_token(), "").0, 404, "só POST");
}

/// A segunda instância acha a ponte da primeira pelo arquivo ao lado da trava e a faz mostrar a janela.
#[test]
fn a_second_launch_activates_the_running_instance() {
    let env = start(&[("claude", says("ok"))]);
    let data_dir = paths_in(&env.root).data_dir;
    assert!(agentdeck_core::activation::activate_running(&data_dir));
    assert!(got_activate(&env));

    // Encerrada, ela solta a trava e apaga o arquivo: não há mais a quem pedir.
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
    assert_eq!(agents, vec![json!("claude")]);

    let (st, b) = ask(&env, json!({"from": "shell", "to": "bogus", "message": "oi"}));
    assert_eq!(st, 400);
    let err = b["error"].as_str().expect("erro");
    assert!(err.starts_with("agente desconhecido ou não instalado \"bogus\". Disponíveis: "), "{err}");

    let (st, b) = ask(&env, json!({"from": "claude", "to": "ClaudeCode", "message": "oi"}));
    assert_eq!((st, b), (400, json!({"error": "um agente não pode perguntar a si mesmo"})));
    let (st, b) = ask(&env, json!({"from": "shell", "to": "claude", "message": "oi", "depth": 2}));
    assert_eq!((st, b), (400, json!({"error": "profundidade máxima de conversa entre agentes atingida"})));
    env.core.shutdown();
}

#[test]
fn workspace_agents_md_and_spawn_specs() {
    let env = start(&[("claude", says("ok"))]);
    let p = env.core.active_project_id();
    let ws = env.core.workspace();
    assert_eq!(ws, env.root.join("workspace"));
    let md = std::fs::read_to_string(ws.join("AGENTS.md")).expect("AGENTS.md criado no workspace do app");
    assert!(md.starts_with("<!-- agentdeck:start -->\nYou are running inside AgentDeck"));
    assert!(md.contains("Your name and role are in the AGENTDECK_AGENT and AGENTDECK_ROLE variables"));
    assert!(md.contains("do NOT use your CLI's internal subagents"));

    let shell = env.core.spawn_spec(&p, "shell").expect("shell");
    assert_eq!(shell.label, "Terminal");
    assert_eq!(shell.cwd, ws);
    assert_eq!(shell.env["AGENTDECK_AGENT"], "shell");
    assert_eq!(shell.env["AGENTDECK_PORT"], env.core.bridge_port().to_string());
    // Cada aba tem o próprio token (o mestre não vai para os agentes), estável enquanto ela existir.
    let token = shell.env["AGENTDECK_TOKEN"].clone();
    assert_ne!(token, env.core.bridge_token());
    assert_eq!(token.len(), 32);
    assert_eq!(env.core.spawn_spec(&p, "shell").expect("de novo").env["AGENTDECK_TOKEN"], token);
    assert!(shell.env["PATH"].starts_with(&env.root.join("bin").display().to_string()));
    assert!(!shell.double_enter_on_mention);

    assert_eq!(env.core.spawn_spec(&p, "nada").expect_err("desconhecido"), "Unknown agent: nada");
    let missing = env.core.spawn_spec(&p, "antigravity").expect_err("não instalado");
    assert!(missing.starts_with("Antigravity CLI (agy) not found."));
    assert_eq!(env.core.spawn_spec(&p, "ollama").expect_err("o agente Ollama foi removido"), "Unknown agent: ollama");
    assert_eq!(env.core.spawn_spec("pdeadbeef", "shell").expect_err("sem projeto"), "Unknown project: pdeadbeef");

    // Outro projeto (não ativo): o terminal abre na pasta dele, que ganha o AGENTS.md na primeira aba.
    let other_dir = env.root.join("outro");
    let other = env.core.create_project(&other_dir, None);
    assert_eq!(env.core.active_project_id(), p, "criar um projeto não troca o ativo");
    let spec = env.core.spawn_spec(&other.id, "shell").expect("shell");
    assert_eq!(spec.cwd, other_dir);
    assert_ne!(spec.env["AGENTDECK_TOKEN"], token, "o shell de cada projeto tem o próprio token");
    assert!(std::fs::read_to_string(other_dir.join("AGENTS.md")).expect("AGENTS.md do outro projeto").contains("AGENTDECK_AGENT"));
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
    assert!(std::fs::read_to_string(&hooks).expect("hooks gravados").contains("ask-agent --signal idle"));
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
    let (st, b) = ask(&env, json!({"from": "shell", "to": "claude", "message": "Qual a capital?"}));
    assert_eq!(st, 200, "{b}");
    let answer = b["answer"].as_str().expect("answer");
    assert!(answer.starts_with("ARGS: [-p] [--output-format] [text] [--session-id] ["), "{answer}");
    assert!(answer.contains("[--append-system-prompt] [<sys>] [--add-dir]"), "{answer}");
    assert!(answer.contains("ENV: claude 1"), "{answer}");
    assert!(answer.contains(
        "[Mensagem do agente \"shell\" recebida pelo AgentDeck]\n\nQual a capital?\n\nResponda diretamente ao agente \"shell\". O usuário acompanha a conversa integrada em tempo real."
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
    let project = env.core.active_project_id();
    assert!(feed.iter().all(|f| f.project.as_deref() == Some(project.as_str())), "a conversa é do projeto de quem perguntou");
    env.core.shutdown();
}

#[test]
fn failing_resumed_session_retries_and_errors_report_tail() {
    // Falha sempre que receber --resume; senão responde. Erros vão para o stderr.
    let claude = json!({"fail_on_arg": "--resume", "stderr": "sessão expirada", "stdout": "novo"});
    let aider = json!({"stderr": "l1\nl2", "exit": 3});
    let env = start(&[("claude", claude), ("aider", aider)]);
    assert_eq!(ask(&env, json!({"from": "shell", "to": "claude", "message": "a"})).1["answer"], "novo");
    // Sessão "expira": a ponte descarta e tenta de novo, limpa.
    assert_eq!(ask(&env, json!({"from": "shell", "to": "claude", "message": "b"})).1["answer"], "novo");

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
    assert_eq!(lines, ["start", "end", "start", "end", "start", "end"], "execuções sobrepostas");
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
    let spec = env.core.spawn_spec_for(&p, &ident).expect("shell sempre existe");
    assert_eq!(spec.kind, "shell");
    assert_eq!(spec.env["AGENTDECK_AGENT"], "shell@qa@ana");
    assert_eq!(spec.cwd, env.root.join("workspace"));
    // `shell@main@ana` não tem cargo: sem AGENTDECK_ROLE.
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
    assert_eq!(env.core.register_instance(&p, "claude").id(), "claude@main@ana", "o nome é reaproveitado");
    env.core.shutdown();
}

#[test]
fn two_projects_each_have_their_own_claude() {
    let env = start(&[]);
    fake_claude(&env);
    let a = env.core.active_project_id();
    let b = env.core.create_project(&env.root.join("b"), Some("Projeto B")).id;
    assert_ne!(a, b);
    assert_eq!(env.core.create_project(&env.root.join("b"), None).id, b, "a mesma pasta é o mesmo projeto");
    // Cada projeto tem o seu `claude`, sem nome extra.
    assert_eq!(env.core.register_instance(&a, "claude").id(), "claude");
    assert_eq!(env.core.register_instance(&b, "claude").id(), "claude");
    env.core.restore_instance(&a, "claude@qa");
    let (ta, tb) = (tab_token_in(&env, &a, "claude"), tab_token_in(&env, &b, "claude"));
    assert_ne!(ta, tb, "um token por projeto");
    assert_eq!(tab_token_in(&env, &b, "claude"), tb, "estável enquanto a instância existir");
    // `instance_ids` junta todos os projetos, sem repetir (o uso só olha o modelo).
    assert_eq!(env.core.instance_ids(), ["claude", "claude@qa"]);
    assert!(env.core.peers(&a).contains(&"claude@qa".to_owned()));
    assert!(!env.core.peers(&b).contains(&"claude@qa".to_owned()), "B não vê as instâncias de A");

    let port = env.core.bridge_port();
    let host = format!("127.0.0.1:{port}");
    // /agents e /signal no projeto do token.
    let agents = |token: &str| http(port, "GET", "/agents", &host, token, "").1["agents"].clone();
    assert!(agents(&ta).as_array().expect("lista").contains(&json!("claude@qa")));
    assert!(!agents(&tb).as_array().expect("lista").contains(&json!("claude@qa")));
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

    // Fechar o claude de A não mexe no de B.
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
    env.core.register_instance(&a, "claude"); // a aba principal do Claude em A
    let port = env.core.bridge_port();
    let ask_as = |token: String, to: &'static str| std::thread::spawn(move || ask_raw(port, &token, json!({"to": to, "message": "oi"})));

    // A: claude → claude@dev (cria claude@dev em A) → claude@qa.
    let first = ask_as(tab_token_in(&env, &a, "claude"), "claude@dev");
    let d = next_delivery(&env);
    assert_eq!((d.project.as_str(), d.ident.id().as_str(), d.create, d.from.as_str()), (a.as_str(), "claude@dev", true, "claude"));
    assert!(env.core.project_instance_ids(&a).contains(&"claude@dev".to_owned()));
    assert!(!env.core.project_instance_ids(&b).contains(&"claude@dev".to_owned()), "B não vê o claude@dev de A");
    assert!(!env.core.peers(&b).contains(&"claude@dev".to_owned()));
    let second = ask_as(tab_token_in(&env, &a, "claude@dev"), "claude@qa");
    assert_eq!(next_delivery(&env).ident.id(), "claude@qa");

    // O claude@dev de B não responde no lugar do de A.
    let host = format!("127.0.0.1:{port}");
    let forged = json!({"to": "", "message": "x", "reply_to": d.request}).to_string();
    assert_eq!(http(port, "POST", "/ask", &host, &tab_token_in(&env, &b, "claude@dev"), &forged).0, 404);

    // Em A, claude@qa perguntar a claude fecharia o ciclo (409)…
    let (st, body) = ask_raw(port, &tab_token_in(&env, &a, "claude@qa"), json!({"to": "claude", "message": "oi"}));
    assert_eq!(st, 409, "{body}");
    assert_eq!(body["error"], "\"claude\" já está esperando uma resposta de \"claude@qa\" (ciclo de espera)");
    // …e perguntar a outro já passa da profundidade máxima.
    let (st, body) = ask_raw(port, &tab_token_in(&env, &a, "claude@qa"), json!({"to": "claude@x", "message": "oi"}));
    assert_eq!((st, body), (400, json!({"error": "profundidade máxima de conversa entre agentes atingida"})));
    // Em B, a mesma pergunta não é ciclo nem está fundo: vai para o claude de B (criado lá).
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
    assert_eq!((st, body), (400, json!({"error": "projeto desconhecido: \"pdeadbeef\""})));
    let asking = std::thread::spawn({
        let b = b.clone();
        move || ask_raw(port, &master, json!({"from": "shell", "to": "claude", "message": "oi", "project": b}))
    });
    let d = next_delivery(&env);
    assert_eq!((d.project.as_str(), d.ident.id().as_str()), (b.as_str(), "claude"));
    env.core.fail_delivery(d.request, "teste");
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
    assert!(env.core.rename_project(&b, "Novo nome"));
    assert!(!env.core.rename_project(&b, "  "));
    assert_eq!(env.core.projects().iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["workspace", "Novo nome"]);
    assert!(env.core.update_project(&b, |p| {
        p.open_tabs = vec!["shell".into()];
        p.id = "outro".into();
    }));
    assert_eq!(env.core.projects()[1].open_tabs, ["shell"]);
    assert_eq!(env.core.projects()[1].id, b, "o id não muda");

    assert!(env.core.remove_project(&b));
    assert!(!env.core.remove_project(&b));
    assert_eq!(http(port, "GET", "/agents", &host, &tb, "").0, 403, "o token das abas do projeto removido morre");
    assert!(env.core.project_instance_ids(&b).is_empty());
    assert_eq!(env.core.active_project_id(), a);
    assert!(env.root.join("b").is_dir(), "a pasta não é apagada");

    // Sem o ativo, sobra nenhum: workspace vazio.
    assert!(env.core.remove_project(&a));
    assert!(env.core.active_project().is_none());
    assert_eq!(env.core.workspace(), std::path::PathBuf::new());
    assert!(env.core.read_dir("").is_empty());
    // Criar um projeto sem nenhum ativo o ativa.
    let c = env.core.create_project(&env.root.join("c"), None);
    assert_eq!(env.core.active_project_id(), c.id);
    assert_eq!(env.core.workspace(), env.root.join("c"));
    // set_workspace é um atalho: cria o projeto da pasta e o ativa.
    env.core.set_workspace(env.root.join("d")).expect("set_workspace");
    assert_eq!(env.core.active_project().expect("ativo").path, env.root.join("d"));
    assert_eq!(env.core.projects().len(), 2);
    env.core.shutdown();
}

/// Próxima entrega pedida à interface (ignora os outros eventos).
fn next_delivery(env: &Env) -> agentdeck_core::Delivery {
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

#[test]
fn a_question_is_typed_into_the_tab_and_answered_by_callback() {
    let env = start(&[("claude", says("headless"))]);
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

#[test]
fn a_role_opens_a_new_instance_and_new_forces_another() {
    let env = start(&[("claude", says("headless"))]);
    let p = env.core.active_project_id();
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
    env.core.close_instance(&p, "claude@dev");
    env.core.close_instance(&p, "claude@dev@ana");
    for t in [first, second, third] {
        let (st, b) = t.join().expect("thread");
        assert_eq!(st, 500);
        assert!(b["error"].as_str().expect("erro").contains("was closed"), "{b}");
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
    env.core.fail_delivery(d.request, "a aba não subiu");
    let (st, b) = asker.join().expect("thread");
    assert_eq!((st, b["error"].as_str()), (500, Some("a aba não subiu")));
    env.core.shutdown();
}

#[test]
fn wait_cycles_and_unknown_or_ambiguous_targets_are_refused() {
    let env = start(&[("claude", says("x")), ("agy", says("x"))]);
    let p = env.core.active_project_id();
    env.core.enable_tab_delivery();
    env.core.register_instance(&p, "claude"); // a aba principal do Claude
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
    env.core.restore_instance(&p, "claude@qa@ana");
    env.core.restore_instance(&p, "claude@qa@bia");
    let (st, body) = ask_raw(port, &token, json!({"from": "shell", "to": "claude@qa", "message": "oi"}));
    assert_eq!(st, 400);
    assert!(body["error"].as_str().expect("erro").contains("ambíguo"), "{body}");

    env.core.shutdown();
    for t in [a, b] {
        let _ = t.join();
    }
}

#[test]
fn without_tab_delivery_the_bridge_still_answers_headless() {
    // A interface não ligou a entrega (CLI/testes) ou o usuário desligou a opção: modo headless.
    let env = start(&[("claude", says("resposta-headless"))]);
    let (st, b) = ask(&env, json!({"from": "shell", "to": "claude", "message": "oi"}));
    assert_eq!((st, b["answer"].as_str()), (200, Some("resposta-headless")));

    env.core.enable_tab_delivery();
    env.core.update_config(|c| c.auto_open_agent_tabs = false);
    let (st, b) = ask(&env, json!({"from": "shell", "to": "claude", "message": "de novo"}));
    assert_eq!((st, b["answer"].as_str()), (200, Some("resposta-headless")));
    assert!(env.rx.try_iter().all(|e| !matches!(e, CoreEvent::Deliver(_))));
    env.core.shutdown();
}

#[test]
fn reply_route_only_completes_pending_questions() {
    let env = start(&[("claude", says("x"))]);
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
    let p = env.core.active_project_id();
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
    env.core.close_instance(&p, "shell@qa");
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
            CoreEvent::AgentSignal { id, busy, .. } => Some((id, busy)),
            _ => None,
        })
        .collect();
    assert_eq!(signals, [("shell@dev".to_owned(), true), ("shell@dev".to_owned(), false)]);
    // Estado inválido, ou sem ser uma aba (token mestre): recusado.
    assert_eq!(http(port, "POST", "/signal", &host, &token, &json!({"state": "x"}).to_string()).0, 400);
    assert_eq!(http(port, "POST", "/signal", &host, env.core.bridge_token(), &json!({"state": "idle"}).to_string()).0, 400);
    env.core.shutdown();
}

/// Deixa o `claude` "instalado" para o core (o arquivo só precisa existir; nunca roda).
fn fake_claude(env: &Env) {
    fake_cli(env, "claude");
}

/// Deixa o agente `name` (`claude`, `codex`, `opencode`, `agy`…) "instalado" para o core. O
/// comportamento fica vazio: estes testes nunca o executam.
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
    // O modelo vem do token (`claude@dev` → `claude`), não do corpo.
    let usage = env.core.usage_snapshot();
    let claude = &usage["claude"];
    assert_eq!(claude.session, Some(agentdeck_core::UsageWindow { used: 0.34, resets_at_ms: Some(1_760_000_000_000) }));
    assert_eq!(claude.weekly, Some(agentdeck_core::UsageWindow { used: 1.0, resets_at_ms: None }));
    assert!(claude.updated_ms > 0 && !claude.local);
    assert!(env.rx.try_iter().any(|e| matches!(e, CoreEvent::UsageChanged)));
    // O mesmo uso de novo não acorda a UI.
    assert_eq!(http(port, "POST", "/usage", &host, &token, &body.to_string()).0, 200);
    assert!(!env.rx.try_iter().any(|e| matches!(e, CoreEvent::UsageChanged)));

    // Fora de 0..=1, sem ser uma aba (token mestre) ou numa aba sem plano (shell): recusado.
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
    let err = second.err().expect("a segunda instância na mesma pasta de dados é recusada").to_string();
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
    assert_eq!(http(port, "GET", "/agents", &format!("127.0.0.1:{port}"), &token, "").0, 403, "token esquecido ao terminar");
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

#[test]
fn the_depth_limit_also_holds_between_tabs() {
    let env = start(&[("claude", says("x")), ("agy", says("x"))]);
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

#[test]
fn a_slow_answer_is_typed_later_into_the_askers_tab() {
    let env = start(&[("claude", says("x"))]);
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
    assert!(b["error"].as_str().expect("erro").contains("was closed"), "{b}");
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

#[test]
fn a_plain_terminal_waits_until_the_end_instead_of_getting_typed_answers() {
    // Numa aba "shell", uma resposta digitada depois seria executada como comando.
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
    assert!(!asking.is_finished(), "não pode voltar com 202 depois de 1 s");
    let claude = tab_token(&env, "claude");
    let body = json!({"to": "", "message": "resposta", "reply_to": d.request}).to_string();
    assert_eq!(http(port, "POST", "/ask", &format!("127.0.0.1:{port}"), &claude, &body).0, 200);
    let (st, b) = asking.join().expect("thread");
    assert_eq!((st, b["answer"].as_str()), (200, Some("resposta")));
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

    // Cada aba consulta a própria posição pelo token.
    let (st, b) = http(port, "GET", "/team", &host, &tab_token(&env, "shell@dev"), "");
    assert_eq!((st, b), (200, json!({"id": "shell@dev", "boss": "shell", "crew": ["shell@qa"], "quota": {}, "models": {}})));
    let (st, _) = http(port, "GET", "/team", &host, env.core.bridge_token(), "");
    assert_eq!(st, 400, "o token mestre não é um agente");

    // Ciclos e instâncias fechadas são recusados.
    assert!(env.core.set_boss(&p, "shell", Some("shell@qa")).is_err(), "o chefe não pode ficar sob a própria equipe");
    assert!(env.core.set_boss(&p, "shell@qa", Some("shell@nada")).is_err());

    // Renomear leva a equipe junto; fechar passa a equipe ao avô; soltar na raiz tira o chefe.
    let ana = agentdeck_core::Ident::parse("shell@dev@ana").expect("id");
    env.core.rename_instance(&p, "shell@dev", &ana).expect("renomeia");
    assert_eq!(env.core.team(&p, "shell@qa").boss.as_deref(), Some("shell@dev@ana"));
    env.core.close_instance(&p, "shell@dev@ana");
    assert_eq!(env.core.team(&p, "shell@qa").boss.as_deref(), Some("shell"));
    assert_eq!(env.core.team(&p, "shell").crew, ["shell@qa"]);
    env.core.set_boss(&p, "shell@qa", None).expect("raiz");
    assert_eq!(env.core.team(&p, "shell"), agentdeck_core::Team::default());
    assert!(env.core.projects()[0].tab_parents.is_empty(), "gravado na config do projeto");
    env.core.shutdown();
}

#[test]
fn an_agent_becomes_the_boss_of_the_role_it_opens_and_the_new_one_is_told() {
    let env = start(&[("claude", says("headless"))]);
    let p = env.core.active_project_id();
    env.core.enable_tab_delivery();
    let port = env.core.bridge_port();
    env.core.restore_instance(&p, "claude");
    let chefe = tab_token(&env, "claude");
    let asker = std::thread::spawn(move || ask_raw(port, &chefe, json!({"to": "claude@dev", "message": "tarefa"})));
    let d = next_delivery(&env);
    assert_eq!((d.ident.id().as_str(), d.create), ("claude@dev", true));
    // O aviso vai quando a interface digita a mensagem.
    let notice = env.core.take_team_notice(&p, "claude@dev").expect("aviso");
    assert!(notice.contains("você responde a \"claude\""), "{notice}");
    assert_eq!(env.core.team(&p, "claude").crew, ["claude@dev"]);
    env.core.close_instance(&p, "claude@dev");
    let _ = asker.join();
    env.core.shutdown();
}

/// Projeto ativo com o `claude` "instalado", entrega nas abas ligada e a aba `claude` aberta.
fn start_with_claude_tab() -> (Env, String) {
    let env = start(&[]);
    fake_claude(&env);
    env.core.enable_tab_delivery();
    let p = env.core.active_project_id();
    env.core.restore_instance(&p, "claude");
    (env, p)
}

/// `POST /ask` com o token de uma aba, numa thread.
fn ask_in_thread(env: &Env, token: String, body: Value) -> std::thread::JoinHandle<(u16, Value)> {
    let port = env.core.bridge_port();
    std::thread::spawn(move || ask_raw(port, &token, body))
}

#[test]
fn renaming_both_tabs_mid_question_still_delivers_the_late_answer() {
    let (env, p) = start_with_claude_tab();
    let asking = ask_in_thread(&env, tab_token(&env, "claude"), json!({"to": "claude@dev", "message": "tarefa", "wait": 1}));
    let q = next_delivery(&env);
    assert_eq!(asking.join().expect("thread").0, 202);
    // As duas abas mudam de nome enquanto a resposta não chega.
    let ident = |s: &str| agentdeck_core::Ident::parse(s).expect("id");
    env.core.rename_instance(&p, "claude", &ident("claude@lead")).expect("renomeia");
    env.core.rename_instance(&p, "claude@dev", &ident("claude@dev@ana")).expect("renomeia");
    // O token da aba segue a instância; a resposta vai pelo número e chega ao nome novo.
    let reply = json!({"to": "", "message": "pronto", "reply_to": q.request});
    assert_eq!(ask_raw(env.core.bridge_port(), &tab_token(&env, "claude@dev@ana"), reply).0, 200);
    let late = next_delivery(&env);
    assert!(late.answer);
    assert_eq!((late.ident.id().as_str(), late.from.as_str()), ("claude@lead", "claude@dev@ana"));
    env.core.shutdown();
}

#[test]
fn after_a_202_asking_the_asker_back_is_a_new_question() {
    let (env, _p) = start_with_claude_tab();
    let asking = ask_in_thread(&env, tab_token(&env, "claude"), json!({"to": "claude@dev", "message": "tarefa", "wait": 1}));
    let q = next_delivery(&env);
    assert_eq!(asking.join().expect("thread").0, 202);
    // claude não espera mais: `ask-agent claude "…"` de claude@dev é uma pergunta nova, digitada na aba dele.
    let back = ask_in_thread(&env, tab_token(&env, "claude@dev"), json!({"to": "claude", "message": "qual branch?"}));
    let d = next_delivery(&env);
    assert_eq!((d.ident.id().as_str(), d.from.as_str(), d.answer), ("claude", "claude@dev", false));
    assert_ne!(d.request, q.request);
    assert!(env.core.question_pending(q.request), "a pergunta original segue esperando o --reply");
    env.core.fail_delivery(d.request, "teste");
    assert_eq!(back.join().expect("thread").0, 500);
    assert!(env.core.question_pending(q.request), "liberar a pergunta nova não cancela a original");
    env.core.shutdown();
}

#[test]
fn the_old_reply_form_resolves_aliases() {
    let (env, _p) = start_with_claude_tab();
    let asking = ask_in_thread(&env, tab_token(&env, "claude"), json!({"to": "claude@dev", "message": "tarefa"}));
    next_delivery(&env);
    // `claudecode` é apelido de `claude`: é a resposta a quem espera, não um ciclo (409).
    let (st, b) = ask_raw(env.core.bridge_port(), &tab_token(&env, "claude@dev"), json!({"to": "claudecode", "message": "feito"}));
    assert_eq!((st, b["answer"].as_str()), (200, Some("Resposta entregue a \"claude\".")), "{b}");
    let (st, b) = asking.join().expect("thread");
    assert_eq!((st, b["answer"].as_str()), (200, Some("feito")));
    env.core.shutdown();
}

/// `GET /inbox` (ou `/inbox?all=1`) com o token de uma aba: as respostas da caixa dela.
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
    let asking = ask_in_thread(&env, claude.clone(), json!({"to": "claude@dev", "message": "tarefa"}));
    let q = next_delivery(&env);
    assert_eq!(
        ask_raw(env.core.bridge_port(), &tab_token(&env, "claude@dev"), json!({"to": "", "message": "feito", "reply_to": q.request})).0,
        200
    );
    let (st, b) = asking.join().expect("thread");
    assert_eq!((st, b["answer"].as_str(), b["request"].as_u64()), (200, Some("feito"), Some(q.request)), "{b}");
    // O 200 foi escrito: a resposta está na caixa, já entregue (a marca vem logo depois da escrita).
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !inbox(&env, &claude, true).iter().all(|e| e["delivered"] == true) {
        assert!(std::time::Instant::now() < deadline, "a resposta síncrona não foi marcada como entregue");
        std::thread::sleep(Duration::from_millis(20));
    }
    let all = inbox(&env, &claude, true);
    assert_eq!(all.len(), 1, "{all:?}");
    assert_eq!(
        (all[0]["request"].as_u64(), all[0]["from"].as_str(), all[0]["text"].as_str()),
        (Some(q.request), Some("claude@dev"), Some("feito"))
    );
    assert_eq!(all[0]["failed"], false);
    assert!(inbox(&env, &claude, false).is_empty(), "entregue não aparece em --inbox");
    // Só o próprio agente lê a caixa dele: a de outro está vazia e o token mestre não tem caixa.
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
        assert_eq!(ask_raw(env.core.bridge_port(), &dev, json!({"to": "", "message": "feito", "reply_to": q.request})).0, 200);
        let (st, b) = asking.join().expect("thread");
        assert_eq!((st, b["request"].as_u64()), (200, Some(q.request)), "{b}");
        q.request
    };
    // Escrever o 200 não prova que o `ask-agent` ainda estava lá: sem a confirmação, segue pendente (ler a
    // caixa a marca como entregue, por isso cada pergunta só é lida uma vez).
    let first = answered("primeira");
    std::thread::sleep(Duration::from_millis(200));
    let pending: Vec<u64> = inbox(&env, &claude, false).iter().filter_map(|e| e["request"].as_u64()).collect();
    assert_eq!(pending, [first]);
    // Confirmada pelo cliente, sai do `--inbox`; outro agente não confirma a resposta de quem perguntou.
    let second = answered("segunda");
    let port = env.core.bridge_port();
    let host = format!("127.0.0.1:{port}");
    let body = json!({ "request": second }).to_string();
    assert_eq!(http(port, "POST", "/inbox/delivered", &host, &dev, &body).0, 404);
    assert_eq!(http(port, "POST", "/inbox/delivered", &host, &claude, &body).0, 200);
    assert!(inbox(&env, &claude, false).is_empty(), "confirmada não aparece em --inbox");
    env.core.shutdown();
}

#[test]
fn a_failed_question_is_kept_in_the_inbox_with_its_reason() {
    let (env, _p) = start_with_claude_tab();
    let claude = tab_token(&env, "claude");
    let asking = ask_in_thread(&env, claude.clone(), json!({"to": "claude@dev", "message": "tarefa"}));
    let q = next_delivery(&env);
    env.core.fail_delivery(q.request, "a aba morreu");
    assert_eq!(asking.join().expect("thread").0, 500);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while inbox(&env, &claude, true).iter().all(|e| e["delivered"] != true) {
        assert!(std::time::Instant::now() < deadline, "a falha não entrou na caixa");
        std::thread::sleep(Duration::from_millis(20));
    }
    let all = inbox(&env, &claude, true);
    assert_eq!((all[0]["failed"].as_bool(), all[0]["text"].as_str()), (Some(true), Some("a aba morreu")), "{all:?}");
    env.core.shutdown();
}

#[test]
fn a_late_answer_stays_in_the_inbox_until_it_is_typed_or_read() {
    let (env, p) = start_with_claude_tab();
    let claude = tab_token(&env, "claude");
    let asking = ask_in_thread(&env, claude.clone(), json!({"to": "claude@dev", "message": "tarefa", "wait": 1}));
    let q = next_delivery(&env);
    let (st, b) = asking.join().expect("thread");
    assert_eq!(st, 202);
    assert!(b["answer"].as_str().expect("aviso").contains("ask-agent --inbox"), "{b}");
    assert!(inbox(&env, &claude, true).is_empty(), "ainda sem resposta");

    let dev = tab_token(&env, "claude@dev");
    assert_eq!(ask_raw(env.core.bridge_port(), &dev, json!({"to": "", "message": "pronto", "reply_to": q.request})).0, 200);
    let late = next_delivery(&env);
    assert!(late.answer);
    // A interface ainda não digitou: a resposta está na caixa, não entregue.
    let pending = inbox(&env, &claude, false);
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!((pending[0]["text"].as_str(), pending[0]["delivered"].as_bool()), (Some("pronto"), Some(false)));
    // Lida em --inbox, não volta; continua em --inbox --all.
    assert!(inbox(&env, &claude, false).is_empty());
    assert_eq!(inbox(&env, &claude, true).len(), 1);

    // A interface digita uma segunda resposta: entregue, fora da lista padrão. Se não pôde digitar, avisa na Conversa integrada.
    let asking = ask_in_thread(&env, claude.clone(), json!({"to": "claude@dev", "message": "outra", "wait": 1}));
    let q2 = next_delivery(&env);
    assert_eq!(asking.join().expect("thread").0, 202);
    assert_eq!(ask_raw(env.core.bridge_port(), &dev, json!({"to": "", "message": "segunda", "reply_to": q2.request})).0, 200);
    assert!(next_delivery(&env).answer);
    env.core.answer_typed(q2.request);
    assert!(inbox(&env, &claude, false).is_empty(), "a digitada já foi entregue");
    env.core.answer_lost(&p, q2.request, "a aba não estava aberta");
    let warning = loop {
        match env.rx.recv_timeout(Duration::from_secs(5)).expect("aviso na Conversa integrada") {
            CoreEvent::Feed(e) if e.kind == FeedKind::Error && e.text.contains("--inbox") => break e,
            _ => continue,
        }
    };
    assert!(warning.text.contains(&format!("#{}", q2.request)) && warning.text.contains("a aba não estava aberta"), "{}", warning.text);
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
    let asking = ask_in_thread(&env, tb.clone(), json!({"to": "claude@dev", "message": "tarefa"}));
    let q = next_delivery(&env);
    assert_eq!(q.project, b);
    assert!(env.core.remove_project(&b));
    let (st, body) = asking.join().expect("thread");
    assert_eq!(st, 500, "{body}");
    assert_eq!(ask_raw(env.core.bridge_port(), &tb, json!({"to": "claude@qa", "message": "oi"})).0, 403);
    assert!(!env.core.question_pending(q.request), "a pergunta some com o projeto");
    // Nada recria a entrada do projeto removido.
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
    let asking = ask_in_thread(&env, tab_token(&env, "claude"), json!({"to": "claude@dev", "message": "tarefa"}));
    let d = next_delivery(&env);
    assert!(!d.prompt.contains("posição na equipe"), "a interface anexa quando digita: {}", d.prompt);
    // A interface descartou a entrega: o aviso continua lá para a próxima mensagem.
    env.core.fail_delivery(d.request, "descartada");
    assert_eq!(asking.join().expect("thread").0, 500);
    assert!(env.core.take_team_notice(&p, "claude@dev").expect("aviso").contains("você responde a \"claude\""));
    assert_eq!(env.core.take_team_notice(&p, "claude@dev"), None, "uma vez só");
    // O chefe que perde um subordinado sem equipe também é avisado.
    let _ = env.core.take_team_notice(&p, "claude");
    env.core.close_instance(&p, "claude@qa");
    assert!(env.core.take_team_notice(&p, "claude").expect("aviso").contains("sua equipe: claude@dev"));
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
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("relógio").as_millis() as u64;
    let w = |used, h: u64| Some(UsageWindow { used, resets_at_ms: Some(now + h * 3_600_000) });
    let group = |name: &str, weekly| UsageGroup { name: name.into(), session: None, weekly };
    let groups = vec![group("Gemini Models", w(0.0, 30)), group("Gemini 4 Argon", w(1.0, 9))];
    env.core.set_usage("antigravity", AgentUsage { weekly: w(1.0, 9), updated_ms: now, groups, ..Default::default() });

    // Argon sem cota: 409 que explica e sugere outro modelo; nenhuma aba é criada.
    let (st, body) =
        ask_raw(port, &master, json!({"from": "shell", "to": "antigravity@arq", "message": "oi", "model": "gemini-4-argon-high"}));
    assert_eq!(st, 409, "{body}");
    let error = body["error"].as_str().expect("erro");
    assert!(error.starts_with("antigravity@arq uses gemini-4-argon-high and its quota for Gemini 4 Argon exhausted, renews in"), "{error}");
    assert!(error.contains("--new --model <model>"), "{error}");
    assert!(env.core.project_instance_ids(&p).is_empty(), "nada foi criado");
    assert!(env.core.projects()[0].tab_models.is_empty());

    // O Flash conta em outro grupo: entrega normalmente.
    let asking = master_ask(&env, json!({"from": "shell", "to": "antigravity@arq", "message": "oi", "model": "gemini-3.8-flash-high"}));
    let d = next_delivery(&env);
    assert_eq!((d.ident.id().as_str(), d.create), ("antigravity@arq", true));
    env.core.fail_delivery(d.request, "teste");
    assert_eq!(asking.join().expect("thread").0, 500);

    // Uma leitura velha não recusa: a cota pode ter voltado.
    let stale = vec![group("Gemini 4 Argon", w(1.0, 9))];
    env.core.set_usage("antigravity", AgentUsage { updated_ms: now - 2 * 3_600_000, groups: stale, ..Default::default() });
    let asking = master_ask(&env, json!({"from": "shell", "to": "antigravity@b", "message": "oi", "model": "gemini-4-argon-low"}));
    let d = next_delivery(&env);
    assert_eq!(d.ident.id(), "antigravity@b");
    env.core.fail_delivery(d.request, "teste");
    assert_eq!(asking.join().expect("thread").0, 500);
}

/// `POST /ask` com o token mestre (o `from` do corpo vale), numa thread.
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

    // Abre `claude@dev` com sonnet: o modelo fica gravado antes de a aba abrir.
    let first = master_ask(&env, json!({"from": "shell", "to": "claude@dev", "message": "tarefa", "model": "sonnet"}));
    let d = next_delivery(&env);
    assert_eq!((d.ident.id().as_str(), d.create), ("claude@dev", true));
    assert_eq!(env.core.instance_model(&p, &d.ident).as_deref(), Some("sonnet"));
    assert_eq!(state(&env).get("claude@dev").map(String::as_str), Some("sonnet"), "gravado na config do projeto");
    assert_eq!(env.core.instance_models(&p).get("claude@dev").map(String::as_str), Some("sonnet"));

    // Outro modelo para a mesma instância: 409 com a saída pelo --new; nada muda.
    let (st, body) = ask_raw(port, &master, json!({"from": "shell", "to": "claude@dev", "message": "oi", "model": "opus"}));
    assert_eq!(st, 409, "{body}");
    assert_eq!(body["error"], "claude@dev is already open with sonnet; use --new to open another one with opus");
    assert_eq!(env.core.project_instance_ids(&p), ["claude@dev"]);
    assert_eq!(state(&env).len(), 1);
    // Pedir um modelo a uma instância aberta com o padrão do CLI também é conflito.
    env.core.restore_instance(&p, "claude@qa");
    let (st, body) = ask_raw(port, &master, json!({"from": "shell", "to": "claude@qa", "message": "oi", "model": "opus"}));
    assert_eq!(
        (st, body["error"].as_str()),
        (409, Some("claude@qa is already open with the default model; use --new to open another one with opus"))
    );

    // O mesmo modelo, ou nenhum, entrega normalmente na aba que já existe.
    for model in [Some("sonnet"), None] {
        let mut body = json!({"from": "shell", "to": "claude@dev", "message": "de novo"});
        if let Some(m) = model {
            body["model"] = json!(m);
        }
        let asking = master_ask(&env, body);
        let d = next_delivery(&env);
        assert_eq!((d.ident.id().as_str(), d.create), ("claude@dev", false), "{model:?}");
        env.core.fail_delivery(d.request, "teste");
        assert_eq!(asking.join().expect("thread").0, 500);
    }

    // --new + --model abre outra instância do cargo com o modelo pedido.
    let second = master_ask(&env, json!({"from": "shell", "to": "claude@dev", "message": "outra", "model": "opus", "new": true}));
    let d = next_delivery(&env);
    assert_eq!((d.ident.id().as_str(), d.create), ("claude@dev@ana", true));
    assert_eq!(env.core.instance_model(&p, &d.ident).as_deref(), Some("opus"));
    assert_eq!(state(&env).get("claude@dev").map(String::as_str), Some("sonnet"), "a primeira não mudou");

    // Modelo fora do padrão, ou agente que não escolhe modelo: 400 antes de criar qualquer coisa.
    let ids_before = env.core.project_instance_ids(&p);
    let bad = |to: &str, model: Value| ask_raw(port, &master, json!({"from": "shell", "to": to, "message": "oi", "model": model}));
    let (st, body) = bad("claude@x", json!("a b"));
    assert_eq!(st, 400);
    assert!(body["error"].as_str().expect("erro").starts_with("modelo inválido: \"a b\""), "{body}");
    assert_eq!(bad("claude@x", json!("a\"; rm")).0, 400);
    assert_eq!(bad("claude@x", json!("m".repeat(129))).0, 400);
    assert_eq!(bad("claude@x", json!(7)).0, 400);
    assert_eq!(bad("aider", json!("gpt")), (400, json!({"error": "o agente aider não aceita escolher modelo"})));
    assert_eq!(env.core.project_instance_ids(&p), ids_before, "nada foi criado");
    assert_eq!(state(&env).len(), 2);

    // Fechar esquece o modelo.
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
    // Profundidade estourada depois de criar: a instância e o modelo somem juntos.
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
    env.core.set_boss(&p, "claude@dev", Some("claude")).expect("equipe");

    let (st, body) = http(port, "GET", "/agents", &host, &tab_token(&env, "claude"), "");
    assert_eq!(st, 200);
    assert_eq!(body["models"], json!({"claude@dev": "sonnet", "claude@main@ana": "opus"}));
    assert_eq!(body["quota"], json!({"claude@dev": "quota: no reading", "claude@main@ana": "quota: no reading"}));
    let (st, body) = http(port, "GET", "/team", &host, &tab_token(&env, "claude@dev"), "");
    assert_eq!(st, 200);
    // Só quem aparece na resposta (ele; o chefe e a equipe, se tivessem modelo).
    assert_eq!(body["models"], json!({"claude@dev": "sonnet"}));
    assert_eq!(body["quota"], json!({"claude@dev": "quota: no reading"}));
    assert_eq!(body["boss"], "claude");
    env.core.shutdown();
}

#[test]
fn the_models_inventory_lists_installed_agents_only_and_survives_a_failed_listing() {
    use agentdeck_core::{AgentUsage, UsageGroup, UsageWindow};
    let env = start(&[]);
    // O `opencode` instalado não lista nada (o falso sai sem saída): uma linha de falha, o resto segue.
    fake_cli(&env, "claude");
    fake_cli(&env, "opencode");
    let port = env.core.bridge_port();
    let host = format!("127.0.0.1:{port}");
    let get = |path: &str| http(port, "GET", path, &host, env.core.bridge_token(), "");

    let (st, body) = get("/models");
    assert_eq!(st, 200, "{body}");
    let text = body["text"].as_str().expect("text");
    assert!(text.starts_with("claude --model: opus, sonnet, haiku, fable\n  cota: no reading\n"), "{text}");
    assert!(text.contains("opencode: could not list the models"), "{text}");
    assert!(text.contains("\n  cota: no reading\n\ncapacidade ("), "a capacidade vem depois dos agentes: {text}");
    assert!(text.contains("Claude Sonnet 5.5 (claude sonnet): complexa, Coding Agent Index 59.3 (2026-10)"), "{text}");
    assert!(!text.contains("antigravity"), "o que não está instalado não aparece: {text}");

    // Um agente só (com apelido); o não instalado e o inválido são recusados.
    let (st, body) = get("/models?agent=claudecode");
    assert_eq!(st, 200);
    assert!(
        body["text"]
            .as_str()
            .is_some_and(|t| t.starts_with("claude --model: opus, sonnet, haiku, fable\n  cota: no reading\n\ncapacidade (")),
        "{body}"
    );
    let (st, body) = get("/models?agent=agy");
    assert_eq!(st, 404);
    assert!(body["error"].as_str().expect("error").contains("is not installed"), "{body}");
    assert_eq!(get("/models?agent=a%20b").0, 400);

    // Com o Antigravity instalado e renovado (4%) e o Claude em 60%, o Antigravity vem antes do Claude.
    fake_cli(&env, "agy");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("relógio").as_millis() as u64;
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
    assert!(agy_pos < claude_pos, "o mais folgado vem primeiro:\n{text}");
    assert!(text.contains("  cota: Gemini 4 Argon 4% used (renews in 6 days)"), "{text}");
    assert!(text.contains("  cota: 60% used (renews in 4 days)"), "{text}");
    env.core.shutdown();
}

#[test]
fn the_model_resolves_to_request_then_saved_then_role_default_and_reaches_each_cli() {
    let env = start_cfg(&[], |c| {
        c.role_models.insert("claude@dev".into(), "sonnet".into());
        c.role_models.insert("claude".into(), "opus".into());
        c.role_models.insert("opencode".into(), "opencode/glm-5.3-flash-free".into());
        c.role_models.insert("aider".into(), "nao-vale".into());
    });
    for cli in ["claude", "codex", "opencode", "agy", "aider"] {
        fake_cli(&env, cli);
    }
    let p = env.core.active_project_id();
    let ident = |s: &str| agentdeck_core::Ident::parse(s).expect("id");
    let saved = |env: &Env| env.core.projects()[0].tab_models.clone();
    let tail = |args: &[String], n: usize| args[args.len() - n..].to_vec();
    let data = env.root.join("dados").display().to_string();

    // c) o padrão do cargo vale, e fica gravado ao abrir.
    let dev = env.core.spawn_spec_for(&p, &ident("claude@dev")).expect("claude@dev");
    assert_eq!(dev.model.as_deref(), Some("sonnet"));
    assert_eq!(tail(&dev.args, 2), ["--model", "sonnet"]);
    assert_eq!(saved(&env).get("claude@dev").map(String::as_str), Some("sonnet"));
    // `modelo@cargo` vence `modelo`; o cargo sem entrada cai no `modelo`.
    assert_eq!(env.core.spawn_spec_for(&p, &ident("claude@qa")).expect("qa").model.as_deref(), Some("opus"));

    // b) o gravado vence um padrão que mudou depois.
    env.core.update_config(|c| {
        c.role_models.insert("claude@dev".into(), "haiku".into());
    });
    assert_eq!(env.core.spawn_spec_for(&p, &ident("claude@dev")).expect("de novo").model.as_deref(), Some("sonnet"));
    // a) o pedido vence o gravado.
    assert_eq!(env.core.assign_model(&p, &ident("claude@dev"), Some("fable")).as_deref(), Some("fable"));
    let spec = env.core.spawn_spec_for(&p, &ident("claude@dev")).expect("fable");
    assert_eq!(tail(&spec.args, 2), ["--model", "fable"]);

    // Codex e Antigravity: --model; o Codex mantém o hook de fim de turno.
    env.core.assign_model(&p, &ident("codex@x"), Some("gpt-5-codex"));
    let codex = env.core.spawn_spec_for(&p, &ident("codex@x")).expect("codex");
    assert_eq!(tail(&codex.args, 2), ["--model", "gpt-5-codex"]);
    assert!(codex.args.iter().any(|a| a.starts_with("notify=")));
    let agy = env.core.spawn_spec_for(&p, &ident("antigravity@x")).expect("agy");
    // Por último, a pasta das instruções da instância (`dados/agy/<id>`), depois da pasta de dados.
    let own = std::path::Path::new(&data).join("agy").join("antigravity@x").to_string_lossy().into_owned();
    assert_eq!((agy.model, tail(&agy.args, 3)), (None, vec![data, "--add-dir".to_owned(), own]), "sem modelo, sem --model");
    env.core.assign_model(&p, &ident("antigravity@x"), Some("claude-sonnet-5-5-medium"));
    let agy = env.core.spawn_spec_for(&p, &ident("antigravity@x")).expect("agy");
    assert_eq!(tail(&agy.args, 2), ["--model", "claude-sonnet-5-5-medium"]);

    // OpenCode: o TUI não tem --model; vai servidor próprio + configuração no ambiente.
    let oc = env.core.spawn_spec_for(&p, &ident("opencode")).expect("opencode");
    assert_eq!(oc.args, ["--standalone"]);
    let conf: Value = serde_json::from_str(&oc.env["OPENCODE_CONFIG_CONTENT"]).expect("json");
    let instructions = env.root.join("dados").join("agentdeck-instructions.md");
    assert_eq!(conf, json!({"model": "opencode/glm-5.3-flash-free", "instructions": [instructions.to_string_lossy()]}));
    // Sem modelo, o OpenCode ainda recebe as instruções do AgentDeck (servidor próprio + arquivo na configuração).
    env.core.update_config(|c| {
        c.role_models.remove("opencode");
    });
    let plain = env.core.spawn_spec_for(&p, &ident("opencode@qa")).expect("opencode sem modelo");
    assert_eq!(plain.args, ["--standalone"]);
    let conf: Value = serde_json::from_str(&plain.env["OPENCODE_CONFIG_CONTENT"]).expect("json");
    assert_eq!(conf, json!({"instructions": [instructions.to_string_lossy()]}));

    // Quem não escolhe modelo não ganha um, nem do padrão nem do pedido.
    assert_eq!(env.core.spawn_spec_for(&p, &ident("aider")).expect("aider").model, None);
    assert_eq!(env.core.assign_model(&p, &ident("aider"), Some("gpt")), None);
    assert_eq!(env.core.spawn_spec_for(&p, &ident("shell")).expect("shell").model, None);
    assert!(!saved(&env).contains_key("aider") && !saved(&env).contains_key("shell"));
    assert!(env.core.register_instance_with_model(&p, "aider", Some("gpt")).is_err());
    assert!(env.core.register_instance_with_model(&p, "claude", Some("a b")).is_err());

    // Renomear leva o modelo; fechar esquece.
    env.core.restore_instance(&p, "claude@dev");
    env.core.rename_instance(&p, "claude@dev", &ident("claude@lead")).expect("renomeia");
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
    let saved = std::fs::read_to_string(&paths.config_file).expect("a config foi gravada");
    assert!(saved.contains("\"tabModels\"") && saved.contains("\"sonnet\""), "{saved}");
    core.shutdown();
    drop(core);
    // Outra abertura do app lê a mesma config: o modelo está lá, e `restore_instance` o respeita.
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
    assert!(b["answer"].as_str().expect("answer").contains("[--model] [opus]"), "o padrão do cargo vale sem pedido: {b}");
    let (st, b) = ask(&env, json!({"from": "shell", "to": "aider", "message": "oi", "model": "gpt"}));
    assert_eq!((st, b), (400, json!({"error": "o agente aider não aceita escolher modelo"})));
    env.core.shutdown();
}

/// Com os CLIs reais instalados: `cargo test -p agentdeck-core --test bridge -- --ignored --nocapture`.
#[test]
#[ignore = "usa o agy e o opencode instalados na máquina"]
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
    let items = core.models_inventory(None).expect("inventário");
    println!("{}\n({:?})", agentdeck_core::models::format_inventory(&items, &core.quota_lines()), started.elapsed());
    core.shutdown();
}

/// Com os CLIs reais instalados: `cargo test -p agentdeck-core --test bridge -- --ignored --nocapture`.
#[test]
#[ignore = "usa o agy e o opencode instalados na máquina"]
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
        println!("{kind}: {} modelos em {:?}; primeiros: {:?}", list.len(), started.elapsed(), &list[..list.len().min(4)]);
        let again = std::time::Instant::now();
        assert_eq!(core.known_models(kind), list);
        assert!(again.elapsed() < Duration::from_millis(50), "a segunda consulta vem do cache");
    }
    core.shutdown();
}
