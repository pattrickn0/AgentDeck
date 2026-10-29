use std::collections::HashMap;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::{git_error, parse_task_id, Task, TaskError, TaskStatus};
use crate::events::{CoreEvent, FeedEntry, FeedKind};
use crate::git::{GitError, Repo};
use crate::t;
use crate::util::now_ms;
use crate::Core;

// ---------- Storage ----------

/// A project's tasks as they are on disk.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ProjectTasks {
    pub(super) version: u32,
    /// Next number to use (never decreases).
    pub(super) next: u64,
    pub(super) tasks: Vec<Task>,
}

impl ProjectTasks {
    pub(super) fn task_mut(&mut self, id: &str) -> Option<&mut Task> {
        self.tasks.iter_mut().find(|t| t.id == id)
    }

    /// The counter never falls below the highest number already used (hand-edited or old file).
    fn fix_counter(&mut self) {
        let max = self.tasks.iter().filter_map(|t| parse_task_id(&t.id)).filter_map(|id| id[1..].parse::<u64>().ok()).max().unwrap_or(0);
        self.next = self.next.max(max + 1).max(1);
        self.version = 1;
    }
}

/// State of the tasks in the core: the projects already read and the lifecycle operations lock.
#[derive(Default)]
pub(crate) struct TaskStore {
    pub(super) projects: Mutex<HashMap<String, ProjectTasks>>,
    /// One lifecycle operation at a time (create, start, merge…): each one may run git.
    pub(super) op: Mutex<()>,
    /// Initial prompt waiting for the process to open (`project/instance` → line), see `Core::set_boot_prompt`.
    pub(super) boot: Mutex<HashMap<String, String>>,
}

