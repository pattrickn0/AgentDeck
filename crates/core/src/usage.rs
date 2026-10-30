//! Agent plan usage (session and weekly limit), shown in the title bar.
//!
//! Sources (each fills [`UsageStore`] with [`Core::set_usage`]):
//! - **Claude**: Claude Code statusline (`ask-agent --usage`, bridge route `POST /usage`);
//! - **Codex**: latest `rate_limits` from `~/.codex/sessions/**/rollout-*.jsonl` files;
//! - **Antigravity**: quota queried via the API that the CLI itself uses;
//!
//! Codex and Antigravity are queried when an open tab exists for them **or** the CLI is installed, and the latest usage
//! of each is kept in `data/usage.json`: the bar shows all agents with quota, even without an open window (Claude's
//! is the latest sent by statusline).
//!
//! STABLE CONTRACT: `UsageWindow`, `AgentUsage`, `Core::usage_snapshot` and `Core::set_usage`
//! are consumed by the UI.

mod antigravity;
mod codex;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::{json, Value};

use crate::{util, Core, CoreEvent};

/// How often Codex is re-read (local file, cheap).
const CODEX_EVERY: Duration = Duration::from_secs(60);
/// How often Antigravity quota is queried (network).
const ANTIGRAVITY_EVERY: Duration = Duration::from_secs(180);
/// Short retry interval when a usage read/query fails (`None`) with open tab.
const RETRY_EVERY: Duration = Duration::from_secs(20);
/// How often installed CLIs are checked.
const INSTALLED_EVERY: Duration = Duration::from_secs(600);
/// The file with latest known usage, in the data directory.
pub const USAGE_FILE: &str = "usage.json";

/// A limit window (5 h session, weekly...).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UsageWindow {
    /// Fraction used, from 0.0 to 1.0.
    pub used: f32,
    /// When the window renews (ms since Unix epoch), if reported by source.
    pub resets_at_ms: Option<u64>,
}

/// Usage of an agent model (`claude`, `codex`, `antigravity`...).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AgentUsage {
    /// Session limit (the circle that fills up).
    pub session: Option<UsageWindow>,
    /// Weekly limit (the outer ring; red when exhausted).
    pub weekly: Option<UsageWindow>,
    /// When the data was read (ms since Unix epoch).
    pub updated_ms: u64,
    /// Local agent, without limit (no quota to show).
    pub local: bool,
    /// Quotas of each model group, when source separates them (Antigravity: "Gemini Models",
    /// "Gemini 4 Argon"...); empty in others. `session` and `weekly` are the summary shown by the bar.
    pub groups: Vec<UsageGroup>,
}

/// Quota of a model group within the plan.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageGroup {
    pub name: String,
    pub session: Option<UsageWindow>,
    pub weekly: Option<UsageWindow>,
}

/// Latest known usage of each model; with `file`, saved to disk on every change.
#[derive(Default)]
pub struct UsageStore {
    map: Mutex<HashMap<String, AgentUsage>>,
    file: Option<PathBuf>,
    /// One save at a time, outside the `map` lock (which UI acquires every frame); records when the last one occurred.
    disk: Mutex<u64>,
}

/// New reading without percentage change: "read ago" goes to disk at most every so often.
const SAVE_READ_EVERY_MS: u64 = 5 * 60 * 1000;

impl UsageStore {
    /// Starts with what `file` stored (empty if it does not exist or cannot be read).
    pub(crate) fn load(file: PathBuf) -> Self {
        let map = std::fs::read(&file)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .map(|v| store_from_json(&v))
            .unwrap_or_default();
        Self { map: Mutex::new(map), file: Some(file), disk: Mutex::new(0) }
    }

