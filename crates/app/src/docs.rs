//! Documents mode of the center: the state and the pure logic (the painting lives in `ui/docs.rs`).
//!
//! The center switches between the agents (the split screen) and the documents: diff of a task, of a commit or of a
//! file, text file and markdown. Small text files are read and **edited** (`Source`: `Ctrl+S` writes, with the same
//! line ending and without adding a BOM; the notice when an agent changes the file with an edit open). Documents are
//! per project and are not saved between openings. Opening a document that is already open only focuses it; opening any
//! document switches the center to Documents, and closing the last one goes back to Agents.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use holt_core::git::{DiffKind, EntryKind, GitDiff, GitEntry, GitError};
use holt_core::workspace::READ_TEXT_MAX;
use holt_core::{t, Eol, TextFile};

use crate::deck::{Deck, UiMsg};
use crate::highlight::{self, Lang, Span};

/// The font and the height of a line of the text file (the same as the diff's).
pub const TEXT_FONT: f32 = 12.0;
pub const TEXT_ROW_H: f32 = 17.0;

/// What the drawing of a file keeps between frames: the highlight per version of the text and the galley already
/// built. The `TextEdit` asks for the galley every frame; without this cache the whole file would be walked, colored and
/// measured again 60 times per second.
#[derive(Debug, Clone, Default)]
pub struct Shaped {
    pub hl: highlight::Cache,
    key: Option<ShapeKey>,
    galley: Option<Arc<egui::Galley>>,
}

/// What the saved galley is valid for: the text, the language, the wrap width, the screen scale and the search.
#[derive(Debug, Clone, PartialEq)]
struct ShapeKey {
    hash: u64,
    lang: Lang,
    wrap: u32,
    ppp: u32,
    marks: Option<(String, u64, usize)>,
}

impl Shaped {
    /// The galley of `text` (monospace, highlighted, wrapping at `wrap` px), rebuilt only if something in the key changed.
    /// `marks` are the search hits, in bytes of `text`, and `current` the current one among them.
    pub fn galley(&mut self, fonts: &egui::Context, text: &str, lang: Lang, wrap: f32, find: &Find) -> Arc<egui::Galley> {
        let hash = highlight::text_hash(text);
        let ppp = fonts.pixels_per_point();
        let marks = find.stamp().map(|(q, v)| (q.clone(), *v, find.current));
        let key = ShapeKey { hash, lang, wrap: wrap.to_bits(), ppp: ppp.to_bits(), marks };
        if let (Some(k), Some(g)) = (&self.key, &self.galley) {
            if *k == key {
                return g.clone();
            }
        }
        let pal = crate::theme::pal();
        let font = crate::theme::mono(TEXT_FONT);
        let matches: &[Range<usize>] = if key.marks.is_some() { &find.ranges } else { &[] };
        let pieces = highlight::pieces(text, self.hl.lines(lang, text, hash), matches, key.marks.as_ref().map(|_| find.current));
        let mut job = egui::text::LayoutJob::default();
        job.wrap.max_width = wrap;
        job.keep_trailing_whitespace = true;
        let format = |color, background| egui::TextFormat {
            font_id: font.clone(),
            color,
            background,
            line_height: Some(TEXT_ROW_H),
            ..Default::default()
        };
        for p in &pieces {
            let (color, background) = pal.syn_mark(p.tok, p.mark, pal.text);
            job.append(&text[p.range.clone()], 0.0, format(color, background));
        }
        if pieces.is_empty() {
            job.append("", 0.0, format(pal.text, egui::Color32::TRANSPARENT));
        }
        let galley = fonts.fonts_mut(|f| f.layout_job(job));
        (self.key, self.galley) = (Some(key), Some(galley.clone()));
        galley
    }
}

/// What the center shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CenterView {
    #[default]
    Agents,
    Docs,
}

/// Where the text of a markdown document comes from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MdSource {
    /// A `.md` of the project (relative path).
    File(String),
    /// The task brief (id): the text comes from the task already loaded.
    TaskBrief(String),
    /// The task report (id).
    TaskReport(String),
}

/// What a document is. Two documents of the same project with the same `DocKind` are the same.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DocKind {
    /// The diff of the task: the branch's (`merged` false) or, after the merge, what it brought. The two are
    /// different documents: a task that changes state does not go back to the old diff.
    TaskDiff {
        id: String,
        merged: bool,
    },
    /// The diff of a commit (full hash).
    CommitDiff(String),
    /// The diff of a changed file (path from the repository root), on the staged or the unstaged side; `orig` is the
    /// old name of a renamed one.
    FileDiff {
        path: String,
        orig: Option<String>,
        staged: bool,
        untracked: bool,
    },
    /// A text file of the project (relative path).
    File(String),
    Markdown(MdSource),
}

impl DocKind {
    /// The project file the document reads, if it is a file (it reloads when the file changes).
    pub fn file_path(&self) -> Option<&str> {
        match self {
            DocKind::File(p) | DocKind::Markdown(MdSource::File(p)) => Some(p),
            _ => None,
        }
    }

    /// The file of a single-file diff (for highlighting the diff code).
    pub fn diff_path(&self) -> Option<&str> {
        match self {
            DocKind::FileDiff { path, .. } => Some(path),
            _ => None,
        }
    }

    /// Can be read again (reload button): everything that comes from the core, not the text of a task.
    pub fn reloadable(&self) -> bool {
        !matches!(self, DocKind::Markdown(MdSource::TaskBrief(_) | MdSource::TaskReport(_)))
    }

    /// The read uses the **active** project at the moment the thread runs (file, commit, changed file): if the user
    /// switched projects in the meantime, the result is from another project. The task diff passes the project and has no such risk.
    pub fn reads_active_project(&self) -> bool {
        matches!(self, DocKind::CommitDiff(_) | DocKind::FileDiff { .. } | DocKind::File(_) | DocKind::Markdown(MdSource::File(_)))
    }
}

/// A file inside a diff (the list on the left).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffFile {
    pub path: String,
    /// `A` new, `D` deleted, `R` renamed, `M` modified.
    pub status: char,
    pub added: usize,
    pub removed: usize,
    /// Index, in `GitDiff::lines`, of this file's `diff --git` line (where the list scrolls to).
    pub line: usize,
}

#[derive(Debug, Clone)]
pub struct DiffDoc {
    pub diff: GitDiff,
    pub files: Vec<DiffFile>,
    /// The file selected in the list (the last one clicked).
    pub selected: usize,
    /// Line the diff scrolls to in the next frame (click on the list).
    pub jump: Option<usize>,
}

impl DiffDoc {
    pub fn new(diff: GitDiff) -> Self {
        let files = diff_files(&diff);
        Self { diff, files, selected: 0, jump: None }
    }

    /// Selects file `n` of the list and asks to scroll to it.
    pub fn select(&mut self, n: usize) {
        if let Some(f) = self.files.get(n) {
            self.selected = n;
            self.jump = Some(f.line);
        }
    }
}

/// The lines of a text file that does not open in the editor (too large, line too long).
#[derive(Debug, Clone)]
pub struct TextDoc {
    pub lines: Vec<String>,
    /// The longest line, in characters (the width of the horizontal scroll).
    pub max_chars: usize,
    /// The highlighting of each line (empty without a language); computed once, when the file loads.
    pub spans: Vec<Vec<Span>>,
}

impl PartialEq for TextDoc {
    fn eq(&self, other: &Self) -> bool {
        self.lines == other.lines
    }
}

/// A line with more characters than this is cut on screen (as the diff does with 1000): a 1 MB file on a single line
/// (minified JSON) must not freeze the painting.
const TEXT_LINE_MAX: usize = 2000;

impl TextDoc {
    pub fn new(text: &str) -> Self {
        let lines: Vec<String> = text
            .lines()
            .map(|l| {
                let l = l.replace('\t', "    ");
                if l.chars().count() > TEXT_LINE_MAX {
                    l.chars().take(TEXT_LINE_MAX).chain(['…']).collect()
                } else {
                    l
                }
            })
            .collect();
        let max_chars = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
        Self { lines, max_chars, spans: Vec::new() }
    }

    /// Like [`TextDoc::new`], with the highlighting of `lang` (only once, here: painting only reads it).
    pub fn highlighted(text: &str, lang: Lang) -> Self {
        let mut doc = Self::new(text);
        if lang != Lang::Plain {
            doc.spans = highlight::highlight_lines(lang, &doc.lines.join("\n"));
        }
        doc
    }
}

/// Above this the file opens read-only, without the editor: the editor drawing reshapes the letters of the whole text and,
/// with several MB, would freeze the interface.
pub const EDIT_MAX: usize = 256 * 1024;
/// A line longer than this (minified JSON) also keeps the editor out.
const EDIT_LINE_MAX: usize = 4000;

/// A text file opened as code: the text on screen (which is edited, if the user asks), what is on the
/// disk, and what is needed to write it back the same. It also serves reading: the same drawing gives selection, copy and search.
#[derive(Debug, Clone)]
pub struct Source {
    /// The text on screen: with `\n`, no BOM.
    pub text: String,
    /// What is on the disk, in the same format; `text != disk` is "unsaved edit".
    pub disk: String,
    pub eol: Eol,
    pub bom: bool,
    pub lang: Lang,
    /// The user is editing (otherwise, only reading).
    pub editing: bool,
    /// Markdown: the code instead of the formatted text.
    pub raw: bool,
    /// Why it cannot be edited: writing it back would change what the user did not touch.
    pub locked: Option<String>,
    /// The disk changed with an unsaved edit: the text from there, waiting for the choice between keeping mine or reloading.
    pub external: Option<String>,
    /// The BOM, the line ending and the lock of the `external` text: reloading it adopts all three (writing with the old
    /// file's ones would rewrite every line, or write over bytes that did not become text).
    external_form: Option<(bool, Eol, Option<String>)>,
    /// The file disappeared (or became unreadable) while there was an unsaved edit: the edit stays here.
    pub gone: bool,
    /// A save in progress, and the error of the last one that failed.
    pub saving: bool,
    pub save_error: Option<String>,
    /// Changes on every text change (the key of what is cached: highlighting, search).
    pub version: u64,
}

