//! Uso dos planos dos agentes (limite da sessão e da semana), mostrado na barra de título.
//!
//! Fontes (cada uma preenche [`UsageStore`] com [`Core::set_usage`]):
//! - **Claude**: a statusline do Claude Code (`ask-agent --usage`, rota `POST /usage` da ponte);
//! - **Codex**: o último `rate_limits` dos arquivos `~/.codex/sessions/**/rollout-*.jsonl`;
//! - **Antigravity**: a cota consultada pela API que o próprio CLI usa;
//!
//! Codex e Antigravity são consultados quando há aba aberta deles **ou** o CLI está instalado, e o último uso de
//! cada um fica em `dados/usage.json`: a barra mostra todos os agentes com cota, mesmo sem janela aberta (o do Claude
//! é o último que a statusline mandou).
//!
//! CONTRATO ESTÁVEL: `UsageWindow`, `AgentUsage`, `Core::usage_snapshot` e `Core::set_usage`
//! são consumidos pela UI.

mod antigravity;
mod codex;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::{json, Value};

use crate::{util, Core, CoreEvent};

/// De quanto em quanto tempo o Codex é relido (arquivo local, barato).
const CODEX_EVERY: Duration = Duration::from_secs(60);
/// De quanto em quanto tempo a cota do Antigravity é consultada (rede).
const ANTIGRAVITY_EVERY: Duration = Duration::from_secs(180);
/// Intervalo curto de nova tentativa quando uma leitura/consulta de uso falha (`None`) com a aba aberta.
const RETRY_EVERY: Duration = Duration::from_secs(20);
/// De quanto em quanto tempo se confere quais CLIs estão instalados.
const INSTALLED_EVERY: Duration = Duration::from_secs(600);
/// O arquivo com o último uso conhecido, na pasta de dados.
pub const USAGE_FILE: &str = "usage.json";

/// Uma janela de limite (sessão de 5 h, semana…).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UsageWindow {
    /// Fração usada, de 0.0 a 1.0.
    pub used: f32,
    /// Quando a janela renova (ms desde a época Unix), se a fonte disser.
    pub resets_at_ms: Option<u64>,
}

/// Uso de um modelo de agente (`claude`, `codex`, `antigravity`…).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AgentUsage {
    /// Limite da sessão (o círculo que enche).
    pub session: Option<UsageWindow>,
    /// Limite semanal (o anel externo; vermelho quando acaba).
    pub weekly: Option<UsageWindow>,
    /// Quando o dado foi lido (ms desde a época Unix).
    pub updated_ms: u64,
    /// Agente local, sem limite (sem cota para mostrar).
    pub local: bool,
    /// As cotas de cada grupo de modelos, quando a fonte as separa (o Antigravity: "Gemini Models",
    /// "Gemini 4 Argon"…); vazio nas outras. `session` e `weekly` são o resumo que a barra mostra.
    pub groups: Vec<UsageGroup>,
}

/// A cota de um grupo de modelos dentro do plano.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageGroup {
    pub name: String,
    pub session: Option<UsageWindow>,
    pub weekly: Option<UsageWindow>,
}

/// Último uso conhecido de cada modelo; com `file`, guardado em disco a cada mudança.
#[derive(Default)]
pub struct UsageStore {
    map: Mutex<HashMap<String, AgentUsage>>,
    file: Option<PathBuf>,
    /// Uma gravação por vez, fora do cadeado do `map` (que a UI pega a cada quadro); guarda quando foi a última.
    disk: Mutex<u64>,
}

/// Leitura nova sem mudança de porcentagem: o "lido há" vai para o disco no máximo a cada tanto.
const SAVE_READ_EVERY_MS: u64 = 5 * 60 * 1000;

impl UsageStore {
    /// Começa com o que `file` guardou (vazio se ele não existe ou não se lê).
    pub(crate) fn load(file: PathBuf) -> Self {
        let map = std::fs::read(&file)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .map(|v| store_from_json(&v))
            .unwrap_or_default();
        Self { map: Mutex::new(map), file: Some(file), disk: Mutex::new(0) }
    }

