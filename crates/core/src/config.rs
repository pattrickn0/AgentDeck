//! Configuração persistida (equivalente ao `config.json` do Electron, com campos extras de layout).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths::AppPaths;

/// Um projeto: uma pasta com as próprias abas (instâncias de agentes) e o próprio layout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProjectCfg {
    /// Id curto e estável (`p` + 8 hex), gerado na criação. É o escopo das instâncias na ponte.
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    /// Id de cada aba aberta, na ordem da barra (restaurado quando o projeto é aberto).
    pub open_tabs: Vec<String>,
    /// Índice em `open_tabs` da aba ativa (-1 = nenhuma).
    pub active_tab: i32,
    /// Ordem das janelas da tela dividida (grupos: `claude`, `antigravity`, `codex`…).
    pub pane_order: Vec<String>,
    /// Proporções das linhas da grade e das colunas de cada linha (vazias = iguais). Só valem se
    /// o formato da grade ainda for o mesmo.
    pub split_rows: Vec<f32>,
    pub split_cols: Vec<Vec<f32>>,
    /// Quem abriu cada aba (id da aba → id de quem a chamou pelo `ask-agent`): a hierarquia da
    /// lista de agentes na barra lateral. Abas abertas pelo usuário não entram.
    pub tab_parents: BTreeMap<String, String>,
    /// Modelo de LLM de cada instância (id da aba → modelo, ex.: `claude@dev` → `sonnet`). O id não
    /// muda com o modelo; só as instâncias que escolheram (ou herdaram) um modelo entram aqui.
    pub tab_models: BTreeMap<String, String>,
    /// A lista de agentes do projeto está recolhida na barra lateral.
    pub collapsed: bool,
}

impl Default for ProjectCfg {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            path: PathBuf::new(),
            open_tabs: Vec::new(),
            active_tab: -1,
            pane_order: Vec::new(),
            split_rows: Vec::new(),
            split_cols: Vec::new(),
            tab_parents: BTreeMap::new(),
            tab_models: BTreeMap::new(),
            collapsed: false,
        }
    }
}

impl ProjectCfg {
    /// Projeto novo na pasta `path`, com um id que não está em `taken`. Sem `name`, o nome é o
    /// da pasta.
    pub fn new(path: PathBuf, name: Option<&str>, taken: &[ProjectCfg]) -> Self {
        let name = name.map(str::trim).filter(|n| !n.is_empty()).map(str::to_owned).unwrap_or_else(|| folder_name(&path));
        let id = loop {
            let id = new_project_id();
            if !taken.iter().any(|p| p.id == id) {
                break id;
            }
        };
        Self { id, name, path, ..Self::default() }
    }
}