/// The text as the editor keeps it (no BOM, with `\n`), the BOM and the line ending; and why it cannot be edited, if it cannot.
fn read_source(raw: &str) -> (String, bool, Eol, Option<String>) {
    let (body, bom) = match raw.strip_prefix('\u{feff}') {
        Some(rest) => (rest, true),
        None => (raw, false),
    };
    let (text, eol, mut locked) = match Eol::normalize(body) {
        Some((text, eol)) => (text, eol, None),
        None => (
            body.replace("\r\n", "\n"),
            Eol::Lf,
            Some(
                t!("This file mixes line endings (CRLF and LF): writing it back would change lines you did not touch. Read-only.")
                    .to_owned(),
            ),
        ),
    };
    if locked.is_none() && body.contains('\u{fffd}') {
        locked = Some(t!("This file is not UTF-8 (some bytes do not become text): writing it back would damage it. Read-only.").to_owned());
    }
    (text, bom, eol, locked)
}

impl Source {
    /// The text fits in the editor: it does not go over `EDIT_MAX` and no line goes over `EDIT_LINE_MAX`.
    pub fn fits(raw: &str) -> bool {
        raw.len() <= EDIT_MAX && !raw.split('\n').any(|l| l.len() > EDIT_LINE_MAX)
    }

    /// The file as code, if it fits in the editor (`EDIT_MAX`, no line too long); otherwise `None`.
    pub fn open(raw: &str, lang: Lang) -> Option<Source> {
        if raw.len() > EDIT_MAX || raw.split('\n').any(|l| l.len() > EDIT_LINE_MAX) {
            return None;
        }
        let (text, bom, eol, locked) = read_source(raw);
        Some(Source {
            disk: text.clone(),
            text,
            eol,
            bom,
            lang,
            editing: false,
            raw: false,
            locked,
            external: None,
            external_form: None,
            gone: false,
            saving: false,
            save_error: None,
            version: 0,
        })
    }

    pub fn dirty(&self) -> bool {
        self.text != self.disk
    }

    /// Is `raw` (what the disk has now) what is already in `disk`?
    pub fn is_disk(&self, raw: &str) -> bool {
        self.external.is_none() && !self.gone && read_source(raw).0 == self.disk
    }

    /// The file was read again (the watcher said so). Without an unsaved edit, the screen follows the disk; with one, nothing
    /// the user typed is lost: if the disk really changed, the text from there waits in `external`.
    pub fn reload(&mut self, raw: &str) {
        let (text, bom, eol, locked) = read_source(raw);
        self.gone = false;
        if !self.dirty() {
            if text != self.text {
                self.version += 1;
            }
            (self.text, self.disk, self.bom, self.eol, self.locked) = (text.clone(), text, bom, eol, locked);
            self.external = None;
            self.external_form = None;
            return;
        }
        if text == self.disk {
            self.external = None;
            self.external_form = None;
        } else if text == self.text {
            // The agent wrote the same as the user: there is nothing to choose.
            self.disk = text;
            self.external = None;
            self.external_form = None;
        } else {
            self.external = Some(text);
            self.external_form = Some((bom, eol, locked));
        }
    }

    /// "Keep mine": the disk that changed becomes the base; the edit goes on and the next save writes it over.
    pub fn keep_mine(&mut self) {
        if let Some(ext) = self.external.take() {
            self.disk = ext;
        }
        self.external_form = None;
    }

    /// "Reload from disk": the edit is discarded.
    pub fn take_disk(&mut self) {
        if let Some(ext) = self.external.take() {
            self.text = ext.clone();
            self.disk = ext;
            self.version += 1;
            self.adopt_external_form();
        }
    }

    /// The `external` text became the one on screen: the BOM, the line ending and the lock become its own.
    fn adopt_external_form(&mut self) {
        if let Some((bom, eol, locked)) = self.external_form.take() {
            (self.bom, self.eol, self.locked) = (bom, eol, locked);
            if self.locked.is_some() {
                self.editing = false;
            }
        }
    }

    /// Starts editing (if allowed). Markdown goes to code while it is edited.
    pub fn set_editing(&mut self, on: bool) {
        if on && self.can_edit() {
            self.editing = true;
            self.raw = true;
        } else if !on {
            self.editing = false;
        }
    }

    /// Throws the edit away: the screen goes back to what is on the disk (or to the new text the disk brought).
    pub fn discard(&mut self) {
        if self.external.is_some() {
            self.adopt_external_form();
        }
        self.text = self.external.take().unwrap_or_else(|| self.disk.clone());
        self.disk = self.text.clone();
        self.version += 1;
        self.save_error = None;
    }

    /// The save of `saved` succeeded.
    pub fn mark_saved(&mut self, saved: &str) {
        self.disk = saved.to_owned();
        self.external = None;
        self.external_form = None;
        self.gone = false;
        self.saving = false;
        self.save_error = None;
    }

    /// The text was edited (the editor returned a change).
    pub fn touched(&mut self) {
        self.version += 1;
    }

    /// Can it be edited? (Not if `locked`.)
    pub fn can_edit(&self) -> bool {
        self.locked.is_none()
    }
}

/// The search of the open document (`Ctrl+F`).
#[derive(Debug, Clone, Default)]
pub struct Find {
    pub open: bool,
    pub query: String,
    /// The position, among the matches, of the current one.
    pub current: usize,
    /// The field asks for focus in the next frame.
    pub focus: bool,
    /// Scroll to the current match in the next frame.
    pub jump: bool,
    /// Matches in bytes of the text (file in the editor) or lines (diff, large file); only one of the two is used.
    pub ranges: Vec<Range<usize>>,
    pub lines: Vec<usize>,
    /// Which query and text version the matches are for.
    key: Option<(String, u64)>,
}

impl Find {
    pub fn show(&mut self) {
        self.open = true;
        self.focus = true;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.focus = false;
    }

    pub fn count(&self) -> usize {
        self.ranges.len() + self.lines.len()
    }

    /// Redoes the matches if the query or the text version changed (`search` returns bytes and lines). A new
    /// query goes to the first match; editing the text does not scroll the screen.
    pub fn refresh(&mut self, version: u64, search: impl FnOnce(&str) -> (Vec<Range<usize>>, Vec<usize>)) {
        let same_query = self.key.as_ref().is_some_and(|(q, _)| *q == self.query);
        if self.key.as_ref().is_some_and(|(q, v)| *q == self.query && *v == version) {
            return;
        }
        (self.ranges, self.lines) = search(&self.query);
        self.key = Some((self.query.clone(), version));
        if !same_query {
            self.current = 0;
            self.jump = true;
        } else if self.current >= self.count() {
            self.current = 0;
        }
    }

    /// The next (or the previous) match, wrapping around.
    pub fn step(&mut self, back: bool) {
        let n = self.count();
        if n > 0 {
            self.current = (self.current + if back { n - 1 } else { 1 }) % n;
            self.jump = true;
        }
    }

    /// Which query and text version the matches are from (the key of what depends on them, like the galley).
    pub fn stamp(&self) -> Option<&(String, u64)> {
        self.key.as_ref().filter(|_| self.open && !self.query.is_empty())
    }

    /// The current match, in bytes (file in the editor).
    pub fn current_range(&self) -> Option<&Range<usize>> {
        self.ranges.get(self.current)
    }

    /// The line of the current match (diff, large file).
    pub fn current_line(&self) -> Option<usize> {
        self.lines.get(self.current).copied()
    }
}

#[derive(Debug, Clone)]
pub enum DocState {
    Loading,
    Failed(String),
    Diff(DiffDoc),
    Text(TextDoc),
    /// A small text file, as code: reads, selects, searches and edits.
    Source(Source),
    /// The markdown text, drawn already formatted (the task's request and report).
    Markdown(String),
    /// A file that does not become text on screen (too large or binary): the explanation.
    Unreadable(String),
}

/// What came from a read done off the interface thread.
#[derive(Debug, Clone)]
pub enum Loaded {
    Diff(Result<GitDiff, String>),
    Text(Result<TextFile, String>),
}

#[derive(Debug, Clone)]
pub struct DocTab {
    pub uid: u64,
    /// Id of the project the document belongs to.
    pub project: String,
    pub kind: DocKind,
    /// The short name, on the tab.
    pub title: String,
    /// The top line of the document (path, task, commit) and the tab tooltip.
    pub subtitle: String,
    pub state: DocState,
    pub find: Find,
    pub shaped: Shaped,
}

impl DocTab {
    pub fn new(uid: u64, project: String, kind: DocKind, title: String, subtitle: String, state: DocState) -> Self {
        Self { uid, project, kind, title, subtitle, state, find: Find::default(), shaped: Shaped::default() }
    }

    /// There is an unsaved edit.
    pub fn dirty(&self) -> bool {
        matches!(&self.state, DocState::Source(s) if s.dirty())
    }

    /// The highlighting language, from the file name.
    pub fn lang(&self) -> Lang {
        self.kind.file_path().map_or(Lang::Plain, Lang::from_path)
    }