    /// Saves and reports whether visible data changed. A reading that only refreshes `updated_ms` also goes to disk,
    /// but at most every [`SAVE_READ_EVERY_MS`] (otherwise, upon reopening the app, the panel would say incorrect "read ago").
    fn set(&self, model: &str, usage: AgentUsage) -> bool {
        let changed = {
            let mut map = self.map.lock();
            let old = map.get(model);
            // `updated_ms` alone is not a visible change enough to wake up the UI.
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
                // Snapshot is taken with the turn to save: latest save is always the newest.
                let text = store_to_json(&self.map.lock()).to_string();
                self.save(&text);
                *last = now;
            }
        }
        changed
    }

    /// Writes next to it and renames: a crash in between does not leave a half-written file.
    fn save(&self, text: &str) {
        let Some(file) = &self.file else { return };
        let tmp = file.with_extension("json.tmp");
        if let Err(e) = std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, file)) {
            log::warn!("could not save usage to {}: {e}", file.display());
        }
    }

    fn snapshot(&self) -> HashMap<String, AgentUsage> {
        self.map.lock().clone()
    }
}

impl Core {
    /// Snapshot of usage by model (cheap; UI calls every frame).
    pub fn usage_snapshot(&self) -> HashMap<String, AgentUsage> {
        self.inner.usage.snapshot()
    }

    /// Updates usage of a model (`claude`, `codex`, `antigravity`...) and notifies UI if changed.
    pub fn set_usage(&self, model: &str, usage: AgentUsage) {
        if self.inner.usage.set(model, usage) {
            self.emit(CoreEvent::UsageChanged);
        }
    }
}

// ---------- Quota of a model (--models, --team and /ask refusal) ----------

/// Usage threshold above which quota counts as exhausted (server rounds: 0.5% remaining cannot produce a response).
pub const EXHAUSTED: f32 = 0.995;
/// Reading older than this cannot be used to refuse a query (quota may have renewed or been purchased).
const FRESH_MS: u64 = 60 * 60 * 1000;

/// The quota applying to a model: that of its group, when source separates (Antigravity: Gemini, Argon,
/// Claude/GPT), or that of the entire plan.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelQuota {
    /// The group ("Gemini 4 Argon"); `None` = agent plan.
    pub group: Option<String>,
    /// Fraction used of the most consumed window, from 0.0 to 1.0.
    pub used: f32,
    /// When it renews (Unix ms), if reported by source.
    pub resets_at_ms: Option<u64>,
}

impl ModelQuota {
    pub fn exhausted(&self) -> bool {
        self.used >= EXHAUSTED
    }

    /// "Gemini 4 Argon quota exhausted, renews in 8 h 45 min", "Gemini 4 Argon quota 4% used, renews in 6 days".
    pub fn note(&self, now_ms: u64) -> String {
        let of = self.group.as_ref().map(|g| format!(" for {g}")).unwrap_or_default();
        let renews = self.resets_at_ms.filter(|&r| r > now_ms).map(|r| format!(", renews in {}", duration_text(r - now_ms)));
        if self.exhausted() {
            format!("quota{of} exhausted{}", renews.unwrap_or_default())
        } else {
            format!("quota{of} {:.0}% used{}", self.used * 100.0, renews.unwrap_or_default())
        }
    }
}

