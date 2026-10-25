//! Modelo de LLM de cada instância (`claude@dev` roda `sonnet`).
//!
//! O modelo é uma propriedade da instância, **não** parte do id: o id continua `modelo[@cargo[@nome]]`
//! e o modelo fica ao lado, em `ProjectCfg.tab_models` (id → modelo), no mesmo molde de `tab_parents`.
//! Ao abrir uma instância vale, na ordem: o modelo pedido (`ask-agent --model`, menu), o gravado em
//! `tab_models` (reabrir depois de fechar o app), o padrão de `Config.role_models` (`modelo@cargo`
//! vence `modelo`) e, por fim, o padrão do próprio CLI (nenhum argumento). Aqui ficam as regras puras
//! (com testes) e o `impl Core` que as aplica; a ponte e a interface só chamam o `Core`.

use std::collections::BTreeMap;
use std::process::Command;
use std::time::{Duration, Instant};

use crate::instances::Ident;
use crate::Core;

/// Tamanho máximo do nome de um modelo.
pub const MAX_MODEL_LEN: usize = 128;

/// Apelidos de modelo aceitos pelo `claude --model`.
pub const CLAUDE_MODELS: [&str; 4] = ["opus", "sonnet", "haiku", "fable"];

/// Nome de modelo seguro para a linha de comando, o JSON e o texto digitado em outras abas: letras,
/// números e `: . _ / # @ + -` (ex.: `sonnet`, `opencode/glm-5.3-flash-free`, `provedor/modelo#variante`).
pub fn valid_model(s: &str) -> bool {
    !s.is_empty() && s.len() <= MAX_MODEL_LEN && s.chars().all(|c| c.is_ascii_alphanumeric() || ":._/#@+-".contains(c))
}

/// O agente aceita escolher o modelo (`--model` ou, no OpenCode, a configuração); os demais
/// agentes não têm a opção.
pub fn accepts_model(kind: &str) -> bool {
    matches!(kind, "claude" | "antigravity" | "codex" | "opencode")
}

/// Padrão de `Config.role_models` para a instância: `modelo@cargo` vence `modelo`. Entradas com
/// nome de modelo inválido são ignoradas.
pub fn default_model(role_models: &BTreeMap<String, String>, ident: &Ident) -> Option<String> {
    let by_role = ident.role.as_ref().map(|r| format!("{}@{r}", ident.kind));
    by_role.iter().chain(std::iter::once(&ident.kind)).find_map(|k| role_models.get(k).filter(|m| valid_model(m)).cloned())
}

/// O modelo com que a instância abre: o pedido, senão o gravado, senão o padrão do cargo.
/// `None` = o padrão do CLI (ou um agente que não escolhe modelo).
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

/// Mensagem do 409 quando a instância `id` já está aberta com outro modelo que o pedido.
/// `None` = sem conflito (o mesmo modelo).
pub fn conflict(id: &str, current: Option<&str>, requested: &str) -> Option<String> {
    if current == Some(requested) {
        return None;
    }
    let current = current.unwrap_or("o modelo padrão");
    Some(format!("{id} já está aberta com {current}; use --new para abrir outra com {requested}"))
}

/// A instância `old` passou a se chamar `new`: o modelo vai junto.
pub fn rename(models: &mut BTreeMap<String, String>, old: &str, new: &str) {
    if let Some(m) = models.remove(old) {
        models.insert(new.to_owned(), m);
    }
}

/// Argumentos que escolhem o modelo na linha de comando de `kind` (aba ou headless interativo).
/// O OpenCode não tem opção no TUI: ver [`opencode_tui`].
pub fn cli_args(kind: &str, model: &str) -> Vec<String> {
    match kind {
        "claude" | "antigravity" | "codex" => vec!["--model".into(), model.into()],
        _ => Vec::new(),
    }
}

/// Como o CLI recebe um prompt inicial interativo ao abrir (o texto não é digitado no terminal, então não
/// depende de a TUI já estar pronta). `None` = o CLI não tem a opção: a entrega continua digitada.
/// `Some(None)` = argumento posicional (Claude: `claude "<prompt>"`); `Some(Some(flag))` = `flag "<prompt>"`.
pub fn boot_prompt_flag(kind: &str) -> Option<Option<&'static str>> {
    match kind {
        "claude" => Some(None),
        "antigravity" => Some(Some("-i")),
        "opencode" => Some(Some("--prompt")),
        _ => None,
    }
}