    /// Applies the result of a read. The state becomes what the document type shows; the selected file
    /// and the scroll of a reread diff are not lost (`selected` is kept only if it still exists). A file
    /// opened as code with an unsaved edit is never replaced by a read: the read only becomes a notice (`Source::reload`).
    pub fn apply(&mut self, loaded: Loaded) {
        if let DocState::Source(src) = &mut self.state {
            match &loaded {
                // A file that grew beyond the editor only follows the disk if there is no edit to protect.
                Loaded::Text(Ok(TextFile::Text(raw))) if src.dirty() || Source::fits(raw) => return src.reload(raw),
                Loaded::Text(_) if src.dirty() => {
                    src.gone = true;
                    return;
                }
                _ => {}
            }
        }
        let previous = match &self.state {
            DocState::Diff(d) => Some(d.selected),
            _ => None,
        };
        let lang = self.lang();
        let is_markdown = matches!(self.kind, DocKind::Markdown(_));
        let from_file = self.kind.file_path().is_some();
        self.state = match loaded {
            Loaded::Diff(Err(e)) | Loaded::Text(Err(e)) => DocState::Failed(e),
            Loaded::Diff(Ok(diff)) => {
                let mut doc = DiffDoc::new(diff);
                if let Some(n) = previous.filter(|n| *n < doc.files.len()) {
                    doc.selected = n;
                }
                DocState::Diff(doc)
            }
            Loaded::Text(Ok(TextFile::TooLarge(bytes))) => DocState::Unreadable(too_large_message(bytes)),
            Loaded::Text(Ok(TextFile::Binary)) => {
                DocState::Unreadable(t!("The file is binary: the content does not show here. Open it in the default program.").into())
            }
            Loaded::Text(Ok(TextFile::Text(text))) => match Source::open(&text, lang).filter(|_| from_file) {
                Some(source) => DocState::Source(source),
                None if is_markdown => DocState::Markdown(text),
                None => DocState::Text(TextDoc::highlighted(&text, lang)),
            },
        };
    }

    /// The text on screen is already the same as what arrived (the watcher's reread does not need to repaint anything).
    pub fn same_text(&self, loaded: &Loaded) -> bool {
        match (&self.state, loaded) {
            (DocState::Source(old), Loaded::Text(Ok(TextFile::Text(new)))) => old.is_disk(new),
            (DocState::Markdown(old), Loaded::Text(Ok(TextFile::Text(new)))) => old == new,
            (DocState::Text(old), Loaded::Text(Ok(TextFile::Text(new)))) => *old == TextDoc::new(new),
            _ => false,
        }
    }
}

/// Why a file over the read limit does not open here.
pub fn too_large_message(bytes: u64) -> String {
    t!(
        "The file has {size} and goes over the limit of {limit}. Open it in the default program.",
        size = size_label(bytes),
        limit = size_label(READ_TEXT_MAX)
    )
}

/// "12 KB", "1.5 MB" ("1,5 MB" in Portuguese).
pub fn size_label(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= KB * KB {
        let text = format!("{:.1} MB", b / (KB * KB));
        if holt_core::i18n::lang() == holt_core::i18n::Lang::PtBr {
            text.replace('.', ",")
        } else {
            text
        }
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} bytes")
    }
}

/// The app's documents and what the center shows.
#[derive(Default)]
pub struct Docs {
    pub view: CenterView,
    tabs: Vec<DocTab>,
    /// The focused document of each project.
    current: HashMap<String, u64>,
    next_uid: u64,
    /// DEBUG ONLY (`HOLT_DEBUG_UI=docs-demo`): the documents are fake; nothing is read or reread.
    pub debug_hold: bool,
    /// DEBUG ONLY (`docs-demo-badge`): the switch badge, instead of the one of the real tabs.
    pub debug_badge: Option<Badge>,
    /// The cache of the formatted markdown (links and scroll), shared by the documents.
    pub md_cache: egui_commonmark::CommonMarkCache,
    /// The open question about an unsaved edit (closing the tab or the app).
    pub confirm: Option<Unsaved>,
    /// Saves in progress and what to do when they finish.
    pub pending: HashMap<u64, After>,
}

/// What the question about an unsaved edit decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unsaved {
    /// Close the document tab (uid).
    Tab(u64),
    /// Close the app.
    Quit,
    /// Close the app with agents in the middle of a turn (closing kills the process and the turn is lost).
    Busy,
    /// Discard the edit and read the file from disk (the Reload button).
    Revert(u64),
}

/// What to do after a save succeeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum After {
    Nothing,
    /// Close the tab.
    Close,
    /// Quit the app (when nothing is left to save).
    Quit,
}

/// The user's answer to the question about an unsaved edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Save,
    Discard,
    Cancel,
}

/// The result of `Docs::open`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Opened {
    pub uid: u64,
    /// The document is new (whoever opened it has to load it).
    pub fresh: bool,
}

impl Docs {
    /// Opens the document, or only focuses it if the project already has it open, and switches the center to Documents. A document
    /// whose text comes from the task (request, report) is not reread: when it is opened again, the state and the titles become
    /// the current ones (the report changed after a return).
    pub fn open(&mut self, project: &str, kind: DocKind, title: String, subtitle: String, state: DocState) -> Opened {
        self.view = CenterView::Docs;
        if let Some(t) = self.tabs.iter_mut().find(|t| t.project == project && t.kind == kind) {
            if !t.kind.reloadable() {
                (t.title, t.subtitle, t.state) = (title, subtitle, state);
            }
            let uid = t.uid;
            self.current.insert(project.to_owned(), uid);
            return Opened { uid, fresh: false };
        }
        self.next_uid += 1;
        let uid = self.next_uid;
        self.tabs.push(DocTab::new(uid, project.to_owned(), kind, title, subtitle, state));
        self.current.insert(project.to_owned(), uid);
        Opened { uid, fresh: true }
    }

    pub fn focus(&mut self, uid: u64) {
        if let Some(t) = self.tabs.iter().find(|t| t.uid == uid) {
            self.current.insert(t.project.clone(), uid);
        }
    }

    /// Closes the document. Focus goes to the neighbour (the one on the right, or the last); if the project is left without
    /// documents and the center showed Documents, it goes back to Agents.
    pub fn close(&mut self, uid: u64) {
        let Some(pos) = self.tabs.iter().position(|t| t.uid == uid) else { return };
        let project = self.tabs.remove(pos).project;
        if self.current.get(&project) == Some(&uid) {
            let next = self.tabs.iter().skip(pos).chain(self.tabs.iter()).find(|t| t.project == project).map(|t| t.uid);
            match next {
                Some(n) => self.current.insert(project.clone(), n),
                None => self.current.remove(&project),
            };
        }
        self.sync(&project);
    }

    /// Takes out of the center what does not exist: Documents in a project with no document becomes Agents.
    pub fn sync(&mut self, project: &str) {
        if self.view == CenterView::Docs && self.count(project) == 0 {
            self.view = CenterView::Agents;
        }
    }

    /// Toggles the switch. Without documents in the project, it stays on Agents.
    pub fn toggle(&mut self, project: &str) {
        self.view = match self.view {
            CenterView::Docs => CenterView::Agents,
            CenterView::Agents if self.count(project) > 0 => CenterView::Docs,
            CenterView::Agents => CenterView::Agents,
        };
    }

    /// How many documents the project has open.
    pub fn count(&self, project: &str) -> usize {
        self.tabs.iter().filter(|t| t.project == project).count()
    }

    /// The documents of the project, in opening order.
    pub fn of_project<'a>(&'a self, project: &'a str) -> impl Iterator<Item = &'a DocTab> {
        self.tabs.iter().filter(move |t| t.project == project)
    }

    /// The focused document of the project (the first one, if none was chosen).
    pub fn current_uid(&self, project: &str) -> Option<u64> {
        self.current.get(project).copied().or_else(|| self.of_project(project).next().map(|t| t.uid))
    }

    pub fn get(&self, uid: u64) -> Option<&DocTab> {
        self.tabs.iter().find(|t| t.uid == uid)
    }

    pub fn get_mut(&mut self, uid: u64) -> Option<&mut DocTab> {
        self.tabs.iter_mut().find(|t| t.uid == uid)
    }

    /// Focus on the next (or previous) document of the project, wrapping around.
    pub fn cycle(&mut self, project: &str, back: bool) {
        let uids: Vec<u64> = self.of_project(project).map(|t| t.uid).collect();
        let n = uids.len();
        if n == 0 {
            return;
        }
        let cur = self.current_uid(project).and_then(|c| uids.iter().position(|u| *u == c)).unwrap_or(0);
        let next = (cur + if back { n - 1 } else { 1 }) % n;
        self.current.insert(project.to_owned(), uids[next]);
    }

    /// What to reread when `project` is active: the files (the watcher said so, or the project came back to the screen) and what
    /// stayed "Loading…" because the answer arrived with another project active and was discarded (`accepts_load`).
    pub fn to_reread(&self, project: &str) -> Vec<u64> {
        self.of_project(project)
            .filter(|t| t.kind.file_path().is_some() || (matches!(t.state, DocState::Loading) && t.kind.reads_active_project()))
            .map(|t| t.uid)
            .collect()
    }

    /// Is the read answer for `uid` valid? Not if the document closed, nor if it reads the active project and the active
    /// one is now not its own (the thread read another project's workspace).
    pub fn accepts_load(&self, uid: u64, active_project: &str) -> bool {
        self.get(uid).is_some_and(|t| !t.kind.reads_active_project() || t.project == active_project)
    }

    /// The documents with an unsaved edit, of every project.
    pub fn unsaved(&self) -> Vec<&DocTab> {
        self.tabs.iter().filter(|t| t.dirty()).collect()
    }

    /// Some save has not come back yet.
    pub fn saving(&self) -> bool {
        self.tabs.iter().any(|t| matches!(&t.state, DocState::Source(s) if s.saving))
    }

    /// A removed project takes its documents with it.
    pub fn drop_project(&mut self, project: &str) {
        self.tabs.retain(|t| t.project != project);
        self.current.remove(project);
        self.sync(project);
    }
}

/// The badge on the "Agents" side of the switch: what happened in the tabs while a document is being read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Badge {
    /// Tabs that finished a turn without being seen.
    pub done: usize,
    /// Tabs working right now.
    pub busy: usize,
}

