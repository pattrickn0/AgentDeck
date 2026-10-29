use super::git_ops::{align_with_base, branch_exists, delete_branch, git_out, safe_worktree, valid_task_branch, worktree_dirty};
use super::lifecycle::queue_holder;
use super::text::{cap_text, short_list};
use super::{git_error, reject_message, Task, TaskError, TaskStatus, AGENTDECK_DIR, MAX_TEXT, REMOVE_TRIES, REMOVE_WAIT};
use crate::events::{CoreEvent, FeedEntry};
use crate::git::{run, Repo, COMMIT_TIMEOUT};
use crate::t;
use crate::util::{self, now_ms};
use crate::Core;

impl Core {
    // ---------- Validate ----------

    /// The user tested and approved: closes the tab, removes the worktree and the branch and marks the task as
    /// completed (`Merged`). Requires `Validating`. Refuses, changing nothing, if the worktree has uncommitted work
    /// or if the branch has commits that the base does not have (the agent kept working): reject the task or
    /// discard it.
    pub fn task_validate(&self, project: &str, id: &str) -> Result<Task, TaskError> {
        let _op = self.inner.tasks.op.lock();
        let task = self.task_or_404(project, id)?;
        if task.status != TaskStatus::Validating {
            return Err(TaskError::Conflict(t!(
                "task {id} is {status}: only a task awaiting validation can be validated",
                id = &task.id,
                status = task.status.label()
            )));
        }
        let repo = self.task_repo(project)?;
        let expected = util::absolutize(&repo.root.join(AGENTDECK_DIR).join("worktrees"));
        if !valid_task_branch(&task.branch) || !crate::git::valid_branch_name(&task.base) || !safe_worktree(&task.worktree, &expected) {
            return Err(TaskError::Failed(t!("data of task {id} outside what was expected; I removed nothing", id = &task.id)));
        }
        let _git = self.inner.git.op.lock();
        let refuse = |msg: String| -> TaskError {
            self.update_task(project, &task.id, |t| t.log(t!("validation refused: {reason}", reason = &msg)));
            TaskError::Conflict(msg)
        };
        if let Some(d) = worktree_dirty(&repo, &task.worktree).filter(|d| !d.is_empty()) {
            return Err(refuse(t!(
                "the task's worktree has uncommitted work ({files}): reject the task asking for the commit, or discard it",
                files = short_list(&d)
            )));
        }
        let mut warnings = Vec::new();
        if branch_exists(&repo, &task.branch) {
            if branch_exists(&repo, &task.base) {
                let ahead = git_out(&repo, &["rev-list", "--count", &format!("{}..{}", task.base, task.branch)]).map_err(git_error)?;
                if ahead.trim() != "0" {
                    return Err(refuse(t!(
                        "the branch {branch} has commits that were not merged into \"{base}\" yet: reject the task so the agent delivers them, or discard it",
                        branch = &task.branch,
                        base = &task.base
                    )));
                }
            } else {
                warnings.push(t!(
                    "the base {base} no longer exists; I did not check whether the branch had commits outside it",
                    base = &task.base
                ));
            }
        }

        // Cleanup: tab, worktree, branch (what it brought is already in the base).
        self.close_task_instance(project, &task);
        warnings.extend(self.remove_task_worktree(&repo, &task));
        if task.worktree.exists() {
            // Something still holds the folder: completing now would leave an orphan folder and branch, with no way to clean up later.
            let why = warnings.join("; ");
            self.update_task(project, &task.id, |t| t.log(t!("validation incomplete: {why}", why = &why)));
            return Err(TaskError::Conflict(t!(
                "could not delete the worktree folder ({why}): close whatever is using it and validate again",
                why
            )));
        }
        if let Err(e) = delete_branch(&repo, &task.branch, true) {
            warnings.push(t!("the branch {branch} remained: {error}", branch = &task.branch, error = e));
        }
        let done = self.update_task(project, &task.id, |t| {
            t.status = TaskStatus::Merged;
            t.finished_ms = Some(now_ms());
            t.error = None;
            t.log(t!("validated; completed; worktree and branch removed"));
            for w in &warnings {
                t.log(t!("warning: {warning}", warning = w));
            }
            t.clone()
        });
        let task = done.ok_or_else(|| TaskError::NotFound(t!("task {id} vanished", id)))?;
        self.emit(CoreEvent::GitChanged);
        self.feed(FeedEntry::system(t!("Task {id} completed: {title}", id = &task.id, title = &task.title)).in_project(project));
        Ok(task)
    }

