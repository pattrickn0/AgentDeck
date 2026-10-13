//! Localização de pastas do app.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AppPaths {
    /// Pasta base do app (onde ficam `dados/`, `workspace/`, `whisper/`).
    pub app_dir: PathBuf,
    /// Pasta dos executáveis (`commanddeck`, `ask-agent`, `ollama-agent`). Vai no início do PATH dos agentes.
    pub bin_dir: PathBuf,
    /// `<app_dir>/dados`
    pub data_dir: PathBuf,
    /// `<app_dir>/workspace`
    pub default_workspace: PathBuf,
    /// `<app_dir>/whisper`
    pub whisper_dir: PathBuf,
    /// `<config_dir>/CommandDeck/config.json`
    pub config_file: PathBuf,
}

impl AppPaths {
    /// Resolve a partir do executável atual. `COMMANDDECK_HOME` sobrescreve `app_dir`.
    /// Em desenvolvimento (`target/debug|release`), sobe até achar a raiz do repositório
    /// (pasta com `workspace/` e `dados/`), para reaproveitar as pastas da versão Electron.
    pub fn detect() -> Self {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        let app_dir = std::env::var_os("COMMANDDECK_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| find_app_dir(&exe_dir));
        Self::with_dirs(app_dir, exe_dir)
    }

    pub fn with_dirs(app_dir: PathBuf, bin_dir: PathBuf) -> Self {
        let config_file = dirs::config_dir()
            .unwrap_or_else(|| app_dir.clone())
            .join("CommandDeck")
            .join("config.json");
        Self {
            data_dir: app_dir.join("dados"),
            default_workspace: app_dir.join("workspace"),
            whisper_dir: app_dir.join("whisper"),
            config_file,
            bin_dir,
            app_dir,
        }
    }
}

fn find_app_dir(exe_dir: &Path) -> PathBuf {
    for dir in exe_dir.ancestors() {
        if dir.join("workspace").is_dir() && dir.join("dados").is_dir() {
            return dir.to_path_buf();
        }
    }
    exe_dir.to_path_buf()
}
