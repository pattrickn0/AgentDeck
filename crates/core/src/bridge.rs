//! Ponte HTTP local agente → agente (equivalente a `startBridge`).
//!
//! Escuta só em 127.0.0.1 (porta efêmera), exige o token `x-commanddeck-token` e valida o
//! cabeçalho `Host` contra DNS rebinding. Requisições para o mesmo par `de->para` são
//! atendidas em ordem de chegada (fila FIFO por par), uma de cada vez.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use parking_lot::{Condvar, Mutex};
use serde_json::{json, Value};
use tiny_http::{Header, Method, Request, Response, Server};

use crate::agents::resolve_alias;
use crate::events::{CoreEvent, FeedEntry, FeedKind};
use crate::headless::HeadlessOptions;
use crate::{Core, CoreInner};

/// agente → agente → agente, no máximo.
pub const MAX_DEPTH: i64 = 2;

// ---------- Fila FIFO por par ----------

#[derive(Default)]
struct Ticketing {
    next: u64,
    serving: u64,
}

/// Fila de um par (`de->para`): senhas atendidas em ordem.
#[derive(Default)]
pub(crate) struct PairQueue {
    state: Mutex<Ticketing>,
    cv: Condvar,
}

/// Vez na fila; ao ser solta, chama o próximo.
pub(crate) struct Turn {
    queue: Arc<PairQueue>,
}

impl Drop for Turn {
    fn drop(&mut self) {
        self.queue.state.lock().serving += 1;
        self.queue.cv.notify_all();
    }
}

/// Filas por chave de sessão (equivalente a `bridgeQueues`).
#[derive(Default)]
pub(crate) struct PairQueues {
    queues: Mutex<HashMap<String, Arc<PairQueue>>>,
}

impl PairQueues {
    /// Pega uma senha (na ordem de chamada) e bloqueia até ser a vez dela.
    pub fn enter(&self, key: &str) -> Turn {
        let queue = self.queues.lock().entry(key.to_owned()).or_default().clone();
        let mut st = queue.state.lock();
        let ticket = st.next;
        st.next += 1;
        while st.serving != ticket {
            queue.cv.wait(&mut st);
        }
        drop(st);
        Turn { queue }
    }
}

// ---------- Servidor ----------

/// Sobe o servidor em 127.0.0.1:0 e devolve (servidor, porta).
pub(crate) fn start(core: &Core) -> anyhow::Result<(Arc<Server>, u16)> {
    let server = Server::http("127.0.0.1:0").map_err(|e| anyhow::anyhow!("falha ao iniciar a ponte HTTP: {e}"))?;
    let port = server.server_addr().to_ip().map(|a| a.port()).ok_or_else(|| anyhow::anyhow!("ponte sem porta TCP"))?;
    let server = Arc::new(server);
    let srv = server.clone();
    let weak = Arc::downgrade(&core.inner);
    std::thread::Builder::new().name("cd-bridge".into()).spawn(move || {
        // Termina quando `Server::unblock` é chamado (shutdown).
        for req in srv.incoming_requests() {
            let weak = weak.clone();
            let _ = std::thread::Builder::new().name("cd-bridge-req".into()).spawn(move || handle(weak, req));
        }
    })?;
    Ok((server, port))
}

fn reply(req: Request, status: u16, body: Value) {
    let text = serde_json::to_string(&body).unwrap_or_else(|_| "{}".into());
    let mut resp = Response::from_string(text).with_status_code(status);
    if let Ok(h) = Header::from_bytes("content-type", "application/json") {
        resp = resp.with_header(h);
    }
    let _ = req.respond(resp);
}

fn header<'a>(req: &'a Request, name: &'static str) -> Option<&'a str> {
    req.headers().iter().find(|h| h.field.equiv(name)).map(|h| h.value.as_str())
}