    /// Grava e diz se mudou o que se vê. Uma leitura que só renova `updated_ms` também vai para o disco, mas no
    /// máximo a cada [`SAVE_READ_EVERY_MS`] (senão, ao reabrir o app, o painel diria "lido há" errado).
    fn set(&self, model: &str, usage: AgentUsage) -> bool {
        let changed = {
            let mut map = self.map.lock();
            let old = map.get(model);
            // `updated_ms` sozinho não é mudança visível o bastante para acordar a UI.
            let changed = old.is_none_or(|o| {
                o.session != usage.session || o.weekly != usage.weekly || o.local != usage.local || o.groups != usage.groups
            });
            map.insert(model.to_owned(), usage);
            changed
        };
        if self.file.is_some() {
            let now = util::now_ms();
            let mut last = self.disk.lock();
            if changed || now.saturating_sub(*last) >= SAVE_READ_EVERY_MS {
                // O retrato é tirado já com a vez de gravar: a última gravação é sempre a mais nova.
                let text = store_to_json(&self.map.lock()).to_string();
                self.save(&text);
                *last = now;
            }
        }
        changed
    }

    /// Grava ao lado e troca: uma queda no meio não deixa o arquivo pela metade.
    fn save(&self, text: &str) {
        let Some(file) = &self.file else { return };
        let tmp = file.with_extension("json.tmp");
        if let Err(e) = std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, file)) {
            log::warn!("não consegui guardar o uso em {}: {e}", file.display());
        }
    }

    fn snapshot(&self) -> HashMap<String, AgentUsage> {
        self.map.lock().clone()
    }
}

impl Core {
    /// Retrato do uso por modelo (barato; a UI chama a cada quadro).
    pub fn usage_snapshot(&self) -> HashMap<String, AgentUsage> {
        self.inner.usage.snapshot()
    }

    /// Atualiza o uso de um modelo (`claude`, `codex`, `antigravity`…) e avisa a UI se mudou.
    pub fn set_usage(&self, model: &str, usage: AgentUsage) {
        if self.inner.usage.set(model, usage) {
            self.emit(CoreEvent::UsageChanged);
        }
    }
}

// ---------- Cota de um modelo (`--models`, `--team` e a recusa do `/ask`) ----------

/// Uso a partir do qual a cota conta como esgotada (o servidor arredonda: 0,5% restante não dá uma resposta).
pub const EXHAUSTED: f32 = 0.995;
/// Leitura mais velha que isto não serve para recusar uma pergunta (a cota pode ter sido renovada ou comprada).
const FRESH_MS: u64 = 60 * 60 * 1000;

/// A cota que vale para um modelo: a do grupo dele, quando a fonte separa (o Antigravity: Gemini, Argon,
/// Claude/GPT), ou a do plano todo.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelQuota {
    /// O grupo ("Gemini 4 Argon"); `None` = o plano do agente.
    pub group: Option<String>,
    /// Fração usada da janela mais gasta, de 0.0 a 1.0.
    pub used: f32,
    /// Quando ela renova (ms Unix), se a fonte disser.
    pub resets_at_ms: Option<u64>,
}

impl ModelQuota {
    pub fn exhausted(&self) -> bool {
        self.used >= EXHAUSTED
    }

    /// "cota de Gemini 4 Argon esgotada, renova em 8 h 45 min", "cota de Gemini 4 Argon 4% usada, renova em 6 dias".
    pub fn note(&self, now_ms: u64) -> String {
        let of = self.group.as_ref().map(|g| format!(" de {g}")).unwrap_or_default();
        let renews = self.resets_at_ms.filter(|&r| r > now_ms).map(|r| format!(", renova em {}", duration_text(r - now_ms)));
        if self.exhausted() {
            format!("cota{of} esgotada{}", renews.unwrap_or_default())
        } else {
            format!("cota{of} {:.0}% usada{}", self.used * 100.0, renews.unwrap_or_default())
        }
    }
}

/// "45 min", "8 h 45 min", "3 dias".
fn duration_text(ms: u64) -> String {
    let min = ms.div_ceil(60_000);
    match (min / 60, min % 60) {
        (0, m) => format!("{m} min"),
        (h, _) if h >= 48 => format!("{} dias", h / 24),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// O grupo de cota de `model` do Antigravity: o Argon tem o seu, os outros Gemini dividem "Gemini Models" e Claude
/// e GPT dividem "Claude and GPT models".
fn group_of<'a>(groups: &'a [UsageGroup], model: &str) -> Option<&'a UsageGroup> {
    let m = model.to_ascii_lowercase();
    groups.iter().find(|g| {
        let n = g.name.to_ascii_lowercase();
        if m.contains("argon") {
            n.contains("argon")
        } else if m.starts_with("gemini") {
            n.contains("gemini") && !n.contains("argon")
        } else {
            n.contains("claude") || n.contains("gpt")
        }
    })
}

