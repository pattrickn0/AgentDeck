//! Working folder: listing for the tree, AGENTS.md with the bridge instructions and watcher.

use std::path::{Component, Path, PathBuf};
use std::sync::Weak;
use std::time::{Duration, Instant};

use notify::{EventKind, RecursiveMode, Watcher};

/// Item of the file tree (equivalent to `workspace:readDir`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    /// Path relative to the workspace, with `/`.
    pub rel_path: String,
    /// Lowercase extension with the dot (`.rs`), empty for folders.
    pub ext: String,
}

/// Names ignored in the tree (`.gemini` is Antigravity's config folder, `~/.gemini/antigravity`;
/// `.holt` keeps the task worktrees, and `.agentdeck` the ones opened before the rename). Holt's own data folder is recognized by its lock file
/// ([`is_app_data_dir`]), not by name, so a project's own `data/` folder stays visible.
pub const IGNORED: &[&str] = &["node_modules", ".git", ".gemini", ".holt", ".agentdeck", "$RECYCLE.BIN", "target"];

/// Is `dir` the data folder of a Holt (it holds the instance lock file)? It is the app's own folder when the
/// project is the Holt repository (`data/`, or the legacy `dados/`). Migration (remove after 0.4,
/// `memory/MAINTENANCE.md` §6): the lock of an AgentDeck counts too.
pub fn is_app_data_dir(dir: &Path) -> bool {
    dir.join(".holt.lock").is_file() || dir.join(".agentdeck.lock").is_file()
}

fn is_ignored_entry(name: &str) -> bool {
    if cfg!(windows) {
        IGNORED.iter().any(|i| i.eq_ignore_ascii_case(name))
    } else {
        IGNORED.contains(&name)
    }
}

pub fn read_dir(workspace: &Path, rel_dir: &str) -> Vec<DirEntry> {
    // Only subfolders of the workspace (`""` = the root): no `..`, absolute path or `C:` on Windows.
    // The backslash is a separator on Windows (it is what `rel_path` used before): there it becomes `/`
    // before validating and joining. On Unix it is a valid name character (`a\b` is a folder), and the
    // `rel_path` that comes back from the tree brings the name intact: swapping it would open another folder.
    let rel_dir = if cfg!(windows) { rel_dir.replace('\\', "/") } else { rel_dir.to_owned() };
    if !rel_dir.is_empty() && !crate::git::safe_rel(&rel_dir) {
        return Vec::new();
    }
    let target = workspace.join(rel_dir.as_str());
    let Ok(rd) = std::fs::read_dir(&target) else { return Vec::new() };
    let rel_norm = rel_dir.trim_start_matches("./").trim_matches('/');
    let mut items: Vec<DirEntry> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if is_ignored_entry(&name) || is_app_data_dir(&e.path()) {
                return None;
            }
            let is_dir = e
                .file_type()
                .map(|t| if t.is_symlink() { std::fs::metadata(e.path()).is_ok_and(|m| m.is_dir()) } else { t.is_dir() })
                .unwrap_or(false);
            let rel_path = if rel_norm.is_empty() || rel_norm == "." { name.clone() } else { format!("{rel_norm}/{name}") };
            let ext = if is_dir {
                String::new()
            } else {
                Path::new(&name).extension().map(|x| format!(".{}", x.to_string_lossy().to_lowercase())).unwrap_or_default()
            };
            Some(DirEntry { name, is_dir, rel_path, ext })
        })
        .collect();
    items.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    items
}

/// Limit of what `read_text` reads: above it the file does not become text on screen.
pub const READ_TEXT_MAX: u64 = 1024 * 1024;

/// What `read_text` found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextFile {
    Text(String),
    /// Larger than [`READ_TEXT_MAX`] (size in bytes); the content is not read.
    TooLarge(u64),
    /// It has null bytes: it is not text.
    Binary,
}

/// Makes sure the accessed path is strictly inside the workspace (lexical normalization:
/// works with paths that do not exist; case-insensitive on Windows).
pub fn resolve_workspace_path(rel_or_abs: &str, workspace: &Path) -> Result<PathBuf, String> {
    if workspace.as_os_str().is_empty() {
        return Err(crate::t!("Workspace is not configured.").into());
    }
    let ws = crate::util::absolutize(workspace);
    let target = crate::util::normalize_lexical(&ws.join(rel_or_abs));
    if !crate::util::path_within(&target, &ws) {
        return Err(crate::t!(
            "Access denied: the path \"{path}\" is outside the configured workspace (\"{root}\").",
            path = rel_or_abs,
            root = workspace.display()
        ));
    }
    Ok(target)
}

/// Reads a text file of the workspace for the read-only view. The path must be
/// relative and safe (`git::safe_rel`) and, after resolving symbolic links, still inside the
/// workspace. Bytes that are not UTF-8 become `�` (old Latin-1 files can still be read).
pub fn read_text(workspace: &Path, rel: &str) -> Result<TextFile, String> {
    if workspace.as_os_str().is_empty() {
        return Err(crate::t!("no project open").into());
    }
    if !crate::git::safe_rel(rel) {
        return Err(crate::t!("invalid path: {path}", path = rel));
    }
    let root = std::fs::canonicalize(workspace).map_err(|e| crate::t!("the project folder does not open: {error}", error = e))?;
    let full =
        std::fs::canonicalize(workspace.join(rel)).map_err(|e| crate::t!("could not open {path}: {error}", path = rel, error = e))?;
    if !crate::util::path_within(&full, &root) {
        return Err(crate::t!("{path} is outside the project folder", path = rel));
    }
    let meta = std::fs::metadata(&full).map_err(|e| crate::t!("could not read {path}: {error}", path = rel, error = e))?;
    if !meta.is_file() {
        return Err(crate::t!("{path} is not a file", path = rel));
    }
    if meta.len() > READ_TEXT_MAX {
        return Ok(TextFile::TooLarge(meta.len()));
    }
    let bytes = std::fs::read(&full).map_err(|e| crate::t!("could not read {path}: {error}", path = rel, error = e))?;
    if bytes.contains(&0) {
        return Ok(TextFile::Binary);
    }
    Ok(TextFile::Text(String::from_utf8_lossy(&bytes).into_owned()))
}

/// The line ending of a text file (the editor works with `\n` and writes back with the same).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Eol {
    #[default]
    Lf,
    Crlf,
}

