//! `ask-agent` — pergunta algo a outro agente pela ponte do AgentDeck (porta de bin/ask-agent.js).
//!
//! Uso: ask-agent <agente> "mensagem"     (ou "-" para ler a mensagem do stdin; "--file <arq>" de um arquivo)
//!      ask-agent --new <agente> "mensagem"   (abre outra instância, mesmo se já houver uma)
//!      ask-agent --model <modelo> <agente> "mensagem"   (escolhe o LLM da instância ao abri-la; combina com --new)
//!      ask-agent --reply <n> "resposta"       (responde à pergunta número n)
//!      ask-agent --list
//!      ask-agent --team                       (seu chefe e sua equipe)
//!      ask-agent --models [agente]            (agentes instalados e modelos que o --model aceita)
//!      ask-agent --boss "mensagem"            (pergunta/reporta ao seu chefe)
//!      ask-agent --signal busy|idle           (hooks do agente: início/fim de turno)
//!      ask-agent --usage                      (statusline do Claude Code: uso do plano)
//!      ask-agent --learn [<arquivo>...]   (o que o projeto já aprendeu, no geral ou sobre esses arquivos)
//!      ask-agent --task new "título" [--agent <modelo>] [--model <m>] [--base <branch>] [--start] -|"pedido"|--file <arq>
//!      ask-agent --task list | show <id> | start <id> | merge <id> | validate <id> | discard <id>
//!      ask-agent --task done <id> -|"relatório"|--file <arq>   (só o agente da própria tarefa)
//!      ask-agent --task return <id> "feedback"|--file <arq>   (devolve uma tarefa em revisão)
//!      ask-agent --task reject <id> "motivo"|--file <arq>|-   (reprova uma tarefa aguardando validação)
//!
//! `<agente>` é `modelo[@cargo[@nome]]`: `claude`, `claude@dev`, `claude@dev@ana`.
//!
//! `--file` lê o texto de um arquivo (UTF-8, ou UTF-16 com BOM, como o PowerShell 5.1 grava). É o
//! caminho recomendado para textos longos ou com acentos: no Windows PowerShell, canalizar pelo shell
//! troca acentos e travessões por `?` (o `$OutputEncoding` é US-ASCII).
//!
//! Numa aba, o `ask-agent` espera a resposta por até `AGENTDECK_ASK_WAIT` segundos (padrão 90,
//! abaixo do timeout padrão do Bash do Claude Code; `0` = até o fim). Se o outro agente demorar
//! mais, ele termina avisando, e a resposta é digitada depois no terminal de quem perguntou.
//!
//! Códigos de saída: 0 = sucesso; 1 = erro operacional; 2 = erro de argumentos ou fora do AgentDeck.
//! `--signal` e `--usage` saem sempre com 0 (um hook que falha não pode atrapalhar o agente).

use std::collections::BTreeMap;
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

use agentdeck_core::usage::{from_claude_statusline, usage_to_json};
use agentdeck_core::util::decode_text;
use agentdeck_core::AgentUsage;
use serde_json::{json, Value};

