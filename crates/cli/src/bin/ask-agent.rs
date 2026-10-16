//! `ask-agent` — pergunta algo a outro agente pela ponte do CommandDeck (porta de bin/ask-agent.js).
//!
//! Uso: ask-agent <agente> "mensagem"     (ou "-" para ler a mensagem do stdin)
//!      ask-agent --new <agente> "mensagem"   (abre outra instância, mesmo se já houver uma)
//!      ask-agent --reply <n> "resposta"       (responde à pergunta número n)
//!      ask-agent --list
//!      ask-agent --team                       (seu chefe e sua equipe)
//!      ask-agent --boss "mensagem"            (pergunta/reporta ao seu chefe)
//!      ask-agent --signal busy|idle           (hooks do agente: início/fim de turno)
//!      ask-agent --usage                      (statusline do Claude Code: uso do plano)
//!
//! `<agente>` é `modelo[@cargo[@nome]]`: `claude`, `claude@dev`, `claude@dev@ana`.
//!
//! Numa aba, o `ask-agent` espera a resposta por até `COMMANDDECK_ASK_WAIT` segundos (padrão 90,
//! abaixo do timeout padrão do Bash do Claude Code; `0` = até o fim). Se o outro agente demorar
//! mais, ele termina avisando, e a resposta é digitada depois no terminal de quem perguntou.
//!
//! Códigos de saída: 0 = sucesso; 1 = erro operacional; 2 = erro de argumentos ou fora do CommandDeck.
//! `--signal` e `--usage` saem sempre com 0 (um hook que falha não pode atrapalhar o agente).

use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

use commanddeck_core::usage::{from_claude_statusline, usage_to_json};
use commanddeck_core::AgentUsage;
use serde_json::{json, Value};

const USAGE: &str =
    "Uso: ask-agent [--new] <agente[@cargo[@nome]]> \"mensagem\"   |   ask-agent --reply <n> \"resposta\"   |   ask-agent --list   |   ask-agent --team   |   ask-agent --boss \"mensagem\"";

/// Segundos que o `ask-agent` espera a resposta antes de deixá-la para ser digitada depois.
const DEFAULT_WAIT: u64 = 90;
/// Limite do `POST /usage`: a statusline roda a cada atualização da tela do Claude.
const USAGE_POST_TIMEOUT: Duration = Duration::from_millis(1500);
/// Limite da statusline própria do usuário (repassada pelo `--usage`).
const USER_STATUSLINE_TIMEOUT: Duration = Duration::from_secs(3);

struct Bridge {
    port: String,
    token: String,
}

impl Bridge {
    /// Faz a requisição (`timeout` = `None`: sem limite, respostas de agentes podem levar minutos).
    fn request(
        &self,
        method: &str,
        route: &str,
        body: Option<&Value>,
        timeout: Option<std::time::Duration>,
    ) -> Result<(u16, Value), String> {
        let agent: ureq::Agent =
            ureq::Agent::config_builder().proxy(None).http_status_as_error(false).timeout_global(timeout).build().into();
        let url = format!("http://127.0.0.1:{}{route}", self.port);
        let resp = match (method, body) {
            ("GET", _) => agent.get(&url).header("x-commanddeck-token", &self.token).header("content-type", "application/json").call(),
            (_, Some(b)) => agent.post(&url).header("x-commanddeck-token", &self.token).send_json(b),
            (_, None) => agent.post(&url).header("x-commanddeck-token", &self.token).send_empty(),
        };
        let mut resp = resp.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let text = resp.body_mut().with_config().limit(1 << 30).read_to_string().map_err(|e| e.to_string())?;
        let json: Value = serde_json::from_str(&text).map_err(|e| format!("resposta inválida da ponte: {e}"))?;
        Ok((status, json))
    }
}

fn fail(msg: impl std::fmt::Display, code: u8) -> ExitCode {
    eprintln!("ask-agent: {msg}");
    ExitCode::from(code)
}

