//! Ponte HTTP local agente → agente (equivalente a `startBridge`).
//!
//! Escuta só em 127.0.0.1 (porta efêmera), exige o token `x-agentdeck-token` e valida o
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
use crate::inbox::InboxEntry;
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
/// Arquivos aceitos por chamada de `POST /learn` (`ask-agent --learn`) e de `POST /outline` (`ask-agent --outline`).
const MAX_LEARN_PATHS: usize = 50;
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
    reply_checked(req, status, body);
}

/// Escreve a resposta HTTP. `false` = a escrita falhou (o cliente já tinha ido embora).
fn reply_checked(req: Request, status: u16, body: Value) -> bool {
    let text = serde_json::to_string(&body).unwrap_or_else(|_| "{}".into());
    let mut resp = Response::from_string(text).with_status_code(status);
    if let Ok(h) = Header::from_bytes("content-type", "application/json") {
        resp = resp.with_header(h);
    }
    req.respond(resp).is_ok()
}

fn header<'a>(req: &'a Request, name: &'static str) -> Option<&'a str> {
    req.headers().iter().find(|h| h.field.equiv(name)).map(|h| h.value.as_str())
}

/// Cabeçalho do token (`x-agentdeck-token`); vazio se faltar.
fn token_header(req: &Request) -> &str {
    header(req, "x-agentdeck-token").filter(|v| !v.is_empty()).unwrap_or("")
}