impl Eol {
    /// The text with `\n` and the line ending it used. `None` if it mixes both (or has a stray `\r`): swapping everything
    /// for a single one would change lines the user did not touch.
    pub fn normalize(text: &str) -> Option<(String, Eol)> {
        let crlf = text.matches("\r\n").count();
        let lf = text.matches('\n').count();
        if text.replace("\r\n", "").contains('\r') {
            return None;
        }
        match (crlf, lf) {
            (0, _) => Some((text.to_owned(), Eol::Lf)),
            (c, l) if c == l => Some((text.replace("\r\n", "\n"), Eol::Crlf)),
            _ => None,
        }
    }

    /// `text_lf` (only with `\n`) with the line ending `self`.
    pub fn apply(self, text_lf: &str) -> String {
        match self {
            Eol::Lf => text_lf.to_owned(),
            Eol::Crlf => text_lf.replace('\n', "\r\n"),
        }
    }
}

/// Writes `text_lf` (with `\n`) to `rel`, with the line ending `eol` and, if `bom`, with the UTF-8 BOM the file already
/// had (never adds one). Beside it and swapping ([`write_atomic`]): an agent reading the file never sees it half written.
/// The path must be relative and safe and stay inside the workspace; the file may no longer exist (deleted by
/// an agent while it was being edited), but its folder does.
pub fn write_text(workspace: &Path, rel: &str, text_lf: &str, eol: Eol, bom: bool) -> Result<(), String> {
    if workspace.as_os_str().is_empty() {
        return Err(crate::t!("no project open").into());
    }
    if !crate::git::safe_rel(rel) {
        return Err(crate::t!("invalid path: {path}", path = rel));
    }
    let root = std::fs::canonicalize(workspace).map_err(|e| crate::t!("the project folder does not open: {error}", error = e))?;
    let joined = workspace.join(rel);
    let target = match std::fs::canonicalize(&joined) {
        Ok(full) => full,
        Err(_) => {
            let name = joined.file_name().ok_or_else(|| crate::t!("invalid path: {path}", path = rel))?.to_owned();
            let parent = joined.parent().ok_or_else(|| crate::t!("invalid path: {path}", path = rel))?;
            std::fs::canonicalize(parent)
                .map_err(|e| crate::t!("the folder of {path} does not open: {error}", path = rel, error = e))?
                .join(name)
        }
    };
    if !crate::util::path_within(&target, &root) {
        return Err(crate::t!("{path} is outside the project folder", path = rel));
    }
    if target.is_dir() {
        return Err(crate::t!("{path} is not a file", path = rel));
    }
    let mut bytes = Vec::with_capacity(text_lf.len() + 3);
    if bom {
        bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    }
    bytes.extend_from_slice(eol.apply(text_lf).as_bytes());
    write_atomic(&target, &bytes).map_err(|e| crate::t!("could not write {path}: {error}", path = rel, error = e))
}

// ---------- AGENTS.md ----------

pub const MARKER_START: &str = "<!-- holt:start -->";
pub const MARKER_END: &str = "<!-- holt:end -->";

fn marker_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        // Also recognizes the markers of older names (`jwic`, `orquestra`, `agentdeck`) and swaps them for the current
        // one. Migration (remove `agentdeck` after 0.4, `memory/MAINTENANCE.md` §6).
        regex::Regex::new(r"<!-- (?:orquestra|jwic|agentdeck|holt):start -->[\s\S]*?<!-- (?:orquestra|jwic|agentdeck|holt):end -->")
            .expect("valid regex")
    })
}

/// Swaps the first marked block for `block` and deletes the others (with the blank lines that
/// separated them from the rest): an app from an older version that does not recognize the new marker adds its
/// own block at the end, and without this both would stay forever.
fn replace_marker_blocks(old: &str, block: &str) -> String {
    let mut out = String::with_capacity(old.len());
    let mut pos = 0;
    for (i, m) in marker_re().find_iter(old).enumerate() {
        out.push_str(&old[pos..m.start()]);
        if i == 0 {
            out.push_str(block);
        } else {
            for _ in 0..2 {
                if out.ends_with("\r\n") {
                    out.truncate(out.len() - 2);
                } else if out.ends_with('\n') {
                    out.pop();
                }
            }
        }
        pos = m.end();
    }
    out.push_str(&old[pos..]);
    out
}

/// Result of [`ensure_agent_context`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentContext {
    /// The workspace is the user's home folder: we never write there.
    SkippedHome,
    /// Outside the app folder, with no pre-existing AGENTS.md and no request to create it.
    SkippedForeign,
    /// Content was already up to date.
    Unchanged,
    Written,
    /// The file did not exist and was created, with the app block only (requested with `create`).
    Created,
}

/// Keeps the Holt marked block in the workspace's AGENTS.md (the Antigravity CLI has no
/// flag to append instructions to the system prompt). Equivalent to `ensureAgentContext`.
///
/// RULE: outside the app folder only updates an AGENTS.md that already exists; creating one in the user's folder
/// (`create`) is only for the CLI that has no other way of receiving the instructions (see `agents::reads_only_agents_md`).
/// In the user's home folder, never.
pub fn ensure_agent_context(
    workspace: &Path,
    app_dir: &Path,
    home: &Path,
    instructions: &str,
    create: bool,
) -> std::io::Result<AgentContext> {
    let ws = crate::util::absolutize(workspace);
    if !home.as_os_str().is_empty() && crate::util::same_path(&ws, &crate::util::absolutize(home)) {
        return Ok(AgentContext::SkippedHome);
    }
    let file = workspace.join("AGENTS.md");
    let is_app_workspace = crate::util::path_within(&ws, &crate::util::absolutize(app_dir));
    if !is_app_workspace && !file.exists() && !create {
        return Ok(AgentContext::SkippedForeign);
    }
    let existed = file.exists();
    let old = if existed { std::fs::read_to_string(&file)? } else { String::new() };
    // Keeps the file's line ending (CRLF in repositories with `core.autocrlf`): mixed EOL
    // would dirty `git status` at every open.
    let eol = if old.contains("\r\n") { "\r\n" } else { "\n" };
    let instructions = instructions.replace("\r\n", "\n").replace('\n', eol);
    let block = format!("{MARKER_START}{eol}{instructions}{eol}{MARKER_END}");
    let new = if marker_re().is_match(&old) {
        replace_marker_blocks(&old, &block)
    } else if old.is_empty() {
        format!("{block}{eol}")
    } else {
        format!("{}{eol}{eol}{block}{eol}", old.trim_end())
    };
    if new == old {
        return Ok(AgentContext::Unchanged);
    }
    write_atomic(&file, new.as_bytes())?;
    Ok(if existed { AgentContext::Written } else { AgentContext::Created })
}

