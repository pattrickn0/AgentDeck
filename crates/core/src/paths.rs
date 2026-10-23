//! Localização de pastas do app.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AppPaths {
    /// Pasta base do app (onde ficam `dados/`, `workspace/`, `whisper/`).
    pub app_dir: PathBuf,
    /// Pasta dos executáveis (`agentdeck`, `ask-agent`). Vai no início do PATH dos agentes.
    pub bin_dir: PathBuf,
    /// `<app_dir>/dados`
    pub data_dir: PathBuf,
    /// `<app_dir>/workspace`
    pub default_workspace: PathBuf,
    /// `<app_dir>/whisper`
    pub whisper_dir: PathBuf,
    /// `<config_dir>/AgentDeck/config.json`
    pub config_file: PathBuf,
}

impl AppPaths {
    /// Resolve a partir do executável atual. `AGENTDECK_HOME` sobrescreve `app_dir` e isola o app
    /// por completo: a configuração também fica dentro dessa pasta (`cfg/config.json`), então testes
    /// e instâncias paralelas não tocam na configuração do usuário.
    /// Em desenvolvimento (`target/debug|release`), sobe até achar a raiz do repositório
    /// (pasta com `workspace/` e `dados/`), para reaproveitar as pastas da versão Electron.
    pub fn detect() -> Self {
        let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_else(|| PathBuf::from("."));
        match crate::env::env_var_os("HOME").map(PathBuf::from) {
            Some(home) => Self::isolated(home, exe_dir),
            None => {
                let found = find_app_dir(&exe_dir);
                Self::with_dirs(fallback_data_dir(&found).unwrap_or(found), exe_dir)
            }
        }
    }

    /// Como [`AppPaths::with_dirs`], mas com a configuração dentro de `app_dir`.
    pub fn isolated(app_dir: PathBuf, bin_dir: PathBuf) -> Self {
        let config_file = app_dir.join("cfg").join("config.json");
        Self { config_file, ..Self::with_dirs(app_dir, bin_dir) }
    }

    pub fn with_dirs(app_dir: PathBuf, bin_dir: PathBuf) -> Self {
        let config_file = dirs::config_dir().unwrap_or_else(|| app_dir.clone()).join("AgentDeck").join("config.json");
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

/// Linux/macOS: um app instalado em pasta do sistema (ex.: `/usr/local/bin`) não consegue criar
/// `dados/` ao lado do executável. Nesse caso os dados vão para a pasta do usuário
/// (`~/.local/share/AgentDeck`, o padrão XDG). Numa pasta portátil (ou no repositório em
/// desenvolvimento) continua tudo ao lado do executável, como no Windows.
#[cfg(unix)]
fn fallback_data_dir(app_dir: &Path) -> Option<PathBuf> {
    // O que precisa ser gravável é `dados/`; só quando ela ainda não existe vale a pasta onde será criada
    // (um instalador pode ter criado `dados/` do usuário numa pasta do sistema, e vice-versa).
    let data = app_dir.join("dados");
    let probe = if data.is_dir() { data.as_path() } else { app_dir };
    if can_write(probe) {
        return None;
    }
    let dir = dirs::data_dir()?.join("AgentDeck");
    log::warn!("{} não é gravável: os dados do app vão para {}", probe.display(), dir.display());
    Some(dir)
}

#[cfg(not(unix))]
fn fallback_data_dir(_app_dir: &Path) -> Option<PathBuf> {
    None
}

/// `access(2)` em vez de tentar criar um arquivo: não deixa lixo na pasta do executável.
#[cfg(unix)]
fn can_write(dir: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else { return false };
    // SAFETY: `path` é uma string C válida (sem NUL) e vive até o fim da chamada.
    unsafe { libc::access(path.as_ptr(), libc::W_OK) == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolated_home_keeps_the_config_inside_it() {
        let home = PathBuf::from("/tmp/agentdeck-home");
        let p = AppPaths::isolated(home.clone(), PathBuf::from("/tmp/bin"));
        assert_eq!(p.config_file, home.join("cfg").join("config.json"));
        assert_eq!(p.data_dir, home.join("dados"));
        // Sem o isolamento a configuração fica na pasta do usuário, fora do app.
        let normal = AppPaths::with_dirs(home.clone(), PathBuf::from("/tmp/bin"));
        assert!(!normal.config_file.starts_with(&home));
    }

    #[test]
    fn a_writable_folder_keeps_the_data_next_to_the_executable() {
        let tmp = tempfile::tempdir().expect("tmp");
        assert!(fallback_data_dir(tmp.path()).is_none(), "pasta gravável não tem desvio");
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_the_user_cannot_write_falls_back_to_the_xdg_data_dir() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().expect("tmp");
        let bin = tmp.path().join("bin");
        std::fs::create_dir(&bin).expect("mkdir");
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o500)).expect("chmod");
        // Rodando como root, `access` ignora as permissões e o caso não dá para simular.
        if !can_write(&bin) {
            assert_eq!(fallback_data_dir(&bin), dirs::data_dir().map(|d| d.join("AgentDeck")));
        }
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).expect("chmod de volta");
    }

    #[cfg(unix)]
    #[test]
    fn what_counts_is_the_data_folder_when_it_exists() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().expect("tmp");
        let app = tmp.path().join("app");
        std::fs::create_dir_all(app.join("dados")).expect("mkdir");
        // `dados/` gravável numa pasta que não é: os dados ficam onde estão.
        std::fs::set_permissions(&app, std::fs::Permissions::from_mode(0o500)).expect("chmod");
        if !can_write(&app) {
            assert!(fallback_data_dir(&app).is_none(), "dados/ gravável não tem desvio");
        }
        // O contrário: pasta gravável e `dados/` não (criada por um sudo antes): desvia.
        std::fs::set_permissions(&app, std::fs::Permissions::from_mode(0o700)).expect("chmod");
        std::fs::set_permissions(app.join("dados"), std::fs::Permissions::from_mode(0o500)).expect("chmod");
        if !can_write(&app.join("dados")) {
            assert_eq!(fallback_data_dir(&app), dirs::data_dir().map(|d| d.join("AgentDeck")));
        }
        std::fs::set_permissions(app.join("dados"), std::fs::Permissions::from_mode(0o700)).expect("chmod de volta");
    }
}