const USAGE: &str =
    "Uso: ask-agent [--new] [--model <modelo>] <agente[@cargo[@nome]]> \"mensagem\"   |   ask-agent --reply <n> \"resposta\"   |   ask-agent --list   |   ask-agent --team   |   ask-agent --models [agente]   |   ask-agent --boss \"mensagem\"   |   ask-agent --learn [<arquivo>...]   |   ask-agent --task new|list|show|start|merge|validate|reject|discard|done|return ... (tarefas: ask-agent --task)";

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
            ("GET", _) => agent
                .get(&url)
                .header("x-agentdeck-token", &self.token)
                .header("x-commanddeck-token", &self.token)
                .header("content-type", "application/json")
                .call(),
            (_, Some(b)) => {
                agent.post(&url).header("x-agentdeck-token", &self.token).header("x-commanddeck-token", &self.token).send_json(b)
            }
            (_, None) => agent.post(&url).header("x-agentdeck-token", &self.token).header("x-commanddeck-token", &self.token).send_empty(),
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
    let env = |k: &str| agentdeck_core::env_var(k);
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--usage") {
        let bridge = env("PORT").zip(env("TOKEN")).map(|(port, token)| Bridge { port, token });
        return statusline(bridge);
    }
    let (Some(port), Some(token)) = (env("PORT"), env("TOKEN")) else {
        if args.first().is_some_and(|a| a == "--signal") {
            return ExitCode::SUCCESS;
        }
        return fail("rode este comando de dentro de um terminal do AgentDeck.", 2);
    };
    let from = env("AGENT").unwrap_or_else(|| "shell".into());
    let depth: i64 = env("DEPTH").and_then(|d| d.trim().parse::<f64>().ok()).map(|d| d as i64).unwrap_or(0);
    let bridge = Bridge { port, token };

    if args.first().is_some_and(|a| a == "--signal") {
        // Hook do agente (o Codex ainda acrescenta um JSON, ignorado). Falhas são silenciosas.
        let state = args.get(1).map(String::as_str).unwrap_or("");
        let _ = bridge.request("POST", "/signal", Some(&json!({ "state": state })), Some(std::time::Duration::from_secs(3)));
        return ExitCode::SUCCESS;
    }
    if args.first().is_some_and(|a| a == "--task") {
        return task_command(&bridge, &args[1..]);
    }
    if args.first().is_some_and(|a| a == "--learn") {
        return learn_command(&bridge, &args[1..]);
    }
    if args.first().is_some_and(|a| a == "--models") {
        return models_command(&bridge, args.get(1).map(String::as_str));
    }
    let (force_new, model) = match take_flags(&mut args) {
        Ok(v) => v,
        Err(e) => return fail(format!("{e}\n{USAGE}"), 2),
    };
    let file_body = match take_body_file(&mut args) {
        Ok(p) => p,
        Err(e) => return fail(format!("{e}\n{USAGE}"), 2),
    };
    let mut reply_to: Option<u64> = None;
    if args.first().is_some_and(|a| a == "--reply") {
        match args.get(1).and_then(|n| n.trim_start_matches('#').parse().ok()) {
            Some(n) => reply_to = Some(n),
            None => return fail(format!("--reply precisa do número da pergunta.\n{USAGE}"), 2),
        }
        // Daqui em diante, `args[0]` faz o papel do destino (ignorado) e o resto é a mensagem.
        args.drain(..2);
        args.insert(0, String::new());
        if model.is_some() {
            return fail(format!("--model não combina com --reply (o modelo vale ao abrir uma instância).\n{USAGE}"), 2);
        }
    }
    if args.first().is_some_and(|a| a == "--team") {
        return match team(&bridge) {
            Ok(t) => {
                println!("Você: {}", with_model(&t.id, &t.models));
                match &t.boss {
                    Some(b) => println!("Chefe: {} (reporte a ele com: ask-agent --boss \"mensagem\")", with_model(b, &t.models)),
                    None => println!("Chefe: nenhum (você responde diretamente ao usuário)"),
                }
                if t.crew.is_empty() {
                    println!("Equipe: nenhum agente");
                } else {
                    println!("Equipe: {}", t.crew.iter().map(|c| with_model(c, &t.models)).collect::<Vec<_>>().join(", "));
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
        if model.is_some() {
            return fail(format!("--model não combina com --boss (o chefe é uma instância que já existe).\n{USAGE}"), 2);
        }
        match team(&bridge) {
            Ok(Team { boss: Some(boss), .. }) => args[0] = boss,
            Ok(_) => return fail("você não tem chefe: responde diretamente ao usuário (escreva a resposta no seu terminal).", 1),
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
                    let models = models_of(&json);
                    let names: Vec<String> = list.iter().filter_map(Value::as_str).map(|n| with_model(n, &models)).collect();
                    println!("{}", names.join("\n"));
                    ExitCode::SUCCESS
                }
                None => fail(json.get("error").and_then(Value::as_str).unwrap_or("resposta inválida da ponte"), 1),
            },
            Err(e) => fail(e, 1),
        };
    }

    let mut message = match &file_body {
        Some(path) => match read_text_file(path) {
            Ok(text) => text,
            Err(e) => return fail(e, 2),
        },
        None => args.get(1..).map(|a| a.join(" ")).unwrap_or_default(),
    };
    // O stdin só é lido com "-" ou quando vem de um pipe/arquivo: num terminal, sem mensagem,
    // a leitura travaria sem aviso. `decode_text`: UTF-8 (ou UTF-16 com BOM) sem erro, e arquivos
    // em cp1252 viram substituições em vez de falhar.
    if file_body.is_none() && (message == "-" || (message.is_empty() && !std::io::stdin().is_terminal())) {
        let mut raw = Vec::new();
        if let Err(e) = std::io::stdin().read_to_end(&mut raw) {
            return fail(format!("falha ao ler o stdin: {e}"), 1);
        }
        message = decode_text(&raw);
    }
    if message.trim().is_empty() {
        return fail("mensagem vazia.", 2);
    }

    let wait = env("ASK_WAIT").and_then(|w| w.trim().parse::<u64>().ok()).unwrap_or(DEFAULT_WAIT);
    let mut body = json!({ "from": from, "to": to, "message": message, "depth": depth, "new": force_new, "wait": wait });
    if let Some(n) = reply_to {
        body["reply_to"] = json!(n);
    }
    if let Some(m) = &model {
        body["model"] = json!(m);
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

/// Opções do começo da linha de comando, em qualquer ordem: `--new`, `--model <m>` e `--model=<m>`.
/// Devolve `(--new, modelo)` e deixa em `args` o resto (destino e mensagem).
fn take_flags(args: &mut Vec<String>) -> Result<(bool, Option<String>), String> {
    let (mut force_new, mut model) = (false, None);
    loop {
        match args.first().map(String::as_str) {
            Some("--new") => force_new = true,
            Some("--model") => {
                if args.len() < 2 || args[1].is_empty() || args[1].starts_with("--") {
                    return Err("--model precisa do nome do modelo (ex.: --model sonnet).".into());
                }
                model = Some(args.remove(1));
            }
            Some(a) if a.starts_with("--model=") => {
                let m = a["--model=".len()..].to_owned();
                if m.is_empty() {
                    return Err("--model precisa do nome do modelo (ex.: --model=sonnet).".into());
                }
                model = Some(m);
            }
            _ => return Ok((force_new, model)),
        }
        args.remove(0);
    }
}

/// `--models [agente]`: os agentes instalados neste computador e os modelos que o `--model` aceita.
fn models_command(bridge: &Bridge, only: Option<&str>) -> ExitCode {
    let route = match only {
        Some(a) => format!("/models?agent={a}"),
        None => "/models".to_owned(),
    };
    // Os CLIs podem levar até 15 s na primeira consulta (depois vem do cache da ponte).
    match bridge.request("GET", &route, None, Some(Duration::from_secs(40))) {
        Ok((200, json)) => {
            println!("{}", json.get("text").and_then(Value::as_str).unwrap_or_default());
            ExitCode::SUCCESS
        }
        Ok((_, json)) => fail(json.get("error").and_then(Value::as_str).unwrap_or("erro desconhecido"), 1),
        Err(e) => fail(e, 1),
    }
}

/// `claude@dev (sonnet)` quando a instância tem modelo.
fn with_model(id: &str, models: &BTreeMap<String, String>) -> String {
    match models.get(id) {
        Some(m) => format!("{id} ({m})"),
        None => id.to_owned(),
    }
}

/// O mapa `models` (id → modelo) de uma resposta da ponte.
fn models_of(json: &Value) -> BTreeMap<String, String> {
    json.get("models")
        .and_then(Value::as_object)
        .map(|o| o.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned()))).collect())
        .unwrap_or_default()
}