/// A cota de `model` em `u`, ignorando as janelas já renovadas. Com grupos, só a do grupo do modelo (sem modelo ou
/// sem grupo dele, `None`: o padrão do CLI pode ser qualquer um).
pub fn model_quota(u: &AgentUsage, model: Option<&str>, now_ms: u64) -> Option<ModelQuota> {
    let (group, windows) = if u.groups.is_empty() {
        (None, [u.session, u.weekly])
    } else {
        let g = group_of(&u.groups, model?)?;
        (Some(g.name.clone()), [g.session, g.weekly])
    };
    let live: Vec<UsageWindow> = windows.into_iter().flatten().filter(|w| w.resets_at_ms.is_none_or(|r| r > now_ms)).collect();
    let used = live.iter().map(|w| w.used).fold(None, |a: Option<f32>, u| Some(a.map_or(u, |a| a.max(u))))?;
    // Esgotada, vale quando a última janela esgotada renova; senão, a da mais gasta.
    let resets_at_ms = if used >= EXHAUSTED {
        live.iter().filter(|w| w.used >= EXHAUSTED).filter_map(|w| w.resets_at_ms).max()
    } else {
        live.iter().find(|w| w.used == used).and_then(|w| w.resets_at_ms)
    };
    Some(ModelQuota { group, used, resets_at_ms })
}

