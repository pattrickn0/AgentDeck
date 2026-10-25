//! A capacidade de cada modelo diante do tamanho da tarefa.
//!
//! Escolher só pela cota manda o modelo mais folgado fazer qualquer coisa, inclusive o que ele não dá conta. A
//! escolha tem duas peças: o **nível da tarefa** ([`Level`]: `simples`, `media`, `complexa`; não é o esforço, que
//! continua sendo o raciocínio do modelo) e o **nível que o modelo alcança**. Este vem de duas fontes:
//!
//! 1. a tabela de benchmarks públicos (`capability.json`, embutida; o usuário a sobrepõe com
//!    `dados/capacidade.json`), com a nota, a fonte, a data e o nível que sai dela;
//! 2. a medida do que o modelo já entregou aqui ([`measure`]): com pelo menos [`MIN_SAMPLE`] tarefas num nível, a
//!    medida manda sobre a tabela (desce um nível quem falha, sobe quem acerta de primeira em sequência).
//!
//! Aqui ficam as regras puras (com testes) e o `impl Core` que as aplica a quem chama: `task_create` e o
//! `ask-agent --models`.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::tasks::{Task, TaskStatus};
use crate::usage::{select_agent_by_quota, AgentQuotaCandidate, EXHAUSTED};
use crate::Core;

/// A tabela embutida (benchmarks de outubro de 2026).
const BUILTIN: &str = include_str!("capability.json");

/// O arquivo (em `dados/`) que sobrepõe a tabela embutida: o mesmo formato, entradas pelo `name`.
pub const OVERRIDE_FILE: &str = "capacidade.json";

/// Tarefas num nível a partir das quais a medida manda sobre a tabela.
pub const MIN_SAMPLE: u32 = 3;

/// O tamanho de uma tarefa, ou o maior tamanho que um modelo dá conta.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Level {
    #[serde(rename = "simples")]
    Simple,
    /// O valor de uma tarefa sem nível informado.
    #[default]
    #[serde(rename = "media")]
    Medium,
    #[serde(rename = "complexa")]
    Complex,
}

impl Level {
    pub const ALL: [Level; 3] = [Level::Simple, Level::Medium, Level::Complex];

    /// O valor de `--level`, do JSON e da tabela.
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Simple => "simples",
            Level::Medium => "media",
            Level::Complex => "complexa",
        }
    }

    /// O nome na interface.
    pub fn label(self) -> &'static str {
        match self {
            Level::Simple => "Simples",
            Level::Medium => "Média",
            Level::Complex => "Complexa",
        }
    }

    /// Lê `simples|media|complexa` (ou `média`, `simple`, `medium`, `complex`), sem diferenciar maiúsculas.
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

    /// Um nível abaixo (o `simples` fica onde está).
    pub fn down(self) -> Level {
        match self {
            Level::Complex => Level::Medium,
            _ => Level::Simple,
        }
    }

    /// Um nível acima; `None` no `complexa`.
    pub fn up(self) -> Option<Level> {
        match self {
            Level::Simple => Some(Level::Medium),
            Level::Medium => Some(Level::Complex),
            Level::Complex => None,
        }
    }
}

// ---------- A tabela ----------

/// Uma linha da tabela: um modelo, a nota e de onde ela veio.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Entry {
    pub name: String,
    /// Os ids de CLI que casam com o modelo (`opus`, `claude-opus-5-5`…): o id do modelo, sem o provedor nem a
    /// `#variante`, é igual ao apelido ou começa com ele seguido de `-`.
    pub aliases: Vec<String>,
    /// A nota na régua; `None` quando o modelo não tem (o nível então é estimado).
    pub score: Option<f32>,
    /// O nível que sai da nota; ausente = calculado pelos limiares da tabela.
    pub level: Option<Level>,
    /// O nível é um palpite, não sai de uma nota da régua.
    pub estimated: bool,
    pub source: String,
    /// `AAAA-MM` da fonte.
    pub date: String,
    pub url: String,
}

/// Os limiares da régua: `complexa` a partir de `complexa`, `media` a partir de `media`, `simples` abaixo.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Thresholds {
    pub complexa: f32,
    pub media: f32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self { complexa: 53.0, media: 40.0 }
    }
}

/// A tabela de capacidade por benchmark.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Table {
    /// A régua (nome e condições do benchmark).
    pub ruler: String,
    pub thresholds: Thresholds,
    /// O modelo que se presume para um agente aberto sem `--model` (o padrão do CLI não é informado).
    pub defaults: BTreeMap<String, String>,
    pub models: Vec<Entry>,
}

