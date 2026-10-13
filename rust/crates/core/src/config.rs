//! Configuração persistida (equivalente ao `config.json` do Electron, com campos extras de layout).

use std::path::PathBuf;

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
    pub bottom_height: f32,
    pub feed_collapsed: bool,
    pub window_width: f32,
    pub window_height: f32,
    /// Host da API do Ollama (padrão `127.0.0.1:11434`, ou `OLLAMA_HOST`).
    pub ollama_host: String,
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
            bottom_height: 310.0,
            feed_collapsed: false,
            window_width: 1280.0,
            window_height: 860.0,
            ollama_host: std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "127.0.0.1:11434".into()),
        }
    }
}

impl Config {
    pub fn default_for(paths: &AppPaths) -> Self {
        Self { workspace: paths.default_workspace.clone(), ..Self::default() }
    }
}