fn main() -> ExitCode {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--usage") {
        let bridge = env("COMMANDDECK_PORT").zip(env("COMMANDDECK_TOKEN")).map(|(port, token)| Bridge { port, token });
        return statusline(bridge);
    }
    let (Some(port), Some(token)) = (env("COMMANDDECK_PORT"), env("COMMANDDECK_TOKEN")) else {
        if args.first().is_some_and(|a| a == "--signal") {
            return ExitCode::SUCCESS;
        }
        return fail("rode este comando de dentro de um terminal do CommandDeck.", 2);
    };
    let from = env("COMMANDDECK_AGENT").unwrap_or_else(|| "shell".into());
    let depth: i64 = env("COMMANDDECK_DEPTH").and_then(|d| d.trim().parse::<f64>().ok()).map(|d| d as i64).unwrap_or(0);
    let bridge = Bridge { port, token };

    if args.first().is_some_and(|a| a == "--signal") {
        // Hook do agente (o Codex ainda acrescenta um JSON, ignorado). Falhas são silenciosas.
        let state = args.get(1).map(String::as_str).unwrap_or("");
        let _ = bridge.request("POST", "/signal", Some(&json!({ "state": state })), Some(std::time::Duration::from_secs(3)));
        return ExitCode::SUCCESS;
    }
    let force_new = args.first().is_some_and(|a| a == "--new");
    if force_new {
        args.remove(0);
    }
    let mut reply_to: Option<u64> = None;
    if args.first().is_some_and(|a| a == "--reply") {
        match args.get(1).and_then(|n| n.trim_start_matches('#').parse().ok()) {
            Some(n) => reply_to = Some(n),
            None => return fail(format!("--reply precisa do número da pergunta.\n{USAGE}"), 2),
        }
        // Daqui em diante, `args[0]` faz o papel do destino (ignorado) e o resto é a mensagem.
        args.drain(..2);
        args.insert(0, String::new());
    }
    if args.first().is_some_and(|a| a == "--team") {
        return match team(&bridge) {
            Ok((id, boss, crew)) => {
                println!("Você: {id}");
                match boss {
                    Some(b) => println!("Chefe: {b} (reporte a ele com: ask-agent --boss \"mensagem\")"),
                    None => println!("Chefe: nenhum (você responde diretamente ao usuário)"),
                }
                if crew.is_empty() {
                    println!("Equipe: nenhum agente");
                } else {
                    println!("Equipe: {}", crew.join(", "));
                }
                ExitCode::SUCCESS
            }
            Err(e) => fail(e, 1),
        };
    }
    if args.first().is_some_and(|a| a == "--boss") {
        // O chefe é uma instância que já existe: `--new` abriria outra em vez de falar com ele.
        // Recusar é mais claro que ignorar em silêncio.
        if force_new {
            return fail(format!("--new não combina com --boss (o chefe é uma instância que já existe).\n{USAGE}"), 2);
        }
        match team(&bridge) {
            Ok((_, Some(boss), _)) => args[0] = boss,
            Ok((_, None, _)) => {
                return fail("você não tem chefe: responde diretamente ao usuário (escreva a resposta no seu terminal).", 1)
            }
            Err(e) => return fail(e, 1),
        }
    }
    let to = args.first().map(String::as_str).unwrap_or("");
    if reply_to.is_none() && (to.is_empty() || to == "-h" || to == "--help") {
        println!("{USAGE}");
        return ExitCode::from(if to.is_empty() { 2 } else { 0 });
    }
    if to == "--list" {
        return match bridge.request("GET", "/agents", None, None) {
            Ok((_, json)) => match json.get("agents").and_then(Value::as_array) {
                Some(list) => {
                    let names: Vec<&str> = list.iter().filter_map(Value::as_str).collect();
                    println!("{}", names.join("\n"));
                    ExitCode::SUCCESS
                }
                None => fail(json.get("error").and_then(Value::as_str).unwrap_or("resposta inválida da ponte"), 1),
            },
            Err(e) => fail(e, 1),
        };
    }

    let mut message = args.get(1..).map(|a| a.join(" ")).unwrap_or_default();
    // O stdin só é lido com "-" ou quando vem de um pipe/arquivo: num terminal, sem mensagem,
    // a leitura travaria sem aviso. Bytes + `from_utf8_lossy`: arquivos em cp1252 não dão erro.
    if message == "-" || (message.is_empty() && !std::io::stdin().is_terminal()) {
        let mut raw = Vec::new();
        if let Err(e) = std::io::stdin().read_to_end(&mut raw) {
            return fail(format!("falha ao ler o stdin: {e}"), 1);
        }
        message = String::from_utf8_lossy(&raw).into_owned();
    }
    if message.trim().is_empty() {
        return fail("mensagem vazia.", 2);
    }

    let wait = env("COMMANDDECK_ASK_WAIT").and_then(|w| w.trim().parse::<u64>().ok()).unwrap_or(DEFAULT_WAIT);
    let mut body = json!({ "from": from, "to": to, "message": message, "depth": depth, "new": force_new, "wait": wait });
    if let Some(n) = reply_to {
        body["reply_to"] = json!(n);
    }
    match bridge.request("POST", "/ask", Some(&body), None) {
        // 202: a resposta ainda não chegou e será digitada depois neste terminal (o texto explica).
        Ok((200 | 202, json)) => {
            let answer = match json.get("answer") {
                Some(Value::String(s)) => s.clone(),
                Some(other) => other.to_string(),
                None => "undefined".into(),
            };
            println!("{answer}");
            ExitCode::SUCCESS
        }
        Ok((_, json)) => fail(json.get("error").and_then(Value::as_str).unwrap_or("erro desconhecido"), 1),
        Err(e) => fail(e, 1),
    }
}