/// O id de um modelo para casar com os apelidos: minúsculo, sem provedor, sem `#variante` nem `[contexto]`.
fn normalize(model: &str) -> String {
    let m = model.trim().to_lowercase().replace([' ', '_'], "-");
    let m = m.rsplit('/').next().unwrap_or_default();
    let m = m.split(['#', '[']).next().unwrap_or_default();
    m.to_owned()
}

/// Quantos caracteres do apelido casam com o modelo (0 = não casa): igual, ou o modelo continua depois de um
/// separador (`-`, `:`…; o `.` não conta, para `gpt-6` não casar com `gpt-6.1-sol`).
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

/// O modelo é de esforço baixo (os do Antigravity trazem `-low` no nome).
fn low_effort_model(model: &str) -> bool {
    let m = model.trim().to_lowercase();
    m.split(['#', '[']).next().unwrap_or_default().ends_with("-low") || m.ends_with("#low")
}

impl Table {
    /// A tabela embutida.
    pub fn builtin() -> Table {
        serde_json::from_str(BUILTIN).expect("capability.json embutido é válido")
    }

    /// A embutida com as entradas de `<data_dir>/capacidade.json` por cima (mesmo `name` = substitui; nome novo =
    /// acrescenta; `defaults` somam-se). Arquivo ausente ou ilegível: fica só a embutida (o erro vai ao log).
    pub fn load(data_dir: &Path) -> Table {
        let mut table = Table::builtin();
        let path = data_dir.join(OVERRIDE_FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    log::warn!("não consegui ler {}: {e}", path.display());
                }
                return table;
            }
        };
        match serde_json::from_str::<Table>(text.trim_start_matches('\u{feff}')) {
            Ok(user) => table.merge(user),
            Err(e) => log::warn!("{} ilegível, usando só a tabela embutida: {e}", path.display()),
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

    /// O modelo que vale para `kind`: o pedido ou, sem ele, o que a tabela presume para o agente.
    pub fn assumed_model(&self, kind: &str, model: Option<&str>) -> Option<String> {
        model.map(str::to_owned).or_else(|| self.defaults.get(kind).cloned())
    }

    /// A linha da tabela do modelo `model` (já o presumido, ver [`Table::assumed_model`]); a de apelido mais longo vence.
    pub fn find(&self, model: &str) -> Option<&Entry> {
        let model = normalize(model);
        self.models
            .iter()
            .filter_map(|e| Some((e, e.aliases.iter().map(|a| alias_len(a, &model)).max().filter(|n| *n > 0)?)))
            .max_by_key(|(_, n)| *n)
            .map(|(e, _)| e)
    }

    /// O nível que a nota da linha dá (o `level` gravado vence; sem nota nem nível, `simples`).
    pub fn level_of(&self, entry: &Entry) -> Level {
        let by_score = match entry.score {
            Some(s) if s >= self.thresholds.complexa => Level::Complex,
            Some(s) if s >= self.thresholds.media => Level::Medium,
            _ => Level::Simple,
        };
        entry.level.unwrap_or(by_score)
    }

    /// O nível da tabela para o modelo (`simples` se ele não está nela).
    pub fn table_level(&self, model: Option<&str>) -> Level {
        model.and_then(|m| self.find(m)).map_or(Level::Simple, |e| self.level_of(e))
    }

    /// A chave com que a medida agrupa as tarefas do modelo: o nome da linha da tabela, ou `agente modelo`.
    pub fn key(&self, kind: &str, model: Option<&str>) -> String {
        let model = self.assumed_model(kind, model);
        match model.as_deref().and_then(|m| self.find(m)) {
            Some(e) => e.name.clone(),
            None => format!("{kind} {}", model.as_deref().unwrap_or("padrão")),
        }
    }

    /// "Coding Agent Index 59.3 (2026-10)" ou "nível estimado (…)": o que o `--models` mostra da fonte.
    pub fn note(&self, entry: &Entry) -> String {
        let ruler = self.ruler.split(" (").next().unwrap_or(&self.ruler).replace("Artificial Analysis ", "");
        let ruler = ruler.trim_end_matches(" v2.0").to_owned();
        match entry.score {
            Some(s) => format!("{ruler} {s:.1} ({})", entry.date),
            None => format!("estimado, sem nota ({}): {}", entry.date, entry.source),
        }
    }
}

// ---------- A medida ----------

