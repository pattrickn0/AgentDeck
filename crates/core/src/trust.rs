//! Folder trust. Claude, Codex and Antigravity ask "Do you trust this folder?" the first time they open
//! in a folder, and a tab opened in the background (by `ask-agent` or by a task) hangs on the question, without
//! anyone seeing it. Opening the folder as a project in AgentDeck is already the user's decision, so the app records trust
//! before opening the CLI (verified on 2026-10-25 with Claude Code 2.1.310, Codex 0.164.0 and `agy` 1.4.7):
//!
//! - **Claude:** `projects["C:/folder"].hasTrustDialogAccepted` in `~/.claude.json`; applies to subfolders (a task's
//!   worktree inherits the project's), so it only writes when neither the folder nor a parent folder was accepted;
//! - **Codex:** `-c projects={'<folder>'={trust_level='trusted'}}` on the command line, without writing `config.toml`;
//! - **Antigravity:** `trustedWorkspaces` in `~/.gemini/antigravity-cli/settings.json`, by **exact path** (neither parent
//!   folder nor different casing works), which is why each worktree asked again; the entry is removed when the worktree is removed.
//!
//! The home folder and a drive root are never recorded: trusting them would apply to everything inside them.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{Map, Value};

/// Writes to CLI files are read, modify and replace: two tabs opening together must not race.
static WRITES: Mutex<()> = Mutex::new(());

/// Prepares trust for `cwd` (and `project_dir` if different) for the CLI `kind` and returns command-line arguments
/// (only Codex uses them). Write failures are only logged: in the worst case, the CLI prompts as before.
pub(crate) fn prepare(kind: &str, cwd: &Path, project_dir: &Path, home: &Path) -> Vec<String> {
    let mut dirs: Vec<&Path> = vec![cwd];
    if !same_path(cwd, project_dir) {
        dirs.push(project_dir);
    }
    dirs.retain(|d| may_trust(d, home));
    let Some(&first) = dirs.first() else { return Vec::new() };
    match kind {
        "claude" => {
            if let Err(e) = trust_claude(&claude_file(home), first) {
                log::warn!("folder trust for Claude ({}): {e}", first.display());
            }
            Vec::new()
        }
        "codex" => codex_args(&dirs),
        "antigravity" if first == cwd => {
            if let Err(e) = trust_agy(&agy_file(home), cwd) {
                log::warn!("folder trust for Antigravity ({}): {e}", cwd.display());
            }
            Vec::new()
        }
        _ => Vec::new(),
    }
}

/// Removes `dir` (the worktree of a task that just completed) from Antigravity's list, so it does not grow with each task.
pub(crate) fn forget(dir: &Path, home: &Path) {
    if let Err(e) = forget_agy(&agy_file(home), dir) {
        log::warn!("folder trust for Antigravity ({}): {e}", dir.display());
    }
}

/// The home folder and a drive root are excluded.
fn may_trust(dir: &Path, home: &Path) -> bool {
    dir.parent().is_some() && !same_path(dir, home)
}

fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        let s = p.to_string_lossy().replace('\\', "/");
        let s = s.trim_end_matches('/').to_owned();
        if cfg!(windows) {
            s.to_lowercase()
        } else {
            s
        }
    };
    norm(a) == norm(b)
}

/// `~/.claude.json` (or `$CLAUDE_CONFIG_DIR/.claude.json`, where Claude Code looks for it when the variable is set).
fn claude_file(home: &Path) -> PathBuf {
    match std::env::var_os("CLAUDE_CONFIG_DIR").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir).join(".claude.json"),
        None => home.join(".claude.json"),
    }
}

fn agy_file(home: &Path) -> PathBuf {
    home.join(".gemini").join("antigravity-cli").join("settings.json")
}

/// Claude's key: forward slashes, without trailing slash (`C:/Users/user/project`).
fn claude_key(dir: &Path) -> String {
    dir.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_owned()
}

/// `dir` or a parent directory already has accepted trust in Claude.
fn claude_trusted(projects: &Map<String, Value>, dir: &Path) -> bool {
    let mut cur = Some(dir);
    while let Some(d) = cur {
        let accepted = projects
            .iter()
            .any(|(k, v)| same_path(Path::new(k), d) && v.get("hasTrustDialogAccepted").and_then(Value::as_bool) == Some(true));
        if accepted {
            return true;
        }
        cur = d.parent();
    }
    false
}

