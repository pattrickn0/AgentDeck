//! System notices (toasts and the bell's history, see `notices.rs`) and the right bar's view.

use super::*;
use holt_core::Tone;

impl Deck {
    /// An entry of the core's feed: system notices and agent errors become notifications; questions and answers
    /// already live in the task's timeline.
    pub fn add_feed(&mut self, e: FeedEntry) {
        if let Some(text) = crate::notices::notice_text(&e) {
            let at = if e.ts == 0 { holt_core::util::now_ms() } else { e.ts };
            self.notices.push(e.tone, text, e.project, e.task, at, Instant::now());
        }
    }

    /// Switches what the right bar shows.
    pub fn set_right_view(&mut self, view: RightView) {
        let opening_git = view == RightView::Git && self.right_view != RightView::Git;
        let opening_github = view == RightView::GitHub && self.right_view != RightView::GitHub;
        let opening_tasks = view == RightView::Tasks && self.right_view != RightView::Tasks;
        self.right_view = view;
        if opening_git {
            self.git_refresh();
        }
        if opening_github {
            self.github_refresh();
        }
        if opening_tasks {
            self.tasks_on_open();
        }
    }

    /// A notice of the interface itself, in the active project.
    pub fn system(&mut self, text: impl Into<String>) {
        self.notify(Tone::Info, text);
    }

    /// A notice with its tone (`Warning` for what was refused or worked around, `Error` for what failed).
    pub fn notify(&mut self, tone: Tone, text: impl Into<String>) {
        let project = (!self.active_project.is_empty()).then(|| self.active_project.clone());
        let entry = FeedEntry { project, ..FeedEntry::system(text).tone(tone) };
        self.add_feed(entry);
    }

    /// Notice in project `project` (which may not be the one on screen).
    pub(super) fn system_in(&mut self, project: &str, text: impl Into<String>) {
        self.add_feed(FeedEntry::system(text).in_project(project));
    }

    /// The notice's task, in its project: switches project if needed and opens the task in the Tasks tab.
    pub fn open_notice_task(&mut self, id: u64) {
        let Some(n) = self.notices.get(id).cloned() else { return };
        self.notices.dismiss(id);
        let Some(task) = n.task else { return };
        if let Some(project) = n.project.filter(|p| *p != self.active_project) {
            self.switch_project(&project);
        }
        self.set_right_view(RightView::Tasks);
        self.tasks_open_detail(&task);
    }

    /// The display name of a notice's project, when it is not the one on screen (the history and the toast name it).
    pub fn notice_project_name(&self, project: Option<&str>) -> Option<String> {
        let id = project.filter(|p| *p != self.active_project)?;
        self.core.projects().into_iter().find(|p| p.id == id).map(|p| p.name)
    }

    pub(crate) fn debug_notices(&mut self) {
        let now = Instant::now();
        let at = holt_core::util::now_ms();
        let min = 60_000;
        let mut add = |tone, text: &str, project: Option<&str>, task: Option<&str>, ago: u64| {
            self.notices.push(tone, text.to_owned(), project.map(str::to_owned), task.map(str::to_owned), at - ago * min, now);
        };
        add(Tone::Info, "Opened the Claude tab for task t6: Document the bridge /tasks routes.", Some("p2"), Some("t6"), 120);
        add(Tone::Success, "Local transcription: large-v3-turbo model ready.", None, None, 60);
        add(Tone::Error, "opencode → claude@t4: No quota: the deepseek-v4.1-flash-free model used up the week.", None, None, 40);
        add(Tone::Info, "Task t4 ready for review: Tasks tab in the right bar", None, Some("t4"), 6);
        add(Tone::Error, "Could not transcribe: the microphone returned no audio.", None, None, 0);
    }
}