/// Resposta de `GET /team`.
struct Team {
    id: String,
    boss: Option<String>,
    crew: Vec<String>,
    /// Modelo de quem aparece acima (id → modelo).
    models: BTreeMap<String, String>,
}

/// `GET /team`: eu, chefe, equipe e os modelos.
fn team(bridge: &Bridge) -> Result<Team, String> {
    let (status, json) = bridge.request("GET", "/team", None, Some(Duration::from_secs(10)))?;
    if status != 200 {
        return Err(json.get("error").and_then(Value::as_str).unwrap_or("erro desconhecido").to_owned());
    }
    let id = json.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
    let boss = json.get("boss").and_then(Value::as_str).map(str::to_owned);
    let crew = json.get("crew").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect());
    Ok(Team { id, boss, crew: crew.unwrap_or_default(), models: models_of(&json) })
}

// ---------- Aprendizados (`--learn`) ----------

const LEARN_USAGE: &str = "Uso: ask-agent --learn [<arquivo>...]   (o que o projeto já aprendeu, no geral ou sobre esses arquivos)";

/// `ask-agent --learn [<caminho>...]`: imprime os aprendizados (`memory/aprendizados/`) que valem para os
/// arquivos; sem caminhos, os globais e o índice dos demais.
fn learn_command(bridge: &Bridge, paths: &[String]) -> ExitCode {
    if paths.iter().any(|p| p == "-h" || p == "--help") {
        println!("{LEARN_USAGE}");
        return ExitCode::SUCCESS;
    }
    match bridge.request("POST", "/learn", Some(&json!({ "paths": paths })), Some(Duration::from_secs(15))) {
        Ok((200, json)) => {
            println!("{}", json.get("text").and_then(Value::as_str).unwrap_or(""));
            ExitCode::SUCCESS
        }
        Ok((_, json)) => fail(json.get("error").and_then(Value::as_str).unwrap_or("erro desconhecido"), 1),
        Err(e) => fail(e, 1),
    }
}

// ---------- Tarefas (`--task`) ----------

const TASK_USAGE: &str = "Uso: ask-agent --task new \"título\" [--agent <modelo>] [--model <m>] [--base <branch>] [--start] -|\"pedido\"|--file <arq>   |   ask-agent --task list   |   ask-agent --task show <id>   |   ask-agent --task start|merge|validate|discard <id>   |   ask-agent --task done <id> -|\"relatório\"|--file <arq>   |   ask-agent --task return|reject <id> \"feedback\"|--file <arq>|-";

/// De onde vem um texto longo (pedido, relatório, feedback).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Body {
    /// Escrito na linha de comando.
    Inline(String),
    /// `-`: entrada padrão.
    Stdin,
    /// `--file <caminho>`: lido do arquivo (UTF-8, ou UTF-16 com BOM).
    File(String),
    /// Não foi dado: vale a entrada padrão se ela for um pipe/arquivo.
    Missing,
}