/// As cotas de `u` numa linha para o `--models`: "Gemini Models 0% usada (renova em 5 h), Gemini 4 Argon 4% usada (renova em 6 dias)…".
pub fn quota_line(u: &AgentUsage, now_ms: u64) -> Option<String> {
    let one = |name: Option<&str>, windows: [Option<UsageWindow>; 2]| {
        let q = model_quota(&AgentUsage { session: windows[0], weekly: windows[1], ..Default::default() }, None, now_ms)?;
        let name = name.map(|n| format!("{n} ")).unwrap_or_default();
        let renews = q.resets_at_ms.filter(|&r| r > now_ms).map(|r| format!(" (renova em {})", duration_text(r - now_ms)));
        Some(if q.exhausted() {
            format!("{name}ESGOTADA{}", renews.unwrap_or_default())
        } else {
            format!("{name}{:.0}% usada{}", q.used * 100.0, renews.unwrap_or_default())
        })
    };
    let parts: Vec<String> = if u.groups.is_empty() {
        one(None, [u.session, u.weekly]).into_iter().collect()
    } else {
        u.groups.iter().filter_map(|g| one(Some(&g.name), [g.session, g.weekly])).collect()
    };
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// O que mostrar quando um dos quatro CLIs oficiais não tem leitura viva: quando foi a última, ou "sem leitura".
pub fn no_reading_text(u: Option<&AgentUsage>, now_ms: u64) -> String {
    match u.filter(|u| u.updated_ms > 0) {
        Some(u) => format!("última leitura há {}", duration_text(now_ms.saturating_sub(u.updated_ms).max(60_000))),
        None => "sem leitura".to_owned(),
    }
}

/// A fração usada que conta para ordenar o agente no `--models` (menor = mais folga): sem grupos, a janela mais
/// gasta; com grupos (Antigravity), a mais gasta entre os grupos do Gemini (ou entre todos, se não houver Gemini).
pub fn agent_used(u: &AgentUsage, now_ms: u64) -> Option<f32> {
    if u.groups.is_empty() {
        return model_quota(u, None, now_ms).map(|q| q.used);
    }
    let any_gemini = u.groups.iter().any(|g| g.name.to_ascii_lowercase().contains("gemini"));
    u.groups
        .iter()
        .filter(|g| !any_gemini || g.name.to_ascii_lowercase().contains("gemini"))
        .filter_map(|g| model_quota(&AgentUsage { session: g.session, weekly: g.weekly, ..Default::default() }, None, now_ms))
        .map(|q| q.used)
        .fold(None, |a: Option<f32>, u| Some(a.map_or(u, |a| a.max(u))))
}

impl Core {
    /// A cota de `model` do agente `kind` (`claude`, `antigravity`…), pela última leitura; `None` sem dado.
    pub fn model_quota(&self, kind: &str, model: Option<&str>) -> Option<ModelQuota> {
        let usage = self.inner.usage.snapshot();
        model_quota(usage.get(kind)?, model, util::now_ms())
    }

    /// A cota de cada instância de `models` (id → texto), para o `--team` e o `--list`: porcentagem e renovação
    /// mesmo abaixo de 80%, ou "cota: sem leitura" / "cota: última leitura há N min" nos quatro CLIs oficiais.
    pub fn quota_notes(&self, models: &std::collections::BTreeMap<String, String>) -> std::collections::BTreeMap<String, String> {
        let now = util::now_ms();
        let usage = self.inner.usage.snapshot();
        models
            .iter()
            .filter_map(|(id, model)| {
                let kind = crate::Ident::parse(id)?.kind;
                if let Some(q) = usage.get(&kind).and_then(|u| model_quota(u, Some(model), now)) {
                    Some((id.clone(), q.note(now)))
                } else if crate::models::accepts_model(&kind) {
                    Some((id.clone(), format!("cota: {}", no_reading_text(usage.get(&kind), now))))
                } else {
                    None
                }
            })
            .collect()
    }

    /// A linha de cota de cada agente (agente → linha), para o `--models`: sempre presente nos quatro CLIs oficiais
    /// (com a leitura viva, "última leitura há N min" ou "sem leitura") e nos demais quando há leitura.
    pub fn quota_lines(&self) -> HashMap<String, String> {
        let now = util::now_ms();
        let usage = self.inner.usage.snapshot();
        let mut out: HashMap<String, String> = usage.iter().filter_map(|(kind, u)| Some((kind.clone(), quota_line(u, now)?))).collect();
        for kind in ["claude", "antigravity", "codex", "opencode"] {
            out.entry(kind.to_owned()).or_insert_with(|| no_reading_text(usage.get(kind), now));
        }
        out
    }

    /// A fração usada de cada agente com leitura viva (agente → 0..=1), para ordenar o `--models` por quem tem mais folga.
    pub fn quota_used(&self) -> HashMap<String, f32> {
        let now = util::now_ms();
        self.inner.usage.snapshot().iter().filter_map(|(kind, u)| Some((kind.clone(), agent_used(u, now)?))).collect()
    }

    /// O erro para uma pergunta a `id` (agente `kind`, modelo `model`) quando a cota do modelo acabou, por uma
    /// leitura recente. Sem dado, com folga ou com leitura velha, `None`: a pergunta segue.
    pub(crate) fn quota_refusal(&self, id: &str, kind: &str, model: Option<&str>) -> Option<String> {
        let usage = self.inner.usage.snapshot();
        let u = usage.get(kind)?;
        let now = util::now_ms();
        if now.saturating_sub(u.updated_ms) > FRESH_MS {
            return None;
        }
        let q = model_quota(u, model, now).filter(ModelQuota::exhausted)?;
        let using = model.map(|m| format!(" usa {m} e a")).unwrap_or_else(|| ": a".to_owned());
        let note = q.note(now);
        Some(format!(
            "{id}{using} {note}; a mensagem não foi entregue. Escolha um modelo com cota (ask-agent --models {kind} mostra a cota de \
             cada grupo) e abra outra instância: ask-agent {id} --new --model <modelo> \"mensagem\". Ou outro agente da equipe (ask-agent --team)."
        ))
    }
}

// ---------- Formatos JSON ----------

/// Corpo do `POST /usage`: `{"session":{"used":0.34,"resets_at_ms":…}|null,"weekly":…}`, com
/// `"groups":[{"name":…,"session":…,"weekly":…}]` quando há grupos.
pub fn usage_to_json(u: &AgentUsage) -> Value {
    let w = |w: &Option<UsageWindow>| w.map_or(Value::Null, |w| json!({ "used": w.used, "resets_at_ms": w.resets_at_ms }));
    let mut v = json!({ "session": w(&u.session), "weekly": w(&u.weekly) });
    if !u.groups.is_empty() {
        let groups: Vec<Value> =
            u.groups.iter().map(|g| json!({ "name": g.name, "session": w(&g.session), "weekly": w(&g.weekly) })).collect();
        v["groups"] = Value::Array(groups);
    }
    v
}

/// A janela `key` de `v` (`None` se ausente ou nula). Recusa `used` fora de 0..=1.
fn window_from_json(v: &Value, key: &str) -> Result<Option<UsageWindow>, String> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(w) => {
            let used = w.get("used").and_then(Value::as_f64).filter(|u| (0.0..=1.0).contains(u));
            let used = used.ok_or_else(|| format!("{key}.used deve ser um número de 0 a 1"))?;
            Ok(Some(UsageWindow { used: used as f32, resets_at_ms: w.get("resets_at_ms").and_then(Value::as_u64) }))
        }
    }
}

/// Lê o corpo do `POST /usage` (`updated_ms` fica 0). Recusa `used` fora de 0..=1; um grupo inválido é pulado.
pub fn usage_from_json(v: &Value) -> Result<AgentUsage, String> {
    let groups = v
        .get("groups")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|g| {
            let name = g.get("name").and_then(Value::as_str)?.to_owned();
            Some(UsageGroup { name, session: window_from_json(g, "session").ok()?, weekly: window_from_json(g, "weekly").ok()? })
        })
        .collect();
    Ok(AgentUsage { session: window_from_json(v, "session")?, weekly: window_from_json(v, "weekly")?, groups, ..Default::default() })
}

