//! LLM model of each instance (`claude@dev` runs `sonnet`).
//!
//! The model is a property of the instance, **not** part of the id: the id is still `model[@role[@name]]`
//! and the model sits next to it, in `ProjectCfg.tab_models` (id → model), the same shape as `tab_parents`.
//! When opening an instance, the order is: the requested model (`ask-agent --model`, menu), the one saved in
//! `tab_models` (reopening after closing the app), the default from `Config.role_models` (`model@role`
//! wins over `model`) and, at last, the CLI's own default (no argument). Here live the pure rules
//! (with tests) and the `impl Core` that applies them; the bridge and the interface only call `Core`.

use std::collections::BTreeMap;
use std::process::Command;
use std::time::{Duration, Instant};

use crate::instances::Ident;
use crate::t;
use crate::Core;

/// Maximum length of a model name.
pub const MAX_MODEL_LEN: usize = 128;

/// Model aliases accepted by `claude --model`.
pub const CLAUDE_MODELS: [&str; 4] = ["opus", "sonnet", "haiku", "fable"];

/// Model name that is safe for the command line, the JSON and text typed in other tabs: letters,
/// digits and `: . _ / # @ + -` (e.g. `sonnet`, `opencode/glm-5.3-flash-free`, `provider/model#variant`).
pub fn valid_model(s: &str) -> bool {
    !s.is_empty() && s.len() <= MAX_MODEL_LEN && s.chars().all(|c| c.is_ascii_alphanumeric() || ":._/#@+-".contains(c))
}

/// The agent accepts choosing the model (`--model` or, in OpenCode, the configuration); the other
/// agents have no such option.
pub fn accepts_model(kind: &str) -> bool {
    matches!(kind, "claude" | "antigravity" | "codex" | "opencode")
}

/// `Config.role_models` default for the instance: `model@role` wins over `model`. Entries with an
/// invalid model name are ignored.
pub fn default_model(role_models: &BTreeMap<String, String>, ident: &Ident) -> Option<String> {
    let by_role = ident.role.as_ref().map(|r| format!("{}@{r}", ident.kind));
    by_role.iter().chain(std::iter::once(&ident.kind)).find_map(|k| role_models.get(k).filter(|m| valid_model(m)).cloned())
}

/// The model the instance opens with: the requested one, else the saved one, else the role default.
/// `None` = the CLI default (or an agent that does not choose a model).
pub fn resolve_model(
    ident: &Ident,
    explicit: Option<&str>,
    persisted: Option<&str>,
    role_models: &BTreeMap<String, String>,
) -> Option<String> {
    if !accepts_model(&ident.kind) {
        return None;
    }
    let valid = |m: Option<&str>| m.filter(|m| valid_model(m)).map(str::to_owned);
    valid(explicit).or_else(|| valid(persisted)).or_else(|| default_model(role_models, ident))
}

/// The 409 message when the instance `id` is already open with a model other than the requested one.
/// `None` = no conflict (the same model).
pub fn conflict(id: &str, current: Option<&str>, requested: &str) -> Option<String> {
    if current == Some(requested) {
        return None;
    }
    let current = current.unwrap_or("the default model");
    Some(format!("{id} is already open with {current}; use --new to open another one with {requested}"))
}

/// The instance `old` was renamed to `new`: the model goes along.
pub fn rename(models: &mut BTreeMap<String, String>, old: &str, new: &str) {
    if let Some(m) = models.remove(old) {
        models.insert(new.to_owned(), m);
    }
}

/// Arguments that choose the model on `kind`'s command line (tab or interactive headless).
/// OpenCode has no option in the TUI: see [`opencode_tui`].
pub fn cli_args(kind: &str, model: &str) -> Vec<String> {
    match kind {
        "claude" | "antigravity" | "codex" => vec!["--model".into(), model.into()],
        _ => Vec::new(),
    }
}

/// How the CLI receives an initial interactive prompt when it opens (the text is not typed in the terminal, so
/// it does not depend on the TUI being ready). `None` = the CLI has no such option: delivery keeps being typed.
/// `Some(None)` = positional argument (Claude: `claude "<prompt>"`); `Some(Some(flag))` = `flag "<prompt>"`.
pub fn boot_prompt_flag(kind: &str) -> Option<Option<&'static str>> {
    match kind {
        "claude" => Some(None),
        "antigravity" => Some(Some("-i")),
        "opencode" => Some(Some("--prompt")),
        _ => None,
    }
}

/// Like [`cli_args`], for the headless run: `opencode run` has `-m provider/model[#variant]`.
pub fn headless_args(kind: &str, model: &str) -> Vec<String> {
    match kind {
        "opencode" => vec!["-m".into(), model.into()],
        _ => cli_args(kind, model),
    }
}

