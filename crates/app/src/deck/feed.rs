//! Integrated conversation.

use super::*;

impl Deck {
    pub fn add_feed(&mut self, e: FeedEntry) {
        // Entries from another project go to its conversation (kept until it comes back to the screen);
        // the global ones (no project, e.g. Whisper) and the interface ones stay in the active project's.
        let elsewhere = e.project.clone().filter(|p| *p != self.active_project && !self.active_project.is_empty());
        let thinking_key = |from: &str| format!("{}#{from}", e.project.as_deref().unwrap_or(&self.active_project));
        let thinking_prev = e.from.as_deref().map(thinking_key).and_then(|k| self.thinking.remove(&k));
        let thinking_new = (e.kind == FeedKind::Thinking).then(|| e.from.as_deref().map(thinking_key)).flatten();
        let to = e.to.as_deref().map(|t| self.agents.who_name(t));
        let item = FeedItem {
            id: self.next_feed_id,
            kind: e.kind,
            who: e.from.as_ref().map(|f| self.agents.who_name(f)),
            to,
            color: self.agents.who_color(e.from.as_deref()),
            time: util::time_pt_br(e.ts),
            text: e.text.clone(),
        };
        self.next_feed_id += 1;
        if let Some(key) = thinking_new {
            self.thinking.insert(key, item.id);
        }
        if let Some(project) = elsewhere {
            let state = self.projects.entry(project).or_default();
            if let Some(id) = thinking_prev {
                state.feed.retain(|f| f.id != id);
            }
            state.feed.push(item);
            cap_feed(&mut state.feed);
            return;
        }
        if let Some(id) = thinking_prev {
            self.feed.retain(|f| f.id != id);
        }
        self.feed.push(item);
        cap_feed(&mut self.feed);
        if self.feed_near_bottom {
            self.feed_scroll_to_bottom = true;
        }
        if self.right_view != RightView::Chat && e.kind != FeedKind::Thinking {
            self.unseen += 1;
        }
    }

    /// Switches what the right bar shows (coming back to the Conversation clears the unseen count).
    pub fn set_right_view(&mut self, view: RightView) {
        let opening_git = view == RightView::Git && self.right_view != RightView::Git;
        let opening_github = view == RightView::GitHub && self.right_view != RightView::GitHub;
        let opening_tasks = view == RightView::Tasks && self.right_view != RightView::Tasks;
        self.right_view = view;
        if view == RightView::Chat {
            self.unseen = 0;
        }
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

    pub fn system(&mut self, text: impl Into<String>) {
        self.add_feed(FeedEntry { ts: 0, kind: FeedKind::System, from: None, to: None, text: text.into(), project: None });
    }

    /// Notice in the conversation of project `project` (which may not be the one on screen).
    pub(super) fn system_in(&mut self, project: &str, text: impl Into<String>) {
        self.add_feed(FeedEntry::new(FeedKind::System, None, None, text).in_project(project));
    }

    /// "Clear": only the conversation of the project on screen.
    pub fn clear_feed(&mut self) {
        self.feed.clear();
        let prefix = format!("{}#", self.active_project);
        self.thinking.retain(|k, _| !k.starts_with(&prefix));
    }

    pub(super) fn debug_feed(&mut self) {
        let mk = |kind, from: Option<&str>, to: Option<&str>, text: &str| FeedEntry::new(kind, from, to, text);
        self.add_feed(mk(FeedKind::User, Some("you"), Some("claude"), "Review @README.md and propose improvements."));
        self.add_feed(mk(FeedKind::Ask, Some("claude"), Some("antigravity"), "Can you check whether the Windows build still uses ConPTY?"));
        self.add_feed(mk(
            FeedKind::Answer,
            Some("antigravity"),
            Some("claude"),
            "Yes — portable-pty uses ConPTY on Windows 10+.\nNothing to change.",
        ));
        self.add_feed(mk(FeedKind::System, None, None, "Active project changed to: workspace"));
        self.add_feed(mk(FeedKind::Error, Some("opencode"), None, "Failed to reach the model provider."));
        self.add_feed(mk(FeedKind::Thinking, Some("claude"), None, "⚙️ [read_file] README.md"));
    }
}