/// Atende uma requisição. `port` é a porta da ponte (conhecida antes da primeira requisição: o
/// `Host` é validado desde o início) e `slot` a vaga dela entre as [`MAX_ACTIVE`].
fn handle(weak: Weak<CoreInner>, mut req: Request, port: u16, slot: Active, bodies: &Arc<AtomicU64>) {
    let Some(inner) = weak.upgrade() else {
        return reply(req, 503, json!({ "error": "AgentDeck encerrando" }));
    };
    let core = Core { inner };
    let Some(auth) = core.authenticate(token_header(&req)) else {
        return reply(req, 403, json!({ "error": "token inválido" }));
    };
    // Validação de Host contra DNS rebinding (sem Host também é recusado).
    let host = header(&req, "host").unwrap_or("");
    if host != format!("127.0.0.1:{port}") && host != format!("localhost:{port}") {
        return reply(req, 403, json!({ "error": "host inválido" }));
    }

    // A segunda abertura do app pede que a janela volte (ver `activation`). Só o token mestre, que o app grava junto da
    // trava da pasta de dados: um agente não traz a janela do usuário para a frente.
    if *req.method() == Method::Post && req.url() == "/activate" {
        if !matches!(auth, Auth::Master) {
            return reply(req, 403, json!({ "error": "só o AgentDeck pode pedir isto" }));
        }
        core.emit(CoreEvent::Activate);
        return reply(req, 200, json!({ "ok": true }));
    }
    if *req.method() == Method::Get && req.url() == "/agents" {
        let project = match &auth {
            Auth::Agent(c) => c.project.clone(),
            Auth::Master => core.active_project_id(),
        };
        let models = core.instance_models(&project);
        return reply(req, 200, json!({ "agents": core.peers(&project), "quota": core.quota_notes(&models), "models": models }));
    }
    if *req.method() == Method::Get && (req.url() == "/models" || req.url().starts_with("/models?")) {
        let url = req.url().to_owned();
        let only = url.split_once('?').and_then(|(_, q)| q.split('&').find_map(|kv| kv.strip_prefix("agent="))).filter(|a| !a.is_empty());
        if only.is_some_and(|a| !crate::models::valid_model(a)) {
            return reply(req, 400, json!({ "error": "agente inválido" }));
        }
        return match core.models_inventory(only) {
            Ok(items) => reply(req, 200, json!({ "text": crate::models::format_inventory(&items, &core.quota_lines()) })),
            Err(e) => reply(req, 404, json!({ "error": e })),
        };
    }
    if *req.method() == Method::Get && req.url() == "/team" {
        let Auth::Agent(c) = &auth else {
            return reply(req, 400, json!({ "error": "só um agente tem chefe e equipe" }));
        };
        // Quem pergunta já está ciente: o aviso pendente não precisa ir na próxima mensagem.
        core.inner.team_changed.lock().remove(&c.key(&c.id));
        let team = core.team(&c.project, &c.id);
        // Os modelos de quem aparece na resposta (ele, o chefe e a equipe), para o `--team`.
        let models: std::collections::BTreeMap<String, String> = core
            .instance_models(&c.project)
            .into_iter()
            .filter(|(id, _)| *id == c.id || team.boss.as_ref() == Some(id) || team.crew.contains(id))
            .collect();
        return reply(
            req,
            200,
            json!({ "id": c.id, "boss": team.boss, "crew": team.crew, "quota": core.quota_notes(&models), "models": models }),
        );
    }
    let route = req.url().to_owned();
    if route == "/tasks" || route.starts_with("/tasks/") || route.starts_with("/tasks?") {
        return tasks_route(&core, req, &auth, bodies);
    }
    if (route == "/learn" || route == "/outline") && *req.method() == Method::Post {
        let (msg, _budget) = match read_json(&mut req, bodies, false) {
            Ok(v) => v,
            Err((status, body)) => return reply(req, status, body),
        };
        let caller = match caller(&core, &auth, &msg, "usuário") {
            Ok(c) => c,
            Err(e) => return reply(req, 400, json!({ "error": e })),
        };
        let paths: Vec<String> = msg
            .get("paths")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).take(MAX_LEARN_PATHS).map(str::to_owned).collect())
            .unwrap_or_default();
        let answer = if route == "/learn" {
            core.learn(&caller.project, &caller.id, &paths)
        } else {
            core.outline(&caller.project, &caller.id, &paths)
        };
        return match answer {
            Ok(text) => reply(req, 200, json!({ "text": text })),
            Err(e) => reply(req, 400, json!({ "error": e })),
        };
    }
    if *req.method() == Method::Get && (route == "/inbox" || route.starts_with("/inbox?")) {
        let Auth::Agent(c) = &auth else {
            return reply(req, 400, json!({ "error": "só um agente tem caixa de entrada" }));
        };
        let all = route.split_once('?').is_some_and(|(_, q)| q.split('&').any(|kv| kv == "all=1"));
        return reply(req, 200, json!({ "answers": core.inner.inbox.read(&c.key(&c.id), all) }));
    }
    if *req.method() == Method::Post && route == "/inbox/delivered" {
        let Auth::Agent(c) = &auth else {
            return reply(req, 400, json!({ "error": "só um agente tem caixa de entrada" }));
        };
        let (msg, _budget) = match read_json(&mut req, bodies, false) {
            Ok(v) => v,
            Err((status, body)) => return reply(req, status, body),
        };
        let Some(request) = msg.get("request").and_then(Value::as_u64) else {
            return reply(req, 400, json!({ "error": "falta o número da pergunta (request)" }));
        };
        // Só a caixa de quem perguntou: um agente não marca como recebida a resposta de outro.
        let mine = core.inner.inbox.find(request).is_some_and(|(asker, _)| asker == c.key(&c.id));
        if mine {
            core.inner.inbox.mark_delivered(request);
        }
        return reply(req, if mine { 200 } else { 404 }, json!({ "ok": mine }));
    }
    if *req.method() != Method::Post || !["/ask", "/reply", "/signal", "/usage"].contains(&route.as_str()) {
        return reply(req, 404, json!({ "error": "rota desconhecida" }));
    }

    let (msg, _budget) = match read_json(&mut req, bodies, false) {
        Ok(v) => v,
        Err((status, body)) => return reply(req, status, body),
    };
    let default_from = if route == "/ask" { "usuário" } else { "" };
    let caller = match caller(&core, &auth, &msg, default_from) {
        Ok(c) => c,
        Err(e) => return reply(req, 400, json!({ "error": e })),
    };
    let client_acks = msg.get("ack").and_then(Value::as_bool).unwrap_or(false);
    let (status, out) = match route.as_str() {
        "/reply" => reply_pending(&core, &caller, &msg),
        "/signal" => signal(&core, &auth, &msg),
        "/usage" => usage(&core, &auth, &msg),
        _ => ask(&core, &caller, &msg, slot),
    };
    // Uma resposta síncrona (200 ou erro de uma pergunta a uma aba) traz o número da pergunta: só vale
    // como entregue se o corpo foi escrito sem erro; senão ela fica na caixa de entrada. Escrever sem erro
    // não prova que chegou (o `tiny_http` ignora a conexão fechada, e o primeiro envio a quem já saiu dá
    // certo): o `ask-agent` que manda `ack` confirma ele mesmo, em `/inbox/delivered`, depois de imprimir.
    let answered = (route == "/ask" && status != 202).then(|| out.get("request").and_then(Value::as_u64)).flatten();
    if reply_checked(req, status, out) {
        if let Some(request) = answered.filter(|_| !client_acks) {
            core.inner.inbox.mark_delivered(request);
        }
    } else if let Some(request) = answered {
        log::warn!("resposta #{request} não escrita: a conexão de quem perguntou já tinha sido encerrada");
        core.answer_lost(&caller.project, request, "a conexão de quem perguntou já tinha sido encerrada");
    }
}