/// How to open the OpenCode TUI with the model and the AgentDeck instructions: `(arguments, environment variable)`.
///
/// The TUI has no `--model` (only `run` and `mini` have `-m`) and no instructions option. The configuration in
/// `OPENCODE_CONFIG_CONTENT` (`{"model": "...", "instructions": ["<file>"]}`) wins over the user's and the
/// project's configuration, and the `instructions` add to theirs (the project's `AGENTS.md` is still read). But it
/// is only read by whoever starts the server, and by default the TUI uses the shared background service (which
/// ignores our process's variable). Hence `--standalone`: the tab's own server, with our environment. Checked
/// with OpenCode 2.0.32 (the TUI status line shows the requested model; `opencode run --standalone` with the
/// configuration obeys a rule from the instructions file). The variant (`model#high`) does not exist in the
/// configuration, only in `run -m`: here only the model goes.
pub fn opencode_tui(model: Option<&str>, instructions: Option<&std::path::Path>) -> (Vec<String>, (String, String)) {
    let mut config = serde_json::Map::new();
    if let Some(model) = model {
        config.insert("model".into(), model.split('#').next().unwrap_or(model).into());
    }
    if let Some(file) = instructions {
        config.insert("instructions".into(), serde_json::json!([file.to_string_lossy()]));
    }
    (vec!["--standalone".into()], ("OPENCODE_CONFIG_CONTENT".into(), serde_json::Value::Object(config).to_string()))
}

/// Reasoning effort of a task. Each CLI translates it in its own way ([`effort_args`], [`effort_model`]);
/// "Default" is the absence (`Option<Effort>` = `None`): nothing goes to the CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Low,
    Medium,
    High,
}

impl Effort {
    pub const ALL: [Effort; 3] = [Effort::Low, Effort::Medium, Effort::High];

    /// The value of `--effort`, of the JSON and of the Antigravity model suffix.
    pub fn as_str(self) -> &'static str {
        match self {
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
        }
    }

    /// The name in the interface.
    pub fn label(self) -> &'static str {
        match self {
            Effort::Low => t!("low"),
            Effort::Medium => t!("medium"),
            Effort::High => t!("high"),
        }
    }

    /// Reads `low|medium|high` (or `baixo|médio|alto`), case-insensitive.
    pub fn parse(s: &str) -> Option<Effort> {
        match s.trim().to_lowercase().as_str() {
            "low" | "baixo" => Some(Effort::Low),
            "medium" | "médio" | "medio" => Some(Effort::Medium),
            "high" | "alto" => Some(Effort::High),
            _ => None,
        }
    }
}

/// The Antigravity models carry the effort in the name: `…-low`, `…-medium` or `…-high`.
fn agy_effort_split(model: &str) -> Option<(&str, Effort)> {
    Effort::ALL.into_iter().find_map(|e| model.strip_suffix(e.as_str())?.strip_suffix('-').map(|base| (base, e)))
}

/// Why the effort does not apply to `kind` with `model` (the interface hint and the request error); `None` = it applies.
/// Claude: `--effort`. Antigravity: it swaps the model name suffix, so it needs a model with a suffix.
/// OpenCode: the variant (`model#variant`) only exists in `run -m`; the interactive tab does not get it.
pub fn effort_unavailable(kind: &str, model: Option<&str>) -> Option<&'static str> {
    match kind {
        "claude" => None,
        "antigravity" => match model {
            None => Some(t!("Pick a model: in Antigravity the effort is part of its name")),
            Some(m) if agy_effort_split(m).is_none() => Some(t!("This Antigravity model has no effort variants")),
            Some(_) => None,
        },
        "opencode" => Some(t!("OpenCode only accepts effort in headless mode; the tab does not get it")),
        _ => Some(t!("This agent has no effort")),
    }
}

/// Arguments that pass the effort to the CLI on the command line (only Claude has the option; Antigravity uses
/// [`effort_model`]).
pub fn effort_args(kind: &str, effort: Effort) -> Vec<String> {
    match kind {
        "claude" => vec!["--effort".into(), effort.as_str().into()],
        _ => Vec::new(),
    }
}

/// The Antigravity model with the effort suffix swapped (`gemini-4-argon-low` + high →
/// `gemini-4-argon-high`). Any other case returns the model as it came.
pub fn effort_model(kind: &str, model: &str, effort: Effort) -> String {
    match (kind, agy_effort_split(model)) {
        ("antigravity", Some((base, _))) => format!("{base}-{}", effort.as_str()),
        _ => model.to_owned(),
    }
}

/// Short form for labels: without the provider (`opencode/glm-5.3` → `glm-5.3`).
pub fn short_model(model: &str) -> &str {
    model.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or(model)
}

