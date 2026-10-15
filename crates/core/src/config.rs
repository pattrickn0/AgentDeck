//! Configuração persistida (equivalente ao `config.json` do Electron, com campos extras de layout).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths::AppPaths;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub workspace: PathBuf,
    pub whisper_lang: String,
    pub sound_enabled: bool,
    /// `kind` de cada aba aberta, na ordem da barra (restaurado na próxima abertura).
    pub open_tabs: Vec<String>,
    pub active_tab_index: i32,
    // --- Layout (novidade da versão Rust; a versão Electron guardava só em CSS) ---
    pub sidebar_width: f32,
    pub sidebar_collapsed: bool,
    /// Largura da barra lateral direita (Conversa integrada + compositor); 0 = padrão. Ela não
    /// recolhe. (Substitui `bottomHeight`/`feedCollapsed` do antigo painel de baixo, ignorados ao ler.)
    pub right_panel_width: f32,
    /// Tela dividida: uma janela por tipo de agente (desligada = uma aba por vez).
    pub split_enabled: bool,
    /// Ordem das janelas da tela dividida (grupos: `claude`, `antigravity`, `ollama`…).
    pub pane_order: Vec<String>,
    /// Proporções das linhas da grade e das colunas de cada linha (vazias = iguais). Só valem se
    /// o formato da grade ainda for o mesmo.
    pub split_rows: Vec<f32>,
    pub split_cols: Vec<Vec<f32>>,
    pub window_width: f32,
    pub window_height: f32,
    /// Host da API do Ollama (padrão `127.0.0.1:11434`, ou `OLLAMA_HOST`).
    pub ollama_host: String,
    /// Abre sozinho a aba de um agente fechado quando outro agente o chama pelo `ask-agent`.
    pub auto_open_agent_tabs: bool,
    /// Projetos / workspaces conhecidos pelo usuário.
    pub projects: Vec<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            workspace: PathBuf::from("workspace"),
            whisper_lang: "pt".into(),
            sound_enabled: true,
            open_tabs: Vec::new(),
            active_tab_index: -1,
            sidebar_width: 220.0,
            sidebar_collapsed: false,
            right_panel_width: 0.0,
            split_enabled: true,
            pane_order: Vec::new(),
            split_rows: Vec::new(),
            split_cols: Vec::new(),
            window_width: 1280.0,
            window_height: 860.0,
            ollama_host: std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "127.0.0.1:11434".into()),
            auto_open_agent_tabs: true,
            projects: Vec::new(),
        }
    }
}

impl Config {
    pub fn default_for(paths: &AppPaths) -> Self {
        let ws = paths.default_workspace.clone();
        Self { workspace: ws.clone(), projects: vec![ws], ..Self::default() }
    }

    /// Carrega a configuração como o `loadConfig` do Electron: usa `paths.config_file` ou,
    /// se não existir, a configuração legada (Electron/"orquestra"). As chaves do arquivo
    /// sobrescrevem os padrões (como `Object.assign`). Um workspace igual à pasta pessoal
    /// do usuário é redirecionado para o workspace isolado do app.
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

    /// Regras do workspace: vazio → padrão; relativo → relativo à pasta do app;
    /// pasta pessoal do usuário → workspace isolado do app.
    pub(crate) fn fix_workspace(&mut self, paths: &AppPaths, home: &Path) {
        if self.workspace.as_os_str().is_empty() {
            self.workspace = paths.default_workspace.clone();
        } else if self.workspace.is_relative() {
            self.workspace = crate::util::normalize_lexical(&paths.app_dir.join(&self.workspace));
        }
        if !home.as_os_str().is_empty() && crate::util::same_path(&crate::util::absolutize(&self.workspace), &crate::util::absolutize(home))
        {
            self.workspace = paths.default_workspace.clone();
        }
        if !self.projects.contains(&self.workspace) {
            self.projects.insert(0, self.workspace.clone());
        }
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

    #[test]
    fn roundtrip_and_merge() {
        let tmp = tempfile::tempdir().expect("tmp");
        let p = paths(tmp.path());
        let mut cfg = Config::default_for(&p);
        cfg.whisper_lang = "en".into();
        cfg.open_tabs = vec!["claude".into(), "ollama:qwen3.5:9b".into()];
        cfg.sidebar_width = 300.0;
        cfg.save(&p.config_file).expect("save");
        let text = std::fs::read_to_string(&p.config_file).expect("read");
        assert!(text.contains("\n  \"whisperLang\": \"en\""), "JSON indentado com camelCase: {text}");
        let back = Config::load_from(std::slice::from_ref(&p.config_file), &p, Path::new("/nonexistent-home"));
        assert_eq!(back.whisper_lang, "en");
        assert_eq!(back.open_tabs, cfg.open_tabs);
        assert_eq!(back.sidebar_width, 300.0);
        assert_eq!(back.workspace, cfg.workspace);
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
        std::fs::write(&legacy, r#"{"whisperLang":"es"}"#).expect("write");
        let cfg = Config::load_from(&[p.config_file.clone(), legacy], &p, Path::new("/nonexistent-home"));
        assert_eq!(cfg.whisper_lang, "es");
    }

    #[test]
    fn partial_json_keeps_defaults() {
        let tmp = tempfile::tempdir().expect("tmp");
        let p = paths(tmp.path());
        let cfg = Config::from_json_over_defaults(r#"{"soundEnabled": false, "extra": 1, "workspace": null}"#, &p);
        assert!(!cfg.sound_enabled);
        assert_eq!(cfg.whisper_lang, "pt");
        assert_eq!(cfg.workspace, p.default_workspace);
        // JSON inválido → padrões.
        let cfg = Config::from_json_over_defaults("{oops", &p);
        assert_eq!(cfg.workspace, p.default_workspace);
    }

    #[test]
    fn home_workspace_is_redirected() {
        let tmp = tempfile::tempdir().expect("tmp");
        let p = paths(tmp.path());
        let home = tmp.path().join("home").join("user");
        let mut cfg = Config { workspace: home.join("."), ..Config::default_for(&p) };
        cfg.fix_workspace(&p, &home);
        assert_eq!(cfg.workspace, p.default_workspace);

        let mut cfg = Config { workspace: home.join("proj"), ..Config::default_for(&p) };
        cfg.fix_workspace(&p, &home);
        assert_eq!(cfg.workspace, home.join("proj"));

        let mut cfg = Config { workspace: PathBuf::from("rel"), ..Config::default_for(&p) };
        cfg.fix_workspace(&p, &home);
        assert_eq!(cfg.workspace, tmp.path().join("rel"));
    }
}