/// Lê o corpo JSON da requisição, reservando antes o tamanho anunciado no orçamento de corpos (o
/// `BodyBudget` devolvido precisa viver até o fim do atendimento). Com `allow_empty`, um corpo vazio
/// vale `{}`. Erro = (status, corpo da resposta).
fn read_json(req: &mut Request, bodies: &Arc<AtomicU64>, allow_empty: bool) -> Result<(Value, BodyBudget), (u16, Value)> {
    // Reserva o tamanho anunciado (sem `Content-Length`, o máximo) antes de ler.
    let announced = req.body_length().map_or(MAX_BODY + 1, |n| (n as u64).min(MAX_BODY + 1));
    let Some(budget) = BodyBudget::reserve(bodies, announced) else {
        return Err((503, json!({ "error": "ponte ocupada: mensagens grandes demais em andamento" })));
    };
    let mut body = Vec::new();
    if req.as_reader().take(MAX_BODY + 1).read_to_end(&mut body).is_err() {
        return Err((400, json!({ "error": "JSON inválido" })));
    }
    if body.len() as u64 > MAX_BODY {
        return Err((413, json!({ "error": "mensagem grande demais (máximo de 16 MB)" })));
    }
    if allow_empty && body.iter().all(u8::is_ascii_whitespace) {
        return Ok((json!({}), budget));
    }
    match serde_json::from_slice::<Value>(&body) {
        Ok(msg) => Ok((msg, budget)),
        Err(_) => Err((400, json!({ "error": "JSON inválido" }))),
    }
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

/// O destino `asked` (`agy`, `claude@dev`…) já resolvido num id de instância: o modelo
/// pelo apelido e a instância (sem criar nenhuma) no projeto. `None` se não há uma só que sirva.
fn existing_instance(core: &Core, project: &str, asked: &str) -> Option<String> {
    let (base, suffix) = asked.split_at(asked.find('@').unwrap_or(asked.len()));
    let kind = resolve_alias(base);
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
/// cria uma pergunta nova.
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
    let Some(kind) = Ident::parse(id).map(|i| i.kind).filter(|k| k != "shell") else {
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
        "[Mensagem do agente \"{from}\" recebida pelo AgentDeck]\n\n{message}\n\n\
Responda executando no shell: ask-agent --reply {request} \"sua resposta\"\n\
(a resposta só chega a \"{from}\" por esse comando; texto escrito apenas aqui não é entregue. Em respostas longas ou com acentos, \
escreva-as num arquivo e use: ask-agent --reply {request} --file <arquivo>). O usuário acompanha a conversa na Conversa integrada."
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
                    return Some(Reply::Failed("AgentDeck encerrando".into()));
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return Some(Reply::Failed("pergunta cancelada".into())),
        }
    }
}