/// The provider of a model: the prefix before the `/` (`opencode-go/deepseek-v4.1-flash` →
/// `opencode-go`). `None` for the models that do not carry a provider in the name (`sonnet`, `haiku`).
pub fn provider(model: &str) -> Option<&str> {
    model.split_once('/').map(|(p, _)| p).filter(|p| !p.is_empty())
}

/// The list with the free ones (`-free`) first, keeping the original order inside each group
/// (stable sort). This is what the model lists show.
pub fn free_first(models: &[String]) -> Vec<String> {
    let mut out = models.to_vec();
    out.sort_by_key(|m| !is_free(m));
    out
}

/// The model list grouped by provider, in the order the providers appear, with the
/// free ones (`-free`) first inside each group.
///
/// Only `opencode models` returns `provider/model`; the other agents list plain names, so
/// their list comes back empty and the interface shows the usual list. Models without a provider in the name
/// stay out of the groups.
pub fn by_provider(models: &[String]) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for model in models {
        let Some(provider) = provider(model) else { continue };
        match out.iter_mut().find(|(p, _)| p == provider) {
            Some((_, list)) => list.push(model.clone()),
            None => out.push((provider.to_owned(), vec![model.clone()])),
        }
    }
    for (_, list) in &mut out {
        *list = free_first(list);
    }
    out
}

/// The first step of the model menu: with **a single provider** there is nothing to choose, so its models
/// appear directly, without the provider step. `None` = the provider list comes first (or there are
/// no groups, as with Claude and Antigravity).
pub fn sole_provider(groups: &[(String, Vec<String>)]) -> Option<&str> {
    match groups {
        [(provider, _)] => Some(provider),
        _ => None,
    }
}

/// Free model (`…-free`, the `#variant` does not count): no cost per token, to highlight in
/// the OpenCode lists.
pub fn is_free(model: &str) -> bool {
    short_model(model).split('#').next().unwrap_or_default().ends_with("-free")
}

/// `agy models`: one line per model, `id<TAB>name` (plus a warning line without a tab).
pub fn parse_agy_models(out: &str) -> Vec<String> {
    let mut seen = Vec::new();
    for id in out.lines().filter(|l| l.contains('\t')).filter_map(|l| l.split('\t').next()).map(str::trim) {
        if valid_model(id) && !seen.iter().any(|s| s == id) {
            seen.push(id.to_owned());
        }
    }
    seen
}

/// `opencode models`: one `provider/model` per line.
pub fn parse_opencode_models(out: &str) -> Vec<String> {
    let mut seen = Vec::new();
    for id in out.lines().map(str::trim) {
        if valid_model(id) && id.contains('/') && !seen.iter().any(|s| s == id) {
            seen.push(id.to_owned());
        }
    }
    seen
}

/// What is known about the models of an installed agent (`ask-agent --models`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelList {
    /// The models that `--model` accepts.
    Models(Vec<String>),
    /// Accepts `--model <name>`, but this computer does not list the names (Codex).
    Unlisted,
    /// Does not choose a model: uses the CLI default.
    Fixed,
    /// The listing failed: the CLI did not answer in time or the list came back empty.
    Failed,
}

/// An installed agent and its models, as `--agent` and `--model` accept them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentModels {
    pub agent: String,
    pub list: ModelList,
}

/// The agents whose models the CLI or the fixed list informs (the others have no list).
pub fn has_model_list(kind: &str) -> bool {
    matches!(kind, "claude" | "antigravity" | "opencode")
}

/// The inventory entry of `kind`, given the list [`Core::known_models`] returned (empty = it failed).
pub fn classify(kind: &str, models: Vec<String>) -> AgentModels {
    let list = if has_model_list(kind) {
        if models.is_empty() {
            ModelList::Failed
        } else {
            ModelList::Models(models)
        }
    } else if accepts_model(kind) {
        ModelList::Unlisted
    } else {
        ModelList::Fixed
    };
    AgentModels { agent: kind.to_owned(), list }
}

