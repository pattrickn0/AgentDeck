//! Ponte HTTP local agente → agente (equivalente a `startBridge`).
//!
//! Escuta só em 127.0.0.1 (porta efêmera), exige o token `x-commanddeck-token` e valida o
//! cabeçalho `Host` contra DNS rebinding. Cada processo de agente recebe um token próprio, e é o
//! token que diz quem está perguntando (o `from` do corpo só vale com o token mestre, usado em
//! testes). Requisições para o mesmo par `de->para` são atendidas em ordem de chegada (fila FIFO
//! por par), uma de cada vez.

use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};
use serde_json::{json, Value};
use tiny_http::{Header, Method, Request, Response, Server};

use crate::agents::resolve_alias;
use crate::events::{CoreEvent, Delivery, FeedEntry, FeedKind};
use crate::exchange::Reply;
use crate::headless::HeadlessOptions;
use crate::instances::{scoped, unscoped, Ident};
use crate::usage::AgentUsage;
use crate::util::terminal_text;
use crate::{Auth, Caller};
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

/// Vez na fila; ao ser solta, chama o próximo (e esquece a fila do par se ninguém mais espera).
pub(crate) struct Turn<'a> {
    owner: &'a PairQueues,
    key: String,
    queue: Arc<PairQueue>,
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        // Mesma ordem de travas de `enter` (mapa, depois a fila): ninguém pega senha no meio.
        let mut queues = self.owner.queues.lock();
        let mut st = self.queue.state.lock();
        st.serving += 1;
        if st.serving == st.next && queues.get(&self.key).is_some_and(|q| Arc::ptr_eq(q, &self.queue)) {
            queues.remove(&self.key);
        }
        drop(st);
        drop(queues);
        self.queue.cv.notify_all();
    }
}

/// Filas por chave de sessão (equivalente a `bridgeQueues`). Só guarda os pares com alguém na
/// fila: o mapa não cresce com cada par que já conversou.
#[derive(Default)]
pub(crate) struct PairQueues {
    queues: Mutex<HashMap<String, Arc<PairQueue>>>,
}

impl PairQueues {
    /// Pega uma senha (na ordem de chamada) e bloqueia até ser a vez dela.
    pub fn enter(&self, key: &str) -> Turn<'_> {
        let mut queues = self.queues.lock();
        let queue = queues.entry(key.to_owned()).or_default().clone();
        let mut st = queue.state.lock();
        drop(queues);
        let ticket = st.next;
        st.next += 1;
        while st.serving != ticket {
            queue.cv.wait(&mut st);
        }
        drop(st);
        Turn { owner: self, key: key.to_owned(), queue }
    }

    /// Pares com alguém na fila agora.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.queues.lock().len()
    }
}

// ---------- Servidor ----------

/// Tamanho máximo do corpo de uma requisição (mensagens longas vão pelo stdin do `ask-agent`).
const MAX_BODY: u64 = 16 << 20;
/// Requisições em atendimento ao mesmo tempo (cada pergunta esperando resposta ocupa uma, também
/// depois do 202, enquanto a resposta atrasada não chega).
const MAX_ACTIVE: usize = 128;
/// Soma dos corpos sendo lidos ou atendidos ao mesmo tempo: sem ela, 128 corpos de 16 MB seriam 2 GB.
const MAX_BODIES: u64 = 64 << 20;

/// Conta uma requisição em atendimento enquanto existir.
pub(crate) struct Active(Arc<AtomicUsize>);

impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Bytes reservados do orçamento de corpos ([`MAX_BODIES`]) enquanto existir.
struct BodyBudget {
    used: Arc<AtomicU64>,
    bytes: u64,
}

impl BodyBudget {
    /// Reserva `bytes`, se couberem no orçamento.
    fn reserve(used: &Arc<AtomicU64>, bytes: u64) -> Option<Self> {
        if used.fetch_add(bytes, Ordering::SeqCst) + bytes > MAX_BODIES {
            used.fetch_sub(bytes, Ordering::SeqCst);
            return None;
        }
        Some(Self { used: used.clone(), bytes })
    }
}

impl Drop for BodyBudget {
    fn drop(&mut self) {
        self.used.fetch_sub(self.bytes, Ordering::SeqCst);
    }
}