/// O que o modelo entregou num nível.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LevelStats {
    /// Concluídas na primeira entrega.
    pub first: u32,
    /// Concluídas depois de devolvidas.
    pub returned: u32,
    /// Quantas devoluções (revisão ou validação) essas e as demais tarefas tiveram, no total.
    pub returns: u32,
    pub discarded: u32,
    /// Tarefas reprovadas na validação (já contadas em `first`/`returned` se foram mescladas depois).
    pub rejected: u32,
    /// Descartada ou devolvida [`BAD_RETURNS`] vezes ou mais.
    bad: u32,
    /// As entregas na ordem em que terminaram: `true` = de primeira.
    recent: Vec<bool>,
}

/// Devolvida tantas vezes ou mais, a tarefa conta como mal-sucedida.
pub const BAD_RETURNS: usize = 2;

impl LevelStats {
    /// As tarefas que já têm desfecho (concluídas ou descartadas).
    pub fn total(&self) -> u32 {
        self.first + self.returned + self.discarded
    }

    /// Amostra suficiente e mais da metade das tarefas descartada ou devolvida 2 vezes ou mais: o modelo não dá
    /// conta deste nível.
    pub fn demotes(&self) -> bool {
        self.total() >= MIN_SAMPLE && self.bad * 2 > self.total()
    }

    /// As [`MIN_SAMPLE`] últimas tarefas do nível foram todas de primeira: o modelo dá conta deste nível.
    pub fn promotes(&self) -> bool {
        let n = MIN_SAMPLE as usize;
        self.recent.len() >= n && self.recent[self.recent.len() - n..].iter().all(|ok| *ok)
    }

    /// "4 de 5 de primeira, 1 devolvida, 1 descartada"; vazio sem tarefa.
    pub fn text(&self) -> String {
        if self.total() == 0 {
            return String::new();
        }
        let mut parts = vec![format!("{} de {} de primeira", self.first, self.total())];
        if self.returned > 0 {
            let times = if self.returns == 1 { "1 devolução".to_owned() } else { format!("{} devoluções", self.returns) };
            parts.push(format!("{} concluída{} depois de devolvida ({times})", self.returned, if self.returned == 1 { "" } else { "s" }));
        }
        if self.discarded > 0 {
            parts.push(format!("{} descartada{}", self.discarded, if self.discarded == 1 { "" } else { "s" }));
        }
        if self.rejected > 0 {
            parts.push(format!("{} rejeitada{} na validação", self.rejected, if self.rejected == 1 { "" } else { "s" }));
        }
        parts.join(", ")
    }
}

/// A medida de um modelo, por nível (na ordem de [`Level::ALL`]).
pub type Record = [LevelStats; 3];

/// Mede cada modelo pelas tarefas: só entram as que o agente chegou a executar e que já têm desfecho
/// (mesclada, em validação ou descartada). Uma tarefa sem nível gravado conta como `media`. As tarefas vêm
/// em qualquer ordem; a sequência de acertos segue a de término.
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
        stats.rejected += u32::from(task.feedbacks.iter().any(|f| f.origin == "validação"));
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

/// O nível que o modelo alcança, e por quê.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capacity {
    pub level: Level,
    /// O que mudou em relação à tabela (esforço baixo, medida), para o motivo registrado.
    pub notes: Vec<String>,
}

/// O nível que vale para um modelo: o da tabela, menos um com esforço `low` (a nota é do esforço máximo), e então
/// a medida: sobe quem acertou de primeira as últimas tarefas do nível acima; desce quem falha no nível em que está.
pub fn capacity(table_level: Level, low_effort: bool, record: Option<&Record>) -> Capacity {
    let mut level = table_level;
    let mut notes = Vec::new();
    if low_effort && level > Level::Simple {
        level = level.down();
        notes.push(format!("esforço baixo: desce para {}", level.as_str()));
    }
    if let Some(record) = record {
        while let Some(next) = level.up().filter(|n| record[n.index()].promotes()) {
            level = next;
            notes.push(format!("medida: {MIN_SAMPLE} tarefas {} seguidas de primeira, sobe para {}", next.as_str(), next.as_str()));
        }
        while level > Level::Simple && record[level.index()].demotes() {
            notes.push(format!("medida: falhou em {}, desce para {}", level.as_str(), level.down().as_str()));
            level = level.down();
        }
    }
    Capacity { level, notes }
}

// ---------- A escolha ----------

