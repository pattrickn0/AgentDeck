//! Local HTTP bridge agent → agent (equivalent to `startBridge`).
//!
//! It listens only on 127.0.0.1 (ephemeral port), requires the `x-agentdeck-token` token and validates
//! the `Host` header against DNS rebinding. Each agent process gets its own token, and it is the token
//! that says who is asking (the body's `from` is only valid with the master token, used in tests).
//! Requests for the same `from->to` pair are served in order of arrival (FIFO queue per pair), one at a
//! time.

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

/// agent → agent → agent, at most.
pub const MAX_DEPTH: i64 = 2;

// ---------- FIFO queue per pair ----------

#[derive(Default)]
struct Ticketing {
    next: u64,
    serving: u64,
}

/// A pair's queue (`from->to`): tickets served in order.
#[derive(Default)]
pub(crate) struct PairQueue {
    state: Mutex<Ticketing>,
    cv: Condvar,
}

/// A turn in the queue; when it is released, it calls the next one (and forgets the pair's queue if
/// nobody else waits).
pub(crate) struct Turn<'a> {
    owner: &'a PairQueues,
    key: String,
    queue: Arc<PairQueue>,
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        // Same lock order as `enter` (map, then the queue): nobody takes a ticket in between.
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

/// Queues per session key (equivalent to `bridgeQueues`). It only keeps the pairs with someone in the
/// queue: the map does not grow with every pair that has ever talked.
#[derive(Default)]
pub(crate) struct PairQueues {
    queues: Mutex<HashMap<String, Arc<PairQueue>>>,
}

impl PairQueues {
    /// Takes a ticket (in call order) and blocks until it is its turn.
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

    /// Pairs with someone in the queue right now.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.queues.lock().len()
    }
}

// ---------- Server ----------

/// Maximum size of a request body (long messages go through the `ask-agent` stdin).
const MAX_BODY: u64 = 16 << 20;
/// Requests being served at the same time (each question waiting for an answer occupies one, also
/// after the 202, while the late answer has not arrived).
const MAX_ACTIVE: usize = 128;
/// Files accepted per `POST /learn` (`ask-agent --learn`) and `POST /outline` (`ask-agent --outline`) call.
const MAX_LEARN_PATHS: usize = 50;
/// Sum of the bodies being read or served at the same time: without it, 128 bodies of 16 MB would be 2 GB.
const MAX_BODIES: u64 = 64 << 20;

/// Counts a request being served while it exists.
pub(crate) struct Active(Arc<AtomicUsize>);

impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Bytes reserved from the body budget ([`MAX_BODIES`]) while it exists.
struct BodyBudget {
    used: Arc<AtomicU64>,
    bytes: u64,
}