/// `p` + 8 hex aleatórios.
fn new_project_id() -> String {
    let mut buf = [0u8; 4];
    if getrandom::fill(&mut buf).is_err() {
        buf.copy_from_slice(&uuid::Uuid::new_v4().as_bytes()[..4]);
    }
    format!("p{}", buf.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

/// Nome da pasta (ou o caminho inteiro, se não há nome: a raiz de um disco).
fn folder_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

/// Como as janelas dos agentes se arrumam no centro.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Organization {
    /// Uma janela só, com todas as abas.
    Tabs,
    /// Uma janela por CLI, numa grade.
    #[default]
    Split,
    /// A janela do CLI selecionado grande e as outras empilhadas ao lado.
    Highlight,
}

impl Organization {
    /// Organização de quem só tem o `splitEnabled` gravado.
    pub fn from_split_enabled(on: bool) -> Self {
        if on {
            Self::Split
        } else {
            Self::Tabs
        }
    }

    /// O valor equivalente do `splitEnabled` (qualquer arrumação em janelas conta como ligada).
    pub fn split_enabled(self) -> bool {
        self != Self::Tabs
    }
}

/// A escolha da pessoa sobre o ditado por voz (Whisper local, download de ~1,6 GB).
///
/// Gravada no `dictation` do `config.json` (`"unasked"`, `"yes"` ou `"no"`): um instalador pode escrever
/// o campo para responder a pergunta antes da primeira abertura do app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DictationChoice {
    /// Ainda não perguntado: na primeira abertura o app pergunta (ou, se o Whisper já está baixado, conta como `Yes`).
    #[default]
    Unasked,
    /// Quer o ditado: o app baixa o que faltar.
    Yes,
    /// Não quer: o app não baixa nada (a pessoa pode baixar depois, pelas configurações).
    No,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    /// Projetos do usuário, na ordem da barra lateral.
    pub project_list: Vec<ProjectCfg>,
    /// Id do projeto ativo (vazio = nenhum projeto).
    pub active_project: String,
    /// Pasta do projeto ativo (vazia = nenhum). Derivada de `active_project`, não é gravada.
    /// No formato antigo era o workspace global (lido só para a migração).
    #[serde(skip_serializing)]
    pub workspace: PathBuf,
    pub whisper_lang: String,
    /// Ditado por voz: ver [`DictationChoice`].
    pub dictation: DictationChoice,
    pub sound_enabled: bool,
    // --- Layout (novidade da versão Rust; a versão Electron guardava só em CSS) ---
    pub sidebar_width: f32,
    pub sidebar_collapsed: bool,
    /// Largura da barra lateral direita (Conversa integrada); 0 = padrão. Ela não
    /// recolhe. (Substitui `bottomHeight`/`feedCollapsed` do antigo painel de baixo, ignorados ao ler.)
    pub right_panel_width: f32,
    /// Como as janelas dos agentes se arrumam. Ausente nos arquivos antigos: vale o `splitEnabled`
    /// (ver `Config::organization`).
    pub organization: Option<Organization>,
    /// Formato antigo da organização (ligada = Dividida): lido para não perder a escolha de quem já
    /// usa e ainda gravado, para uma versão anterior entender o arquivo.
    pub split_enabled: bool,
    /// Destaque: fração da largura do centro que a janela grande ocupa (0 = padrão, 2/3).
    pub highlight_ratio: f32,
    /// Tamanho da janela no modo janela (nunca o da tela cheia: com a janela maximizada fica o de antes).
    pub window_width: f32,
    pub window_height: f32,
    /// A janela estava maximizada ao fechar: reabre maximizada, mas com o tamanho normal guardado acima.
    pub window_maximized: bool,
    /// Abre sozinho a aba de um agente fechado quando outro agente o chama pelo `ask-agent`.
    pub auto_open_agent_tabs: bool,
    /// Com o terminal focado, Ctrl+B é do app (barra lateral) em vez de ir ao terminal (prefixo
    /// do tmux); desligado, vai ao terminal. Ctrl+T é sempre do app.
    pub app_shortcuts_first: bool,
    /// Modelo padrão das instâncias novas, por `modelo@cargo` (`claude@dev`) ou só por `modelo`
    /// (`opencode`); o `modelo@cargo` vale mais. Sem entrada, vale o padrão do próprio CLI.
    pub role_models: BTreeMap<String, String>,
    /// O grupo de modelos que a barra de título mostra, por agente (`antigravity` → `Gemini 4 Argon`). Sem
    /// entrada (ou com um grupo que a fonte não devolve mais), vale o resumo.
    pub usage_pins: BTreeMap<String, String>,
    // --- Formato antigo (antes dos projetos): lido só para a migração, nunca gravado. Agora
    // cada projeto guarda as próprias abas e o próprio layout (ver `ProjectCfg`). ---
    /// Pastas conhecidas (`projects`); os projetos agora ficam em `projectList`.
    #[serde(skip_serializing)]
    pub projects: Vec<PathBuf>,
    #[serde(skip_serializing)]
    pub open_tabs: Vec<String>,
    #[serde(skip_serializing)]
    pub active_tab_index: i32,
    #[serde(skip_serializing)]
    pub pane_order: Vec<String>,
    #[serde(skip_serializing)]
    pub split_rows: Vec<f32>,
    #[serde(skip_serializing)]
    pub split_cols: Vec<Vec<f32>>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            project_list: Vec::new(),
            active_project: String::new(),
            workspace: PathBuf::new(),
            whisper_lang: "pt".into(),
            dictation: DictationChoice::default(),
            sound_enabled: true,
            sidebar_width: 220.0,
            sidebar_collapsed: false,
            right_panel_width: 0.0,
            organization: None,
            split_enabled: true,
            highlight_ratio: 0.0,
            window_width: 1280.0,
            window_height: 860.0,
            window_maximized: false,
            auto_open_agent_tabs: true,
            app_shortcuts_first: false,
            role_models: BTreeMap::new(),
            usage_pins: BTreeMap::new(),
            projects: Vec::new(),
            open_tabs: Vec::new(),
            active_tab_index: -1,
            pane_order: Vec::new(),
            split_rows: Vec::new(),
            split_cols: Vec::new(),
        }
    }
}

impl Config {
    /// A organização das janelas: a gravada ou, nos arquivos antigos, a que o `splitEnabled` dá.
    pub fn organization(&self) -> Organization {
        self.organization.unwrap_or_else(|| Organization::from_split_enabled(self.split_enabled))
    }

    /// Grava a organização nos dois campos (o antigo continua coerente).
    pub fn set_organization(&mut self, org: Organization) {
        self.organization = Some(org);
        self.split_enabled = org.split_enabled();
    }