/// Como [`cli_args`], para a execução headless: o `opencode run` tem `-m provedor/modelo[#variante]`.
pub fn headless_args(kind: &str, model: &str) -> Vec<String> {
    match kind {
        "opencode" => vec!["-m".into(), model.into()],
        _ => cli_args(kind, model),
    }
}

/// Como abrir o TUI do OpenCode com o modelo e as instruções do AgentDeck: `(argumentos, variável de ambiente)`.
///
/// O TUI não tem `--model` (só `run` e `mini` têm `-m`) nem opção de instruções. A configuração em
/// `OPENCODE_CONFIG_CONTENT` (`{"model": "...", "instructions": ["<arquivo>"]}`) vale por cima da configuração do
/// usuário e do projeto, e as `instructions` somam-se às dele (o `AGENTS.md` do projeto continua lido). Mas ela só
/// é lida por quem sobe o servidor, e por padrão o TUI usa o serviço em segundo plano compartilhado (que ignora a
/// variável do nosso processo). Por isso `--standalone`: servidor próprio da aba, com o nosso ambiente. Verificado
/// com o OpenCode 2.0.32 (a linha de status do TUI mostra o modelo pedido; `opencode run --standalone` com a
/// configuração obedece a uma regra do arquivo de instruções). A variante (`modelo#alta`) não existe na
/// configuração, só no `run -m`: aqui vai só o modelo.
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

/// Esforço de raciocínio de uma tarefa. Cada CLI o traduz do seu jeito ([`effort_args`], [`effort_model`]);
/// "Padrão" é a ausência (`Option<Effort>` = `None`): nada vai ao CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Low,
    Medium,
    High,
}

impl Effort {
    pub const ALL: [Effort; 3] = [Effort::Low, Effort::Medium, Effort::High];

    /// O valor de `--effort`, do JSON e do sufixo dos modelos do Antigravity.
    pub fn as_str(self) -> &'static str {
        match self {
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
        }
    }

    /// O nome na interface.
    pub fn label(self) -> &'static str {
        match self {
            Effort::Low => "baixo",
            Effort::Medium => "médio",
            Effort::High => "alto",
        }
    }

    /// Lê `low|medium|high` (ou `baixo|médio|alto`), sem diferenciar maiúsculas.
    pub fn parse(s: &str) -> Option<Effort> {
        match s.trim().to_lowercase().as_str() {
            "low" | "baixo" => Some(Effort::Low),
            "medium" | "médio" | "medio" => Some(Effort::Medium),
            "high" | "alto" => Some(Effort::High),
            _ => None,
        }
    }
}

/// Os modelos do Antigravity trazem o esforço no nome: `…-low`, `…-medium` ou `…-high`.
fn agy_effort_split(model: &str) -> Option<(&str, Effort)> {
    Effort::ALL.into_iter().find_map(|e| model.strip_suffix(e.as_str())?.strip_suffix('-').map(|base| (base, e)))
}

/// Por que o esforço não vale para `kind` com `model` (a dica da interface e o erro do pedido); `None` = vale.
/// Claude: `--effort`. Antigravity: troca o sufixo do nome do modelo, então precisa de um modelo com sufixo.
/// OpenCode: a variante (`modelo#variante`) só existe no `run -m`; a aba interativa não a recebe.
pub fn effort_unavailable(kind: &str, model: Option<&str>) -> Option<&'static str> {
    match kind {
        "claude" => None,
        "antigravity" => match model {
            None => Some("Escolha um modelo: no Antigravity o esforço é parte do nome dele"),
            Some(m) if agy_effort_split(m).is_none() => Some("Este modelo do Antigravity não tem versões por esforço"),
            Some(_) => None,
        },
        "opencode" => Some("O OpenCode só aceita esforço no modo headless; a aba não recebe"),
        _ => Some("Este agente não tem esforço"),
    }
}

/// Argumentos que passam o esforço ao CLI na linha de comando (só o Claude tem a opção; o Antigravity usa
/// [`effort_model`]).
pub fn effort_args(kind: &str, effort: Effort) -> Vec<String> {
    match kind {
        "claude" => vec!["--effort".into(), effort.as_str().into()],
        _ => Vec::new(),
    }
}