/// Um modelo que pode receber a tarefa.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub kind: String,
    /// O modelo (pedido ou presumido) que o motivo cita.
    pub model: Option<String>,
    /// A fração da cota usada; `None` = sem leitura.
    pub used: Option<f32>,
    /// O maior nível que o modelo alcança.
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

/// O escolhido, o motivo para registrar na tarefa e o aviso quando a escolha não é a ideal.
#[derive(Debug, Clone, PartialEq)]
pub struct Selection {
    pub kind: String,
    pub used: Option<f32>,
    pub reason: String,
    pub warning: Option<String>,
}

/// O candidato de `pool` com mais folga na cota.
fn by_quota<'a>(pool: &[&'a Candidate]) -> Option<&'a Candidate> {
    let list: Vec<AgentQuotaCandidate> = pool.iter().map(|c| AgentQuotaCandidate { kind: c.kind.clone(), used: c.used }).collect();
    let picked = select_agent_by_quota(&list)?;
    pool.iter().find(|c| c.kind == picked.kind).copied()
}

/// Entre os candidatos capazes do `level`, o de mais folga na cota (a regra de [`select_agent_by_quota`]).
/// Sem nenhum capaz com cota, o capaz menos gasto, com aviso; sem nenhum capaz, o que mais alcança (desempate
/// pela cota), com aviso. Nunca desce a um modelo incapaz sem dizer.
pub fn select(candidates: &[Candidate], level: Level) -> Option<Selection> {
    let capable: Vec<&Candidate> = candidates.iter().filter(|c| c.capacity >= level).collect();
    let with_quota: Vec<&Candidate> = capable.iter().copied().filter(|c| c.has_quota()).collect();
    let (chosen, warning) = if let Some(c) = by_quota(&with_quota) {
        (c, None)
    } else if let Some(c) = by_quota(&capable) {
        (c, Some(format!("nenhum modelo capaz de tarefa {} tem cota; usei {}, o menos gasto", level.as_str(), c.label())))
    } else {
        let best = candidates.iter().map(|c| c.capacity).max()?;
        let top: Vec<&Candidate> = candidates.iter().filter(|c| c.capacity == best).collect();
        let usable: Vec<&Candidate> = top.iter().copied().filter(|c| c.has_quota()).collect();
        let c = by_quota(&usable).or_else(|| by_quota(&top))?;
        (c, Some(format!("nenhum modelo disponível dá conta de tarefa {}; {} vai até {}", level.as_str(), c.label(), best.as_str())))
    };
    let mut parts = vec![match chosen.used {
        _ if chosen.capacity < level => format!("{}, o que mais alcança (até {})", chosen.label(), chosen.capacity.as_str()),
        Some(used) if chosen.has_quota() => format!("{}, capaz e com {:.0}% livre", chosen.label(), (1.0 - used).clamp(0.0, 1.0) * 100.0),
        Some(_) => format!("{}, capaz e sem cota", chosen.label()),
        None => format!("{}, capaz (sem leitura de cota)", chosen.label()),
    }];
    for other in candidates.iter().filter(|c| c.kind != chosen.kind) {
        parts.push(if other.capacity < level {
            format!("{} só vai até {}", other.label(), other.capacity.as_str())
        } else {
            match other.used {
                Some(u) => format!("{} {:.0}% usada", other.label(), u * 100.0),
                None => format!("{} sem leitura de cota", other.label()),
            }
        });
    }
    let reason = format!("{}: {}", level.as_str(), parts.join("; "));
    Some(Selection { kind: chosen.kind.clone(), used: chosen.used, reason, warning })
}

/// O aviso quando a tarefa está acima do que o modelo alcança; `None` se ele dá conta.
pub fn advisory(label: &str, capacity: &Capacity, level: Level) -> Option<String> {
    (capacity.level < level).then(|| {
        let why = if capacity.notes.is_empty() { String::new() } else { format!(" ({})", capacity.notes.join("; ")) };
        format!("a tarefa é {} e {label} só vai até {}{why}", level.as_str(), capacity.level.as_str())
    })
}

// ---------- No Core ----------

impl Core {
    /// A tabela de capacidade (a embutida com a de `dados/capacidade.json` por cima), lida a cada chamada.
    pub fn capability_table(&self) -> Table {
        Table::load(&self.paths().data_dir)
    }

    /// A medida de cada modelo pelas tarefas de todos os projetos (modelo → nível → desfechos).
    pub fn capability_records(&self, table: &Table) -> HashMap<String, Record> {
        let tasks: Vec<Task> = self.projects().iter().flat_map(|p| self.tasks(&p.id)).collect();
        measure(&tasks, table)
    }