/// Sobe o servidor em 127.0.0.1:0 e devolve (servidor, porta).
pub(crate) fn start(core: &Core) -> anyhow::Result<(Arc<Server>, u16)> {
    let server = Server::http("127.0.0.1:0").map_err(|e| anyhow::anyhow!("falha ao iniciar a ponte HTTP: {e}"))?;
    let port = server.server_addr().to_ip().map(|a| a.port()).ok_or_else(|| anyhow::anyhow!("ponte sem porta TCP"))?;
    let server = Arc::new(server);
    let srv = server.clone();
    let weak = Arc::downgrade(&core.inner);
    std::thread::Builder::new().name("cd-bridge".into()).spawn(move || {
        let active = Arc::new(AtomicUsize::new(0));
        let bodies = Arc::new(AtomicU64::new(0));
        // Termina quando `Server::unblock` é chamado (shutdown).
        for req in srv.incoming_requests() {
            if active.fetch_add(1, Ordering::SeqCst) >= MAX_ACTIVE {
                active.fetch_sub(1, Ordering::SeqCst);
                reply(req, 503, json!({ "error": "ponte ocupada: perguntas demais em andamento" }));
                continue;
            }
            let slot = Active(active.clone());
            let (weak, bodies) = (weak.clone(), bodies.clone());
            // Se a thread não subir, o fechamento é descartado e `slot` libera a vaga.
            let _ = std::thread::Builder::new().name("cd-bridge-req".into()).spawn(move || handle(weak, req, port, slot, &bodies));
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

/// Atende uma requisição. `port` é a porta da ponte (conhecida antes da primeira requisição: o
/// `Host` é validado desde o início) e `slot` a vaga dela entre as [`MAX_ACTIVE`].
fn handle(weak: Weak<CoreInner>, mut req: Request, port: u16, slot: Active, bodies: &Arc<AtomicU64>) {
    let Some(inner) = weak.upgrade() else {
        return reply(req, 503, json!({ "error": "CommandDeck encerrando" }));
    };
    let core = Core { inner };
    let Some(auth) = core.authenticate(header(&req, "x-commanddeck-token").unwrap_or("")) else {
        return reply(req, 403, json!({ "error": "token inválido" }));
    };
    // Validação de Host contra DNS rebinding (sem Host também é recusado).
    let host = header(&req, "host").unwrap_or("");
    if host != format!("127.0.0.1:{port}") && host != format!("localhost:{port}") {
        return reply(req, 403, json!({ "error": "host inválido" }));
    }

    if *req.method() == Method::Get && req.url() == "/agents" {
        let project = match &auth {
            Auth::Agent(c) => c.project.clone(),
            Auth::Master => core.active_project_id(),
        };
        return reply(req, 200, json!({ "agents": core.peers(&project) }));
    }
    if *req.method() == Method::Get && req.url() == "/team" {
        let Auth::Agent(c) = &auth else {
            return reply(req, 400, json!({ "error": "só um agente tem chefe e equipe" }));
        };
        // Quem pergunta já está ciente: o aviso pendente não precisa ir na próxima mensagem.
        core.inner.team_changed.lock().remove(&c.key(&c.id));
        let team = core.team(&c.project, &c.id);
        return reply(req, 200, json!({ "id": c.id, "boss": team.boss, "crew": team.crew }));
    }
    let route = req.url().to_owned();
    if *req.method() != Method::Post || !["/ask", "/reply", "/signal", "/usage"].contains(&route.as_str()) {
        return reply(req, 404, json!({ "error": "rota desconhecida" }));
    }

    // Reserva o tamanho anunciado (sem `Content-Length`, o máximo) antes de ler.
    let announced = req.body_length().map_or(MAX_BODY + 1, |n| (n as u64).min(MAX_BODY + 1));
    let Some(_budget) = BodyBudget::reserve(bodies, announced) else {
        return reply(req, 503, json!({ "error": "ponte ocupada: mensagens grandes demais em andamento" }));
    };
    let mut body = Vec::new();
    if req.as_reader().take(MAX_BODY + 1).read_to_end(&mut body).is_err() {
        return reply(req, 400, json!({ "error": "JSON inválido" }));
    }
    if body.len() as u64 > MAX_BODY {
        return reply(req, 413, json!({ "error": "mensagem grande demais (máximo de 16 MB)" }));
    }
    let Ok(msg) = serde_json::from_slice::<Value>(&body) else {
        return reply(req, 400, json!({ "error": "JSON inválido" }));
    };
    drop(body);
    let default_from = if route == "/ask" { "usuário" } else { "" };
    let caller = match caller(&core, &auth, &msg, default_from) {
        Ok(c) => c,
        Err(e) => return reply(req, 400, json!({ "error": e })),
    };
    let (status, out) = match route.as_str() {
        "/reply" => reply_pending(&core, &caller, &msg),
        "/signal" => signal(&core, &auth, &msg),
        "/usage" => usage(&core, &auth, &msg),
        _ => ask(&core, &caller, &msg, slot),
    };
    reply(req, status, out);
}

/// Quem está chamando. Com o token de um agente, é ele, no projeto dele (o `from` e o `project`
/// do corpo são ignorados); com o token mestre, o `from` do corpo, desde que seja um id válido,
/// no projeto `project` do corpo ou, sem ele, no projeto ativo.
fn caller(core: &Core, auth: &Auth, msg: &Value, default_from: &str) -> Result<Caller, String> {
    match auth {
        Auth::Agent(c) => Ok(c.clone()),
        Auth::Master => {
            let from = js_string_or(msg.get("from"), default_from).to_lowercase();
            if from != "usuário" && Ident::parse(&from).is_none() {
                return Err(format!("remetente inválido: \"{}\"", from.escape_debug()));
            }
            let project = match msg.get("project").and_then(Value::as_str).filter(|p| !p.is_empty()) {
                Some(p) if core.project_path(p).is_some() => p.to_owned(),
                Some(p) => return Err(format!("projeto desconhecido: \"{}\"", p.escape_debug())),
                None => core.active_project_id(),
            };
            if project.is_empty() {
                return Err("nenhum projeto aberto".into());
            }
            Ok(Caller { id: from, project, tab: false })
        }
    }
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
    // Negativa furaria o limite de profundidade.
    .max(0)
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

/// O destino `asked` (`agy`, `qwen3.5`, `claude@dev`…) já resolvido num id de instância: o modelo
/// pelo apelido e a instância (sem criar nenhuma) no projeto. `None` se não há uma só que sirva.
fn existing_instance(core: &Core, project: &str, asked: &str) -> Option<String> {
    let (base, suffix) = asked.split_at(asked.find('@').unwrap_or(asked.len()));
    let models = core.inner.agents.resolve_ollama(None).map(|o| o.models).unwrap_or_default();
    let kind = resolve_target(base, &core.inner.agents.peers(), &models);
    let want = Ident::parse(&format!("{kind}{suffix}"))?;
    core.inner.instances.lock().get(project)?.find(&want).ok().flatten().map(|i| i.id())
}

/// Chave (com escopo) do destino da pergunta `request` agora: segue uma renomeação da aba.
fn target_now(core: &Core, request: u64, fallback: &str) -> String {
    core.inner.exchange.parties(request).map_or_else(|| fallback.to_owned(), |(_, target)| target)
}

/// O prazo da pergunta `request` acabou sem resposta: tira a pergunta da lista. Se a resposta chegou
/// bem nessa hora (entre o fim da espera e aqui), ela vale: quem respondeu já ouviu "entregue".
fn give_up(core: &Core, request: u64, rx: &crossbeam_channel::Receiver<Reply>, why: String) -> Reply {
    if core.inner.exchange.remove(request) {
        Reply::Failed(why)
    } else {
        rx.try_recv().unwrap_or(Reply::Failed(why))
    }
}

/// Quanto tempo uma pergunta entregue numa aba espera pela resposta.
/// O prazo conta a partir da última atividade da aba (ver [`reply_deadline`]).
const TAB_REPLY_TIMEOUT: Duration = Duration::from_secs(600);
/// Teto absoluto, mesmo que a aba continue ativa: evita esperar para sempre um agente travado.
const TAB_REPLY_MAX: Duration = Duration::from_secs(3600);

/// Até quando esperar a resposta de `target` (chave com escopo) a uma pergunta entregue em `sent`.
///
/// São [`TAB_REPLY_TIMEOUT`] depois da pergunta *ou* depois da última atividade da aba (saída no
/// terminal ou turno em andamento), o que for mais tarde: um agente ocupado numa tarefa longa não
/// estoura o prazo, que só começa a correr quando ele sossega. Limitado a [`TAB_REPLY_MAX`].
fn reply_deadline(core: &Core, target: &str, sent: Instant) -> Instant {
    deadline_after(sent, core.inner.activity.last(target))
}

impl Caller {
    /// Chave com o escopo do projeto de quem chama para o id `id` (o dele, ou o de outra instância).
    fn key(&self, id: &str) -> String {
        scoped(&self.project, id)
    }

    /// Entrada da Conversa integrada no projeto de quem chama.
    fn feed(&self, core: &Core, kind: FeedKind, from: &str, to: &str, text: String) {
        core.feed(FeedEntry::new(kind, Some(from), Some(to), text).in_project(&self.project));
    }
}

fn deadline_after(sent: Instant, last_activity: Option<Instant>) -> Instant {
    let quiet_since = last_activity.map_or(sent, |a| a.max(sent));
    (quiet_since + TAB_REPLY_TIMEOUT).min(sent + TAB_REPLY_MAX)
}

/// Mensagem que acompanha uma resposta entregue a quem perguntou.
fn delivered(to: &str) -> Value {
    json!({ "answer": format!("Resposta entregue a \"{to}\".") })
}

/// `POST /reply` `{"to","message"[,"reply_to"]}`: o chamador responde à pergunta que `to` fez (ou
/// à pergunta número `reply_to`). Só conclui uma pergunta **pendente** (404 se não houver): nunca
/// cria uma pergunta nova. É o que o `ollama-agent` usa para devolver sozinho a resposta, sem
/// depender de o modelo rodar `ask-agent`.
fn reply_pending(core: &Core, caller: &Caller, msg: &Value) -> (u16, Value) {
    let from = caller.id.as_str();
    let message = js_string_or(msg.get("message"), "");
    let to = match msg.get("reply_to").and_then(Value::as_u64) {
        Some(id) => core.inner.exchange.reply_id(id, &caller.key(from), &message).map(|asker| unscoped(&asker).1.to_owned()),
        None => {
            let to = js_string_or(msg.get("to"), "").to_lowercase();
            core.inner.exchange.reply(&caller.key(from), &caller.key(&to), &message).then_some(to)
        }
    };
    let Some(to) = to else {
        return (404, json!({ "delivered": false, "error": "nenhuma pergunta pendente" }));
    };
    caller.feed(core, FeedKind::Answer, from, &to, message);
    (200, json!({ "delivered": true }))
}

/// `POST /signal` `{"state":"busy"|"idle"}`: hook do agente avisando que começou ou terminou um
/// turno (`ask-agent --signal`). Só o processo de uma aba sinaliza.
fn signal(core: &Core, auth: &Auth, msg: &Value) -> (u16, Value) {
    let Auth::Agent(Caller { id, project, tab: true }) = auth else {
        return (400, json!({ "error": "só o processo de uma aba envia sinais" }));
    };
    let busy = match msg.get("state").and_then(Value::as_str) {
        Some("busy") => true,
        Some("idle") => false,
        _ => return (400, json!({ "error": "estado inválido (use busy ou idle)" })),
    };
    core.emit(CoreEvent::AgentSignal { project: project.clone(), id: id.clone(), busy });
    (200, json!({ "ok": true }))
}

/// `POST /usage` `{"session":{"used","resets_at_ms"}|null,"weekly":…}`: uso do plano do agente
/// (`ask-agent --usage`, a statusline do Claude). O modelo vem do token; só o processo de uma aba envia.
fn usage(core: &Core, auth: &Auth, msg: &Value) -> (u16, Value) {
    let Auth::Agent(Caller { id, tab: true, .. }) = auth else {
        return (400, json!({ "error": "só o processo de uma aba envia o uso" }));
    };
    let Some(kind) = Ident::parse(id).map(|i| i.kind).filter(|k| k != "shell" && !k.starts_with("ollama")) else {
        return (400, json!({ "error": "esta aba não tem limite de uso" }));
    };
    match crate::usage::usage_from_json(msg) {
        Ok(usage) => {
            core.set_usage(&kind, AgentUsage { updated_ms: crate::util::now_ms(), ..usage });
            (200, json!({ "ok": true }))
        }
        Err(e) => (400, json!({ "error": e })),
    }
}

/// Texto digitado no terminal da instância que recebe a pergunta número `request`.
fn tab_prompt(from: &str, request: u64, message: &str) -> String {
    let message = terminal_text(message);
    format!(
        "[Mensagem do agente \"{from}\" recebida pelo CommandDeck]\n\n{message}\n\n\
Responda executando no shell: ask-agent --reply {request} \"sua resposta\"\n\
(a resposta só chega a \"{from}\" por esse comando; texto escrito apenas aqui não é entregue. Para respostas longas, \
envie pela entrada padrão: ask-agent --reply {request} -). O usuário acompanha a conversa na Conversa integrada."
    )
}

/// Espera a resposta até `until()` (reavaliado a cada volta, pois o prazo pode se estender).
/// `None` = o prazo acabou sem resposta.
fn wait_reply(core: &Core, rx: &crossbeam_channel::Receiver<Reply>, until: impl Fn() -> Instant) -> Option<Reply> {
    loop {
        let left = until().saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        match rx.recv_timeout(left.min(Duration::from_millis(500))) {
            Ok(reply) => return Some(reply),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                if core.inner.state.shutdown.load(Ordering::SeqCst) {
                    return Some(Reply::Failed("CommandDeck encerrando".into()));
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return Some(Reply::Failed("pergunta cancelada".into())),
        }
    }
}

/// Resposta que chegou depois de o `ask-agent` de quem perguntou já ter terminado: é digitada
/// no terminal de quem perguntou, como uma mensagem nova. `target` é o id sem o projeto.
fn deliver_late(core: &Core, project: &str, asker: Ident, target: &str, request: u64, outcome: Reply) {
    let prompt = match outcome {
        Reply::Answer(text) => format!(
            "[Resposta do agente \"{target}\" à sua pergunta #{request}, recebida pelo CommandDeck]\n\n{}\n\n\
Continue a tarefa com base nesta resposta. O usuário acompanha a conversa na Conversa integrada.",
            terminal_text(&text)
        ),
        Reply::Failed(text) => {
            core.feed(FeedEntry::new(FeedKind::Error, Some(target), Some(&asker.id()), text.clone()).in_project(project));
            format!("[CommandDeck] A pergunta #{request} para \"{target}\" ficou sem resposta: {}", terminal_text(&text))
        }
    };
    core.emit(CoreEvent::Deliver(Delivery {
        request,
        project: project.to_owned(),
        ident: asker,
        create: false,
        from: target.to_owned(),
        prompt,
        answer: true,
    }));
}

/// Pergunta entregue numa aba (instância) interativa e respondida por `ask-agent --reply`.
/// O destino é resolvido (e criado, se preciso) no projeto de quem pergunta.
///
/// Se quem pergunta é uma aba e pediu `wait` segundos, o `ask-agent` dela volta depois desse
/// tempo (antes de o Bash do agente estourar o próprio timeout) e a resposta, quando chegar, é
/// digitada no terminal dela.
fn ask_tab(core: &Core, caller: &Caller, asked: &str, message: &str, opts: TabAsk, slot: Active) -> (u16, Value) {
    let from = caller.id.as_str();
    let project = caller.project.as_str();
    let (base, suffix) = match asked.find('@') {
        Some(i) => (&asked[..i], &asked[i..]),
        None => (asked, ""),
    };
    let kinds = core.inner.agents.peers();
    let models = core.inner.agents.resolve_ollama(None).map(|o| o.models).unwrap_or_default();
    let kind = resolve_target(base, &kinds, &models);
    let Some(want) = kinds.contains(&kind).then(|| Ident::parse(&format!("{kind}{suffix}"))).flatten() else {
        return (
            400,
            json!({ "error": format!("agente desconhecido ou não instalado \"{asked}\". Disponíveis: {}", core.peers(project).join(", ")) }),
        );
    };
    let resolved = core.with_instances(project, |list| list.resolve_or_create(&want, opts.force_new));
    let (ident, created) = match resolved {
        Ok(v) => v,
        Err(options) => {
            return (400, json!({ "error": format!("\"{asked}\" é ambíguo: {}. Use o nome completo.", options.join(", ")) }));
        }
    };
    let id = ident.id();
    let forget = |status: u16, error: Value| {
        if created {
            core.with_instances(project, |list| list.remove(&id));
        }
        (status, error)
    };
    if id == from {
        return forget(400, json!({ "error": "um agente não pode perguntar a si mesmo" }));
    }
    // Com abas a profundidade não viaja no ambiente (a pergunta é digitada): mede-se pela cadeia
    // de perguntas pendentes de quem pergunta. Um ciclo de espera é recusado primeiro (409).
    let (from_key, target_key) = (caller.key(from), caller.key(&id));
    let depth = opts.depth.max(core.inner.exchange.chain_depth(&from_key));
    if depth >= MAX_DEPTH && !core.inner.exchange.would_cycle(&from_key, &target_key) {
        return forget(400, json!({ "error": "profundidade máxima de conversa entre agentes atingida" }));
    }
    let (request, rx) = match core.inner.exchange.register_at(&from_key, &target_key, depth) {
        Ok(v) => v,
        // A mensagem de erro do Exchange cita as chaves com escopo: refeita com os ids.
        Err(_) => {
            return forget(409, json!({ "error": format!("\"{id}\" já está esperando uma resposta de \"{from}\" (ciclo de espera)") }))
        }
    };
    // O projeto foi removido no meio do caminho (a remoção falha as perguntas registradas antes
    // dela; esta é conferida aqui, depois de registrada).
    if core.project_path(project).is_none() {
        core.inner.exchange.remove(request);
        return forget(400, json!({ "error": "o projeto foi removido" }));
    }

    caller.feed(core, FeedKind::Ask, from, &id, message.to_owned());
    // Quem abriu a instância passa a ser o chefe dela (o usuário, pelo token mestre, não conta).
    if created && core.project_instance_ids(project).iter().any(|o| o == from) {
        if let Err(e) = core.set_boss(project, &id, Some(from)) {
            log::warn!("chefe de \"{id}\": {e}");
        }
    }
    // O aviso de equipe pendente vai quando a interface digitar (`Core::take_team_notice`): se a
    // entrega for descartada, ele não se perde.
    let prompt = tab_prompt(from, request, message);
    core.emit(CoreEvent::Deliver(Delivery {
        request,
        project: project.to_owned(),
        ident,
        create: created,
        from: from.to_owned(),
        prompt,
        answer: false,
    }));

    let sent = Instant::now();
    // Só um agente recebe uma resposta atrasada digitada: num terminal comum (`shell`) o texto
    // seria executado como comando. Ali o `ask-agent` espera até o fim.
    let asker = Ident::parse(from).filter(|i| caller.tab && i.kind != "shell");
    let detach_at = match (opts.wait, &asker) {
        (Some(w), Some(_)) if w > 0 => Some(Instant::now() + Duration::from_secs(w)),
        _ => None,
    };
    // O destino é lido pelo número a cada volta: renomear a aba no meio da pergunta não perde a
    // atividade dela (o prazo conta a partir dela).
    let deadline = || reply_deadline(core, &target_now(core, request, &target_key), sent);
    let waited = wait_reply(core, &rx, || detach_at.map_or_else(deadline, |d| d.min(deadline())));
    let (asker_key, target_key_now) = core.inner.exchange.parties(request).unwrap_or_else(|| (from_key, target_key.clone()));
    let (asker_id, target_id) = (unscoped(&asker_key).1.to_owned(), unscoped(&target_key_now).1.to_owned());
    let outcome = match waited {
        Some(reply) => reply,
        None => match (detach_at, asker) {
            (Some(_), Some(asker)) if Instant::now() < deadline() => {
                core.inner.exchange.detach(request);
                // A resposta chegou no instante do 202: vai agora mesmo.
                match rx.try_recv() {
                    Ok(reply) => reply,
                    Err(_) => return detach(core, project, request, rx, sent, asker, target_key_now, slot),
                }
            }
            _ => give_up(core, request, &rx, format!("\"{target_id}\" não respondeu em 10 minutos")),
        },
    };
    core.inner.exchange.remove(request);
    match outcome {
        Reply::Answer(answer) => (200, json!({ "answer": answer })),
        Reply::Failed(text) => {
            caller.feed(core, FeedKind::Error, &target_id, &asker_id, text.clone());
            (500, json!({ "error": text }))
        }
    }
}

/// O `ask-agent` de quem perguntou volta agora (202) e a resposta, quando chegar, é digitada no
/// terminal dele por uma thread que fica com a vaga (`slot`) da requisição. `target_key` é o
/// destino com escopo; quem perguntou e o destino são relidos pelo número no fim (renomeações).
#[allow(clippy::too_many_arguments)]
fn detach(
    core: &Core,
    project: &str,
    request: u64,
    rx: crossbeam_channel::Receiver<Reply>,
    sent: Instant,
    asker: Ident,
    target_key: String,
    slot: Active,
) -> (u16, Value) {
    let core2 = core.clone();
    let project = project.to_owned();
    let target = unscoped(&target_key).1.to_owned();
    let spawned = std::thread::Builder::new().name("cd-bridge-late".into()).spawn(move || {
        let _slot = slot;
        let core = core2;
        let waited = wait_reply(&core, &rx, || reply_deadline(&core, &target_now(&core, request, &target_key), sent));
        let parties = core.inner.exchange.parties(request);
        let (asker, target) = match &parties {
            Some((a, t)) => (Ident::parse(unscoped(a).1).unwrap_or(asker), unscoped(t).1.to_owned()),
            None => (asker, unscoped(&target_key).1.to_owned()),
        };
        let outcome = waited.unwrap_or_else(|| give_up(&core, request, &rx, format!("\"{target}\" não respondeu em 10 minutos")));
        core.inner.exchange.remove(request);
        deliver_late(&core, &project, asker, &target, request, outcome);
    });
    if spawned.is_err() {
        core.inner.exchange.fail(request, "falha interna da ponte");
        core.inner.exchange.remove(request);
        return (500, json!({ "error": "falha interna da ponte" }));
    }
    (
        202,
        json!({
            "pending": request,
            "answer": format!(
                "A pergunta #{request} foi entregue a \"{target}\", que ainda está trabalhando nela. A resposta será digitada \
        neste terminal quando chegar, como uma nova mensagem. Não repita a pergunta: siga com outra parte da tarefa ou encerre o turno e aguarde."
            ),
        }),
    )
}

/// Opções de uma pergunta a uma aba.
struct TabAsk {
    force_new: bool,
    depth: i64,
    /// Segundos que o `ask-agent` de quem pergunta aceita esperar (`None`/0 = até o fim).
    wait: Option<u64>,
}

/// Processa um `POST /ask` já decodificado. Retorna (status HTTP, corpo). Tudo acontece no
/// projeto de quem chama: as instâncias, as perguntas pendentes e as sessões headless.
fn ask(core: &Core, caller: &Caller, msg: &Value, slot: Active) -> (u16, Value) {
    let from = caller.id.as_str();
    let project = caller.project.as_str();
    let asked = js_string_or(msg.get("to"), "").to_lowercase();
    let depth = js_depth(msg.get("depth"));
    let message = match msg.get("message") {
        Some(Value::String(s)) => s.clone(),
        None | Some(Value::Null) => String::new(),
        Some(other) => other.to_string(),
    };

    // Resposta explícita à pergunta número N (`ask-agent --reply N "texto"`).
    if let Some(id) = msg.get("reply_to").and_then(Value::as_u64) {
        return match core.inner.exchange.reply_id(id, &caller.key(from), &message) {
            Some(asker) => {
                let asker = unscoped(&asker).1;
                caller.feed(core, FeedKind::Answer, from, asker, message);
                (200, delivered(asker))
            }
            None => (
                404,
                json!({ "error": format!("a pergunta #{id} não está esperando resposta de \"{from}\" (já foi respondida, cancelada ou era para outro agente)") }),
            ),
        };
    }
    // `from` está respondendo a uma pergunta que `asked` fez e cujo `ask-agent` ainda espera:
    // entrega a ele. (Uma pergunta nova de `from` para quem o espera fecharia um ciclo de espera;
    // por isso é resposta.) O nome passa pelos apelidos (`agy`, `qwen3.5`) e pelas instâncias.
    let asker = existing_instance(core, project, &asked).unwrap_or_else(|| asked.clone());
    if core.inner.exchange.reply_waiting(&caller.key(from), &caller.key(&asker), &message) {
        caller.feed(core, FeedKind::Answer, from, &asker, message);
        return (200, delivered(&asker));
    }
    // Com a interface ligada, a pergunta vai para a aba (instância) do agente; sem ela, em headless.
    if core.tab_delivery() {
        let opts = TabAsk {
            force_new: msg.get("new").and_then(Value::as_bool).unwrap_or(false),
            depth,
            wait: msg.get("wait").and_then(Value::as_u64),
        };
        return ask_tab(core, caller, &asked, &message, opts, slot);
    }

    let available = core.peers(project);
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

    caller.feed(core, FeedKind::Ask, from, &to, message.clone());
    let prompt = format!(
        "[Mensagem do agente \"{from}\" recebida pelo CommandDeck]\n\n{message}\n\nResponda diretamente ao agente \"{from}\". O usuário acompanha a conversa integrada em tempo real."
    );
    let session_key = format!("{}->{}", caller.key(from), caller.key(&to));
    let _live = core.headless_ask_started(project, from, &to);
    let result = {
        let _turn = core.inner.bridge_queues.enter(&session_key);
        let kind = Ident::parse(&to).map(|i| i.kind).unwrap_or_else(|| to.clone());
        core.run_headless(
            &kind,
            &prompt,
            HeadlessOptions {
                session_key: Some(session_key.clone()),
                depth: depth + 1,
                project: project.to_owned(),
                cwd: core.project_path(project).unwrap_or_default(),
                ..Default::default()
            },
        )
    };
    match result {
        Ok(answer) => {
            caller.feed(core, FeedKind::Answer, &to, from, answer.clone());
            (200, json!({ "answer": answer }))
        }
        Err(e) => {
            let text = e.to_string();
            caller.feed(core, FeedKind::Error, &to, from, text.clone());
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
    fn the_tab_prompt_names_the_request() {
        let p = tab_prompt("claude@dev", 7, "faça\x1b[201~ x");
        assert!(p.starts_with("[Mensagem do agente \"claude@dev\" recebida pelo CommandDeck]\n\nfaça[201~ x\n\n"), "{p}");
        assert!(p.contains("ask-agent --reply 7 \"sua resposta\""), "{p}");
        // O `ollama-agent` lê o número desta linha (`bridge_request`): mudou aqui, mude lá.
        assert!(p.contains("Responda executando no shell: ask-agent --reply 7 "), "{p}");
        assert!(!p.contains('\x1b'));
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
        assert_eq!(queues.len(), 0, "sem ninguém na fila, os pares são esquecidos");
    }

    #[test]
    fn a_pair_queue_is_kept_while_someone_waits() {
        let queues = Arc::new(PairQueues::default());
        let first = queues.enter("a->b");
        let waiting = std::thread::spawn({
            let q = queues.clone();
            move || drop(q.enter("a->b"))
        });
        while queues.queues.lock().get("a->b").is_none_or(|q| q.state.lock().next < 2) {
            std::thread::yield_now();
        }
        drop(first);
        assert!(queues.len() <= 1);
        waiting.join().expect("thread");
        assert_eq!(queues.len(), 0);
        // E volta a funcionar depois de esquecido.
        drop(queues.enter("a->b"));
        assert_eq!(queues.len(), 0);
    }

    #[test]
    fn the_body_budget_caps_the_bytes_in_flight() {
        let used = Arc::new(AtomicU64::new(0));
        let big: Vec<BodyBudget> = (0..4).map(|_| BodyBudget::reserve(&used, MAX_BODY).expect("cabe")).collect();
        assert!(BodyBudget::reserve(&used, 1).is_none(), "orçamento cheio");
        assert_eq!(used.load(Ordering::SeqCst), MAX_BODIES, "a tentativa recusada não fica reservada");
        drop(big);
        assert_eq!(used.load(Ordering::SeqCst), 0);
        assert!(BodyBudget::reserve(&used, 1).is_some());
    }
}

#[cfg(test)]
mod deadline_tests {
    use super::*;

    #[test]
    fn prazo_conta_do_fim_da_atividade() {
        let t0 = Instant::now();
        let min = |n: u64| Duration::from_secs(n * 60);
        // sem atividade: 10 min depois da pergunta
        assert_eq!(deadline_after(t0, None), t0 + min(10));
        // atividade antes da pergunta não adia
        assert_eq!(deadline_after(t0 + min(5), Some(t0)), t0 + min(15));
        // aba ainda trabalhando 30 min depois: o prazo vai para 40 min
        assert_eq!(deadline_after(t0, Some(t0 + min(30))), t0 + min(40));
        // teto de 60 min mesmo com atividade contínua
        assert_eq!(deadline_after(t0, Some(t0 + min(55))), t0 + min(60));
    }
}