fn handle(weak: Weak<CoreInner>, mut req: Request) {
    let Some(inner) = weak.upgrade() else {
        return reply(req, 503, json!({ "error": "CommandDeck encerrando" }));
    };
    let core = Core { inner };
    if header(&req, "x-commanddeck-token") != Some(core.bridge_token()) {
        return reply(req, 403, json!({ "error": "token inválido" }));
    }
    // Validação de Host contra DNS rebinding.
    let port = core.bridge_port();
    let host = header(&req, "host").unwrap_or("");
    if port != 0 && !host.is_empty() && host != format!("127.0.0.1:{port}") && host != format!("localhost:{port}") {
        return reply(req, 403, json!({ "error": "host inválido" }));
    }

    if *req.method() == Method::Get && req.url() == "/agents" {
        return reply(req, 200, json!({ "agents": core.peers() }));
    }
    if *req.method() != Method::Post || req.url() != "/ask" {
        return reply(req, 404, json!({ "error": "rota desconhecida" }));
    }

    let mut body = Vec::new();
    if req.as_reader().read_to_end(&mut body).is_err() {
        return reply(req, 400, json!({ "error": "JSON inválido" }));
    }
    let Ok(msg) = serde_json::from_slice::<Value>(&body) else {
        return reply(req, 400, json!({ "error": "JSON inválido" }));
    };
    let (status, out) = ask(&core, &msg);
    reply(req, status, out);
}