    // ---------- Reject ----------

    /// The user tested and rejected: `Validating` goes back to `Running` in the same worktree (which already has the
    /// base's code) and the reason is delivered to the same agent (the tab is reopened if it was closed). If the
    /// worktree folder vanished in the meantime, it is recreated from the branch (or the base). The cycle goes on:
    /// done → review → merge → awaiting validation.
    pub fn task_reject(&self, project: &str, id: &str, feedback: &str) -> Result<Task, TaskError> {
        let _op = self.inner.tasks.op.lock();
        let task = self.task_or_404(project, id)?;
        if task.status != TaskStatus::Validating {
            return Err(TaskError::Conflict(t!(
                "task {id} is {status}: only a task awaiting validation can be rejected",
                id = &task.id,
                status = task.status.label()
            )));
        }
        let feedback = feedback.trim();
        if feedback.is_empty() {
            return Err(TaskError::Invalid(t!("the reason is empty: say what did not work in the test").into()));
        }
        if !self.tab_delivery() {
            return Err(TaskError::Conflict(t!("tasks run in the agents' tabs: turn on \"Open agent tabs\" in Settings").into()));
        }
        let feedback = cap_text(feedback, MAX_TEXT);
        let aligned = {
            let repo = self.task_repo(project)?;
            let _git = self.inner.git.op.lock();
            self.restore_worktree(&repo, &task)?;
            align_with_base(&repo, &task)
        };
        let updated = self.update_task(project, &task.id, |t| {
            t.status = TaskStatus::Running;
            t.finished_ms = None;
            t.log(t!("rejected in validation; returned to the agent with the reason"));
            if let Err(e) = &aligned {
                t.log(t!(
                    "warning: could not align the branch {branch} with {base}: {error}",
                    branch = &t.branch,
                    base = &t.base,
                    error = e
                ));
            }
            // `before` does not have the new feedback: the message shows only the earlier ones.
            let before = t.clone();
            t.add_feedback("validation", &feedback);
            (before, t.clone())
        });
        let (before, task) = updated.ok_or_else(|| TaskError::NotFound(t!("task {id} vanished", id)))?;
        // `Validating` did not hold the scope: if another task took that space in the meantime, only warn.
        if let Some(other) = queue_holder(&self.tasks(project), &task) {
            self.update_task(project, &task.id, |t| {
                t.log(t!("warning: the scope overlaps that of {other}, which is in progress; the next merge may conflict", other = &other))
            });
        }
        let ident = self.open_task_instance(project, &task);
        let prompt = reject_message(&before, &feedback, aligned.is_ok(), &self.feedback_learnings(&before, &feedback));
        let boot = self.task_boot_prompt(project, &task, "feedback", "failed validation, with the reason in the file", &prompt);
        self.deliver_to_task(project, &task, ident, prompt, boot, t!("rejection reason delivered to the agent"));
        Ok(task)
    }

    /// Ensures the worktree folder: if it was deleted by hand, recreates it from the task's branch (or from the base,
    /// if the branch also vanished). Only touches `<root>/.agentdeck/worktrees`.
    fn restore_worktree(&self, repo: &Repo, task: &Task) -> Result<(), TaskError> {
        let expected = util::absolutize(&repo.root.join(AGENTDECK_DIR).join("worktrees"));
        if !valid_task_branch(&task.branch) || !crate::git::valid_branch_name(&task.base) || !safe_worktree(&task.worktree, &expected) {
            return Err(TaskError::Failed(t!("data of task {id} outside what was expected; I touched nothing", id = &task.id)));
        }
        if task.worktree.is_dir() {
            return Ok(());
        }
        let _ = repo.write(&["worktree", "prune"], None);
        let path = task.worktree.to_string_lossy().into_owned();
        let args: Vec<&str> = if branch_exists(repo, &task.branch) {
            vec!["worktree", "add", &path, &task.branch]
        } else {
            vec!["worktree", "add", "-b", &task.branch, &path, &task.base]
        };
        run(repo.cmd(&args), None, COMMIT_TIMEOUT)
            .map(|_| ())
            .map_err(|e| TaskError::Failed(t!("the worktree folder vanished and I could not recreate it: {error}", error = e)))
    }

    // ---------- Discard ----------

