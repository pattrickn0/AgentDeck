//! Short-term memory: the "current situation" of the project.
//!
//! It tells the agent what is happening in the project right now, so it does not step on anyone: the other tasks
//! in progress (with the files their branch already touched) and the latest commits of the base. It goes out two
//! ways: pushed at the start of a task (`task_situation`) and on demand by the `ask-agent --learn` of any tab
//! ([`crate::Core::learn`]), which also warns when a file the agent is about to touch is already touched by another
//! task ([`clash_warnings`]). Whoever builds the list of tasks with the files is [`Core::busy_tasks`]; the rest
//! are pure functions.

use crate::git::Repo;
use crate::tasks::{changed_files, union_files, worktree_dirty_unlocked};
use crate::{Core, TaskStatus};

/// Budget (in characters) of the whole section.
pub const BUDGET: usize = 1_500;
/// Other tasks cited, and files shown per task.
const MAX_TASKS: usize = 6;
const MAX_FILES: usize = 6;

/// A task in progress in the project (which is not the one starting).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtherTask {
    pub id: String,
    pub title: String,
    /// The instance of the agent (`claude@t12`).
    pub instance: String,
    /// The state, for people (as `TaskStatus::label` writes it).
    pub status: String,
    /// Files the branch already touched (`git diff --name-only base...branch`); empty = none so far.
    pub files: Vec<String>,
    /// Files the task's request cites and that are not yet in `files`: that is where the clash starts, before the
    /// first commit. Empty in `Validating` (it is already in the base).
    pub cited: Vec<String>,
}

fn clip(text: &str, max: usize) -> String {
    let (head, cut) = crate::util::truncate_chars(text, max);
    if cut {
        format!("{}…", head.trim_end())
    } else {
        head.to_owned()
    }
}

/// Files that do not count as a clash: the history merges by union (`.gitattributes`), so two tasks adding to it
/// do not conflict.
const MERGE_SAFE: &[&str] = &["memory/history.md"];

/// The latest commits of `base` (`git log --oneline`); empty if git fails.
pub(crate) fn base_commits(repo: &Repo, base: &str) -> Vec<String> {
    repo.read(&["log", "--oneline", "-8", base, "--"])
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().map(str::to_owned).collect())
        .unwrap_or_default()
}

impl Core {
    /// The project tasks that are touching the code right now (in progress, in review, waiting for validation),
    /// except the one of instance `except_instance`, each with the files it touches and the ones its request
    /// cites. Each task asks git for two commands (the branch and the worktree), so they run in parallel, one
    /// thread per task.
    pub(crate) fn busy_tasks(&self, project: &str, repo: &Repo, except_instance: &str) -> Vec<OtherTask> {
        let busy: Vec<_> = self
            .tasks(project)
            .into_iter()
            .filter(|t| {
                t.instance != except_instance && matches!(t.status, TaskStatus::Running | TaskStatus::Review | TaskStatus::Validating)
            })
            .collect();
        std::thread::scope(|scope| {
            let jobs: Vec<_> = busy
                .into_iter()
                .map(|t| {
                    scope.spawn(move || {
                        // `Validating` is already in the base (the branch was aligned): there is no diff to show.
                        // In progress, it joins what is already in the branch with what is not yet committed.
                        let (files, cited) = match t.status {
                            TaskStatus::Validating => (Vec::new(), Vec::new()),
                            _ => {
                                let files =
                                    union_files(changed_files(repo, &t.base, &t.branch), worktree_dirty_unlocked(repo, &t.worktree));
                                let brief = if t.brief.trim().is_empty() { t.title.as_str() } else { t.brief.as_str() };
                                let mut cited = crate::learnings::paths_in_text(brief, &t.worktree);
                                cited.retain(|c| !files.contains(c));
                                (files, cited)
                            }
                        };
                        OtherTask { id: t.id, title: t.title, instance: t.instance, status: t.status.label().to_owned(), files, cited }
                    })
                })
                .collect();
            jobs.into_iter().filter_map(|j| j.join().ok()).collect()
        })
    }
}

