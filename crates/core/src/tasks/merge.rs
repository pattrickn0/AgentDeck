use super::git_ops::{
    align_with_base, changed_files, git_out, safe_worktree, short_hash, untracked_clash, valid_task_branch, worktree_dirty,
};
use super::stash::{stash_pop, stash_push, Stash};
use super::text::short_list;
use super::{dirty_paths, git_error, MergeBlock, MergeOutcome, MergeRefusal, Task, TaskError, TaskStatus};
use crate::events::{CoreEvent, FeedEntry};
use crate::git::{run, RepoState, COMMIT_TIMEOUT};
use crate::t;
use crate::util::now_ms;
use crate::Core;

impl Core {
    // ---------- Merge ----------

    /// Merges the task's branch into the base (`git merge --no-ff`) in the main repository and puts the task in
    /// `Validating`: the tab, the worktree and the branch stay (the user will still test it) and the task's branch is
    /// aligned with the already merged base (`git merge --ff-only`). Requires `Review` (or `Failed` with a report,
    /// to try again). Refuses, changing nothing, if the main repository is not on the base or has uncommitted
    /// changes in tracked files (`AGENTS.md`, which the app rewrites, does not count), or if the task's worktree has
    /// uncommitted work. On a conflict, it undoes the merge (`merge --abort`) and the task stays in `Review`, with
    /// the conflicting files in the message. If git fails for another reason, the task also stays in `Review`, with
    /// the error in the history and returned to the caller. If the main folder's `AGENTS.md` differs only by the
    /// app's block and the branch changes it, it goes back to the HEAD's during the merge and the block is rewritten
    /// afterwards.
    pub fn task_merge(&self, project: &str, id: &str) -> Result<Task, TaskError> {
        self.merge_task(project, id, false).map(|o| o.task).map_err(|r| r.error)
    }

    /// Like [`Core::task_merge`], for the interface: the refusal that has a way out comes with the structured reason
    /// ([`MergeRefusal::block`], for the screen to offer the button), and `stash_main` turns on "stash my changes,
    /// merge and give them back": the uncommitted changes of the main repository go to a `git stash` (identified by
    /// the task) and come back (`stash pop`) after the merge, whatever the outcome. If they do not come back clean
    /// (conflict with what the task brought), they stay in the stash, intact, and the notice says how to recover
    /// them. Only the user's click turns this on: the command line (`task_merge`) never stashes anything.
    pub fn task_merge_assisted(&self, project: &str, id: &str, stash_main: bool) -> Result<MergeOutcome, MergeRefusal> {
        self.merge_task(project, id, stash_main)
    }