/// "45 min", "8 h 45 min", "3 days".
fn duration_text(ms: u64) -> String {
    let min = ms.div_ceil(60_000);
    match (min / 60, min % 60) {
        (0, m) => format!("{m} min"),
        (h, _) if h >= 48 => format!("{} days", h / 24),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// Antigravity model quota group: Argon has its own, other Gemini share "Gemini Models", and Claude
/// and GPT share "Claude and GPT models".
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

/// Quota of `model` in `u`, ignoring already renewed windows. With groups, only that of model's group
/// (without model or without its group, `None`: CLI default can be any).
pub fn model_quota(u: &AgentUsage, model: Option<&str>, now_ms: u64) -> Option<ModelQuota> {
    let (group, windows) = if u.groups.is_empty() {
        (None, [u.session, u.weekly])
    } else {
        let g = group_of(&u.groups, model?)?;
        (Some(g.name.clone()), [g.session, g.weekly])
    };
    let live: Vec<UsageWindow> = windows.into_iter().flatten().filter(|w| w.resets_at_ms.is_none_or(|r| r > now_ms)).collect();
    let used = live.iter().map(|w| w.used).fold(None, |a: Option<f32>, u| Some(a.map_or(u, |a| a.max(u))))?;
    // Exhausted, takes when the latest exhausted window renews; otherwise, that of the most consumed.
    let resets_at_ms = if used >= EXHAUSTED {
        live.iter().filter(|w| w.used >= EXHAUSTED).filter_map(|w| w.resets_at_ms).max()
    } else {
        live.iter().find(|w| w.used == used).and_then(|w| w.resets_at_ms)
    };
    Some(ModelQuota { group, used, resets_at_ms })
}

/// The fraction used that applies to a model, and where it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelUsage {
    pub used: f32,
    /// The model's group ("Gemini 4 Argon"); `None` = the agent's plan.
    pub group: Option<String>,
    /// The source separates groups but this model (or none) maps to none: `used` is the agent number, not the model's.
    pub fallback: bool,
}

/// Usage that applies to `model`: its group's when the source has groups, else the plan's. A model with no group
/// (unknown, or the CLI default) falls back to the agent number, flagged by `fallback`.
pub fn model_usage(u: &AgentUsage, model: Option<&str>, now_ms: u64) -> Option<ModelUsage> {
    if let Some(q) = model_quota(u, model, now_ms) {
        return Some(ModelUsage { used: q.used, group: q.group, fallback: false });
    }
    if u.groups.is_empty() || model.is_some_and(|m| group_of(&u.groups, m).is_some()) {
        return None;
    }
    Some(ModelUsage { used: agent_used(u, now_ms)?, group: None, fallback: true })
}

/// Quotas of `u` in a line for `--models`: "Gemini Models 0% used (renews in 5 h), Gemini 4 Argon 4% used (renews in 6 days)…".
pub fn quota_line(u: &AgentUsage, now_ms: u64) -> Option<String> {
    let one = |name: Option<&str>, windows: [Option<UsageWindow>; 2]| {
        let q = model_quota(&AgentUsage { session: windows[0], weekly: windows[1], ..Default::default() }, None, now_ms)?;
        let name = name.map(|n| format!("{n} ")).unwrap_or_default();
        let renews = q.resets_at_ms.filter(|&r| r > now_ms).map(|r| format!(" (renews in {})", duration_text(r - now_ms)));
        Some(if q.exhausted() {
            format!("{name}EXHAUSTED{}", renews.unwrap_or_default())
        } else {
            format!("{name}{:.0}% used{}", q.used * 100.0, renews.unwrap_or_default())
        })
    };
    let parts: Vec<String> = if u.groups.is_empty() {
        one(None, [u.session, u.weekly]).into_iter().collect()
    } else {
        u.groups.iter().filter_map(|g| one(Some(&g.name), [g.session, g.weekly])).collect()
    };
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// What to show when one of the official CLIs has no live reading: when the last one was, or "no reading".
pub fn no_reading_text(u: Option<&AgentUsage>, now_ms: u64) -> String {
    match u.filter(|u| u.updated_ms > 0) {
        Some(u) => format!("last read {} ago", duration_text(now_ms.saturating_sub(u.updated_ms).max(60_000))),
        None => "no reading".to_owned(),
    }
}

/// The fraction used that counts for ordering agent in `--models` (lower = more headroom): without groups, the most
/// consumed window; with groups (Antigravity), the most consumed among Gemini groups (or among all, if no Gemini).
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

const OFFICIAL_AGENTS: [&str; 3] = ["claude", "antigravity", "codex"];

/// Candidate eligible for agent choice. Task capability can filter this list before selection.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentQuotaCandidate {
    pub kind: String,
    pub used: Option<f32>,
}

/// Choice and reason, ready to record in task or explain to user.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentQuotaChoice {
    pub kind: String,
    pub used: Option<f32>,
    pub reason: String,
}