/// `GET /team`: (eu, chefe, equipe).
fn team(bridge: &Bridge) -> Result<(String, Option<String>, Vec<String>), String> {
    let (status, json) = bridge.request("GET", "/team", None, Some(Duration::from_secs(10)))?;
    if status != 200 {
        return Err(json.get("error").and_then(Value::as_str).unwrap_or("erro desconhecido").to_owned());
    }
    let id = json.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
    let boss = json.get("boss").and_then(Value::as_str).map(str::to_owned);
    let crew = json.get("crew").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect());
    Ok((id, boss, crew.unwrap_or_default()))
}

// ---------- Statusline do Claude Code (`--usage`) ----------

/// `--usage`: lê o JSON que o Claude Code passa à statusline, manda o uso do plano à ponte
/// (`POST /usage`) e imprime a linha: a da statusline própria do usuário, se ele tiver uma (o
/// nosso `--settings` a substitui), ou "sessão 34% · semana 12%". Sem dados, não imprime nada.
fn statusline(bridge: Option<Bridge>) -> ExitCode {
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let status: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
    let usage = from_claude_statusline(&status);
    // A requisição corre junto com a statusline do usuário.
    let post = bridge.filter(|_| usage.session.is_some() || usage.weekly.is_some()).map(|bridge| {
        let body = usage_to_json(&usage);
        std::thread::spawn(move || {
            let _ = bridge.request("POST", "/usage", Some(&body), Some(USAGE_POST_TIMEOUT));
        })
    });
    let line = user_statusline(&status).and_then(|cmd| run_user_statusline(&cmd, &input)).unwrap_or_else(|| usage_line(&usage));
    if let Some(post) = post {
        let _ = post.join();
    }
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(line.as_bytes());
    let _ = out.flush();
    ExitCode::SUCCESS
}