impl Badge {
    /// From the `(done, busy)` of each tab of the project.
    pub fn of(tabs: impl IntoIterator<Item = (bool, bool)>) -> Self {
        tabs.into_iter()
            .fold(Self::default(), |b, (done, busy)| Self { done: b.done + usize::from(done), busy: b.busy + usize::from(busy) })
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.done == 0 && self.busy == 0
    }
}

/// The files of a diff, in the order they appear, with what each one changed. The path comes from `+++ b/…`
/// (or from `--- a/…` for a deleted file), because the `diff --git a/x b/y` header is ambiguous with spaces.
pub fn diff_files(diff: &GitDiff) -> Vec<DiffFile> {
    let mut files: Vec<DiffFile> = Vec::new();
    let mut in_header = false;
    for (i, line) in diff.lines.iter().enumerate() {
        if line.kind == DiffKind::Meta {
            let t = line.text.trim_end_matches('\r');
            if let Some(rest) =
                t.strip_prefix("diff --git ").or_else(|| t.strip_prefix("diff --cc ")).or_else(|| t.strip_prefix("diff --combined "))
            {
                in_header = true;
                files.push(DiffFile { path: header_path(rest, t.starts_with("diff --git ")), status: 'M', added: 0, removed: 0, line: i });
                continue;
            }
            let Some(file) = files.last_mut().filter(|_| in_header) else { continue };
            if t.starts_with("new file mode") {
                file.status = 'A';
            } else if t.starts_with("deleted file mode") {
                file.status = 'D';
            } else if let Some(to) = t.strip_prefix("rename to ") {
                file.status = 'R';
                file.path = unquote(to);
            } else if let Some(p) = t.strip_prefix("+++ ").filter(|p| *p != "/dev/null") {
                file.path = unquote(p.strip_prefix("b/").unwrap_or(p));
            } else if let Some(p) = t.strip_prefix("--- ").filter(|p| *p != "/dev/null") {
                if file.status == 'D' {
                    file.path = unquote(p.strip_prefix("a/").unwrap_or(p));
                }
            } else if t.starts_with("Binary files ") {
                // `Binary files a/x and b/y differ`: the header already gave the path.
            }
            continue;
        }
        if line.kind == DiffKind::Hunk {
            in_header = false;
        }
        if let Some(file) = files.last_mut() {
            match line.kind {
                DiffKind::Add => file.added += 1,
                DiffKind::Del => file.removed += 1,
                _ => {}
            }
        }
    }
    files
}

/// The path of a `diff --git a/x b/y` (the part after `diff --git `): the `b/…`; with spaces, where
/// the `a/` half and the `b/` half are equal.
fn header_path(rest: &str, git: bool) -> String {
    if !git {
        return unquote(rest);
    }
    let rest = rest.trim();
    // Without a rename both halves are equal: `a/<p> b/<p>`.
    if let Some(a) = rest.strip_prefix("a/").filter(|a| a.len() >= 3 && (a.len() - 3) % 2 == 0) {
        let n = (a.len() - 3) / 2;
        if a.is_char_boundary(n) && a.is_char_boundary(n + 3) && &a[n..n + 3] == " b/" && a[..n] == a[n + 3..] {
            return unquote(&a[..n]);
        }
    }
    match rest.rfind(" b/") {
        Some(i) => unquote(&rest[i + 3..]),
        None => unquote(rest),
    }
}

fn unquote(path: &str) -> String {
    let p = path.trim();
    p.strip_prefix('"').and_then(|p| p.strip_suffix('"')).unwrap_or(p).to_owned()
}

/// The file name (last component) of a path with `/` or `\`.
pub fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().filter(|n| !n.is_empty()).unwrap_or(path)
}

// ---------------------------------------------------------------------------
// The Deck and the documents: opening, reading (off the interface thread) and switching the center
// ---------------------------------------------------------------------------

impl Deck {
    /// The center goes back to showing the agents (clicking an agent, opening a tab).
    pub fn show_agents(&mut self) {
        self.docs.view = CenterView::Agents;
    }

    /// The center switched sides: the terminal loses the keyboard while documents are read, and when coming back the tab that
    /// was active counts as seen.
    pub(crate) fn center_changed(&mut self) {
        self.center_seen = self.docs.view;
        match self.docs.view {
            CenterView::Docs => {
                self.term_focus = false;
                self.ime_preedit.clear();
                self.new_tab_menu_open = false;
            }
            CenterView::Agents => {
                if let Some(tab) = self.active.and_then(|i| self.tabs.get_mut(i)) {
                    tab.done = false;
                }
                self.set_term_focus(true);
            }
        }
    }

    /// The badge on the "Agents" side of the switch: the tabs of the active project that finished without being seen or are working.
    pub fn agents_badge(&self) -> Badge {
        if let Some(badge) = self.docs.debug_badge {
            return badge;
        }
        Badge::of(self.visible().into_iter().map(|i| (self.tabs[i].done, self.tabs[i].busy)))
    }

    /// Opens (or focuses) a document of the active project and starts reading it if it is new.
    fn docs_open(&mut self, kind: DocKind, title: String, subtitle: String, state: DocState) {
        let project = self.active_project.clone();
        if project.is_empty() {
            return;
        }
        let opened = self.docs.open(&project, kind, title, subtitle, state);
        if opened.fresh {
            self.docs_load(opened.uid);
        }
    }

    /// The task diff: the branch's, or what the merge brought (see `tasks_ui::shows_merged_diff`).
    pub fn docs_open_task_diff(&mut self, id: &str) {
        let Some(task) = self.tasks.task(id) else { return };
        let merged = crate::tasks_ui::shows_merged_diff(task);
        let title = format!("{} · {}", task.id, if merged { "mesclagem" } else { "diff" });
        let subtitle = crate::tasks_ui::diff_title(task);
        self.docs_open(DocKind::TaskDiff { id: id.to_owned(), merged }, title, subtitle, DocState::Loading);
    }

    /// The task's request or report, already formatted. The text comes from the loaded task, without reading anything.
    pub fn docs_open_task_text(&mut self, id: &str, report: bool) {
        let Some(task) = self.tasks.task(id) else { return };
        let (what, text) = if report {
            (t!("report"), task.report.clone().unwrap_or_else(|| t!("The task has no report yet.").to_owned()))
        } else {
            (t!("request"), task.brief.clone())
        };
        let source = if report { MdSource::TaskReport(id.to_owned()) } else { MdSource::TaskBrief(id.to_owned()) };
        let (title, subtitle) = (format!("{} · {what}", task.id), format!("{} · {what}: {}", task.id, task.title));
        self.docs_open(DocKind::Markdown(source), title, subtitle, DocState::Markdown(text));
    }

    /// The diff of a commit from the History of the Git tab.
    pub fn docs_open_commit(&mut self, hash: &str, short_hash: &str, subject: &str) {
        let (title, subtitle) = (short_hash.to_owned(), format!("{short_hash}  {subject}"));
        self.docs_open(DocKind::CommitDiff(hash.to_owned()), title, subtitle, DocState::Loading);
    }

    /// The diff of a changed file of the Git tab (read-only; staging by hunk stays in the right bar).
    pub fn docs_open_file_diff(&mut self, entry: &GitEntry, staged: bool) {
        let side = if staged {
            t!("staged")
        } else if entry.kind == EntryKind::Untracked {
            t!("untracked")
        } else {
            t!("not staged")
        };
        let title = format!("{} · diff", file_name(&entry.path));
        let subtitle = format!("{}  · {side}", entry.path);
        let kind = DocKind::FileDiff {
            path: entry.path.clone(),
            orig: entry.orig_path.clone(),
            staged,
            untracked: entry.kind == EntryKind::Untracked,
        };
        self.docs_open(kind, title, subtitle, DocState::Loading);
    }

    /// A project file (path relative to it): `.md` formatted, the rest as text.
    pub fn docs_open_file(&mut self, rel: &str) {
        let ext = std::path::Path::new(rel).extension().and_then(|e| e.to_str()).unwrap_or("");
        let is_md = ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown");
        let kind = if is_md { DocKind::Markdown(MdSource::File(rel.to_owned())) } else { DocKind::File(rel.to_owned()) };
        self.docs_open(kind, file_name(rel).to_owned(), rel.to_owned(), DocState::Loading);
    }

    /// Closes a document (if it was the last one of the project, the center goes back to the agents). Without asking: the
    /// interface uses [`Deck::docs_request_close`].
    pub fn docs_close(&mut self, uid: u64) {
        self.docs.close(uid);
        if self.docs.confirm == Some(Unsaved::Tab(uid)) || self.docs.confirm == Some(Unsaved::Revert(uid)) {
            self.docs.confirm = None;
        }
        self.docs.pending.remove(&uid);
    }

    /// The request to close a document (the ×): with an unsaved edit, it asks first.
    pub fn docs_request_close(&mut self, uid: u64) {
        if self.docs.get(uid).is_some_and(DocTab::dirty) {
            self.docs.focus(uid);
            self.docs.confirm = Some(Unsaved::Tab(uid));
        } else {
            self.docs_close(uid);
        }
    }

    /// The request to quit the app (the tray's "Quit"; the window × only hides it): with an unsaved edit or
    /// with an agent in the middle of a turn, it asks first.
    pub fn request_quit(&mut self) {
        let first = self.docs.unsaved().first().map(|t| (t.uid, t.project.clone()));
        let Some((uid, project)) = first else {
            self.quit_unless_working();
            return;
        };
        // Shows the first document with an edit, if it belongs to the project on screen.
        if project == self.active_project {
            self.docs.focus(uid);
            self.docs.view = CenterView::Docs;
        }
        self.docs.confirm = Some(Unsaved::Quit);
    }

    /// Quits, or asks first if some agent is in the middle of a turn.
    fn quit_unless_working(&mut self) {
        if self.tabs.iter().any(|t| t.busy) {
            self.docs.confirm = Some(Unsaved::Busy);
        } else {
            self.quit_requested = true;
        }
    }