    /// O que o modelo (`kind` com `model`, ou o presumido) alcança, pela tabela, pelo esforço e pela medida.
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

    /// Os candidatos da escolha automática: os CLIs oficiais disponíveis em `choices`, cada um com o modelo que
    /// abriria (`role_models` ou o presumido pela tabela), a cota e o que alcança.
    pub fn capability_candidates(&self, choices: &[String], effort: Option<crate::models::Effort>) -> Vec<Candidate> {
        let (table, used) = (self.capability_table(), self.quota_used());
        let records = self.capability_records(&table);
        let role_models = self.config().role_models;
        ["claude", "antigravity", "codex"]
            .iter()
            .filter(|kind| choices.iter().any(|c| c == **kind))
            .map(|kind| {
                let model = crate::models::resolve_model(&crate::instances::Ident::main(kind), None, None, &role_models);
                let low = effort == Some(crate::models::Effort::Low) && crate::models::effort_unavailable(kind, model.as_deref()).is_none();
                let capacity = self.model_capacity(&table, &records, kind, model.as_deref(), low);
                Candidate {
                    kind: (*kind).to_owned(),
                    model: table.assumed_model(kind, model.as_deref()),
                    used: used.get(*kind).copied(),
                    capacity: capacity.level,
                }
            })
            .collect()
    }

    /// O aviso de capacidade de uma escolha explícita (ou do formulário): a tarefa está acima do que o modelo alcança.
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

    /// As linhas de capacidade do `--models`, uma por modelo (da tabela, dos listados em `items` e dos medidos):
    /// "Claude Sonnet 5.5 (sonnet): complexa, Coding Agent Index 59.3 (2026-10) · medida media: 4 de 5 de primeira".
    pub fn capability_lines(&self, items: &[crate::models::AgentModels]) -> Vec<String> {
        let table = self.capability_table();
        let records = self.capability_records(&table);
        let role_models = self.config().role_models;
        // (chave da medida, nome do modelo, ids com que ele aparece), na ordem em que aparecem.
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
                    None => format!("{name}: fora da tabela, simples"),
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
                    line.push_str(&format!(" · medida {}", measured.join(" | ")));
                }
                if eff.level != table_level {
                    line.push_str(&format!(" · vale {} ({})", eff.level.as_str(), eff.notes.join("; ")));
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
        // Fora da tabela: simples. Um prefixo sem separador não casa.
        assert_eq!(level("modelo-novo"), Level::Simple);
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
        assert_eq!(t.key("goose", None), "goose padrão");
    }

