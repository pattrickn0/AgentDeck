//! Task write scope: the paths the task declares it will modify.
//!
//! Pure functions, no git or disk access. The scope is a list of **paths** (files and directories relative to the
//! repository root), never globs: a directory reserves everything inside it. Two scopes overlap when a path is
//! equal to another or is a prefix of it **by component** (`crates/core` covers `crates/core/src/git.rs`;
//! `crates/co` does not cover `crates/core`). Comparison ignores case and treats `/` and `\` as equal (the
//! repository lives on Windows). The scope is used to queue tasks and warn about out-of-scope files in
//! `tasks/` (`lifecycle.rs`, `finish.rs`).

/// Paths that no task reserves (excluded from overlap checking and the "out of scope" warning):
/// every task modifies them, and they are already resolved during merge (see `memory/learnings/history-merges-by-union.md`).
/// Each item applies to itself and, if a directory, to everything inside it.
pub const UNRESERVED: &[&str] = &["memory", "Cargo.lock"];

/// How many paths a scope accepts, and the length of each.
pub const MAX_SCOPE_PATHS: usize = 64;
pub const MAX_SCOPE_PATH_CHARS: usize = 260;

/// The path components, in lowercase, with `/` and `\` equivalent; `.` and empty components are dropped.
fn components(path: &str) -> Vec<String> {
    path.split(['/', '\\']).filter(|c| !c.is_empty() && *c != ".").map(str::to_lowercase).collect()
}

/// `inner` is equal to `outer` or is inside it (by component).
fn covers(outer: &[String], inner: &[String]) -> bool {
    inner.len() >= outer.len() && inner[..outer.len()] == *outer
}

/// Is the path one that no task reserves ([`UNRESERVED`])?
pub fn is_unreserved(path: &str) -> bool {
    let parts = components(path);
    UNRESERVED.iter().any(|u| covers(&components(u), &parts))
}

/// Normalizes a scope path: `/` separator, no `./` or trailing slash. Rejects (with reason) anything that is not a
/// path relative to the repository root: empty, absolute, containing `..`, wildcards (`*`, `?`) or too long.
pub fn normalize(raw: &str) -> Result<String, String> {
    let text = raw.trim();
    if text.is_empty() {
        return Err("empty path in scope".into());
    }
    let shown = || text.chars().take(60).collect::<String>().escape_debug().to_string();
    if text.chars().count() > MAX_SCOPE_PATH_CHARS {
        return Err(format!("path too long in scope: \"{}…\"", shown()));
    }
    if text.contains(['*', '?']) {
        return Err(format!("scope is a list of paths, not patterns (glob): \"{}\"", shown()));
    }
    let windows_drive = text.as_bytes().get(1) == Some(&b':') && text.as_bytes()[0].is_ascii_alphabetic();
    if text.starts_with(['/', '\\']) || windows_drive {
        return Err(format!("scope path is relative to repository root, not absolute: \"{}\"", shown()));
    }
    let parts: Vec<&str> = text.split(['/', '\\']).filter(|c| !c.is_empty() && *c != ".").collect();
    if parts.is_empty() {
        return Err(format!("invalid path in scope: \"{}\"", shown()));
    }
    if parts.contains(&"..") {
        return Err(format!("scope path cannot escape repository (..): \"{}\"", shown()));
    }
    Ok(parts.join("/"))
}

/// Normalizes the entire list, without duplicate paths (same comparison as overlaps) and in the given order.
pub fn normalize_all<S: AsRef<str>>(raw: &[S]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for r in raw {
        let path = normalize(r.as_ref())?;
        if !out.iter().any(|p| components(p) == components(&path)) {
            out.push(path);
        }
    }
    if out.len() > MAX_SCOPE_PATHS {
        return Err(format!("scope too large: at most {MAX_SCOPE_PATHS} paths (group into folders)"));
    }
    Ok(out)
}

/// The scope without unreserved paths: this is what counts for checks.
fn reserved(scope: &[String]) -> Vec<Vec<String>> {
    scope.iter().filter(|p| !is_unreserved(p)).map(|p| components(p)).collect()
}

/// Do the two scopes overlap? A scope with no reserved paths (empty, or only `memory/…`/`Cargo.lock`)
/// overlaps nothing.
pub fn overlaps(a: &[String], b: &[String]) -> bool {
    let (a, b) = (reserved(a), reserved(b));
    a.iter().any(|x| b.iter().any(|y| covers(x, y) || covers(y, x)))
}

