//! `ask-agent` — pergunta algo a outro agente pela ponte do CommandDeck (porta de bin/ask-agent.js).
//!
//! Uso: ask-agent <agente> "mensagem"     (ou "-" para ler a mensagem do stdin)
//!      ask-agent --new <agente> "mensagem"   (abre outra instância, mesmo se já houver uma)
//!      ask-agent --reply <n> "resposta"       (responde à pergunta número n)
//!      ask-agent --list
//!      ask-agent --signal busy|idle           (hooks do agente: início/fim de turno)
//!
//! `<agente>` é `modelo[@cargo[@nome]]`: `claude`, `claude@dev`, `claude@dev@ana`.
//!
//! Numa aba, o `ask-agent` espera a resposta por até `COMMANDDECK_ASK_WAIT` segundos (padrão 90,
//! abaixo do timeout padrão do Bash do Claude Code; `0` = até o fim). Se o outro agente demorar
//! mais, ele termina avisando, e a resposta é digitada depois no terminal de quem perguntou.
//!
//! Códigos de saída: 0 = sucesso; 1 = erro operacional; 2 = erro de argumentos ou fora do CommandDeck.
//! `--signal` sai sempre com 0 (um hook que falha não pode atrapalhar o agente).

use std::io::Read;
use std::process::ExitCode;

use serde_json::{json, Value};

const USAGE: &str =
    "Uso: ask-agent [--new] <agente[@cargo[@nome]]> \"mensagem\"   |   ask-agent --reply <n> \"resposta\"   |   ask-agent --list";

/// Segundos que o `ask-agent` espera a resposta antes de deixá-la para ser digitada depois.
const DEFAULT_WAIT: u64 = 90;

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
    if message.is_empty() || message == "-" {
        message.clear();
        if let Err(e) = std::io::stdin().read_to_string(&mut message) {
            return fail(format!("falha ao ler o stdin: {e}"), 1);
        }
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