/// Marks `dir` as accepted in `~/.claude.json`. Without the file (Claude Code never opened), does not create it: first run has
/// other prompts (login, theme) that the user needs to see anyway.
fn trust_claude(file: &Path, dir: &Path) -> std::io::Result<()> {
    let _guard = WRITES.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut root) = read_json(file)? else { return Ok(()) };
    let Some(obj) = root.as_object_mut() else { return Ok(()) };
    let projects = obj.entry("projects").or_insert_with(|| Value::Object(Map::new()));
    let Some(projects) = projects.as_object_mut() else { return Ok(()) };
    if claude_trusted(projects, dir) {
        return Ok(());
    }
    let entry = projects.entry(claude_key(dir)).or_insert_with(|| Value::Object(Map::new()));
    if let Some(entry) = entry.as_object_mut() {
        entry.insert("hasTrustDialogAccepted".into(), Value::Bool(true));
    }
    write_json(file, &root)?;
    log::info!("folder trust recorded for Claude: {}", dir.display());
    Ok(())
}

/// The path as `agy` sees it in current folder: on Windows, with `\` (the process receives normalized folder).
fn agy_key(dir: &Path) -> String {
    let s = dir.to_string_lossy();
    let s = if cfg!(windows) { s.replace('/', "\\") } else { s.into_owned() };
    let trimmed = s.trim_end_matches(['\\', '/']);
    if trimmed.is_empty() {
        s
    } else {
        trimmed.to_owned()
    }
}

/// Adds `dir` to Antigravity's `trustedWorkspaces`. Without the `antigravity-cli` folder (`agy` never opened), creates
/// nothing; without the file, creates it with just the list.
fn trust_agy(file: &Path, dir: &Path) -> std::io::Result<()> {
    if !file.parent().is_some_and(Path::is_dir) {
        return Ok(());
    }
    let _guard = WRITES.lock().unwrap_or_else(|e| e.into_inner());
    let mut root = read_json(file)?.unwrap_or_else(|| Value::Object(Map::new()));
    let Some(obj) = root.as_object_mut() else { return Ok(()) };
    let list = obj.entry("trustedWorkspaces").or_insert_with(|| Value::Array(Vec::new()));
    let Some(list) = list.as_array_mut() else { return Ok(()) };
    let key = agy_key(dir);
    if list.iter().any(|v| v.as_str() == Some(key.as_str())) {
        return Ok(());
    }
    list.push(Value::String(key));
    write_json(file, &root)?;
    log::info!("folder trust recorded for Antigravity: {}", dir.display());
    Ok(())
}

fn forget_agy(file: &Path, dir: &Path) -> std::io::Result<()> {
    let _guard = WRITES.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut root) = read_json(file)? else { return Ok(()) };
    let Some(list) = root.get_mut("trustedWorkspaces").and_then(Value::as_array_mut) else { return Ok(()) };
    let key = agy_key(dir);
    let before = list.len();
    list.retain(|v| v.as_str() != Some(key.as_str()));
    if list.len() != before {
        write_json(file, &root)?;
    }
    Ok(())
}

/// `-c projects={'<folder>'={trust_level='trusted'}}`. The value is TOML with single quotes, which survive the `.cmd` shim
/// (double quotes are removed by `cmd_safe_arg`). A folder with `'` cannot fit in a TOML string literal and is excluded. For
/// Codex trust applies by repository root; the worktree and the project are passed together and either is enough.
fn codex_args(dirs: &[&Path]) -> Vec<String> {
    let entries: Vec<String> = dirs
        .iter()
        .map(|d| d.to_string_lossy().into_owned())
        .filter(|s| !s.contains(['\'', '\n', '\r']))
        .map(|s| format!("'{s}'={{trust_level='trusted'}}"))
        .collect();
    if entries.is_empty() {
        return Vec::new();
    }
    vec!["-c".into(), format!("projects={{{}}}", entries.join(","))]
}

/// `None` when file does not exist. A file that is not JSON is an error: never rewrite what was not understood.
fn read_json(file: &Path) -> std::io::Result<Option<Value>> {
    let bytes = match std::fs::read(file) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let text = String::from_utf8_lossy(bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes)).into_owned();
    serde_json::from_str(&text).map(Some).map_err(std::io::Error::other)
}