/// O modelo do Antigravity com o sufixo de esforço trocado (`gemini-4-argon-low` + alto →
/// `gemini-4-argon-high`). Qualquer outro caso devolve o modelo como veio.
pub fn effort_model(kind: &str, model: &str, effort: Effort) -> String {
    match (kind, agy_effort_split(model)) {
        ("antigravity", Some((base, _))) => format!("{base}-{}", effort.as_str()),
        _ => model.to_owned(),
    }
}

/// Forma curta para rótulos: sem o provedor (`opencode/glm-5.3` → `glm-5.3`).
pub fn short_model(model: &str) -> &str {
    model.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or(model)
}

/// O provedor de um modelo: o prefixo antes da `/` (`opencode-go/deepseek-v4.1-flash` →
/// `opencode-go`). `None` para os modelos que não trazem provedor no nome (`sonnet`, `haiku`).
pub fn provider(model: &str) -> Option<&str> {
    model.split_once('/').map(|(p, _)| p).filter(|p| !p.is_empty())
}

/// A lista com os gratuitos (`-free`) primeiro, mantendo a ordem original dentro de cada grupo
/// (ordenação estável). É o que as listas de modelo mostram.
pub fn free_first(models: &[String]) -> Vec<String> {
    let mut out = models.to_vec();
    out.sort_by_key(|m| !is_free(m));
    out
}

/// A lista de modelos agrupada por provedor, na ordem em que os provedores aparecem, com os
/// gratuitos (`-free`) primeiro em cada grupo.
///
/// Só o `opencode models` devolve `provedor/modelo`; os demais agentes listam nomes simples, então
/// a lista deles volta vazia e a interface mostra a lista de sempre. Modelos sem provedor no nome
/// ficam de fora dos grupos.
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

/// O passo inicial do menu de modelo: com **um provedor só** não há o que escolher, então os modelos
/// dele aparecem direto, sem o passo dos provedores. `None` = a lista de provedores vem primeiro (ou
/// não há grupos, caso de Claude e Antigravity).
pub fn sole_provider(groups: &[(String, Vec<String>)]) -> Option<&str> {
    match groups {
        [(provider, _)] => Some(provider),
        _ => None,
    }
}

/// Modelo gratuito (`…-free`, o `#variante` não conta): sem custo por token, para destacar nas
/// listas do OpenCode.
pub fn is_free(model: &str) -> bool {
    short_model(model).split('#').next().unwrap_or_default().ends_with("-free")
}

/// `agy models`: uma linha por modelo, `id<TAB>nome` (mais uma linha de aviso sem tabulação).
pub fn parse_agy_models(out: &str) -> Vec<String> {
    let mut seen = Vec::new();
    for id in out.lines().filter(|l| l.contains('\t')).filter_map(|l| l.split('\t').next()).map(str::trim) {
        if valid_model(id) && !seen.iter().any(|s| s == id) {
            seen.push(id.to_owned());
        }
    }
    seen
}

/// `opencode models`: um `provedor/modelo` por linha.
pub fn parse_opencode_models(out: &str) -> Vec<String> {
    let mut seen = Vec::new();
    for id in out.lines().map(str::trim) {
        if valid_model(id) && id.contains('/') && !seen.iter().any(|s| s == id) {
            seen.push(id.to_owned());
        }
    }
    seen
}

/// O que se sabe dos modelos de um agente instalado (`ask-agent --models`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelList {
    /// Os modelos que o `--model` aceita.
    Models(Vec<String>),
    /// Aceita `--model <nome>`, mas este computador não lista os nomes (Codex).
    Unlisted,
    /// Não escolhe modelo: usa o padrão do CLI.
    Fixed,
    /// A listagem falhou: o CLI não respondeu a tempo ou a lista veio vazia.
    Failed,
}

/// Um agente instalado e os seus modelos, como o `--agent` e o `--model` os aceitam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentModels {
    pub agent: String,
    pub list: ModelList,
}

/// Os agentes cujos modelos o CLI ou a lista fixa informam (os demais não têm lista).
pub fn has_model_list(kind: &str) -> bool {
    matches!(kind, "claude" | "antigravity" | "opencode")
}

/// A entrada do inventário de `kind`, dada a lista que [`Core::known_models`] devolveu (vazia = falhou).
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