/// Chooses candidate with lowest quota usage. Without readings, keeps received order; caller uses official order
/// to obtain predictable fallback. Pure function to allow filtering candidates by capability.
pub fn select_agent_by_quota(candidates: &[AgentQuotaCandidate]) -> Option<AgentQuotaChoice> {
    let candidate = candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| candidate.used.is_some())
        .min_by(|(a_index, a), (b_index, b)| a.used.unwrap().total_cmp(&b.used.unwrap()).then_with(|| a_index.cmp(b_index)))
        .map(|(_, candidate)| candidate)
        .or_else(|| candidates.first())?;
    let describe = |candidate: &AgentQuotaCandidate| match candidate.used {
        Some(used) => format!("{} {:.0}% used", candidate.kind, used * 100.0),
        None => format!("{} with no reading", candidate.kind),
    };
    let alternatives = candidates.iter().filter(|other| other.kind != candidate.kind).map(describe).collect::<Vec<_>>();
    let reason =
        if alternatives.is_empty() { describe(candidate) } else { format!("{} ({})", describe(candidate), alternatives.join(", ")) };
    Some(AgentQuotaChoice { kind: candidate.kind.clone(), used: candidate.used, reason })
}

/// Warns when chosen agent exceeded 80% and another available official CLI is below 50%.
pub fn quota_advisory(kind: &str, choices: &[String], used: &HashMap<String, f32>) -> Option<String> {
    let current = *used.get(kind)?;
    if current <= 0.80 {
        return None;
    }
    let alternative = OFFICIAL_AGENTS
        .iter()
        .copied()
        .filter(|other| *other != kind)
        .filter(|other| choices.iter().any(|choice| choice == other))
        .filter_map(|other| used.get(other).map(|value| (other, *value)))
        .filter(|(_, value)| *value < 0.50)
        .min_by(|a, b| a.1.total_cmp(&b.1))?;
    let current_pct = format!("{:.0}", current * 100.0);
    let alt_kind = alternative.0;
    let alt_pct = format!("{:.0}", alternative.1 * 100.0);
    Some(crate::t!("{kind} is at {current_pct}% quota; {alt_kind} is at {alt_pct}%", kind, current_pct, alt_kind, alt_pct,))
}

impl Core {
    /// Quota of `model` for agent `kind` (`claude`, `antigravity`...), by latest reading; `None` without data.
    pub fn model_quota(&self, kind: &str, model: Option<&str>) -> Option<ModelQuota> {
        let usage = self.inner.usage.snapshot();
        model_quota(usage.get(kind)?, model, util::now_ms())
    }

    /// Usage that applies to `model` of agent `kind` (its group's, or the agent number flagged as fallback).
    pub fn model_usage(&self, kind: &str, model: Option<&str>) -> Option<ModelUsage> {
        let usage = self.inner.usage.snapshot();
        model_usage(usage.get(kind)?, model, util::now_ms())
    }

    /// Quota of each `models` instance (id -> text), for `--team` and `--list`: percentage and renewal
    /// even below 80%, or "quota: no reading" / "quota: last read N min ago" on the official CLIs.
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
                    Some((id.clone(), format!("quota: {}", no_reading_text(usage.get(&kind), now))))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Quota line of each agent (agent -> line), for `--models`: always present on official CLIs
    /// (with live reading, "last read N min ago" or "no reading") and on others when there is a reading.
    pub fn quota_lines(&self) -> HashMap<String, String> {
        let now = util::now_ms();
        let usage = self.inner.usage.snapshot();
        let mut out: HashMap<String, String> = usage.iter().filter_map(|(kind, u)| Some((kind.clone(), quota_line(u, now)?))).collect();
        for kind in ["claude", "antigravity", "codex", "opencode"] {
            out.entry(kind.to_owned()).or_insert_with(|| no_reading_text(usage.get(kind), now));
        }
        out
    }

