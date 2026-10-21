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

/// O agente aceita escolher o modelo (`--model` ou, no OpenCode, a configuração). Os modelos do
/// Ollama já vão no tipo (`ollama:qwen3.5:9b`) e os demais agentes não têm a opção.
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

/// Como abrir o TUI do OpenCode com um modelo: `(argumentos, variável de ambiente)`.
///
/// O TUI não tem `--model` (só `run` e `mini` têm `-m`). A configuração `{"model": "..."}` em
/// `OPENCODE_CONFIG_CONTENT` vale por cima da configuração do usuário e do projeto, mas só é lida por
/// quem sobe o servidor, e por padrão o TUI usa o serviço em segundo plano compartilhado (que ignora a
/// variável do nosso processo). Por isso `--standalone`: servidor próprio da aba, com o nosso ambiente.
/// Verificado com o OpenCode 2.0.32 (a linha de status do TUI mostra o modelo pedido). A variante
/// (`modelo#alta`) não existe na configuração, só no `run -m`: aqui vai só o modelo.
pub fn opencode_tui(model: &str) -> (Vec<String>, (String, String)) {
    let base = model.split('#').next().unwrap_or(model);
    (vec!["--standalone".into()], ("OPENCODE_CONFIG_CONTENT".into(), serde_json::json!({ "model": base }).to_string()))
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
        for k in ["ollama", "ollama:qwen3.5:9b", "shell", "aider", "goose", "copilot", "q", "interpreter"] {
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
        let all = defaults(&[("shell", "x"), ("ollama", "x")]);
        assert_eq!(resolve_model(&id("shell"), Some("x"), None, &all), None);
        assert_eq!(resolve_model(&id("ollama:qwen3.5:9b"), None, None, &all), None);
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
        let (args, (key, value)) = opencode_tui("opencode/glm-5.3-flash-free");
        assert_eq!(args, ["--standalone"]);
        assert_eq!(key, "OPENCODE_CONFIG_CONTENT");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&value).expect("json"),
            serde_json::json!({ "model": "opencode/glm-5.3-flash-free" })
        );
        // A variante só existe no `run -m`.
        assert_eq!(opencode_tui("opencode/big-pickle#high").1 .1, r#"{"model":"opencode/big-pickle"}"#);
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
            "ollama/qwen3.5:4b",
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
        assert_eq!(groups.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(), ["ollama", "opencode", "opencode-go"]);
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
        let two = by_provider(&["opencode/a".to_owned(), "ollama/b".to_owned()]);
        assert_eq!(sole_provider(&two), None);
        assert_eq!(sole_provider(&[]), None);
    }

    #[test]
    fn providers_and_free_models_are_recognized() {
        assert_eq!(provider("opencode-go/gpt-6-luna"), Some("opencode-go"));
        assert_eq!(provider("ollama/qwen3.5:9b"), Some("ollama"));
        assert_eq!(provider("sonnet"), None);
        assert_eq!(provider("/x"), None);
        assert!(is_free("opencode/deepseek-v4.1-flash-free"));
        assert!(is_free("opencode/beam-free#alta"));
        assert!(!is_free("opencode/deepseek-v4.1-flash"));
        assert!(!is_free("sonnet"));
    }

    #[test]
    fn model_lists_are_parsed() {
        let agy = "Fetching available models...\ngemini-4-argon-medium\tGemini 4 Argon (Medium)\nclaude-opus-5-5-low\tClaude Opus 5.5 (Low)\ngemini-4-argon-medium\trepetido\n";
        assert_eq!(parse_agy_models(agy), ["gemini-4-argon-medium", "claude-opus-5-5-low"]);
        let oc = "ollama/qwen3.5:4b\nopencode/big-pickle\n\nruído sem barra\nopencode/big-pickle\n";
        assert_eq!(parse_opencode_models(oc), ["ollama/qwen3.5:4b", "opencode/big-pickle"]);
        assert!(parse_agy_models("erro").is_empty());
    }
}