/// Writes to a temporary file alongside and renames: the CLI never reads a half-written file.
fn write_json(file: &Path, value: &Value) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
    let tmp = file.with_extension(format!("agentdeck-{}.tmp", std::process::id()));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, file).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(file: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap()
    }

    #[test]
    fn the_home_folder_and_a_drive_root_are_never_trusted() {
        let home = Path::new("/home/voce");
        assert!(!may_trust(home, home));
        assert!(!may_trust(Path::new("/home/voce/"), home));
        assert!(!may_trust(Path::new("/"), home));
        assert!(may_trust(Path::new("/home/you/project"), home));
        assert!(prepare("codex", home, home, home).is_empty());
    }

    #[test]
    fn codex_gets_the_trust_on_the_command_line() {
        let args = codex_args(&[Path::new("/p/project/.agentdeck/worktrees/t1"), Path::new("/p/project")]);
        assert_eq!(
            args,
            ["-c", "projects={'/p/project/.agentdeck/worktrees/t1'={trust_level='trusted'},'/p/project'={trust_level='trusted'}}"]
        );
        assert!(codex_args(&[Path::new("/p/o'brien")]).is_empty(), "single quotes do not fit in string literal");
    }

    /// Project tab (folder = project) passes a single key: repeated key is invalid TOML and Codex will not open.
    #[test]
    fn codex_never_repeats_a_folder() {
        let home = Path::new("/home/voce");
        let p = Path::new("/p/project");
        assert_eq!(prepare("codex", p, Path::new("/p/project/"), home), ["-c", "projects={'/p/project'={trust_level='trusted'}}"]);
    }

    #[test]
    fn claude_marks_a_new_folder_and_respects_a_trusted_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join(".claude.json");
        // Without the file, nothing is created.
        trust_claude(&file, Path::new("/p/new")).unwrap();
        assert!(!file.exists());

        std::fs::write(&file, r#"{"numStartups": 3, "projects": {"/p/project": {"hasTrustDialogAccepted": true, "x": 1}}}"#).unwrap();
        // Worktree inside project inherits: file does not change.
        let before = std::fs::read_to_string(&file).unwrap();
        trust_claude(&file, Path::new("/p/project/.agentdeck/worktrees/t1")).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), before);

        trust_claude(&file, Path::new("/p/new")).unwrap();
        let v = json(&file);
        assert_eq!(v["projects"]["/p/new"]["hasTrustDialogAccepted"], true);
        assert_eq!(v["projects"]["/p/project"]["x"], 1, "the rest stays as it was");
        assert_eq!(v["numStartups"], 3);
    }

    #[test]
    fn claude_turns_an_existing_refusal_into_trust_and_keeps_the_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join(".claude.json");
        std::fs::write(&file, r#"{"projects": {"C:/p/new": {"hasTrustDialogAccepted": false, "allowedTools": ["a"]}}}"#).unwrap();
        trust_claude(&file, Path::new(r"C:\p\new")).unwrap();
        let v = json(&file);
        assert_eq!(v["projects"]["C:/p/new"]["hasTrustDialogAccepted"], true);
        assert_eq!(v["projects"]["C:/p/new"]["allowedTools"][0], "a");
    }

    #[test]
    fn antigravity_lists_the_exact_folder_and_forgets_it() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("antigravity-cli");
        let file = dir.join("settings.json");
        let wt = tmp.path().join("project").join("wt");
        // Without the agy directory, nothing is created.
        trust_agy(&file, &wt).unwrap();
        assert!(!file.exists());

        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&file, "\u{feff}{\"model\": \"Flash\", \"trustedWorkspaces\": [\"/outra\"]}").unwrap();
        trust_agy(&file, &wt).unwrap();
        trust_agy(&file, &wt).unwrap();
        let v = json(&file);
        let key = agy_key(&wt);
        assert_eq!(v["trustedWorkspaces"], serde_json::json!(["/outra", key]), "only once");
        assert_eq!(v["model"], "Flash");

        forget_agy(&file, &wt).unwrap();
        assert_eq!(json(&file)["trustedWorkspaces"], serde_json::json!(["/outra"]));
    }

    #[test]
    fn a_file_that_is_not_json_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("settings.json");
        std::fs::write(&file, "{ this is not json").unwrap();
        assert!(trust_agy(&file, Path::new("/p/x")).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{ this is not json");
    }

    #[cfg(windows)]
    #[test]
    fn on_windows_the_agy_key_uses_backslashes() {
        assert_eq!(agy_key(Path::new("C:/Users/you/project/")), r"C:\Users\you\project");
        assert!(claude_trusted(
            serde_json::json!({"c:/users/you/project": {"hasTrustDialogAccepted": true}}).as_object().unwrap(),
            Path::new(r"C:\Users\you\project\.agentdeck\worktrees\t3")
        ));
    }
}