/// Resposta que chegou depois de o `ask-agent` de quem perguntou já ter terminado: é digitada
/// no terminal de quem perguntou, como uma mensagem nova. `target` é o id sem o projeto.
fn deliver_late(core: &Core, project: &str, asker: Ident, target: &str, request: u64, outcome: Reply) {
    let (text, failed) = match &outcome {
        Reply::Answer(text) => (text, false),
        Reply::Failed(text) => (text, true),
    };
    // Fica na caixa de entrada até a interface digitá-la no terminal (`Core::answer_typed`).
    inbox_push(core, &scoped(project, &asker.id()), request, target, text, failed);
    let prompt = match outcome {
        Reply::Answer(text) => format!(
            "[Resposta do agente \"{target}\" à sua pergunta #{request}, recebida pelo AgentDeck]\n\n{}\n\n\
Continue a tarefa com base nesta resposta. O usuário acompanha a conversa na Conversa integrada.",
            terminal_text(&text)
        ),
        Reply::Failed(text) => {
            core.feed(FeedEntry::new(FeedKind::Error, Some(target), Some(&asker.id()), text.clone()).in_project(project));
            format!("[AgentDeck] A pergunta #{request} para \"{target}\" ficou sem resposta: {}", terminal_text(&text))
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
        boot: None,
    }));
}

/// Pergunta entregue numa aba (instância) interativa e respondida por `ask-agent --reply`.
/// O destino é resolvido (e criado, se preciso) no projeto de quem pergunta.
///
/// Se quem pergunta é uma aba e pediu `wait` segundos, o `ask-agent` dela volta depois desse
/// tempo (antes de o Bash do agente estourar o próprio timeout) e a resposta, quando chegar, é
/// digitada no terminal dela.
fn ask_tab(core: &Core, caller: &Caller, asked: &str, message: &str, opts: TabAsk<'_>, slot: Active) -> (u16, Value) {
    let from = caller.id.as_str();
    let project = caller.project.as_str();
    let (base, suffix) = match asked.find('@') {
        Some(i) => (&asked[..i], &asked[i..]),
        None => (asked, ""),
    };
    let kinds = core.inner.agents.peers();
    let kind = resolve_alias(base).to_owned();
    let Some(want) = kinds.contains(&kind).then(|| Ident::parse(&format!("{kind}{suffix}"))).flatten() else {
        return (
            400,
            json!({ "error": format!("agente desconhecido ou não instalado \"{asked}\". Disponíveis: {}", core.peers(project).join(", ")) }),
        );
    };
    if let Some(m) = opts.model {
        if !crate::models::accepts_model(&want.kind) {
            return (400, json!({ "error": format!("o agente {} não aceita escolher modelo", want.kind) }));
        }
        // A instância pedida já está aberta com outro modelo: recusa antes de criar ou registrar qualquer coisa.
        if !opts.force_new {
            let open = core.with_instances(project, |list| list.find(&want)).ok().flatten();
            if let Some(open) = open {
                if let Some(error) = crate::models::conflict(&open.id(), core.instance_model(project, &open).as_deref(), m) {
                    return (409, json!({ "error": error }));
                }
            }
        }
    }
    let resolved = core.with_instances(project, |list| list.resolve_or_create(&want, opts.force_new));
    let (ident, created) = match resolved {
        Ok(v) => v,
        Err(options) => {
            return (400, json!({ "error": format!("\"{asked}\" é ambíguo: {}. Use o nome completo.", options.join(", ")) }));
        }
    };
    let id = ident.id();
    // Instância nova: o modelo pedido (senão o padrão do cargo) fica gravado já, antes de a aba abrir.
    if created {
        core.assign_model(project, &ident, opts.model);
    }
    let forget = |status: u16, error: Value| {
        if created {
            core.with_instances(project, |list| list.remove(&id));
            core.forget_model(project, &id);
        }
        (status, error)
    };
    if id == from {
        return forget(400, json!({ "error": "um agente não pode perguntar a si mesmo" }));
    }
    // Um modelo sem cota não responde: melhor o agente saber agora e escolher outro.
    if let Some(error) = core.quota_refusal(&id, &ident.kind, core.instance_model(project, &ident).as_deref()) {
        return forget(409, json!({ "error": error }));
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
        boot: None,
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
        Reply::Answer(answer) => {
            inbox_push(core, &asker_key, request, &target_id, &answer, false);
            (200, json!({ "answer": answer, "request": request }))
        }
        Reply::Failed(text) => {
            inbox_push(core, &asker_key, request, &target_id, &text, true);
            caller.feed(core, FeedKind::Error, &target_id, &asker_id, text.clone());
            (500, json!({ "error": text, "request": request }))
        }
    }
}