/// `text` without the app's marked blocks and without line-ending differences: what is left belongs to the user.
fn without_marker_blocks(text: &str) -> String {
    marker_re().replace_all(&text.replace("\r\n", "\n"), "").trim().to_owned()
}

/// `true` if `current` differs from `committed` only in the app's marked blocks (outside them the text is the same).
pub fn only_marker_blocks_differ(committed: &str, current: &str) -> bool {
    without_marker_blocks(committed) == without_marker_blocks(current)
}

/// Puts in `file` the first marked block of `saved` (what the app had written): swaps the block the file
/// has now or, if it has none, appends it at the end. Keeps the file's line ending. Without a block in `saved`,
/// does nothing. Used by the task merge, which puts the file back to the HEAD version so git can merge.
pub fn restore_marker_block(file: &Path, saved: &str) -> std::io::Result<()> {
    let Some(found) = marker_re().find(saved) else { return Ok(()) };
    let old = std::fs::read_to_string(file).unwrap_or_default();
    let eol = if old.contains("\r\n") { "\r\n" } else { "\n" };
    let block = found.as_str().replace("\r\n", "\n").replace('\n', eol);
    let new = if marker_re().is_match(&old) {
        replace_marker_blocks(&old, &block)
    } else if old.is_empty() {
        format!("{block}{eol}")
    } else {
        format!("{}{eol}{eol}{block}{eol}", old.trim_end())
    };
    if new == old {
        return Ok(());
    }
    write_atomic(file, new.as_bytes())
}

/// Writes beside and swaps: an agent reading AGENTS.md at the same time never sees it half written.
/// If the swap fails (on Windows, a file open without swap permission), writes directly.
fn write_atomic(file: &Path, data: &[u8]) -> std::io::Result<()> {
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = file.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, data)?;
    if std::fs::rename(&tmp, file).is_err() {
        let _ = std::fs::remove_file(&tmp);
        std::fs::write(file, data)?;
    }
    Ok(())
}

// ---------- Watcher ----------

/// Changes in these folders do not reload the tree (`.gemini`: Antigravity's configuration).
const WATCH_IGNORED: &[&str] = &[".git", "node_modules", ".gemini", ".holt", ".agentdeck", "target"];

/// `path` relative to `base`, comparing by components (no case difference and no verbatim prefix
/// `\\?\` on Windows: `c:\x`, `C:\x` and `\\?\C:\x` are the same folder). `None` if it is outside.
fn rel_within(base: &Path, path: &Path) -> Option<PathBuf> {
    let base = crate::util::strip_verbatim(base);
    let path = crate::util::strip_verbatim(path);
    crate::util::path_within(&path, &base).then(|| path.components().skip(base.components().count()).collect())
}

/// `true` if the change in `path` must be ignored. Outside the workspace nothing is ignored: the folders
/// above it (a project in `C:\data\app`, for example) do not count.
pub(crate) fn watch_ignored(workspace: &Path, path: &Path) -> bool {
    let Some(rel) = rel_within(workspace, path) else { return false };
    // Holt's own data folder (the app writes there all the time) is only checked at the first level.
    if rel.components().next().is_some_and(|c| is_app_data_dir(&workspace.join(c.as_os_str()))) {
        return true;
    }
    rel.components().any(|c| match c {
        Component::Normal(s) => WATCH_IGNORED.iter().any(|i| crate::util::same_component(s, std::ffi::OsStr::new(i))),
        _ => false,
    })
}

/// Is the change in `path` in the workspace repository's git state (`.git/HEAD`, `index`, refs,
/// merge/rebase/cherry-pick/revert in progress…)? The `*.lock` files (git temporaries) do not count.
pub(crate) fn git_state_change(workspace: &Path, path: &Path) -> bool {
    git_state_change_in(&workspace.join(".git"), path)
}

/// Like [`git_state_change`], but comparing directly with the `git_dir` folder (which may be
/// outside `workspace/.git` in worktrees, submodules or subfolders of the repository).
pub(crate) fn git_state_change_in(git_dir: &Path, path: &Path) -> bool {
    let Some(rel) = rel_within(git_dir, path) else { return false };
    let name = rel.to_string_lossy();
    if name.ends_with(".lock") {
        return false;
    }
    let first = rel.components().next().map(|c| c.as_os_str().to_string_lossy().into_owned()).unwrap_or_default();
    matches!(
        first.as_str(),
        "HEAD"
            | "index"
            | "FETCH_HEAD"
            | "MERGE_HEAD"
            | "ORIG_HEAD"
            | "CHERRY_PICK_HEAD"
            | "REVERT_HEAD"
            | "rebase-merge"
            | "rebase-apply"
            | "packed-refs"
            | "refs"
    )
}

fn resolve_git_path(base: &Path, raw: &str) -> Option<PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let p = Path::new(raw);
    let abs = if p.is_absolute() {
        crate::util::normalize_lexical(p)
    } else {
        crate::util::normalize_lexical(&crate::util::absolutize(base).join(p))
    };
    abs.is_dir().then_some(abs)
}

/// Reads a `.git` file (`gitdir: <path>`) of a worktree/submodule when the `git` binary is not
/// available (e.g. isolated test environments).
fn git_dirs_from_dotgit_file(workspace: &Path) -> Vec<PathBuf> {
    let dotgit = workspace.join(".git");
    if !dotgit.is_file() {
        return Vec::new();
    }
    let Ok(text) = std::fs::read_to_string(&dotgit) else { return Vec::new() };
    let Some(raw) = text.lines().find_map(|l| l.strip_prefix("gitdir:")) else { return Vec::new() };
    let Some(gd) = resolve_git_path(workspace, raw) else { return Vec::new() };
    let mut out = vec![gd.clone()];
    if let Ok(common) = std::fs::read_to_string(gd.join("commondir")) {
        if let Some(cd) = resolve_git_path(&gd, &common) {
            if !crate::util::same_path(&cd, &gd) {
                out.push(cd);
            }
        }
    }
    out
}