    /// Names of the tabs with an agent in the middle of a turn.
    pub fn working_tabs(&self) -> Vec<String> {
        self.tabs.iter().filter(|t| t.busy).map(|t| t.label.clone()).collect()
    }

    /// Reads the document again (reload button). A file opened as code does not go through "Loading…": if it has
    /// an unsaved edit, it asks before throwing it away; otherwise it rereads and the screen follows the disk.
    pub fn docs_reload(&mut self, uid: u64) {
        let Some(tab) = self.docs.get_mut(uid).filter(|t| t.kind.reloadable()) else { return };
        if tab.dirty() {
            self.docs.confirm = Some(Unsaved::Revert(uid));
        } else if matches!(tab.state, DocState::Source(_)) {
            self.docs_load(uid);
        } else {
            tab.state = DocState::Loading;
            self.docs_load(uid);
        }
    }

    /// `Ctrl+F`: opens the document search. The formatted markdown has nowhere to mark the match: the search shows the code.
    pub fn docs_find(&mut self, uid: u64) {
        let Some(tab) = self.docs.get_mut(uid) else { return };
        if let DocState::Source(src) = &mut tab.state {
            src.raw = true;
        }
        if matches!(tab.state, DocState::Source(_) | DocState::Text(_) | DocState::Diff(_)) {
            tab.find.show();
        }
    }

    /// Saves the edit of `uid` (`Ctrl+S`). `after` is what to do when the save succeeds. It does not save with the disk
    /// changed and the choice (keep mine / reload) still to be made: the document notice asks for that first.
    pub fn docs_save(&mut self, uid: u64, after: After) {
        let Some(tab) = self.docs.get_mut(uid) else { return };
        let (Some(rel), project) = (tab.kind.file_path().map(str::to_owned), tab.project.clone()) else { return };
        let DocState::Source(src) = &mut tab.state else { return };
        if src.saving {
            // The save already in progress takes what comes next (e.g. "Save all and quit" right after Ctrl+S).
            if after != After::Nothing {
                self.docs.pending.insert(uid, after);
            }
            return;
        }
        if src.locked.is_some() || src.external.is_some() {
            // Asked by the close question (Ctrl+S and the button are already off): shows the document and says why.
            if after != After::Nothing {
                let why = if src.external.is_some() {
                    t!("the file changed on disk; first choose between keeping your edit or reloading the one from disk")
                } else {
                    t!("the file is read-only")
                };
                let name = tab.subtitle.clone();
                self.docs.focus(uid);
                self.docs.view = CenterView::Docs;
                self.notify(holt_core::Tone::Warning, t!("I did not save {name}: {why}.", name, why));
            }
            return;
        }
        if !src.dirty() {
            return self.docs_finish(uid, after);
        }
        src.saving = true;
        src.save_error = None;
        let (text, eol, bom) = (src.text.clone(), src.eol, src.bom);
        self.docs.pending.insert(uid, after);
        self.spawn_bg("doc-save", move |core| {
            let result = core.write_text(&project, &rel, &text, eol, bom);
            Some(UiMsg::DocSaved(uid, text, result))
        });
    }

    /// Saves every edit (the app close question) and quits when the last one finishes. If one of them cannot be saved
    /// (the disk changed and the choice is missing), the app does not quit: the document is shown.
    pub fn docs_save_all_and_quit(&mut self) {
        let blocked = self.docs.unsaved().iter().find(|t| matches!(&t.state, DocState::Source(s) if s.external.is_some())).map(|t| t.uid);
        if let Some(uid) = blocked {
            self.docs.focus(uid);
            self.docs.view = CenterView::Docs;
            return;
        }
        let dirty: Vec<u64> = self.docs.unsaved().iter().map(|t| t.uid).collect();
        for uid in dirty {
            self.docs_save(uid, After::Quit);
        }
    }

    /// The save of `uid` came back. `saved` is the text that was saved (the one on screen may have changed since).
    pub fn on_doc_saved(&mut self, uid: u64, saved: String, result: Result<(), String>) {
        let after = self.docs.pending.remove(&uid).unwrap_or(After::Nothing);
        let Some(tab) = self.docs.get_mut(uid) else { return };
        let DocState::Source(src) = &mut tab.state else { return };
        match result {
            Ok(()) => src.mark_saved(&saved),
            Err(e) => {
                src.saving = false;
                src.save_error = Some(e.clone());
                let name = tab.subtitle.clone();
                self.docs.focus(uid);
                self.docs.view = CenterView::Docs;
                // The document may be from a project that is not on screen: the notice also goes to the Conversation.
                self.notify(holt_core::Tone::Error, t!("Could not save {name}: {error}", name, error = e));
                return;
            }
        }
        self.docs_finish(uid, after);
    }

    /// What comes after a save (or after a "nothing to save").
    fn docs_finish(&mut self, uid: u64, after: After) {
        match after {
            After::Nothing => {}
            After::Close => {
                // More was typed during the save: the tab stays, with what is left to save.
                if self.docs.get(uid).is_some_and(|t| !t.dirty()) {
                    self.docs_close(uid);
                }
            }
            After::Quit => {
                if self.docs.unsaved().is_empty() && !self.docs.saving() {
                    self.quit_requested = true;
                }
            }
        }
    }

    /// The answer to the question about an unsaved edit.
    pub fn docs_answer(&mut self, answer: Answer) {
        let Some(asked) = self.docs.confirm else { return };
        if answer == Answer::Cancel {
            self.docs.confirm = None;
            return;
        }
        match (asked, answer) {
            (Unsaved::Tab(uid), Answer::Save) => {
                self.docs.confirm = None;
                self.docs_save(uid, After::Close);
            }
            (Unsaved::Tab(uid), Answer::Discard) => self.docs_close(uid),
            (Unsaved::Quit, Answer::Save) => {
                self.docs.confirm = None;
                self.docs_save_all_and_quit();
            }
            (Unsaved::Quit, Answer::Discard) => {
                self.docs.confirm = None;
                self.quit_unless_working();
            }
            (Unsaved::Busy, Answer::Discard | Answer::Save) => {
                self.docs.confirm = None;
                self.quit_requested = true;
            }
            (Unsaved::Revert(uid), Answer::Discard | Answer::Save) => {
                self.docs.confirm = None;
                if let Some(DocState::Source(src)) = self.docs.get_mut(uid).map(|t| &mut t.state) {
                    src.discard();
                }
                self.docs_load(uid);
            }
            (_, Answer::Cancel) => {}
        }
    }

    /// The open files of the active project changed on disk (the watcher said so): rereads, without flicker.
    pub fn docs_reload_files(&mut self) {
        if self.docs.debug_hold {
            return;
        }
        for uid in self.docs.to_reread(&self.active_project) {
            self.docs_load(uid);
        }
    }

    /// Reads the document in a thread; the result comes back as `UiMsg::Doc`.
    fn docs_load(&mut self, uid: u64) {
        let Some(tab) = self.docs.get(uid).filter(|_| !self.docs.debug_hold) else { return };
        let (project, kind) = (tab.project.clone(), tab.kind.clone());
        let text_err = |e: GitError| e.to_string();
        match kind {
            DocKind::TaskDiff { id, .. } => self.spawn_bg("doc-task-diff", move |core| {
                Some(UiMsg::Doc(uid, Loaded::Diff(core.task_diff(&project, &id).map_err(|e| e.message().to_owned()))))
            }),
            DocKind::CommitDiff(hash) => self.spawn_bg("doc-commit-diff", move |core| {
                Some(UiMsg::Doc(uid, Loaded::Diff(core.git_commit_detail(&hash).map(|d| d.diff).map_err(text_err))))
            }),
            DocKind::FileDiff { path, orig, staged, untracked } => self.spawn_bg("doc-file-diff", move |core| {
                let kind = if untracked { EntryKind::Untracked } else { EntryKind::Ordinary };
                let entry = GitEntry { path, orig_path: orig, index: '.', worktree: '.', kind };
                Some(UiMsg::Doc(uid, Loaded::Diff(core.git_diff(&entry, staged).map_err(text_err))))
            }),
            DocKind::File(rel) | DocKind::Markdown(MdSource::File(rel)) => {
                self.spawn_bg("doc-file", move |core| Some(UiMsg::Doc(uid, Loaded::Text(core.read_text(&rel)))))
            }
            DocKind::Markdown(MdSource::TaskBrief(_) | MdSource::TaskReport(_)) => {}
        }
    }