fn load_file(path: &Path) -> ProjectTasks {
    let mut tasks = match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str::<ProjectTasks>(&text).unwrap_or_else(|e| {
            // Unreadable: keep a copy before the next write puts an empty list on top.
            let backup = path.with_extension(format!("json.unreadable-{}", now_ms()));
            match std::fs::copy(path, &backup) {
                Ok(_) => log::error!("unreadable tasks in {} ({e}); copy kept in {}", path.display(), backup.display()),
                Err(e2) => log::error!("unreadable tasks in {} ({e}; and the copy failed: {e2})", path.display()),
            }
            ProjectTasks::default()
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ProjectTasks::default(),
        Err(e) => {
            log::error!("could not read the tasks in {}: {e}", path.display());
            ProjectTasks::default()
        }
    };
    tasks.fix_counter();
    tasks
}

/// Writes beside and swaps: a crash in the middle does not leave the file half written.
fn save_file(path: &Path, tasks: &ProjectTasks) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(tasks).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// Project id that works as a file name (`p` + hexadecimal, in practice).
fn safe_project_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

impl Core {
    fn tasks_path(&self, project: &str) -> Option<PathBuf> {
        safe_project_id(project).then(|| self.paths().data_dir.join("tasks").join(format!("{project}.json")))
    }

    /// Applies `f` to the project's tasks (read from disk the first time). With `save`, writes and tells the
    /// interface (`TasksChanged`) at the end.
    pub(super) fn with_tasks<R>(&self, project: &str, save: bool, f: impl FnOnce(&mut ProjectTasks) -> R) -> R {
        let path = self.tasks_path(project);
        let out = {
            let mut all = self.inner.tasks.projects.lock();
            let Some(path) = &path else {
                // Strange id (not a file name): nothing is read or written.
                return f(&mut ProjectTasks::default());
            };
            let entry = all.entry(project.to_owned()).or_insert_with(|| load_file(path));
            let out = f(entry);
            if save {
                if let Err(e) = save_file(path, entry) {
                    log::warn!("could not save the tasks in {}: {e}", path.display());
                }
            }
            out
        };
        if save {
            self.emit(CoreEvent::TasksChanged { project: project.to_owned() });
        }
        out
    }

    /// The project's tasks, oldest to newest.
    pub fn tasks(&self, project: &str) -> Vec<Task> {
        self.with_tasks(project, false, |p| p.tasks.clone())
    }

    /// A project task (`t3`, `T3` or `3`).
    pub fn task(&self, project: &str, id: &str) -> Option<Task> {
        let id = parse_task_id(id)?;
        self.with_tasks(project, false, |p| p.tasks.iter().find(|t| t.id == id).cloned())
    }

    /// Stores the feed entry on the timeline of each project task it belongs to (see
    /// [`Task::owns_message`]): a question between two tasks shows up in both. Only writes the file if there is an owner.
    pub(crate) fn record_task_message(&self, entry: &FeedEntry) {
        let Some(project) = entry.project.as_deref() else { return };
        if !matches!(entry.kind, FeedKind::Ask | FeedKind::Answer | FeedKind::Error) {
            return;
        }
        let owners: Vec<String> =
            self.with_tasks(project, false, |p| p.tasks.iter().filter(|t| t.owns_message(entry)).map(|t| t.id.clone()).collect());
        if owners.is_empty() {
            return;
        }
        self.with_tasks(project, true, |p| {
            for id in &owners {
                if let Some(t) = p.task_mut(id) {
                    t.record_message(entry);
                }
            }
        });
    }

    /// The task whose agent is the instance `instance` (and which has not ended yet), if any.
    pub fn task_of_instance(&self, project: &str, instance: &str) -> Option<Task> {
        self.with_tasks(project, false, |p| {
            p.tasks.iter().find(|t| t.instance == instance && !t.status.is_terminal() && t.status != TaskStatus::Draft).cloned()
        })
    }

    /// The folder the instance's terminal should open in: the worktree of the task in progress, in review or
    /// awaiting validation. A `Failed` task that already delivered the report (the merge failed) also
    /// stays in the worktree: reopening the app must not drop the agent into the main repository.
    pub(crate) fn task_cwd(&self, project: &str, instance: &str) -> Option<PathBuf> {
        self.with_tasks(project, false, |p| {
            p.tasks
                .iter()
                .find(|t| {
                    t.instance == instance
                        && match t.status {
                            TaskStatus::Running | TaskStatus::Review | TaskStatus::Validating => true,
                            TaskStatus::Failed => t.report.is_some(),
                            _ => false,
                        }
                })
                .map(|t| t.worktree.clone())
        })
    }

    pub(super) fn task_or_404(&self, project: &str, id: &str) -> Result<Task, TaskError> {
        self.task(project, id).ok_or_else(|| TaskError::NotFound(t!("task \"{id}\" not found in this project", id = id.escape_debug())))
    }

    /// Changes an existing task and saves. `None` if it vanished.
    pub(super) fn update_task<R>(&self, project: &str, id: &str, f: impl FnOnce(&mut Task) -> R) -> Option<R> {
        self.with_tasks(project, true, |p| p.task_mut(id).map(f))
    }

    /// Marks the task as `Failed` with the message and returns the error to the caller.
    pub(super) fn fail_task(&self, project: &str, id: &str, message: String) -> TaskError {
        self.update_task(project, id, |t| {
            t.status = TaskStatus::Failed;
            t.error = Some(message.clone());
            t.log(t!("error: {message}", message = &message));
        });
        TaskError::Failed(message)
    }

    /// The merge failed for an external reason (not a conflict): the error goes to the history and to the caller, and the
    /// task stays (or goes back) in `Review`, where it can still be merged again, returned or discarded. `Failed` here
    /// would be a dead end: the desk refused to return and complete.
    pub(super) fn fail_merge(&self, project: &str, id: &str, message: String) -> TaskError {
        self.update_task(project, id, |t| {
            t.status = TaskStatus::Review;
            t.error = None;
            t.log(t!("error: {message}", message = &message));
        });
        TaskError::Failed(message)
    }

    // ---------- Git ----------

    /// The project's repository (its root, even if the project is a subfolder).
    pub(super) fn task_repo(&self, project: &str) -> Result<Repo, TaskError> {
        let path = self.project_path(project).ok_or_else(|| TaskError::NotFound(t!("unknown project: {project}", project)))?;
        self.git_repo_at(&path).map_err(|e| match e {
            GitError::NotARepo => TaskError::Invalid(t!("the project is not a git repository; tasks need one (run git init)").into()),
            other => git_error(other),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{entry, sample};
    use super::*;

    #[test]
    fn the_timeline_round_trips_through_the_file_and_old_files_still_load() {
        let tmp = tempfile::tempdir().expect("tmp");
        let file = tmp.path().join("p1.json");
        let mut t = Task { instance: "claude@t3".into(), ..sample() };
        t.record_message(&entry(FeedKind::Ask, Some("claude@t3"), Some("antigravity"), "question"));
        save_file(&file, &ProjectTasks { next: 4, tasks: vec![t.clone()], ..ProjectTasks::default() }).expect("save");
        assert_eq!(load_file(&file).tasks[0].messages, t.messages);
        // A file from before the timeline (without `messages`) opens with it empty.
        std::fs::write(&file, r#"{"version":1,"next":4,"tasks":[{"id":"t3","instance":"claude@t3"}]}"#).expect("write");
        assert!(load_file(&file).tasks[0].messages.is_empty());
    }

    #[test]
    fn the_counter_never_goes_back() {
        let mut p = ProjectTasks { next: 2, tasks: vec![Task { id: "t7".into(), ..Task::default() }], ..ProjectTasks::default() };
        p.fix_counter();
        assert_eq!(p.next, 8);
        let mut p = ProjectTasks { next: 40, ..ProjectTasks::default() };
        p.fix_counter();
        assert_eq!(p.next, 40);
        let mut p = ProjectTasks::default();
        p.fix_counter();
        assert_eq!(p.next, 1);
    }

    #[test]
    fn the_file_round_trips_and_an_unreadable_one_is_kept() {
        let tmp = tempfile::tempdir().expect("tmp");
        let file = tmp.path().join("tasks").join("p1.json");
        assert!(load_file(&file).tasks.is_empty());
        let mut p = ProjectTasks { next: 5, tasks: vec![sample()], ..ProjectTasks::default() };
        p.fix_counter();
        save_file(&file, &p).expect("save");
        save_file(&file, &p).expect("save again on top");
        let back = load_file(&file);
        assert_eq!((back.next, back.tasks.len()), (5, 1));
        assert_eq!(back.tasks[0], sample());
        let text = std::fs::read_to_string(&file).expect("read");
        assert!(text.contains("\"status\": \"draft\"") && text.contains("\"created_ms\""), "{text}");
        assert!(!file.with_extension("json.tmp").exists());
        // Unreadable: comes back empty, but the original is kept beside it.
        std::fs::write(&file, "{ this is not json").expect("write");
        assert!(load_file(&file).tasks.is_empty());
        let kept = std::fs::read_dir(file.parent().expect("folder"))
            .expect("dir")
            .flatten()
            .any(|e| e.file_name().to_string_lossy().contains("unreadable"));
        assert!(kept, "the copy of the unreadable file should be kept");
    }

    #[test]
    fn project_ids_are_file_names() {
        assert!(safe_project_id("p1a2b3c4d"));
        for bad in ["", "../x", "a/b", "a\\b", "a b", "x.json"] {
            assert!(!safe_project_id(bad), "{bad}");
        }
    }
}