    /// Discards the task (any state that is not final): closes the tab, removes the worktree (by force, even if the
    /// folder already vanished) and the branch. Works without the repository: then only the record changes. In
    /// `Validating`, what was already merged stays in the base: discarding does not undo the merge.
    pub fn task_discard(&self, project: &str, id: &str) -> Result<Task, TaskError> {
        let _op = self.inner.tasks.op.lock();
        let task = self.task_or_404(project, id)?;
        if task.status.is_terminal() {
            return Err(TaskError::Conflict(t!("task {id} is already {status}", id = &task.id, status = task.status.label())));
        }
        let was_validating = task.status == TaskStatus::Validating;
        let mut warnings = Vec::new();
        if task.started_ms.is_some() {
            self.close_task_instance(project, &task);
        }
        match self.task_repo(project) {
            Ok(repo) => {
                let _git = self.inner.git.op.lock();
                warnings.extend(self.remove_task_worktree(&repo, &task));
                if valid_task_branch(&task.branch) {
                    if let Err(e) = delete_branch(&repo, &task.branch, true) {
                        warnings.push(t!("the branch {branch} remained: {error}", branch = &task.branch, error = e));
                    }
                }
            }
            Err(e) => warnings.push(t!("repository unavailable ({error}): only the task record changed", error = e)),
        }
        let updated = self.update_task(project, &task.id, |t| {
            t.status = TaskStatus::Discarded;
            t.finished_ms = Some(now_ms());
            if was_validating {
                t.log(t!("discarded; worktree and branch removed (what was already merged stays in {base})", base = &t.base));
            } else {
                t.log(t!("discarded; worktree and branch removed"));
            }
            for w in &warnings {
                t.log(t!("warning: {warning}", warning = w));
            }
            t.clone()
        });
        let task = updated.ok_or_else(|| TaskError::NotFound(t!("task {id} vanished", id)))?;
        self.emit(CoreEvent::GitChanged);
        self.feed(FeedEntry::system(t!("Task {id} discarded: {title}", id = &task.id, title = &task.title)).in_project(project));
        self.advance_queue_locked(project);
        Ok(task)
    }

    /// Forgets the task's instance in the core and tells the interface to close its tab.
    fn close_task_instance(&self, project: &str, task: &Task) {
        self.close_instance(project, &task.instance);
        self.emit(CoreEvent::CloseInstance { project: project.to_owned(), id: task.instance.clone() });
    }

    /// Removes the task's worktree and its marks in git. Only touches the folder if it really is in
    /// `<root>/.agentdeck/worktrees` (the JSON may have been edited). Retries for a few seconds: on Windows the
    /// agent's terminal still holds the folder right after the tab closes. Returns the warnings.
    pub(super) fn remove_task_worktree(&self, repo: &Repo, task: &Task) -> Vec<String> {
        let mut warnings = Vec::new();
        let allowed = util::absolutize(&repo.root.join(AGENTDECK_DIR).join("worktrees"));
        if safe_worktree(&task.worktree, &allowed) {
            let dir = util::absolutize(&task.worktree);
            let path = dir.to_string_lossy().into_owned();
            let mut last = String::new();
            for attempt in 0..REMOVE_TRIES {
                if !dir.exists() {
                    break;
                }
                match repo.write(&["worktree", "remove", "--force", &path], None) {
                    Ok(_) => {}
                    Err(e) => last = e.to_string(),
                }
                if !dir.exists() {
                    break;
                }
                if attempt + 1 < REMOVE_TRIES {
                    std::thread::sleep(REMOVE_WAIT);
                }
            }
            if dir.exists() {
                // Git gave up (or removed only the record): delete the folder, which is ours.
                if let Err(e) = std::fs::remove_dir_all(&dir) {
                    // What was left and who holds it: without this the cause cannot be found (seen on `t14`).
                    let left = crate::file_lock::leftovers(&dir).describe(&dir);
                    let git = if last.is_empty() { String::new() } else { format!("{last}; ") };
                    log::warn!("task {}: the worktree {} did not go away ({git}{e}); {left}", task.id, dir.display());
                    warnings.push(t!("the folder {folder} remained ({git}{error}); {left}", folder = dir.display(), git, error = e, left));
                }
            }
            if !dir.exists() {
                crate::trust::forget(&dir, &self.inner.home);
            }
        } else {
            warnings.push(t!("the folder {folder} is not in .agentdeck/worktrees: I did not touch it", folder = task.worktree.display()));
        }
        // Remove from git the worktrees whose folder no longer exists (including the one that just vanished).
        if let Err(e) = repo.write(&["worktree", "prune"], None) {
            warnings.push(t!("git worktree prune failed: {error}", error = e));
        }
        warnings
    }
}