/// Finds the workspace's real `.git` (`git rev-parse --absolute-git-dir --git-common-dir`, without a
/// shell). Called outside the UI thread.
pub(crate) fn discover_git_dirs(core: &crate::Core, workspace: &Path) -> Vec<PathBuf> {
    let mut git = core.inner.agents.which(if cfg!(windows) { "git.exe" } else { "git" });
    if git.is_none() && cfg!(windows) {
        git = core.inner.agents.system_dir("ProgramFiles").map(|p| p.join("Git").join("cmd").join("git.exe")).filter(|p| p.is_file());
    }
    let mut out = Vec::new();
    if let Some(git) = git {
        let mut cmd = std::process::Command::new(git);
        cmd.current_dir(workspace)
            .args(["-c", "core.quotepath=false", "rev-parse", "--absolute-git-dir", "--git-common-dir"])
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0");
        for k in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_PREFIX"] {
            cmd.env_remove(k);
        }
        if let Ok(res) = crate::process::run_captured(cmd, None, Duration::from_secs(5), None, None) {
            if res.exit == crate::process::Exit::Code(Some(0)) {
                for line in res.stdout.lines() {
                    if let Some(p) = resolve_git_path(workspace, line) {
                        if !out.iter().any(|e: &PathBuf| crate::util::same_path(e, &p)) {
                            out.push(p);
                        }
                    }
                }
            }
        }
    }
    if out.is_empty() {
        out = git_dirs_from_dotgit_file(workspace);
    }
    out
}

/// Silence that ends a burst of events.
const DEBOUNCE_QUIET: Duration = Duration::from_millis(300);
/// Ceiling of the wait: under continuous events (a build, a log) emits at least at this interval.
const DEBOUNCE_MAX: Duration = Duration::from_millis(1000);

/// Joins the events (`true` = git, `false` = tree) and calls `emit(tree, git)` after
/// `quiet` without events or, at most, `max` after the first. Ends when the channel closes
/// (emitting what was pending) or when `emit` returns `false`.
fn debounce(rx: &crossbeam_channel::Receiver<bool>, quiet: Duration, max: Duration, mut emit: impl FnMut(bool, bool) -> bool) {
    while let Ok(first) = rx.recv() {
        let (mut tree, mut git) = (!first, first);
        let deadline = Instant::now() + max;
        let mut open = true;
        while let Some(left) = deadline.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()) {
            match rx.recv_timeout(quiet.min(left)) {
                Ok(is_git) => {
                    git |= is_git;
                    tree |= !is_git;
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => break,
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    open = false;
                    break;
                }
            }
        }
        if !emit(tree, git) || !open {
            return;
        }
    }
}