/// Um subcomando de `--task`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TaskCmd {
    New { title: String, brief: Body, agent: Option<String>, model: Option<String>, base: Option<String>, start: bool },
    List,
    Show(String),
    Start(String),
    Merge(String),
    Validate(String),
    Discard(String),
    Done { id: String, report: Body },
    Return { id: String, feedback: Body },
    Reject { id: String, feedback: Body },
}

/// `-` sozinho = entrada padrão; `--file <caminho>` = arquivo; texto = linha de comando; nada = `Missing`.
fn body_of(words: &[String]) -> Result<Body, String> {
    match words {
        [] => Ok(Body::Missing),
        [dash] if dash == "-" => Ok(Body::Stdin),
        [flag, path] if flag == "--file" => Ok(Body::File(path.clone())),
        [one] if one.starts_with("--file=") => Ok(Body::File(one["--file=".len()..].to_owned())),
        [flag, ..] if flag == "--file" => Err("--file precisa do caminho do arquivo.".into()),
        words => Ok(Body::Inline(words.join(" "))),
    }
}

/// Tira um `--file <caminho>` (ou `--file=<caminho>`) da linha de comando e devolve o caminho.
/// Vale para a mensagem genérica (`ask-agent <agente> --file <arq>`, `--reply ... --file <arq>`).
fn take_body_file(args: &mut Vec<String>) -> Result<Option<String>, String> {
    let mut path = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--file" {
            let value = args.get(i + 1).cloned().filter(|v| !v.is_empty() && !v.starts_with("--"));
            path = Some(value.ok_or("--file precisa do caminho do arquivo.")?);
            args.drain(i..=i + 1);
        } else if let Some(p) = args[i].strip_prefix("--file=") {
            path = Some(p.to_owned());
            args.remove(i);
        } else {
            i += 1;
        }
    }
    Ok(path)
}

/// O id de tarefa (`t3`, `3`) como a ponte o espera.
fn task_id(arg: Option<&String>) -> Result<String, String> {
    let arg = arg.ok_or("falta o id da tarefa (ex.: t3).")?;
    agentdeck_core::tasks::parse_task_id(arg).ok_or_else(|| format!("id de tarefa inválido: \"{arg}\" (use t3 ou 3)."))
}

/// Lê o que vem depois de `--task`.
fn parse_task(args: &[String]) -> Result<TaskCmd, String> {
    let Some(sub) = args.first().map(String::as_str) else { return Err("falta o subcomando de --task.".into()) };
    let rest = &args[1..];
    match sub {
        "list" | "ls" => match rest {
            [] => Ok(TaskCmd::List),
            _ => Err("--task list não recebe argumentos.".into()),
        },
        "show" | "start" | "merge" | "validate" | "discard" => {
            if rest.len() > 1 {
                return Err(format!("--task {sub} recebe só o id da tarefa."));
            }
            let id = task_id(rest.first())?;
            Ok(match sub {
                "show" => TaskCmd::Show(id),
                "start" => TaskCmd::Start(id),
                "merge" => TaskCmd::Merge(id),
                "validate" => TaskCmd::Validate(id),
                _ => TaskCmd::Discard(id),
            })
        }
        "done" => Ok(TaskCmd::Done { id: task_id(rest.first())?, report: body_of(&rest[1..])? }),
        "return" => Ok(TaskCmd::Return { id: task_id(rest.first())?, feedback: body_of(&rest[1..])? }),
        "reject" => Ok(TaskCmd::Reject { id: task_id(rest.first())?, feedback: body_of(&rest[1..])? }),
        "new" => {
            let (mut agent, mut model, mut base, mut start) = (None, None, None, false);
            let mut brief_file: Option<String> = None;
            let mut words: Vec<String> = Vec::new();
            let mut it = rest.iter();
            while let Some(a) = it.next() {
                let mut value = |name: &str, inline: Option<&str>| -> Result<String, String> {
                    let v = match inline {
                        Some(v) => Some(v.to_owned()),
                        None => it.next().cloned(),
                    };
                    v.filter(|v| !v.is_empty() && !v.starts_with("--")).ok_or_else(|| format!("{name} precisa de um valor."))
                };
                match a.as_str() {
                    "--start" => start = true,
                    "--agent" => agent = Some(value("--agent", None)?),
                    "--model" => model = Some(value("--model", None)?),
                    "--base" => base = Some(value("--base", None)?),
                    "--file" => brief_file = Some(value("--file", None)?),
                    s if s.starts_with("--agent=") => agent = Some(value("--agent", Some(&s["--agent=".len()..]))?),
                    s if s.starts_with("--model=") => model = Some(value("--model", Some(&s["--model=".len()..]))?),
                    s if s.starts_with("--base=") => base = Some(value("--base", Some(&s["--base=".len()..]))?),
                    s if s.starts_with("--file=") => brief_file = Some(value("--file", Some(&s["--file=".len()..]))?),
                    s if s.starts_with("--") && s.len() > 2 => return Err(format!("opção desconhecida de --task new: {s}")),
                    _ => words.push(a.clone()),
                }
            }
            if words.is_empty() || words[0].trim().is_empty() || words[0] == "-" {
                return Err("falta o título da tarefa: ask-agent --task new \"título\" ... -".into());
            }
            let title = words.remove(0);
            let brief = match brief_file {
                Some(path) => Body::File(path),
                None => body_of(&words)?,
            };
            Ok(TaskCmd::New { title, brief, agent, model, base, start })
        }
        other => Err(format!("subcomando desconhecido de --task: {other}")),
    }
}