impl BodyBudget {
    /// Reserves `bytes`, if they fit in the budget.
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

/// Brings the server up on 127.0.0.1:0 and returns (server, port).
pub(crate) fn start(core: &Core) -> anyhow::Result<(Arc<Server>, u16)> {
    let server = Server::http("127.0.0.1:0").map_err(|e| anyhow::anyhow!("failed to start the HTTP bridge: {e}"))?;
    let port = server.server_addr().to_ip().map(|a| a.port()).ok_or_else(|| anyhow::anyhow!("bridge without a TCP port"))?;
    let server = Arc::new(server);
    let srv = server.clone();
    let weak = Arc::downgrade(&core.inner);
    std::thread::Builder::new().name("cd-bridge".into()).spawn(move || {
        let active = Arc::new(AtomicUsize::new(0));
        let bodies = Arc::new(AtomicU64::new(0));
        // It ends when `Server::unblock` is called (shutdown).
        for req in srv.incoming_requests() {
            if active.fetch_add(1, Ordering::SeqCst) >= MAX_ACTIVE {
                active.fetch_sub(1, Ordering::SeqCst);
                reply(req, 503, json!({ "error": "bridge busy: too many questions in progress" }));
                continue;
            }
            let slot = Active(active.clone());
            let (weak, bodies) = (weak.clone(), bodies.clone());
            // If the thread does not come up, the request is dropped and `slot` frees the place.
            // Agents always read English, whatever language the user picked for the app.
            let _ = std::thread::Builder::new()
                .name("cd-bridge-req".into())
                .spawn(move || crate::i18n::english(|| handle(weak, req, port, slot, &bodies)));
        }
    })?;
    Ok((server, port))
}

fn reply(req: Request, status: u16, body: Value) {
    reply_checked(req, status, body);
}

/// Writes the HTTP response. `false` = the write failed (the client had already gone away).
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

/// The token header (`x-agentdeck-token`); empty if missing.
fn token_header(req: &Request) -> &str {
    header(req, "x-agentdeck-token").filter(|v| !v.is_empty()).unwrap_or("")
}

/// Serves a request. `port` is the bridge port (known before the first request: the `Host` is
/// validated from the start) and `slot` its place among the [`MAX_ACTIVE`].
fn handle(weak: Weak<CoreInner>, mut req: Request, port: u16, slot: Active, bodies: &Arc<AtomicU64>) {
    let Some(inner) = weak.upgrade() else {
        return reply(req, 503, json!({ "error": "AgentDeck shutting down" }));
    };
    let core = Core { inner };
    let Some(auth) = core.authenticate(token_header(&req)) else {
        return reply(req, 403, json!({ "error": "invalid token" }));
    };
    // Host validation against DNS rebinding (no Host is refused too).
    let host = header(&req, "host").unwrap_or("");
    if host != format!("127.0.0.1:{port}") && host != format!("localhost:{port}") {
        return reply(req, 403, json!({ "error": "invalid host" }));
    }

    // The app's second opening asks the window to come back (see `activation`). Only the master token,
    // which the app writes next to the data folder lock: an agent does not bring the user's window to the front.
    if *req.method() == Method::Post && req.url() == "/activate" {
        if !matches!(auth, Auth::Master) {
            return reply(req, 403, json!({ "error": "only AgentDeck itself can request this" }));
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
            return reply(req, 400, json!({ "error": "invalid agent" }));
        }
        return match core.models_inventory(only) {
            Ok(items) => {
                let mut text = crate::models::format_inventory(&items, &core.quota_lines());
                let capacity = core.capability_lines(&items);
                if !capacity.is_empty() {
                    text.push_str("\n\ncapability (level the model reaches: simple, medium, complex; --level on the task):\n  ");
                    text.push_str(&capacity.join("\n  "));
                }
                reply(req, 200, json!({ "text": text }))
            }
            Err(e) => reply(req, 404, json!({ "error": e })),
        };
    }
    if *req.method() == Method::Get && req.url() == "/team" {
        let Auth::Agent(c) = &auth else {
            return reply(req, 400, json!({ "error": "only an agent has a boss and a crew" }));
        };
        // Whoever asks is already aware: the pending notice does not have to go in the next message.
        core.inner.team_changed.lock().remove(&c.key(&c.id));
        let team = core.team(&c.project, &c.id);
        // The models of whoever appears in the answer (themselves, the boss and the crew), for `--team`.
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
            return reply(req, 400, json!({ "error": "only an agent has an inbox" }));
        };
        let all = route.split_once('?').is_some_and(|(_, q)| q.split('&').any(|kv| kv == "all=1"));
        return reply(req, 200, json!({ "answers": core.inner.inbox.read(&c.key(&c.id), all) }));
    }
    if *req.method() == Method::Post && route == "/inbox/delivered" {
        let Auth::Agent(c) = &auth else {
            return reply(req, 400, json!({ "error": "only an agent has an inbox" }));
        };
        let (msg, _budget) = match read_json(&mut req, bodies, false) {
            Ok(v) => v,
            Err((status, body)) => return reply(req, status, body),
        };
        let Some(request) = msg.get("request").and_then(Value::as_u64) else {
            return reply(req, 400, json!({ "error": "missing the question number (request)" }));
        };
        // Only the asker's box: an agent does not mark another one's answer as received.
        let mine = core.inner.inbox.find(request).is_some_and(|(asker, _)| asker == c.key(&c.id));
        if mine {
            core.inner.inbox.mark_delivered(request);
        }
        return reply(req, if mine { 200 } else { 404 }, json!({ "ok": mine }));
    }
    if *req.method() != Method::Post || !["/ask", "/reply", "/signal", "/usage"].contains(&route.as_str()) {
        return reply(req, 404, json!({ "error": "unknown route" }));
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
    // A synchronous answer (200 or a question-to-a-tab error) carries the question number: it only counts
    // as delivered if the body was written without error; otherwise it stays in the inbox. Writing without
    // error does not prove it arrived (`tiny_http` ignores the closed connection, and the first send to
    // whoever already left succeeds): the `ask-agent` that sends `ack` confirms it itself, in
    // `/inbox/delivered`, after printing.
    let answered = (route == "/ask" && status != 202).then(|| out.get("request").and_then(Value::as_u64)).flatten();
    if reply_checked(req, status, out) {
        if let Some(request) = answered.filter(|_| !client_acks) {
            core.inner.inbox.mark_delivered(request);
        }
    } else if let Some(request) = answered {
        log::warn!("answer #{request} not written: the asker's connection had already been closed");
        core.answer_lost(&caller.project, request, "the asker's connection had already been closed");
    }
}

/// Reads the request's JSON body, reserving the announced size in the body budget before reading (the
/// returned `BodyBudget` must live until the end of the serving). With `allow_empty`, an empty body
/// means `{}`. Error = (status, response body).
fn read_json(req: &mut Request, bodies: &Arc<AtomicU64>, allow_empty: bool) -> Result<(Value, BodyBudget), (u16, Value)> {
    // Reserves the announced size (without `Content-Length`, the maximum) before reading.
    let announced = req.body_length().map_or(MAX_BODY + 1, |n| (n as u64).min(MAX_BODY + 1));
    let Some(budget) = BodyBudget::reserve(bodies, announced) else {
        return Err((503, json!({ "error": "bridge busy: too many large messages in progress" })));
    };
    let mut body = Vec::new();
    if req.as_reader().take(MAX_BODY + 1).read_to_end(&mut body).is_err() {
        return Err((400, json!({ "error": "invalid JSON" })));
    }
    if body.len() as u64 > MAX_BODY {
        return Err((413, json!({ "error": "message too large (16 MB maximum)" })));
    }
    if allow_empty && body.iter().all(u8::is_ascii_whitespace) {
        return Ok((json!({}), budget));
    }
    match serde_json::from_slice::<Value>(&body) {
        Ok(msg) => Ok((msg, budget)),
        Err(_) => Err((400, json!({ "error": "invalid JSON" }))),
    }
}

/// Who is calling. With an agent's token, it is that agent, in its project (the body's `from` and
/// `project` are ignored); with the master token, the body's `from`, as long as it is a valid id, in
/// the body's `project` or, without one, in the active project.
fn caller(core: &Core, auth: &Auth, msg: &Value, default_from: &str) -> Result<Caller, String> {
    match auth {
        Auth::Agent(c) => Ok(c.clone()),
        Auth::Master => {
            let from = js_string_or(msg.get("from"), default_from).to_lowercase();
            if from != "usuário" && Ident::parse(&from).is_none() {
                return Err(format!("invalid sender: \"{}\"", from.escape_debug()));
            }
            let project = match msg.get("project").and_then(Value::as_str).filter(|p| !p.is_empty()) {
                Some(p) if core.project_path(p).is_some() => p.to_owned(),
                Some(p) => return Err(format!("unknown project: \"{}\"", p.escape_debug())),
                None => core.active_project_id(),
            };
            if project.is_empty() {
                return Err("no open project".into());
            }
            Ok(Caller { id: from, project, tab: false })
        }
    }
}

/// JS `String(x || default)`.
fn js_string_or(v: Option<&Value>, default: &str) -> String {
    match v {
        None | Some(Value::Null) | Some(Value::Bool(false)) => default.to_owned(),
        Some(Value::String(s)) if s.is_empty() => default.to_owned(),
        Some(Value::Number(n)) if n.as_f64() == Some(0.0) => default.to_owned(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

/// JS `Number(x || 0)` (non-numeric values become 0).
fn js_depth(v: Option<&Value>) -> i64 {
    match v {
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse::<f64>().map(|f| f as i64).unwrap_or(0),
        Some(Value::Bool(true)) => 1,
        _ => 0,
    }
    // A negative one would break through the depth limit.
    .max(0)
}

/// The target `asked` (`agy`, `claude@dev`…) already resolved into an instance id: the model through
/// the alias and the instance (without creating any) in the project. `None` if not a single one serves.
fn existing_instance(core: &Core, project: &str, asked: &str) -> Option<String> {
    let (base, suffix) = asked.split_at(asked.find('@').unwrap_or(asked.len()));
    let kind = resolve_alias(base);
    let want = Ident::parse(&format!("{kind}{suffix}"))?;
    core.inner.instances.lock().get(project)?.find(&want).ok().flatten().map(|i| i.id())
}

/// (Scoped) key of question `request`'s target right now: it follows a rename of the tab.
fn target_now(core: &Core, request: u64, fallback: &str) -> String {
    core.inner.exchange.parties(request).map_or_else(|| fallback.to_owned(), |(_, target)| target)
}

/// Question `request`'s deadline ended without an answer: take the question out of the list. If the
/// answer arrived right at that moment (between the end of the wait and here), it counts: whoever
/// answered already heard "delivered".
fn give_up(core: &Core, request: u64, rx: &crossbeam_channel::Receiver<Reply>, why: String) -> Reply {
    if core.inner.exchange.remove(request) {
        Reply::Failed(why)
    } else {
        rx.try_recv().unwrap_or(Reply::Failed(why))
    }
}

/// How long a question delivered to a tab waits for the answer.
/// The deadline counts from the tab's last activity (see [`reply_deadline`]).
const TAB_REPLY_TIMEOUT: Duration = Duration::from_secs(600);
/// Absolute cap, even if the tab stays active: it avoids waiting forever for a stuck agent.
const TAB_REPLY_MAX: Duration = Duration::from_secs(3600);

/// Until when to wait for `target`'s (scoped key) answer to a question delivered at `sent`.
///
/// It is [`TAB_REPLY_TIMEOUT`] after the question *or* after the tab's last activity (terminal output
/// or a turn in progress), whichever is later: an agent busy with a long task does not blow the
/// deadline, which only starts running when it settles. Limited to [`TAB_REPLY_MAX`].
fn reply_deadline(core: &Core, target: &str, sent: Instant) -> Instant {
    deadline_after(sent, core.inner.activity.last(target))
}

impl Caller {
    /// Key with the caller's project scope for id `id` (its own, or another instance's).
    fn key(&self, id: &str) -> String {
        scoped(&self.project, id)
    }

    /// Integrated conversation entry in the caller's project.
    fn feed(&self, core: &Core, kind: FeedKind, from: &str, to: &str, text: String) {
        core.feed(FeedEntry::new(kind, Some(from), Some(to), text).in_project(&self.project));
    }
}

fn deadline_after(sent: Instant, last_activity: Option<Instant>) -> Instant {
    let quiet_since = last_activity.map_or(sent, |a| a.max(sent));
    (quiet_since + TAB_REPLY_TIMEOUT).min(sent + TAB_REPLY_MAX)
}

/// Message that accompanies an answer delivered to whoever asked.
fn delivered(to: &str) -> Value {
    json!({ "answer": format!("Answer delivered to \"{to}\".") })
}

/// `POST /reply` `{"to","message"[,"reply_to"]}`: the caller answers the question `to` made (or
/// question number `reply_to`). It only concludes a **pending** question (404 if there is none): it
/// never creates a new question.
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
        return (404, json!({ "delivered": false, "error": "no pending question" }));
    };
    caller.feed(core, FeedKind::Answer, from, &to, message);
    (200, json!({ "delivered": true }))
}

/// `POST /signal` `{"state":"busy"|"idle"}`: agent hook saying that it started or ended a turn
/// (`ask-agent --signal`). Only a tab's process signals.
fn signal(core: &Core, auth: &Auth, msg: &Value) -> (u16, Value) {
    let Auth::Agent(Caller { id, project, tab: true }) = auth else {
        return (400, json!({ "error": "only a tab's process sends signals" }));
    };
    let busy = match msg.get("state").and_then(Value::as_str) {
        Some("busy") => true,
        Some("idle") => false,
        _ => return (400, json!({ "error": "invalid state (use busy or idle)" })),
    };
    core.emit(CoreEvent::AgentSignal { project: project.clone(), id: id.clone(), busy });
    (200, json!({ "ok": true }))
}

/// `POST /usage` `{"session":{"used","resets_at_ms"}|null,"weekly":…}`: the agent's plan usage
/// (`ask-agent --usage`, Claude's statusline). The model comes from the token; only a tab's process sends it.
fn usage(core: &Core, auth: &Auth, msg: &Value) -> (u16, Value) {
    let Auth::Agent(Caller { id, tab: true, .. }) = auth else {
        return (400, json!({ "error": "only a tab's process sends usage" }));
    };
    let Some(kind) = Ident::parse(id).map(|i| i.kind).filter(|k| k != "shell") else {
        return (400, json!({ "error": "this tab has no usage limit" }));
    };
    match crate::usage::usage_from_json(msg) {
        Ok(usage) => {
            core.set_usage(&kind, AgentUsage { updated_ms: crate::util::now_ms(), ..usage });
            (200, json!({ "ok": true }))
        }
        Err(e) => (400, json!({ "error": e })),
    }
}

/// Text typed into the terminal of the instance that receives question number `request`.
fn tab_prompt(from: &str, request: u64, message: &str) -> String {
    let message = terminal_text(message);
    format!(
        "[Message from the agent \"{from}\" received by AgentDeck]\n\n{message}\n\n\
Answer by running in the shell: ask-agent --reply {request} \"your answer\"\n\
(the answer only reaches \"{from}\" through that command; text written only here is not delivered. For long answers or ones with accents, \
write them to a file and use: ask-agent --reply {request} --file <file>). The user follows the conversation in the Conversation tab."
    )
}

/// Waits for the answer until `until()` (re-evaluated on every round, since the deadline may extend).
/// `None` = the deadline ended without an answer.
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
                    return Some(Reply::Failed("AgentDeck shutting down".into()));
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return Some(Reply::Failed("question cancelled".into())),
        }
    }
}

