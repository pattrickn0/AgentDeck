//! Location of the app's folders.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AppPaths {
    /// Base folder of the app (where `data/`, `workspace/` and `whisper/` live).
    pub app_dir: PathBuf,
    /// Folder of the executables (`agentdeck`, `ask-agent`). It goes at the start of the agents' PATH.
    pub bin_dir: PathBuf,
    /// `<app_dir>/data` (or the legacy `<app_dir>/dados` while an older version still holds it).
    pub data_dir: PathBuf,
    /// `<app_dir>/workspace`
    pub default_workspace: PathBuf,
    /// `<app_dir>/whisper`
    pub whisper_dir: PathBuf,
    /// `<config_dir>/AgentDeck/config.json`
    pub config_file: PathBuf,
}

impl AppPaths {
    /// Resolves from the current executable. `AGENTDECK_HOME` overrides `app_dir` and isolates the app
    /// completely: the config also lives inside that folder (`cfg/config.json`), so tests and
    /// parallel instances never touch the user's config.
    /// In development (`target/debug|release`), walks up to the repository root (the folder with
    /// `workspace/` and `data/`), to reuse the folders of the Electron version.
    /// The folders with the Portuguese names of older versions are moved to the English ones here (see
    /// [`AppPaths::adopt_legacy_names`]).
    pub fn detect() -> Self {
        let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_else(|| PathBuf::from("."));
        let mut paths = match crate::env::env_var_os("HOME").map(PathBuf::from) {
            Some(home) => Self::isolated(home, exe_dir),
            None => {
                let found = find_app_dir(&exe_dir);
                Self::with_dirs(fallback_data_dir(&found).unwrap_or(found), exe_dir)
            }
        };
        paths.adopt_legacy_names();
        paths
    }

    /// Moves what older versions saved under Portuguese names to the English ones: `dados/` to `data/`, and inside
    /// it `anexos/` to `attachments/`, `tarefas/` to `tasks/` and `capacidade.json` to `capability.json`. An entry
    /// that already exists in the new place is kept there (an identical file in the old place is dropped). While an
    /// older version still runs on `dados/` (its lock is held), this run keeps using `dados/` and moves nothing.
    pub fn adopt_legacy_names(&mut self) {
        let legacy = self.app_dir.join(LEGACY_DATA_DIR);
        if legacy.is_dir() {
            if legacy_lock_held(&legacy) {
                log::warn!("an older AgentDeck is using {}: this run keeps that folder", legacy.display());
                self.data_dir = legacy;
                return;
            }
            merge_into(&legacy, &self.data_dir);
        }
        for (old, new) in LEGACY_DATA_ENTRIES {
            let old = self.data_dir.join(old);
            if old.exists() {
                merge_into(&old, &self.data_dir.join(new));
            }
        }
    }

    /// Like [`AppPaths::with_dirs`], but with the config inside `app_dir`.
    pub fn isolated(app_dir: PathBuf, bin_dir: PathBuf) -> Self {
        let config_file = app_dir.join("cfg").join("config.json");
        Self { config_file, ..Self::with_dirs(app_dir, bin_dir) }
    }

    pub fn with_dirs(app_dir: PathBuf, bin_dir: PathBuf) -> Self {
        let config_file = dirs::config_dir().unwrap_or_else(|| app_dir.clone()).join("AgentDeck").join("config.json");
        Self {
            data_dir: app_dir.join(DATA_DIR),
            default_workspace: app_dir.join("workspace"),
            whisper_dir: app_dir.join("whisper"),
            config_file,
            bin_dir,
            app_dir,
        }
    }
}

/// The data folder inside the app folder.
pub const DATA_DIR: &str = "data";

/// The data folder's name before the English rename.
pub const LEGACY_DATA_DIR: &str = "dados";

/// Entries of the data folder renamed to English: (old, new).
const LEGACY_DATA_ENTRIES: [(&str, &str); 3] = [("anexos", "attachments"), ("tarefas", "tasks"), ("capacidade.json", "capability.json")];

/// Does a running older version hold the lock of the legacy data folder?
fn legacy_lock_held(legacy: &Path) -> bool {
    let Ok(file) = std::fs::OpenOptions::new().write(true).open(legacy.join(".agentdeck.lock")) else { return false };
    matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock))
}

/// Moves `from` to `to`. Folders are merged entry by entry; an entry that exists on both sides stays where it is in
/// `to`, and its copy in `from` goes away only when both are identical files. `from` is removed when it ends empty.
fn merge_into(from: &Path, to: &Path) {
    if !to.exists() {
        if let Some(parent) = to.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::rename(from, to) {
            Ok(()) => log::info!("moved {} to {}", from.display(), to.display()),
            Err(e) => log::warn!("could not move {} to {}: {e}", from.display(), to.display()),
        }
        return;
    }
    if from.is_dir() && to.is_dir() {
        if let Ok(entries) = std::fs::read_dir(from) {
            for entry in entries.flatten() {
                merge_into(&entry.path(), &to.join(entry.file_name()));
            }
        }
        let _ = std::fs::remove_dir(from);
    } else if from.is_file() && to.is_file() && std::fs::read(from).ok() == std::fs::read(to).ok() {
        let _ = std::fs::remove_file(from);
    } else {
        log::warn!("{} was kept: {} already exists", from.display(), to.display());
    }
}

fn find_app_dir(exe_dir: &Path) -> PathBuf {
    for dir in exe_dir.ancestors() {
        if dir.join("workspace").is_dir() && (dir.join(DATA_DIR).is_dir() || dir.join(LEGACY_DATA_DIR).is_dir()) {
            return dir.to_path_buf();
        }
    }
    exe_dir.to_path_buf()
}

