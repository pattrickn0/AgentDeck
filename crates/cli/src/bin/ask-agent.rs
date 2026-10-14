//! `ask-agent` — pergunta algo a outro agente pela ponte do CommandDeck (porta de bin/ask-agent.js).
//!
//! Uso: ask-agent <agente> "mensagem"     (ou "-" para ler a mensagem do stdin)
//!      ask-agent --list
//!
//! Códigos de saída: 0 = sucesso; 1 = erro operacional; 2 = erro de argumentos ou fora do CommandDeck.

use std::io::Read;
use std::process::ExitCode;

use serde_json::{json, Value};

const USAGE: &str = "Uso: ask-agent <agente> \"mensagem\"   |   ask-agent --list";

struct Bridge {
    port: String,
    token: String,
}

impl Bridge {
    /// Faz a requisição sem timeout (respostas de agentes podem levar minutos).
    fn request(&self, method: &str, route: &str, body: Option<&Value>) -> Result<(u16, Value), String> {
        let agent: ureq::Agent = ureq::Agent::config_builder().proxy(None).http_status_as_error(false).build().into();
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
    let (Some(port), Some(token)) = (env("COMMANDDECK_PORT"), env("COMMANDDECK_TOKEN")) else {
        return fail("rode este comando de dentro de um terminal do CommandDeck.", 2);
    };
    let from = env("COMMANDDECK_AGENT").unwrap_or_else(|| "shell".into());
    let depth: i64 = env("COMMANDDECK_DEPTH").and_then(|d| d.trim().parse::<f64>().ok()).map(|d| d as i64).unwrap_or(0);
    let bridge = Bridge { port, token };

    let args: Vec<String> = std::env::args().skip(1).collect();
    let to = args.first().map(String::as_str).unwrap_or("");
    if to.is_empty() || to == "-h" || to == "--help" {
        println!("{USAGE}");
        return ExitCode::from(if to.is_empty() { 2 } else { 0 });
    }
    if to == "--list" {
        return match bridge.request("GET", "/agents", None) {
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

    let mut message = args[1..].join(" ");
    if message.is_empty() || message == "-" {
        message.clear();
        if let Err(e) = std::io::stdin().read_to_string(&mut message) {
            return fail(format!("falha ao ler o stdin: {e}"), 1);
        }
    }
    if message.trim().is_empty() {
        return fail("mensagem vazia.", 2);
    }

    let body = json!({ "from": from, "to": to, "message": message, "depth": depth });
    match bridge.request("POST", "/ask", Some(&body)) {
        Ok((200, json)) => {
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
