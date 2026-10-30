//! The capacity of each model for the size of the task.
//!
//! Choosing by quota alone sends the model with the most headroom to do anything, including what it cannot handle.
//! The choice has two pieces: the **task level** ([`Level`]: `simples`, `media`, `complexa`; it is not the effort, which
//! stays as the model's reasoning) and the **level the model reaches**. This comes from two sources:
//!
//! 1. the table of public benchmarks (`capability.json`, embedded; the user overrides it with
//!    `data/capability.json`), with the score, the source, the date and the level that comes from it;
//! 2. the measure of what the model has already delivered here ([`measure`]): with at least [`MIN_SAMPLE`] tasks in a
//!    level, the measure wins over the table (whoever fails drops one level, whoever gets it right on the first try
//!    in a row goes up).
//!
//! Here live the pure rules (with tests) and the `impl Core` that applies them to the caller: `task_create` and
//! `ask-agent --models`.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::t;
use crate::tasks::{Task, TaskStatus};
use crate::usage::{select_agent_by_quota, AgentQuotaCandidate, EXHAUSTED};
use crate::Core;

/// The embedded table (October 2026 benchmarks).
const BUILTIN: &str = include_str!("capability.json");

/// The file (in `data/`) that overrides the embedded table: the same format, entries by `name`.
pub const OVERRIDE_FILE: &str = "capability.json";

/// The name the override file had before the English rename; still read when the new one does not exist.
pub const LEGACY_OVERRIDE_FILE: &str = "capacidade.json";

/// Tasks in a level from which the measure wins over the table.
pub const MIN_SAMPLE: u32 = 3;

/// The size of a task, or the biggest size a model can handle.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Level {
    #[serde(rename = "simple", alias = "simples")]
    Simple,
    /// The value of a task with no level informed.
    #[default]
    #[serde(rename = "medium", alias = "media")]
    Medium,
    #[serde(rename = "complex", alias = "complexa")]
    Complex,
}

impl Level {
    pub const ALL: [Level; 3] = [Level::Simple, Level::Medium, Level::Complex];

    /// The value of `--level`, of the JSON and of the table.
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Simple => "simple",
            Level::Medium => "medium",
            Level::Complex => "complex",
        }
    }

    /// The name in the interface.
    pub fn label(self) -> &'static str {
        match self {
            Level::Simple => t!("Simple"),
            Level::Medium => t!("Medium"),
            Level::Complex => t!("Complex"),
        }
    }

    /// The name inside a sentence, without the capital and in the current language ("complex" / "complexa").
    fn in_sentence(self) -> &'static str {
        match self {
            Level::Simple => t!("simple"),
            Level::Medium => t!("medium"),
            Level::Complex => t!("complex"),
        }
    }

    /// Reads `simples|media|complexa` (or `média`, `simple`, `medium`, `complex`), case-insensitive.
    pub fn parse(s: &str) -> Option<Level> {
        match s.trim().to_lowercase().as_str() {
            "simples" | "simple" | "low" => Some(Level::Simple),
            "media" | "média" | "medium" => Some(Level::Medium),
            "complexa" | "complex" | "high" => Some(Level::Complex),
            _ => None,
        }
    }

    fn index(self) -> usize {
        self as usize
    }

    /// One level below (`simples` stays where it is).
    pub fn down(self) -> Level {
        match self {
            Level::Complex => Level::Medium,
            _ => Level::Simple,
        }
    }

    /// One level above; `None` at `complexa`.
    pub fn up(self) -> Option<Level> {
        match self {
            Level::Simple => Some(Level::Medium),
            Level::Medium => Some(Level::Complex),
            Level::Complex => None,
        }
    }
}

// ---------- The table ----------

/// A row of the table: a model, the score and where it came from.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Entry {
    pub name: String,
    /// The CLI ids that match the model (`opus`, `claude-opus-5-5`…): the model id, without the provider or the
    /// `#variant`, is equal to the alias or starts with it followed by `-`.
    pub aliases: Vec<String>,
    /// The score on the ruler; `None` when the model has none (the level is then estimated).
    pub score: Option<f32>,
    /// The level that comes from the score; absent = computed by the table thresholds.
    pub level: Option<Level>,
    /// The level is a guess, it does not come from a score on the ruler.
    pub estimated: bool,
    pub source: String,
    /// `YYYY-MM` of the source.
    pub date: String,
    pub url: String,
}

/// The ruler thresholds: `complex` from `complex`, `medium` from `medium`, `simple` below (older tables use `complexa`/`media`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Thresholds {
    #[serde(alias = "complexa")]
    pub complex: f32,
    #[serde(alias = "media")]
    pub medium: f32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self { complex: 53.0, medium: 40.0 }
    }
}

/// The capacity table by benchmark.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Table {
    /// The ruler (benchmark name and conditions).
    pub ruler: String,
    pub thresholds: Thresholds,
    /// The model assumed for an agent opened without `--model` (the CLI default is not informed).
    pub defaults: BTreeMap<String, String>,
    pub models: Vec<Entry>,
}