/// Guarda na caixa de entrada de `asker_key` (chave com escopo) o desfecho da pergunta `request`, ainda
/// como não entregue. Quem o entrega marca depois (ver `Core::answer_typed` e o fim de [`handle`]).
fn inbox_push(core: &Core, asker_key: &str, request: u64, from: &str, text: &str, failed: bool) {
    let entry = InboxEntry { request, from: from.to_owned(), text: text.to_owned(), failed, ts: crate::util::now_ms(), delivered: false };
    core.inner.inbox.push(asker_key, entry);
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
        neste terminal quando chegar, como uma nova mensagem. Não repita a pergunta: siga com outra parte da tarefa ou encerre o turno e aguarde. \
        Se a resposta não aparecer, rode: ask-agent --inbox"
            ),
        }),
    )
}

/// Opções de uma pergunta a uma aba.
struct TabAsk<'a> {
    force_new: bool,
    /// Modelo pedido (`ask-agent --model`), já validado.
    model: Option<&'a str>,
    depth: i64,
    /// Segundos que o `ask-agent` de quem pergunta aceita esperar (`None`/0 = até o fim).
    wait: Option<u64>,
}

/// O campo `model` do `/ask`: ausente ou vazio = nenhum; texto fora do padrão = erro.
fn requested_model(v: Option<&Value>) -> Result<Option<String>, String> {
    let m = match v {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(s)) if s.trim().is_empty() => return Ok(None),
        Some(Value::String(s)) => s.trim(),
        Some(_) => return Err("modelo inválido: use um texto".into()),
    };
    if crate::models::valid_model(m) {
        Ok(Some(m.to_owned()))
    } else {
        Err(format!(
            "modelo inválido: \"{}\" (use letras, números e : . _ / # @ + -, até {} caracteres)",
            m.chars().take(40).collect::<String>().escape_debug(),
            crate::models::MAX_MODEL_LEN
        ))
    }
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
    let model = match requested_model(msg.get("model")) {
        Ok(m) => m,
        Err(e) => return (400, json!({ "error": e })),
    };
    // Com a interface ligada, a pergunta vai para a aba (instância) do agente; sem ela, em headless.
    if core.tab_delivery() {
        let opts = TabAsk {
            model: model.as_deref(),
            force_new: msg.get("new").and_then(Value::as_bool).unwrap_or(false),
            depth,
            wait: msg.get("wait").and_then(Value::as_u64),
        };
        return ask_tab(core, caller, &asked, &message, opts, slot);
    }

    let available = core.peers(project);
    let to = resolve_alias(&asked).to_owned();
    if !available.contains(&to) {
        return (
            400,
            json!({ "error": format!("agente desconhecido ou não instalado \"{asked}\". Disponíveis: {}", available.join(", ")) }),
        );
    }
    if to == from {
        return (400, json!({ "error": "um agente não pode perguntar a si mesmo" }));
    }
    let to_ident = Ident::parse(&to);
    let kind = to_ident.as_ref().map(|i| i.kind.clone()).unwrap_or_else(|| to.clone());
    if model.is_some() && !crate::models::accepts_model(&kind) {
        return (400, json!({ "error": format!("o agente {kind} não aceita escolher modelo") }));
    }
    if depth >= MAX_DEPTH {
        return (400, json!({ "error": "profundidade máxima de conversa entre agentes atingida" }));
    }
    // Sem abas não há instância viva: vale o pedido, senão o gravado ou o padrão do cargo.
    let model = model.or_else(|| to_ident.as_ref().and_then(|i| core.instance_model(project, i)));
    if let Some(error) = core.quota_refusal(&to, &kind, model.as_deref()) {
        return (409, json!({ "error": error }));
    }

    caller.feed(core, FeedKind::Ask, from, &to, message.clone());
    let prompt = format!(
        "[Mensagem do agente \"{from}\" recebida pelo AgentDeck]\n\n{message}\n\nResponda diretamente ao agente \"{from}\". O usuário acompanha a conversa integrada em tempo real."
    );
    let session_key = format!("{}->{}", caller.key(from), caller.key(&to));
    let result = {
        let _turn = core.inner.bridge_queues.enter(&session_key);
        core.run_headless(
            &kind,
            &prompt,
            HeadlessOptions {
                model,
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

// ---------- Tarefas (`/tasks`) ----------

/// O que uma chamada à rota de tarefas quer fazer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskAction {
    List,
    Show,
    Create,
    Start,
    Done,
    Return,
    Merge,
    Validate,
    Reject,
    Discard,
}

/// O que se lê do método e do caminho: `(ação, id da tarefa, project da query)`.
type TaskRoute = (TaskAction, Option<String>, Option<String>);

/// Lê método e caminho. Erro = (status, mensagem).
fn parse_task_route(method: &Method, url: &str) -> Result<TaskRoute, (u16, String)> {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    let project = query.split('&').find_map(|kv| kv.strip_prefix("project=")).filter(|p| !p.is_empty()).map(str::to_owned);
    let rest = path.strip_prefix("/tasks").unwrap_or("").trim_matches('/');
    let parts: Vec<&str> = if rest.is_empty() { Vec::new() } else { rest.split('/').collect() };
    let unknown = || (404, "rota desconhecida".to_owned());
    let id =
        |text: &str| crate::tasks::parse_task_id(text).ok_or_else(|| (400, format!("id de tarefa inválido: \"{}\"", text.escape_debug())));
    match (method, parts.as_slice()) {
        (Method::Get, []) => Ok((TaskAction::List, None, project)),
        (Method::Post, []) => Ok((TaskAction::Create, None, project)),
        (Method::Get, [t]) => Ok((TaskAction::Show, Some(id(t)?), project)),
        (Method::Post, [t, verb]) => {
            let action = match *verb {
                "start" => TaskAction::Start,
                "done" => TaskAction::Done,
                "return" => TaskAction::Return,
                "merge" => TaskAction::Merge,
                "validate" => TaskAction::Validate,
                "reject" => TaskAction::Reject,
                "discard" => TaskAction::Discard,
                _ => return Err(unknown()),
            };
            Ok((action, Some(id(t)?), project))
        }
        _ => Err(unknown()),
    }
}

/// Quem pode o quê. O agente de uma tarefa (`own` = o id dela) só consulta e entrega o relatório da
/// própria tarefa; qualquer outro agente faz o resto, menos entregar o relatório de uma tarefa. O token
/// mestre (testes e depuração; nenhum agente o recebe) pode tudo.
fn task_permission(action: TaskAction, target: Option<&str>, master: bool, own: Option<&str>) -> Result<(), String> {
    if master {
        return Ok(());
    }
    match (action, own) {
        (TaskAction::List | TaskAction::Show, _) => Ok(()),
        (TaskAction::Done, Some(own)) if target == Some(own) => Ok(()),
        (TaskAction::Done, _) => Err("só o agente da própria tarefa entrega o relatório (ask-agent --task done)".into()),
        (_, Some(_)) => Err("o agente de uma tarefa só consulta tarefas e entrega o relatório da própria (ask-agent --task done)".into()),
        (_, None) => Ok(()),
    }
}

/// `GET /tasks`, `GET /tasks/<id>`, `POST /tasks` e `POST /tasks/<id>/start|done|return|merge|validate|reject|discard`.
/// O projeto vem do token (com o mestre, do corpo ou de `?project=`).
fn tasks_route(core: &Core, mut req: Request, auth: &Auth, bodies: &Arc<AtomicU64>) {
    let method = req.method().clone();
    let (action, id, query_project) = match parse_task_route(&method, req.url()) {
        Ok(v) => v,
        Err((status, error)) => return reply(req, status, json!({ "error": error })),
    };
    let mut msg = json!({});
    let mut _budget = None;
    if method == Method::Post {
        match read_json(&mut req, bodies, true) {
            Ok((m, b)) => (msg, _budget) = (m, Some(b)),
            Err((status, body)) => return reply(req, status, body),
        }
    }
    let Some(fields) = msg.as_object_mut() else {
        return reply(req, 400, json!({ "error": "o corpo precisa ser um objeto JSON" }));
    };
    if let Some(p) = query_project.filter(|_| !fields.contains_key("project")) {
        fields.insert("project".into(), Value::String(p));
    }
    let caller = match caller(core, auth, &msg, "usuário") {
        Ok(c) => c,
        Err(e) => return reply(req, 400, json!({ "error": e })),
    };
    let own = core.task_of_instance(&caller.project, &caller.id).map(|t| t.id);
    if let Err(e) = task_permission(action, id.as_deref(), matches!(auth, Auth::Master), own.as_deref()) {
        return reply(req, 403, json!({ "error": e }));
    }
    let (status, out) = run_task_action(core, &caller, action, id.as_deref(), &msg);
    reply(req, status, out);
}

fn task_json(task: &crate::tasks::Task) -> Value {
    serde_json::to_value(task).unwrap_or(Value::Null)
}

/// Texto opcional do corpo: ausente, `null` ou vazio = `None`; algo que não é texto = erro.
fn body_text(msg: &Value, key: &str) -> Result<Option<String>, (u16, Value)> {
    match msg.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone()).filter(|s| !s.trim().is_empty())),
        Some(_) => Err((400, json!({ "error": format!("\"{key}\" precisa ser um texto") }))),
    }
}