/// Texto de um `Body`: a linha de comando, a entrada padrão ou um arquivo. Sem nada e num terminal,
/// erro (ler travaria sem aviso).
fn read_body(body: Body, what: &str) -> Result<String, String> {
    let text = match body {
        Body::Inline(text) => text,
        Body::Stdin => read_stdin()?,
        Body::File(path) => read_text_file(&path)?,
        Body::Missing if !std::io::stdin().is_terminal() => read_stdin()?,
        Body::Missing => {
            return Err(format!("falta {what}: passe o texto como argumento, pela entrada padrão (-) ou em --file <arquivo>."))
        }
    };
    Ok(text)
}

fn read_stdin() -> Result<String, String> {
    let mut raw = Vec::new();
    std::io::stdin().read_to_end(&mut raw).map_err(|e| format!("falha ao ler o stdin: {e}"))?;
    Ok(decode_text(&raw))
}

/// Lê um arquivo como texto (UTF-8 ou UTF-16 com BOM; ver `decode_text`). É o caminho que mantém os
/// acentos: o shell do agente (PowerShell) os trocaria por `?` ao canalizar pelo `stdin`.
fn read_text_file(path: &str) -> Result<String, String> {
    let raw = std::fs::read(path).map_err(|e| format!("falha ao ler o arquivo {path}: {e}"))?;
    Ok(decode_text(&raw))
}

/// Texto de um campo da tarefa na resposta da ponte.
fn field<'a>(task: &'a Value, key: &str) -> &'a str {
    task.get(key).and_then(Value::as_str).unwrap_or("")
}

fn status_label(status: &str) -> &str {
    match status {
        "draft" => "rascunho",
        "running" => "andamento",
        "review" => "revisão",
        "validating" => "validação",
        "merged" => "concluída",
        "discarded" => "descartada",
        "failed" => "erro",
        other => other,
    }
}

/// `claude@t3 (sonnet)` ou só a instância.
fn who(task: &Value) -> String {
    match task.get("model").and_then(Value::as_str) {
        Some(m) => format!("{} ({m})", field(task, "instance")),
        None => field(task, "instance").to_owned(),
    }
}

/// Uma linha de `--task list`: id, estado, agente com modelo e título.
fn task_line(task: &Value) -> String {
    format!("{:<5} {:<10} {:<26} {}", field(task, "id"), status_label(field(task, "status")), who(task), field(task, "title"))
}

/// `--task show`: tudo o que a pessoa (ou o agente) precisa para revisar.
fn task_detail(task: &Value) -> String {
    let mut out = format!("Tarefa {}: {}\n", field(task, "id"), field(task, "title"));
    out.push_str(&format!("Estado: {}\n", status_label(field(task, "status"))));
    out.push_str(&format!("Agente: {}\n", who(task)));
    out.push_str(&format!("Base: {} · branch: {}\n", field(task, "base"), field(task, "branch")));
    let wt = field(task, "worktree");
    if !wt.is_empty() {
        out.push_str(&format!("Worktree: {wt}\n"));
    }
    if let Some(c) = task.get("creator").and_then(Value::as_str) {
        out.push_str(&format!("Criada por: {c}\n"));
    }
    if let Some(e) = task.get("error").and_then(Value::as_str) {
        out.push_str(&format!("Erro: {e}\n"));
    }
    if let Some(stat) = task.get("diff_stat").and_then(Value::as_str) {
        out.push_str(&format!("\nAlterações (git diff --stat {}...{}):\n{stat}\n", field(task, "base"), field(task, "branch")));
    }
    if let Some(report) = task.get("report").and_then(Value::as_str) {
        out.push_str(&format!("\nRelatório:\n{report}\n"));
    }
    if let Some(events) = task.get("events").and_then(Value::as_array) {
        out.push_str("\nHistórico:\n");
        let skip = events.len().saturating_sub(10);
        for e in events.iter().skip(skip) {
            out.push_str(&format!("  - {}\n", field(e, "text")));
        }
    }
    out
}

