use std::path::Path;

use super::text::cap_text;
use super::{Task, MAX_STAT};
use crate::git::{run, GitError, Repo, COMMIT_TIMEOUT, WRITE_TIMEOUT};
use crate::t;
use crate::util;

/// Dirty paths from a `git status --porcelain=v1` (the destination, in renames).
pub fn dirty_paths(porcelain: &str) -> Vec<String> {
    porcelain
        .lines()
        .filter(|l| l.len() > 3)
        .map(|l| {
            let path = &l[3..];
            let path = path.rsplit_once(" -> ").map_or(path, |(_, new)| new);
            path.trim_matches('"').to_owned()
        })
        .collect()
}

/// The worktree folder is inside `<root>/.agentdeck/worktrees` (and is not the parent folder itself).
pub(super) fn safe_worktree(worktree: &Path, allowed: &Path) -> bool {
    let wt = util::absolutize(worktree);
    util::path_within(&wt, allowed) && !util::same_path(&wt, allowed)
}

/// Task branch (`tarefa/…`) with an acceptable name.
pub(super) fn valid_task_branch(branch: &str) -> bool {
    branch.starts_with("tarefa/") && crate::git::valid_branch_name(branch)
}

/// Standard output of a git command in the main repository (limit of the local operations).
pub(super) fn git_out(repo: &Repo, args: &[&str]) -> Result<String, GitError> {
    run(repo.cmd(args), None, WRITE_TIMEOUT).map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
}

/// Files the branch brought relative to the base (`git diff --name-only base...branch`); empty if the branch no
/// longer exists or git fails.
pub(crate) fn changed_files(repo: &Repo, base: &str, branch: &str) -> Vec<String> {
    if !valid_task_branch(branch) || !crate::git::valid_branch_name(base) {
        return Vec::new();
    }
    let range = format!("{base}...{branch}");
    repo.read(&["diff", "--name-only", "--no-ext-diff", &range, "--"])
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect())
        .unwrap_or_default()
}

/// `git diff --stat base...branch`, cut; "no changes" when empty.
pub(super) fn diff_stat(repo: &Repo, base: &str, branch: &str) -> Result<String, String> {
    let out = git_out(repo, &["diff", "--stat", &format!("{base}...{branch}"), "--"]).map_err(|e| e.to_string())?;
    let out = out.trim();
    if out.is_empty() {
        return Ok(t!("(no changes relative to the base)").into());
    }
    Ok(cap_text(out, MAX_STAT))
}

/// Paths with changes (including new, untracked ones) in the worktree. `None` if the folder does not exist
/// or git did not answer.
pub(super) fn worktree_dirty(repo: &Repo, worktree: &Path) -> Option<Vec<String>> {
    if !worktree.is_dir() {
        return None;
    }
    let cmd = crate::git::base_command(&repo.git, worktree, &["status", "--porcelain=v1"]);
    let out = run(cmd, None, WRITE_TIMEOUT).ok()?;
    Some(dirty_paths(&String::from_utf8_lossy(&out.stdout)))
}

/// Like [`worktree_dirty`], but asking git not to take the worktree's `index.lock` (`--no-optional-locks`): it
/// serves to look at another agent's folder without getting in the way of its `git add`/`commit`.
pub(crate) fn worktree_dirty_unlocked(repo: &Repo, worktree: &Path) -> Option<Vec<String>> {
    if !worktree.is_dir() {
        return None;
    }
    let cmd = crate::git::base_command(&repo.git, worktree, &["--no-optional-locks", "status", "--porcelain=v1"]);
    let out = run(cmd, None, WRITE_TIMEOUT).ok()?;
    Some(dirty_paths(&String::from_utf8_lossy(&out.stdout)))
}

/// Joins the branch's files with the uncommitted ones: normalized (`/`), without repeats, in alphabetical order and
/// without `AGENTS.md`, which the app rewrites and is not the agent's work.
pub(crate) fn union_files(committed: Vec<String>, dirty: Option<Vec<String>>) -> Vec<String> {
    let mut files: Vec<String> = committed
        .into_iter()
        .chain(dirty.unwrap_or_default())
        .map(|f| crate::learnings::normalize_path(&f))
        .filter(|f| !f.is_empty() && f != "AGENTS.md")
        .collect();
    files.sort();
    files.dedup();
    files
}

