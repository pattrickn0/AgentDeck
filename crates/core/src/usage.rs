//! Uso dos planos dos agentes (limite da sessão e da semana), mostrado na barra de título.
//!
//! Fontes (cada uma preenche [`UsageStore`] com [`Core::set_usage`]):
//! - **Claude**: a statusline do Claude Code (`ask-agent --usage`, rota `POST /usage` da ponte);
//! - **Codex**: o último `rate_limits` dos arquivos `~/.codex/sessions/**/rollout-*.jsonl`;
//! - **Antigravity**: a cota consultada pela API que o próprio CLI usa;
//! - **Ollama**: local, sem limite.
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
            let changed = old.is_none_or(|o| o.session != usage.session || o.weekly != usage.weekly || o.local != usage.local);
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
            if installed_at.is_none_or(|t| t.elapsed() >= INSTALLED_EVERY) {
                for id in ["codex", "antigravity", "ollama"] {
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
            if wanted("ollama") && !core.inner.usage.map.lock().contains_key("ollama") {
                core.set_usage("ollama", AgentUsage { local: true, updated_ms: util::now_ms(), ..Default::default() });
            }
            if wanted("codex") && due(&last, "codex", CODEX_EVERY) {
                let usage = codex::read(&core.inner.home);
                mark(&mut last, "codex", CODEX_EVERY, usage.is_some() || !has("codex"));
                if let Some(u) = usage {
                    core.set_usage("codex", u);
                }
            }
            if wanted("antigravity") && due(&last, "antigravity", ANTIGRAVITY_EVERY) {
                let usage = antigravity::fetch(&core.inner.home);
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
        };
        assert!(store.set("claude", claude.clone()));
        // Local não vai para o disco (não tem cota).
        assert!(store.set("ollama", AgentUsage { local: true, updated_ms: 1, ..Default::default() }));
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