/// O escopo de escrita do corpo: uma lista de textos (um caminho cada) ou um texto com um caminho por linha.
/// Ausente, `null` ou vazio = sem escopo. A normalização dos caminhos é de `task_create`.
fn body_scope(msg: &Value) -> Result<Vec<String>, (u16, Value)> {
    let bad = || (400, json!({ "error": "\"scope\" precisa ser uma lista de caminhos (ou um texto com um caminho por linha)" }));
    match msg.get("scope") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::String(s)) => Ok(s.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect()),
        Some(Value::Array(items)) => items.iter().map(|i| i.as_str().map(|s| s.to_owned()).ok_or_else(bad)).collect(),
        Some(_) => Err(bad()),
    }
}

/// Os campos de `POST /tasks`.
fn task_spec(msg: &Value, creator: &str) -> Result<crate::tasks::TaskSpec, (u16, Value)> {
    let effort = match body_text(msg, "effort")? {
        None => None,
        Some(e) => match crate::models::Effort::parse(&e) {
            Some(e) => Some(e),
            None => {
                let error =
                    format!("esforço inválido: \"{}\" (use low, medium ou high)", e.chars().take(20).collect::<String>().escape_debug());
                return Err((400, json!({ "error": error })));
            }
        },
    };
    Ok(crate::tasks::TaskSpec {
        effort,
        title: body_text(msg, "title")?.unwrap_or_default(),
        brief: body_text(msg, "brief")?.unwrap_or_default(),
        kind: body_text(msg, "kind")?.unwrap_or_default(),
        model: body_text(msg, "model")?,
        base: body_text(msg, "base")?,
        creator: Some(creator.to_owned()),
        scope: body_scope(msg)?,
    })
}