    fn merge_task(&self, project: &str, id: &str, stash_main: bool) -> Result<MergeOutcome, MergeRefusal> {
        let _op = self.inner.tasks.op.lock();
        let task = self.task_or_404(project, id)?;
        let retry = task.status == TaskStatus::Failed && task.report.is_some();
        if task.status != TaskStatus::Review && !retry {
            return Err(TaskError::Conflict(t!(
                "task {id} is {status}: only a task in review can be merged",
                id = &task.id,
                status = task.status.label()
            ))
            .into());
        }
        let repo = self.task_repo(project)?;
        let refuse_with = |t: &Task, msg: String, block: Option<MergeBlock>| -> MergeRefusal {
            self.update_task(project, &t.id, |t| t.log(t!("merge refused: {reason}", reason = &msg)));
            MergeRefusal { error: TaskError::Conflict(msg), block, stash_note: None }
        };
        let refuse = |t: &Task, msg: String| -> MergeRefusal { refuse_with(t, msg, None) };
        if !valid_task_branch(&task.branch) || !crate::git::valid_branch_name(&task.base) || !safe_worktree(&task.worktree, &repo.root) {
            return Err(TaskError::Failed(t!("data of task {id} outside what was expected; I merged nothing", id = &task.id)).into());
        }
        let _git = self.inner.git.op.lock();

        // The main repository must be ready to receive the merge.
        match repo.read(&["symbolic-ref", "--short", "-q", "HEAD"]) {
            Ok(out) if String::from_utf8_lossy(&out.stdout).trim() == task.base => {}
            Ok(out) => {
                let now = String::from_utf8_lossy(&out.stdout).trim().to_owned();
                return Err(refuse(
                    &task,
                    t!(
                        "the main repository is on the branch \"{now}\", not \"{base}\": switch to \"{base}\" and try again",
                        now,
                        base = &task.base
                    ),
                ));
            }
            Err(_) => {
                return Err(refuse(
                    &task,
                    t!("the main repository has a detached HEAD: switch to \"{base}\" and try again", base = &task.base),
                ))
            }
        }
        if repo.state() != RepoState::Normal {
            return Err(refuse(
                &task,
                t!("the main repository is in the middle of another operation (merge, rebase…): finish or abort it first").into(),
            ));
        }
        let status = git_out(&repo, &["status", "--porcelain=v1", "--untracked-files=no"]).map_err(git_error)?;
        // The app rewrites the AGENTS.md block; git protects the file if the merge touches it.
        let dirty: Vec<String> = dirty_paths(&status).into_iter().filter(|p| p != "AGENTS.md").collect();
        if !dirty.is_empty() && !stash_main {
            return Err(refuse_with(
                &task,
                t!("the main repository has uncommitted changes ({files}): commit or stash them and try again", files = short_list(&dirty)),
                Some(MergeBlock::MainDirty(dirty)),
            ));
        }
        if let Some(d) = worktree_dirty(&repo, &task.worktree).filter(|d| !d.is_empty()) {
            return Err(refuse_with(
                &task,
                t!(
                    "the task's worktree has uncommitted work ({files}): return the task asking for the commit, or discard it",
                    files = short_list(&d)
                ),
                Some(MergeBlock::TaskDirty(d)),
            ));
        }
        if repo.read(&["rev-parse", "--verify", "-q", &format!("refs/heads/{}", task.branch)]).is_err() {
            return Err(refuse(&task, t!("the branch {branch} no longer exists: discard the task", branch = &task.branch)));
        }
        let ahead = git_out(&repo, &["rev-list", "--count", &format!("{}..{}", task.base, task.branch)]).map_err(git_error)?;
        if ahead.trim() == "0" {
            // Rejected and delivered without a new commit: the agent concluded there was nothing to change. What the
            // last merge brought is already in the base; it goes back to waiting for validation, with no new merge commit.
            if let Some(last) = task.merge_commit.as_deref() {
                let short = short_hash(last).to_owned();
                let updated = self.update_task(project, &task.id, |t| {
                    t.status = TaskStatus::Validating;
                    t.finished_ms = Some(now_ms());
                    t.error = None;
                    t.log(t!("no new commits since the last merge ({hash}); awaiting validation again", hash = &short));
                    t.clone()
                });
                let task = updated.ok_or_else(|| TaskError::NotFound(t!("task {id} vanished", id)))?;
                self.feed(
                    FeedEntry::system(t!(
                        "Task {id} has no new commits, awaiting validation again: {title}",
                        id = &task.id,
                        title = &task.title
                    ))
                    .in_project(project),
                );
                return Ok(MergeOutcome { task, note: None });
            }
            return Err(refuse(
                &task,
                t!(
                    "the branch {branch} has no new commits relative to \"{base}\": there is nothing to merge (discard the task)",
                    branch = &task.branch,
                    base = &task.base
                ),
            ));
        }
        if let Err(e) = repo.check_gpg_ready() {
            return Err(refuse(&task, e.to_string()));
        }
        // A new (untracked) file of the user's that the branch also creates: git would refuse in the middle of the merge.
        // It applies with or without "stash": the stash does not take untracked files, and here nothing was stashed yet.
        let clash = untracked_clash(&repo, &task.base, &task.branch);
        if !clash.is_empty() {
            return Err(refuse(
                &task,
                t!(
                    "the main repository has new files of yours, uncommitted, that the task also creates ({files}): move them to another folder (or delete them) and try again",
                    files = short_list(&clash)
                ),
            ));
        }

        // "Stash my changes": what is uncommitted goes to the stash (except AGENTS.md, which the app handles
        // separately, below). From here on every outcome goes through `give_back`, which returns the stash.
        let mut stashed: Option<Stash> = None;
        if stash_main && !dirty.is_empty() {
            match stash_push(&repo, &task) {
                Ok(s) => stashed = s,
                Err(e) => {
                    return Err(refuse(
                        &task,
                        t!("could not stash the main repository's changes ({error}); nothing was changed", error = e),
                    ))
                }
            }
            if stashed.is_some() {
                self.update_task(project, &task.id, |t| {
                    t.log(t!("stashed {count} uncommitted file(s) of the main repository", count = dirty.len()))
                });
            }
        }
        let give_back = || -> Option<String> {
            let stash = stashed.as_ref()?;
            let (note, kept) = stash_pop(&repo, stash);
            self.update_task(project, &task.id, |t| {
                t.log(if kept { t!("stashed changes: still in the stash") } else { t!("stashed changes: given back") })
            });
            Some(note)
        };

        // The app rewrites the AGENTS.md block in the main folder, and git refuses to merge over a changed file that
        // the branch also touches. If the change is only the app's block, the file goes back to the HEAD's for the
        // merge and the block is rewritten afterwards (on success, undone conflict or failure). If the user edited the
        // text outside the block, nothing is discarded: it refuses.
        let mut app_block: Option<String> = None;
        let agents_dirty = dirty_paths(&status).iter().any(|p| p == "AGENTS.md");
        if agents_dirty && changed_files(&repo, &task.base, &task.branch).iter().any(|f| f == "AGENTS.md") {
            let file = repo.root.join("AGENTS.md");
            let current = std::fs::read_to_string(&file).unwrap_or_default();
            let committed = git_out(&repo, &["show", "HEAD:AGENTS.md"]).unwrap_or_default();
            if !crate::workspace::only_marker_blocks_differ(&committed, &current) {
                let note = give_back();
                return Err(refuse(
                    &task,
                    with_note(
                        t!(
                            "the main repository's AGENTS.md has uncommitted changes of yours, and the branch {branch} also touches it: commit or stash them and try again",
                            branch = &task.branch
                        ),
                        note,
                    ),
                ));
            }
            if let Err(e) = repo.write(&["checkout", "--", "AGENTS.md"], None) {
                let note = give_back();
                return Err(refuse(
                    &task,
                    with_note(t!("could not prepare the main repository's AGENTS.md for the merge: {error}", error = e), note),
                ));
            }
            app_block = Some(current);
        }
        let restore_block = || {
            let Some(saved) = &app_block else { return };
            if let Err(e) = crate::workspace::restore_marker_block(&repo.root.join("AGENTS.md"), saved) {
                log::warn!("failed to rewrite the AGENTS.md block after the merge: {e}");
                self.update_task(project, &task.id, |t| {
                    t.log(t!("warning: could not rewrite the app's block in AGENTS.md: {error}", error = &e))
                });
            }
        };
        // The merge (`examples/memeval.rs` finds the merged tasks in the history by the `Merge task tN` subject).
        let message = format!("Merge task {}: {}", task.id, task.title);
        if let Err(e) = run(repo.cmd(&["merge", "--no-ff", "-m", &message, &task.branch]), None, COMMIT_TIMEOUT) {
            if repo.state() == RepoState::Merging {
                let files = git_out(&repo, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
                let files: Vec<String> = files.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect();
                return match repo.write(&["merge", "--abort"], None) {
                    Ok(_) => {
                        restore_block();
                        let note = give_back();
                        let msg = t!(
                            "conflict while merging {branch} into \"{base}\" (files: {files}); the merge was undone and the task is still in review. Return it to the agent asking it to update the branch with the base, or resolve it by hand",
                            branch = &task.branch,
                            base = &task.base,
                            files = if files.is_empty() { t!("not identified").to_owned() } else { short_list(&files) }
                        );
                        self.update_task(project, &task.id, |t| t.log(t!("conflict in the merge ({files}); undone", files = short_list(&files))));
                        Err(MergeRefusal {
                            error: TaskError::Conflict(with_note(msg, note.clone())),
                            block: Some(MergeBlock::Conflict(files)),
                            stash_note: note,
                        })
                    }
                    // If it cannot be undone, the folder is in the middle of the merge: the stash is not given back on top.
                    Err(abort) => Err(self
                        .fail_merge(
                            project,
                            &task.id,
                            if stashed.is_some() {
                                t!(
                                    "conflict while merging and could not undo it ({abort}): run git merge --abort in {folder}; your uncommitted changes are still stashed (git stash list)",
                                    abort,
                                    folder = repo.root.display()
                                )
                            } else {
                                t!(
                                    "conflict while merging and could not undo it ({abort}): run git merge --abort in {folder}",
                                    abort,
                                    folder = repo.root.display()
                                )
                            },
                        )
                        .into()),
                };
            }
            restore_block();
            let note = give_back();
            return Err(self.fail_merge(project, &task.id, with_note(t!("the merge failed: {error}", error = e), note)).into());
        }
        restore_block();
        let note = give_back();
        let full_head = git_out(&repo, &["rev-parse", "HEAD"]).map(|s| s.trim().to_owned()).unwrap_or_default();
        let head = short_hash(&full_head).to_owned();

        // The task's branch becomes the base itself: if rejected, the agent continues from there.
        let aligned = align_with_base(&repo, &task);
        let merged = self.update_task(project, &task.id, |t| {
            t.status = TaskStatus::Validating;
            t.finished_ms = Some(now_ms());
            t.error = None;
            t.merge_commit = Some(full_head.clone()).filter(|h| !h.is_empty());
            t.log(t!("merged into {base} ({hash}); awaiting validation", base = &t.base, hash = &head));
            if let Err(e) = &aligned {
                t.log(t!(
                    "warning: could not align the branch {branch} with {base}: {error}",
                    branch = &t.branch,
                    base = &t.base,
                    error = e
                ));
            }
            t.clone()
        });
        let task = merged.ok_or_else(|| TaskError::NotFound(t!("task {id} vanished", id)))?;
        self.emit(CoreEvent::GitChanged);
        self.feed(
            FeedEntry::system(t!(
                "Task {id} merged into {base}, awaiting validation: {title}",
                id = &task.id,
                base = &task.base,
                title = &task.title
            ))
            .in_project(project),
        );
        // The task no longer holds the scope: whoever waited for it departs from the base already with this merge.
        self.advance_queue_locked(project);
        Ok(MergeOutcome { task, note })
    }
}

/// The message followed by the stash notice, if there was one.
fn with_note(msg: String, note: Option<String>) -> String {
    match note {
        Some(n) => format!("{msg} {n}"),
        None => msg,
    }
}
