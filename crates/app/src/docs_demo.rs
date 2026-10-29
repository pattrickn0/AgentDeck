//! DEBUG ONLY: fake documents for Documents mode (`AGENTDECK_DEBUG_UI=docs-demo…`), for screenshots without git,
//! without agents and without real files.
//!
//! - `docs-demo`: the diff of a task with four files, in focus;
//! - `docs-demo-file`, `docs-demo-md`, `docs-demo-report`, `docs-demo-unreadable`: the document in focus;
//! - `docs-demo-edit` (editing, with an unsaved change), `docs-demo-find` (search open in the file), `docs-demo-diff-find`
//!   (search in the diff), `docs-demo-external` (the disk changed with an unsaved edit), `docs-demo-confirm` (the question
//!   when closing the tab), `docs-demo-large` (large file, read only), `docs-demo-md-code` (markdown as code);
//! - `docs-demo-many`: ten documents (the tab strip scrolls);
//! - `docs-demo-badge`: the agents in view, with the switch badge (two tabs finished, one is working).

use agentdeck_core::git::parse_diff;

use crate::deck::Deck;
use crate::docs::{Badge, CenterView, DiffDoc, DocKind, DocState, MdSource, Source, TextDoc, Unsaved};
use crate::highlight::Lang;

const DIFF: &str = "diff --git a/crates/app/src/docs.rs b/crates/app/src/docs.rs
new file mode 100644
index 0000000..a1b2c3d
--- /dev/null
+++ b/crates/app/src/docs.rs
@@ -0,0 +1,9 @@
+//! Documents mode of the center: the state and the pure logic.
+
+use std::collections::HashMap;
+
+/// What the center shows.
+pub enum CenterView {
+    Agents,
+    Docs,
+}
diff --git a/crates/app/src/ui/tasks.rs b/crates/app/src/ui/tasks.rs
index 4d5e6f7..8a9b0c1 100644
--- a/crates/app/src/ui/tasks.rs
+++ b/crates/app/src/ui/tasks.rs
@@ -62,7 +62,7 @@ pub fn draw(deck: &mut Deck, ui: &mut Ui, rect: Rect) {
     match deck.tasks.view.clone() {
         TasksView::List => list_view(deck, ui, rect, now, &mut acts),
         TasksView::Detail(id) => match deck.tasks.task(&id).cloned() {
-            Some(task) if deck.tasks.diff.as_ref().is_some_and(|d| d.id == id) => diff_screen(deck, ui, rect, &task, &mut acts),
             Some(task) => detail_view(deck, ui, rect, &task, now, &mut acts),
             None => list_view(deck, ui, rect, now, &mut acts),
         },
@@ -98,7 +97,7 @@ fn apply(deck: &mut Deck, act: Act) {
         Act::Task(id, act) => deck.tasks_act(&id, act),
-        Act::OpenDiff(id) => deck.tasks_open_diff(&id),
+        Act::OpenDiff(id) => deck.docs_open_task_diff(&id),
         Act::DismissNotice => deck.tasks.notice = None,
         Act::ConfirmYes => deck.tasks_confirm_accepted(),
         Act::ConfirmNo => deck.tasks.confirm = None,
diff --git a/crates/app/src/tasks_ui.rs b/crates/app/src/tasks_ui.rs
index 1111111..2222222 100644
--- a/crates/app/src/tasks_ui.rs
+++ b/crates/app/src/tasks_ui.rs
@@ -410,12 +410,6 @@ pub enum Confirm {
     Discard(String),
 }

-/// Full diff opened in place of the detail.
-#[derive(Debug, Clone)]
-pub struct OpenDiff {
-    pub id: String,
-    pub diff: Option<Result<GitDiff, String>>,
-}
-
 /// A lifecycle operation in progress.
 #[derive(Debug, Clone, PartialEq, Eq)]
 pub struct Busy {
diff --git a/memory/backlog.md b/memory/backlog.md
index 9f8e7d6..5c4b3a2 100644
--- a/memory/backlog.md
+++ b/memory/backlog.md
@@ -12,3 +12,4 @@
 - Task desk: review without dead ends.
-- Brief and report show up as plain text.
+- File editor: phase 2 of Documents mode.
+- Side-by-side diff.
 - Terminal: duplicated history in Claude Code.
";

const FILE: &str = "//! Reads a project's text file, only to show it on screen.

use std::path::Path;

/// The limit of what is read: above it the file does not become text.
pub const READ_TEXT_MAX: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextFile {
    Text(String),
    /// Larger than the limit (size in bytes).
    TooLarge(u64),
    /// Has null bytes: it is not text.
    Binary,
}

pub fn read_text(workspace: &Path, rel: &str) -> Result<TextFile, String> {
    if !crate::git::safe_rel(rel) {
        return Err(format!(\"invalid path: {rel}\"));
    }
    let full = std::fs::canonicalize(workspace.join(rel)).map_err(|e| format!(\"could not open {rel}: {e}\"))?;
    let meta = std::fs::metadata(&full).map_err(|e| format!(\"could not read {rel}: {e}\"))?;
    if meta.len() > READ_TEXT_MAX {
        return Ok(TextFile::TooLarge(meta.len()));
    }
    let bytes = std::fs::read(&full).map_err(|e| format!(\"could not read {rel}: {e}\"))?;
    if bytes.contains(&0) {
        return Ok(TextFile::Binary);
    }
    Ok(TextFile::Text(String::from_utf8_lossy(&bytes).into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_outside_the_workspace_is_refused() {
        assert!(read_text(Path::new(\".\"), \"../secret.txt\").is_err());
    }
}
";

const REPORT: &str = "# t19 report

Documents in the center: an **Agents | Documents** switch to read diff, report and files with the full width.

## What was done

- the switch in the title bar, with the badge on *Agents*;
- the diff of a task, of a commit and of a file, with the file list on the left;
- the file view (`Core::read_text`, 1 MB limit);
- the formatted markdown (brief, report and the explorer's `.md` files).

## Checked

- [x] `cargo fmt --all`
- [x] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] manual test on Windows with the real agents

## Files

| File | What changed |
| :-- | :-- |
| `crates/app/src/docs.rs` | state and pure logic |
| `crates/app/src/ui/docs.rs` | the painting |
| `crates/core/src/workspace.rs` | `read_text` |

> The terminals keep running hidden; the PTY is **not** resized.

```rust
pub fn toggle(&mut self, project: &str) {
    self.view = match self.view {
        CenterView::Docs => CenterView::Agents,
        CenterView::Agents if self.count(project) > 0 => CenterView::Docs,
        CenterView::Agents => CenterView::Agents,
    };
}
```

1. Open a diff from the desk.
2. Click a file in the list.
3. Go back to the agents with **Ctrl+Shift+D**.
";

const README: &str = "# Maintenance guide

This guide is for whoever is going to maintain AgentDeck, alone or together with AI agents.

## Everyday commands

```bash
cargo run -p agentdeck
cargo test --workspace
```

See also [OVERVIEW.md](OVERVIEW.md).
";

/// Applies the screenshot state `item` (one of the `docs-demo*`).
pub fn apply(deck: &mut Deck, item: &str) {
    deck.docs.debug_hold = true;
    let project = deck.active_project.clone();
    let open = |deck: &mut Deck, kind: DocKind, title: &str, subtitle: &str, state: DocState| {
        deck.docs.open(&project, kind, title.to_owned(), subtitle.to_owned(), state);
    };
    open(
        deck,
        DocKind::Markdown(MdSource::TaskReport("t19".into())),
        "t19 · report",
        "t19 · report: Documents in the center: Agents | Documents switch",
        DocState::Markdown(REPORT.into()),
    );
    open(
        deck,
        DocKind::Markdown(MdSource::File("memory/MAINTENANCE.md".into())),
        "MAINTENANCE.md",
        "memory/MAINTENANCE.md",
        DocState::Source(Source::open(README, Lang::Markdown).expect("fits in the editor")),
    );
    let file_state = if item == "docs-demo-large" {
        DocState::Text(TextDoc::highlighted(FILE, Lang::Rust))
    } else {
        DocState::Source(Source::open(FILE, Lang::Rust).expect("fits in the editor"))
    };
    open(deck, DocKind::File("crates/core/src/workspace.rs".into()), "workspace.rs", "crates/core/src/workspace.rs", file_state);
    open(
        deck,
        DocKind::TaskDiff { id: "t19".into(), merged: false },
        "t19 · diff",
        "t19 · changes against dev",
        DocState::Diff(DiffDoc::new(parse_diff(DIFF, 5000))),
    );
    if item == "docs-demo-many" {
        for n in 1..=9 {
            let path = format!("crates/app/src/ui/file_{n}.rs");
            let state = DocState::Source(Source::open(FILE, Lang::Rust).expect("fits in the editor"));
            open(deck, DocKind::File(path.clone()), &format!("file_{n}.rs"), &path, state);
        }
    }
    if item == "docs-demo-unreadable" {
        open(
            deck,
            DocKind::File("whisper/models/ggml-base.bin".into()),
            "ggml-base.bin",
            "whisper/models/ggml-base.bin",
            DocState::Unreadable(crate::docs::too_large_message(147_955_000)),
        );
    }
    // The focus: each variant looks at one document.
    let focus = match item {
        "docs-demo-file" | "docs-demo-many" | "docs-demo-edit" | "docs-demo-find" | "docs-demo-external" | "docs-demo-confirm"
        | "docs-demo-large" => DocKind::File("crates/core/src/workspace.rs".into()),
        "docs-demo-md" | "docs-demo-md-code" => DocKind::Markdown(MdSource::File("memory/MAINTENANCE.md".into())),
        "docs-demo-report" => DocKind::Markdown(MdSource::TaskReport("t19".into())),
        "docs-demo-unreadable" => DocKind::File("whisper/models/ggml-base.bin".into()),
        _ => DocKind::TaskDiff { id: "t19".into(), merged: false },
    };
    open(deck, focus, "", "", DocState::Loading);
    if item == "docs-demo-badge" {
        deck.docs.view = CenterView::Agents;
        deck.docs.debug_badge = Some(Badge { done: 2, busy: 1 });
    }
    let uid = deck.docs.current_uid(&project);
    let source = |deck: &mut Deck, f: &mut dyn FnMut(&mut Source)| {
        if let Some(DocState::Source(src)) = uid.and_then(|u| deck.docs.get_mut(u)).map(|t| &mut t.state) {
            f(src);
        }
    };
    match item {
        "docs-demo-edit" | "docs-demo-confirm" | "docs-demo-external" => source(deck, &mut |src| {
            src.set_editing(true);
            src.text = src.text.replacen("Ok(TextFile::Text(", "Ok(TextFile::Utf8(", 1) + "\n// new note, not saved yet\n";
            src.touched();
            if item == "docs-demo-external" {
                src.external = Some(src.disk.replacen("READ_TEXT_MAX", "READ_TEXT_LIMIT", 1));
            }
        }),
        "docs-demo-find" | "docs-demo-diff-find" => {
            if item == "docs-demo-find" {
                // A long file, with the hit at the end: the search scrolls to it.
                source(deck, &mut |src| {
                    src.text = format!("{}// end of file\n", FILE.repeat(6));
                    src.disk = src.text.clone();
                    src.touched();
                });
            }
            if let Some(tab) = uid.and_then(|u| deck.docs.get_mut(u)) {
                tab.find.show();
                tab.find.query = if item == "docs-demo-find" { "end of file".into() } else { "doc".into() };
            }
        }
        "docs-demo-md-code" => source(deck, &mut |src| src.raw = true),
        _ => {}
    }
    if item == "docs-demo-confirm" {
        deck.docs.confirm = uid.map(Unsaved::Tab);
    }
}