    /// Configuração de uma instalação nova: nenhum projeto.
    pub fn default_for(_paths: &AppPaths) -> Self {
        Self::default()
    }

    /// Configuração com um único projeto, ativo, na pasta `path` (testes e uso embutido).
    pub fn with_project(path: PathBuf) -> Self {
        let project = ProjectCfg::new(path.clone(), None, &[]);
        Self { active_project: project.id.clone(), workspace: path, project_list: vec![project], ..Self::default() }
    }

    /// Carrega a configuração como o `loadConfig` do Electron: usa `paths.config_file` ou,
    /// se não existir, a configuração legada (Electron/"orquestra"). As chaves do arquivo
    /// sobrescrevem os padrões (como `Object.assign`). O formato antigo (um workspace global) é
    /// migrado para projetos (ver [`Config::fix_workspace`]).
    pub fn load(paths: &AppPaths) -> Self {
        Self::load_from(&config_candidates(paths), paths, &crate::util::home_dir())
    }

    /// Como [`Config::load`], com a lista de arquivos candidatos e a pasta pessoal explícitas.
    pub(crate) fn load_from(candidates: &[PathBuf], paths: &AppPaths, home: &Path) -> Self {
        let file = candidates.iter().find(|p| p.is_file());
        let mut cfg = match file.and_then(|f| std::fs::read_to_string(f).ok().map(|t| (f, t))) {
            Some((f, text)) => Self::try_from_json_over_defaults(&text, paths).unwrap_or_else(|| {
                // Ilegível: guarda uma cópia antes que a próxima gravação ponha os padrões por cima
                // (senão a lista de projetos se perde sem aviso).
                let backup = f.with_extension(format!("json.ilegivel-{}", crate::util::now_ms()));
                match std::fs::copy(f, &backup) {
                    Ok(_) => log::error!("configuração ilegível em {}; cópia guardada em {}", f.display(), backup.display()),
                    Err(e) => log::error!("configuração ilegível em {} (e a cópia falhou: {e})", f.display()),
                }
                Self::default_for(paths)
            }),
            None => Self::default_for(paths),
        };
        let from_legacy = file.filter(|f| **f != paths.config_file);
        // Da configuração do CommandDeck vem tudo (é o mesmo formato). Das mais antigas não vêm as
        // abas: a primeira abertura começa sem agentes.
        if from_legacy.is_some_and(|f| !is_commanddeck_config(f)) {
            cfg.open_tabs.clear();
            cfg.active_tab_index = -1;
            for p in &mut cfg.project_list {
                p.open_tabs.clear();
                p.active_tab = -1;
            }
        }
        cfg.fix_workspace(paths, home);
        // Migração do nome: copia a configuração do CommandDeck para o lugar novo (o antigo fica como
        // está, para quem ainda abrir a versão anterior).
        if let Some(f) = from_legacy.filter(|f| is_commanddeck_config(f)) {
            match cfg.save(&paths.config_file) {
                Ok(()) => log::info!("configuração copiada de {} para {}", f.display(), paths.config_file.display()),
                Err(e) => log::warn!("não consegui copiar a configuração de {} para {}: {e}", f.display(), paths.config_file.display()),
            }
        }
        cfg
    }

    /// Aplica o JSON salvo sobre os padrões. JSON inválido = padrões.
    pub fn from_json_over_defaults(text: &str, paths: &AppPaths) -> Self {
        Self::try_from_json_over_defaults(text, paths).unwrap_or_else(|| Self::default_for(paths))
    }

    /// Como [`Config::from_json_over_defaults`], mas `None` se o JSON não pôde ser lido.
    pub fn try_from_json_over_defaults(text: &str, paths: &AppPaths) -> Option<Self> {
        let defaults = Self::default_for(paths);
        let (Ok(serde_json::Value::Object(loaded)), Ok(serde_json::Value::Object(mut base))) =
            (serde_json::from_str::<serde_json::Value>(text), serde_json::to_value(&defaults))
        else {
            return None;
        };
        for (k, v) in loaded {
            // `null` não sobrescreve um padrão tipado (o serde rejeitaria o objeto inteiro).
            if !v.is_null() {
                base.insert(k, v);
            }
        }
        serde_json::from_value(serde_json::Value::Object(base)).ok()
    }