/// Answer that arrived after the asker's `ask-agent` had already finished: it is typed into the
/// asker's terminal, as a new message. `target` is the id without the project.
fn deliver_late(core: &Core, project: &str, asker: Ident, target: &str, request: u64, outcome: Reply) {
    let (text, failed) = match &outcome {
        Reply::Answer(text) => (text, false),
        Reply::Failed(text) => (text, true),
    };
    // It stays in the inbox until the interface types it into the terminal (`Core::answer_typed`).
    inbox_push(core, &scoped(project, &asker.id()), request, target, text, failed);
    let prompt = match outcome {
        Reply::Answer(text) => format!(
            "[Answer from the agent \"{target}\" to your question #{request}, received by AgentDeck]\n\n{}\n\n\
Continue the task based on this answer. The user follows the conversation in the Conversation tab.",
            terminal_text(&text)
        ),
        Reply::Failed(text) => {
            core.feed(FeedEntry::new(FeedKind::Error, Some(target), Some(&asker.id()), text.clone()).in_project(project));
            format!("[AgentDeck] Question #{request} to \"{target}\" ended without an answer: {}", terminal_text(&text))
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

/// Question delivered to an interactive tab (instance) and answered by `ask-agent --reply`.
/// The target is resolved (and created, if needed) in the asker's project.
///
/// If the asker is a tab and asked for `wait` seconds, its `ask-agent` returns after that time (before
/// the agent's Bash blows its own timeout) and the answer, when it arrives, is typed into its terminal.
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
            json!({ "error": format!("unknown or not installed agent \"{asked}\". Available: {}", core.peers(project).join(", ")) }),
        );
    };
    if let Some(m) = opts.model {
        if !crate::models::accepts_model(&want.kind) {
            return (400, json!({ "error": format!("the agent {} does not accept choosing a model", want.kind) }));
        }
        // The requested instance is already open with another model: refuse before creating or registering anything.
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
            return (400, json!({ "error": format!("\"{asked}\" is ambiguous: {}. Use the full name.", options.join(", ")) }));
        }
    };
    let id = ident.id();
    // New instance: the requested model (otherwise the role's default) is stored right away, before the tab opens.
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
        return forget(400, json!({ "error": "an agent cannot ask itself" }));
    }
    // A model without quota does not answer: better for the agent to know now and choose another one.
    if let Some(error) = core.quota_refusal(&id, &ident.kind, core.instance_model(project, &ident).as_deref()) {
        return forget(409, json!({ "error": error }));
    }
    // With tabs the depth does not travel through the environment (the question is typed): it is measured
    // by the asker's chain of pending questions. A wait cycle is refused first (409).
    let (from_key, target_key) = (caller.key(from), caller.key(&id));
    let depth = opts.depth.max(core.inner.exchange.chain_depth(&from_key));
    if depth >= MAX_DEPTH && !core.inner.exchange.would_cycle(&from_key, &target_key) {
        return forget(400, json!({ "error": "maximum conversation depth between agents reached" }));
    }
    let (request, rx) = match core.inner.exchange.register_at(&from_key, &target_key, depth) {
        Ok(v) => v,
        // The Exchange error message quotes the scoped keys: rebuilt with the ids.
        Err(_) => {
            return forget(409, json!({ "error": format!("\"{id}\" is already waiting for an answer from \"{from}\" (wait cycle)") }))
        }
    };
    // The project was removed in between (the removal fails the questions registered before it; this one is
    // checked here, after being registered).
    if core.project_path(project).is_none() {
        core.inner.exchange.remove(request);
        return forget(400, json!({ "error": "the project was removed" }));
    }

    caller.feed(core, FeedKind::Ask, from, &id, message.to_owned());
    // Whoever opened the instance becomes its boss (the user, through the master token, does not count).
    if created && core.project_instance_ids(project).iter().any(|o| o == from) {
        if let Err(e) = core.set_boss(project, &id, Some(from)) {
            log::warn!("boss of \"{id}\": {e}");
        }
    }
    // The pending team notice goes when the interface types it (`Core::take_team_notice`): if the delivery
    // is discarded, it is not lost.
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
    // Only an agent receives a late answer typed in: in a common terminal (`shell`) the text would be
    // executed as a command. There the `ask-agent` waits until the end.
    let asker = Ident::parse(from).filter(|i| caller.tab && i.kind != "shell");
    let detach_at = match (opts.wait, &asker) {
        (Some(w), Some(_)) if w > 0 => Some(Instant::now() + Duration::from_secs(w)),
        _ => None,
    };
    // The target is read by the number on every round: renaming the tab in the middle of the question does
    // not lose its activity (the deadline counts from it).
    let deadline = || reply_deadline(core, &target_now(core, request, &target_key), sent);
    let waited = wait_reply(core, &rx, || detach_at.map_or_else(deadline, |d| d.min(deadline())));
    let (asker_key, target_key_now) = core.inner.exchange.parties(request).unwrap_or_else(|| (from_key, target_key.clone()));
    let (asker_id, target_id) = (unscoped(&asker_key).1.to_owned(), unscoped(&target_key_now).1.to_owned());
    let outcome = match waited {
        Some(reply) => reply,
        None => match (detach_at, asker) {
            (Some(_), Some(asker)) if Instant::now() < deadline() => {
                core.inner.exchange.detach(request);
                // The answer arrived at the instant of the 202: it goes right now.
                match rx.try_recv() {
                    Ok(reply) => reply,
                    Err(_) => return detach(core, project, request, rx, sent, asker, target_key_now, slot),
                }
            }
            _ => give_up(core, request, &rx, format!("\"{target_id}\" did not answer within 10 minutes")),
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

/// Stores in `asker_key`'s (scoped key) inbox the outcome of question `request`, still as not delivered.
/// Whoever delivers it marks it later (see `Core::answer_typed` and the end of [`handle`]).
fn inbox_push(core: &Core, asker_key: &str, request: u64, from: &str, text: &str, failed: bool) {
    let entry = InboxEntry { request, from: from.to_owned(), text: text.to_owned(), failed, ts: crate::util::now_ms(), delivered: false };
    core.inner.inbox.push(asker_key, entry);
}

/// The asker's `ask-agent` returns now (202) and the answer, when it arrives, is typed into its terminal
/// by a thread that keeps the request's `slot`. `target_key` is the scoped target; the asker and the
/// target are read again by the number at the end (renames).
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
    // The late answer goes to an agent too: English, like the request threads.
    let spawned = std::thread::Builder::new().name("cd-bridge-late".into()).spawn(move || {
        crate::i18n::english(|| {
            let _slot = slot;
            let core = core2;
            let waited = wait_reply(&core, &rx, || reply_deadline(&core, &target_now(&core, request, &target_key), sent));
            let parties = core.inner.exchange.parties(request);
            let (asker, target) = match &parties {
                Some((a, t)) => (Ident::parse(unscoped(a).1).unwrap_or(asker), unscoped(t).1.to_owned()),
                None => (asker, unscoped(&target_key).1.to_owned()),
            };
            let outcome = waited.unwrap_or_else(|| give_up(&core, request, &rx, format!("\"{target}\" did not answer within 10 minutes")));
            core.inner.exchange.remove(request);
            deliver_late(&core, &project, asker, &target, request, outcome);
        })
    });
    if spawned.is_err() {
        core.inner.exchange.fail(request, "bridge internal failure");
        core.inner.exchange.remove(request);
        return (500, json!({ "error": "bridge internal failure" }));
    }
    (
        202,
        json!({
            "pending": request,
            "answer": format!(
                "Question #{request} was delivered to \"{target}\", which is still working on it. The answer will be typed into \
        this terminal when it arrives, as a new message. Do not repeat the question: go on with another part of the task or end the turn and wait. \
        If the answer does not show up, run: ask-agent --inbox"
            ),
        }),
    )
}

/// Options of a question to a tab.
struct TabAsk<'a> {
    force_new: bool,
    /// Requested model (`ask-agent --model`), already validated.
    model: Option<&'a str>,
    depth: i64,
    /// Seconds the asker's `ask-agent` accepts waiting (`None`/0 = until the end).
    wait: Option<u64>,
}