/// A model's id to match the aliases: lowercase, without provider, without `#variant` or `[context]`.
fn normalize(model: &str) -> String {
    let m = model.trim().to_lowercase().replace([' ', '_'], "-");
    let m = m.rsplit('/').next().unwrap_or_default();
    let m = m.split(['#', '[']).next().unwrap_or_default();
    m.to_owned()
}

/// How many characters of the alias match the model (0 = no match): equal, or the model continues after a
/// separator (`-`, `:`…; the `.` does not count, so `gpt-6` does not match `gpt-6.1-sol`).
fn alias_len(alias: &str, model: &str) -> usize {
    let alias = normalize(alias);
    if alias.is_empty() || !model.starts_with(&alias) {
        return 0;
    }
    match model[alias.len()..].chars().next() {
        None => alias.len(),
        Some(c) if !c.is_ascii_alphanumeric() && c != '.' => alias.len(),
        Some(_) => 0,
    }
}

/// The model is a low-effort one (the Antigravity ones carry `-low` in the name).
fn low_effort_model(model: &str) -> bool {
    let m = model.trim().to_lowercase();
    m.split(['#', '[']).next().unwrap_or_default().ends_with("-low") || m.ends_with("#low")
}

impl Table {
    /// The embedded table.
    pub fn builtin() -> Table {
        serde_json::from_str(BUILTIN).expect("the embedded capability.json is valid")
    }

    /// The embedded one with the entries of `<data_dir>/capability.json` (or the legacy `capacidade.json`) on top (same `name` = replaces; a new name =
    /// adds; `defaults` are merged). Missing or unreadable file: only the embedded one stays (the error goes to the log).
    pub fn load(data_dir: &Path) -> Table {
        let mut table = Table::builtin();
        let path = [OVERRIDE_FILE, LEGACY_OVERRIDE_FILE]
            .iter()
            .map(|name| data_dir.join(name))
            .find(|p| p.exists())
            .unwrap_or_else(|| data_dir.join(OVERRIDE_FILE));
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    log::warn!("could not read {}: {e}", path.display());
                }
                return table;
            }
        };
        match serde_json::from_str::<Table>(text.trim_start_matches('\u{feff}')) {
            Ok(user) => table.merge(user),
            Err(e) => log::warn!("{} is unreadable, using only the embedded table: {e}", path.display()),
        }
        table
    }

    fn merge(&mut self, user: Table) {
        if !user.ruler.is_empty() {
            self.ruler = user.ruler;
        }
        self.defaults.extend(user.defaults);
        for entry in user.models {
            match self.models.iter_mut().find(|e| e.name.eq_ignore_ascii_case(&entry.name)) {
                Some(old) => *old = entry,
                None => self.models.push(entry),
            }
        }
    }

    /// The model that applies to `kind`: the requested one or, without it, the one the table assumes for the agent.
    pub fn assumed_model(&self, kind: &str, model: Option<&str>) -> Option<String> {
        model.map(str::to_owned).or_else(|| self.defaults.get(kind).cloned())
    }

    /// The table row of the model `model` (already the assumed one, see [`Table::assumed_model`]); the longest alias wins.
    pub fn find(&self, model: &str) -> Option<&Entry> {
        let model = normalize(model);
        self.models
            .iter()
            .filter_map(|e| Some((e, e.aliases.iter().map(|a| alias_len(a, &model)).max().filter(|n| *n > 0)?)))
            .max_by_key(|(_, n)| *n)
            .map(|(e, _)| e)
    }

    /// The level the row score gives (a saved `level` wins; with neither score nor level, `simples`).
    pub fn level_of(&self, entry: &Entry) -> Level {
        let by_score = match entry.score {
            Some(s) if s >= self.thresholds.complex => Level::Complex,
            Some(s) if s >= self.thresholds.medium => Level::Medium,
            _ => Level::Simple,
        };
        entry.level.unwrap_or(by_score)
    }

    /// The table level for the model (`simples` if it is not in it).
    pub fn table_level(&self, model: Option<&str>) -> Level {
        model.and_then(|m| self.find(m)).map_or(Level::Simple, |e| self.level_of(e))
    }

    /// The key with which the measure groups the model's tasks: the table row name, or `agent model`.
    /// It is an internal identity, always in English: the table row names must not change with the language.
    pub fn key(&self, kind: &str, model: Option<&str>) -> String {
        let model = self.assumed_model(kind, model);
        match model.as_deref().and_then(|m| self.find(m)) {
            Some(e) => e.name.clone(),
            None => format!("{kind} {}", model.as_deref().unwrap_or("default")),
        }
    }

    /// "Coding Agent Index 59.3 (2026-10)" or "estimated, no score (…)": what `--models` shows of the source.
    pub fn note(&self, entry: &Entry) -> String {
        let ruler = self.ruler.split(" (").next().unwrap_or(&self.ruler).replace("Artificial Analysis ", "");
        let ruler = ruler.trim_end_matches(" v2.0").to_owned();
        match entry.score {
            Some(s) => format!("{ruler} {s:.1} ({})", entry.date),
            None => format!("estimated, no score ({}): {}", entry.date, entry.source),
        }
    }
}