    /// A document read arrived. A document closed in the meantime, or one of a project that is no longer the active one
    /// (the read was done in the other one's folder), ignores the answer; a file that did not change is not repainted.
    pub fn on_doc_loaded(&mut self, uid: u64, loaded: Loaded) {
        if self.docs.debug_hold || !self.docs.accepts_load(uid, &self.active_project) {
            return;
        }
        if let Some(tab) = self.docs.get_mut(uid).filter(|t| !t.same_text(&loaded)) {
            tab.apply(loaded);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use holt_core::git::parse_diff;

    const P: &str = "proj";

    fn open(docs: &mut Docs, project: &str, kind: DocKind) -> Opened {
        docs.open(project, kind, "t".into(), String::new(), DocState::Loading)
    }

    #[test]
    fn opening_creates_a_document_and_switches_the_center_to_documents() {
        let mut docs = Docs::default();
        assert_eq!(docs.view, CenterView::Agents);
        let o = open(&mut docs, P, DocKind::File("a.rs".into()));
        assert!(o.fresh);
        assert_eq!(docs.view, CenterView::Docs);
        assert_eq!(docs.count(P), 1);
        assert_eq!(docs.current_uid(P), Some(o.uid));
    }

    #[test]
    fn opening_an_open_document_only_focuses_it() {
        let mut docs = Docs::default();
        let a = open(&mut docs, P, DocKind::File("a.rs".into()));
        let b = open(&mut docs, P, DocKind::TaskDiff { id: "t1".into(), merged: false });
        assert_eq!(docs.current_uid(P), Some(b.uid));
        docs.view = CenterView::Agents;
        let again = open(&mut docs, P, DocKind::File("a.rs".into()));
        assert_eq!(again, Opened { uid: a.uid, fresh: false });
        assert_eq!(docs.count(P), 2);
        assert_eq!(docs.current_uid(P), Some(a.uid));
        assert_eq!(docs.view, CenterView::Docs, "focusing also takes the center to the documents");
    }

    #[test]
    fn the_same_kind_in_two_projects_is_two_documents() {
        let mut docs = Docs::default();
        let a = open(&mut docs, "p1", DocKind::File("a.rs".into()));
        let b = open(&mut docs, "p2", DocKind::File("a.rs".into()));
        assert_ne!(a.uid, b.uid);
        assert!(b.fresh);
        // The same key with another side is another document.
        let file_diff = |staged| DocKind::FileDiff { path: "a.rs".into(), orig: None, staged, untracked: false };
        let s = open(&mut docs, "p1", file_diff(true));
        let u = open(&mut docs, "p1", file_diff(false));
        assert_ne!(s.uid, u.uid);
        // The task diff before and after merging too.
        let before = open(&mut docs, "p1", DocKind::TaskDiff { id: "t1".into(), merged: false });
        let after = open(&mut docs, "p1", DocKind::TaskDiff { id: "t1".into(), merged: true });
        assert_ne!(before.uid, after.uid);
    }

    #[test]
    fn documents_belong_to_a_project() {
        let mut docs = Docs::default();
        open(&mut docs, "p1", DocKind::File("a.rs".into()));
        open(&mut docs, "p1", DocKind::File("b.rs".into()));
        open(&mut docs, "p2", DocKind::File("c.rs".into()));
        assert_eq!(docs.count("p1"), 2);
        assert_eq!(docs.count("p2"), 1);
        assert_eq!(docs.count("p3"), 0);
        let names: Vec<&str> = docs.of_project("p1").map(|t| t.kind.file_path().unwrap_or("")).collect();
        assert_eq!(names, ["a.rs", "b.rs"]);
        // Each project remembers its focus.
        let c = docs.current_uid("p2").expect("foco");
        assert_eq!(docs.get(c).map(|t| t.project.as_str()), Some("p2"));
        // In a project without documents the center does not stay on Documents.
        docs.sync("p3");
        assert_eq!(docs.view, CenterView::Agents);
    }

    #[test]
    fn closing_moves_focus_to_the_neighbour_and_the_last_one_goes_back_to_agents() {
        let mut docs = Docs::default();
        let a = open(&mut docs, P, DocKind::File("a.rs".into())).uid;
        let b = open(&mut docs, P, DocKind::File("b.rs".into())).uid;
        let c = open(&mut docs, P, DocKind::File("c.rs".into())).uid;
        docs.focus(b);
        docs.close(b);
        assert_eq!(docs.current_uid(P), Some(c), "the one on the right takes over");
        docs.close(c);
        assert_eq!(docs.current_uid(P), Some(a), "without a neighbour on the right, the one on the left");
        // Closing one that is not focused does not touch the focus.
        let d = open(&mut docs, P, DocKind::File("d.rs".into())).uid;
        docs.focus(d);
        docs.close(a);
        assert_eq!(docs.current_uid(P), Some(d));
        assert_eq!(docs.view, CenterView::Docs);
        docs.close(d);
        assert_eq!(docs.count(P), 0);
        assert_eq!(docs.view, CenterView::Agents);
        // Closing one that does not exist does nothing.
        docs.close(999);
    }

    #[test]
    fn the_switch_needs_documents_to_show_them() {
        let mut docs = Docs::default();
        docs.toggle(P);
        assert_eq!(docs.view, CenterView::Agents);
        let uid = open(&mut docs, P, DocKind::File("a.rs".into())).uid;
        docs.toggle(P);
        assert_eq!(docs.view, CenterView::Agents);
        docs.toggle(P);
        assert_eq!(docs.view, CenterView::Docs);
        docs.toggle("outro");
        assert_eq!(docs.view, CenterView::Agents);
        docs.toggle("outro");
        assert_eq!(docs.view, CenterView::Agents, "the other project has no documents");
        assert_eq!(docs.current_uid(P), Some(uid));
    }

    #[test]
    fn cycling_goes_around_the_project_documents() {
        let mut docs = Docs::default();
        let a = open(&mut docs, P, DocKind::File("a.rs".into())).uid;
        let b = open(&mut docs, P, DocKind::File("b.rs".into())).uid;
        let x = open(&mut docs, "p2", DocKind::File("x.rs".into())).uid;
        let c = open(&mut docs, P, DocKind::File("c.rs".into())).uid;
        docs.cycle(P, false);
        assert_eq!(docs.current_uid(P), Some(a));
        docs.cycle(P, false);
        assert_eq!(docs.current_uid(P), Some(b));
        docs.cycle(P, true);
        docs.cycle(P, true);
        assert_eq!(docs.current_uid(P), Some(c));
        assert_eq!(docs.current_uid("p2"), Some(x));
        docs.cycle("vazio", false);
    }

    #[test]
    fn dropping_a_project_takes_its_documents() {
        let mut docs = Docs::default();
        open(&mut docs, "p1", DocKind::File("a.rs".into()));
        open(&mut docs, "p2", DocKind::File("b.rs".into()));
        docs.drop_project("p1");
        assert_eq!((docs.count("p1"), docs.count("p2")), (0, 1));
        assert_eq!(docs.current_uid("p1"), None);
    }

    #[test]
    fn file_documents_are_the_ones_reread_when_the_workspace_changes() {
        let mut docs = Docs::default();
        let a = open(&mut docs, P, DocKind::File("a.rs".into())).uid;
        let m = open(&mut docs, P, DocKind::Markdown(MdSource::File("LEIA.md".into()))).uid;
        open(&mut docs, P, DocKind::Markdown(MdSource::TaskBrief("t1".into())));
        open(&mut docs, P, DocKind::TaskDiff { id: "t1".into(), merged: false });
        open(&mut docs, "p2", DocKind::File("z.rs".into()));
        assert_eq!(docs.to_reread(P), [a, m]);
        assert!(DocKind::File("a".into()).reloadable() && DocKind::CommitDiff("abc".into()).reloadable());
        assert!(!DocKind::Markdown(MdSource::TaskReport("t1".into())).reloadable());
    }

    #[test]
    fn reopening_the_task_request_or_report_shows_the_current_text() {
        let mut docs = Docs::default();
        let kind = DocKind::Markdown(MdSource::TaskReport("t1".into()));
        let text = |s: &str| DocState::Markdown(s.to_owned());
        let first = docs.open(P, kind.clone(), "t1 · report".into(), "old".into(), text("old report"));
        assert!(first.fresh);
        let again = docs.open(P, kind, "t1 · report".into(), "new".into(), text("new report"));
        assert_eq!(again, Opened { uid: first.uid, fresh: false });
        let tab = docs.get(first.uid).expect("tab");
        assert!(matches!(&tab.state, DocState::Markdown(t) if t == "new report"));
        assert_eq!(tab.subtitle, "new");
        // What comes from the core is not replaced on reopening (only "Reload" or the watcher rereads).
        let file = DocKind::File("a.rs".into());
        let f = docs.open(P, file.clone(), "a.rs".into(), String::new(), DocState::Loading);
        docs.get_mut(f.uid).expect("tab").state = DocState::Failed("x".into());
        docs.open(P, file, "a.rs".into(), String::new(), DocState::Loading);
        assert!(matches!(docs.get(f.uid).map(|t| &t.state), Some(DocState::Failed(_))));
    }

    #[test]
    fn a_read_that_arrives_after_switching_project_is_ignored() {
        let mut docs = Docs::default();
        let file = open(&mut docs, "p1", DocKind::File("a.rs".into())).uid;
        let md = open(&mut docs, "p1", DocKind::Markdown(MdSource::File("a.md".into()))).uid;
        let commit = open(&mut docs, "p1", DocKind::CommitDiff("abc".into())).uid;
        let changed = open(&mut docs, "p1", DocKind::FileDiff { path: "a.rs".into(), orig: None, staged: false, untracked: false }).uid;
        let task = open(&mut docs, "p1", DocKind::TaskDiff { id: "t1".into(), merged: false }).uid;
        for uid in [file, md, commit, changed] {
            assert!(docs.accepts_load(uid, "p1"));
            assert!(!docs.accepts_load(uid, "p2"), "read in the other project's folder");
        }
        // The task diff passes the project in the call: it is valid from anywhere.
        assert!(docs.accepts_load(task, "p2"));
        // Closed document: ignored.
        docs.close(file);
        assert!(!docs.accepts_load(file, "p1"));
    }

    #[test]
    fn what_was_discarded_while_loading_is_read_again_when_the_project_returns() {
        let mut docs = Docs::default();
        let file = open(&mut docs, "p1", DocKind::File("a.rs".into())).uid;
        let commit = open(&mut docs, "p1", DocKind::CommitDiff("abc".into())).uid;
        let done = open(&mut docs, "p1", DocKind::CommitDiff("def".into())).uid;
        docs.get_mut(done).expect("tab").state = DocState::Failed("x".into());
        open(&mut docs, "p1", DocKind::TaskDiff { id: "t1".into(), merged: false });
        open(&mut docs, "p2", DocKind::CommitDiff("ghi".into()));
        assert_eq!(docs.to_reread("p1"), [file, commit]);
    }

    #[test]
    fn badge_counts_finished_and_working_tabs() {
        assert!(Badge::of([]).is_empty());
        assert!(Badge::of([(false, false), (false, false)]).is_empty());
        assert_eq!(Badge::of([(true, false), (false, true), (true, false), (false, false)]), Badge { done: 2, busy: 1 });
    }

    const DIFF: &str = "diff --git a/src/a.rs b/src/a.rs
index 111..222 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,3 +1,4 @@
 fn a() {}
-let x = 1;
+let x = 2;
+let y = 3;
 fn b() {}
diff --git a/new file.txt b/new file.txt
new file mode 100644
index 0000000..333
--- /dev/null
+++ b/new file.txt
@@ -0,0 +1,2 @@
+one
+dois
diff --git a/old.txt b/old.txt
deleted file mode 100644
index 444..0000000
--- a/old.txt
+++ /dev/null
@@ -1 +0,0 @@
-adeus
diff --git a/before.rs b/after.rs
similarity index 90%
rename from before.rs
rename to after.rs
index 555..666 100644
--- a/before.rs
+++ b/after.rs
@@ -1 +1 @@
-a
+b
diff --git a/img.png b/img.png
index 777..888 100644
Binary files a/img.png and b/img.png differ
";

    #[test]
    fn the_file_list_comes_from_the_diff_headers() {
        let diff = parse_diff(DIFF, 1000);
        let files = diff_files(&diff);
        let got: Vec<(&str, char, usize, usize)> = files.iter().map(|f| (f.path.as_str(), f.status, f.added, f.removed)).collect();
        assert_eq!(
            got,
            [("src/a.rs", 'M', 2, 1), ("new file.txt", 'A', 2, 0), ("old.txt", 'D', 0, 1), ("after.rs", 'R', 1, 1), ("img.png", 'M', 0, 0),]
        );
        // Each file points at its own `diff --git` line.
        for f in &files {
            assert!(diff.lines[f.line].text.starts_with("diff --git "));
        }
        assert_eq!(files[0].line, 0);
        assert!(diff.lines[files[1].line].text.contains("new file.txt"));
    }

    #[test]
    fn a_diff_without_files_has_an_empty_list() {
        assert!(diff_files(&GitDiff::default()).is_empty());
        assert!(diff_files(&parse_diff("", 10)).is_empty());
    }

    #[test]
    fn a_truncated_diff_lists_only_what_it_has() {
        let diff = parse_diff(DIFF, 12);
        assert!(diff.truncated);
        assert_eq!(diff_files(&diff).len(), 2);
    }

    #[test]
    fn a_combined_diff_names_the_file_from_its_header() {
        let text = "diff --cc conflito.txt\nindex 1,2..3\n--- a/conflito.txt\n+++ b/conflito.txt\n@@@ -1,1 -1,1 +1,5 @@@\n++<<<<<<< ours\n";
        let files = diff_files(&parse_diff(text, 100));
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "conflito.txt");
        assert_eq!(files[0].added, 1);
    }

    #[test]
    fn selecting_a_file_asks_the_diff_to_scroll_to_it() {
        let mut doc = DiffDoc::new(parse_diff(DIFF, 1000));
        assert_eq!((doc.selected, doc.jump), (0, None));
        doc.select(2);
        assert_eq!((doc.selected, doc.jump), (2, Some(doc.files[2].line)));
        doc.select(99);
        assert_eq!(doc.selected, 2, "an index outside the list changes nothing");
    }

    fn tab(kind: DocKind) -> DocTab {
        DocTab::new(1, P.into(), kind, "t".into(), String::new(), DocState::Loading)
    }

    #[test]
    fn a_loaded_file_becomes_text_or_markdown_by_kind() {
        let mut t = tab(DocKind::File("a.rs".into()));
        t.apply(Loaded::Text(Ok(TextFile::Text("one\n\ttwo\n".into()))));
        // A file that fits in the editor opens as code (reads, searches and edits)...
        let DocState::Source(src) = &t.state else { panic!("code") };
        assert_eq!((src.text.as_str(), src.lang, src.editing), ("one\n\ttwo\n", Lang::Rust, false));
        // ...and a file too large, or with a line too long, is only read.
        let big = "x\n".repeat(EDIT_MAX / 2 + 1);
        t.apply(Loaded::Text(Ok(TextFile::Text(big))));
        assert!(matches!(&t.state, DocState::Text(d) if d.lines.len() > 1000));
        t.apply(Loaded::Text(Ok(TextFile::Text(format!("curta\n{}\n", "y".repeat(EDIT_LINE_MAX + 1))))));
        let DocState::Text(doc) = &t.state else { panic!("text") };
        assert_eq!(doc.lines[0], "curta");
        t.apply(Loaded::Text(Ok(TextFile::Text("one\n\ttwo\n".into()))));
        assert!(matches!(&t.state, DocState::Source(_)));

        let mut m = tab(DocKind::Markdown(MdSource::File("a.md".into())));
        m.apply(Loaded::Text(Ok(TextFile::Text("# Hi".into()))));
        let DocState::Source(md) = &m.state else { panic!("markdown as code") };
        assert_eq!((md.text.as_str(), md.lang), ("# Hi", Lang::Markdown));
        // A task's request and report do not come from a file: they stay as formatted markdown.
        let mut brief = tab(DocKind::Markdown(MdSource::TaskBrief("t1".into())));
        brief.apply(Loaded::Text(Ok(TextFile::Text("# Hi".into()))));
        assert!(matches!(&brief.state, DocState::Markdown(t) if t == "# Hi"));
    }

    fn source(raw: &str) -> Source {
        Source::open(raw, Lang::Plain).expect("cabe no editor")
    }

    #[test]
    fn the_line_ending_and_the_bom_come_back_as_they_were() {
        let s = source("\u{feff}one\r\ntwo\r\n");
        assert_eq!((s.text.as_str(), s.eol, s.bom, s.locked.is_none()), ("one\ntwo\n", Eol::Crlf, true, true));
        let s = source("one\ntwo");
        assert_eq!((s.eol, s.bom), (Eol::Lf, false));
        // Mixed line endings, or text that is not UTF-8: read-only, with the reason.
        assert!(source("a\r\nb\n").locked.as_deref().is_some_and(|m| m.contains("mixes")));
        assert!(source("a\u{fffd}b").locked.as_deref().is_some_and(|m| m.contains("UTF-8")));
        assert!(!source("a\u{fffd}b").can_edit());
    }

    #[test]
    fn a_big_file_or_a_long_line_does_not_open_in_the_editor() {
        assert!(Source::open(&"a\n".repeat(EDIT_MAX / 2), Lang::Plain).is_some());
        assert!(Source::open(&format!("{}a", "a\n".repeat(EDIT_MAX / 2)), Lang::Plain).is_none());
        assert!(Source::open(&format!("ok\n{}", "a".repeat(EDIT_LINE_MAX + 1)), Lang::Plain).is_none());
    }

    #[test]
    fn without_unsaved_edits_the_screen_follows_the_disk() {
        let mut s = source("one\n");
        let v = s.version;
        s.reload("one\n");
        assert_eq!(s.version, v, "the same text does not count as a change");
        s.reload("two\n");
        assert_eq!((s.text.as_str(), s.disk.as_str(), s.external.is_none()), ("two\n", "two\n", true));
        assert!(s.version > v);
        assert!(!s.dirty());
    }

    #[test]
    fn reloading_the_agents_text_takes_its_line_ending_bom_and_lock() {
        let mut s = source("one\n");
        s.set_editing(true);
        s.text = "one\nmine\n".into();
        s.touched();
        s.reload("\u{feff}one\r\nagent\r\n");
        s.take_disk();
        assert_eq!((s.text.as_str(), s.eol, s.bom), ("one\nagent\n", Eol::Crlf, true));
        // Keeping mine keeps the format of the file that was open.
        let mut s = source("one\n");
        s.set_editing(true);
        s.text = "one\nmine\n".into();
        s.touched();
        s.reload("one\r\nagent\r\n");
        s.keep_mine();
        assert_eq!((s.eol, s.bom), (Eol::Lf, false));
        // The agent's text that is not UTF-8 locks saving, and the edit goes away.
        let mut s = source("one\n");
        s.set_editing(true);
        s.text = "one\nmine\n".into();
        s.touched();
        s.reload("one\u{fffd}\n");
        s.take_disk();
        assert!(s.locked.is_some() && !s.editing);
    }

    #[test]
    fn an_agent_changing_the_file_during_an_edit_asks_instead_of_replacing() {
        let mut s = source("one\n");
        s.set_editing(true);
        s.text = "one\nmy line\n".into();
        s.touched();
        assert!(s.dirty());
        // The disk is still the same: nothing to warn about (it is what the watcher's read brings after saving).
        s.reload("one\n");
        assert!(s.external.is_none());
        assert!(s.is_disk("one\n"));
        // The agent changed it: its text waits; mine stays intact.
        s.reload("one\nagent line\n");
        assert_eq!(s.external.as_deref(), Some("one\nagent line\n"));
        assert_eq!(s.text, "one\nmy line\n");
        assert!(!s.is_disk("one\nagent line\n"), "the reread repeats the notice instead of being ignored");
        // Keep mine: the new disk becomes the base and the edit goes on, to be saved over it.
        s.keep_mine();
        assert_eq!((s.disk.as_str(), s.text.as_str(), s.external.is_none()), ("one\nagent line\n", "one\nmy line\n", true));
        assert!(s.dirty());
        // Reload from disk: the edit is discarded.
        s.reload("outra\n");
        s.take_disk();
        assert_eq!((s.text.as_str(), s.disk.as_str(), s.dirty()), ("outra\n", "outra\n", false));
    }

    #[test]
    fn an_agent_writing_what_the_user_typed_needs_no_choice() {
        let mut s = source("one\n");
        s.text = "two\n".into();
        s.reload("two\n");
        assert!(s.external.is_none() && !s.dirty());
    }

    #[test]
    fn a_save_that_finished_while_typing_leaves_the_rest_unsaved() {
        let mut s = source("one\n");
        s.text = "two\n".into();
        s.saving = true;
        // The user typed more while the save of "two" was going on.
        s.text = "two and a half\n".into();
        s.mark_saved("two\n");
        assert!(!s.saving && s.dirty(), "what came after still has to be saved");
        s.mark_saved("two and a half\n");
        assert!(!s.dirty());
    }

    #[test]
    fn discarding_goes_back_to_the_disk() {
        let mut s = source("one\n");
        s.text = "x\n".into();
        s.save_error = Some("falhou".into());
        s.discard();
        assert_eq!((s.text.as_str(), s.dirty(), s.save_error.is_none()), ("one\n", false, true));
    }

    #[test]
    fn a_dirty_file_is_never_replaced_by_a_read() {
        let mut t = tab(DocKind::File("a.rs".into()));
        t.apply(Loaded::Text(Ok(TextFile::Text("one\n".into()))));
        let DocState::Source(src) = &mut t.state else { panic!("code") };
        src.set_editing(true);
        src.text = "mine\n".into();
        // The file was deleted, became binary or too large: the edit stays and the notice shows.
        for gone in [Loaded::Text(Err("sumiu".into())), Loaded::Text(Ok(TextFile::Binary)), Loaded::Text(Ok(TextFile::TooLarge(9)))] {
            t.apply(gone);
            let DocState::Source(src) = &t.state else { panic!("the edit stays") };
            assert!(src.gone && src.text == "mine\n" && src.editing);
        }
        // It came back: the notice goes away and the new disk becomes the "changed" notice.
        t.apply(Loaded::Text(Ok(TextFile::Text("new\n".into()))));
        let DocState::Source(src) = &t.state else { panic!("code") };
        assert!(!src.gone && src.external.as_deref() == Some("new\n") && src.text == "mine\n");
        // The same read again does not repaint when the screen already shows it (no edit) and repeats the notice with one.
        assert!(!t.same_text(&Loaded::Text(Ok(TextFile::Text("new\n".into())))));
    }

    #[test]
    fn a_tab_with_unsaved_edits_says_so() {
        let mut t = tab(DocKind::File("a.rs".into()));
        assert!(!t.dirty());
        t.apply(Loaded::Text(Ok(TextFile::Text("one\n".into()))));
        assert!(!t.dirty());
        if let DocState::Source(s) = &mut t.state {
            s.text.push('x');
        }
        assert!(t.dirty());
        let mut docs = Docs::default();
        docs.tabs.push(t);
        assert_eq!(docs.unsaved().len(), 1);
    }

    #[test]
    fn the_search_walks_through_the_matches_and_wraps_around() {
        let mut f = Find::default();
        f.show();
        f.query = "ab".into();
        f.refresh(1, |q| (highlight::find_all("AB ab xab", q), Vec::new()));
        assert_eq!((f.count(), f.current, f.jump), (3, 0, true));
        f.jump = false;
        f.step(false);
        f.step(false);
        assert_eq!(f.current_range(), Some(&(7..9)));
        f.step(false);
        assert_eq!(f.current, 0, "wraps around");
        f.step(true);
        assert_eq!(f.current, 2);
        // The same query and version do not redo; editing the text redoes without scrolling the screen.
        f.jump = false;
        f.refresh(1, |_| panic!("should not search again"));
        f.refresh(2, |q| (highlight::find_all("ab", q), Vec::new()));
        assert_eq!((f.count(), f.current, f.jump), (1, 0, false));
        // A new query goes back to the first match and scrolls to it.
        f.query = "b".into();
        f.refresh(2, |q| (highlight::find_all("ab b", q), Vec::new()));
        assert_eq!((f.count(), f.current, f.jump), (2, 0, true));
        assert_eq!(f.stamp().map(|(q, v)| (q.as_str(), *v)), Some(("b", 2)));
        f.close();
        assert!(f.stamp().is_none(), "closed, the search marks nothing");
    }

    #[test]
    fn the_search_by_lines_serves_the_diff_and_the_big_file() {
        let mut f = Find::default();
        f.show();
        f.query = "hi".into();
        f.refresh(0, |q| (Vec::new(), highlight::find_lines(["hi", "x", "Hi!"], q)));
        assert_eq!((f.count(), f.current_line()), (2, Some(0)));
        f.step(false);
        assert_eq!(f.current_line(), Some(2));
    }

    #[test]
    fn the_shaped_text_is_rebuilt_only_when_the_text_the_width_or_the_search_change() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let mut out = ctx.run_ui(egui::RawInput::default(), |_| {});
        out.textures_delta.clear();
        let mut shaped = Shaped::default();
        let mut find = Find::default();
        let a = shaped.galley(&ctx, "fn a() {}\nlet x = 1;", Lang::Rust, 600.0, &find);
        let again = shaped.galley(&ctx, "fn a() {}\nlet x = 1;", Lang::Rust, 600.0, &find);
        assert!(Arc::ptr_eq(&a, &again), "the same text reuses the galley");
        assert_eq!(shaped.hl.runs, 1);
        let wider = shaped.galley(&ctx, "fn a() {}\nlet x = 1;", Lang::Rust, 800.0, &find);
        assert!(!Arc::ptr_eq(&a, &wider));
        assert_eq!(shaped.hl.runs, 1, "the width does not redo the highlighting");
        let edited = shaped.galley(&ctx, "fn b() {}\nlet x = 1;", Lang::Rust, 800.0, &find);
        assert!(!Arc::ptr_eq(&wider, &edited));
        assert_eq!(shaped.hl.runs, 2);
        // The open search marks the matches (another galley); closed, it goes back to the previous one.
        find.show();
        find.query = "x".into();
        find.refresh(1, |q| (highlight::find_all("fn b() {}\nlet x = 1;", q), Vec::new()));
        let marked = shaped.galley(&ctx, "fn b() {}\nlet x = 1;", Lang::Rust, 800.0, &find);
        assert!(!Arc::ptr_eq(&edited, &marked));
        assert_eq!(shaped.hl.runs, 2, "the search does not redo the highlighting");
        // The whole text is in the galley, without losing or repeating anything.
        assert_eq!(marked.job.text, "fn b() {}\nlet x = 1;");
    }