/// The `/ask` `model` field: missing or empty = none; text outside the pattern = error.
fn requested_model(v: Option<&Value>) -> Result<Option<String>, String> {
    let m = match v {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(s)) if s.trim().is_empty() => return Ok(None),
        Some(Value::String(s)) => s.trim(),
        Some(_) => return Err("invalid model: use a text".into()),
    };
    if crate::models::valid_model(m) {
        Ok(Some(m.to_owned()))
    } else {
        Err(format!(
            "invalid model: \"{}\" (use letters, digits and : . _ / # @ + -, up to {} characters)",
            m.chars().take(40).collect::<String>().escape_debug(),
            crate::models::MAX_MODEL_LEN
        ))
    }
}

/// Processes an already decoded `POST /ask`. Returns (HTTP status, body). Everything happens in the
/// caller's project: the instances, the pending questions and the headless sessions.
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

    // Explicit answer to question number N (`ask-agent --reply N "text"`).
    if let Some(id) = msg.get("reply_to").and_then(Value::as_u64) {
        return match core.inner.exchange.reply_id(id, &caller.key(from), &message) {
            Some(asker) => {
                let asker = unscoped(&asker).1;
                caller.feed(core, FeedKind::Answer, from, asker, message);
                (200, delivered(asker))
            }
            None => (
                404,
                json!({ "error": format!("question #{id} is not waiting for an answer from \"{from}\" (it was already answered, cancelled or was for another agent)") }),
            ),
        };
    }
    // `from` is answering a question that `asked` made and whose `ask-agent` still waits: it delivers to
    // it. (A new question from `from` to whoever waits for it would close a wait cycle; that is why this
    // message can only be an answer.) The name goes through the aliases (`agy`, `qwen3.5`) and the instances.
    let asker = existing_instance(core, project, &asked).unwrap_or_else(|| asked.clone());
    if core.inner.exchange.reply_waiting(&caller.key(from), &caller.key(&asker), &message) {
        caller.feed(core, FeedKind::Answer, from, &asker, message);
        return (200, delivered(&asker));
    }
    let model = match requested_model(msg.get("model")) {
        Ok(m) => m,
        Err(e) => return (400, json!({ "error": e })),
    };
    // With the interface running, the question goes to the agent's tab (instance); without it, headless.
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
        return (400, json!({ "error": format!("unknown or not installed agent \"{asked}\". Available: {}", available.join(", ")) }));
    }
    if to == from {
        return (400, json!({ "error": "an agent cannot ask itself" }));
    }
    let to_ident = Ident::parse(&to);
    let kind = to_ident.as_ref().map(|i| i.kind.clone()).unwrap_or_else(|| to.clone());
    if model.is_some() && !crate::models::accepts_model(&kind) {
        return (400, json!({ "error": format!("the agent {kind} does not accept choosing a model") }));
    }
    if depth >= MAX_DEPTH {
        return (400, json!({ "error": "maximum conversation depth between agents reached" }));
    }
    // Without tabs there is no live instance: the request counts, otherwise the stored one or the role's default.
    let model = model.or_else(|| to_ident.as_ref().and_then(|i| core.instance_model(project, i)));
    if let Some(error) = core.quota_refusal(&to, &kind, model.as_deref()) {
        return (409, json!({ "error": error }));
    }

    caller.feed(core, FeedKind::Ask, from, &to, message.clone());
    let prompt = format!(
        "[Message from the agent \"{from}\" received by AgentDeck]\n\n{message}\n\nAnswer the agent \"{from}\" directly. The user follows the conversation in the Conversation tab in real time."
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

// ---------- Tasks (`/tasks`) ----------

/// What a call to the tasks route wants to do.
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

/// What is read from the method and the path: `(action, task id, project from the query)`.
type TaskRoute = (TaskAction, Option<String>, Option<String>);

/// Reads method and path. Error = (status, message).
fn parse_task_route(method: &Method, url: &str) -> Result<TaskRoute, (u16, String)> {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    let project = query.split('&').find_map(|kv| kv.strip_prefix("project=")).filter(|p| !p.is_empty()).map(str::to_owned);
    let rest = path.strip_prefix("/tasks").unwrap_or("").trim_matches('/');
    let parts: Vec<&str> = if rest.is_empty() { Vec::new() } else { rest.split('/').collect() };
    let unknown = || (404, "unknown route".to_owned());
    let id = |text: &str| crate::tasks::parse_task_id(text).ok_or_else(|| (400, format!("invalid task id: \"{}\"", text.escape_debug())));
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

/// Who may do what. A task's agent (`own` = its id) only reads and delivers the report of its own task;
/// any other agent does the rest, except delivering a task's report. The master token (tests and
/// debugging; no agent receives it) may do everything.
fn task_permission(action: TaskAction, target: Option<&str>, master: bool, own: Option<&str>) -> Result<(), String> {
    if master {
        return Ok(());
    }
    match (action, own) {
        (TaskAction::List | TaskAction::Show, _) => Ok(()),
        (TaskAction::Done, Some(own)) if target == Some(own) => Ok(()),
        (TaskAction::Done, _) => Err("only a task's own agent delivers the report (ask-agent --task done)".into()),
        (_, Some(_)) => Err("the agent of a task only reads tasks and delivers the report of its own (ask-agent --task done)".into()),
        (_, None) => Ok(()),
    }
}

/// `GET /tasks`, `GET /tasks/<id>`, `POST /tasks` and `POST /tasks/<id>/start|done|return|merge|validate|reject|discard`.
/// The project comes from the token (with the master one, from the body or from `?project=`).
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
        return reply(req, 400, json!({ "error": "the body must be a JSON object" }));
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

/// Optional body text: missing, `null` or empty = `None`; something that is not text = error.
fn body_text(msg: &Value, key: &str) -> Result<Option<String>, (u16, Value)> {
    match msg.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone()).filter(|s| !s.trim().is_empty())),
        Some(_) => Err((400, json!({ "error": format!("\"{key}\" must be text") }))),
    }
}