    /// Migra o formato antigo, normaliza as pastas dos projetos (relativa → relativa à pasta do
    /// app), valida o projeto ativo e deixa `workspace` = a pasta dele (vazia sem projeto).
    /// Idempotente.
    pub(crate) fn fix_workspace(&mut self, paths: &AppPaths, home: &Path) {
        self.migrate_legacy(paths, home);
        for p in &mut self.project_list {
            if p.path.is_relative() && !p.path.as_os_str().is_empty() {
                p.path = crate::util::normalize_lexical(&paths.app_dir.join(&p.path));
            }
            if p.name.is_empty() {
                p.name = folder_name(&p.path);
            }
        }
        // Projetos sem id ou sem pasta (config editada à mão) são descartados.
        self.project_list.retain(|p| !p.id.is_empty() && !p.path.as_os_str().is_empty());
        if self.project(&self.active_project).is_none() {
            self.active_project = self.project_list.first().map(|p| p.id.clone()).unwrap_or_default();
        }
        self.workspace = self.active().map(|p| p.path.clone()).unwrap_or_default();
    }

    /// Formato antigo (`projects` + `workspace` + abas e layout globais) → um projeto por pasta.
    /// As abas e o layout vão para o projeto da pasta do `workspace`, que fica ativo. Regras do
    /// workspace antigo: vazio → o workspace do app; relativo → relativo à pasta do app; a pasta
    /// pessoal do usuário → o workspace do app.
    fn migrate_legacy(&mut self, paths: &AppPaths, home: &Path) {
        let has_legacy = !self.projects.is_empty() || !self.workspace.as_os_str().is_empty();
        if self.project_list.is_empty() && has_legacy {
            let fix = |dir: &Path| -> PathBuf {
                let dir = if dir.as_os_str().is_empty() {
                    paths.default_workspace.clone()
                } else if dir.is_relative() {
                    crate::util::normalize_lexical(&paths.app_dir.join(dir))
                } else {
                    dir.to_path_buf()
                };
                let is_home =
                    !home.as_os_str().is_empty() && crate::util::same_path(&crate::util::absolutize(&dir), &crate::util::absolutize(home));
                if is_home {
                    paths.default_workspace.clone()
                } else {
                    dir
                }
            };
            let ws = fix(&self.workspace);
            let mut dirs: Vec<PathBuf> = Vec::new();
            for dir in self.projects.iter().map(|d| fix(d)).chain(std::iter::once(ws.clone())) {
                if !dirs.iter().any(|d| crate::util::same_path(d, &dir)) {
                    dirs.push(dir);
                }
            }
            for dir in dirs {
                let mut project = ProjectCfg::new(dir, None, &self.project_list);
                if crate::util::same_path(&project.path, &ws) {
                    project.open_tabs = std::mem::take(&mut self.open_tabs);
                    project.active_tab = self.active_tab_index;
                    project.pane_order = std::mem::take(&mut self.pane_order);
                    project.split_rows = std::mem::take(&mut self.split_rows);
                    project.split_cols = std::mem::take(&mut self.split_cols);
                    self.active_project = project.id.clone();
                }
                self.project_list.push(project);
            }
        }
        self.projects.clear();
        self.open_tabs.clear();
        self.active_tab_index = -1;
        self.pane_order.clear();
        self.split_rows.clear();
        self.split_cols.clear();
    }

    /// O projeto `id`.
    pub fn project(&self, id: &str) -> Option<&ProjectCfg> {
        self.project_list.iter().find(|p| p.id == id)
    }

    /// O projeto ativo.
    pub fn active(&self) -> Option<&ProjectCfg> {
        self.project(&self.active_project)
    }

    /// Persiste em JSON indentado (2 espaços, como `JSON.stringify(config, null, 2)`).
    pub fn save(&self, file: &Path) -> std::io::Result<()> {
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        // Grava ao lado e troca: uma queda no meio não deixa o arquivo pela metade.
        let tmp = file.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, file)
    }
}

/// Configurações de versões anteriores: `CommandDeck` (o nome antes de AgentDeck, no mesmo formato),
/// a versão Electron (userData = "CommandDeck Terminal") e o nome antigo "orquestra". A primeira que
/// existir é a origem quando `AgentDeck/config.json` ainda não existe.
///
/// ÚNICA reserva do nome CommandDeck que o app ainda carrega (as variáveis `COMMANDDECK_*`, o cabeçalho
/// da ponte, o marcador do `AGENTS.md` e a trava saíram na t32): quem nunca abriu o AgentDeck perderia a
/// lista de projetos. Sai (esta função, `is_commanddeck_config`, a cópia em `load_from` e os testes
/// `*_commanddeck_*`) numa versão depois da promoção da `dev` para a `main`; ver `memory/pendencias.md`.
pub(crate) fn legacy_config_files() -> Vec<PathBuf> {
    let Some(base) = dirs::config_dir() else { return Vec::new() };
    vec![
        base.join("CommandDeck").join("config.json"),
        base.join("CommandDeck Terminal").join("config.json"),
        base.join("orquestra").join("config.json"),
    ]
}