/// O retrato guardado em `usage.json`: `{"claude":{"session":…,"weekly":…,"updated_ms":…}}`. Os locais não vão
/// (não têm cota; voltam quando o poller os vê).
fn store_to_json(map: &HashMap<String, AgentUsage>) -> Value {
    let mut out = serde_json::Map::new();
    for (model, u) in map.iter().filter(|(_, u)| !u.local) {
        let mut v = usage_to_json(u);
        v["updated_ms"] = json!(u.updated_ms);
        out.insert(model.clone(), v);
    }
    Value::Object(out)
}

/// Lê o `usage.json`; uma entrada inválida é pulada, não derruba as outras.
fn store_from_json(v: &Value) -> HashMap<String, AgentUsage> {
    let Some(obj) = v.as_object() else { return HashMap::new() };
    obj.iter()
        .filter_map(|(model, e)| {
            let u = usage_from_json(e).ok()?;
            Some((model.clone(), AgentUsage { updated_ms: e.get("updated_ms").and_then(Value::as_u64).unwrap_or(0), ..u }))
        })
        .collect()
}

/// Uso a partir do JSON que o Claude Code passa à statusline:
/// `rate_limits.{five_hour,seven_day}.{used_percentage (0..100), resets_at (s Unix)}`.
/// Com chave de API (`rate_limits` nulo) ou num formato desconhecido, vem vazio.
pub fn from_claude_statusline(v: &Value) -> AgentUsage {
    let window = |key: &str| {
        let w = v.get("rate_limits")?.get(key)?;
        let pct = w.get("used_percentage").and_then(Value::as_f64).filter(|p| p.is_finite())?;
        Some(UsageWindow { used: (pct / 100.0).clamp(0.0, 1.0) as f32, resets_at_ms: w.get("resets_at").and_then(epoch_ms) })
    };
    AgentUsage { session: window("five_hour"), weekly: window("seven_day"), ..Default::default() }
}

/// Instante em ms Unix: número em segundos (ou já em ms, se grande demais para segundos) ou
/// texto RFC 3339.
pub(crate) fn epoch_ms(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => {
            let n = n.as_f64().filter(|n| n.is_finite() && *n > 0.0)?;
            // 1e11 s fica no ano 5138: acima disso, já são milissegundos.
            Some(if n >= 1e11 { n as u64 } else { (n * 1000.0) as u64 })
        }
        Value::String(s) => rfc3339_ms(s),
        _ => None,
    }
}

/// `2025-09-18T12:34:56.789Z` (ou com `±hh:mm`) → ms Unix.
pub(crate) fn rfc3339_ms(s: &str) -> Option<u64> {
    let s = s.trim();
    // Só dígitos ASCII: `parse` aceitaria sinais (`+5`, `-5`) e produziria datas erradas.
    let num = |a: usize, b: usize| s.get(a..b).filter(|t| t.bytes().all(|c| c.is_ascii_digit())).and_then(|t| t.parse::<i64>().ok());
    if s.len() < 19 || !matches!(s.as_bytes()[10], b'T' | b't' | b' ') || !matches!((s.as_bytes()[4], s.as_bytes()[7]), (b'-', b'-')) {
        return None;
    }
    let (y, mo, d, h, mi, sec) = (num(0, 4)?, num(5, 7)?, num(8, 10)?, num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let mut rest = &s[19..];
    let mut frac_ms = 0i64;
    if let Some(r) = rest.strip_prefix('.') {
        let digits = r.find(|c: char| !c.is_ascii_digit()).unwrap_or(r.len());
        let d3: String = r[..digits].chars().chain("000".chars()).take(3).collect();
        frac_ms = d3.parse().ok()?;
        rest = &r[digits..];
    }
    let offset_min = match rest {
        "Z" | "z" | "" => 0,
        _ => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            // Exatamente `±hh:mm`.
            let b = rest.as_bytes();
            if b.len() != 6 || b[3] != b':' || ![1, 2, 4, 5].iter().all(|&i| b[i].is_ascii_digit()) {
                return None;
            }
            let hh = i64::from(b[1] - b'0') * 10 + i64::from(b[2] - b'0');
            let mm = i64::from(b[4] - b'0') * 10 + i64::from(b[5] - b'0');
            if hh > 23 || mm > 59 {
                return None;
            }
            sign * (hh * 60 + mm)
        }
    };
    // Dias desde 1970-01-01 (algoritmo `days_from_civil` de Howard Hinnant).
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + h * 3600 + mi * 60 + sec - offset_min * 60;
    u64::try_from(secs * 1000 + frac_ms).ok()
}

