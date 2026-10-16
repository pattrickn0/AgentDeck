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
    /// Ordem das janelas da tela dividida (grupos: `claude`, `antigravity`, `ollama`…).
    pub pane_order: Vec<String>,
    /// Proporções das linhas da grade e das colunas de cada linha (vazias = iguais). Só valem se
    /// o formato da grade ainda for o mesmo.
    pub split_rows: Vec<f32>,
    pub split_cols: Vec<Vec<f32>>,
    /// Quem abriu cada aba (id da aba → id de quem a chamou pelo `ask-agent`): a hierarquia da
    /// lista de agentes na barra lateral. Abas abertas pelo usuário não entram.
    pub tab_parents: BTreeMap<String, String>,
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
    pub sound_enabled: bool,
    // --- Layout (novidade da versão Rust; a versão Electron guardava só em CSS) ---
    pub sidebar_width: f32,
    pub sidebar_collapsed: bool,
    /// Largura da barra lateral direita (Conversa integrada + compositor); 0 = padrão. Ela não
    /// recolhe. (Substitui `bottomHeight`/`feedCollapsed` do antigo painel de baixo, ignorados ao ler.)
    pub right_panel_width: f32,
    /// Tela dividida: uma janela por tipo de agente (desligada = uma aba por vez).
    pub split_enabled: bool,
    pub window_width: f32,
    pub window_height: f32,
    /// Host da API do Ollama (padrão `127.0.0.1:11434`, ou `OLLAMA_HOST`).
    pub ollama_host: String,
    /// Abre sozinho a aba de um agente fechado quando outro agente o chama pelo `ask-agent`.
    pub auto_open_agent_tabs: bool,
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
            sound_enabled: true,
            sidebar_width: 220.0,
            sidebar_collapsed: false,
            right_panel_width: 0.0,
            split_enabled: true,
            window_width: 1280.0,
            window_height: 860.0,
            ollama_host: std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "127.0.0.1:11434".into()),
            auto_open_agent_tabs: true,
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
        let mut candidates = vec![paths.config_file.clone()];
        candidates.extend(legacy_config_files());
        Self::load_from(&candidates, paths, &crate::util::home_dir())
    }

    /// Como [`Config::load`], com a lista de arquivos candidatos e a pasta pessoal explícitas.
    pub(crate) fn load_from(candidates: &[PathBuf], paths: &AppPaths, home: &Path) -> Self {
        let file = candidates.iter().find(|p| p.is_file());
        let mut cfg = match file.and_then(|f| std::fs::read_to_string(f).ok()) {
            Some(text) => Self::from_json_over_defaults(&text, paths),
            None => Self::default_for(paths),
        };
        // Da configuração legada não vêm as abas: a primeira abertura começa sem agentes.
        if file.is_some_and(|f| *f != paths.config_file) {
            cfg.open_tabs.clear();
            cfg.active_tab_index = -1;
            for p in &mut cfg.project_list {
                p.open_tabs.clear();
                p.active_tab = -1;
            }
        }
        cfg.fix_workspace(paths, home);
        cfg
    }

    /// Aplica o JSON salvo sobre os padrões. JSON inválido = padrões.
    pub fn from_json_over_defaults(text: &str, paths: &AppPaths) -> Self {
        let defaults = Self::default_for(paths);
        let (Ok(serde_json::Value::Object(loaded)), Ok(serde_json::Value::Object(mut base))) =
            (serde_json::from_str::<serde_json::Value>(text), serde_json::to_value(&defaults))
        else {
            return defaults;
        };
        for (k, v) in loaded {
            // `null` não sobrescreve um padrão tipado (o serde rejeitaria o objeto inteiro).
            if !v.is_null() {
                base.insert(k, v);
            }
        }
        serde_json::from_value(serde_json::Value::Object(base)).unwrap_or(defaults)
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
        std::fs::write(file, text)
    }
}

/// Configurações da versão Electron (userData = "CommandDeck Terminal") e do nome antigo "orquestra".
pub(crate) fn legacy_config_files() -> Vec<PathBuf> {
    let Some(base) = dirs::config_dir() else { return Vec::new() };
    vec![base.join("CommandDeck Terminal").join("config.json"), base.join("orquestra").join("config.json")]
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
    fn roundtrip_and_merge() {
        let tmp = tempfile::tempdir().expect("tmp");
        let p = paths(tmp.path());
        let mut cfg = Config::default_for(&p);
        cfg.whisper_lang = "en".into();
        let mut project = ProjectCfg::new(tmp.path().join("proj"), None, &[]);
        project.open_tabs = vec!["claude".into(), "ollama:qwen3.5:9b".into()];
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