// ---------- The measure ----------

/// What the model delivered in a level.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LevelStats {
    /// Finished on the first delivery.
    pub first: u32,
    /// Finished after being returned.
    pub returned: u32,
    /// How many returns (review or validation) those and the other tasks had, in total.
    pub returns: u32,
    pub discarded: u32,
    /// Tasks rejected in validation (already counted in `first`/`returned` if they were merged later).
    pub rejected: u32,
    /// Discarded or returned [`BAD_RETURNS`] times or more.
    bad: u32,
    /// The deliveries in the order they finished: `true` = on the first try.
    recent: Vec<bool>,
}

/// Returned this many times or more, the task counts as unsuccessful.
pub const BAD_RETURNS: usize = 2;

impl LevelStats {
    /// The tasks that already have an outcome (finished or discarded).
    pub fn total(&self) -> u32 {
        self.first + self.returned + self.discarded
    }

    /// Enough sample and more than half of the tasks discarded or returned 2 times or more: the model cannot
    /// handle this level.
    pub fn demotes(&self) -> bool {
        self.total() >= MIN_SAMPLE && self.bad * 2 > self.total()
    }

    /// The last [`MIN_SAMPLE`] tasks of the level were all on the first try: the model can handle this level.
    pub fn promotes(&self) -> bool {
        let n = MIN_SAMPLE as usize;
        self.recent.len() >= n && self.recent[self.recent.len() - n..].iter().all(|ok| *ok)
    }

    /// "1 of 4 on the first try, 2 finished after being returned (3 returns), 1 discarded, 1 rejected in validation";
    /// empty with no task.
    pub fn text(&self) -> String {
        if self.total() == 0 {
            return String::new();
        }
        let mut parts = vec![format!("{} of {} on the first try", self.first, self.total())];
        if self.returned > 0 {
            let times = if self.returns == 1 { "1 return".to_owned() } else { format!("{} returns", self.returns) };
            parts.push(format!("{} finished after being returned ({times})", self.returned));
        }
        if self.discarded > 0 {
            parts.push(format!("{} discarded", self.discarded));
        }
        if self.rejected > 0 {
            parts.push(format!("{} rejected in validation", self.rejected));
        }
        parts.join(", ")
    }
}

/// The measure of a model, by level (in the order of [`Level::ALL`]).
pub type Record = [LevelStats; 3];

/// Measures each model by the tasks: only those the agent got to run and that already have an outcome
/// (merged, validating or discarded) are counted. A task with no saved level counts as `media`. The tasks come
/// in any order; the sequence of hits follows the order they finished.
pub fn measure<'a>(tasks: impl IntoIterator<Item = &'a Task>, table: &Table) -> HashMap<String, Record> {
    let mut tasks: Vec<&Task> = tasks.into_iter().filter(|t| t.started_ms.is_some() || t.report.is_some()).collect();
    tasks.sort_by_key(|t| t.finished_ms.unwrap_or(t.created_ms));
    let mut out: HashMap<String, Record> = HashMap::new();
    for task in tasks {
        let done = matches!(task.status, TaskStatus::Merged | TaskStatus::Validating);
        if !done && task.status != TaskStatus::Discarded {
            continue;
        }
        let stats = &mut out.entry(table.key(&task.kind, task.model.as_deref())).or_default()[task.level.index()];
        let returns = task.feedbacks.len();
        stats.returns += returns as u32;
        stats.rejected += u32::from(task.feedbacks.iter().any(|f| f.origin == "validation"));
        if !done {
            stats.discarded += 1;
            stats.bad += 1;
            stats.recent.push(false);
            continue;
        }
        if returns == 0 {
            stats.first += 1;
        } else {
            stats.returned += 1;
        }
        stats.bad += u32::from(returns >= BAD_RETURNS);
        stats.recent.push(returns == 0);
    }
    out
}

/// The level the model reaches, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capacity {
    pub level: Level,
    /// What changed compared to the table (low effort, measure), for the recorded reason.
    pub notes: Vec<String>,
}

/// The level that applies to a model: the table one, minus one with `low` effort (the score is of the maximum effort),
/// and then the measure: whoever got the last tasks of the level above right on the first try goes up; whoever fails
/// in the level it is in goes down.
pub fn capacity(table_level: Level, low_effort: bool, record: Option<&Record>) -> Capacity {
    let mut level = table_level;
    let mut notes = Vec::new();
    if low_effort && level > Level::Simple {
        level = level.down();
        notes.push(t!("low effort: down to {level}", level = level.in_sentence()));
    }
    if let Some(record) = record {
        while let Some(next) = level.up().filter(|n| record[n.index()].promotes()) {
            level = next;
            notes.push(t!(
                "measured: {count} {level} tasks in a row on the first try, up to {next}",
                count = MIN_SAMPLE,
                level = next.in_sentence(),
                next = next.in_sentence()
            ));
        }
        while level > Level::Simple && record[level.index()].demotes() {
            notes.push(t!("measured: failed at {level}, down to {next}", level = level.in_sentence(), next = level.down().in_sentence()));
            level = level.down();
        }
    }
    Capacity { level, notes }
}