/// The short hash (7 characters) of a full hash, for messages and events.
pub(super) fn short_hash(hash: &str) -> &str {
    hash.get(..7).unwrap_or(hash)
}

/// The branch exists in this repository.
pub(super) fn branch_exists(repo: &Repo, branch: &str) -> bool {
    repo.read(&["rev-parse", "--verify", "-q", &format!("refs/heads/{branch}")]).is_ok()
}

/// Puts the task's branch on the same commit as the already merged base, inside the worktree (`git merge --ff-only`:
/// the branch is an ancestor of the merge commit, so it advances without creating a commit). Error = git's message.
pub(super) fn align_with_base(repo: &Repo, task: &Task) -> Result<(), String> {
    if !task.worktree.is_dir() {
        return Err(t!("the folder {folder} does not exist", folder = task.worktree.display()));
    }
    let cmd = crate::git::base_command(&repo.git, &task.worktree, &["merge", "--ff-only", &task.base]);
    run(cmd, None, COMMIT_TIMEOUT).map(|_| ()).map_err(|e| e.to_string())
}

/// Deletes the branch (if it exists). `force` = `-D`.
pub(super) fn delete_branch(repo: &Repo, branch: &str, force: bool) -> Result<(), String> {
    if repo.read(&["rev-parse", "--verify", "-q", &format!("refs/heads/{branch}")]).is_err() {
        return Ok(());
    }
    repo.write(&["branch", if force { "-D" } else { "-d" }, branch], None).map(|_| ()).map_err(|e| e.to_string())
}

/// New files of the user's (untracked, in the main repository) that the task's branch also creates.
pub(super) fn untracked_clash(repo: &Repo, base: &str, branch: &str) -> Vec<String> {
    let range = format!("{base}...{branch}");
    let added = git_out(repo, &["diff", "--name-only", "-z", "--no-renames", "--diff-filter=A", &range, "--"]).unwrap_or_default();
    let untracked = git_out(repo, &["ls-files", "--others", "--exclude-standard", "-z"]).unwrap_or_default();
    let mine: std::collections::HashSet<&str> = untracked.split('\0').filter(|p| !p.is_empty()).collect();
    added.split('\0').filter(|p| mine.contains(p)).map(str::to_owned).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_paths() {
        let text = " M src/a.rs\n?? new file.txt\nR  old.rs -> new.rs\nA  \"with quotes.txt\"\n";
        assert_eq!(dirty_paths(text), ["src/a.rs", "new file.txt", "new.rs", "with quotes.txt"]);
        assert!(dirty_paths("").is_empty());
    }

    #[test]
    fn the_situation_files_join_the_branch_and_the_uncommitted_work() {
        let got = union_files(
            vec!["b.rs".into(), "a.rs".into(), "AGENTS.md".into()],
            Some(vec!["c\\d.rs".into(), "a.rs".into(), "AGENTS.md".into()]),
        );
        assert_eq!(got, ["a.rs", "b.rs", "c/d.rs"]);
        // A worktree that git did not answer does not erase what the branch already has.
        assert_eq!(union_files(vec!["a.rs".into()], None), ["a.rs"]);
        assert!(union_files(Vec::new(), Some(Vec::new())).is_empty());
    }

    #[test]
    fn only_the_expected_folder_is_ever_touched() {
        // As in the real code, the allowed folder already comes absolute (on Windows, `/repo` gets the drive).
        let allowed = &util::absolutize(Path::new("/repo/.agentdeck/worktrees"));
        assert!(safe_worktree(Path::new("/repo/.agentdeck/worktrees/t1"), allowed));
        assert!(!safe_worktree(Path::new("/repo/.agentdeck/worktrees"), allowed));
        assert!(!safe_worktree(Path::new("/repo/src"), allowed));
        assert!(!safe_worktree(Path::new("/repo/.agentdeck/worktrees/../../src"), allowed));
        assert!(valid_task_branch("tarefa/t1-x") && !valid_task_branch("main") && !valid_task_branch("tarefa/../x"));
    }
}