/// Starts a recursive watcher with a 300 ms debounce that emits
/// [`crate::CoreEvent::WorkspaceChanged`] (and [`crate::CoreEvent::GitChanged`] when the state of the
/// repository in `.git/` changes). The watcher stops when the returned value is dropped.
pub(crate) fn start_watcher(workspace: &Path, core: Weak<crate::CoreInner>) -> notify::Result<notify::RecommendedWatcher> {
    // `true` = change in the git repository; `false` = in the file tree.
    let (tx, rx) = crossbeam_channel::unbounded::<bool>();
    let ws = workspace.to_path_buf();
    let extra_git: std::sync::Arc<parking_lot::RwLock<Vec<PathBuf>>> = std::sync::Arc::default();
    let extra_for_cb = extra_git.clone();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(ev) = res else { return };
        // Reads (Access) do not change the tree — avoids a loop with the listing itself.
        if matches!(ev.kind, EventKind::Access(_)) {
            return;
        }
        let extra = extra_for_cb.read();
        if ev.paths.iter().any(|p| git_state_change(&ws, p) || extra.iter().any(|gd| git_state_change_in(gd, p))) {
            let _ = tx.send(true);
        }
        let in_extra = |p: &Path| extra.iter().any(|gd| rel_within(gd, p).is_some());
        if !ev.paths.is_empty() && ev.paths.iter().all(|p| watch_ignored(&ws, p) || in_extra(p)) {
            return;
        }
        let _ = tx.send(false);
    })?;
    watcher.watch(workspace, RecursiveMode::Recursive)?;
    let ws_for_thread = workspace.to_path_buf();
    std::thread::Builder::new()
        .name("cd-watch-debounce".into())
        .spawn(move || {
            // Finds the real `.git` outside the UI thread (worktrees, submodules, subfolder of a repo).
            if let Some(inner) = core.upgrade() {
                let c = crate::Core { inner: inner.clone() };
                let default_git = ws_for_thread.join(".git");
                let dirs: Vec<PathBuf> =
                    discover_git_dirs(&c, &ws_for_thread).into_iter().filter(|gd| !crate::util::same_path(gd, &default_git)).collect();
                if !dirs.is_empty() {
                    *extra_git.write() = dirs.clone();
                    let mut guard = inner.watcher.lock();
                    if crate::util::same_path(&c.workspace(), &ws_for_thread) {
                        if let Some(w) = guard.as_mut() {
                            for gd in &dirs {
                                if !crate::util::path_within(gd, &ws_for_thread) {
                                    let _ = w.watch(gd, RecursiveMode::Recursive);
                                }
                            }
                        }
                    }
                }
            }
            // Ends when the watcher (owner of `tx`) is dropped.
            debounce(&rx, DEBOUNCE_QUIET, DEBOUNCE_MAX, |tree, git| {
                let Some(inner) = core.upgrade() else { return false };
                let core = crate::Core { inner };
                if tree {
                    core.emit(crate::CoreEvent::WorkspaceChanged);
                }
                if git {
                    core.emit(crate::CoreEvent::GitChanged);
                }
                true
            });
        })
        .map_err(|e| notify::Error::generic(&e.to_string()))?;
    Ok(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_resolution() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(&ws).expect("mkdir");
        assert_eq!(resolve_workspace_path("a/b.txt", &ws).expect("ok"), ws.join("a").join("b.txt"));
        assert_eq!(resolve_workspace_path("new/missing/../x.txt", &ws).expect("ok"), ws.join("new").join("x.txt"));
        assert_eq!(resolve_workspace_path(".", &ws).expect("ok"), ws);
        assert_eq!(resolve_workspace_path(&ws.join("in.txt").to_string_lossy(), &ws).expect("ok"), ws.join("in.txt"));
        for bad in ["../outside.txt", "a/../../outside", "../ws-sibling/x", "../../../../../../etc/passwd"] {
            let err = crate::i18n::english(|| resolve_workspace_path(bad, &ws)).expect_err(bad);
            assert!(err.starts_with("Access denied: the path"), "{err}");
        }
        let outside = tmp.path().join("outside.txt");
        assert!(resolve_workspace_path(&outside.to_string_lossy(), &ws).is_err());
        #[cfg(unix)]
        assert!(resolve_workspace_path("/etc/passwd", &ws).is_err());
        #[cfg(windows)]
        {
            let upper = ws.to_string_lossy().to_uppercase();
            assert!(resolve_workspace_path(&format!("{upper}\\x.txt"), &ws).is_ok());
            assert!(resolve_workspace_path("C:\\Windows\\win.ini", &ws).is_err());
        }
    }

    #[test]
    fn read_text_reads_text_and_refuses_big_binary_and_outside_files() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(ws.join("sub")).expect("mkdir");
        std::fs::write(ws.join("sub").join("a.rs"), "fn main() {}\n").expect("write");
        std::fs::write(ws.join("latin.txt"), b"caf\xe9").expect("write");
        std::fs::write(ws.join("bin.dat"), b"MZ\0\0abc").expect("write");
        std::fs::write(ws.join("big.txt"), vec![b'x'; READ_TEXT_MAX as usize + 1]).expect("write");
        std::fs::write(ws.join("limit.txt"), vec![b'x'; READ_TEXT_MAX as usize]).expect("write");
        std::fs::write(tmp.path().join("secret.txt"), "outside").expect("write");

        assert_eq!(read_text(&ws, "sub/a.rs"), Ok(TextFile::Text("fn main() {}\n".into())));
        assert_eq!(read_text(&ws, "latin.txt"), Ok(TextFile::Text("caf\u{fffd}".into())));
        assert_eq!(read_text(&ws, "bin.dat"), Ok(TextFile::Binary));
        assert_eq!(read_text(&ws, "big.txt"), Ok(TextFile::TooLarge(READ_TEXT_MAX + 1)));
        assert!(matches!(read_text(&ws, "limit.txt"), Ok(TextFile::Text(t)) if t.len() as u64 == READ_TEXT_MAX));
        // Outside the workspace, absolute, folder, nonexistent and no project.
        assert!(read_text(&ws, "../secret.txt").is_err());
        assert!(read_text(&ws, &tmp.path().join("secret.txt").to_string_lossy()).is_err());
        assert!(read_text(&ws, "sub").is_err());
        assert!(read_text(&ws, "does-not-exist.txt").is_err());
        assert!(read_text(&ws, "").is_err());
        assert!(read_text(Path::new(""), "sub/a.rs").is_err());
    }

    #[test]
    fn line_endings_are_detected_and_restored() {
        assert_eq!(Eol::normalize("a\nb\n"), Some(("a\nb\n".into(), Eol::Lf)));
        assert_eq!(Eol::normalize("a\r\nb\r\n"), Some(("a\nb\n".into(), Eol::Crlf)));
        assert_eq!(Eol::normalize("no line ending"), Some(("no line ending".into(), Eol::Lf)));
        assert_eq!(Eol::normalize(""), Some((String::new(), Eol::Lf)));
        // Mixed or with a stray \r: it cannot be given back the same.
        assert_eq!(Eol::normalize("a\r\nb\n"), None);
        assert_eq!(Eol::normalize("a\rb\n"), None);
        assert_eq!(Eol::Crlf.apply("a\nb\n"), "a\r\nb\r\n");
        assert_eq!(Eol::Lf.apply("a\nb\n"), "a\nb\n");
        let (lf, eol) = Eol::normalize("x\r\ny\r\n").expect("crlf");
        assert_eq!(eol.apply(&lf), "x\r\ny\r\n");
    }

    #[test]
    fn write_text_keeps_the_line_ending_and_never_adds_a_bom() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(ws.join("sub")).expect("mkdir");
        std::fs::write(tmp.path().join("outside.txt"), "outside").expect("write");
        write_text(&ws, "sub/a.txt", "one\ntwo\n", Eol::Crlf, false).expect("writes");
        assert_eq!(std::fs::read(ws.join("sub/a.txt")).expect("read"), b"one\r\ntwo\r\n");
        write_text(&ws, "sub/a.txt", "x\n", Eol::Lf, false).expect("rewrites");
        assert_eq!(std::fs::read(ws.join("sub/a.txt")).expect("read"), b"x\n");
        // The BOM only comes back if the file already had one.
        write_text(&ws, "sub/b.txt", "x", Eol::Lf, true).expect("writes");
        assert_eq!(std::fs::read(ws.join("sub/b.txt")).expect("read"), b"\xEF\xBB\xBFx");
        // No temporary file is left behind.
        let names: Vec<String> =
            std::fs::read_dir(ws.join("sub")).expect("ls").flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names.len(), 2, "{names:?}");
        // Outside the project, absolute, folder, nonexistent folder and no project.
        assert!(write_text(&ws, "../outside.txt", "x", Eol::Lf, false).is_err());
        assert!(write_text(&ws, &tmp.path().join("outside.txt").to_string_lossy(), "x", Eol::Lf, false).is_err());
        assert!(write_text(&ws, "sub", "x", Eol::Lf, false).is_err());
        assert!(write_text(&ws, "does/not/exist/a.txt", "x", Eol::Lf, false).is_err());
        assert!(write_text(Path::new(""), "a.txt", "x", Eol::Lf, false).is_err());
        assert_eq!(std::fs::read_to_string(tmp.path().join("outside.txt")).expect("read"), "outside");
    }

    #[test]
    fn read_dir_orders_and_filters() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ws = tmp.path();
        for d in ["zeta", "Alpha", "node_modules", ".git", "data", "target"] {
            std::fs::create_dir_all(ws.join(d)).expect("mkdir");
        }
        // Holt's own data folder (it has the lock file) is hidden; a plain `data/` of the project would show.
        std::fs::write(ws.join("data").join(".holt.lock"), "").expect("lock");
        assert!(is_app_data_dir(&ws.join("data")));
        if cfg!(windows) {
            std::fs::create_dir_all(ws.join("$Recycle.Bin")).expect("mkdir");
        }
        for f in ["b.TXT", "a.rs", "C.md"] {
            std::fs::write(ws.join(f), "x").expect("write");
        }
        std::fs::write(ws.join("zeta").join("in.json"), "{}").expect("write");
        let items = read_dir(ws, "");
        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["Alpha", "zeta", "a.rs", "b.TXT", "C.md"]);
        assert_eq!(items[3].ext, ".txt");
        assert!(items[0].is_dir && items[0].ext.is_empty());
        let sub = read_dir(ws, "zeta");
        assert_eq!(sub[0].rel_path, "zeta/in.json");
        // The backslash is a separator only on Windows; on Unix it is part of the name (see the test below).
        if cfg!(windows) {
            let sub_bs = read_dir(ws, "zeta\\");
            assert_eq!(sub_bs[0].rel_path, "zeta/in.json");
        }
        assert!(read_dir(ws, "does-not-exist").is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn read_dir_opens_a_unix_folder_with_a_backslash_in_the_name() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ws = tmp.path();
        std::fs::create_dir_all(ws.join("a\\b")).expect("mkdir");
        std::fs::write(ws.join("a\\b").join("in.txt"), "x").expect("write");
        let root = read_dir(ws, "");
        assert_eq!(root[0].rel_path, "a\\b");
        let sub = read_dir(ws, &root[0].rel_path);
        assert_eq!(sub.len(), 1);
        assert_eq!(sub[0].name, "in.txt");
    }

    #[test]
    fn read_dir_stays_inside_the_workspace() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(ws.join("sub")).expect("mkdir");
        std::fs::write(tmp.path().join("outside.txt"), "x").expect("write");
        std::fs::write(ws.join("sub").join("inside.txt"), "x").expect("write");
        for bad in ["..", "../", "../x", "a/../b", "sub/../sub", "/x", "C:..\\x", "sub/../.."] {
            assert!(read_dir(&ws, bad).is_empty(), "{bad:?}");
        }
        let abs = tmp.path().to_string_lossy().into_owned();
        assert!(read_dir(&ws, &abs).is_empty(), "absolute");
        assert_eq!(read_dir(&ws, "./sub")[0].name, "inside.txt");
    }

    #[test]
    fn agents_md_rules() {
        let tmp = tempfile::tempdir().expect("tmp");
        let app = tmp.path().join("app");
        let home = tmp.path().join("home");
        let ws_app = app.join("workspace");
        let foreign = tmp.path().join("outside");
        for d in [&ws_app, &home, &foreign] {
            std::fs::create_dir_all(d).expect("mkdir");
        }

        // Home folder: never.
        assert_eq!(ensure_agent_context(&home, &app, &home, "X", false).expect("ok"), AgentContext::SkippedHome);
        assert!(!home.join("AGENTS.md").exists());
        // Outside the app, without AGENTS.md: does not create it.
        assert_eq!(ensure_agent_context(&foreign, &app, &home, "X", false).expect("ok"), AgentContext::SkippedForeign);
        assert!(!foreign.join("AGENTS.md").exists());
        // Inside the app: creates it.
        assert_eq!(ensure_agent_context(&ws_app, &app, &home, "X", false).expect("ok"), AgentContext::Created);
        let text = std::fs::read_to_string(ws_app.join("AGENTS.md")).expect("read");
        assert_eq!(text, format!("{MARKER_START}\nX\n{MARKER_END}\n"));
        assert_eq!(ensure_agent_context(&ws_app, &app, &home, "X", false).expect("ok"), AgentContext::Unchanged);

        // Outside the app, an existing AGENTS.md with a legacy block: replaces only the block.
        std::fs::write(foreign.join("AGENTS.md"), "# Rules\n\n<!-- jwic:start -->\nold $1\n<!-- jwic:end -->\n\nend\n").expect("write");
        assert_eq!(ensure_agent_context(&foreign, &app, &home, "new $1", false).expect("ok"), AgentContext::Written);
        let text = std::fs::read_to_string(foreign.join("AGENTS.md")).expect("read");
        assert_eq!(text, format!("# Rules\n\n{MARKER_START}\nnew $1\n{MARKER_END}\n\nend\n"));

        // Existing AGENTS.md without a block: appends at the end.
        std::fs::write(foreign.join("AGENTS.md"), "# Rules\n\n\n").expect("write");
        ensure_agent_context(&foreign, &app, &home, "Y", false).expect("ok");
        let text = std::fs::read_to_string(foreign.join("AGENTS.md")).expect("read");
        assert_eq!(text, format!("# Rules\n\n{MARKER_START}\nY\n{MARKER_END}\n"));

        // A "sister" folder with the same string prefix does not count as inside the app.
        let sibling = tmp.path().join("app-x");
        std::fs::create_dir_all(&sibling).expect("mkdir");
        assert_eq!(ensure_agent_context(&sibling, &app, &home, "X", false).expect("ok"), AgentContext::SkippedForeign);
    }

    #[test]
    fn agents_md_is_created_only_when_asked_and_never_in_the_home() {
        let tmp = tempfile::tempdir().expect("tmp");
        let app = tmp.path().join("app");
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        for d in [&app, &home, &project] {
            std::fs::create_dir_all(d).expect("mkdir");
        }
        // Home folder: not even when asked.
        assert_eq!(ensure_agent_context(&home, &app, &home, "X", true).expect("ok"), AgentContext::SkippedHome);
        assert!(!home.join("AGENTS.md").exists());
        // New project, when asked: creates it with the block only; the second time nothing changes (nothing new in `git status`).
        assert_eq!(ensure_agent_context(&project, &app, &home, "X", true).expect("ok"), AgentContext::Created);
        let text = std::fs::read_to_string(project.join("AGENTS.md")).expect("read");
        assert_eq!(
            text,
            format!(
                "{MARKER_START}
X
{MARKER_END}
"
            )
        );
        assert_eq!(ensure_agent_context(&project, &app, &home, "X", true).expect("ok"), AgentContext::Unchanged);
        // A user AGENTS.md that already exists is never "created": it only gets the block.
        std::fs::write(
            project.join("AGENTS.md"),
            "# Mine
",
        )
        .expect("write");
        assert_eq!(ensure_agent_context(&project, &app, &home, "X", true).expect("ok"), AgentContext::Written);
        assert!(std::fs::read_to_string(project.join("AGENTS.md")).expect("read").starts_with(
            "# Mine
"
        ));
    }

    #[test]
    fn agents_md_keeps_crlf() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (app, home, ws) = (tmp.path().join("app"), tmp.path().join("home"), tmp.path().join("outside"));
        std::fs::create_dir_all(&ws).expect("mkdir");
        let file = ws.join("AGENTS.md");
        let only_crlf = |t: &str| t.matches('\n').count() == t.matches("\r\n").count();
        for start in [
            "# Rules\r\n\r\nend\r\n",
            "# Rules\r\n\r\n<!-- holt:start -->\r\nold\r\n<!-- holt:end -->\r\n",
            "# Rules\r\n\r\n<!-- jwic:start -->\r\nold\r\n<!-- jwic:end -->\r\n",
        ] {
            std::fs::write(&file, start).expect("write");
            assert_eq!(ensure_agent_context(&ws, &app, &home, "line 1\nline 2", false).expect("ok"), AgentContext::Written);
            let text = std::fs::read_to_string(&file).expect("read");
            assert!(only_crlf(&text), "{text:?}");
            assert!(text.contains(&format!("{MARKER_START}\r\nline 1\r\nline 2\r\n{MARKER_END}\r\n")), "{text:?}");
            // Second time: nothing changes.
            assert_eq!(ensure_agent_context(&ws, &app, &home, "line 1\nline 2", false).expect("ok"), AgentContext::Unchanged);
            assert_eq!(std::fs::read_to_string(&file).expect("read"), text);
        }
        // No temporary file is left behind.
        let names: Vec<String> =
            std::fs::read_dir(&ws).expect("ls").flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["AGENTS.md"]);
    }

    /// An AGENTS.md written by an old version (`jwic` markers) gets its block swapped for the
    /// new one, without duplicating and without touching the rest of the file.
    #[test]
    fn an_old_name_block_is_replaced_not_duplicated() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (app, home, ws) = (tmp.path().join("app"), tmp.path().join("home"), tmp.path().join("outside"));
        std::fs::create_dir_all(&ws).expect("mkdir");
        let file = ws.join("AGENTS.md");
        for eol in ["\n", "\r\n"] {
            let before = format!("# Rules{eol}{eol}Use tabs.{eol}{eol}");
            let after = format!("{eol}## End{eol}Thanks.{eol}");
            let old_block = format!("<!-- jwic:start -->{eol}old text{eol}<!-- jwic:end -->");
            std::fs::write(&file, format!("{before}{old_block}{eol}{after}")).expect("write");
            assert_eq!(ensure_agent_context(&ws, &app, &home, "new", false).expect("ok"), AgentContext::Written);
            let text = std::fs::read_to_string(&file).expect("read");
            let new_block = format!("{MARKER_START}{eol}new{eol}{MARKER_END}");
            assert_eq!(text, format!("{before}{new_block}{eol}{after}"), "only the block changes");
            assert_eq!(text.matches("<!-- holt:start -->").count(), 1);
            assert!(!text.contains("jwic"), "{text:?}");
            // With the new block, nothing else changes.
            assert_eq!(ensure_agent_context(&ws, &app, &home, "new", false).expect("ok"), AgentContext::Unchanged);
        }
    }

    /// Migration from AgentDeck: its block is replaced by the Holt one (not left beside it), the merge check ignores
    /// both, and a block saved before the merge is put back over an `agentdeck:` one.
    #[test]
    fn an_agentdeck_block_is_replaced_by_the_holt_one() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (app, home, ws) = (tmp.path().join("app"), tmp.path().join("home"), tmp.path().join("outside"));
        std::fs::create_dir_all(&ws).expect("mkdir");
        let file = ws.join("AGENTS.md");
        for eol in ["\n", "\r\n"] {
            let old_block = format!("<!-- agentdeck:start -->{eol}old text{eol}<!-- agentdeck:end -->");
            let new_block = format!("{MARKER_START}{eol}new{eol}{MARKER_END}");
            std::fs::write(&file, format!("# Rules{eol}{eol}{old_block}{eol}")).expect("write");
            assert_eq!(ensure_agent_context(&ws, &app, &home, "new", false).expect("ok"), AgentContext::Written);
            let text = std::fs::read_to_string(&file).expect("read");
            assert_eq!(text, format!("# Rules{eol}{eol}{new_block}{eol}"));
            assert!(!text.to_lowercase().contains("agentdeck"), "{text:?}");

            // The merge check: HEAD still has the old block, the file the new one (or vice versa): only blocks differ.
            let committed = format!("# Rules\n\n{}\n", old_block.replace("\r\n", "\n"));
            assert!(only_marker_blocks_differ(&committed, &text));
            assert!(!only_marker_blocks_differ(&committed, &text.replace("# Rules", "# My rules")));

            // The saved block (an old one) goes back over the file the merge left with the other name.
            std::fs::write(&file, format!("# Rules{eol}{eol}{old_block}{eol}")).expect("write");
            restore_marker_block(&file, &format!("{new_block}{eol}")).expect("restore");
            assert_eq!(std::fs::read_to_string(&file).expect("read"), format!("# Rules{eol}{eol}{new_block}{eol}"));
            restore_marker_block(&file, &format!("x{old_block}")).expect("restore");
            assert!(std::fs::read_to_string(&file).expect("read").contains("old text"), "a saved agentdeck block is recognized too");
        }
    }

    /// The old app does not recognize `holt:` and appends its block at the end: the new app swaps the
    /// first one and deletes the others, without leaving blank lines piling up and without touching the rest.
    #[test]
    fn extra_blocks_from_an_old_app_are_removed() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (app, home, ws) = (tmp.path().join("app"), tmp.path().join("home"), tmp.path().join("outside"));
        std::fs::create_dir_all(&ws).expect("mkdir");
        let file = ws.join("AGENTS.md");
        for eol in ["\n", "\r\n"] {
            let new_block = format!("{MARKER_START}{eol}new{eol}{MARKER_END}");
            let new_a = format!("<!-- holt:start -->{eol}old{eol}<!-- holt:end -->");
            let old_b = format!("<!-- jwic:start -->{eol}ancient{eol}<!-- jwic:end -->");
            // At the end (the old app's case), in the middle and glued to the first.
            let cases = [
                (format!("# Rules{eol}{eol}{new_a}{eol}{eol}{old_b}{eol}"), format!("# Rules{eol}{eol}{new_block}{eol}")),
                (
                    format!("# Rules{eol}{eol}{new_a}{eol}{eol}{old_b}{eol}{eol}End.{eol}"),
                    format!("# Rules{eol}{eol}{new_block}{eol}{eol}End.{eol}"),
                ),
                (format!("{new_a}{eol}{old_b}{eol}"), format!("{new_block}{eol}")),
            ];
            for (start, expected) in cases {
                std::fs::write(&file, &start).expect("write");
                assert_eq!(ensure_agent_context(&ws, &app, &home, "new", false).expect("ok"), AgentContext::Written);
                assert_eq!(std::fs::read_to_string(&file).expect("read"), expected, "{start:?}");
                assert_eq!(ensure_agent_context(&ws, &app, &home, "new", false).expect("ok"), AgentContext::Unchanged);
                assert_eq!(std::fs::read_to_string(&file).expect("read"), expected);
            }
        }
    }

    #[test]
    fn debounce_emits_under_a_continuous_stream_and_on_disconnect() {
        let (tx, rx) = crossbeam_channel::unbounded::<bool>();
        let feeder = std::thread::spawn(move || {
            // One event every 20 ms for ~700 ms: there is never 100 ms of silence.
            for i in 0..35 {
                let _ = tx.send(i % 5 == 0);
                std::thread::sleep(Duration::from_millis(20));
            }
            // Last event and the channel closes right after: it cannot be lost.
            let _ = tx.send(false);
        });
        let started = Instant::now();
        let mut emitted = Vec::new();
        debounce(&rx, Duration::from_millis(100), Duration::from_millis(250), |tree, git| {
            emitted.push((started.elapsed(), tree, git));
            true
        });
        feeder.join().expect("feeder");
        assert!(emitted.len() >= 2, "{emitted:?}");
        assert!(emitted[0].0 < Duration::from_millis(600), "the ceiling holds under a continuous stream: {emitted:?}");
        assert!(emitted[0].1 && emitted[0].2);
        // `emit` returning `false` stops it.
        let (tx, rx) = crossbeam_channel::unbounded::<bool>();
        tx.send(true).expect("send");
        let mut calls = 0;
        debounce(&rx, Duration::from_millis(10), Duration::from_millis(50), |_, _| {
            calls += 1;
            false
        });
        assert_eq!(calls, 1);
    }

    #[test]
    fn watcher_filter() {
        let ws = Path::new("/w");
        assert!(watch_ignored(ws, Path::new("/w/.git/index")));
        // …but the repository state becomes its own event (GitChanged), without git's `.lock` files.
        for p in [
            "/w/.git/HEAD",
            "/w/.git/index",
            "/w/.git/refs/heads/main",
            "/w/.git/FETCH_HEAD",
            "/w/.git/packed-refs",
            "/w/.git/CHERRY_PICK_HEAD",
            "/w/.git/REVERT_HEAD",
            "/w/.git/rebase-merge/done",
            "/w/.git/rebase-apply",
        ] {
            assert!(git_state_change(ws, Path::new(p)), "{p}");
        }
        // External `.git` (worktree, submodule or subfolder of a repository).
        let ext_git = Path::new("/repo/.git/worktrees/wt");
        assert!(git_state_change_in(ext_git, Path::new("/repo/.git/worktrees/wt/HEAD")));
        assert!(git_state_change_in(ext_git, Path::new("/repo/.git/worktrees/wt/CHERRY_PICK_HEAD")));
        assert!(!git_state_change_in(ext_git, Path::new("/repo/.git/worktrees/wt/index.lock")));
        // Outside the workspace nothing is ignored (a parent folder named `data` does not count).
        assert!(!watch_ignored(Path::new("/data/app"), Path::new("/outside/data/x")));
        assert!(!watch_ignored(Path::new("/data/app"), Path::new("/data/app/src/x")));
        if cfg!(windows) {
            // Drive letter with another case and verbatim prefix `\\?\`: it is the same repository.
            assert!(git_state_change(Path::new(r"c:\w"), Path::new(r"C:\W\.git\HEAD")));
            assert!(git_state_change(Path::new(r"c:\w"), Path::new(r"\\?\C:\W\.git\HEAD")));
            assert!(git_state_change(Path::new(r"\\?\C:\w"), Path::new(r"c:\w\.git\CHERRY_PICK_HEAD")));
            assert!(watch_ignored(Path::new(r"c:\w"), Path::new(r"C:\w\target\x")));
            assert!(watch_ignored(Path::new(r"c:\w"), Path::new(r"\\?\C:\w\Target\x")));
            assert!(git_state_change_in(Path::new(r"c:\repo\.git"), Path::new(r"\\?\C:\REPO\.git\REVERT_HEAD")));
        }
        for p in ["/w/.git/index.lock", "/w/.git/objects/ab/cd", "/w/.git/logs/HEAD", "/w/src/HEAD", "/w/.gitignore"] {
            assert!(!git_state_change(ws, Path::new(p)), "{p}");
        }
        assert!(watch_ignored(ws, Path::new("/w/sub/node_modules/x")));
        assert!(watch_ignored(ws, Path::new("/w/target/debug/x")));
        assert!(!watch_ignored(ws, Path::new("/w/src/main.rs")));
        assert!(!watch_ignored(ws, Path::new("/w/.gitignore")));
    }

    #[test]
    fn watcher_observes_external_git_dir() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path();
        let ws = root.join("worktree");
        let real_git = root.join("repo-git").join("worktrees").join("wt");
        let common_git = root.join("repo-git");
        std::fs::create_dir_all(&ws).expect("mkdir ws");
        std::fs::create_dir_all(real_git.join("rebase-merge")).expect("mkdir git");
        std::fs::create_dir_all(common_git.join("refs").join("heads")).expect("mkdir refs");
        // Simulates a worktree without depending on the `git` binary installed on the machine.
        std::fs::write(ws.join(".git"), format!("gitdir: {}\n", real_git.display())).expect("dotgit");
        std::fs::write(real_git.join("commondir"), "../..\n").expect("commondir");

        let dirs = git_dirs_from_dotgit_file(&ws);
        assert_eq!(dirs.len(), 2, "{dirs:?}");
        assert!(crate::util::same_path(&dirs[0], &real_git));
        assert!(crate::util::same_path(&dirs[1], &common_git));

        let paths = crate::paths::AppPaths {
            app_dir: root.join("app"),
            bin_dir: root.join("bin"),
            data_dir: root.join("data"),
            default_workspace: ws.clone(),
            whisper_dir: root.join("whisper"),
            config_file: root.join("cfg").join("config.json"),
        };
        let opts = crate::StartOptions {
            search_path: Some(root.join("empty").into_os_string()),
            home_dir: Some(root.join("home")),
            watch_workspace: true,
            config: Some(crate::Config::with_project(ws.clone())),
            system_dirs: false,
        };
        let (core, rx) = crate::Core::start_with(paths, opts).expect("start");
        // Waits for the `cd-watch-debounce` thread to register the external `git_dir` in the watcher.
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut got_git = false;
        while Instant::now() < deadline && !got_git {
            std::fs::write(real_git.join("CHERRY_PICK_HEAD"), "deadbeef\n").expect("write head");
            while let Ok(ev) = rx.recv_timeout(Duration::from_millis(450)) {
                if matches!(ev, crate::CoreEvent::GitChanged) {
                    got_git = true;
                    break;
                }
            }
        }
        core.shutdown();
        assert!(got_git, "a change in the external git_dir should emit GitChanged");
    }
}