/// `a, b (free), c`: the free ones first and marked.
fn join_marked(models: &[String], strip: impl Fn(&str) -> &str) -> String {
    free_first(models)
        .iter()
        .map(|m| if is_free(m) { format!("{} (free)", strip(m)) } else { strip(m).to_owned() })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The text of `ask-agent --models`: one line per agent (OpenCode, one per provider), short enough for
/// an agent to read, and the quota of whoever has one (`quotas`: agent → line, from [`Core::quota_lines`]).
/// Pure function (tested); whoever fetches the data is [`Core::models_inventory`].
///
/// This text is agent-facing: it is always English, without `t!`. Its shape is also parsed:
/// `crates/cli/src/bin/ask-agent.rs` (`quota_usage_from_inventory`) goes by the ` --model` heading and by
/// the `  quota:` prefix to choose a better agent by quota.
pub fn format_inventory(items: &[AgentModels], quotas: &std::collections::HashMap<String, String>) -> String {
    if items.is_empty() {
        return "No agent installed on this computer.".to_owned();
    }
    let mut lines = Vec::new();
    for item in items {
        let agent = &item.agent;
        match &item.list {
            ModelList::Models(models) => {
                let groups = by_provider(models);
                if groups.is_empty() {
                    lines.push(format!("{agent} --model: {}", join_marked(models, |m| m)));
                } else {
                    lines.push(format!("{agent} --model <provider>/<model>:"));
                    lines.extend(
                        groups
                            .iter()
                            .map(|(p, list)| format!("  {p}/: {}", join_marked(list, |m| m.strip_prefix(&format!("{p}/")).unwrap_or(m)))),
                    );
                }
            }
            ModelList::Unlisted => lines.push(format!("{agent} --model <name> (this computer does not list the models)")),
            ModelList::Fixed => lines.push(format!("{agent}: no model choice (uses the CLI default)")),
            ModelList::Failed => {
                lines.push(format!("{agent}: could not list the models (the CLI did not answer); without --model it uses the default"))
            }
        }
        if let Some(q) = quotas.get(agent) {
            lines.push(format!("  quota: {q}"));
        } else if accepts_model(agent) {
            lines.push("  quota: no reading".to_owned());
        }
    }
    lines.join("\n")
}

/// Orders the official agents with the most headroom first (smallest fraction used); those without a reading come
/// after those with headroom and before the exhausted ones, keeping the original order on a tie.
pub fn sort_by_quota(items: &mut [AgentModels], used: &std::collections::HashMap<String, f32>) {
    items.sort_by_key(|item| {
        if !accepts_model(&item.agent) {
            return (1, 0, 0);
        }
        match used.get(&item.agent).copied() {
            Some(u) if u < crate::usage::EXHAUSTED => (0, 0, (u.clamp(0.0, 1.0) * 10_000.0).round() as i32),
            None => (0, 1, 0),
            Some(u) => (0, 2, (u.clamp(0.0, 1.0) * 10_000.0).round() as i32),
        }
    });
}

/// How long an agent's model list stays cached.
const LIST_TTL: Duration = Duration::from_secs(600);
/// A failed query is only retried after this.
const LIST_RETRY: Duration = Duration::from_secs(60);
/// Time limit of `agy models` / `opencode models`.
const LIST_TIMEOUT: Duration = Duration::from_secs(15);

/// A cached query: when, whether it worked and the list.
type CachedList = (Instant, bool, Vec<String>);

/// Cache of the model lists per agent.
#[derive(Default)]
pub(crate) struct ModelLists {
    lists: parking_lot::Mutex<std::collections::HashMap<String, CachedList>>,
}

impl ModelLists {
    fn fresh(&self, kind: &str) -> Option<Vec<String>> {
        let lists = self.lists.lock();
        let (at, ok, list) = lists.get(kind)?;
        (at.elapsed() < if *ok { LIST_TTL } else { LIST_RETRY }).then(|| list.clone())
    }
}

impl Core {
    /// Effective model of the instance (saved or role default), without saving anything. `None` = the CLI default.
    pub fn instance_model(&self, project: &str, ident: &Ident) -> Option<String> {
        let cfg = self.inner.config.read();
        let persisted = cfg.project(project).and_then(|p| p.tab_models.get(&ident.id()));
        resolve_model(ident, None, persisted.map(String::as_str), &cfg.role_models)
    }

    /// Resolves the model the instance opens with (requested > saved > role default) and **saves** it
    /// in `tab_models`, so it shows in the interface and comes back after reopening the app. Idempotent.
    pub fn assign_model(&self, project: &str, ident: &Ident, explicit: Option<&str>) -> Option<String> {
        let id = ident.id();
        let (resolved, persisted) = {
            let cfg = self.inner.config.read();
            let persisted = cfg.project(project).and_then(|p| p.tab_models.get(&id).cloned());
            (resolve_model(ident, explicit, persisted.as_deref(), &cfg.role_models), persisted)
        };
        if resolved != persisted {
            self.update_project(project, |p| match &resolved {
                Some(m) => {
                    p.tab_models.insert(id, m.clone());
                }
                None => {
                    p.tab_models.remove(&id);
                }
            });
        }
        resolved
    }

    /// Forgets the instance's saved model (it closed or never got to open).
    pub(crate) fn forget_model(&self, project: &str, id: &str) {
        if self.inner.config.read().project(project).is_some_and(|p| p.tab_models.contains_key(id)) {
            self.update_project(project, |p| {
                p.tab_models.remove(id);
            });
        }
    }

    /// Model of each instance open in the project that has one (id → model), for lists and `--team`.
    pub fn instance_models(&self, project: &str) -> BTreeMap<String, String> {
        self.project_instance_ids(project)
            .into_iter()
            .filter_map(|id| {
                let ident = Ident::parse(&id)?;
                Some((id, self.instance_model(project, &ident)?))
            })
            .collect()
    }

    /// New instance opened by the user ("+" menu) already with the chosen model (`None` = default).
    /// Error if the model is invalid or the agent does not choose a model.
    pub fn register_instance_with_model(&self, project: &str, kind: &str, model: Option<&str>) -> Result<Ident, String> {
        if let Some(m) = model {
            if !valid_model(m) {
                return Err(t!("invalid model: \"{model}\"", model = m));
            }
            if !accepts_model(kind) {
                return Err(t!("the {kind} agent does not accept choosing a model", kind));
            }
        }
        let ident = self.register_instance(project, kind);
        self.assign_model(project, &ident, model);
        Ok(ident)
    }

    /// Models known for an agent, for the new-agent menu. Claude: the aliases; Antigravity and
    /// OpenCode: what the CLI lists (`agy models`, `opencode models`), cached. BLOCKING (up to 15 s
    /// the first time): call it outside the interface thread. Empty = only the default.
    pub fn known_models(&self, kind: &str) -> Vec<String> {
        let parse: fn(&str) -> Vec<String> = match kind {
            "claude" => return CLAUDE_MODELS.map(String::from).to_vec(),
            "antigravity" => parse_agy_models,
            "opencode" => parse_opencode_models,
            _ => return Vec::new(),
        };
        if let Some(list) = self.inner.model_lists.fresh(kind) {
            return list;
        }
        let list = self.inner.agents.resolve(kind).and_then(|r| {
            let (program, pre) = r.command();
            let mut cmd = Command::new(program);
            cmd.args(pre).arg("models").current_dir(&self.inner.home);
            let out = crate::process::run_captured(cmd, None, LIST_TIMEOUT, None, None).ok()?;
            let list = parse(&out.stdout);
            (!list.is_empty()).then_some(list)
        });
        let ok = list.is_some();
        let list = list.unwrap_or_default();
        self.inner.model_lists.lists.lock().insert(kind.to_owned(), (Instant::now(), ok, list.clone()));
        list
    }

    /// The installed agents and their models (`ask-agent --models`), with the [`Core::known_models`] cache.
    /// `only` limits it to one agent (accepts aliases such as `agy`). The CLI queries
    /// run together, so it takes as long as the slowest one (up to `LIST_TIMEOUT`). BLOCKING.
    /// Error = the requested agent is not installed.
    pub fn models_inventory(&self, only: Option<&str>) -> Result<Vec<AgentModels>, String> {
        let agents = &self.inner.agents;
        let mut kinds: Vec<String> =
            crate::agents::AGENTS.iter().filter(|a| a.peer && agents.resolve(a.id).is_some()).map(|a| a.id.to_owned()).collect();
        if let Some(only) = only {
            let name = crate::agents::resolve_alias(only);
            if !kinds.iter().any(|k| k == name) {
                let have: Vec<&str> = kinds.iter().map(String::as_str).collect();
                return Err(format!("the agent \"{only}\" is not installed on this computer (installed: {})", have.join(", ")));
            }
            kinds.retain(|k| k == name);
        }
        let mut out: Vec<AgentModels> = std::thread::scope(|s| {
            let jobs: Vec<_> = kinds.iter().map(|k| s.spawn(move || classify(k, self.known_models(k)))).collect();
            jobs.into_iter().zip(&kinds).map(|(j, k)| j.join().unwrap_or_else(|_| classify(k, Vec::new()))).collect()
        });
        if only.is_none() {
            sort_by_quota(&mut out, &self.quota_used());
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> Ident {
        Ident::parse(s).expect("valid id")
    }

    fn defaults(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn model_names_are_validated() {
        for ok in ["sonnet", "opencode/glm-5.3-flash-free", "provider/model#high", "qwen3.5:9b", "a@b+c_d.e"] {
            assert!(valid_model(ok), "{ok}");
        }
        for bad in ["", "two names", "a\"b", "a&b", "a|b", "a%b", "a^b", "a<b", "a\nb", "\x1b[0m", "naïve", "$(x)", "a;b", "a`b"] {
            assert!(!valid_model(bad), "{bad:?}");
        }
        assert!(valid_model(&"a".repeat(128)));
        assert!(!valid_model(&"a".repeat(129)));
    }

    #[test]
    fn only_some_agents_choose_a_model() {
        for k in ["claude", "antigravity", "codex", "opencode"] {
            assert!(accepts_model(k), "{k}");
        }
        for k in ["shell", "aider", "goose", "copilot", "q", "interpreter"] {
            assert!(!accepts_model(k), "{k}");
        }
    }

    #[test]
    fn resolution_order_is_request_then_saved_then_role_then_kind_then_cli_default() {
        let cfg = defaults(&[("claude@dev", "sonnet"), ("claude", "opus"), ("opencode", "opencode/glm-5.3-flash-free")]);
        let dev = id("claude@dev");
        // a) the request wins over everything.
        assert_eq!(resolve_model(&dev, Some("haiku"), Some("fable"), &cfg).as_deref(), Some("haiku"));
        // b) with no request, the saved one.
        assert_eq!(resolve_model(&dev, None, Some("fable"), &cfg).as_deref(), Some("fable"));
        // c) with none, `model@role` wins over `model`.
        assert_eq!(resolve_model(&dev, None, None, &cfg).as_deref(), Some("sonnet"));
        assert_eq!(
            resolve_model(&id("claude@qa"), None, None, &cfg).as_deref(),
            Some("opus"),
            "a role with no entry falls back to the model"
        );
        assert_eq!(resolve_model(&id("claude"), None, None, &cfg).as_deref(), Some("opus"));
        assert_eq!(resolve_model(&id("claude@dev@ana"), None, None, &cfg).as_deref(), Some("sonnet"), "the name does not count");
        assert_eq!(resolve_model(&id("opencode@x"), None, None, &cfg).as_deref(), Some("opencode/glm-5.3-flash-free"));
        // d) nothing: the CLI default.
        assert_eq!(resolve_model(&id("antigravity"), None, None, &cfg), None);
        // Whoever does not choose a model never gets one, not even from the default.
        let all = defaults(&[("shell", "x"), ("aider", "x")]);
        assert_eq!(resolve_model(&id("shell"), Some("x"), None, &all), None);
        assert_eq!(resolve_model(&id("aider"), None, None, &all), None);
        // Invalid values (a hand-edited config) are ignored.
        let bad = defaults(&[("claude", "a b"), ("claude@dev", "ok;rm")]);
        assert_eq!(resolve_model(&dev, Some("x y"), Some("z&"), &bad), None);
        assert_eq!(resolve_model(&dev, None, Some("z&"), &defaults(&[("claude", "opus")])).as_deref(), Some("opus"));
    }

    #[test]
    fn a_different_model_is_a_conflict() {
        assert_eq!(conflict("claude@dev", Some("sonnet"), "sonnet"), None);
        assert_eq!(
            conflict("claude@dev", Some("sonnet"), "opus").as_deref(),
            Some("claude@dev is already open with sonnet; use --new to open another one with opus")
        );
        assert_eq!(
            conflict("claude@dev", None, "opus").as_deref(),
            Some("claude@dev is already open with the default model; use --new to open another one with opus")
        );
    }

    #[test]
    fn renaming_carries_the_model() {
        let mut m = defaults(&[("claude@dev", "sonnet"), ("claude@qa", "opus")]);
        rename(&mut m, "claude@dev", "claude@lead");
        assert_eq!(m, defaults(&[("claude@lead", "sonnet"), ("claude@qa", "opus")]));
        rename(&mut m, "does-not-exist", "x");
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn cli_args_per_agent() {
        for k in ["claude", "antigravity", "codex"] {
            assert_eq!(cli_args(k, "m1"), ["--model", "m1"], "{k}");
        }
        assert!(cli_args("opencode", "m1").is_empty(), "the OpenCode TUI has no --model");
        assert!(cli_args("shell", "m1").is_empty());
        // Headless: `opencode run` has -m (with the variant); the others use the same --model.
        assert_eq!(headless_args("opencode", "p/m#high"), ["-m", "p/m#high"]);
        assert_eq!(headless_args("claude", "m1"), ["--model", "m1"]);
        assert!(headless_args("aider", "m1").is_empty());
    }

    #[test]
    fn opencode_tui_uses_a_private_server_with_the_model_in_the_config() {
        let (args, (key, value)) = opencode_tui(Some("opencode/glm-5.3-flash-free"), None);
        assert_eq!(args, ["--standalone"]);
        assert_eq!(key, "OPENCODE_CONFIG_CONTENT");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&value).expect("json"),
            serde_json::json!({ "model": "opencode/glm-5.3-flash-free" })
        );
        // The variant only exists in `run -m`.
        assert_eq!(opencode_tui(Some("opencode/big-pickle#high"), None).1 .1, r#"{"model":"opencode/big-pickle"}"#);
    }

    #[test]
    fn opencode_tui_always_gets_the_instructions_file_even_without_a_model() {
        let file = std::path::Path::new("/data/agentdeck-instructions.md");
        let (args, (_, value)) = opencode_tui(None, Some(file));
        assert_eq!(args, ["--standalone"]);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&value).expect("json"),
            serde_json::json!({ "instructions": ["/data/agentdeck-instructions.md"] })
        );
        let (_, (_, both)) = opencode_tui(Some("p/m#high"), Some(file));
        let both: serde_json::Value = serde_json::from_str(&both).expect("json");
        assert_eq!((both["model"].as_str(), both["instructions"][0].as_str()), (Some("p/m"), Some("/data/agentdeck-instructions.md")));
    }

    #[test]
    fn effort_is_read_in_english_or_portuguese() {
        assert_eq!(Effort::parse("low"), Some(Effort::Low));
        assert_eq!(Effort::parse(" Médio "), Some(Effort::Medium));
        assert_eq!(Effort::parse("ALTO"), Some(Effort::High));
        for bad in ["", "xhigh", "max", "standard"] {
            assert_eq!(Effort::parse(bad), None, "{bad}");
        }
        for e in Effort::ALL {
            assert_eq!(Effort::parse(e.as_str()), Some(e));
        }
        assert_eq!(serde_json::to_string(&Effort::Medium).expect("json"), r#""medium""#);
    }

    #[test]
    fn effort_reaches_each_cli_its_own_way() {
        // Claude: --effort.
        assert_eq!(effort_args("claude", Effort::High), ["--effort", "high"]);
        assert_eq!(effort_args("claude", Effort::Low), ["--effort", "low"]);
        // Antigravity: the model name suffix; the other models stay as they are.
        assert_eq!(effort_model("antigravity", "gemini-4-argon-low", Effort::High), "gemini-4-argon-high");
        assert_eq!(effort_model("antigravity", "claude-sonnet-5-5-medium", Effort::Low), "claude-sonnet-5-5-low");
        assert_eq!(effort_model("antigravity", "gemini-4-flash", Effort::High), "gemini-4-flash");
        assert_eq!(effort_model("claude", "sonnet", Effort::High), "sonnet");
        // The others get nothing on the command line.
        for k in ["antigravity", "opencode", "codex", "shell"] {
            assert!(effort_args(k, Effort::High).is_empty(), "{k}");
        }
    }

    #[test]
    fn effort_explains_why_it_is_unavailable() {
        assert_eq!(effort_unavailable("claude", None), None);
        assert_eq!(effort_unavailable("claude", Some("sonnet")), None);
        assert_eq!(effort_unavailable("antigravity", Some("gemini-4-argon-low")), None);
        assert!(effort_unavailable("antigravity", None).is_some_and(|d| d.contains("model")));
        assert!(effort_unavailable("antigravity", Some("gemini-4-flash")).is_some());
        assert!(effort_unavailable("opencode", Some("opencode/big-pickle")).is_some_and(|d| d.contains("headless")));
        for k in ["codex", "shell"] {
            assert!(effort_unavailable(k, None).is_some(), "{k}");
        }
    }

    #[test]
    fn short_names_drop_the_provider() {
        assert_eq!(short_model("opencode/glm-5.3-flash-free"), "glm-5.3-flash-free");
        assert_eq!(short_model("sonnet"), "sonnet");
        assert_eq!(short_model("a/"), "a/");
    }

    #[test]
    fn models_are_grouped_by_provider() {
        let list = [
            "lmstudio/qwen3.5:4b",
            "opencode/paid-before",
            "opencode/glm-5.3-flash-free",
            "opencode-go/deepseek-v4.1-flash",
            "opencode/big-pickle",
            "opencode-go/glm-5.3",
            "opencode/beam-free",
        ]
        .map(str::to_owned)
        .to_vec();
        let groups = by_provider(&list);
        assert_eq!(groups.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(), ["lmstudio", "opencode", "opencode-go"]);
        // Free ones first, and the original order inside each group (stable sort).
        assert_eq!(groups[1].1, ["opencode/glm-5.3-flash-free", "opencode/beam-free", "opencode/paid-before", "opencode/big-pickle"]);
        assert_eq!(groups[2].1, ["opencode-go/deepseek-v4.1-flash", "opencode-go/glm-5.3"]);
        // No provider in the name (Claude, Antigravity): nothing to group.
        assert!(by_provider(&["sonnet".to_owned(), "gemini-4-argon".to_owned()]).is_empty());
        assert!(by_provider(&[]).is_empty());
    }

    #[test]
    fn free_models_come_first_and_keep_the_order() {
        let list = ["paid-a", "free-b-free", "paid-c", "free-d-free"].map(str::to_owned).to_vec();
        assert_eq!(free_first(&list), ["free-b-free", "free-d-free", "paid-a", "paid-c"]);
        // The original list does not change.
        assert_eq!(list[0], "paid-a");
        assert!(free_first(&[]).is_empty());
    }

    #[test]
    fn a_single_provider_skips_the_provider_step() {
        let one = by_provider(&["opencode/paid".to_owned(), "opencode/free-free".to_owned()]);
        assert_eq!(sole_provider(&one), Some("opencode"));
        let two = by_provider(&["opencode/a".to_owned(), "lmstudio/b".to_owned()]);
        assert_eq!(sole_provider(&two), None);
        assert_eq!(sole_provider(&[]), None);
    }

    #[test]
    fn providers_and_free_models_are_recognized() {
        assert_eq!(provider("opencode-go/gpt-6-luna"), Some("opencode-go"));
        assert_eq!(provider("lmstudio/qwen3.5:9b"), Some("lmstudio"));
        assert_eq!(provider("sonnet"), None);
        assert_eq!(provider("/x"), None);
        assert!(is_free("opencode/deepseek-v4.1-flash-free"));
        assert!(is_free("opencode/beam-free#high"));
        assert!(!is_free("opencode/deepseek-v4.1-flash"));
        assert!(!is_free("sonnet"));
    }

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_inventory_is_formatted_one_line_per_agent_and_one_per_provider() {
        let items = vec![
            classify("claude", strs(&["opus", "sonnet"])),
            classify("antigravity", strs(&["gemini-4-argon-low", "gemini-4-argon-free"])),
            classify("codex", Vec::new()),
            classify(
                "opencode",
                strs(&["opencode/paid", "opencode/glm-5.3-flash-free", "opencode-go/deepseek-v4.1-flash", "opencode/beam-free#high"]),
            ),
            classify("aider", Vec::new()),
        ];
        let expected = [
            "claude --model: opus, sonnet",
            "  quota: last reading 10 min ago",
            "antigravity --model: gemini-4-argon-free (free), gemini-4-argon-low",
            "  quota: Gemini 4 Argon EXHAUSTED",
            "codex --model <name> (this computer does not list the models)",
            "  quota: no reading",
            "opencode --model <provider>/<model>:",
            "  opencode/: glm-5.3-flash-free (free), beam-free#high (free), paid",
            "  opencode-go/: deepseek-v4.1-flash",
            "  quota: no reading",
            "aider: no model choice (uses the CLI default)",
        ];
        let quotas = std::collections::HashMap::from([
            ("claude".to_owned(), "last reading 10 min ago".to_owned()),
            ("antigravity".to_owned(), "Gemini 4 Argon EXHAUSTED".to_owned()),
        ]);
        assert_eq!(format_inventory(&items, &quotas), expected.join("\n"));
    }

    #[test]
    fn official_agents_are_ordered_by_who_has_the_most_quota_headroom_first() {
        let mut items = vec![
            classify("claude", strs(&["opus"])),
            classify("antigravity", strs(&["gemini-4-argon-high"])),
            classify("codex", Vec::new()),
            classify("opencode", strs(&["opencode/glm-5.3-flash-free"])),
            classify("aider", Vec::new()),
        ];
        // Antigravity just renewed (4%), Claude at 60%, Codex exhausted, OpenCode with no reading.
        let used =
            std::collections::HashMap::from([("claude".to_owned(), 0.60), ("antigravity".to_owned(), 0.04), ("codex".to_owned(), 1.0)]);
        sort_by_quota(&mut items, &used);
        let names: Vec<&str> = items.iter().map(|i| i.agent.as_str()).collect();
        assert_eq!(names, ["antigravity", "claude", "opencode", "codex", "aider"]);
    }

    #[test]
    fn a_failed_listing_is_one_line_and_the_rest_stays() {
        // The installed CLI that listed nothing (timeout, error): one line + the quota, without dropping the others.
        let items = vec![classify("opencode", Vec::new()), classify("claude", strs(&["opus"]))];
        assert_eq!(items[0].list, ModelList::Failed);
        let text = format_inventory(&items, &Default::default());
        assert_eq!(text.lines().count(), 4, "{text}");
        assert!(text.starts_with("opencode: could not list the models"), "{text}");
        assert!(text.ends_with("claude --model: opus\n  quota: no reading"), "{text}");
        // No agent installed.
        assert_eq!(format_inventory(&[], &Default::default()), "No agent installed on this computer.");
    }

    #[test]
    fn model_lists_are_parsed() {
        let agy = "Fetching available models...\ngemini-4-argon-medium\tGemini 4 Argon (Medium)\nclaude-opus-5-5-low\tClaude Opus 5.5 (Low)\ngemini-4-argon-medium\trepeated\n";
        assert_eq!(parse_agy_models(agy), ["gemini-4-argon-medium", "claude-opus-5-5-low"]);
        let oc = "lmstudio/qwen3.5:4b\nopencode/big-pickle\n\nnoise without a slash\nopencode/big-pickle\n";
        assert_eq!(parse_opencode_models(oc), ["lmstudio/qwen3.5:4b", "opencode/big-pickle"]);
        assert!(parse_agy_models("error").is_empty());
    }
}
