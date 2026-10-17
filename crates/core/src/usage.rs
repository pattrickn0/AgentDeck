//! Uso dos planos dos agentes (limite da sessão e da semana), mostrado na barra de título.
//!
//! Fontes (cada uma preenche [`UsageStore`] com [`Core::set_usage`]):
//! - **Claude**: a statusline do Claude Code (`ask-agent --usage`, rota `POST /usage` da ponte);
//! - **Codex**: o último `rate_limits` dos arquivos `~/.codex/sessions/**/rollout-*.jsonl`;
//! - **Antigravity**: a cota consultada pela API que o próprio CLI usa;
//! - **Ollama**: local, sem limite.
//!
//! CONTRATO ESTÁVEL: `UsageWindow`, `AgentUsage`, `Core::usage_snapshot` e `Core::set_usage`
//! são consumidos pela UI.

mod antigravity;
mod codex;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::{json, Value};

use crate::{util, Core, CoreEvent};

/// De quanto em quanto tempo o Codex é relido (arquivo local, barato).
const CODEX_EVERY: Duration = Duration::from_secs(60);
/// De quanto em quanto tempo a cota do Antigravity é consultada (rede).
const ANTIGRAVITY_EVERY: Duration = Duration::from_secs(180);
/// Intervalo curto de nova tentativa quando uma leitura/consulta de uso falha (`None`).
const RETRY_EVERY: Duration = Duration::from_secs(15);

/// Uma janela de limite (sessão de 5 h, semana…).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UsageWindow {
    /// Fração usada, de 0.0 a 1.0.
    pub used: f32,
    /// Quando a janela renova (ms desde a época Unix), se a fonte disser.
    pub resets_at_ms: Option<u64>,
}

/// Uso de um modelo de agente (`claude`, `codex`, `antigravity`, `ollama`…).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AgentUsage {
    /// Limite da sessão (o círculo que enche).
    pub session: Option<UsageWindow>,
    /// Limite semanal (o anel externo; vermelho quando acaba).
    pub weekly: Option<UsageWindow>,
    /// Quando o dado foi lido (ms desde a época Unix).
    pub updated_ms: u64,
    /// Agente local, sem limite (Ollama).
    pub local: bool,
}

/// Último uso conhecido de cada modelo.
#[derive(Default)]
pub struct UsageStore {
    map: Mutex<HashMap<String, AgentUsage>>,
}

impl UsageStore {
    /// Grava e diz se mudou.
    fn set(&self, model: &str, usage: AgentUsage) -> bool {
        let mut map = self.map.lock();
        let old = map.get(model);
        // `updated_ms` sozinho não é mudança visível o bastante para acordar a UI.
        let changed = old.is_none_or(|o| o.session != usage.session || o.weekly != usage.weekly || o.local != usage.local);
        map.insert(model.to_owned(), usage);
        changed
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

// ---------- Formatos JSON ----------

/// Corpo do `POST /usage`: `{"session":{"used":0.34,"resets_at_ms":…}|null,"weekly":…}`.
pub fn usage_to_json(u: &AgentUsage) -> Value {
    let w = |w: &Option<UsageWindow>| w.map_or(Value::Null, |w| json!({ "used": w.used, "resets_at_ms": w.resets_at_ms }));
    json!({ "session": w(&u.session), "weekly": w(&u.weekly) })
}

/// Lê o corpo do `POST /usage` (`updated_ms` fica 0). Recusa `used` fora de 0..=1.
pub fn usage_from_json(v: &Value) -> Result<AgentUsage, String> {
    let window = |key: &str| -> Result<Option<UsageWindow>, String> {
        match v.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(w) => {
                let used = w.get("used").and_then(Value::as_f64).filter(|u| (0.0..=1.0).contains(u));
                let used = used.ok_or_else(|| format!("{key}.used deve ser um número de 0 a 1"))?;
                Ok(Some(UsageWindow { used: used as f32, resets_at_ms: w.get("resets_at_ms").and_then(Value::as_u64) }))
            }
        }
    };
    Ok(AgentUsage { session: window("session")?, weekly: window("weekly")?, ..Default::default() })
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
    let num = |a: usize, b: usize| s.get(a..b).and_then(|t| t.parse::<i64>().ok());
    if s.len() < 19 || !matches!(s.as_bytes()[10], b'T' | b't' | b' ') {
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
            let hh: i64 = rest.get(1..3)?.parse().ok()?;
            let mm: i64 = rest.get(4..6)?.parse().ok()?;
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

/// Thread `cd-usage`: enquanto houver aba de um modelo, mantém o uso dele atualizado.
/// O Claude não entra aqui: ele avisa sozinho pela statusline (`POST /usage`).
pub(crate) fn start_poller(core: &Core) {
    let weak = Arc::downgrade(&core.inner);
    let _ = std::thread::Builder::new().name("cd-usage".into()).spawn(move || {
        let mut last: HashMap<&'static str, Instant> = HashMap::new();
        loop {
            std::thread::sleep(Duration::from_secs(2));
            let Some(inner) = weak.upgrade() else { return };
            let core = Core { inner };
            if core.inner.state.shutdown.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            let open: Vec<String> = core.instance_ids();
            let has = |model: &str| {
                open.iter().any(|id| id == model || id.starts_with(&format!("{model}@")) || id.starts_with(&format!("{model}:")))
            };
            let due = |last: &HashMap<&str, Instant>, key: &str, every: Duration| last.get(key).is_none_or(|t| t.elapsed() >= every);
            if has("ollama") && !core.inner.usage.map.lock().contains_key("ollama") {
                core.set_usage("ollama", AgentUsage { local: true, updated_ms: util::now_ms(), ..Default::default() });
            }
            if has("codex") && due(&last, "codex", CODEX_EVERY) {
                last.insert("codex", Instant::now());
                if let Some(u) = codex::read(&core.inner.home) {
                    core.set_usage("codex", u);
                }
            }
            if has("antigravity") && due(&last, "antigravity", ANTIGRAVITY_EVERY) {
                last.insert("antigravity", Instant::now());
                if let Some(u) = antigravity::fetch(&core.inner.home) {
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
        assert_eq!(usage_from_json(&usage_to_json(&u)), Ok(u));
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
        assert_eq!(epoch_ms(&json!(1_758_198_896)), Some(1_758_198_896_000));
        assert_eq!(epoch_ms(&json!(1_758_198_896_789u64)), Some(1_758_198_896_789));
        assert_eq!(epoch_ms(&json!("2025-09-18T12:34:56.789Z")), Some(1_758_198_896_789));
    }
}