// ---------- The choice ----------

/// A model that can receive the task.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub kind: String,
    /// The model (requested or assumed) that the reason cites.
    pub model: Option<String>,
    /// The fraction of the quota used; `None` = no reading.
    pub used: Option<f32>,
    /// The biggest level the model reaches.
    pub capacity: Level,
}

impl Candidate {
    fn label(&self) -> String {
        match &self.model {
            Some(m) => format!("{} {}", self.kind, crate::models::short_model(m)),
            None => self.kind.clone(),
        }
    }

    fn has_quota(&self) -> bool {
        self.used.is_none_or(|u| u < EXHAUSTED)
    }
}

/// The chosen one, the reason to record in the task and the warning when the choice is not the ideal one.
#[derive(Debug, Clone, PartialEq)]
pub struct Selection {
    pub kind: String,
    pub used: Option<f32>,
    pub reason: String,
    pub warning: Option<String>,
}

/// The `pool` candidate with the most headroom in the quota.
fn by_quota<'a>(pool: &[&'a Candidate]) -> Option<&'a Candidate> {
    let list: Vec<AgentQuotaCandidate> = pool.iter().map(|c| AgentQuotaCandidate { kind: c.kind.clone(), used: c.used }).collect();
    let picked = select_agent_by_quota(&list)?;
    pool.iter().find(|c| c.kind == picked.kind).copied()
}

/// Among the capable candidates at `level`, the one with the most headroom in the quota (the rule of [`select_agent_by_quota`]).
/// With no capable one with quota, the least spent capable one, with a warning; with no capable one, the one that reaches
/// furthest (tie-break by quota), with a warning. It never drops to an incapable model without saying so.
pub fn select(candidates: &[Candidate], level: Level) -> Option<Selection> {
    let capable: Vec<&Candidate> = candidates.iter().filter(|c| c.capacity >= level).collect();
    let with_quota: Vec<&Candidate> = capable.iter().copied().filter(|c| c.has_quota()).collect();
    let (chosen, warning) = if let Some(c) = by_quota(&with_quota) {
        (c, None)
    } else if let Some(c) = by_quota(&capable) {
        (
            c,
            Some(t!(
                "no model capable of a {level} task has quota; I used {agent}, the least spent",
                level = level.in_sentence(),
                agent = c.label()
            )),
        )
    } else {
        let best = candidates.iter().map(|c| c.capacity).max()?;
        let top: Vec<&Candidate> = candidates.iter().filter(|c| c.capacity == best).collect();
        let usable: Vec<&Candidate> = top.iter().copied().filter(|c| c.has_quota()).collect();
        let c = by_quota(&usable).or_else(|| by_quota(&top))?;
        (
            c,
            Some(t!(
                "no available model can handle a {level} task; {agent} only reaches {capacity}",
                level = level.in_sentence(),
                agent = c.label(),
                capacity = best.in_sentence()
            )),
        )
    };
    let mut parts = vec![match chosen.used {
        _ if chosen.capacity < level => t!(
            "{agent}, the one that reaches furthest (up to {capacity})",
            agent = chosen.label(),
            capacity = chosen.capacity.in_sentence()
        ),
        Some(used) if chosen.has_quota() => {
            t!(
                "{agent}, capable and with {free}% free",
                agent = chosen.label(),
                free = format!("{:.0}", (1.0 - used).clamp(0.0, 1.0) * 100.0)
            )
        }
        Some(_) => t!("{agent}, capable and without quota", agent = chosen.label()),
        None => t!("{agent}, capable (no quota reading)", agent = chosen.label()),
    }];
    for other in candidates.iter().filter(|c| c.kind != chosen.kind) {
        parts.push(if other.capacity < level {
            t!("{agent} only reaches {capacity}", agent = other.label(), capacity = other.capacity.in_sentence())
        } else {
            match other.used {
                Some(u) => t!("{agent} {used}% used", agent = other.label(), used = format!("{:.0}", u * 100.0)),
                None => t!("{agent} no quota reading", agent = other.label()),
            }
        });
    }
    let reason = t!("{level}: {parts}", level = level.in_sentence(), parts = parts.join("; "));
    Some(Selection { kind: chosen.kind.clone(), used: chosen.used, reason, warning })
}

/// The warning when the task is above what the model reaches; `None` if it can handle it.
pub fn advisory(label: &str, capacity: &Capacity, level: Level) -> Option<String> {
    (capacity.level < level).then(|| {
        let why = if capacity.notes.is_empty() { String::new() } else { format!(" ({})", capacity.notes.join("; ")) };
        t!(
            "the task is {level} and {agent} only reaches {capacity}{why}",
            level = level.in_sentence(),
            agent = label,
            capacity = capacity.level.in_sentence(),
            why
        )
    })
}