    #[test]
    fn the_user_file_overrides_by_name_and_adds_models() {
        let dir = tempfile::tempdir().expect("tmp");
        std::fs::write(
            dir.path().join(OVERRIDE_FILE),
            r#"{"defaults":{"codex":"gpt-5.6-terra"},"models":[
                {"name":"gpt-6 luna","aliases":["gpt-6-luna"],"score":50.0,"source":"meu teste","date":"2026-11"},
                {"name":"Modelo Novo","aliases":["novo"],"level":"complexa","source":"x","date":"2026-11"}]}"#,
        )
        .expect("grava");
        let t = Table::load(dir.path());
        assert_eq!(t.models.len(), 19);
        assert_eq!(t.table_level(Some("gpt-6-luna")), Level::Medium);
        assert_eq!(t.table_level(Some("novo")), Level::Complex);
        assert_eq!(t.assumed_model("codex", None).as_deref(), Some("gpt-5.6-terra"));
        assert_eq!(t.assumed_model("claude", None).as_deref(), Some("sonnet"));
        // Um arquivo ilegível não derruba a tabela embutida.
        std::fs::write(dir.path().join(OVERRIDE_FILE), "{ não é json").expect("grava");
        assert_eq!(Table::load(dir.path()), Table::builtin());
        assert_eq!(Table::load(&dir.path().join("nao-existe")), Table::builtin());
    }

    #[test]
    fn the_measure_counts_outcomes_per_model_and_level() {
        let t = table();
        let tasks = vec![
            task("claude", Some("sonnet"), Level::Medium, TaskStatus::Merged, &[], 1),
            task("claude", None, Level::Medium, TaskStatus::Validating, &["revisão"], 2),
            task("claude", Some("sonnet"), Level::Medium, TaskStatus::Merged, &["revisão", "validação"], 3),
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
            "1 de 4 de primeira, 2 concluídas depois de devolvida (3 devoluções), 1 descartada, 1 rejeitada na validação"
        );
        assert_eq!(rec["Claude Sonnet 5.5"][Level::Complex.index()].text(), "1 de 1 de primeira");
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
        // Mais da metade descartada ou devolvida duas vezes ou mais: 2 de 3.
        let tasks = vec![
            task("claude", Some("sonnet"), Level::Complex, TaskStatus::Discarded, &[], 1),
            task("claude", Some("sonnet"), Level::Complex, TaskStatus::Merged, &["revisão", "revisão"], 2),
            task("claude", Some("sonnet"), Level::Complex, TaskStatus::Merged, &[], 3),
        ];
        let rec = measure(&tasks, &t);
        let c = capacity(Level::Complex, false, rec.get("Claude Sonnet 5.5"));
        assert_eq!(c.level, Level::Medium);
        assert_eq!(c.notes.len(), 1);
        // Metade exata não derruba; uma devolução só não é falha.
        let tasks = vec![
            task("claude", Some("sonnet"), Level::Complex, TaskStatus::Discarded, &[], 1),
            task("claude", Some("sonnet"), Level::Complex, TaskStatus::Merged, &["revisão"], 2),
            task("claude", Some("sonnet"), Level::Complex, TaskStatus::Merged, &[], 3),
            task("claude", Some("sonnet"), Level::Complex, TaskStatus::Merged, &[], 4),
        ];
        let rec = measure(&tasks, &t);
        assert_eq!(capacity(Level::Complex, false, rec.get("Claude Sonnet 5.5")).level, Level::Complex);
        // Falhar no media derruba também o que já tinha descido, até o simples.
        let tasks: Vec<Task> = (1..=6)
            .map(|n| task("claude", Some("sonnet"), if n <= 3 { Level::Complex } else { Level::Medium }, TaskStatus::Discarded, &[], n))
            .collect();
        let rec = measure(&tasks, &t);
        assert_eq!(capacity(Level::Complex, false, rec.get("Claude Sonnet 5.5")).level, Level::Simple);
        // O simples é o piso.
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
        // Uma devolvida no meio da sequência (as 3 últimas) não promove.
        let rec = measure(&[win(1), task("codex", Some("gpt-6-luna"), Level::Medium, TaskStatus::Merged, &["revisão"], 2), win(3)], &t);
        assert_eq!(capacity(Level::Simple, false, rec.get("GPT-6 Luna")).level, Level::Simple);
        // Mas um tropeço antigo, fora das 3 últimas, não impede.
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
            "complexa: antigravity antigravity-m, capaz e com 50% livre; claude claude-m 74% usada; codex codex-m só vai até simples"
        );
        // Em tarefa simples todos são capazes e a folga decide.
        assert_eq!(select(&c, Level::Simple).expect("escolha").kind, "codex");
        // Sem leitura de cota perde para quem tem folga, mas é capaz.
        let c = [candidate("claude", None, Level::Complex), candidate("antigravity", Some(0.9), Level::Complex)];
        assert_eq!(select(&c, Level::Medium).expect("escolha").kind, "antigravity");
        let c = [candidate("claude", None, Level::Complex), candidate("codex", Some(0.1), Level::Simple)];
        let s = select(&c, Level::Complex).expect("escolha");
        assert_eq!((s.kind.as_str(), s.warning), ("claude", None));
        assert!(s.reason.contains("capaz (sem leitura de cota)"), "{}", s.reason);
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
        assert!(warning.contains("nenhum modelo capaz de tarefa complexa tem cota") && warning.contains("antigravity"), "{warning}");
        assert!(s.reason.contains("sem cota"), "{}", s.reason);
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
        assert!(warning.contains("nenhum modelo disponível dá conta de tarefa complexa") && warning.contains("vai até media"), "{warning}");
        assert!(select(&[], Level::Medium).is_none());
    }

    #[test]
    fn the_advisory_appears_only_above_the_capacity() {
        let low = Capacity { level: Level::Simple, notes: vec!["esforço baixo: desce para simples".into()] };
        assert_eq!(
            advisory("codex gpt-6-luna", &low, Level::Complex).as_deref(),
            Some("a tarefa é complexa e codex gpt-6-luna só vai até simples (esforço baixo: desce para simples)")
        );
        assert_eq!(advisory("codex gpt-6-luna", &low, Level::Simple), None);
    }
}
