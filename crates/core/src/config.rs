//! Persisted configuration (equivalent to the Electron `config.json`, with extra layout fields).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths::AppPaths;

/// A project: a folder with its own tabs (agent instances) and its own layout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProjectCfg {
    /// Short, stable id (`p` + 8 hex), generated on creation. It is the scope of the instances on the bridge.
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    /// Id of each open tab, in the order of the bar (restored when the project is opened).
    pub open_tabs: Vec<String>,
    /// Index in `open_tabs` of the active tab (-1 = none).
    pub active_tab: i32,
    /// Order of the split-screen windows (groups: `claude`, `antigravity`, `codex`…).
    pub pane_order: Vec<String>,
    /// Proportions of the grid rows and of the columns of each row (empty = equal). They only apply if
    /// the grid shape is still the same.
    pub split_rows: Vec<f32>,
    pub split_cols: Vec<Vec<f32>>,
    /// Who opened each tab (tab id → id of whoever called it through `ask-agent`): the hierarchy of the
    /// agent list in the sidebar. Tabs opened by the user do not go in.
    pub tab_parents: BTreeMap<String, String>,
    /// LLM model of each instance (tab id → model, e.g. `claude@dev` → `sonnet`). The id does not
    /// change with the model; only the instances that chose (or inherited) a model go in here.
    pub tab_models: BTreeMap<String, String>,
    /// CLI conversation of each tab (tab id → the CLI's own session id), so reopening the app brings the conversation back
    /// (see `resume.rs`). Saved as soon as the id is known; forgotten when the tab is closed for good.
    pub tab_sessions: BTreeMap<String, String>,
    /// The project's agent list is collapsed in the sidebar.
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
            tab_sessions: BTreeMap::new(),
            collapsed: false,
        }
    }
}

impl ProjectCfg {
    /// New project in folder `path`, with an id that is not in `taken`. Without `name`, the name is the
    /// folder's.
    pub fn new(path: PathBuf, name: Option<&str>, taken: &[ProjectCfg]) -> Self {
        Self::with_id(path, name, None, taken)
    }

    /// Like [`ProjectCfg::new`], but with the `id` of a removed project (the same folder added again
    /// reuses the id). If the id already belongs to a project of `taken` or is empty, another one is generated.
    fn with_id(path: PathBuf, name: Option<&str>, id: Option<String>, taken: &[ProjectCfg]) -> Self {
        let name = name.map(str::trim).filter(|n| !n.is_empty()).map(str::to_owned).unwrap_or_else(|| folder_name(&path));
        let id = id.filter(|id| !id.is_empty() && !taken.iter().any(|p| p.id == *id)).unwrap_or_else(|| loop {
            let id = new_project_id();
            if !taken.iter().any(|p| p.id == id) {
                break id;
            }
        });
        Self { id, name, path, ..Self::default() }
    }
}

/// A project that left the list: the id and the folder, so that adding the same folder again reuses the id
/// (and the tasks in `data/tasks/<id>.json` come back to the desk).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RemovedProject {
    pub id: String,
    pub path: PathBuf,
}

impl Default for RemovedProject {
    fn default() -> Self {
        Self { id: String::new(), path: PathBuf::new() }
    }
}

/// How many removed projects the config remembers (`removedProjects`).
const REMOVED_PROJECTS_MAX: usize = 50;

/// `true` if the two folders are the same thing for the person: `\` and `/` are the same, no trailing
/// slash and, only on Windows, case-insensitive.
fn same_folder(a: &Path, b: &Path) -> bool {
    fn norm(p: &Path) -> String {
        let mut s = crate::util::strip_verbatim(p).to_string_lossy().replace('\\', "/");
        while s.len() > 1 && s.ends_with('/') {
            s.pop();
        }
        if cfg!(windows) {
            s.to_lowercase()
        } else {
            s
        }
    }
    norm(a) == norm(b)
}