/// `a, b (grátis), c`: os gratuitos primeiro e marcados.
fn join_marked(models: &[String], strip: impl Fn(&str) -> &str) -> String {
    free_first(models)
        .iter()
        .map(|m| if is_free(m) { format!("{} (grátis)", strip(m)) } else { strip(m).to_owned() })
        .collect::<Vec<_>>()
        .join(", ")
}

/// O texto de `ask-agent --models`: uma linha por agente (o OpenCode, uma por provedor), curto o
/// bastante para um agente ler, e a cota de quem tem (`quotas`: agente → linha, de [`Core::quota_lines`]).
/// Função pura (testada); quem busca os dados é [`Core::models_inventory`].
pub fn format_inventory(items: &[AgentModels], quotas: &std::collections::HashMap<String, String>) -> String {
    if items.is_empty() {
        return "Nenhum agente instalado neste computador.".to_owned();
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
                    lines.push(format!("{agent} --model <provedor>/<modelo>:"));
                    lines.extend(
                        groups
                            .iter()
                            .map(|(p, list)| format!("  {p}/: {}", join_marked(list, |m| m.strip_prefix(&format!("{p}/")).unwrap_or(m)))),
                    );
                }
            }
            ModelList::Unlisted => lines.push(format!("{agent} --model <nome> (este computador não lista os modelos)")),
            ModelList::Fixed => lines.push(format!("{agent}: sem escolha de modelo (usa o padrão do CLI)")),
            ModelList::Failed => {
                lines.push(format!("{agent}: não consegui listar os modelos (o CLI não respondeu); sem --model ele usa o padrão"))
            }
        }
        if let Some(q) = quotas.get(agent) {
            lines.push(format!("  cota: {q}"));
        } else if accepts_model(agent) {
            lines.push("  cota: sem leitura".to_owned());
        }
    }
    lines.join("\n")
}

/// Ordena os agentes oficiais por quem tem mais folga primeiro (menor fração usada); quem não tem leitura vem
/// depois de quem tem folga e antes de quem está esgotado, mantendo a ordem original em caso de empate.
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

/// Quanto tempo a lista de modelos de um agente fica em cache.
const LIST_TTL: Duration = Duration::from_secs(600);
/// Uma consulta que falhou só é repetida depois disto.
const LIST_RETRY: Duration = Duration::from_secs(60);
/// Limite de tempo de `agy models` / `opencode models`.
const LIST_TIMEOUT: Duration = Duration::from_secs(15);

/// Uma consulta guardada: quando, se deu certo e a lista.
type CachedList = (Instant, bool, Vec<String>);