/// `String(x || padrão)` do JS.
fn js_string_or(v: Option<&Value>, default: &str) -> String {
    match v {
        None | Some(Value::Null) | Some(Value::Bool(false)) => default.to_owned(),
        Some(Value::String(s)) if s.is_empty() => default.to_owned(),
        Some(Value::Number(n)) if n.as_f64() == Some(0.0) => default.to_owned(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

/// `Number(x || 0)` do JS (valores não numéricos viram 0).
fn js_depth(v: Option<&Value>) -> i64 {
    match v {
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse::<f64>().map(|f| f as i64).unwrap_or(0),
        Some(Value::Bool(true)) => 1,
        _ => 0,
    }
}

/// Resolve o destino pedido: apelidos, `ollama` → primeiro modelo, e nomes parciais de
/// modelos Ollama (`qwen3.5` → `ollama:qwen3.5:9b`).
pub(crate) fn resolve_target(asked: &str, available: &[String], ollama_models: &[String]) -> String {
    let mut to = resolve_alias(asked).to_owned();
    if available.contains(&to) || ollama_models.is_empty() {
        return to;
    }
    if to == "ollama" {
        let first = format!("ollama:{}", ollama_models[0]);
        if available.contains(&first) {
            to = first;
        }
    } else {
        let found = ollama_models.iter().find(|m| {
            let ml = m.to_lowercase();
            let base = ml.strip_suffix(":latest").unwrap_or(&ml);
            ml == asked || ml.contains(asked) || base == asked || asked.contains(base)
        });
        if let Some(m) = found {
            let sub = format!("ollama:{m}");
            if available.contains(&sub) {
                to = sub;
            } else if available.iter().any(|a| a == "ollama") {
                to = "ollama".into();
            }
        }
    }
    to
}

/// Processa um `POST /ask` já decodificado. Retorna (status HTTP, corpo).
fn ask(core: &Core, msg: &Value) -> (u16, Value) {
    let from = js_string_or(msg.get("from"), "usuário");
    let asked = js_string_or(msg.get("to"), "").to_lowercase();
    let depth = js_depth(msg.get("depth"));
    let message = match msg.get("message") {
        Some(Value::String(s)) => s.clone(),
        None | Some(Value::Null) => String::new(),
        Some(other) => other.to_string(),
    };

    let available = core.peers();
    let models = core.inner.agents.resolve_ollama(None).map(|o| o.models).unwrap_or_default();
    let to = resolve_target(&asked, &available, &models);
    if !available.contains(&to) {
        return (
            400,
            json!({ "error": format!("agente desconhecido ou não instalado \"{asked}\". Disponíveis: {}", available.join(", ")) }),
        );
    }
    if to == from {
        return (400, json!({ "error": "um agente não pode perguntar a si mesmo" }));
    }
    if depth >= MAX_DEPTH {
        return (400, json!({ "error": "profundidade máxima de conversa entre agentes atingida" }));
    }

    // A aba do agente chamado abre antes de a pergunta chegar ao feed (mesmo canal, ordem preservada).
    // A resposta continua saindo pelo modo headless: a aba é só o terminal normal do agente.
    if core.config().auto_open_agent_tabs {
        core.emit(CoreEvent::EnsureAgentTab { agent: to.clone() });
    }
    core.feed(FeedEntry::new(FeedKind::Ask, Some(&from), Some(&to), message.clone()));
    let prompt = format!(
        "[Mensagem do agente \"{from}\" recebida pelo CommandDeck]\n\n{message}\n\nResponda diretamente ao agente \"{from}\". O usuário acompanha a conversa integrada em tempo real."
    );
    let session_key = format!("{from}->{to}");
    let result = {
        let _turn = core.inner.bridge_queues.enter(&session_key);
        core.run_headless(&to, &prompt, HeadlessOptions { session_key: Some(session_key.clone()), depth: depth + 1, ..Default::default() })
    };
    match result {
        Ok(answer) => {
            core.feed(FeedEntry::new(FeedKind::Answer, Some(&to), Some(&from), answer.clone()));
            (200, json!({ "answer": answer }))
        }
        Err(e) => {
            let text = e.to_string();
            core.feed(FeedEntry::new(FeedKind::Error, Some(&to), Some(&from), text.clone()));
            (500, json!({ "error": text }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn target_resolution() {
        let models = vec!["qwen3.5:9b".to_owned(), "llama3:latest".to_owned()];
        let avail: Vec<String> =
            ["claude", "antigravity", "ollama", "ollama:qwen3.5:9b", "ollama:llama3:latest"].iter().map(|s| s.to_string()).collect();
        assert_eq!(resolve_target("agy", &avail, &models), "antigravity");
        assert_eq!(resolve_target("claude", &avail, &models), "claude");
        assert_eq!(resolve_target("qwen3.5", &avail, &models), "ollama:qwen3.5:9b");
        assert_eq!(resolve_target("llama3", &avail, &models), "ollama:llama3:latest");
        assert_eq!(resolve_target("xyz", &avail, &models), "xyz");
        // "ollama" já disponível fica "ollama"; senão vira o primeiro modelo.
        assert_eq!(resolve_target("ollama", &avail, &models), "ollama");
        let avail2: Vec<String> = vec!["ollama:qwen3.5:9b".into()];
        assert_eq!(resolve_target("local", &avail2, &models), "ollama:qwen3.5:9b");
    }

    #[test]
    fn js_coercions() {
        assert_eq!(js_string_or(None, "usuário"), "usuário");
        assert_eq!(js_string_or(Some(&json!("")), "usuário"), "usuário");
        assert_eq!(js_string_or(Some(&json!("claude")), "x"), "claude");
        assert_eq!(js_string_or(Some(&json!(5)), "x"), "5");
        assert_eq!(js_depth(Some(&json!(1))), 1);
        assert_eq!(js_depth(Some(&json!("2"))), 2);
        assert_eq!(js_depth(Some(&json!("abc"))), 0);
        assert_eq!(js_depth(None), 0);
    }

    #[test]
    fn pair_queue_is_fifo_and_exclusive() {
        let queues = Arc::new(PairQueues::default());
        let active = Arc::new(AtomicUsize::new(0));
        let order = Arc::new(Mutex::new(Vec::new()));
        // Segura a vez para que as próximas fiquem esperando em ordem.
        let first = queues.enter("a->b");
        let mut handles = Vec::new();
        for i in 0..5 {
            let (q, act, ord) = (queues.clone(), active.clone(), order.clone());
            handles.push(std::thread::spawn(move || {
                let _t = q.enter("a->b");
                assert_eq!(act.fetch_add(1, Ordering::SeqCst), 0, "duas execuções simultâneas no mesmo par");
                ord.lock().push(i);
                std::thread::sleep(Duration::from_millis(20));
                act.fetch_sub(1, Ordering::SeqCst);
            }));
            // Garante a ordem de chegada.
            std::thread::sleep(Duration::from_millis(30));
        }
        // Outro par não espera.
        drop(queues.enter("c->d"));
        drop(first);
        for h in handles {
            h.join().expect("thread");
        }
        assert_eq!(*order.lock(), vec![0, 1, 2, 3, 4]);
    }
}