/// One warning per file of `paths` that another task of `others` also touches, saying where it came from
/// (`already touched`: it is in its branch or worktree; `cited by the request`: it did not touch it yet, but its
/// request cites the file). `memory/history.md` does not count. One per line; empty if there is no clash.
pub fn clash_warnings(others: &[OtherTask], paths: &[String]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for path in paths {
        if MERGE_SAFE.iter().any(|m| m.eq_ignore_ascii_case(path)) || lines.iter().any(|l| l.starts_with(&format!("{path}: "))) {
            continue;
        }
        let who: Vec<String> = others
            .iter()
            .filter_map(|t| {
                let origin = if t.files.iter().any(|f| f.eq_ignore_ascii_case(path)) {
                    "already touched"
                } else if t.cited.iter().any(|f| f.eq_ignore_ascii_case(path)) {
                    "cited by the request"
                } else {
                    return None;
                };
                Some(format!("{} ({}, {}; {origin})", t.id, t.instance, t.status))
            })
            .collect();
        if who.is_empty() {
            continue;
        }
        let also = if who.len() == 1 { "also touches it" } else { "also touch it" };
        lines.push(format!("{path}: {} {also}; changes to the same lines will conflict when merging", who.join(" and ")));
    }
    lines.join("\n")
}

/// The "Current situation" section. `own_paths` are the files the task's request cites: if another task touches
/// one of them, the line warns. `commits` are lines of `git log --oneline` of the base. It fits in `budget`
/// characters: what does not fit is cut from the end (the commits first).
pub fn render(others: &[OtherTask], commits: &[String], base: &str, own_paths: &[String], budget: usize) -> String {
    let mut lines: Vec<String> = Vec::new();
    if others.is_empty() {
        lines.push("Other tasks in progress: none.".to_owned());
    } else {
        lines.push("Other tasks in progress (avoid the same files):".to_owned());
        for t in others.iter().take(MAX_TASKS) {
            let mut line = format!("- {} \"{}\" ({}, {})", t.id, clip(&t.title, 60), t.instance, t.status);
            if t.files.is_empty() {
                line.push_str(": no files in the branch yet");
                if !t.cited.is_empty() {
                    let shown: Vec<&str> = t.cited.iter().take(MAX_FILES).map(String::as_str).collect();
                    line.push_str(&format!("; the request cites {}", shown.join(", ")));
                }
            } else {
                let shown: Vec<&str> = t.files.iter().take(MAX_FILES).map(String::as_str).collect();
                line.push_str(&format!(": touches {}", shown.join(", ")));
                if t.files.len() > MAX_FILES {
                    line.push_str(&format!(" and {} more", t.files.len() - MAX_FILES));
                }
                let clash: Vec<&str> = t.files.iter().chain(&t.cited).filter(|f| own_paths.contains(f)).map(String::as_str).collect();
                if !clash.is_empty() {
                    line.push_str(&format!(
                        ". WARNING: your request cites {}; coordinate with that task before touching it",
                        clash.join(", ")
                    ));
                }
            }
            lines.push(line);
        }
        if others.len() > MAX_TASKS {
            lines.push(format!("- … and {} more task(s)", others.len() - MAX_TASKS));
        }
    }
    if !commits.is_empty() {
        lines.push(format!("Latest commits on {base}:"));
        lines.extend(commits.iter().map(|c| format!("- {}", clip(c.trim(), 90))));
    }
    let mut out = String::new();
    for line in lines {
        let line = clip(&line, 400);
        if out.chars().count() + line.chars().count() + 2 > budget {
            out.push('…');
            break;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn other(id: &str, files: &[&str]) -> OtherTask {
        OtherTask {
            id: id.into(),
            title: format!("Title {id}"),
            instance: format!("claude@{id}"),
            status: TaskStatus::Running.label().into(),
            files: files.iter().map(|f| (*f).to_owned()).collect(),
            cited: Vec::new(),
        }
    }

    fn commits() -> Vec<String> {
        (0..8).map(|n| format!("abc{n:04} commit number {n} with a very long message to take up some space")).collect()
    }

    #[test]
    fn an_empty_project_still_lists_the_base_commits() {
        let out = render(&[], &["1fa59a7 feat: something".into()], "dev", &[], BUDGET);
        assert_eq!(out, "Other tasks in progress: none.\nLatest commits on dev:\n- 1fa59a7 feat: something");
    }

    #[test]
    fn other_tasks_show_their_files_and_flag_a_clash() {
        let out = render(
            &[other("t12", &["crates/app/src/ui/tabs.rs", "crates/core/src/tasks.rs"]), other("t14", &[])],
            &[],
            "dev",
            &["crates/core/src/tasks.rs".to_owned()],
            BUDGET,
        );
        let running = TaskStatus::Running.label();
        assert!(
            out.contains(&format!(
                "- t12 \"Title t12\" (claude@t12, {running}): touches crates/app/src/ui/tabs.rs, crates/core/src/tasks.rs"
            )),
            "{out}"
        );
        assert!(out.contains("WARNING: your request cites crates/core/src/tasks.rs"), "{out}");
        assert!(out.contains(&format!("- t14 \"Title t14\" (claude@t14, {running}): no files in the branch yet")), "{out}");
        assert!(!out.contains("Latest commits"), "{out}");
    }

    #[test]
    fn long_lists_are_summarised() {
        let files: Vec<String> = (0..10).map(|n| format!("f{n}.rs")).collect();
        let many: Vec<OtherTask> = (1..=9).map(|n| OtherTask { files: files.clone(), ..other(&format!("t{n}"), &[]) }).collect();
        let out = render(&many, &[], "dev", &[], 100_000);
        assert!(out.contains("f5.rs and 4 more"), "{out}");
        assert!(out.contains("- … and 3 more task(s)") && !out.contains("- t7 "), "{out}");
    }

    #[test]
    fn clashes_name_the_other_tasks_and_skip_the_history() {
        let a = other("t37", &["crates/app/src/app.rs", "memory/history.md"]);
        let b = OtherTask { cited: vec!["crates/app/src/app.rs".into(), "memory/history.md".into()], ..other("t40", &["x.rs"]) };
        let paths = ["crates/app/src/app.rs", "memory/history.md", "y.rs"].map(String::from);
        let out = clash_warnings(&[a.clone(), b], &paths);
        let running = TaskStatus::Running.label();
        assert_eq!(
            out,
            format!(
                "crates/app/src/app.rs: t37 (claude@t37, {running}; already touched) and t40 (claude@t40, {running}; cited by the request) also touch it; changes to the same lines will conflict when merging"
            )
        );
        let one = clash_warnings(&[a], &paths[..1]);
        assert!(one.contains(&format!("t37 (claude@t37, {running}; already touched) also touches it;")), "{one}");
        assert_eq!(clash_warnings(&[], &paths), "");
    }

    #[test]
    fn a_task_with_no_files_yet_shows_what_its_request_cites() {
        let t = OtherTask { cited: vec!["a.rs".into(), "b.rs".into()], ..other("t12", &[]) };
        let out = render(&[t], &[], "dev", &[], BUDGET);
        assert!(out.contains("no files in the branch yet; the request cites a.rs, b.rs"), "{out}");
    }

    #[test]
    fn the_budget_cuts_the_commits_first() {
        let tasks = [other("t12", &["a.rs"])];
        let out = render(&tasks, &commits(), "dev", &[], BUDGET);
        assert!(out.chars().count() <= BUDGET, "{}", out.chars().count());
        assert!(out.contains("- t12 ") && out.contains("Latest commits on dev:"), "{out}");
        // With little room, the commits disappear from the end, never the tasks.
        let tight = render(&tasks, &commits(), "dev", &[], 200);
        assert!(tight.chars().count() <= 200 && tight.contains("- t12 ") && tight.ends_with('…'), "{tight}");
        assert!(!tight.contains("abc0007"), "{tight}");
    }
}