/// `p` + 8 random hex digits.
fn new_project_id() -> String {
    let mut buf = [0u8; 4];
    if getrandom::fill(&mut buf).is_err() {
        buf.copy_from_slice(&uuid::Uuid::new_v4().as_bytes()[..4]);
    }
    format!("p{}", buf.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

/// The folder name (or the whole path, if there is no name: the root of a drive).
fn folder_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

/// How the agent windows are arranged in the center.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Organization {
    /// A single window, with all the tabs.
    Tabs,
    /// One window per CLI, in a grid.
    #[default]
    Split,
    /// The window of the selected CLI large and the others stacked beside it.
    Highlight,
}

impl Organization {
    /// The arrangement for whoever only has `splitEnabled` saved.
    pub fn from_split_enabled(on: bool) -> Self {
        if on {
            Self::Split
        } else {
            Self::Tabs
        }
    }

    /// The equivalent value of `splitEnabled` (any arrangement in windows counts as on).
    pub fn split_enabled(self) -> bool {
        self != Self::Tabs
    }
}

/// The person's choice about voice dictation (local Whisper, a download of ~1.6 GB).
///
/// Saved in `config.json`'s `dictation` (`"unasked"`, `"yes"` or `"no"`): an installer can write
/// the field to answer the question before the app's first opening.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DictationChoice {
    /// Not asked yet: on the first opening the app asks (or, if Whisper is already downloaded, it counts as `Yes`).
    #[default]
    Unasked,
    /// The person wants dictation: the app downloads whatever is missing.
    Yes,
    /// The person does not want it: the app downloads nothing (they can download later, from the settings).
    No,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    /// The user's projects, in the sidebar order.
    pub project_list: Vec<ProjectCfg>,
    /// The last projects that left the list (see [`RemovedProject`]): adding the same folder again
    /// reuses the id. Absent in old files.
    #[serde(default)]
    pub removed_projects: Vec<RemovedProject>,
    /// Id of the active project (empty = no project).
    pub active_project: String,
    /// Folder of the active project (empty = none). Derived from `active_project`, not saved.
    /// In the old format it was the global workspace (read only for the migration).
    #[serde(skip_serializing)]
    pub workspace: PathBuf,
    /// Spoken language of the dictation: `auto` or a Whisper code (`en`, `pt`...). A fresh config follows the UI language.
    pub whisper_lang: String,
    /// Dictation: Whisper translates what it hears into English (`-tr`).
    pub whisper_translate: bool,
    /// Voice dictation: see [`DictationChoice`].
    pub dictation: DictationChoice,
    pub sound_enabled: bool,
    // --- Layout (new in the Rust version; the Electron version kept it only in CSS) ---
    pub sidebar_width: f32,
    pub sidebar_collapsed: bool,
    /// Width of the right bar (tasks, git, GitHub); 0 = default. It does not
    /// collapse. (Replaces `bottomHeight`/`feedCollapsed` of the old bottom panel, ignored when reading.)
    pub right_panel_width: f32,
    /// How the agent windows are arranged. Absent in old files: `splitEnabled` applies
    /// (see `Config::organization`).
    pub organization: Option<Organization>,
    /// Old format of the organization (on = Split): read so that whoever already uses it does not lose the
    /// choice, and still saved, so that an earlier version understands the file.
    pub split_enabled: bool,
    /// Highlight: fraction of the center width that the large window takes (0 = default, 2/3).
    pub highlight_ratio: f32,
    /// Window size in windowed mode (never the full-screen one: with the window maximized the previous one stays).
    pub window_width: f32,
    pub window_height: f32,
    /// The window was maximized when closing: it reopens maximized, but with the normal size saved above.
    pub window_maximized: bool,
    /// Opens the tab of a closed agent by itself when another agent calls it through `ask-agent`.
    pub auto_open_agent_tabs: bool,
    /// With the terminal focused, Ctrl+B belongs to the app (sidebar) instead of going to the terminal (the tmux
    /// prefix); off, it goes to the terminal. Ctrl+T always belongs to the app.
    pub app_shortcuts_first: bool,
    /// Language of the app's text (`"en"` or `"pt-BR"`); absent or unknown = English. Agents always get English.
    pub language: crate::i18n::Lang,
    /// Default model of new instances, by `model@role` (`claude@dev`) or only by `model`
    /// (`opencode`); the `model@role` wins. Without an entry, the CLI's own default applies.
    pub role_models: BTreeMap<String, String>,
    /// The model group the title bar shows, per agent (`antigravity` → `Gemini 4 Argon`). Without an
    /// entry (or with a group the source no longer returns), the summary applies.
    pub usage_pins: BTreeMap<String, String>,
    // --- Old format (before the projects): read only for the migration, never saved. Now
    // each project keeps its own tabs and its own layout (see `ProjectCfg`). ---
    /// Known folders (`projects`); the projects now live in `projectList`.
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
            removed_projects: Vec::new(),
            active_project: String::new(),
            workspace: PathBuf::new(),
            whisper_lang: crate::i18n::Lang::En.whisper_code().into(),
            whisper_translate: false,
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
            language: crate::i18n::Lang::En,
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
    /// The arrangement of the windows: the saved one or, in old files, the one `splitEnabled` gives.
    pub fn organization(&self) -> Organization {
        self.organization.unwrap_or_else(|| Organization::from_split_enabled(self.split_enabled))
    }

    /// Saves the arrangement in both fields (the old one stays coherent).
    pub fn set_organization(&mut self, org: Organization) {
        self.organization = Some(org);
        self.split_enabled = org.split_enabled();
    }

    /// Configuration of a fresh installation: no project.
    pub fn default_for(_paths: &AppPaths) -> Self {
        Self::default()
    }

    /// Configuration with a single active project in folder `path` (tests and embedded use).
    pub fn with_project(path: PathBuf) -> Self {
        let project = ProjectCfg::new(path.clone(), None, &[]);
        Self { active_project: project.id.clone(), workspace: path, project_list: vec![project], ..Self::default() }
    }

    /// Loads the configuration like Electron's `loadConfig`: uses `paths.config_file` or, if it does not
    /// exist, the legacy configuration (Electron/"orquestra"). The keys of the file
    /// override the defaults (like `Object.assign`). The old format (a global workspace) is
    /// migrated to projects (see [`Config::fix_workspace`]).
    pub fn load(paths: &AppPaths) -> Self {
        Self::load_from(&config_candidates(paths), paths, &crate::util::home_dir())
    }

    /// Like [`Config::load`], with the list of candidate files and the home folder explicit.
    pub(crate) fn load_from(candidates: &[PathBuf], paths: &AppPaths, home: &Path) -> Self {
        let file = candidates.iter().find(|p| p.is_file());
        let mut cfg = match file {
            Some(f) => {
                let text = std::fs::read(f).ok().and_then(|b| config_text(&b));
                text.and_then(|t| Self::try_from_json_over_defaults(&t, paths)).unwrap_or_else(|| {
                    // Unreadable: keeps a copy before the next save puts the defaults on top of it
                    // (otherwise the project list is lost without warning).
                    let backup = f.with_extension(format!("json.unreadable-{}", crate::util::now_ms()));
                    match std::fs::copy(f, &backup) {
                        Ok(_) => log::error!("unreadable config at {}; copy kept at {}", f.display(), backup.display()),
                        Err(e) => log::error!("unreadable config at {} (and the copy failed: {e})", f.display()),
                    }
                    Self::default_for(paths)
                })
            }
            None => Self::default_for(paths),
        };
        let from_legacy = file.filter(|f| **f != paths.config_file);
        // From the older configurations the tabs do not come: the first opening starts without agents.
        if from_legacy.is_some() {
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

    /// Applies the saved JSON on top of the defaults. Invalid JSON = defaults.
    pub fn from_json_over_defaults(text: &str, paths: &AppPaths) -> Self {
        Self::try_from_json_over_defaults(text, paths).unwrap_or_else(|| Self::default_for(paths))
    }

    /// Like [`Config::from_json_over_defaults`], but `None` if the JSON could not be read.
    pub fn try_from_json_over_defaults(text: &str, paths: &AppPaths) -> Option<Self> {
        let defaults = Self::default_for(paths);
        let (Ok(serde_json::Value::Object(loaded)), Ok(serde_json::Value::Object(mut base))) =
            (serde_json::from_str::<serde_json::Value>(text), serde_json::to_value(&defaults))
        else {
            return None;
        };
        // A file without a dictation language follows its UI language (a fresh config is English).
        if !loaded.contains_key("whisperLang") {
            let ui = loaded.get("language").and_then(|l| l.as_str()).map(crate::i18n::Lang::from_code).unwrap_or_default();
            base.insert("whisperLang".into(), ui.whisper_code().into());
        }
        for (k, v) in loaded {
            // `null` does not override a typed default (serde would reject the whole object).
            if !v.is_null() {
                base.insert(k, v);
            }
        }
        serde_json::from_value(serde_json::Value::Object(base)).ok()
    }

    /// Migrates the old format, normalizes the project folders (relative → relative to the app
    /// folder), validates the active project and leaves `workspace` = its folder (empty without a project).
    /// Idempotent.
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
        // Projects without an id or without a folder (hand-edited config) are dropped.
        self.project_list.retain(|p| !p.id.is_empty() && !p.path.as_os_str().is_empty());
        if self.project(&self.active_project).is_none() {
            self.active_project = self.project_list.first().map(|p| p.id.clone()).unwrap_or_default();
        }
        self.workspace = self.active().map(|p| p.path.clone()).unwrap_or_default();
    }

    /// Old format (`projects` + `workspace` + global tabs and layout) → one project per folder.
    /// The tabs and the layout go to the project of the `workspace` folder, which becomes active. Old workspace
    /// rules: empty → the app workspace; relative → relative to the app folder; the user's home
    /// folder → the app workspace.
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

    /// The project `id`.
    pub fn project(&self, id: &str) -> Option<&ProjectCfg> {
        self.project_list.iter().find(|p| p.id == id)
    }

    /// The active project.
    pub fn active(&self) -> Option<&ProjectCfg> {
        self.project(&self.active_project)
    }

    /// Creates a project in folder `path` and puts it at the end of the list. If the folder is that of a
    /// removed project (see [`Config::remove_project`]), reuses its id and takes the item out of
    /// `removed_projects` — as long as no project in the list already uses that id. Without `name`, the name is
    /// the folder's.
    pub fn add_project(&mut self, path: PathBuf, name: Option<&str>) -> ProjectCfg {
        let removed = match self.removed_projects.iter().position(|r| same_folder(&r.path, &path)) {
            Some(i) => Some(self.removed_projects.remove(i)),
            None => None,
        };
        let project = ProjectCfg::with_id(path, name, removed.map(|r| r.id), &self.project_list);
        self.project_list.push(project.clone());
        project
    }

    /// Takes project `id` out of the list and keeps the id + folder pair in `removed_projects`, so that the same
    /// folder added again reuses the id (and finds the old tasks). The list keeps the last
    /// [`REMOVED_PROJECTS_MAX`]. If it was the active one, activates the first that is left (or none) and
    /// adjusts `workspace`. `false` if the project does not exist.
    pub fn remove_project(&mut self, id: &str) -> bool {
        let Some(pos) = self.project_list.iter().position(|p| p.id == id) else {
            return false;
        };
        let project = self.project_list.remove(pos);
        // An old entry of the same id or the same folder no longer means anything.
        self.removed_projects.retain(|r| r.id != project.id && !same_folder(&r.path, &project.path));
        self.removed_projects.push(RemovedProject { id: project.id, path: project.path });
        let extra = self.removed_projects.len().saturating_sub(REMOVED_PROJECTS_MAX);
        self.removed_projects.drain(..extra);
        if self.active_project == id {
            self.active_project = self.project_list.first().map(|p| p.id.clone()).unwrap_or_default();
            self.workspace = self.active().map(|p| p.path.clone()).unwrap_or_default();
        }
        true
    }

    /// Persists as indented JSON (2 spaces, like `JSON.stringify(config, null, 2)`).
    pub fn save(&self, file: &Path) -> std::io::Result<()> {
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        // Writes beside it and swaps: a crash in the middle does not leave a half-written file.
        let tmp = file.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, file)
    }
}