/// Thread `cd-usage`: mantém atualizado o uso de quem tem aba aberta ou o CLI instalado.
/// O Claude não entra aqui: ele avisa sozinho pela statusline (`POST /usage`).
pub(crate) fn start_poller(core: &Core) {
    let weak = Arc::downgrade(&core.inner);
    let _ = std::thread::Builder::new().name("cd-usage".into()).spawn(move || {
        let mut last: HashMap<&'static str, Instant> = HashMap::new();
        let mut installed: HashMap<&'static str, bool> = HashMap::new();
        let mut installed_at: Option<Instant> = None;
        let mut agy_version: Option<String> = None;
        let mut agy_version_at: Option<Instant> = None;
        loop {
            std::thread::sleep(Duration::from_secs(2));
            let Some(inner) = weak.upgrade() else { return };
            let core = Core { inner };
            if core.inner.state.shutdown.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            let open: Vec<String> = core.instance_ids();
            let has = |model: &str| open.iter().any(|id| id == model || id.starts_with(&format!("{model}@")));
            if installed_at.is_none_or(|t| t.elapsed() >= INSTALLED_EVERY) {
                for id in ["codex", "antigravity"] {
                    installed.insert(id, core.inner.agents.resolve(id).is_some());
                }
                installed_at = Some(Instant::now());
            }
            let wanted = |model: &str| has(model) || installed.get(model).copied().unwrap_or(false);
            let due = |last: &HashMap<&str, Instant>, key: &str, every: Duration| last.get(key).is_none_or(|t| t.elapsed() >= every);
            // Marca a leitura: depois de uma falha (sem login, rede fora, rollout sem dados) com a aba aberta volta
            // em `RETRY_EVERY`; sem aba (só instalado), no intervalo normal.
            let mark = |last: &mut HashMap<&'static str, Instant>, key: &'static str, every: Duration, ok: bool| {
                let at = if ok {
                    Instant::now()
                } else {
                    Instant::now().checked_sub(every.saturating_sub(RETRY_EVERY)).unwrap_or_else(Instant::now)
                };
                last.insert(key, at);
            };
            if wanted("codex") && due(&last, "codex", CODEX_EVERY) {
                let usage = codex::read(&core.inner.home);
                mark(&mut last, "codex", CODEX_EVERY, usage.is_some() || !has("codex"));
                if let Some(u) = usage {
                    core.set_usage("codex", u);
                }
            }
            if wanted("antigravity") && due(&last, "antigravity", ANTIGRAVITY_EVERY) {
                // O CLI se atualiza sozinho: a versão vai no pedido da cota (ver `antigravity`). Só se pergunta ao
                // `agy` quando a cota vai ser lida (rodá-lo pode levar segundos e trava esta thread).
                if agy_version_at.is_none_or(|t| t.elapsed() >= INSTALLED_EVERY) {
                    agy_version = core.inner.agents.resolve("antigravity").and_then(|r| antigravity::installed_version(&r));
                    agy_version_at = Some(Instant::now());
                }
                let usage = antigravity::fetch(&core.inner.home, agy_version.as_deref());
                mark(&mut last, "antigravity", ANTIGRAVITY_EVERY, usage.is_some() || !has("antigravity"));
                if let Some(u) = usage {
                    core.set_usage("antigravity", u);
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_visible_changes_count() {
        let store = UsageStore::default();
        let u = |used, at| AgentUsage { session: Some(UsageWindow { used, resets_at_ms: None }), updated_ms: at, ..Default::default() };
        assert!(store.set("claude", u(0.3, 1)));
        assert!(!store.set("claude", u(0.3, 2)));
        assert!(store.set("claude", u(0.4, 3)));
        assert_eq!(store.snapshot()["claude"].updated_ms, 3);
        // Uma cota de grupo que muda também conta.
        let group =
            UsageGroup { name: "Gemini 4 Argon".into(), session: None, weekly: Some(UsageWindow { used: 0.5, resets_at_ms: None }) };
        assert!(store.set("claude", AgentUsage { groups: vec![group], ..u(0.4, 4) }));
    }

    #[test]
    fn the_last_usage_survives_a_restart_and_a_bad_file_starts_empty() {
        let tmp = tempfile::tempdir().expect("tmp");
        let file = tmp.path().join(USAGE_FILE);
        let store = UsageStore::load(file.clone());
        let claude = AgentUsage {
            session: Some(UsageWindow { used: 0.25, resets_at_ms: Some(9_000) }),
            weekly: Some(UsageWindow { used: 0.5, resets_at_ms: None }),
            updated_ms: 1_234,
            local: false,
            groups: vec![UsageGroup {
                name: "Gemini 4 Argon".into(),
                session: None,
                weekly: Some(UsageWindow { used: 0.9, resets_at_ms: Some(7) }),
            }],
        };
        assert!(store.set("claude", claude.clone()));
        // Local não vai para o disco (não tem cota).
        assert!(store.set("local", AgentUsage { local: true, updated_ms: 1, ..Default::default() }));
        let again = UsageStore::load(file.clone());
        assert_eq!(again.snapshot(), HashMap::from([("claude".to_owned(), claude)]));
        // Arquivo corrompido: começa vazio; entrada inválida é pulada.
        std::fs::write(&file, "{ nao e json").expect("write");
        assert!(UsageStore::load(file.clone()).snapshot().is_empty());
        let mixed = r#"{"codex":{"session":{"used":7}},"claude":{"session":null,"weekly":{"used":0.1},"updated_ms":5}}"#;
        std::fs::write(&file, mixed).expect("write");
        let partial = UsageStore::load(file).snapshot();
        assert_eq!(partial.len(), 1);
        assert_eq!(partial["claude"].updated_ms, 5);
    }

    #[test]
    fn each_model_counts_against_its_own_group() {
        // A cota real do Antigravity em 24/10: Argon esgotado na semana, Flash quase intacto, Claude/GPT em 95%.
        const NOW: u64 = 1_000_000_000;
        let w = |used, h: u64| Some(UsageWindow { used, resets_at_ms: Some(NOW + h * 3_600_000) });
        let group = |name: &str, session, weekly| UsageGroup { name: name.into(), session, weekly };
        let agy = AgentUsage {
            session: w(0.0, 5),
            weekly: w(1.0, 9),
            updated_ms: NOW,
            local: false,
            groups: vec![
                group("Gemini Models", w(0.0, 5), w(0.004, 34)),
                group("Gemini 4 Argon", None, w(1.0, 9)),
                group("Claude and GPT models", w(0.39, 4), w(0.95, 140)),
            ],
        };
        let argon = model_quota(&agy, Some("gemini-4-argon-high"), NOW).expect("argon");
        assert!(argon.exhausted());
        assert_eq!(argon.note(NOW), "cota de Gemini 4 Argon esgotada, renova em 9 h");
        let flash = model_quota(&agy, Some("gemini-3.8-flash-high"), NOW).expect("flash");
        assert_eq!(
            (flash.group.as_deref(), flash.note(NOW).as_str()),
            (Some("Gemini Models"), "cota de Gemini Models 0% usada, renova em 34 h")
        );
        let sonnet = model_quota(&agy, Some("claude-sonnet-5-5-high"), NOW).expect("sonnet");
        assert_eq!(sonnet.note(NOW), "cota de Claude and GPT models 95% usada, renova em 5 dias");
        // Sem modelo, o padrão do CLI pode ser qualquer grupo: não se sabe.
        assert_eq!(model_quota(&agy, None, NOW), None);
        // Renovada desde a leitura: deixa de contar.
        assert!(!model_quota(&agy, Some("gemini-4-argon-low"), NOW + 10 * 3_600_000).is_some_and(|q| q.exhausted()));
        assert_eq!(
            quota_line(&agy, NOW).as_deref(),
            Some(
                "Gemini Models 0% usada (renova em 34 h), Gemini 4 Argon ESGOTADA (renova em 9 h), Claude and GPT models 95% usada (renova em 5 dias)"
            )
        );
        // Abaixo de 80% também mostra quanto foi usado e quando renova.
        let renewed = AgentUsage { groups: vec![group("Gemini 4 Argon", None, w(0.04, 6 * 24))], updated_ms: NOW, ..Default::default() };
        assert_eq!(quota_line(&renewed, NOW).as_deref(), Some("Gemini 4 Argon 4% usada (renova em 6 dias)"));
        assert_eq!(agent_used(&renewed, NOW), Some(0.04));
        // Na ordenação do Antigravity valem os grupos do Gemini, não o de Claude/GPT.
        let agy_renewed = AgentUsage {
            groups: vec![
                group("Gemini Models", w(0.0, 5), w(0.004, 34)),
                group("Gemini 4 Argon", None, w(0.04, 6 * 24)),
                group("Claude and GPT models", w(0.39, 4), w(0.95, 140)),
            ],
            updated_ms: NOW,
            ..Default::default()
        };
        assert_eq!(agent_used(&agy_renewed, NOW), Some(0.04));
        // Sem leitura viva: diz quando foi a última ou "sem leitura".
        assert_eq!(no_reading_text(None, NOW), "sem leitura");
        assert_eq!(no_reading_text(Some(&AgentUsage::default()), NOW), "sem leitura");
        assert_eq!(no_reading_text(Some(&agy), NOW + 15 * 60_000), "última leitura há 15 min");
        // Sem grupos (o Claude): vale o plano, a janela mais gasta.
        let claude = AgentUsage { session: w(0.2, 1), weekly: w(0.5, 30), ..Default::default() };
        let q = model_quota(&claude, Some("sonnet"), NOW).expect("claude");
        assert_eq!((q.group.clone(), q.used, q.resets_at_ms), (None, 0.5, Some(NOW + 30 * 3_600_000)));
        assert_eq!(q.note(NOW), "cota 50% usada, renova em 30 h");
        assert_eq!(agent_used(&claude, NOW), Some(0.5));
        assert_eq!(duration_text(45 * 60_000), "45 min");
        assert_eq!(duration_text(125 * 60_000), "2 h 5 min");
    }

    #[test]
    fn claude_statusline_rate_limits() {
        let v = json!({ "model": {}, "rate_limits": {
            "five_hour": { "used_percentage": 34.0, "resets_at": 1_760_000_000 },
            "seven_day": { "used_percentage": 120.0, "resets_at": null }
        }});
        let u = from_claude_statusline(&v);
        assert_eq!(u.session, Some(UsageWindow { used: 0.34, resets_at_ms: Some(1_760_000_000_000) }));
        assert_eq!(u.weekly, Some(UsageWindow { used: 1.0, resets_at_ms: None }));
        // Chave de API: `rate_limits` nulo; ou só uma das janelas.
        assert_eq!(from_claude_statusline(&json!({ "rate_limits": null })), AgentUsage::default());
        let u = from_claude_statusline(&json!({ "rate_limits": { "five_hour": { "used_percentage": 5 } } }));
        assert!(u.session.is_some() && u.weekly.is_none());
    }

    #[test]
    fn bridge_body_round_trip_and_validation() {
        let u = AgentUsage { session: Some(UsageWindow { used: 0.5, resets_at_ms: Some(42) }), weekly: None, ..Default::default() };
        assert_eq!(usage_from_json(&usage_to_json(&u)), Ok(u.clone()));
        let with_groups = AgentUsage {
            groups: vec![UsageGroup {
                name: "Gemini Models".into(),
                session: Some(UsageWindow { used: 0.25, resets_at_ms: None }),
                weekly: None,
            }],
            ..u
        };
        assert_eq!(usage_from_json(&usage_to_json(&with_groups)), Ok(with_groups));
        // Grupo inválido é pulado; o resto vale.
        let bad_group = json!({ "groups": [{ "name": "x", "session": { "used": 3 } }, { "session": null }] });
        assert_eq!(usage_from_json(&bad_group).map(|u| u.groups.len()), Ok(0));
        assert!(usage_from_json(&json!({ "session": { "used": 1.5 } })).is_err());
        assert!(usage_from_json(&json!({ "weekly": { "used": "x" } })).is_err());
        assert_eq!(usage_from_json(&json!({})), Ok(AgentUsage::default()));
    }

    #[test]
    fn timestamps() {
        assert_eq!(rfc3339_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(rfc3339_ms("2025-09-18T12:34:56.789Z"), Some(1_758_198_896_789));
        assert_eq!(rfc3339_ms("2025-09-18T09:34:56.789-03:00"), Some(1_758_198_896_789));
        assert_eq!(rfc3339_ms("2024-02-29T00:00:00.5Z"), Some(1_709_164_800_500));
        assert_eq!(rfc3339_ms("ontem"), None);
        // Sinais e lixo nos campos ou no fuso não passam por número.
        for bad in [
            "2025-09-18T-5:00:00Z",
            "2025-09-18T+5:00:00Z",
            "2025-09-18T12:+0:00Z",
            "+025-09-18T12:00:00Z",
            "2025-09-18T12:00:00+03x00lixo",
            "2025-09-18T12:00:00+0300",
            "2025-09-18T12:00:00+03:00:00",
            "2025-09-18T12:00:00+3:00",
            "2025-09-18T12:00:00+24:00",
            "2025-09-18T12:00:00+03:60",
            "2025/09/18T12:00:00Z",
        ] {
            assert_eq!(rfc3339_ms(bad), None, "{bad}");
        }
        assert_eq!(epoch_ms(&json!(1_758_198_896)), Some(1_758_198_896_000));
        assert_eq!(epoch_ms(&json!(1_758_198_896_789u64)), Some(1_758_198_896_789));
        assert_eq!(epoch_ms(&json!("2025-09-18T12:34:56.789Z")), Some(1_758_198_896_789));
    }
}