/// Linux/macOS: an app installed in a system folder (e.g. `/usr/local/bin`) cannot create
/// `data/` next to the executable. In that case the data goes to the user's folder
/// (`~/.local/share/AgentDeck`, the XDG default). In a portable folder (or in the repository
/// during development) everything stays next to the executable, as on Windows.
#[cfg(unix)]
fn fallback_data_dir(app_dir: &Path) -> Option<PathBuf> {
    // What has to be writable is `data/`; only when it does not exist yet does the folder where it will be created
    // count (an installer may have created the user's `data/` in a system folder, and vice versa).
    let data = app_dir.join(DATA_DIR);
    let probe = if data.is_dir() { data.as_path() } else { app_dir };
    if can_write(probe) {
        return None;
    }
    let dir = dirs::data_dir()?.join("AgentDeck");
    log::warn!("{} is not writable: the app data goes to {}", probe.display(), dir.display());
    Some(dir)
}

#[cfg(not(unix))]
fn fallback_data_dir(_app_dir: &Path) -> Option<PathBuf> {
    None
}

/// `access(2)` instead of trying to create a file: it leaves no litter in the executable's folder.
#[cfg(unix)]
fn can_write(dir: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else { return false };
    // SAFETY: `path` is a valid C string (no NUL) and lives until the end of the call.
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
        assert_eq!(p.data_dir, home.join("data"));
        // Without isolation the config stays in the user's folder, outside the app.
        let normal = AppPaths::with_dirs(home.clone(), PathBuf::from("/tmp/bin"));
        assert!(!normal.config_file.starts_with(&home));
    }

    #[test]
    fn the_portuguese_folders_of_older_versions_move_to_the_english_names() {
        let tmp = tempfile::tempdir().expect("tmp");
        let app = tmp.path();
        let legacy = app.join("dados");
        std::fs::create_dir_all(legacy.join("tarefas")).expect("mkdir");
        std::fs::create_dir_all(legacy.join("anexos")).expect("mkdir");
        std::fs::write(legacy.join("tarefas").join("p1.json"), "{}").expect("write");
        std::fs::write(legacy.join("anexos").join("a.png"), "x").expect("write");
        std::fs::write(legacy.join("capacidade.json"), "{}").expect("write");
        std::fs::write(legacy.join("README.md"), "same").expect("write");
        std::fs::write(legacy.join("notes.md"), "old").expect("write");
        // The repository already brings `data/` with the README; a file with other content is not overwritten.
        std::fs::create_dir_all(app.join("data")).expect("mkdir");
        std::fs::write(app.join("data").join("README.md"), "same").expect("write");
        std::fs::write(app.join("data").join("notes.md"), "new").expect("write");
        let mut p = AppPaths::isolated(app.to_path_buf(), app.join("bin"));
        p.adopt_legacy_names();
        let data = app.join("data");
        assert_eq!(p.data_dir, data);
        assert!(data.join("tasks").join("p1.json").is_file() && data.join("attachments").join("a.png").is_file());
        assert!(data.join("capability.json").is_file() && !data.join("tarefas").exists() && !data.join("anexos").exists());
        assert_eq!(std::fs::read_to_string(data.join("notes.md")).expect("read"), "new");
        assert!(legacy.join("notes.md").is_file() && !legacy.join("README.md").exists(), "only the different file stays behind");
    }

    #[test]
    fn a_running_older_version_keeps_its_legacy_folder() {
        let tmp = tempfile::tempdir().expect("tmp");
        let legacy = tmp.path().join("dados");
        std::fs::create_dir_all(&legacy).expect("mkdir");
        let held = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(legacy.join(".agentdeck.lock")).expect("lock");
        held.try_lock().expect("lock");
        let mut p = AppPaths::isolated(tmp.path().to_path_buf(), tmp.path().join("bin"));
        p.adopt_legacy_names();
        assert_eq!(p.data_dir, legacy);
        assert!(!tmp.path().join("data").exists());
    }

    #[test]
    fn a_writable_folder_keeps_the_data_next_to_the_executable() {
        let tmp = tempfile::tempdir().expect("tmp");
        assert!(fallback_data_dir(tmp.path()).is_none(), "a writable folder has no fallback");
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_the_user_cannot_write_falls_back_to_the_xdg_data_dir() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().expect("tmp");
        let bin = tmp.path().join("bin");
        std::fs::create_dir(&bin).expect("mkdir");
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o500)).expect("chmod");
        // Running as root, `access` ignores the permissions and the case cannot be simulated.
        if !can_write(&bin) {
            assert_eq!(fallback_data_dir(&bin), dirs::data_dir().map(|d| d.join("AgentDeck")));
        }
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).expect("chmod back");
    }

    #[cfg(unix)]
    #[test]
    fn what_counts_is_the_data_folder_when_it_exists() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().expect("tmp");
        let app = tmp.path().join("app");
        std::fs::create_dir_all(app.join("data")).expect("mkdir");
        // `data/` writable inside a folder that is not: the data stays where it is.
        std::fs::set_permissions(&app, std::fs::Permissions::from_mode(0o500)).expect("chmod");
        if !can_write(&app) {
            assert!(fallback_data_dir(&app).is_none(), "a writable data/ has no fallback");
        }
        // The opposite: a writable folder and a `data/` that is not (created by a sudo earlier): falls back.
        std::fs::set_permissions(&app, std::fs::Permissions::from_mode(0o700)).expect("chmod");
        std::fs::set_permissions(app.join("data"), std::fs::Permissions::from_mode(0o500)).expect("chmod");
        if !can_write(&app.join("data")) {
            assert_eq!(fallback_data_dir(&app), dirs::data_dir().map(|d| d.join("AgentDeck")));
        }
        std::fs::set_permissions(app.join("data"), std::fs::Permissions::from_mode(0o700)).expect("chmod back");
    }
}