    #[test]
    fn the_editor_shows_up_to_a_quarter_megabyte_and_lines_up_to_four_thousand_bytes() {
        assert_eq!((EDIT_MAX, EDIT_LINE_MAX), (256 * 1024, 4000));
    }

    #[test]
    fn a_huge_line_is_cut_for_the_screen() {
        let doc = TextDoc::new(&format!("curta\n{}\n", "x".repeat(TEXT_LINE_MAX + 500)));
        assert_eq!(doc.lines[0], "curta");
        assert_eq!(doc.lines[1].chars().count(), TEXT_LINE_MAX + 1);
        assert!(doc.lines[1].ends_with('…'));
        assert_eq!(doc.max_chars, TEXT_LINE_MAX + 1);
    }

    #[test]
    fn big_binary_and_failed_files_explain_themselves() {
        let mut t = tab(DocKind::File("a.bin".into()));
        t.apply(Loaded::Text(Ok(TextFile::TooLarge(3 * 1024 * 1024))));
        assert!(matches!(&t.state, DocState::Unreadable(m) if m.contains("3.0 MB") && m.contains("1.0 MB")));
        t.apply(Loaded::Text(Ok(TextFile::Binary)));
        assert!(matches!(&t.state, DocState::Unreadable(m) if m.contains("binary")));
        t.apply(Loaded::Text(Err("could not open a.bin".into())));
        assert!(matches!(&t.state, DocState::Failed(m) if m.contains("a.bin")));
    }