/// Arquivos a tentar, em ordem: o de `paths.config_file` e, depois, os das versões anteriores. Com
/// `AGENTDECK_HOME` (a configuração fica dentro da pasta do app) o app é isolado e não lê as
/// configurações do usuário.
pub(crate) fn config_candidates(paths: &AppPaths) -> Vec<PathBuf> {
    let mut candidates = vec![paths.config_file.clone()];
    if !paths.config_file.starts_with(&paths.app_dir) {
        candidates.extend(legacy_config_files());
    }
    candidates
}

/// `true` se `file` é a configuração do CommandDeck: o mesmo formato do AgentDeck, então abas e
/// layout valem e o arquivo é copiado para o lugar novo.
fn is_commanddeck_config(file: &Path) -> bool {
    file.parent().and_then(Path::file_name).is_some_and(|d| d == "CommandDeck")
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn paths(root: &Path) -> AppPaths {
        AppPaths {
            app_dir: root.to_path_buf(),
            bin_dir: root.join("bin"),
            data_dir: root.join("dados"),
            default_workspace: root.join("workspace"),
            whisper_dir: root.join("whisper"),
            config_file: root.join("cfg").join("config.json"),
        }
    }

    fn no_home() -> &'static Path {
        Path::new("/nonexistent-home")
    }

    #[test]
    fn the_old_split_flag_still_picks_the_organization() {
        let tmp = tempfile::tempdir().expect("tmp");
        let p = paths(tmp.path());
        let read = |json: &str| Config::from_json_over_defaults(json, &p);
        // Sem `organization` (arquivo antigo): vale o `splitEnabled`, ou o padrão (Dividida).
        assert_eq!(read("{\"splitEnabled\": false}").organization(), Organization::Tabs);
        assert_eq!(read("{\"splitEnabled\": true}").organization(), Organization::Split);
        assert_eq!(read("{}").organization(), Organization::Split);
        // A organização gravada manda, mesmo com o `splitEnabled` dizendo outra coisa.
        assert_eq!(read("{\"organization\": \"highlight\", \"splitEnabled\": false}").organization(), Organization::Highlight);
        assert_eq!(read("{\"organization\": \"tabs\"}").organization(), Organization::Tabs);
        // Gravar mantém o campo antigo coerente e volta igual.
        let mut cfg = Config::default();
        cfg.set_organization(Organization::Highlight);
        assert!(cfg.split_enabled);
        let text = serde_json::to_string(&cfg).expect("json");
        assert!(text.contains("\"organization\":\"highlight\""), "{text}");
        assert_eq!(read(&text).organization(), Organization::Highlight);
        cfg.set_organization(Organization::Tabs);
        assert!(!cfg.split_enabled);
    }

    /// A escolha do ditado: arquivo antigo = "não perguntado"; um instalador pode gravar `"dictation": "yes"|"no"`.
    #[test]
    fn the_dictation_choice_defaults_to_unasked_and_is_writable_from_outside() {
        let read = |json: &str| serde_json::from_str::<Config>(json).expect("config");
        assert_eq!(read("{}").dictation, DictationChoice::Unasked);
        assert_eq!(read("{\"dictation\": \"yes\"}").dictation, DictationChoice::Yes);
        assert_eq!(read("{\"dictation\": \"no\"}").dictation, DictationChoice::No);
        let text = serde_json::to_string(&Config { dictation: DictationChoice::No, ..Config::default() }).expect("json");
        assert!(text.contains("\"dictation\":\"no\""), "{text}");
    }

    #[test]
    fn a_config_saved_with_the_removed_ollama_agent_still_loads() {
        let tmp = tempfile::tempdir().expect("tmp");
        let p = paths(tmp.path());
        let text = r#"{ "ollamaHost": "127.0.0.1:11434", "whisperLang": "en",
            "projectList": [{ "id": "p1", "path": "C:/x", "openTabs": ["claude", "ollama:qwen3.5:9b"] }] }"#;
        let cfg = Config::from_json_over_defaults(text, &p);
        assert_eq!(cfg.whisper_lang, "en");
        assert_eq!(cfg.project_list[0].open_tabs, ["claude", "ollama:qwen3.5:9b"]);
        let saved = serde_json::to_string(&cfg).expect("json");
        assert!(!saved.contains("ollamaHost"), "{saved}");
    }

    #[test]
    fn roundtrip_and_merge() {
        let tmp = tempfile::tempdir().expect("tmp");
        let p = paths(tmp.path());
        let mut cfg = Config::default_for(&p);
        cfg.whisper_lang = "en".into();
        let mut project = ProjectCfg::new(tmp.path().join("proj"), None, &[]);
        project.open_tabs = vec!["claude".into(), "claude@dev".into()];
        project.split_rows = vec![0.4, 0.6];
        cfg.active_project = project.id.clone();
        cfg.project_list.push(project.clone());
        cfg.sidebar_width = 300.0;
        cfg.save(&p.config_file).expect("save");
        let text = std::fs::read_to_string(&p.config_file).expect("read");
        assert!(text.contains("\n  \"whisperLang\": \"en\""), "JSON indentado com camelCase: {text}");
        assert!(text.contains("\"projectList\"") && text.contains("\"openTabs\"") && text.contains("\"splitRows\""), "{text}");
        // Os campos do formato antigo não são mais gravados.
        for old in ["\"workspace\"", "\"projects\"", "activeTabIndex"] {
            assert!(!text.contains(old), "{old} gravado: {text}");
        }
        let back = Config::load_from(std::slice::from_ref(&p.config_file), &p, no_home());
        assert_eq!(back.whisper_lang, "en");
        assert_eq!(back.project_list, vec![project.clone()]);
        assert_eq!(back.active_project, project.id);
        assert_eq!(back.sidebar_width, 300.0);
        assert_eq!(back.workspace, tmp.path().join("proj"));
        assert!(!p.config_file.with_extension("json.tmp").exists(), "o temporário vira o arquivo");

        // Arquivo ilegível: volta aos padrões, mas guarda uma cópia antes que alguém grave por cima.
        std::fs::write(&p.config_file, "{\"projectList\": [ quebrado").expect("write");
        let broken = Config::load_from(std::slice::from_ref(&p.config_file), &p, no_home());
        assert!(broken.project_list.is_empty());
        let dir = p.config_file.parent().expect("dir");
        let kept: Vec<_> = std::fs::read_dir(dir)
            .expect("ls")
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .filter(|n| n.contains("ilegivel"))
            .collect();
        assert_eq!(kept.len(), 1, "cópia do arquivo ilegível: {kept:?}");
    }

    #[test]
    fn models_are_saved_per_instance_and_per_role() {
        let tmp = tempfile::tempdir().expect("tmp");
        let p = paths(tmp.path());
        let mut project = ProjectCfg::new(tmp.path().join("proj"), None, &[]);
        project.tab_models.insert("claude@dev".into(), "sonnet".into());
        let mut cfg = Config { active_project: project.id.clone(), project_list: vec![project.clone()], ..Config::default() };
        cfg.role_models.insert("opencode".into(), "opencode/glm-5.3-flash-free".into());
        cfg.usage_pins.insert("antigravity".into(), "Gemini 4 Argon".into());
        cfg.save(&p.config_file).expect("save");
        let text = std::fs::read_to_string(&p.config_file).expect("read");
        assert!(text.contains("\"tabModels\"") && text.contains("\"roleModels\""), "{text}");
        let back = Config::load_from(std::slice::from_ref(&p.config_file), &p, no_home());
        assert_eq!(back.project_list, vec![project]);
        assert_eq!(back.role_models, cfg.role_models);
        assert_eq!(back.usage_pins, cfg.usage_pins);
        // Uma config antiga, sem os campos, continua valendo.
        let old = Config::from_json_over_defaults(r#"{"projectList":[{"id":"p1","name":"x","path":"/x"}]}"#, &p);
        assert!(old.project_list[0].tab_models.is_empty() && old.role_models.is_empty());
    }

    #[test]
    fn project_ids_are_short_and_distinct() {
        let a = ProjectCfg::new(PathBuf::from("/x/meu-app"), None, &[]);
        assert_eq!(a.id.len(), 9);
        assert!(a.id.starts_with('p') && a.id[1..].chars().all(|c| c.is_ascii_hexdigit()), "{}", a.id);
        assert_eq!(a.name, "meu-app");
        assert_eq!(a.active_tab, -1);
        let b = ProjectCfg::new(PathBuf::from("/x/outro"), Some("  Outro nome "), std::slice::from_ref(&a));
        assert_ne!(a.id, b.id);
        assert_eq!(b.name, "Outro nome");
    }

    #[test]
    fn the_old_bottom_panel_keys_are_ignored() {
        let tmp = tempfile::tempdir().expect("tmp");
        let p = paths(tmp.path());
        let cfg = Config::from_json_over_defaults(r#"{"bottomHeight": 420, "feedCollapsed": true, "sidebarWidth": 250}"#, &p);
        assert_eq!((cfg.sidebar_width, cfg.right_panel_width), (250.0, 0.0));
        let cfg = Config::from_json_over_defaults(r#"{"rightPanelWidth": 410}"#, &p);
        assert_eq!(cfg.right_panel_width, 410.0);
        let text = serde_json::to_string(&cfg).expect("json");
        assert!(!text.contains("bottomHeight") && !text.contains("feedCollapsed"));
    }

    #[test]
    fn legacy_fallback_is_used() {
        let tmp = tempfile::tempdir().expect("tmp");
        let p = paths(tmp.path());
        let legacy = tmp.path().join("orquestra").join("config.json");
        std::fs::create_dir_all(legacy.parent().expect("pai")).expect("mkdir");
        let ws = tmp.path().join("ws-antigo");
        let json = serde_json::json!({"whisperLang": "es", "workspace": ws, "openTabs": ["antigravity", "claude"], "activeTabIndex": 1});
        std::fs::write(&legacy, json.to_string()).expect("write");
        let cfg = Config::load_from(&[p.config_file.clone(), legacy], &p, no_home());
        assert_eq!(cfg.whisper_lang, "es");
        let project = cfg.active().expect("o workspace vira projeto");
        assert_eq!(project.path, ws);
        assert!(project.open_tabs.is_empty(), "as abas da config legada não são abertas");
        assert_eq!(project.active_tab, -1);
    }

    /// Migração do nome: a configuração do CommandDeck vira a do AgentDeck, com projetos e abas, e o
    /// arquivo antigo continua lá.
    #[test]
    fn the_commanddeck_config_is_copied_to_the_new_place() {
        let tmp = tempfile::tempdir().expect("tmp");
        let p = paths(tmp.path());
        let old = tmp.path().join("CommandDeck").join("config.json");
        std::fs::create_dir_all(old.parent().expect("pai")).expect("mkdir");
        let mut project = ProjectCfg::new(tmp.path().join("meu-projeto"), None, &[]);
        project.open_tabs = vec!["claude".into(), "claude@dev".into()];
        project.active_tab = 1;
        let before = Config {
            active_project: project.id.clone(),
            project_list: vec![project.clone()],
            whisper_lang: "es".into(),
            ..Config::default()
        };
        before.save(&old).expect("grava a antiga");
        let old_text = std::fs::read_to_string(&old).expect("read");
        assert!(!p.config_file.exists());

        let cfg = Config::load_from(&[p.config_file.clone(), old.clone()], &p, no_home());
        assert_eq!(cfg.whisper_lang, "es");
        assert_eq!(cfg.project_list, vec![project.clone()], "projetos e abas vêm junto");
        // Gravou no lugar novo e deixou o antigo como estava.
        assert!(p.config_file.is_file());
        assert_eq!(std::fs::read_to_string(&old).expect("antigo"), old_text);
        let again = Config::load_from(std::slice::from_ref(&p.config_file), &p, no_home());
        assert_eq!(again.project_list, vec![project]);
        assert_eq!(again.whisper_lang, "es");
    }

    /// Com a configuração nova já existente, a antiga não é lida nem sobrescreve nada.
    #[test]
    fn the_new_config_wins_over_the_commanddeck_one() {
        let tmp = tempfile::tempdir().expect("tmp");
        let p = paths(tmp.path());
        let old = tmp.path().join("CommandDeck").join("config.json");
        std::fs::create_dir_all(old.parent().expect("pai")).expect("mkdir");
        std::fs::write(&old, r#"{"whisperLang": "es"}"#).expect("write");
        Config { whisper_lang: "en".into(), ..Config::default() }.save(&p.config_file).expect("grava a nova");
        let cfg = Config::load_from(&[p.config_file.clone(), old.clone()], &p, no_home());
        assert_eq!(cfg.whisper_lang, "en");
        assert_eq!(std::fs::read_to_string(&old).expect("antigo"), r#"{"whisperLang": "es"}"#);
    }

    #[test]
    fn candidates_start_with_the_new_file_then_commanddeck() {
        let tmp = tempfile::tempdir().expect("tmp");
        let normal = AppPaths { config_file: tmp.path().join("cfg-do-usuario").join("config.json"), ..paths(&tmp.path().join("app")) };
        let c = config_candidates(&normal);
        assert_eq!(c[0], normal.config_file);
        if dirs::config_dir().is_some() {
            assert_eq!(c[1].parent().and_then(Path::file_name).and_then(|n| n.to_str()), Some("CommandDeck"));
            assert_eq!(legacy_config_files().len(), c.len() - 1);
        }
        // Isolado (`AGENTDECK_HOME`): não lê as configurações do usuário.
        assert_eq!(config_candidates(&paths(tmp.path())), vec![paths(tmp.path()).config_file]);
    }

    #[test]
    fn partial_json_keeps_defaults() {
        let tmp = tempfile::tempdir().expect("tmp");
        let p = paths(tmp.path());
        let cfg = Config::from_json_over_defaults(r#"{"soundEnabled": false, "extra": 1, "workspace": null}"#, &p);
        assert!(!cfg.sound_enabled);
        assert_eq!(cfg.whisper_lang, "pt");
        // JSON inválido → padrões.
        let cfg = Config::from_json_over_defaults("{oops", &p);
        assert!(cfg.project_list.is_empty());
    }

    #[test]
    fn a_new_install_has_no_project() {
        let tmp = tempfile::tempdir().expect("tmp");
        let p = paths(tmp.path());
        let cfg = Config::load_from(std::slice::from_ref(&p.config_file), &p, no_home());
        assert!(cfg.project_list.is_empty());
        assert_eq!(cfg.active_project, "");
        assert_eq!(cfg.workspace, PathBuf::new(), "não existe mais workspace padrão");
    }

    #[test]
    fn the_old_format_is_migrated_with_tabs_and_layout() {
        let tmp = tempfile::tempdir().expect("tmp");
        let p = paths(tmp.path());
        let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
        let json = serde_json::json!({
            "workspace": b,
            "projects": [a, b, "rel"],
            "openTabs": ["claude", "claude@dev"],
            "activeTabIndex": 1,
            "paneOrder": ["claude", "shell"],
            "splitRows": [0.3, 0.7],
            "splitCols": [[0.5, 0.5]],
            "soundEnabled": false
        });
        std::fs::create_dir_all(p.config_file.parent().expect("pai")).expect("mkdir");
        std::fs::write(&p.config_file, json.to_string()).expect("write");
        let cfg = Config::load_from(std::slice::from_ref(&p.config_file), &p, no_home());
        let paths_of: Vec<&Path> = cfg.project_list.iter().map(|p| p.path.as_path()).collect();
        assert_eq!(paths_of, [a.as_path(), b.as_path(), tmp.path().join("rel").as_path()]);
        assert_eq!(cfg.project_list.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["a", "b", "rel"]);
        let active = cfg.active().expect("ativo");
        assert_eq!(active.path, b);
        assert_eq!(active.open_tabs, ["claude", "claude@dev"]);
        assert_eq!(active.active_tab, 1);
        assert_eq!(active.pane_order, ["claude", "shell"]);
        assert_eq!((active.split_rows.clone(), active.split_cols.clone()), (vec![0.3, 0.7], vec![vec![0.5, 0.5]]));
        assert!(cfg.project_list[0].open_tabs.is_empty(), "as abas globais vão só para o projeto do workspace");
        assert_eq!(cfg.workspace, b);
        assert!(!cfg.sound_enabled);

        // Gravada e lida de novo: igual, e sem migrar outra vez.
        cfg.save(&p.config_file).expect("save");
        let again = Config::load_from(std::slice::from_ref(&p.config_file), &p, no_home());
        assert_eq!(again.project_list, cfg.project_list);
        assert_eq!(again.active_project, cfg.active_project);
    }

    #[test]
    fn the_workspace_outside_the_list_also_becomes_a_project() {
        let tmp = tempfile::tempdir().expect("tmp");
        let p = paths(tmp.path());
        let home = tmp.path().join("home").join("user");
        // Workspace na pasta pessoal → o workspace do app (regra antiga), que vira um projeto.
        let mut cfg = Config { workspace: home.join("."), projects: vec![home.join("proj")], ..Config::default() };
        cfg.fix_workspace(&p, &home);
        assert_eq!(cfg.project_list.iter().map(|p| p.path.clone()).collect::<Vec<_>>(), [home.join("proj"), p.default_workspace.clone()]);
        assert_eq!(cfg.workspace, p.default_workspace);
        // Idempotente.
        let before = cfg.project_list.clone();
        cfg.fix_workspace(&p, &home);
        assert_eq!(cfg.project_list, before);

        let mut cfg = Config { workspace: PathBuf::from("rel"), ..Config::default() };
        cfg.fix_workspace(&p, &home);
        assert_eq!(cfg.workspace, tmp.path().join("rel"));
        assert_eq!(cfg.project_list.len(), 1);
    }

    #[test]
    fn an_unknown_active_project_falls_back_to_the_first() {
        let tmp = tempfile::tempdir().expect("tmp");
        let p = paths(tmp.path());
        let first = ProjectCfg::new(tmp.path().join("x"), None, &[]);
        let mut cfg = Config { project_list: vec![first.clone()], active_project: "pdeadbeef".into(), ..Config::default() };
        cfg.fix_workspace(&p, no_home());
        assert_eq!(cfg.active_project, first.id);
        assert_eq!(cfg.workspace, first.path);
    }
}