/// Configuration of the old name "orquestra": the source (without tabs) when `Holt/config.json` does not exist yet.
pub(crate) fn legacy_config_files() -> Vec<PathBuf> {
    let Some(base) = dirs::config_dir() else { return Vec::new() };
    vec![base.join("orquestra").join("config.json")]
}

/// The text of `config.json`: UTF-8, with or without BOM, or UTF-16 with BOM (edited with PowerShell 5.1: `Set-Content
/// -Encoding utf8` writes the BOM and `Out-File` writes UTF-16). Without a BOM and not UTF-8, `None`: the file counts as
/// unreadable (the copy stays) instead of becoming text with replacements, which would spoil the project folders when saving.
fn config_text(bytes: &[u8]) -> Option<String> {
    let bom = [&[0xEF, 0xBB, 0xBF][..], &[0xFF, 0xFE], &[0xFE, 0xFF]].iter().any(|b| bytes.starts_with(b));
    if bom {
        Some(crate::util::decode_text(bytes))
    } else {
        String::from_utf8(bytes.to_vec()).ok()
    }
}

/// Files to try, in order: the one at `paths.config_file` and, after it, those of earlier versions. With
/// `HOLT_HOME` (the configuration lives inside the app folder) the app is isolated and does not read the
/// user's configurations.
pub(crate) fn config_candidates(paths: &AppPaths) -> Vec<PathBuf> {
    let mut candidates = vec![paths.config_file.clone()];
    if !paths.config_file.starts_with(&paths.app_dir) {
        candidates.extend(legacy_config_files());
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn paths(root: &Path) -> AppPaths {
        AppPaths {
            app_dir: root.to_path_buf(),
            bin_dir: root.join("bin"),
            data_dir: root.join("data"),
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
        // Without `organization` (old file): `splitEnabled` applies, or the default (Split).
        assert_eq!(read("{\"splitEnabled\": false}").organization(), Organization::Tabs);
        assert_eq!(read("{\"splitEnabled\": true}").organization(), Organization::Split);
        assert_eq!(read("{}").organization(), Organization::Split);
        // The saved organization wins, even with `splitEnabled` saying otherwise.
        assert_eq!(read("{\"organization\": \"highlight\", \"splitEnabled\": false}").organization(), Organization::Highlight);
        assert_eq!(read("{\"organization\": \"tabs\"}").organization(), Organization::Tabs);
        // Saving keeps the old field coherent and reads back the same.
        let mut cfg = Config::default();
        cfg.set_organization(Organization::Highlight);
        assert!(cfg.split_enabled);
        let text = serde_json::to_string(&cfg).expect("json");
        assert!(text.contains("\"organization\":\"highlight\""), "{text}");
        assert_eq!(read(&text).organization(), Organization::Highlight);
        cfg.set_organization(Organization::Tabs);
        assert!(!cfg.split_enabled);
    }

    /// The dictation choice: old file = "not asked"; an installer can write `"dictation": "yes"|"no"`.
    #[test]
    fn the_dictation_choice_defaults_to_unasked_and_is_writable_from_outside() {
        let read = |json: &str| serde_json::from_str::<Config>(json).expect("config");
        assert_eq!(read("{}").dictation, DictationChoice::Unasked);
        assert_eq!(read("{\"dictation\": \"yes\"}").dictation, DictationChoice::Yes);
        assert_eq!(read("{\"dictation\": \"no\"}").dictation, DictationChoice::No);
        let text = serde_json::to_string(&Config { dictation: DictationChoice::No, ..Config::default() }).expect("json");
        assert!(text.contains("\"dictation\":\"no\""), "{text}");
    }

    /// The language: absent (every config saved before it existed) or unknown = English; `"pt-BR"` = Portuguese.
    #[test]
    fn the_language_defaults_to_english_and_saves_its_code() {
        use crate::i18n::Lang;
        let tmp = tempfile::tempdir().expect("tmp");
        let p = paths(tmp.path());
        assert_eq!(Config::from_json_over_defaults("{}", &p).language, Lang::En);
        assert_eq!(Config::from_json_over_defaults("{\"language\": \"pt-BR\"}", &p).language, Lang::PtBr);
        let odd = Config::from_json_over_defaults("{\"language\": 7, \"whisperLang\": \"es\"}", &p);
        assert_eq!((odd.language, odd.whisper_lang.as_str()), (Lang::En, "es"), "a bad value keeps the rest of the file");
        let text = serde_json::to_string(&Config { language: Lang::PtBr, ..Config::default() }).expect("json");
        assert!(text.contains("\"language\":\"pt-BR\""), "{text}");
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
        assert!(text.contains("\n  \"whisperLang\": \"en\""), "indented camelCase JSON: {text}");
        assert!(text.contains("\"projectList\"") && text.contains("\"openTabs\"") && text.contains("\"splitRows\""), "{text}");
        // The fields of the old format are no longer saved.
        for old in ["\"workspace\"", "\"projects\"", "activeTabIndex"] {
            assert!(!text.contains(old), "{old} saved: {text}");
        }
        let back = Config::load_from(std::slice::from_ref(&p.config_file), &p, no_home());
        assert_eq!(back.whisper_lang, "en");
        assert_eq!(back.project_list, vec![project.clone()]);
        assert_eq!(back.active_project, project.id);
        assert_eq!(back.sidebar_width, 300.0);
        assert_eq!(back.workspace, tmp.path().join("proj"));
        assert!(!p.config_file.with_extension("json.tmp").exists(), "the temporary file becomes the file");

        // Unreadable file: back to the defaults, but keeps a copy before someone saves on top of it.
        std::fs::write(&p.config_file, "{\"projectList\": [ quebrado").expect("write");
        let broken = Config::load_from(std::slice::from_ref(&p.config_file), &p, no_home());
        assert!(broken.project_list.is_empty());
        let dir = p.config_file.parent().expect("dir");
        let kept: Vec<_> = std::fs::read_dir(dir)
            .expect("ls")
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .filter(|n| n.contains("unreadable"))
            .collect();
        assert_eq!(kept.len(), 1, "copy of the unreadable file: {kept:?}");
    }

    #[test]
    fn a_config_edited_in_powershell_keeps_its_projects() {
        // `Set-Content -Encoding utf8` writes the BOM; PowerShell 5.1's `Out-File` writes UTF-16 LE with a BOM.
        let tmp = tempfile::tempdir().expect("tmp");
        let p = paths(tmp.path());
        std::fs::create_dir_all(p.config_file.parent().expect("dir")).expect("mkdir");
        let json = format!(r#"{{"whisperLang":"en","projectList":[{{"id":"p1","path":{:?}}}]}}"#, tmp.path().join("zoë"));
        let utf16: Vec<u8> = [0xFF, 0xFE].into_iter().chain(json.encode_utf16().flat_map(u16::to_le_bytes)).collect();
        for bytes in [[&[0xEF, 0xBB, 0xBF][..], json.as_bytes()].concat(), utf16] {
            std::fs::write(&p.config_file, &bytes).expect("write");
            let cfg = Config::load_from(std::slice::from_ref(&p.config_file), &p, no_home());
            assert_eq!(cfg.whisper_lang, "en");
            assert_eq!(cfg.project_list.iter().map(|p| p.path.clone()).collect::<Vec<_>>(), [tmp.path().join("zoë")]);
        }
        // Without a BOM and not UTF-8 (cp1252): unreadable, with the copy, instead of "jo�o" saved afterwards.
        std::fs::write(&p.config_file, b"{\"projectList\":[{\"id\":\"p1\",\"path\":\"C:/jo\xE3o\"}]}").expect("write");
        assert!(Config::load_from(std::slice::from_ref(&p.config_file), &p, no_home()).project_list.is_empty());
        let kept = std::fs::read_dir(p.config_file.parent().expect("dir")).expect("ls").flatten();
        assert_eq!(kept.filter(|e| e.file_name().to_string_lossy().contains("unreadable")).count(), 1);
    }

    #[test]
    fn models_are_saved_per_instance_and_per_role() {
        let tmp = tempfile::tempdir().expect("tmp");
        let p = paths(tmp.path());
        let mut project = ProjectCfg::new(tmp.path().join("proj"), None, &[]);
        project.tab_models.insert("claude@dev".into(), "sonnet".into());
        project.tab_sessions.insert("claude@dev".into(), "429e5864-ad7e-46ed-9f76-b280602c8ff5".into());
        let mut cfg = Config { active_project: project.id.clone(), project_list: vec![project.clone()], ..Config::default() };
        cfg.role_models.insert("opencode".into(), "opencode/glm-5.3-flash-free".into());
        cfg.usage_pins.insert("antigravity".into(), "Gemini 4 Argon".into());
        cfg.save(&p.config_file).expect("save");
        let text = std::fs::read_to_string(&p.config_file).expect("read");
        assert!(text.contains("\"tabModels\"") && text.contains("\"roleModels\"") && text.contains("\"tabSessions\""), "{text}");
        let back = Config::load_from(std::slice::from_ref(&p.config_file), &p, no_home());
        assert_eq!(back.project_list, vec![project]);
        assert_eq!(back.role_models, cfg.role_models);
        assert_eq!(back.usage_pins, cfg.usage_pins);
        // An old config, without the fields, still applies.
        let old = Config::from_json_over_defaults(r#"{"projectList":[{"id":"p1","name":"x","path":"/x"}]}"#, &p);
        assert!(old.project_list[0].tab_models.is_empty() && old.project_list[0].tab_sessions.is_empty() && old.role_models.is_empty());
    }

    #[test]
    fn project_ids_are_short_and_distinct() {
        let a = ProjectCfg::new(PathBuf::from("/x/meu-app"), None, &[]);
        assert_eq!(a.id.len(), 9);
        assert!(a.id.starts_with('p') && a.id[1..].chars().all(|c| c.is_ascii_hexdigit()), "{}", a.id);
        assert_eq!(a.name, "meu-app");
        assert_eq!(a.active_tab, -1);
        let b = ProjectCfg::new(PathBuf::from("/x/other"), Some("  Other name "), std::slice::from_ref(&a));
        assert_ne!(a.id, b.id);
        assert_eq!(b.name, "Other name");
    }

    /// Removing the project and adding the same folder again gives back the same id (the tasks in
    /// `data/tasks/<id>.json` come back to the desk).
    #[test]
    fn reading_the_same_folder_after_removing_it_reuses_the_id() {
        let tmp = tempfile::tempdir().expect("tmp");
        let mut cfg = Config::default();
        let a = cfg.add_project(tmp.path().join("proj"), None);
        assert!(cfg.remove_project(&a.id));
        assert_eq!(cfg.removed_projects, [RemovedProject { id: a.id.clone(), path: a.path.clone() }]);
        assert!(cfg.project_list.is_empty());

        let again = cfg.add_project(tmp.path().join("proj"), None);
        assert_eq!(again.id, a.id, "the same folder is the same project");
        assert_eq!(again.name, "proj");
        assert!(cfg.removed_projects.is_empty(), "the item leaves the list when it is reused");
        assert_eq!(cfg.project_list, [again]);
        assert!(!cfg.remove_project("pnada"), "a project that does not exist does not go into the list");
        assert!(cfg.removed_projects.is_empty());
    }

    /// The folder counts as the person types it: another slash, a trailing slash and, on Windows, another case.
    #[test]
    fn the_folder_comparison_ignores_the_kind_of_slash_and_the_trailing_one() {
        let tmp = tempfile::tempdir().expect("tmp");
        let folder = if cfg!(windows) { tmp.path().join("Project") } else { tmp.path().join("project") };
        let same = if cfg!(windows) { tmp.path().join("PROJECT") } else { tmp.path().join("project") };
        let variants = [
            PathBuf::from(format!("{}/", same.display())),
            PathBuf::from(format!("{}\\", same.display())),
            PathBuf::from(format!("{}/", same.display().to_string().replace('\\', "/"))),
        ];
        for variant in variants {
            let mut cfg = Config::default();
            let a = cfg.add_project(folder.clone(), None);
            assert!(cfg.remove_project(&a.id));
            assert_eq!(cfg.add_project(variant.clone(), None).id, a.id, "{}", variant.display());
        }
    }

    #[test]
    fn a_different_or_unavailable_id_still_gets_a_new_one() {
        let tmp = tempfile::tempdir().expect("tmp");
        let mut cfg = Config::default();
        let a = cfg.add_project(tmp.path().join("a"), None);
        assert!(cfg.remove_project(&a.id));
        // Another folder: a new id, and the removed pair is still kept.
        let b = cfg.add_project(tmp.path().join("b"), None);
        assert_ne!(b.id, a.id);
        assert_eq!(cfg.removed_projects.len(), 1);
        // An active project already with that id (hand-edited config): a new id instead of two equal ones.
        cfg.project_list.push(ProjectCfg { id: a.id.clone(), path: tmp.path().join("other"), ..ProjectCfg::default() });
        let c = cfg.add_project(a.path.clone(), None);
        assert_ne!(c.id, a.id);
        assert_eq!(cfg.project_list.iter().filter(|p| p.id == a.id).count(), 1);
        assert!(cfg.removed_projects.is_empty(), "the folder was added again: the old item leaves");
    }

    /// The list keeps the last 50 pairs; the old config, without `removedProjects`, opens normally.
    #[test]
    fn the_removed_projects_are_bounded_and_optional() {
        let tmp = tempfile::tempdir().expect("tmp");
        let p = paths(tmp.path());
        let mut cfg = Config::default();
        for i in 0..55 {
            let a = cfg.add_project(tmp.path().join(format!("p{i}")), None);
            assert!(cfg.remove_project(&a.id));
        }
        assert_eq!(cfg.removed_projects.len(), 50);
        assert_eq!(cfg.removed_projects.first().expect("first").path, tmp.path().join("p5"));
        assert_eq!(cfg.removed_projects.last().expect("last").path, tmp.path().join("p54"));
        let first_id = cfg.removed_projects[0].id.clone();

        let old = Config::from_json_over_defaults(r#"{"projectList":[{"id":"p1","name":"x","path":"/x"}]}"#, &p);
        assert!(old.removed_projects.is_empty(), "a file without the field keeps opening");
        let saved = Config { removed_projects: cfg.removed_projects.clone(), ..Config::default() };
        let text = serde_json::to_string(&saved).expect("json");
        assert!(text.contains(&format!(r#""removedProjects":[{{"id":"{first_id}","path":"#)), "{text}");
        let back = Config::from_json_over_defaults(&text, &p);
        assert_eq!(back.removed_projects, cfg.removed_projects);
    }

    #[test]
    fn removing_the_active_project_activates_the_first_left() {
        let tmp = tempfile::tempdir().expect("tmp");
        let mut cfg = Config::default();
        let a = cfg.add_project(tmp.path().join("a"), None);
        let b = cfg.add_project(tmp.path().join("b"), None);
        cfg.active_project = b.id.clone();
        assert!(cfg.remove_project(&b.id));
        assert_eq!(cfg.active_project, a.id);
        assert_eq!(cfg.workspace, a.path);
        assert!(cfg.remove_project(&a.id));
        assert!(cfg.active_project.is_empty() && cfg.workspace.as_os_str().is_empty());
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
        std::fs::create_dir_all(legacy.parent().expect("parent")).expect("mkdir");
        let ws = tmp.path().join("ws-old");
        let json = serde_json::json!({"whisperLang": "es", "workspace": ws, "openTabs": ["antigravity", "claude"], "activeTabIndex": 1});
        std::fs::write(&legacy, json.to_string()).expect("write");
        let cfg = Config::load_from(&[p.config_file.clone(), legacy], &p, no_home());
        assert_eq!(cfg.whisper_lang, "es");
        let project = cfg.active().expect("the workspace becomes a project");
        assert_eq!(project.path, ws);
        assert!(project.open_tabs.is_empty(), "the tabs of the legacy config are not opened");
        assert_eq!(project.active_tab, -1);
    }

    /// With the new configuration already in place, the old one is neither read nor overwrites anything.
    #[test]
    fn the_new_config_wins_over_the_old_one() {
        let tmp = tempfile::tempdir().expect("tmp");
        let p = paths(tmp.path());
        let old = tmp.path().join("orquestra").join("config.json");
        std::fs::create_dir_all(old.parent().expect("parent")).expect("mkdir");
        std::fs::write(&old, r#"{"whisperLang": "es"}"#).expect("write");
        Config { whisper_lang: "en".into(), ..Config::default() }.save(&p.config_file).expect("writes the new one");
        let cfg = Config::load_from(&[p.config_file.clone(), old.clone()], &p, no_home());
        assert_eq!(cfg.whisper_lang, "en");
        assert_eq!(std::fs::read_to_string(&old).expect("old"), r#"{"whisperLang": "es"}"#);
    }

    #[test]
    fn candidates_start_with_the_new_file_then_orquestra() {
        let tmp = tempfile::tempdir().expect("tmp");
        let normal = AppPaths { config_file: tmp.path().join("cfg-do-usuario").join("config.json"), ..paths(&tmp.path().join("app")) };
        let c = config_candidates(&normal);
        assert_eq!(c[0], normal.config_file);
        if dirs::config_dir().is_some() {
            assert_eq!(c[1].parent().and_then(Path::file_name).and_then(|n| n.to_str()), Some("orquestra"));
            assert_eq!(legacy_config_files().len(), c.len() - 1);
        }
        // Isolated (`HOLT_HOME`): does not read the user's configurations.
        assert_eq!(config_candidates(&paths(tmp.path())), vec![paths(tmp.path()).config_file]);
    }

    #[test]
    fn dictation_language_follows_the_ui_language_unless_saved() {
        let tmp = tempfile::tempdir().expect("tmp");
        let p = paths(tmp.path());
        assert_eq!(Config::default().whisper_lang, "en");
        assert!(!Config::default().whisper_translate);
        let pt = Config::from_json_over_defaults(r#"{"language": "pt-BR"}"#, &p);
        assert_eq!(pt.whisper_lang, "pt");
        let kept = Config::from_json_over_defaults(r#"{"language": "pt-BR", "whisperLang": "es", "whisperTranslate": true}"#, &p);
        assert_eq!((kept.whisper_lang.as_str(), kept.whisper_translate), ("es", true));
    }

    #[test]
    fn partial_json_keeps_defaults() {
        let tmp = tempfile::tempdir().expect("tmp");
        let p = paths(tmp.path());
        let cfg = Config::from_json_over_defaults(r#"{"soundEnabled": false, "extra": 1, "workspace": null}"#, &p);
        assert!(!cfg.sound_enabled);
        assert_eq!(cfg.whisper_lang, "en");
        // Invalid JSON → defaults.
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
        assert_eq!(cfg.workspace, PathBuf::new(), "there is no default workspace any more");
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
        std::fs::create_dir_all(p.config_file.parent().expect("parent")).expect("mkdir");
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
        assert!(cfg.project_list[0].open_tabs.is_empty(), "the global tabs go only to the workspace project");
        assert_eq!(cfg.workspace, b);
        assert!(!cfg.sound_enabled);

        // Saved and read again: the same, and without migrating again.
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
        // Workspace in the home folder → the app workspace (old rule), which becomes a project.
        let mut cfg = Config { workspace: home.join("."), projects: vec![home.join("proj")], ..Config::default() };
        cfg.fix_workspace(&p, &home);
        assert_eq!(cfg.project_list.iter().map(|p| p.path.clone()).collect::<Vec<_>>(), [home.join("proj"), p.default_workspace.clone()]);
        assert_eq!(cfg.workspace, p.default_workspace);
        // Idempotent.
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