// ---------- No Core ----------

impl Core {
    /// The capacity table (the embedded one with `data/capability.json` on top), read on each call.
    pub fn capability_table(&self) -> Table {
        Table::load(&self.paths().data_dir)
    }

    /// The measure of each model, from the tasks of every project (model → level → outcomes).
    pub fn capability_records(&self, table: &Table) -> HashMap<String, Record> {
        let tasks: Vec<Task> = self.projects().iter().flat_map(|p| self.tasks(&p.id)).collect();
        measure(&tasks, table)
    }

    /// What the model (`kind` with `model`, or the assumed one) reaches, by the table, by the effort and by the measure.
    pub fn model_capacity(
        &self,
        table: &Table,
        records: &HashMap<String, Record>,
        kind: &str,
        model: Option<&str>,
        low_effort: bool,
    ) -> Capacity {
        let model = table.assumed_model(kind, model);
        let low = low_effort || model.as_deref().is_some_and(low_effort_model);
        capacity(table.table_level(model.as_deref()), low, records.get(&table.key(kind, model.as_deref())))
    }

    /// The candidates of the automatic choice: the official CLIs available in `choices`, each with the model it
    /// would open (`role_models` or the one assumed by the table), the quota and what it reaches.
    pub fn capability_candidates(&self, choices: &[String], effort: Option<crate::models::Effort>) -> Vec<Candidate> {
        let table = self.capability_table();
        let records = self.capability_records(&table);
        let role_models = self.config().role_models;
        ["claude", "antigravity", "codex"]
            .iter()
            .filter(|kind| choices.iter().any(|c| c == **kind))
            .map(|kind| {
                let model = crate::models::resolve_model(&crate::instances::Ident::main(kind), None, None, &role_models);
                let low = effort == Some(crate::models::Effort::Low) && crate::models::effort_unavailable(kind, model.as_deref()).is_none();
                let capacity = self.model_capacity(&table, &records, kind, model.as_deref(), low);
                let model = table.assumed_model(kind, model.as_deref());
                // The quota of the model's group (Antigravity), not the agent's summary.
                let used = self.model_usage(kind, model.as_deref()).map(|u| u.used);
                Candidate { kind: (*kind).to_owned(), model, used, capacity: capacity.level }
            })
            .collect()
    }

    /// The capacity warning of an explicit choice (or of the form): the task is above what the model reaches.
    pub fn capability_advisory(
        &self,
        kind: &str,
        model: Option<&str>,
        level: Level,
        effort: Option<crate::models::Effort>,
    ) -> Option<String> {
        let table = self.capability_table();
        let records = self.capability_records(&table);
        let low = effort == Some(crate::models::Effort::Low) && crate::models::effort_unavailable(kind, model).is_none();
        let capacity = self.model_capacity(&table, &records, kind, model, low);
        let shown = table.assumed_model(kind, model);
        let label = match &shown {
            Some(m) => format!("{kind} {}", crate::models::short_model(m)),
            None => kind.to_owned(),
        };
        advisory(&label, &capacity, level)
    }