/// Cache das listas de modelos por agente.
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
    /// Modelo efetivo da instância (gravado ou padrão do cargo), sem gravar nada. `None` = o padrão do CLI.
    pub fn instance_model(&self, project: &str, ident: &Ident) -> Option<String> {
        let cfg = self.inner.config.read();
        let persisted = cfg.project(project).and_then(|p| p.tab_models.get(&ident.id()));
        resolve_model(ident, None, persisted.map(String::as_str), &cfg.role_models)
    }

    /// Resolve o modelo com que a instância abre (pedido > gravado > padrão do cargo) e o **grava**
    /// em `tab_models`, para aparecer na interface e voltar depois de reabrir o app. Idempotente.
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

    /// Esquece o modelo gravado da instância (ela fechou ou nunca chegou a abrir).
    pub(crate) fn forget_model(&self, project: &str, id: &str) {
        if self.inner.config.read().project(project).is_some_and(|p| p.tab_models.contains_key(id)) {
            self.update_project(project, |p| {
                p.tab_models.remove(id);
            });
        }
    }

    /// Modelo de cada instância aberta no projeto que tem um (id → modelo), para listas e `--team`.
    pub fn instance_models(&self, project: &str) -> BTreeMap<String, String> {
        self.project_instance_ids(project)
            .into_iter()
            .filter_map(|id| {
                let ident = Ident::parse(&id)?;
                Some((id, self.instance_model(project, &ident)?))
            })
            .collect()
    }

    /// Nova instância aberta pelo usuário (menu "+") já com o modelo escolhido (`None` = padrão).
    /// Erro se o modelo é inválido ou o agente não escolhe modelo.
    pub fn register_instance_with_model(&self, project: &str, kind: &str, model: Option<&str>) -> Result<Ident, String> {
        if let Some(m) = model {
            if !valid_model(m) {
                return Err(format!("modelo inválido: \"{m}\""));
            }
            if !accepts_model(kind) {
                return Err(format!("o agente {kind} não aceita escolher modelo"));
            }
        }
        let ident = self.register_instance(project, kind);
        self.assign_model(project, &ident, model);
        Ok(ident)
    }

    /// Modelos conhecidos de um agente, para o menu de novo agente. Claude: os apelidos; Antigravity e
    /// OpenCode: o que o CLI lista (`agy models`, `opencode models`), em cache. BLOQUEANTE (até 15 s
    /// na primeira vez): chame fora da thread da interface. Vazio = só o padrão.
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

    /// Os agentes instalados e os seus modelos (`ask-agent --models`), com o cache de [`Core::known_models`].
    /// `only` limita a um agente (aceita apelidos como `agy`). As consultas aos CLIs
    /// correm juntas, então demora o mesmo que a mais lenta (até `LIST_TIMEOUT`). BLOQUEANTE.
    /// Erro = o agente pedido não está instalado.
    pub fn models_inventory(&self, only: Option<&str>) -> Result<Vec<AgentModels>, String> {
        let agents = &self.inner.agents;
        let mut kinds: Vec<String> =
            crate::agents::AGENTS.iter().filter(|a| a.peer && agents.resolve(a.id).is_some()).map(|a| a.id.to_owned()).collect();
        if let Some(only) = only {
            let name = crate::agents::resolve_alias(only);
            if !kinds.iter().any(|k| k == name) {
                let have: Vec<&str> = kinds.iter().map(String::as_str).collect();
                return Err(format!("o agente \"{only}\" não está instalado neste computador (instalados: {})", have.join(", ")));
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
        Ident::parse(s).expect("id válido")
    }

    fn defaults(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn model_names_are_validated() {
        for ok in ["sonnet", "opencode/glm-5.3-flash-free", "provedor/modelo#alta", "qwen3.5:9b", "a@b+c_d.e"] {
            assert!(valid_model(ok), "{ok}");
        }
        for bad in ["", "dois nomes", "a\"b", "a&b", "a|b", "a%b", "a^b", "a<b", "a\nb", "\x1b[0m", "usuário", "$(x)", "a;b", "a`b"] {
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
        // a) o pedido vence tudo.
        assert_eq!(resolve_model(&dev, Some("haiku"), Some("fable"), &cfg).as_deref(), Some("haiku"));
        // b) sem pedido, o gravado.
        assert_eq!(resolve_model(&dev, None, Some("fable"), &cfg).as_deref(), Some("fable"));
        // c) sem nenhum, `modelo@cargo` vence `modelo`.
        assert_eq!(resolve_model(&dev, None, None, &cfg).as_deref(), Some("sonnet"));
        assert_eq!(resolve_model(&id("claude@qa"), None, None, &cfg).as_deref(), Some("opus"), "o cargo sem entrada cai no modelo");
        assert_eq!(resolve_model(&id("claude"), None, None, &cfg).as_deref(), Some("opus"));
        assert_eq!(resolve_model(&id("claude@dev@ana"), None, None, &cfg).as_deref(), Some("sonnet"), "o nome não conta");
        assert_eq!(resolve_model(&id("opencode@x"), None, None, &cfg).as_deref(), Some("opencode/glm-5.3-flash-free"));
        // d) nada: o padrão do CLI.
        assert_eq!(resolve_model(&id("antigravity"), None, None, &cfg), None);
        // Quem não escolhe modelo nunca ganha um, nem do padrão.
        let all = defaults(&[("shell", "x"), ("aider", "x")]);
        assert_eq!(resolve_model(&id("shell"), Some("x"), None, &all), None);
        assert_eq!(resolve_model(&id("aider"), None, None, &all), None);
        // Valores inválidos (config editada à mão) são ignorados.
        let bad = defaults(&[("claude", "a b"), ("claude@dev", "ok;rm")]);
        assert_eq!(resolve_model(&dev, Some("x y"), Some("z&"), &bad), None);
        assert_eq!(resolve_model(&dev, None, Some("z&"), &defaults(&[("claude", "opus")])).as_deref(), Some("opus"));
    }

    #[test]
    fn a_different_model_is_a_conflict() {
        assert_eq!(conflict("claude@dev", Some("sonnet"), "sonnet"), None);
        assert_eq!(
            conflict("claude@dev", Some("sonnet"), "opus").as_deref(),
            Some("claude@dev já está aberta com sonnet; use --new para abrir outra com opus")
        );
        assert_eq!(
            conflict("claude@dev", None, "opus").as_deref(),
            Some("claude@dev já está aberta com o modelo padrão; use --new para abrir outra com opus")
        );
    }

    #[test]
    fn renaming_carries_the_model() {
        let mut m = defaults(&[("claude@dev", "sonnet"), ("claude@qa", "opus")]);
        rename(&mut m, "claude@dev", "claude@lead");
        assert_eq!(m, defaults(&[("claude@lead", "sonnet"), ("claude@qa", "opus")]));
        rename(&mut m, "nao-existe", "x");
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn cli_args_per_agent() {
        for k in ["claude", "antigravity", "codex"] {
            assert_eq!(cli_args(k, "m1"), ["--model", "m1"], "{k}");
        }
        assert!(cli_args("opencode", "m1").is_empty(), "o TUI do OpenCode não tem --model");
        assert!(cli_args("shell", "m1").is_empty());
        // Headless: o `opencode run` tem -m (com a variante); os demais usam o mesmo --model.
        assert_eq!(headless_args("opencode", "p/m#alta"), ["-m", "p/m#alta"]);
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
        // A variante só existe no `run -m`.
        assert_eq!(opencode_tui(Some("opencode/big-pickle#high"), None).1 .1, r#"{"model":"opencode/big-pickle"}"#);
    }

    #[test]
    fn opencode_tui_always_gets_the_instructions_file_even_without_a_model() {
        let file = std::path::Path::new("/dados/agentdeck-instructions.md");
        let (args, (_, value)) = opencode_tui(None, Some(file));
        assert_eq!(args, ["--standalone"]);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&value).expect("json"),
            serde_json::json!({ "instructions": ["/dados/agentdeck-instructions.md"] })
        );
        let (_, (_, both)) = opencode_tui(Some("p/m#alta"), Some(file));
        let both: serde_json::Value = serde_json::from_str(&both).expect("json");
        assert_eq!((both["model"].as_str(), both["instructions"][0].as_str()), (Some("p/m"), Some("/dados/agentdeck-instructions.md")));
    }

    #[test]
    fn effort_is_read_in_english_or_portuguese() {
        assert_eq!(Effort::parse("low"), Some(Effort::Low));
        assert_eq!(Effort::parse(" Médio "), Some(Effort::Medium));
        assert_eq!(Effort::parse("ALTO"), Some(Effort::High));
        for bad in ["", "xhigh", "max", "padrão"] {
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
        // Antigravity: o sufixo do nome do modelo; os outros modelos ficam como estão.
        assert_eq!(effort_model("antigravity", "gemini-4-argon-low", Effort::High), "gemini-4-argon-high");
        assert_eq!(effort_model("antigravity", "claude-sonnet-5-5-medium", Effort::Low), "claude-sonnet-5-5-low");
        assert_eq!(effort_model("antigravity", "gemini-4-flash", Effort::High), "gemini-4-flash");
        assert_eq!(effort_model("claude", "sonnet", Effort::High), "sonnet");
        // Os demais não recebem nada na linha de comando.
        for k in ["antigravity", "opencode", "codex", "shell"] {
            assert!(effort_args(k, Effort::High).is_empty(), "{k}");
        }
    }

    #[test]
    fn effort_explains_why_it_is_unavailable() {
        assert_eq!(effort_unavailable("claude", None), None);
        assert_eq!(effort_unavailable("claude", Some("sonnet")), None);
        assert_eq!(effort_unavailable("antigravity", Some("gemini-4-argon-low")), None);
        assert!(effort_unavailable("antigravity", None).is_some_and(|d| d.contains("modelo")));
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
            "opencode/pago-antes",
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
        // Gratuitos primeiro, e a ordem original dentro de cada grupo (ordenação estável).
        assert_eq!(groups[1].1, ["opencode/glm-5.3-flash-free", "opencode/beam-free", "opencode/pago-antes", "opencode/big-pickle"]);
        assert_eq!(groups[2].1, ["opencode-go/deepseek-v4.1-flash", "opencode-go/glm-5.3"]);
        // Sem provedor no nome (Claude, Antigravity): nada a agrupar.
        assert!(by_provider(&["sonnet".to_owned(), "gemini-4-argon".to_owned()]).is_empty());
        assert!(by_provider(&[]).is_empty());
    }

    #[test]
    fn free_models_come_first_and_keep_the_order() {
        let list = ["pago-a", "gratis-b-free", "pago-c", "gratis-d-free"].map(str::to_owned).to_vec();
        assert_eq!(free_first(&list), ["gratis-b-free", "gratis-d-free", "pago-a", "pago-c"]);
        // A lista original não muda.
        assert_eq!(list[0], "pago-a");
        assert!(free_first(&[]).is_empty());
    }

    #[test]
    fn a_single_provider_skips_the_provider_step() {
        let one = by_provider(&["opencode/pago".to_owned(), "opencode/gratis-free".to_owned()]);
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
        assert!(is_free("opencode/beam-free#alta"));
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
                strs(&["opencode/pago", "opencode/glm-5.3-flash-free", "opencode-go/deepseek-v4.1-flash", "opencode/beam-free#alta"]),
            ),
            classify("aider", Vec::new()),
        ];
        let expected = [
            "claude --model: opus, sonnet",
            "  cota: última leitura há 10 min",
            "antigravity --model: gemini-4-argon-free (grátis), gemini-4-argon-low",
            "  cota: Gemini 4 Argon ESGOTADA",
            "codex --model <nome> (este computador não lista os modelos)",
            "  cota: sem leitura",
            "opencode --model <provedor>/<modelo>:",
            "  opencode/: glm-5.3-flash-free (grátis), beam-free#alta (grátis), pago",
            "  opencode-go/: deepseek-v4.1-flash",
            "  cota: sem leitura",
            "aider: sem escolha de modelo (usa o padrão do CLI)",
        ];
        let quotas = std::collections::HashMap::from([
            ("claude".to_owned(), "última leitura há 10 min".to_owned()),
            ("antigravity".to_owned(), "Gemini 4 Argon ESGOTADA".to_owned()),
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
        // Antigravity acabou de renovar (4%), Claude em 60%, Codex esgotado, OpenCode sem leitura.
        let used =
            std::collections::HashMap::from([("claude".to_owned(), 0.60), ("antigravity".to_owned(), 0.04), ("codex".to_owned(), 1.0)]);
        sort_by_quota(&mut items, &used);
        let names: Vec<&str> = items.iter().map(|i| i.agent.as_str()).collect();
        assert_eq!(names, ["antigravity", "claude", "opencode", "codex", "aider"]);
    }

    #[test]
    fn a_failed_listing_is_one_line_and_the_rest_stays() {
        // O CLI instalado que não listou nada (tempo esgotado, erro): uma linha + a cota, sem derrubar as outras.
        let items = vec![classify("opencode", Vec::new()), classify("claude", strs(&["opus"]))];
        assert_eq!(items[0].list, ModelList::Failed);
        let text = format_inventory(&items, &Default::default());
        assert_eq!(text.lines().count(), 4, "{text}");
        assert!(text.starts_with("opencode: não consegui listar os modelos"), "{text}");
        assert!(text.ends_with("claude --model: opus\n  cota: sem leitura"), "{text}");
        // Nenhum agente instalado.
        assert_eq!(format_inventory(&[], &Default::default()), "Nenhum agente instalado neste computador.");
    }

    #[test]
    fn model_lists_are_parsed() {
        let agy = "Fetching available models...\ngemini-4-argon-medium\tGemini 4 Argon (Medium)\nclaude-opus-5-5-low\tClaude Opus 5.5 (Low)\ngemini-4-argon-medium\trepetido\n";
        assert_eq!(parse_agy_models(agy), ["gemini-4-argon-medium", "claude-opus-5-5-low"]);
        let oc = "lmstudio/qwen3.5:4b\nopencode/big-pickle\n\nruído sem barra\nopencode/big-pickle\n";
        assert_eq!(parse_opencode_models(oc), ["lmstudio/qwen3.5:4b", "opencode/big-pickle"]);
        assert!(parse_agy_models("erro").is_empty());
    }
}