/// The body's write scope: a list of texts (one path each) or a text with one path per line.
/// Missing, `null` or empty = no scope. The normalization of the paths belongs to `task_create`.
fn body_scope(msg: &Value) -> Result<Vec<String>, (u16, Value)> {
    let bad = || (400, json!({ "error": "\"scope\" must be a list of paths (or a text with one path per line)" }));
    match msg.get("scope") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::String(s)) => Ok(s.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect()),
        Some(Value::Array(items)) => items.iter().map(|i| i.as_str().map(|s| s.to_owned()).ok_or_else(bad)).collect(),
        Some(_) => Err(bad()),
    }
}

/// The fields of `POST /tasks`.
fn task_spec(msg: &Value, creator: &str) -> Result<crate::tasks::TaskSpec, (u16, Value)> {
    let effort = match body_text(msg, "effort")? {
        None => None,
        Some(e) => match crate::models::Effort::parse(&e) {
            Some(e) => Some(e),
            None => {
                let error =
                    format!("invalid effort: \"{}\" (use low, medium or high)", e.chars().take(20).collect::<String>().escape_debug());
                return Err((400, json!({ "error": error })));
            }
        },
    };
    let level = match body_text(msg, "level")? {
        None => crate::capability::Level::default(),
        Some(l) => match crate::capability::Level::parse(&l) {
            Some(l) => l,
            None => {
                let error =
                    format!("invalid level: \"{}\" (use simple, medium or complex)", l.chars().take(20).collect::<String>().escape_debug());
                return Err((400, json!({ "error": error })));
            }
        },
    };
    Ok(crate::tasks::TaskSpec {
        effort,
        level,
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
        TaskAction::Show => core.task(project, id).ok_or_else(|| TaskError::NotFound(format!("task \"{id}\" not found in this project"))),
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
                        let error = format!("task {} was created, but did not start: {e}", task.id);
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
        let p = tab_prompt("claude@dev", 7, "do\x1b[201~ x");
        assert!(p.starts_with("[Message from the agent \"claude@dev\" received by AgentDeck]\n\ndo[201~ x\n\n"), "{p}");
        assert!(p.contains("ask-agent --reply 7 \"your answer\""), "{p}");
        assert!(p.contains("Answer by running in the shell: ask-agent --reply 7 "), "{p}");
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
        // It holds the turn so that the next ones wait in order.
        let first = queues.enter("a->b");
        let mut handles = Vec::new();
        for i in 0..5 {
            let (q, act, ord) = (queues.clone(), active.clone(), order.clone());
            handles.push(std::thread::spawn(move || {
                let _t = q.enter("a->b");
                assert_eq!(act.fetch_add(1, Ordering::SeqCst), 0, "two simultaneous runs on the same pair");
                ord.lock().push(i);
                std::thread::sleep(Duration::from_millis(20));
                act.fetch_sub(1, Ordering::SeqCst);
            }));
            // Guarantees the order of arrival.
            std::thread::sleep(Duration::from_millis(30));
        }
        // Another pair does not wait.
        drop(queues.enter("c->d"));
        drop(first);
        for h in handles {
            h.join().expect("thread");
        }
        assert_eq!(*order.lock(), vec![0, 1, 2, 3, 4]);
        assert_eq!(queues.len(), 0, "with nobody in the queue, the pairs are forgotten");
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
        // And it works again after being forgotten.
        drop(queues.enter("a->b"));
        assert_eq!(queues.len(), 0);
    }

    #[test]
    fn task_routes_are_parsed() {
        let ok = |m: Method, url: &str| parse_task_route(&m, url).expect("route");
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
        // Task t1's agent: it reads and delivers its own.
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
        // Any other agent does the rest, except delivering a report.
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
        // The master token may do everything.
        assert!(task_permission(Done, Some("t1"), true, None).is_ok() && task_permission(Merge, Some("t1"), true, Some("t1")).is_ok());
    }

    #[test]
    fn the_body_budget_caps_the_bytes_in_flight() {
        let used = Arc::new(AtomicU64::new(0));
        let big: Vec<BodyBudget> = (0..4).map(|_| BodyBudget::reserve(&used, MAX_BODY).expect("fits")).collect();
        assert!(BodyBudget::reserve(&used, 1).is_none(), "budget full");
        assert_eq!(used.load(Ordering::SeqCst), MAX_BODIES, "the refused attempt is not left reserved");
        drop(big);
        assert_eq!(used.load(Ordering::SeqCst), 0);
        assert!(BodyBudget::reserve(&used, 1).is_some());
    }
}

#[cfg(test)]
mod deadline_tests {
    use super::*;

    #[test]
    fn the_deadline_counts_from_the_end_of_the_activity() {
        let t0 = Instant::now();
        let min = |n: u64| Duration::from_secs(n * 60);
        // no activity: 10 min after the question
        assert_eq!(deadline_after(t0, None), t0 + min(10));
        // activity before the question does not postpone it
        assert_eq!(deadline_after(t0 + min(5), Some(t0)), t0 + min(15));
        // tab still working 30 min later: the deadline goes to 40 min
        assert_eq!(deadline_after(t0, Some(t0 + min(30))), t0 + min(40));
        // 60 min cap even with continuous activity
        assert_eq!(deadline_after(t0, Some(t0 + min(55))), t0 + min(60));
    }
}