    /// Fraction used of each agent with live reading (agent -> 0..=1), to order `--models` by who has the most headroom.
    pub fn quota_used(&self) -> HashMap<String, f32> {
        let now = util::now_ms();
        self.inner.usage.snapshot().iter().filter_map(|(kind, u)| Some((kind.clone(), agent_used(u, now)?))).collect()
    }

    /// Best available official CLI and its used quota, when there is a reading.
    pub fn best_agent_by_quota(&self, choices: &[String]) -> Option<AgentQuotaChoice> {
        let used = self.quota_used();
        let candidates: Vec<AgentQuotaCandidate> = OFFICIAL_AGENTS
            .iter()
            .filter(|kind| choices.iter().any(|choice| choice == **kind))
            .map(|kind| AgentQuotaCandidate { kind: (*kind).to_owned(), used: used.get(*kind).copied() })
            .collect();
        select_agent_by_quota(&candidates)
    }

    /// Advisory without blocking above 80%, when another available official CLI is below 50%.
    pub fn quota_advisory(&self, kind: &str, choices: &[String]) -> Option<String> {
        quota_advisory(kind, choices, &self.quota_used())
    }

    /// Error for a question to `id` (agent `kind`, model `model`) when model quota is exhausted, per
    /// recent reading. Without data, with headroom, or with stale reading, `None`: question proceeds.
    pub(crate) fn quota_refusal(&self, id: &str, kind: &str, model: Option<&str>) -> Option<String> {
        let usage = self.inner.usage.snapshot();
        let u = usage.get(kind)?;
        let now = util::now_ms();
        if now.saturating_sub(u.updated_ms) > FRESH_MS {
            return None;
        }
        let q = model_quota(u, model, now).filter(ModelQuota::exhausted)?;
        let using = model.map(|m| format!(" uses {m} and its")).unwrap_or_else(|| ":".to_owned());
        let note = q.note(now);
        Some(format!(
            "{id}{using} {note}; message was not delivered. Choose a model with quota (ask-agent --models {kind} shows quota for \
             each group) and open another instance: ask-agent {id} --new --model <model> \"message\". Or another team agent (ask-agent --team)."
        ))
    }
}

// ---------- JSON Formats ----------

/// Body of `POST /usage`: `{"session":{"used":0.34,"resets_at_ms":…}|null,"weekly":…}`, with
/// `"groups":[{"name":…,"session":…,"weekly":…}]` when there are groups.
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

/// Window `key` of `v` (`None` if missing or null). Rejects `used` outside 0..=1.
fn window_from_json(v: &Value, key: &str) -> Result<Option<UsageWindow>, String> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(w) => {
            let used = w.get("used").and_then(Value::as_f64).filter(|u| (0.0..=1.0).contains(u));
            let used = used.ok_or_else(|| format!("{key}.used must be a number from 0 to 1"))?;
            Ok(Some(UsageWindow { used: used as f32, resets_at_ms: w.get("resets_at_ms").and_then(Value::as_u64) }))
        }
    }
}

/// Reads body of `POST /usage` (`updated_ms` stays 0). Rejects `used` outside 0..=1; an invalid group is skipped.
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

/// Snapshot stored in `usage.json`: `{"claude":{"session":…,"weekly":…,"updated_ms":…}}`. Local agents are omitted
/// (no quota; reappear when poller sees them).
fn store_to_json(map: &HashMap<String, AgentUsage>) -> Value {
    let mut out = serde_json::Map::new();
    for (model, u) in map.iter().filter(|(_, u)| !u.local) {
        let mut v = usage_to_json(u);
        v["updated_ms"] = json!(u.updated_ms);
        out.insert(model.clone(), v);
    }
    Value::Object(out)
}

/// Reads `usage.json`; an invalid entry is skipped without failing others.
fn store_from_json(v: &Value) -> HashMap<String, AgentUsage> {
    let Some(obj) = v.as_object() else { return HashMap::new() };
    obj.iter()
        .filter_map(|(model, e)| {
            let u = usage_from_json(e).ok()?;
            Some((model.clone(), AgentUsage { updated_ms: e.get("updated_ms").and_then(Value::as_u64).unwrap_or(0), ..u }))
        })
        .collect()
}