fn run_task_action(core: &Core, caller: &Caller, action: TaskAction, id: Option<&str>, msg: &Value) -> (u16, Value) {
    use crate::tasks::TaskError;
    let project = caller.project.as_str();
    let id = id.unwrap_or_default();
    let outcome = match action {
        TaskAction::List => {
            let tasks: Vec<Value> = core.tasks(project).iter().map(crate::tasks::Task::summary_json).collect();
            return (200, json!({ "tasks": tasks }));
        }
        TaskAction::Show => {
            core.task(project, id).ok_or_else(|| TaskError::NotFound(format!("tarefa \"{id}\" não encontrada neste projeto")))
        }
        TaskAction::Create => {
            let spec = match task_spec(msg, &caller.id) {
                Ok(spec) => spec,
                Err(e) => return e,
            };
            match core.task_create(project, spec) {
                Ok(task) if msg.get("start").and_then(Value::as_bool).unwrap_or(false) => match core.task_start(project, &task.id) {
                    Ok(started) => Ok(started),
                    Err(e) => {
                        let task = core.task(project, &task.id).unwrap_or(task);
                        let error = format!("a tarefa {} foi criada, mas não iniciou: {e}", task.id);
                        return (e.http_status(), json!({ "error": error, "task": task_json(&task) }));
                    }
                },
                other => other,
            }
        }
        TaskAction::Start => core.task_start(project, id),
        TaskAction::Merge => core.task_merge(project, id),
        TaskAction::Validate => core.task_validate(project, id),
        TaskAction::Discard => core.task_discard(project, id),
        TaskAction::Done => match body_text(msg, "report") {
            Ok(report) => core.task_done(project, id, &report.unwrap_or_default()),
            Err(e) => return e,
        },
        TaskAction::Return => match body_text(msg, "feedback") {
            Ok(feedback) => core.task_return(project, id, &feedback.unwrap_or_default()),
            Err(e) => return e,
        },
        TaskAction::Reject => match body_text(msg, "feedback") {
            Ok(feedback) => core.task_reject(project, id, &feedback.unwrap_or_default()),
            Err(e) => return e,
        },
    };
    match outcome {
        Ok(task) => (200, json!({ "task": task_json(&task) })),
        Err(e) => (e.http_status(), json!({ "error": e.message() })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn the_tab_prompt_names_the_request() {
        let p = tab_prompt("claude@dev", 7, "faça\x1b[201~ x");
        assert!(p.starts_with("[Mensagem do agente \"claude@dev\" recebida pelo AgentDeck]\n\nfaça[201~ x\n\n"), "{p}");
        assert!(p.contains("ask-agent --reply 7 \"sua resposta\""), "{p}");
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
    fn task_routes_are_parsed() {
        let ok = |m: Method, url: &str| parse_task_route(&m, url).expect("rota");
        assert_eq!(ok(Method::Get, "/tasks"), (TaskAction::List, None, None));
        assert_eq!(ok(Method::Get, "/tasks?project=p1"), (TaskAction::List, None, Some("p1".into())));
        assert_eq!(ok(Method::Post, "/tasks"), (TaskAction::Create, None, None));
        assert_eq!(ok(Method::Get, "/tasks/t3"), (TaskAction::Show, Some("t3".into()), None));
        assert_eq!(ok(Method::Get, "/tasks/3/"), (TaskAction::Show, Some("t3".into()), None));
        for (verb, action) in [
            ("start", TaskAction::Start),
            ("done", TaskAction::Done),
            ("return", TaskAction::Return),
            ("merge", TaskAction::Merge),
            ("validate", TaskAction::Validate),
            ("reject", TaskAction::Reject),
            ("discard", TaskAction::Discard),
        ] {
            assert_eq!(ok(Method::Post, &format!("/tasks/t3/{verb}?project=p9")), (action, Some("t3".into()), Some("p9".into())));
        }
        for (m, url, status) in [
            (Method::Get, "/tasks/t3/start", 404),
            (Method::Post, "/tasks/t3/explode", 404),
            (Method::Post, "/tasks/t3", 404),
            (Method::Get, "/tasks/t3/a/b", 404),
            (Method::Get, "/tasks/abc", 400),
            (Method::Post, "/tasks/../merge", 400),
        ] {
            assert_eq!(parse_task_route(&m, url).map_err(|e| e.0), Err(status), "{url}");
        }
    }

    #[test]
    fn a_task_agent_only_reads_and_reports_its_own_task() {
        use TaskAction::*;
        // O agente da tarefa t1: consulta e entrega a própria.
        for (action, target, ok) in [
            (List, None, true),
            (Show, Some("t2"), true),
            (Done, Some("t1"), true),
            (Done, Some("t2"), false),
            (Create, None, false),
            (Start, Some("t1"), false),
            (Return, Some("t1"), false),
            (Merge, Some("t1"), false),
            (Validate, Some("t1"), false),
            (Reject, Some("t1"), false),
            (Discard, Some("t1"), false),
        ] {
            assert_eq!(task_permission(action, target, false, Some("t1")).is_ok(), ok, "{action:?} {target:?}");
        }
        // Qualquer outro agente faz o resto, menos entregar relatório.
        for (action, ok) in [
            (List, true),
            (Create, true),
            (Start, true),
            (Return, true),
            (Merge, true),
            (Validate, true),
            (Reject, true),
            (Discard, true),
            (Done, false),
        ] {
            assert_eq!(task_permission(action, Some("t1"), false, None).is_ok(), ok, "{action:?}");
        }
        // O token mestre pode tudo.
        assert!(task_permission(Done, Some("t1"), true, None).is_ok() && task_permission(Merge, Some("t1"), true, Some("t1")).is_ok());
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