    /// The capacity lines of `--models`, one per model (from the table, from those listed in `items` and from the measured
    /// ones): "Claude Sonnet 5.5 (sonnet): complexa, Coding Agent Index 59.3 (2026-10) · measured media: 1 of 5 on the
    /// first try".
    pub fn capability_lines(&self, items: &[crate::models::AgentModels]) -> Vec<String> {
        let table = self.capability_table();
        let records = self.capability_records(&table);
        let role_models = self.config().role_models;
        // (measure key, model name, ids with which it appears), in the order they appear.
        let mut rows: Vec<(String, String, Vec<String>)> = Vec::new();
        let mut add = |kind: &str, model: Option<&str>| {
            let key = table.key(kind, model);
            let id = table.assumed_model(kind, model).map(|m| format!("{kind} {m}")).unwrap_or_else(|| kind.to_owned());
            match rows.iter_mut().find(|(k, _, _)| *k == key) {
                Some((_, _, ids)) => {
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
                None => rows.push((key.clone(), key, vec![id])),
            }
        };
        for item in items.iter().filter(|i| crate::models::accepts_model(&i.agent)) {
            match &item.list {
                crate::models::ModelList::Models(list) => {
                    list.iter().filter(|m| table.find(m).is_some()).for_each(|m| add(&item.agent, Some(m)))
                }
                _ => {
                    let configured = crate::models::resolve_model(&crate::instances::Ident::main(&item.agent), None, None, &role_models);
                    add(&item.agent, configured.as_deref());
                }
            }
        }
        for key in records.keys() {
            if !rows.iter().any(|(k, _, _)| k == key) {
                rows.push((key.clone(), key.clone(), vec![key.clone()]));
            }
        }
        rows.iter()
            .map(|(key, name, ids)| {
                let entry = table.models.iter().find(|e| &e.name == key);
                let head = match entry {
                    Some(e) => format!("{name} ({}): {}, {}", ids.join(", "), table.level_of(e).as_str(), table.note(e)),
                    None => format!("{name}: not in the table, simples"),
                };
                let measured: Vec<String> = Level::ALL
                    .iter()
                    .rev()
                    .filter_map(|l| records.get(key).map(|r| (l, r[l.index()].text())).filter(|(_, t)| !t.is_empty()))
                    .map(|(l, t)| format!("{}: {t}", l.as_str()))
                    .collect();
                let rec = records.get(key);
                let table_level = entry.map_or(Level::Simple, |e| table.level_of(e));
                let eff = capacity(table_level, false, rec);
                let mut line = head;
                if !measured.is_empty() {
                    line.push_str(&format!(" · measured {}", measured.join(" | ")));
                }
                if eff.level != table_level {
                    line.push_str(&format!(" · counts as {} ({})", eff.level.as_str(), eff.notes.join("; ")));
                }
                line
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::Feedback;

    fn table() -> Table {
        Table::builtin()
    }

    fn task(kind: &str, model: Option<&str>, level: Level, status: TaskStatus, feedbacks: &[&str], at: u64) -> Task {
        Task {
            kind: kind.into(),
            model: model.map(str::to_owned),
            level,
            status,
            started_ms: Some(at),
            finished_ms: Some(at),
            feedbacks: feedbacks.iter().map(|o| Feedback { at_ms: at, origin: (*o).into(), text: "x".into() }).collect(),
            ..Task::default()
        }
    }

    fn candidate(kind: &str, used: Option<f32>, capacity: Level) -> Candidate {
        Candidate { kind: kind.into(), model: Some(format!("{kind}-m")), used, capacity }
    }

    #[test]
    fn the_builtin_table_follows_the_ruler_thresholds() {
        let t = table();
        assert_eq!(t.models.len(), 18);
        for e in &t.models {
            if let Some(score) = e.score {
                let by_score = if score >= 53.0 {
                    Level::Complex
                } else if score >= 40.0 {
                    Level::Medium
                } else {
                    Level::Simple
                };
                assert_eq!(e.level, Some(by_score), "{} {score}", e.name);
            }
            assert!(!e.source.is_empty() && !e.date.is_empty() && !e.aliases.is_empty(), "{}", e.name);
        }
        let terra = t.models.iter().find(|e| e.name == "GPT-5.6 Terra").expect("terra");
        assert!(terra.estimated && terra.score.is_none());
    }

    #[test]
    fn cli_ids_find_the_table_entry() {
        let t = table();
        let level = |m: &str| t.table_level(Some(m));
        assert_eq!(level("opus"), Level::Complex);
        assert_eq!(level("sonnet"), Level::Complex);
        assert_eq!(level("claude-opus-5-5-high"), Level::Complex);
        assert_eq!(level("haiku"), Level::Simple);
        assert_eq!(level("gemini-4-argon-high"), Level::Complex);
        assert_eq!(level("gemini-3.8-flash-high"), Level::Simple);
        assert_eq!(level("opencode/deepseek-v4.1-flash-free"), Level::Medium);
        assert_eq!(level("opencode-go/deepseek-v4.1-flash#alta"), Level::Medium);
        assert_eq!(level("gpt-6-luna"), Level::Simple);
        assert_eq!(level("gpt-6.1-sol"), Level::Complex);
        assert_eq!(level("gpt-5.6-terra"), Level::Medium);
        // Outside the table: simples. A prefix without a separator does not match.
        assert_eq!(level("new-model"), Level::Simple);
        assert_eq!(level("opuscule"), Level::Simple);
        assert!(t.find("gpt-6").is_none());
    }

    #[test]
    fn an_agent_without_model_uses_the_presumed_one() {
        let t = table();
        assert_eq!(t.assumed_model("claude", None).as_deref(), Some("sonnet"));
        assert_eq!(t.assumed_model("claude", Some("haiku")).as_deref(), Some("haiku"));
        assert_eq!(t.assumed_model("shell", None), None);
        assert_eq!(t.key("claude", None), "Claude Sonnet 5.5");
        assert_eq!(t.key("opencode", Some("opencode/deepseek-v4.1-flash-free")), "DeepSeek V4.1 Flash");
        assert_eq!(t.key("goose", None), "goose default");
    }

    #[test]
    fn the_user_file_overrides_by_name_and_adds_models() {
        let dir = tempfile::tempdir().expect("tmp");
        std::fs::write(
            dir.path().join(OVERRIDE_FILE),
            r#"{"defaults":{"codex":"gpt-5.6-terra"},"models":[
                {"name":"gpt-6 luna","aliases":["gpt-6-luna"],"score":50.0,"source":"my test","date":"2026-11"},
                {"name":"New Model","aliases":["new-model"],"level":"complexa","source":"x","date":"2026-11"}]}"#,
        )
        .expect("write");
        let t = Table::load(dir.path());
        assert_eq!(t.models.len(), 19);
        assert_eq!(t.table_level(Some("gpt-6-luna")), Level::Medium);
        assert_eq!(t.table_level(Some("new-model")), Level::Complex);
        assert_eq!(t.assumed_model("codex", None).as_deref(), Some("gpt-5.6-terra"));
        assert_eq!(t.assumed_model("claude", None).as_deref(), Some("sonnet"));
        // An unreadable file does not bring the embedded table down.
        std::fs::write(dir.path().join(OVERRIDE_FILE), "{ not json").expect("write");
        assert_eq!(Table::load(dir.path()), Table::builtin());
        assert_eq!(Table::load(&dir.path().join("does-not-exist")), Table::builtin());
    }

    #[test]
    fn the_measure_counts_outcomes_per_model_and_level() {
        let t = table();
        let tasks = vec![
            task("claude", Some("sonnet"), Level::Medium, TaskStatus::Merged, &[], 1),
            task("claude", None, Level::Medium, TaskStatus::Validating, &["review"], 2),
            task("claude", Some("sonnet"), Level::Medium, TaskStatus::Merged, &["review", "validation"], 3),
            task("claude", Some("sonnet"), Level::Medium, TaskStatus::Discarded, &[], 4),
            task("claude", Some("sonnet"), Level::Medium, TaskStatus::Running, &[], 5),
            task("claude", Some("sonnet"), Level::Complex, TaskStatus::Merged, &[], 6),
            task("claude", Some("sonnet"), Level::Medium, TaskStatus::Draft, &[], 7),
        ];
        let rec = measure(&tasks, &t);
        let medium = &rec["Claude Sonnet 5.5"][Level::Medium.index()];
        assert_eq!((medium.first, medium.returned, medium.returns, medium.discarded, medium.rejected), (1, 2, 3, 1, 1));
        assert_eq!(medium.total(), 4);
        assert_eq!(
            medium.text(),
            "1 of 4 on the first try, 2 finished after being returned (3 returns), 1 discarded, 1 rejected in validation"
        );
        assert_eq!(rec["Claude Sonnet 5.5"][Level::Complex.index()].text(), "1 of 1 on the first try");
        assert_eq!(rec["Claude Sonnet 5.5"][Level::Simple.index()].text(), "");
    }

    #[test]
    fn few_tasks_change_nothing() {
        let t = table();
        let tasks = vec![
            task("codex", Some("gpt-6-luna"), Level::Simple, TaskStatus::Discarded, &[], 1),
            task("codex", Some("gpt-6-luna"), Level::Simple, TaskStatus::Discarded, &[], 2),
            task("claude", Some("sonnet"), Level::Complex, TaskStatus::Discarded, &[], 3),
            task("claude", Some("sonnet"), Level::Complex, TaskStatus::Discarded, &[], 4),
        ];
        let rec = measure(&tasks, &t);
        assert_eq!(capacity(Level::Complex, false, rec.get("Claude Sonnet 5.5")), Capacity { level: Level::Complex, notes: vec![] });
        let good = measure(&vec![task("codex", Some("gpt-6-luna"), Level::Medium, TaskStatus::Merged, &[], 1); 2], &t);
        assert_eq!(capacity(Level::Simple, false, good.get("GPT-6 Luna")).level, Level::Simple);
    }

    #[test]
    fn a_model_that_fails_goes_down_a_level() {
        let t = table();
        // More than half discarded or returned twice or more: 2 of 3.
        let tasks = vec![
            task("claude", Some("sonnet"), Level::Complex, TaskStatus::Discarded, &[], 1),
            task("claude", Some("sonnet"), Level::Complex, TaskStatus::Merged, &["review", "review"], 2),
            task("claude", Some("sonnet"), Level::Complex, TaskStatus::Merged, &[], 3),
        ];
        let rec = measure(&tasks, &t);
        let c = capacity(Level::Complex, false, rec.get("Claude Sonnet 5.5"));
        assert_eq!(c.level, Level::Medium);
        assert_eq!(c.notes.len(), 1);
        // An exact half does not bring it down; a single return is not a failure.
        let tasks = vec![
            task("claude", Some("sonnet"), Level::Complex, TaskStatus::Discarded, &[], 1),
            task("claude", Some("sonnet"), Level::Complex, TaskStatus::Merged, &["review"], 2),
            task("claude", Some("sonnet"), Level::Complex, TaskStatus::Merged, &[], 3),
            task("claude", Some("sonnet"), Level::Complex, TaskStatus::Merged, &[], 4),
        ];
        let rec = measure(&tasks, &t);
        assert_eq!(capacity(Level::Complex, false, rec.get("Claude Sonnet 5.5")).level, Level::Complex);
        // Failing at the medium level also brings down what had already dropped, down to simple.
        let tasks: Vec<Task> = (1..=6)
            .map(|n| task("claude", Some("sonnet"), if n <= 3 { Level::Complex } else { Level::Medium }, TaskStatus::Discarded, &[], n))
            .collect();
        let rec = measure(&tasks, &t);
        assert_eq!(capacity(Level::Complex, false, rec.get("Claude Sonnet 5.5")).level, Level::Simple);
        // Simple is the floor.
        assert_eq!(capacity(Level::Simple, false, rec.get("Claude Sonnet 5.5")).level, Level::Simple);
    }

    #[test]
    fn three_first_try_wins_in_a_row_go_up_a_level() {
        let t = table();
        let win = |n: u64| task("codex", Some("gpt-6-luna"), Level::Medium, TaskStatus::Merged, &[], n);
        let rec = measure(&[win(1), win(2), win(3)], &t);
        let c = capacity(Level::Simple, false, rec.get("GPT-6 Luna"));
        assert_eq!(c.level, Level::Medium);
        assert_eq!(c.notes.len(), 1);
        // One returned in the middle of the sequence (the last 3) does not promote.
        let rec = measure(&[win(1), task("codex", Some("gpt-6-luna"), Level::Medium, TaskStatus::Merged, &["review"], 2), win(3)], &t);
        assert_eq!(capacity(Level::Simple, false, rec.get("GPT-6 Luna")).level, Level::Simple);
        // But an old stumble, outside the last 3, does not prevent it.
        let rec = measure(&[task("codex", Some("gpt-6-luna"), Level::Medium, TaskStatus::Discarded, &[], 0), win(1), win(2), win(3)], &t);
        assert_eq!(capacity(Level::Simple, false, rec.get("GPT-6 Luna")).level, Level::Medium);
    }

    #[test]
    fn low_effort_costs_one_level() {
        assert_eq!(capacity(Level::Complex, true, None).level, Level::Medium);
        assert_eq!(capacity(Level::Medium, true, None).level, Level::Simple);
        assert_eq!(capacity(Level::Simple, true, None).level, Level::Simple);
        assert_eq!(capacity(Level::Complex, false, None).level, Level::Complex);
        assert!(low_effort_model("gemini-4-argon-low") && !low_effort_model("gemini-4-argon-high") && !low_effort_model("sonnet"));
    }

    #[test]
    fn the_choice_is_the_most_free_among_the_capable() {
        let c = [
            candidate("claude", Some(0.74), Level::Complex),
            candidate("codex", Some(0.1), Level::Simple),
            candidate("antigravity", Some(0.5), Level::Complex),
        ];
        let s = select(&c, Level::Complex).expect("escolha");
        assert_eq!(s.kind, "antigravity");
        assert_eq!(s.warning, None);
        assert_eq!(
            s.reason,
            "complex: antigravity antigravity-m, capable and with 50% free; claude claude-m 74% used; codex codex-m only reaches simple"
        );
        // On a simple task all are capable and the free quota decides.
        assert_eq!(select(&c, Level::Simple).expect("escolha").kind, "codex");
        // Without a quota reading it loses to whoever has free quota, but it is capable.
        let c = [candidate("claude", None, Level::Complex), candidate("antigravity", Some(0.9), Level::Complex)];
        assert_eq!(select(&c, Level::Medium).expect("escolha").kind, "antigravity");
        let c = [candidate("claude", None, Level::Complex), candidate("codex", Some(0.1), Level::Simple)];
        let s = select(&c, Level::Complex).expect("escolha");
        assert_eq!((s.kind.as_str(), s.warning), ("claude", None));
        assert!(s.reason.contains("capable (no quota reading)"), "{}", s.reason);
    }

    #[test]
    fn with_no_capable_model_with_quota_the_least_spent_capable_is_used_with_a_warning() {
        let c = [
            candidate("claude", Some(1.0), Level::Complex),
            candidate("antigravity", Some(0.999), Level::Complex),
            candidate("codex", Some(0.1), Level::Simple),
        ];
        let s = select(&c, Level::Complex).expect("escolha");
        assert_eq!(s.kind, "antigravity");
        let warning = s.warning.expect("aviso");
        assert!(warning.contains("no model capable of a complex task has quota") && warning.contains("antigravity"), "{warning}");
        assert!(s.reason.contains("without quota"), "{}", s.reason);
    }

    #[test]
    fn with_no_capable_model_the_one_that_reaches_furthest_is_used_with_a_warning() {
        let c = [
            candidate("codex", Some(0.1), Level::Simple),
            candidate("antigravity", Some(0.3), Level::Medium),
            candidate("claude", Some(0.2), Level::Medium),
        ];
        let s = select(&c, Level::Complex).expect("escolha");
        assert_eq!(s.kind, "claude");
        let warning = s.warning.expect("aviso");
        assert!(warning.contains("no available model can handle a complex task") && warning.contains("only reaches medium"), "{warning}");
        assert!(select(&[], Level::Medium).is_none());
    }

    #[test]
    fn the_advisory_appears_only_above_the_capacity() {
        let low = Capacity { level: Level::Simple, notes: vec!["low effort: down to simple".into()] };
        assert_eq!(
            advisory("codex gpt-6-luna", &low, Level::Complex).as_deref(),
            Some("the task is complex and codex gpt-6-luna only reaches simple (low effort: down to simple)")
        );
        assert_eq!(advisory("codex gpt-6-luna", &low, Level::Simple), None);
    }
}