/// `ask-agent --task ...`: fala com `/tasks` da ponte.
fn task_command(bridge: &Bridge, args: &[String]) -> ExitCode {
    let cmd = match parse_task(args) {
        Ok(c) => c,
        Err(e) => return fail(format!("{e}\n{TASK_USAGE}"), 2),
    };
    // Corpo já lido (a entrada padrão antes de falar com a ponte) e o pedido HTTP.
    let (method, route, body) = match cmd {
        TaskCmd::List => ("GET", "/tasks".to_owned(), None),
        TaskCmd::Show(id) => ("GET", format!("/tasks/{id}"), None),
        TaskCmd::Start(id) => ("POST", format!("/tasks/{id}/start"), Some(json!({}))),
        TaskCmd::Merge(id) => ("POST", format!("/tasks/{id}/merge"), Some(json!({}))),
        TaskCmd::Validate(id) => ("POST", format!("/tasks/{id}/validate"), Some(json!({}))),
        TaskCmd::Discard(id) => ("POST", format!("/tasks/{id}/discard"), Some(json!({}))),
        TaskCmd::Done { id, report } => match read_body(report, "o relatório") {
            Ok(r) if r.trim().is_empty() => return fail("relatório vazio: diga o que foi feito, os arquivos, os testes e o que falta.", 2),
            Ok(r) => ("POST", format!("/tasks/{id}/done"), Some(json!({ "report": r }))),
            Err(e) => return fail(e, 2),
        },
        TaskCmd::Return { id, feedback } => match read_body(feedback, "o feedback") {
            Ok(f) if f.trim().is_empty() => return fail("feedback vazio: diga o que precisa mudar.", 2),
            Ok(f) => ("POST", format!("/tasks/{id}/return"), Some(json!({ "feedback": f }))),
            Err(e) => return fail(e, 2),
        },
        TaskCmd::Reject { id, feedback } => match read_body(feedback, "o motivo") {
            Ok(f) if f.trim().is_empty() => return fail("motivo vazio: diga o que não funcionou no teste.", 2),
            Ok(f) => ("POST", format!("/tasks/{id}/reject"), Some(json!({ "feedback": f }))),
            Err(e) => return fail(e, 2),
        },
        TaskCmd::New { title, brief, agent, model, base, start } => match read_body(brief, "o pedido da tarefa") {
            Ok(b) => {
                let mut body = json!({ "title": title, "brief": b, "start": start });
                for (key, value) in [("kind", agent), ("model", model), ("base", base)] {
                    if let Some(v) = value {
                        body[key] = json!(v);
                    }
                }
                ("POST", "/tasks".to_owned(), Some(body))
            }
            Err(e) => return fail(e, 2),
        },
    };
    let new = route == "/tasks" && method == "POST";
    let verb = route.rsplit('/').next().unwrap_or("").to_owned();
    // Iniciar ou mesclar roda o git (pode levar um tempo): o limite é folgado.
    let (status, json) = match bridge.request(method, &route, body.as_ref(), Some(Duration::from_secs(600))) {
        Ok(v) => v,
        Err(e) => return fail(e, 1),
    };
    if status != 200 {
        return fail(json.get("error").and_then(Value::as_str).unwrap_or("erro desconhecido"), 1);
    }
    if let Some(list) = json.get("tasks").and_then(Value::as_array) {
        if list.is_empty() {
            println!("Nenhuma tarefa neste projeto.");
        }
        for task in list {
            println!("{}", task_line(task));
        }
        return ExitCode::SUCCESS;
    }
    let task = json.get("task").cloned().unwrap_or(Value::Null);
    let id = field(&task, "id").to_owned();
    match (new, verb.as_str()) {
        (true, _) => {
            println!("Tarefa {id} criada: {}", field(&task, "title"));
            println!("  agente: {} · base: {} · branch: {}", who(&task), field(&task, "base"), field(&task, "branch"));
            if field(&task, "status") == "running" {
                println!("  iniciada na worktree {}", field(&task, "worktree"));
                println!(
                    "  o agente avisa com: ask-agent --task done {id} --file <arquivo>  (o relatório fica em: ask-agent --task show {id})"
                );
            } else {
                println!("  inicie com: ask-agent --task start {id}");
            }
        }
        (_, "start") => println!("Tarefa {id} iniciada: {} trabalha na worktree {}", who(&task), field(&task, "worktree")),
        (_, "merge") => {
            println!("Tarefa {id} mesclada em {}; aguardando validação (o agente, a worktree e a branch continuam).", field(&task, "base"))
        }
        (_, "validate") => println!("Tarefa {id} validada e concluída; worktree e branch removidas."),
        (_, "reject") => println!("Tarefa {id} reprovada; {} recebeu o motivo e volta a trabalhar.", field(&task, "instance")),
        (_, "discard") => println!("Tarefa {id} descartada; worktree e branch removidas."),
        (_, "done") => println!("Relatório da tarefa {id} entregue; ela está em revisão."),
        (_, "return") => println!("Tarefa {id} devolvida a {} com o seu feedback.", field(&task, "instance")),
        _ => print!("{}", task_detail(&task)),
    }
    ExitCode::SUCCESS
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
    let pct = |w: Option<agentdeck_core::UsageWindow>| w.map(|w| format!("{:.0}%", w.used * 100.0));
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
        .unwrap_or_else(|| agentdeck_core::util::home_dir().join(".claude"));
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

#[cfg(test)]
mod tests {
    use super::*;

    type Parsed = (Result<(bool, Option<String>), String>, Vec<String>);

    fn flags(args: &[&str]) -> Parsed {
        let mut v: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        (take_flags(&mut v), v)
    }

    fn rest(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    fn words(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    fn task(args: &[&str]) -> Result<TaskCmd, String> {
        parse_task(&words(args))
    }

    #[test]
    fn task_new_forms_are_parsed() {
        let new = |title: &str, brief: Body, agent: Option<&str>, model: Option<&str>, base: Option<&str>, start: bool| TaskCmd::New {
            title: title.into(),
            brief,
            agent: agent.map(String::from),
            model: model.map(String::from),
            base: base.map(String::from),
            start,
        };
        assert_eq!(task(&["new", "Corrigir login", "-"]), Ok(new("Corrigir login", Body::Stdin, None, None, None, false)));
        assert_eq!(task(&["new", "Corrigir login"]), Ok(new("Corrigir login", Body::Missing, None, None, None, false)));
        assert_eq!(
            task(&["new", "T", "Faça", "isto", "agora"]),
            Ok(new("T", Body::Inline("Faça isto agora".into()), None, None, None, false))
        );
        assert_eq!(
            task(&["new", "T", "--agent", "opencode", "--model", "opencode/glm", "--base", "dev", "--start", "-"]),
            Ok(new("T", Body::Stdin, Some("opencode"), Some("opencode/glm"), Some("dev"), true))
        );
        // As opções valem em qualquer posição e com `=`.
        assert_eq!(
            task(&["new", "--start", "--agent=claude", "T", "--model=sonnet", "--base=main", "pedido"]),
            Ok(new("T", Body::Inline("pedido".into()), Some("claude"), Some("sonnet"), Some("main"), true))
        );
        // O pedido vem de um arquivo (mantém os acentos): `--file <arq>` ou `--file=<arq>`.
        assert_eq!(task(&["new", "T", "--file", "pedido.md"]), Ok(new("T", Body::File("pedido.md".into()), None, None, None, false)));
        assert_eq!(
            task(&["new", "T", "--start", "--file=pedido.md"]),
            Ok(new("T", Body::File("pedido.md".into()), None, None, None, true))
        );
        for bad in [
            &["new"][..],
            &["new", "-"],
            &["new", "  "],
            &["new", "T", "--agent"],
            &["new", "T", "--model", "--start"],
            &["new", "T", "--base="],
            &["new", "T", "--agente", "x"],
            &["new", "T", "--file"],
            &["new", "T", "--file="],
        ] {
            assert!(task(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn task_other_forms_are_parsed() {
        assert_eq!(task(&["list"]), Ok(TaskCmd::List));
        assert_eq!(task(&["ls"]), Ok(TaskCmd::List));
        assert_eq!(task(&["show", "t3"]), Ok(TaskCmd::Show("t3".into())));
        assert_eq!(task(&["show", "3"]), Ok(TaskCmd::Show("t3".into())));
        assert_eq!(task(&["start", "T4"]), Ok(TaskCmd::Start("t4".into())));
        assert_eq!(task(&["merge", "t5"]), Ok(TaskCmd::Merge("t5".into())));
        assert_eq!(task(&["validate", "5"]), Ok(TaskCmd::Validate("t5".into())));
        assert_eq!(task(&["discard", "t6"]), Ok(TaskCmd::Discard("t6".into())));
        assert_eq!(task(&["done", "t3", "-"]), Ok(TaskCmd::Done { id: "t3".into(), report: Body::Stdin }));
        assert_eq!(task(&["done", "t3"]), Ok(TaskCmd::Done { id: "t3".into(), report: Body::Missing }));
        assert_eq!(
            task(&["done", "t3", "feito", "tudo"]),
            Ok(TaskCmd::Done { id: "t3".into(), report: Body::Inline("feito tudo".into()) })
        );
        assert_eq!(
            task(&["return", "t3", "Faltou o teste."]),
            Ok(TaskCmd::Return { id: "t3".into(), feedback: Body::Inline("Faltou o teste.".into()) })
        );
        assert_eq!(task(&["return", "t3", "-"]), Ok(TaskCmd::Return { id: "t3".into(), feedback: Body::Stdin }));
        assert_eq!(
            task(&["reject", "t3", "Não", "salva."]),
            Ok(TaskCmd::Reject { id: "t3".into(), feedback: Body::Inline("Não salva.".into()) })
        );
        assert_eq!(task(&["reject", "t3", "-"]), Ok(TaskCmd::Reject { id: "t3".into(), feedback: Body::Stdin }));
        assert_eq!(task(&["reject", "t3", "--file", "m.md"]), Ok(TaskCmd::Reject { id: "t3".into(), feedback: Body::File("m.md".into()) }));
        // Relatório/feedback de um arquivo (mantém os acentos).
        assert_eq!(task(&["done", "t3", "--file", "r.md"]), Ok(TaskCmd::Done { id: "t3".into(), report: Body::File("r.md".into()) }));
        assert_eq!(task(&["done", "t3", "--file=r.md"]), Ok(TaskCmd::Done { id: "t3".into(), report: Body::File("r.md".into()) }));
        assert_eq!(task(&["return", "t3", "--file", "f.md"]), Ok(TaskCmd::Return { id: "t3".into(), feedback: Body::File("f.md".into()) }));
        for bad in [
            &[][..],
            &["explodir"],
            &["list", "x"],
            &["show"],
            &["show", "claude@t3"],
            &["start", "t1", "t2"],
            &["done"],
            &["done", "t3", "--file"],
            &["done", "xyz", "-"],
            &["return"],
            &["reject"],
            &["validate"],
            &["validate", "t1", "t2"],
        ] {
            assert!(task(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn task_output_is_readable() {
        let t = json!({
            "id": "t3", "title": "Corrigir login", "status": "review", "base": "main", "branch": "tarefa/t3-corrigir-login",
            "instance": "claude@t3", "model": "sonnet", "creator": "claude", "worktree": "/r/.agentdeck/worktrees/t3",
            "diff_stat": " a.rs | 2 +-", "report": "Feito.", "events": [{ "at_ms": 1, "text": "criada" }, { "at_ms": 2, "text": "iniciada" }]
        });
        assert_eq!(task_line(&t), "t3    revisão    claude@t3 (sonnet)         Corrigir login");
        let d = task_detail(&t);
        for needle in [
            "Tarefa t3: Corrigir login",
            "Estado: revisão",
            "Agente: claude@t3 (sonnet)",
            "Base: main · branch: tarefa/t3-corrigir-login",
            "Criada por: claude",
            "a.rs | 2 +-",
            "Relatório:\nFeito.",
            "  - iniciada",
        ] {
            assert!(d.contains(needle), "{needle}\n{d}");
        }
        let draft = json!({ "id": "t4", "title": "T", "status": "draft", "instance": "ollama:qwen3.5:4b@t4" });
        assert_eq!(task_line(&draft), "t4    rascunho   ollama:qwen3.5:4b@t4       T");
    }

    #[test]
    fn model_flag_is_parsed_in_any_order_before_the_target() {
        assert_eq!(flags(&["claude@dev", "oi"]), (Ok((false, None)), rest(&["claude@dev", "oi"])));
        assert_eq!(flags(&["--new", "claude", "oi"]).0, Ok((true, None)));
        assert_eq!(flags(&["--model", "sonnet", "claude@dev", "oi"]), (Ok((false, Some("sonnet".into()))), rest(&["claude@dev", "oi"])));
        assert_eq!(flags(&["--new", "--model", "opencode/glm-5.3", "opencode", "oi"]).0, Ok((true, Some("opencode/glm-5.3".into()))));
        assert_eq!(flags(&["--model", "opus", "--new", "claude", "oi"]).0, Ok((true, Some("opus".into()))));
        assert_eq!(flags(&["--model=haiku", "claude", "oi"]), (Ok((false, Some("haiku".into()))), rest(&["claude", "oi"])));
        // O que vem depois do destino é mensagem, mesmo que pareça uma opção.
        assert_eq!(flags(&["claude", "--model", "x"]), (Ok((false, None)), rest(&["claude", "--model", "x"])));
    }

    #[test]
    fn model_flag_needs_a_name() {
        for bad in [&["--model"][..], &["--model", "--new", "claude"], &["--model", "", "claude"], &["--model="]] {
            assert!(flags(bad).0.is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_message_body_can_come_from_a_file() {
        for (args, path, left) in [
            (&["claude", "oi"][..], None, &["claude", "oi"][..]),
            (&["claude", "--file", "m.md"], Some("m.md"), &["claude"]),
            (&["--reply", "3", "--file=m.md"], Some("m.md"), &["--reply", "3"]),
        ] {
            let mut v = words(args);
            assert_eq!(take_body_file(&mut v), Ok(path.map(String::from)), "{args:?}");
            assert_eq!(v, words(left));
        }
        for bad in [&["claude", "--file"][..], &["claude", "--file", "--new"]] {
            let mut v = words(bad);
            assert!(take_body_file(&mut v).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn models_show_next_to_the_instances() {
        let json = json!({ "agents": ["claude", "claude@dev"], "models": { "claude@dev": "sonnet", "x": 1 } });
        let models = models_of(&json);
        assert_eq!(with_model("claude@dev", &models), "claude@dev (sonnet)");
        assert_eq!(with_model("claude", &models), "claude");
        assert!(models_of(&json!({})).is_empty());
    }
}
