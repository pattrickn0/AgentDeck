use super::git_ops::git_out;
use super::text::short_list;
use super::Task;
use crate::git::{run, Repo, COMMIT_TIMEOUT};
use crate::t;

/// The stash the app created to keep the main repository's changes (the hash identifies the entry: the stash stack
/// is shared with the other worktrees, `stash@{0}` may not be ours at the time of giving back).
pub(super) struct Stash {
    sha: String,
    message: String,
}

/// The top of the stash (`refs/stash`), if there is one.
fn stash_top(repo: &Repo) -> Option<String> {
    repo.read(&["rev-parse", "-q", "--verify", "refs/stash"]).ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
}

/// Stashes the uncommitted changes of the main repository's tracked files, except `AGENTS.md` (the app handles it
/// separately). Does not take untracked files. `Ok(None)` = there was nothing to stash.
pub(super) fn stash_push(repo: &Repo, task: &Task) -> Result<Option<Stash>, String> {
    let listed = git_out(repo, &["diff", "--name-only", "-z", "--no-renames", "HEAD", "--"]).map_err(|e| e.to_string())?;
    let mut stdin = Vec::new();
    for path in listed.split('\0').filter(|p| !p.is_empty() && *p != "AGENTS.md") {
        stdin.extend_from_slice(path.as_bytes());
        stdin.push(0);
    }
    if stdin.is_empty() {
        return Ok(None);
    }
    let before = stash_top(repo);
    // The entry's name is what the user sees in `git stash list`, and the notices quote it: plain English.
    let message = format!("holt: stashed before merging task {}", task.id);
    let args = ["stash", "push", "-m", message.as_str(), "--pathspec-from-file=-", "--pathspec-file-nul"];
    run(repo.cmd(&args), Some(stdin), COMMIT_TIMEOUT).map_err(|e| e.to_string())?;
    let after = stash_top(repo);
    Ok(after.filter(|a| Some(a) != before.as_ref()).map(|sha| Stash { sha, message }))
}

/// The current position (`stash@{n}`) of the stash entry, if it still exists.
fn stash_ref(repo: &Repo, sha: &str) -> Option<String> {
    let list = git_out(repo, &["stash", "list", "--format=%H %gd"]).ok()?;
    list.lines().find_map(|l| l.split_once(' ').filter(|(h, _)| *h == sha).map(|(_, r)| r.trim().to_owned()))
}

/// The files where giving the stash back on top of `HEAD` would conflict, without touching the folder
/// (`git merge-tree --write-tree`, git 2.38+). `None` = could not check (old git, error).
fn stash_pop_conflicts(repo: &Repo, sha: &str) -> Option<Vec<String>> {
    let base = format!("--merge-base={sha}^1");
    let out = repo.cmd(&["merge-tree", "--write-tree", "--name-only", "--no-messages", &base, "HEAD", sha]).output().ok()?;
    match out.status.code() {
        Some(0) => Some(Vec::new()),
        Some(1) => {
            let text = String::from_utf8_lossy(&out.stdout);
            // The first line is the tree; the conflicting files come after it, up to an empty line.
            Some(text.lines().skip(1).take_while(|l| !l.is_empty()).map(str::to_owned).collect())
        }
        _ => None,
    }
}

/// Gives the stash back to the main folder after the merge. Only does `stash pop` when the check says there is no
/// conflict; when in doubt, or on a conflict, it leaves it in the stash (intact) and the notice explains how to
/// recover it. Returns the notice and whether the stash is still kept.
pub(super) fn stash_pop(repo: &Repo, stash: &Stash) -> (String, bool) {
    let kept = |why: String| {
        (
            t!(
                "Your uncommitted changes are still kept in git's stash (\"{message}\"): {why} Nothing was lost. To recover them: open a terminal in the project folder, run git stash list to find the entry and git stash pop to give it back (resolving whatever clashes).",
                message = &stash.message,
                why
            ),
            true,
        )
    };
    if stash_ref(repo, &stash.sha).is_none() {
        return (
            t!("The stash with your changes is no longer in git's list (did someone use or delete it?); check with git stash list.").into(),
            true,
        );
    }
    match stash_pop_conflicts(repo, &stash.sha) {
        Some(files) if files.is_empty() => {}
        Some(files) => {
            return kept(t!("giving them back, they would clash with what the task brought in {files}.", files = short_list(&files)))
        }
        None => return kept(t!("I could not check whether they clash with what the task brought.").into()),
    }
    // Applies by sha, not by `stash@{n}`: the stack belongs to all worktrees, and another `stash push` between the check
    // and the giving back would change the position (the `pop` would give back and delete someone else's entry). The
    // entry only leaves afterwards, looked up again by its sha.
    match repo.write(&["stash", "apply", &stash.sha], None) {
        Ok(_) => {
            if let Some(reference) = stash_ref(repo, &stash.sha) {
                if let Err(e) = repo.write(&["stash", "drop", &reference], None) {
                    log::warn!("the changes came back, but the stash entry remained ({e})");
                }
            }
            (t!("Your uncommitted changes were stashed during the merge and are already back in the folder.").to_owned(), false)
        }
        Err(e) => {
            let unmerged = git_out(repo, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
            let mess =
                if unmerged.trim().is_empty() { "" } else { t!(" The folder was left with conflicting files: check with git status.") };
            kept(t!("git could not give them back ({error}).{mess}", error = e, mess))
        }
    }
}