/// Of the modified files (git paths, relative to root), those outside the scope, excluding unreserved paths.
/// Empty if the task declared no reserved scope: then there is nothing to deviate from.
pub fn outside(scope: &[String], files: &[String]) -> Vec<String> {
    let scope = reserved(scope);
    if scope.is_empty() {
        return Vec::new();
    }
    files
        .iter()
        .filter(|f| !is_unreserved(f))
        .filter(|f| {
            let parts = components(f);
            !scope.iter().any(|s| covers(s, &parts))
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| (*p).to_owned()).collect()
    }

    #[test]
    fn a_folder_covers_what_is_inside_it_by_component() {
        assert!(overlaps(&s(&["crates/core"]), &s(&["crates/core/src/git.rs"])));
        assert!(overlaps(&s(&["crates/core/src/git.rs"]), &s(&["crates/core"])));
        assert!(overlaps(&s(&["crates/core"]), &s(&["crates/core"])));
        assert!(!overlaps(&s(&["crates/co"]), &s(&["crates/core"])));
        assert!(!overlaps(&s(&["crates/core"]), &s(&["crates/core2/x.rs"])));
        assert!(!overlaps(&s(&["crates/app"]), &s(&["crates/cli"])));
    }

    #[test]
    fn comparison_ignores_case_and_separator_kind() {
        assert!(overlaps(&s(&["Crates\\Core"]), &s(&["crates/core/src/git.rs"])));
        assert!(overlaps(&s(&["crates/core/"]), &s(&["./crates//core/src"])));
    }

    #[test]
    fn memory_and_cargo_lock_reserve_nothing() {
        assert!(!overlaps(&s(&["memory/backlog.md"]), &s(&["memory/backlog.md"])));
        assert!(!overlaps(&s(&["memory"]), &s(&["memory/OVERVIEW.md"])));
        assert!(!overlaps(&s(&["Cargo.lock"]), &s(&["cargo.lock"])));
        assert!(overlaps(&s(&["crates/app", "memory/backlog.md"]), &s(&["crates/app/src", "memory/backlog.md"])));
        assert!(!overlaps(&s(&["crates/app", "memory/backlog.md"]), &s(&["crates/cli", "memory/backlog.md"])));
        assert!(is_unreserved("MEMORY/learnings/x.md") && !is_unreserved("memory2/x.md") && !is_unreserved("crates/memory"));
    }

    #[test]
    fn an_empty_scope_overlaps_nothing() {
        assert!(!overlaps(&[], &s(&["crates/core"])));
        assert!(!overlaps(&s(&["crates/core"]), &[]));
        assert!(!overlaps(&[], &[]));
    }

    #[test]
    fn normalize_cleans_paths_and_refuses_the_rest() {
        assert_eq!(normalize(" .\\crates\\core\\ ").unwrap(), "crates/core");
        assert_eq!(normalize("crates/core/src/git.rs").unwrap(), "crates/core/src/git.rs");
        for bad in ["", "  ", "/etc/passwd", "\\x", "C:\\x", "c:/x", "../x", "a/../b", "crates/*.rs", "crates/?", ".", "./"] {
            assert!(normalize(bad).is_err(), "{bad:?} should be rejected");
        }
        assert!(normalize(&"a/".repeat(200)).is_err());
    }

    #[test]
    fn normalize_all_drops_repeats() {
        assert_eq!(normalize_all(&["crates/core", "Crates\\Core\\", "crates/app"]).unwrap(), s(&["crates/core", "crates/app"]));
        assert!(normalize_all(&["ok", ""]).is_err());
        let many: Vec<String> = (0..MAX_SCOPE_PATHS + 1).map(|i| format!("d{i}")).collect();
        assert!(normalize_all(&many).is_err());
    }

    #[test]
    fn outside_lists_files_the_scope_does_not_cover() {
        let scope = s(&["crates/core/src/tasks.rs", "crates/cli"]);
        let files = s(&[
            "crates/core/src/tasks.rs",
            "crates/cli/src/bin/ask-agent.rs",
            "crates/core/src/git.rs",
            "memory/OVERVIEW.md",
            "Cargo.lock",
        ]);
        assert_eq!(outside(&scope, &files), s(&["crates/core/src/git.rs"]));
        assert!(outside(&[], &files).is_empty(), "without scope there is no deviation");
        assert!(outside(&s(&["memory/x.md"]), &files).is_empty(), "scope with only unreserved paths also has no deviation");
    }
}