/// Usage from JSON that Claude Code passes to statusline:
/// `rate_limits.{five_hour,seven_day}.{used_percentage (0..100), resets_at (s Unix)}`.
/// With API key (null `rate_limits`) or in unknown format, returns empty.
pub fn from_claude_statusline(v: &Value) -> AgentUsage {
    let window = |key: &str| {
        let w = v.get("rate_limits")?.get(key)?;
        let pct = w.get("used_percentage").and_then(Value::as_f64).filter(|p| p.is_finite())?;
        Some(UsageWindow { used: (pct / 100.0).clamp(0.0, 1.0) as f32, resets_at_ms: w.get("resets_at").and_then(epoch_ms) })
    };
    AgentUsage { session: window("five_hour"), weekly: window("seven_day"), ..Default::default() }
}

/// Instant in Unix ms: number in seconds (or already in ms, if too large for seconds) or
/// RFC 3339 text.
pub(crate) fn epoch_ms(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => {
            let n = n.as_f64().filter(|n| n.is_finite() && *n > 0.0)?;
            // 1e11 s is in year 5138: above that, it is already milliseconds.
            Some(if n >= 1e11 { n as u64 } else { (n * 1000.0) as u64 })
        }
        Value::String(s) => rfc3339_ms(s),
        _ => None,
    }
}

/// `2025-09-18T12:34:56.789Z` (or with `±hh:mm`) -> Unix ms.
pub(crate) fn rfc3339_ms(s: &str) -> Option<u64> {
    let s = s.trim();
    // ASCII digits only: `parse` would accept signs (`+5`, `-5`) and produce incorrect dates.
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
            // Exactly `±hh:mm`.
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
    // Days since 1970-01-01 (Howard Hinnant's `days_from_civil` algorithm).
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + h * 3600 + mi * 60 + sec - offset_min * 60;
    u64::try_from(secs * 1000 + frac_ms).ok()
}