/// "sessão 34% · semana 12%", com quebra de linha (vazio sem dados).
fn usage_line(u: &AgentUsage) -> String {
    let pct = |w: Option<commanddeck_core::UsageWindow>| w.map(|w| format!("{:.0}%", w.used * 100.0));
    let parts: Vec<String> =
        [("sessão", pct(u.session)), ("semana", pct(u.weekly))].into_iter().filter_map(|(k, v)| Some(format!("{k} {}", v?))).collect();
    if parts.is_empty() {
        String::new()
    } else {
        parts.join(" · ") + "\n"
    }
}

/// Comando da statusline do usuário, na ordem de precedência do Claude Code: projeto local,
/// projeto e usuário (`CLAUDE_CONFIG_DIR` ou `~/.claude`). A nossa própria é ignorada.
fn user_statusline(status: &Value) -> Option<String> {
    let project = ["project_dir", "current_dir"]
        .iter()
        .find_map(|k| status.get("workspace")?.get(*k)?.as_str().filter(|s| !s.is_empty()).map(PathBuf::from))
        .or_else(|| std::env::current_dir().ok());
    let config_dir = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| commanddeck_core::util::home_dir().join(".claude"));
    let mut files: Vec<PathBuf> = Vec::new();
    if let Some(p) = &project {
        files.push(p.join(".claude").join("settings.local.json"));
        files.push(p.join(".claude").join("settings.json"));
    }
    files.push(config_dir.join("settings.json"));
    files.iter().find_map(|f| statusline_in(f))
}

fn statusline_in(file: &Path) -> Option<String> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()?;
    let sl = v.get("statusLine")?;
    if sl.get("type").and_then(Value::as_str).is_some_and(|t| t != "command") {
        return None;
    }
    let cmd = sl.get("command")?.as_str()?.trim();
    (!cmd.is_empty() && !cmd.contains("ask-agent --usage")).then(|| cmd.to_owned())
}

/// Roda a statusline do usuário com o mesmo stdin; `None` se ela não roda ou demora demais.
fn run_user_statusline(cmd: &str, input: &str) -> Option<String> {
    let mut child = shell(cmd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        let input = input.to_owned();
        std::thread::spawn(move || {
            let _ = stdin.write_all(input.as_bytes());
        });
    }
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        let _ = tx.send(out);
    });
    match rx.recv_timeout(USER_STATUSLINE_TIMEOUT) {
        Ok(out) => {
            let _ = child.wait();
            Some(String::from_utf8_lossy(&out).into_owned())
        }
        Err(_) => {
            let _ = child.kill();
            None
        }
    }
}

/// O shell em que o Claude Code roda a statusline: `sh` no Unix; no Windows, o Git Bash
/// (`CLAUDE_CODE_GIT_BASH_PATH` ou o `bash.exe` do PATH, sem o do WSL em `System32`), ou o `cmd`.
fn shell(cmd: &str) -> Command {
    if cfg!(windows) {
        let windir = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let bash = std::env::var_os("CLAUDE_CODE_GIT_BASH_PATH").filter(|v| !v.is_empty()).map(PathBuf::from).or_else(|| {
            std::env::split_paths(&std::env::var_os("PATH")?)
                .map(|d| d.join("bash.exe"))
                .find(|p| p.is_file() && !p.to_string_lossy().to_lowercase().starts_with(&windir.to_string_lossy().to_lowercase()))
        });
        match bash {
            Some(bash) => {
                let mut c = Command::new(bash);
                c.args(["-c", cmd]);
                c
            }
            None => cmd_shell(cmd),
        }
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", cmd]);
        c
    }
}

/// `cmd /D /S /C "<comando>"` com a linha crua: a citação do Rust trocaria `"` por `\"`, que o
/// `cmd` não entende. Com `/S`, o `cmd` tira só as aspas de fora.
#[cfg(windows)]
fn cmd_shell(cmd: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut c = Command::new("cmd");
    c.raw_arg(format!("/D /S /C \"{cmd}\""));
    c
}

#[cfg(not(windows))]
fn cmd_shell(cmd: &str) -> Command {
    let mut c = Command::new("cmd");
    c.args(["/D", "/S", "/C", cmd]);
    c
}
