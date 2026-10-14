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
    /// Resolve a partir do executável atual. `COMMANDDECK_HOME` sobrescreve `app_dir` e isola o app
    /// por completo: a configuração também fica dentro dessa pasta (`cfg/config.json`), então testes
    /// e instâncias paralelas não tocam na configuração do usuário.
    /// Em desenvolvimento (`target/debug|release`), sobe até achar a raiz do repositório
    /// (pasta com `workspace/` e `dados/`), para reaproveitar as pastas da versão Electron.
    pub fn detect() -> Self {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        match std::env::var_os("COMMANDDECK_HOME").filter(|v| !v.is_empty()).map(PathBuf::from) {
            Some(home) => Self::isolated(home, exe_dir),
            None => Self::with_dirs(find_app_dir(&exe_dir), exe_dir),
        }
    }

    /// Como [`AppPaths::with_dirs`], mas com a configuração dentro de `app_dir`.
    pub fn isolated(app_dir: PathBuf, bin_dir: PathBuf) -> Self {
        let config_file = app_dir.join("cfg").join("config.json");
        Self { config_file, ..Self::with_dirs(app_dir, bin_dir) }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolated_home_keeps_the_config_inside_it() {
        let home = PathBuf::from("/tmp/commanddeck-home");
        let p = AppPaths::isolated(home.clone(), PathBuf::from("/tmp/bin"));
        assert_eq!(p.config_file, home.join("cfg").join("config.json"));
        assert_eq!(p.data_dir, home.join("dados"));
        // Sem o isolamento a configuração fica na pasta do usuário, fora do app.
        let normal = AppPaths::with_dirs(home.clone(), PathBuf::from("/tmp/bin"));
        assert!(!normal.config_file.starts_with(&home));
    }
}