/// Thread `cd-usage`: keeps usage updated for agents with open tab or installed CLI.
/// Claude does not enter here: it reports on its own via statusline (`POST /usage`).
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
            // Marks the reading: after a failure (not logged in, offline, rollout without data) with open tab returns
            // in `RETRY_EVERY`; without tab (installed only), at normal interval.
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
                // The CLI updates itself: version goes in quota request (see `antigravity`). Only asks `agy`
                // when quota is about to be read (running it can take seconds and blocks this thread).
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
    use crate::i18n::{with_lang, Lang};

    #[test]
    fn only_visible_changes_count() {
        let store = UsageStore::default();
        let u = |used, at| AgentUsage { session: Some(UsageWindow { used, resets_at_ms: None }), updated_ms: at, ..Default::default() };
        assert!(store.set("claude", u(0.3, 1)));
        assert!(!store.set("claude", u(0.3, 2)));
        assert!(store.set("claude", u(0.4, 3)));
        assert_eq!(store.snapshot()["claude"].updated_ms, 3);
        // A group quota that changes also counts.
        let group =
            UsageGroup { name: "Gemini 4 Argon".into(), session: None, weekly: Some(UsageWindow { used: 0.5, resets_at_ms: None }) };
        assert!(store.set("claude", AgentUsage { groups: vec![group], ..u(0.4, 4) }));
    }

    #[test]
    fn quota_selection_chooses_the_least_used_candidate_and_lists_the_reason() {
        let candidates = [
            AgentQuotaCandidate { kind: "claude".into(), used: Some(0.74) },
            AgentQuotaCandidate { kind: "antigravity".into(), used: Some(0.45) },
            AgentQuotaCandidate { kind: "codex".into(), used: Some(0.0) },
        ];
        let choice = select_agent_by_quota(&candidates).expect("candidate");
        assert_eq!(choice.kind, "codex");
        assert_eq!(choice.reason, "codex 0% used (claude 74% used, antigravity 45% used)");
    }

    #[test]
    fn quota_selection_breaks_ties_by_candidate_order() {
        let candidates = [
            AgentQuotaCandidate { kind: "claude".into(), used: Some(0.25) },
            AgentQuotaCandidate { kind: "codex".into(), used: Some(0.25) },
        ];
        assert_eq!(select_agent_by_quota(&candidates).expect("candidate").kind, "claude");
    }

    #[test]
    fn quota_selection_uses_the_first_candidate_when_none_has_a_reading() {
        let candidates =
            [AgentQuotaCandidate { kind: "claude".into(), used: None }, AgentQuotaCandidate { kind: "codex".into(), used: None }];
        let choice = select_agent_by_quota(&candidates).expect("candidate");
        assert_eq!(choice.kind, "claude");
        assert_eq!(choice.reason, "claude with no reading (codex with no reading)");
    }

    #[test]
    fn quota_advisory_requires_more_than_80_percent_and_an_alternative_below_50() {
        let choices = vec!["claude".into(), "codex".into(), "opencode".into()];
        assert_eq!(quota_advisory("claude", &choices, &HashMap::from([("claude".into(), 0.79), ("codex".into(), 0.1)])), None);
        assert!(quota_advisory("claude", &choices, &HashMap::from([("claude".into(), 0.81), ("codex".into(), 0.49)])).is_some());
        assert_eq!(quota_advisory("claude", &choices, &HashMap::from([("claude".into(), 0.81), ("codex".into(), 0.51)])), None);
        assert_eq!(quota_advisory("claude", &choices, &HashMap::from([("claude".into(), 0.81), ("codex".into(), 0.5)])), None);
        assert_eq!(quota_advisory("claude", &choices, &HashMap::from([("claude".into(), 0.81), ("opencode".into(), 0.0)])), None);

        let adv = quota_advisory("claude", &choices, &HashMap::from([("claude".into(), 0.81), ("codex".into(), 0.49)])).unwrap();
        with_lang(Lang::En, || {
            assert_eq!(
                quota_advisory("claude", &choices, &HashMap::from([("claude".into(), 0.81), ("codex".into(), 0.49)])).as_deref(),
                Some("claude is at 81% quota; codex is at 49%")
            );
        });
        with_lang(Lang::PtBr, || {
            assert_eq!(
                quota_advisory("claude", &choices, &HashMap::from([("claude".into(), 0.81), ("codex".into(), 0.49)])).as_deref(),
                Some("claude está com 81% da cota usada; codex está com 49%")
            );
        });
        let _ = adv;
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
        // Local does not go to disk (no quota).
        assert!(store.set("local", AgentUsage { local: true, updated_ms: 1, ..Default::default() }));
        let again = UsageStore::load(file.clone());
        assert_eq!(again.snapshot(), HashMap::from([("claude".to_owned(), claude)]));
        // Corrupted file: starts empty; invalid entry is skipped.
        std::fs::write(&file, "{ not json").expect("write");
        assert!(UsageStore::load(file.clone()).snapshot().is_empty());
        let mixed = r#"{"codex":{"session":{"used":7}},"claude":{"session":null,"weekly":{"used":0.1},"updated_ms":5}}"#;
        std::fs::write(&file, mixed).expect("write");
        let partial = UsageStore::load(file).snapshot();
        assert_eq!(partial.len(), 1);
        assert_eq!(partial["claude"].updated_ms, 5);
    }

    #[test]
    fn each_model_counts_against_its_own_group() {
        // Real Antigravity quota on 24/10: Argon exhausted in the week, Flash almost untouched, Claude/GPT at 95%.
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
        assert_eq!(argon.note(NOW), "quota for Gemini 4 Argon exhausted, renews in 9 h");
        let flash = model_quota(&agy, Some("gemini-3.8-flash-high"), NOW).expect("flash");
        assert_eq!(
            (flash.group.as_deref(), flash.note(NOW).as_str()),
            (Some("Gemini Models"), "quota for Gemini Models 0% used, renews in 34 h")
        );
        let sonnet = model_quota(&agy, Some("claude-sonnet-5-5-high"), NOW).expect("sonnet");
        assert_eq!(sonnet.note(NOW), "quota for Claude and GPT models 95% used, renews in 5 days");
        // Without model, CLI default could be any group: unknown.
        assert_eq!(model_quota(&agy, None, NOW), None);
        // Renewed since reading: stops counting.
        assert!(!model_quota(&agy, Some("gemini-4-argon-low"), NOW + 10 * 3_600_000).is_some_and(|q| q.exhausted()));
        assert_eq!(
            quota_line(&agy, NOW).as_deref(),
            Some(
                "Gemini Models 0% used (renews in 34 h), Gemini 4 Argon EXHAUSTED (renews in 9 h), Claude and GPT models 95% used (renews in 5 days)"
            )
        );
        // Below 80% also shows how much was used and when it renews.
        let renewed = AgentUsage { groups: vec![group("Gemini 4 Argon", None, w(0.04, 6 * 24))], updated_ms: NOW, ..Default::default() };
        assert_eq!(quota_line(&renewed, NOW).as_deref(), Some("Gemini 4 Argon 4% used (renews in 6 days)"));
        assert_eq!(agent_used(&renewed, NOW), Some(0.04));
        // In Antigravity ordering, Gemini groups apply, not Claude/GPT.
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
        // Without live reading: reports when the last one was or "no reading".
        assert_eq!(no_reading_text(None, NOW), "no reading");
        assert_eq!(no_reading_text(Some(&AgentUsage::default()), NOW), "no reading");
        assert_eq!(no_reading_text(Some(&agy), NOW + 15 * 60_000), "last read 15 min ago");
        // Without groups (Claude): plan applies, the most consumed window.
        let claude = AgentUsage { session: w(0.2, 1), weekly: w(0.5, 30), ..Default::default() };
        let q = model_quota(&claude, Some("sonnet"), NOW).expect("claude");
        assert_eq!((q.group.clone(), q.used, q.resets_at_ms), (None, 0.5, Some(NOW + 30 * 3_600_000)));
        assert_eq!(q.note(NOW), "quota 50% used, renews in 30 h");
        assert_eq!(agent_used(&claude, NOW), Some(0.5));
        // The model's group decides; an unmapped or default model falls back to the agent number and says so.
        let used = |m| model_usage(&agy_renewed, m, NOW).map(|u| (u.used, u.group, u.fallback));
        assert_eq!(used(Some("gemini-4-argon-high")), Some((0.04, Some("Gemini 4 Argon".into()), false)));
        assert_eq!(used(Some("claude-sonnet-5-5-high")), Some((0.95, Some("Claude and GPT models".into()), false)));
        assert_eq!(used(Some("gemini-3.8-flash-high")), Some((0.004, Some("Gemini Models".into()), false)));
        assert_eq!(used(None), Some((0.04, None, true)));
        let no_claude = AgentUsage { groups: vec![group("Gemini 4 Argon", None, w(0.04, 6 * 24))], updated_ms: NOW, ..Default::default() };
        assert_eq!(model_usage(&no_claude, Some("claude-sonnet-5-5-high"), NOW).map(|u| (u.used, u.fallback)), Some((0.04, true)));
        assert_eq!(model_usage(&claude, Some("sonnet"), NOW).map(|u| (u.used, u.fallback)), Some((0.5, false)));
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
        // API key: null `rate_limits`; or only one window.
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
        // Invalid group is skipped; remainder is valid.
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
        assert_eq!(rfc3339_ms("yesterday"), None);
        // Signs and junk in fields or timezone do not pass as numbers.
        for bad in [
            "2025-09-18T-5:00:00Z",
            "2025-09-18T+5:00:00Z",
            "2025-09-18T12:+0:00Z",
            "+025-09-18T12:00:00Z",
            "2025-09-18T12:00:00+03x00junk",
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