    #[test]
    fn a_reloaded_diff_keeps_the_selected_file_when_it_still_exists() {
        let mut t = tab(DocKind::TaskDiff { id: "t1".into(), merged: false });
        t.apply(Loaded::Diff(Ok(parse_diff(DIFF, 1000))));
        let DocState::Diff(d) = &mut t.state else { panic!("diff") };
        d.select(3);
        t.apply(Loaded::Diff(Ok(parse_diff(DIFF, 1000))));
        assert!(matches!(&t.state, DocState::Diff(d) if d.selected == 3));
        t.apply(Loaded::Diff(Ok(parse_diff(&DIFF[..DIFF.find("diff --git a/new").unwrap_or(0)], 1000))));
        assert!(matches!(&t.state, DocState::Diff(d) if d.selected == 0 && d.files.len() == 1));
        t.apply(Loaded::Diff(Err("a branch sumiu".into())));
        assert!(matches!(&t.state, DocState::Failed(_)));
    }

    #[test]
    fn an_unchanged_file_does_not_need_repainting() {
        let mut t = tab(DocKind::File("a.rs".into()));
        t.apply(Loaded::Text(Ok(TextFile::Text("one\ntwo".into()))));
        assert!(t.same_text(&Loaded::Text(Ok(TextFile::Text("one\ntwo".into())))));
        assert!(!t.same_text(&Loaded::Text(Ok(TextFile::Text("one\nthree".into())))));
        assert!(!t.same_text(&Loaded::Text(Ok(TextFile::Binary))));
        assert!(!t.same_text(&Loaded::Diff(Ok(GitDiff::default()))));
    }

    #[test]
    fn sizes_use_the_decimal_separator_of_the_language() {
        assert_eq!(size_label(512), "512 bytes");
        assert_eq!(size_label(2048), "2 KB");
        assert_eq!(size_label(1_572_864), "1.5 MB");
        let pt = holt_core::i18n::with_lang(holt_core::i18n::Lang::PtBr, || size_label(1_572_864));
        assert_eq!(pt, "1,5 MB");
    }

    #[test]
    fn the_file_name_is_the_last_component() {
        assert_eq!(file_name("crates/app/src/docs.rs"), "docs.rs");
        assert_eq!(file_name("a\\b\\c.md"), "c.md");
        assert_eq!(file_name("solto.txt"), "solto.txt");
    }
}
