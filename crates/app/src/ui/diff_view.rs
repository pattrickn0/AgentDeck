//! Drawing of a diff, the same in the right bar and in Documents mode (as on GitHub and in VS Code with
//! word wrap): lines wrap at the visible width, with the continuation indented after the `+`/`−`, and there is no
//! horizontal scrolling. The technical git lines (`diff --git`, `index`, `---`, `+++`) are hidden; in a diff of several
//! files, each one gets a header with its path.

use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

use egui::text::{CCursor, LayoutJob};
use egui::{Align2, Id, Pos2, Rect, TextFormat, Ui, Vec2};

use holt_core::git::{DiffKind, DiffLine, GitDiff};

use super::select::{self, Pos, Selected};
use crate::highlight::{self, Carry, Lang, Mark, Span};
use crate::theme;

/// Height of a diff text line.
pub const ROW_H: f32 = 17.0;
/// Body of the diff's monospace font.
pub const FONT: f32 = 12.0;
/// File header and the space above it (except for the first one).
const FILE_H: f32 = 26.0;
const FILE_GAP: f32 = 10.0;
/// Right margin of the text (the scroll bar sits there).
const RIGHT_PAD: f32 = 14.0;
/// A tab becomes spaces.
const TAB: &str = "    ";

/// What the diff knows beyond its lines: the file (for highlighting, in a diff without `diff --git`, like the one of an
/// untracked file) and the Documents mode search.
#[derive(Debug, Clone, Copy, Default)]
pub struct View<'a> {
    /// The path of the diff's file, if there is only one.
    pub path: Option<&'a str>,
    /// What is being searched (empty = nothing), the lines that have it (in order) and the line of the current match.
    pub query: &'a str,
    pub found: &'a [usize],
    pub current: Option<usize>,
    /// The diff line to scroll to in this frame.
    pub jump: Option<usize>,
}

/// How a diff line shows on screen.
#[derive(Debug, Clone, PartialEq)]
enum Shape {
    /// Technical git header that the file header already sums up.
    Hidden,
    /// Start of a file: the path (or "old → new").
    File(String),
    /// Text in screen lines: the pieces (byte ranges of the text with tabs already expanded, see [`body_of`]).
    Rows(Vec<Range<usize>>),
}

/// What is measured once per diff and width: the shape and the accumulated height of each line.
struct Layout {
    shapes: Vec<Shape>,
    /// `tops[i]` is the y of line `i`; `tops[n]`, the total height.
    tops: Vec<f32>,
}

/// The widths available for the text, in px: the plain line (after the marker) and the hunk header (which
/// shares its first line with the buttons).
#[derive(Debug, Clone, Copy)]
struct Widths {
    body: f32,
    hunk: f32,
}

/// Splits `text` into pieces that fit in `max_w` px, measuring each character with `width` (the diff font:
/// emoji and CJK are wider). Breaks after the last space of the piece, if any, so the word goes whole to the next
/// line. Returns byte ranges, always on a character boundary; empty text = one piece.
pub fn wrap_ranges(text: &str, max_w: f32, width: &mut impl FnMut(char) -> f32) -> Vec<Range<usize>> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = Vec::new();
    let mut start = 0;
    let mut used = 0.0;
    let mut after_space: Option<usize> = None;
    let mut k = 0;
    while k < chars.len() {
        let (at, c) = chars[k];
        let w = width(c);
        if used + w > max_w && at > start {
            let end = after_space.filter(|&e| e > start).unwrap_or(at);
            out.push(start..end);
            start = end;
            // What went past the cut counts again in the new piece; the current character is measured again.
            used = chars.iter().filter(|(b, _)| *b >= start && *b < at).map(|(_, c)| width(*c)).sum();
            after_space = None;
            continue;
        }
        used += w;
        if c == ' ' {
            after_space = Some(at + 1);
        }
        k += 1;
    }
    out.push(start..text.len());
    out
}

/// The file title in a `diff --git a/x b/y` line (or `diff --cc x`): `y`, or `x → y` if it was renamed.
fn file_title(meta: &str) -> Option<String> {
    if let Some(rest) = meta.strip_prefix("diff --git ") {
        return Some(match rest.split_once(" b/") {
            Some((a, b)) => {
                let a = a.strip_prefix("a/").unwrap_or(a);
                if a == b {
                    b.to_owned()
                } else {
                    format!("{a} → {b}")
                }
            }
            None => rest.to_owned(),
        });
    }
    meta.strip_prefix("diff --cc ").or_else(|| meta.strip_prefix("diff --combined ")).map(str::to_owned)
}

/// The marker (`+`, `-`, space) and the rest of the line. Only a git diff has a marker: the one of an untracked
/// file is the plain file.
fn split_marker(line: &DiffLine, marked: bool) -> (&str, &str) {
    let text = line.text.trim_end_matches('\r');
    let has = marked && matches!(line.kind, DiffKind::Add | DiffKind::Del | DiffKind::Context);
    match text.chars().next().filter(|_| has) {
        Some(c) => text.split_at(c.len_utf8()),
        None => ("", text),
    }
}

/// The text that wraps and is painted, without the marker and with tabs replaced by spaces.
fn body_of(line: &DiffLine, marked: bool) -> String {
    split_marker(line, marked).1.replace('\t', TAB)
}

/// The shape of each line. The `index`/`---`/`+++` lines are hidden; `diff --git` becomes the file header with
/// `headers` (a diff of several files) and is hidden without it (the path is already in the title).
fn shape(line: &DiffLine, widths: Widths, marked: bool, headers: bool, width: &mut impl FnMut(char) -> f32) -> Shape {
    if line.kind == DiffKind::Meta {
        if let Some(title) = file_title(&line.text) {
            return if headers { Shape::File(title) } else { Shape::Hidden };
        }
        if ["index ", "--- ", "+++ "].iter().any(|p| line.text.starts_with(p)) {
            return Shape::Hidden;
        }
    }
    let max_w = if line.kind == DiffKind::Hunk { widths.hunk } else { widths.body };
    Shape::Rows(wrap_ranges(&body_of(line, marked), max_w, width))
}

fn layout(diff: &GitDiff, widths: Widths, marked: bool, headers: bool, width: &mut impl FnMut(char) -> f32) -> Layout {
    let shapes: Vec<Shape> = diff.lines.iter().map(|l| shape(l, widths, marked, headers, width)).collect();
    let mut tops = Vec::with_capacity(shapes.len() + 1);
    let mut y = 0.0;
    for (i, s) in shapes.iter().enumerate() {
        tops.push(y);
        y += match s {
            Shape::Hidden => 0.0,
            Shape::File(_) if i == 0 => FILE_H,
            Shape::File(_) => FILE_GAP + FILE_H,
            Shape::Rows(rows) => rows.len() as f32 * ROW_H,
        };
    }
    tops.push(y);
    Layout { shapes, tops }
}

/// How many columns each gutter number has and which ones show (old, new).
fn gutter_cols(diff: &GitDiff) -> (usize, bool, bool) {
    let max = diff.lines.iter().flat_map(|l| [l.old, l.new]).flatten().max().unwrap_or(0);
    let digits = max.to_string().len().max(3);
    (digits, diff.lines.iter().any(|l| l.old.is_some()), diff.lines.iter().any(|l| l.new.is_some()))
}

/// Fingerprint of the diff content: the stored measurement only holds for this text.
fn content_hash(diff: &GitDiff) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for l in &diff.lines {
        std::mem::discriminant(&l.kind).hash(&mut h);
        l.text.hash(&mut h);
    }
    h.finish()
}

/// The language of a single-file diff, from the header path (`b/…`, or the new name in a rename).
fn lang_of_header(meta: &str) -> Option<Lang> {
    let title = file_title(meta)?;
    let path = title.rsplit_once(" → ").map_or(title.as_str(), |(_, new)| new);
    Some(Lang::from_path(path))
}

/// The highlighting of each diff line, over the text that is painted (`body_of`: no marker, tabs as spaces). Each
/// side of the diff (the old one: context and `−`; the new one: context and `+`) carries its state from one line to the
/// next (block comment, multi-line string), reset at each hunk and each file.
fn diff_spans(diff: &GitDiff, marked: bool, hint: Option<&str>) -> Vec<Vec<Span>> {
    let mut lang = hint.map_or(Lang::Plain, Lang::from_path);
    let (mut old, mut new) = (Carry::default(), Carry::default());
    diff.lines
        .iter()
        .map(|line| {
            if let Some(found) = (line.kind == DiffKind::Meta).then(|| lang_of_header(&line.text)).flatten() {
                (lang, old, new) = (found, Carry::default(), Carry::default());
            }
            match line.kind {
                DiffKind::Hunk => {
                    (old, new) = (Carry::default(), Carry::default());
                    Vec::new()
                }
                DiffKind::Meta => Vec::new(),
                DiffKind::Add => highlight::highlight_line(lang, &body_of(line, marked), &mut new),
                DiffKind::Del => highlight::highlight_line(lang, &body_of(line, marked), &mut old),
                DiffKind::Context => {
                    let spans = highlight::highlight_line(lang, &body_of(line, marked), &mut new);
                    old = new;
                    spans
                }
            }
        })
        .collect()
}

/// Is this diff line painted as text (and searched)? Not if it is a technical header or a file title.
fn is_text_line(line: &DiffLine) -> bool {
    line.kind != DiffKind::Meta
        || (file_title(&line.text).is_none() && !["index ", "--- ", "+++ "].iter().any(|p| line.text.starts_with(p)))
}

/// The diff lines with `query` in the visible text (ignoring ASCII case).
pub fn search(diff: &GitDiff, query: &str) -> Vec<usize> {
    let marked = diff.lines.iter().any(|l| l.kind == DiffKind::Hunk);
    let bodies: Vec<String> = diff.lines.iter().map(|l| if is_text_line(l) { body_of(l, marked) } else { String::new() }).collect();
    highlight::find_lines(bodies.iter().map(String::as_str), query)
}

/// The byte of `orig` (the line text with tabs) that matches byte `at` of the text with tabs replaced by
/// spaces; in the middle of a tab, its start.
fn unexpanded(orig: &str, at: usize) -> usize {
    let mut seen = 0;
    for (i, c) in orig.char_indices() {
        let w = if c == '\t' { TAB.len() } else { c.len_utf8() };
        if seen + w > at {
            return i;
        }
        seen += w;
    }
    orig.len()
}

/// The text of a selection, for copying: only the code (no line number nor `+`/`−`), with the tabs back.
fn selection_text(diff: &GitDiff, marked: bool, sel: Selected) -> String {
    let mut out: Vec<String> = Vec::new();
    for n in sel.start.line..=sel.end.line.min(diff.lines.len().saturating_sub(1)) {
        let line = &diff.lines[n];
        if !is_text_line(line) {
            continue;
        }
        let orig = split_marker(line, marked).1;
        let len = body_of(line, marked).len();
        let Some(r) = sel.on_line(n, len) else { continue };
        out.push(orig[unexpanded(orig, r.start)..unexpanded(orig, r.end)].to_owned());
    }
    out.join("\n")
}

/// The part of text line `body` that piece `piece` shows, with the highlighting of `spans` and the search marks, as a
/// single text piece.
pub(super) fn piece_job(
    body: &str,
    piece: &Range<usize>,
    spans: &[Span],
    found: &[Range<usize>],
    current: bool,
    plain: egui::Color32,
    font: &egui::FontId,
) -> LayoutJob {
    let text = body.get(piece.clone()).unwrap_or("");
    let shift = |r: &Range<usize>| r.start.max(piece.start) - piece.start..r.end.min(piece.end).max(piece.start) - piece.start;
    let local: Vec<Span> = spans
        .iter()
        .filter(|s| s.range.end > piece.start && s.range.start < piece.end)
        .map(|s| Span { range: shift(&s.range), tok: s.tok })
        .collect();
    let marks: Vec<Range<usize>> = found.iter().filter(|r| r.end > piece.start && r.start < piece.end).map(shift).collect();
    let pal = theme::pal();
    let mut job = LayoutJob::default();
    for p in highlight::pieces(text, &[local], &marks, None) {
        // On the line of the current match, every match counts as the current one.
        let mark = if current && p.mark == Mark::Found { Mark::Current } else { p.mark };
        let (color, background) = pal.syn_mark(p.tok, mark, plain);
        job.append(&text[p.range], 0.0, TextFormat { font_id: font.clone(), color, background, ..Default::default() });
    }
    job
}

/// Draws `diff` in `area`, scrolling only vertically. `id` identifies the scroll; `view_opts`, what the diff knows beyond
/// its lines (the line to scroll to in this frame, the search). `hunk_reserve` is the width the buttons take on the right of each hunk header
/// (its text wraps before them); `on_hunk(ui, line, n)` draws them over the header of hunk `n`.
pub fn draw(
    ui: &mut Ui,
    area: Rect,
    diff: &GitDiff,
    id: Id,
    hunk_reserve: f32,
    view_opts: View,
    mut on_hunk: impl FnMut(&mut Ui, Rect, usize),
) {
    let font = theme::mono(FONT);
    let cell = ui.fonts_mut(|f| f.glyph_width(&font, 'M')).max(1.0);
    let (digits, has_old, has_new) = gutter_cols(diff);
    let num_w = digits as f32 * cell;
    let num_cols = usize::from(has_old) + usize::from(has_new);
    let gutter = 6.0 + num_cols as f32 * (num_w + 8.0) + 4.0;
    let marked = diff.lines.iter().any(|l| l.kind == DiffKind::Hunk);
    let headers = diff.lines.iter().filter(|l| l.kind == DiffKind::Meta && file_title(&l.text).is_some()).count() > 1;
    // The vertical scroll bar sits over the right margin; the marker takes one cell.
    let text_w = (area.width() - gutter - RIGHT_PAD).max(8.0 * cell);
    let widths = Widths { body: text_w - if marked { cell } else { 0.0 }, hunk: (text_w - hunk_reserve).max(8.0 * cell) };

    // The measurement only changes with the content or the width: it is kept between frames.
    type Fingerprint = (u64, i32, i32, bool, bool);
    let fingerprint: Fingerprint = (content_hash(diff), widths.body.round() as i32, widths.hunk.round() as i32, marked, headers);
    let key = id.with("diff-layout");
    let cached = ui.ctx().data(|d| d.get_temp::<(Fingerprint, Arc<Layout>)>(key));
    let lay = match cached {
        Some((f, lay)) if f == fingerprint => lay,
        _ => {
            let lay = Arc::new(ui.fonts_mut(|f| layout(diff, widths, marked, headers, &mut |c| f.glyph_width(&font, c))));
            ui.ctx().data_mut(|d| d.insert_temp(key, (fingerprint, lay.clone())));
            lay
        }
    };

    // The highlighting only changes with the content (not with the width): kept separately.
    let spans_key = id.with("diff-spans");
    let spans_print = (fingerprint.0, view_opts.path.map(str::to_owned));
    let spans = match ui.ctx().data(|d| d.get_temp::<((u64, Option<String>), Arc<Vec<Vec<Span>>>)>(spans_key)) {
        Some((f, spans)) if f == spans_print => spans,
        _ => {
            let spans = Arc::new(diff_spans(diff, marked, view_opts.path));
            ui.ctx().data_mut(|d| d.insert_temp(spans_key, (spans_print, spans.clone())));
            spans
        }
    };
    let found = view_opts.found;
    let sel_id = id.with("diff-sel");

    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::top_down(egui::Align::Min)));
    child.set_clip_rect(area);
    let mut scroll = egui::ScrollArea::vertical().id_salt(id).auto_shrink([false, false]);
    if let Some(line) = view_opts.jump {
        scroll = scroll.vertical_scroll_offset(lay.tops[line.min(diff.lines.len())]);
    }
    let total = *lay.tops.last().unwrap_or(&0.0);
    scroll.show_viewport(&mut child, |ui, view| {
        ui.set_min_height(total + 8.0);
        let origin = ui.max_rect().min;
        let width = ui.max_rect().width();
        let pal = theme::pal();
        let sel = select_text(ui, sel_id, area, origin, diff, &lay, marked, gutter, cell, &font);
        let first = lay.tops.partition_point(|&t| t <= view.min.y).saturating_sub(1);
        let mut hunk_no = diff.lines[..first].iter().filter(|l| l.kind == DiffKind::Hunk).count();
        let num_color = theme::with_opacity(pal.text_2, 0.7);
        for i in first..diff.lines.len() {
            let top = lay.tops[i];
            if top > view.max.y {
                break;
            }
            let line = &diff.lines[i];
            let h = lay.tops[i + 1] - top;
            let r = Rect::from_min_size(origin + Vec2::new(0.0, top), Vec2::new(width, h));
            let p = ui.painter().clone();
            match &lay.shapes[i] {
                Shape::Hidden => {}
                Shape::File(title) => {
                    let head = Rect::from_min_max(Pos2::new(r.min.x, r.max.y - FILE_H), r.max);
                    p.rect_filled(head, 0.0, pal.chip);
                    p.hline(head.x_range(), head.max.y - 0.5, egui::Stroke::new(1.0, pal.separator));
                    let g = super::galley_trunc(&p, title, theme::ui_semibold(12.0), pal.text_strong, head.width() - 20.0);
                    super::place(&p, head.shrink2(Vec2::new(10.0, 0.0)), Align2::LEFT_CENTER, g, pal.text_strong);
                }
                Shape::Rows(rows) => {
                    let (bg, fg) = match line.kind {
                        DiffKind::Add => (Some(theme::with_alpha(pal.success, 0.12)), pal.text),
                        DiffKind::Del => (Some(theme::with_alpha(pal.danger, 0.14)), pal.text),
                        DiffKind::Hunk => (Some(theme::with_alpha(pal.info, 0.10)), pal.info),
                        DiffKind::Meta | DiffKind::Context => (None, pal.text_2),
                    };
                    if let Some(bg) = bg {
                        p.rect_filled(r, 0.0, bg);
                    }
                    let y0 = r.min.y + ROW_H / 2.0;
                    let mut nx = r.min.x + 6.0 + num_w;
                    for (show, n) in [(has_old, line.old), (has_new, line.new)] {
                        if show {
                            let t = n.map(|v| v.to_string()).unwrap_or_default();
                            p.text(Pos2::new(nx, y0), Align2::RIGHT_CENTER, t, font.clone(), num_color);
                            nx += num_w + 8.0;
                        }
                    }
                    let tx = r.min.x + gutter;
                    let (marker, _) = split_marker(line, marked);
                    let mw = if marker.is_empty() { 0.0 } else { cell };
                    if !marker.is_empty() {
                        p.text(Pos2::new(tx, y0), Align2::LEFT_CENTER, marker, font.clone(), fg);
                    }
                    let body = body_of(line, marked);
                    let line_spans = spans.get(i).map_or(&[][..], Vec::as_slice);
                    let hits = if found.binary_search(&i).is_ok() { highlight::find_all(&body, view_opts.query) } else { Vec::new() };
                    let on_current = view_opts.current == Some(i);
                    let selected = sel.and_then(|s| s.on_line(i, body.len())).filter(|r| !r.is_empty());
                    for (k, range) in rows.iter().enumerate() {
                        let at = Pos2::new(tx + mw, r.min.y + k as f32 * ROW_H);
                        let job = piece_job(&body, range, line_spans, &hits, on_current, fg, &font);
                        let galley = p.layout_job(job);
                        if let Some(sr) = selected.as_ref().filter(|sr| sr.end > range.start && sr.start < range.end) {
                            let piece = body.get(range.clone()).unwrap_or("");
                            let a = char_index(piece, sr.start.max(range.start) - range.start);
                            let z = char_index(piece, sr.end.min(range.end) - range.start);
                            let (xa, xz) = (galley.pos_from_cursor(CCursor::new(a)).min.x, galley.pos_from_cursor(CCursor::new(z)).min.x);
                            let band = Rect::from_min_max(Pos2::new(at.x + xa, at.y), Pos2::new(at.x + xz.max(xa + 2.0), at.y + ROW_H));
                            p.rect_filled(band, 0.0, ui.visuals().selection.bg_fill);
                        }
                        p.galley(Pos2::new(at.x, at.y + (ROW_H - galley.size().y) / 2.0), galley, fg);
                    }
                    if line.kind == DiffKind::Hunk {
                        on_hunk(ui, Rect::from_min_size(r.min, Vec2::new(width, ROW_H)), hunk_no);
                        hunk_no += 1;
                    }
                }
            }
        }
    });
}

/// How many characters there are before byte `byte` of `text` (egui's `CCursor` counts characters).
fn char_index(text: &str, byte: usize) -> usize {
    text.get(..byte).map_or_else(|| text.chars().count(), |t| t.chars().count())
}

/// The byte of character number `chars` of `text`.
fn byte_index(text: &str, chars: usize) -> usize {
    text.char_indices().nth(chars).map_or(text.len(), |(i, _)| i)
}

/// Text selection in the diff: drag, Ctrl+A and Ctrl+C (see `select.rs`). Position = diff line and byte of the visible text.
#[allow(clippy::too_many_arguments)]
fn select_text(
    ui: &mut Ui,
    id: Id,
    area: Rect,
    origin: Pos2,
    diff: &GitDiff,
    lay: &Layout,
    marked: bool,
    gutter: f32,
    cell: f32,
    font: &egui::FontId,
) -> Option<Selected> {
    let painter = ui.painter().clone();
    let n = diff.lines.len();
    let end = diff
        .lines
        .iter()
        .rposition(is_text_line)
        .map_or(Pos::default(), |line| Pos { line, col: body_of(&diff.lines[line], marked).len() });
    let hit = |p: Pos2| -> Pos {
        if n == 0 {
            return Pos::default();
        }
        let y = p.y - origin.y;
        let i = lay.tops.partition_point(|&t| t <= y).saturating_sub(1).min(n - 1);
        let Shape::Rows(rows) = &lay.shapes[i] else { return Pos { line: i, col: 0 } };
        let k = (((y - lay.tops[i]) / ROW_H).max(0.0) as usize).min(rows.len() - 1);
        let line = &diff.lines[i];
        let body = body_of(line, marked);
        let piece = body.get(rows[k].clone()).unwrap_or("");
        let mw = if split_marker(line, marked).0.is_empty() { 0.0 } else { cell };
        let g = painter.layout_no_wrap(piece.to_owned(), font.clone(), egui::Color32::WHITE);
        let c = g.cursor_from_pos(Vec2::new((p.x - (origin.x + gutter + mw)).max(0.0), 0.0)).index;
        Pos { line: i, col: rows[k].start + byte_index(piece, c.into()) }
    };
    select::drive(ui, id, area, end, hit, |sel| selection_text(diff, marked, sel))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(kind: DiffKind, text: &str) -> DiffLine {
        DiffLine { kind, old: None, new: None, text: text.into() }
    }

    /// Test font: one cell per character, two for CJK and emoji.
    fn cells(c: char) -> f32 {
        if c as u32 >= 0x2E80 {
            2.0
        } else {
            1.0
        }
    }

    fn pieces(t: &str, max: f32) -> Vec<String> {
        wrap_ranges(t, max, &mut cells).into_iter().map(|r| t[r].to_owned()).collect()
    }

    fn rows(s: Shape) -> usize {
        match s {
            Shape::Rows(r) => r.len(),
            other => panic!("esperava linhas, veio {other:?}"),
        }
    }

    const W: Widths = Widths { body: 40.0, hunk: 40.0 };

    #[test]
    fn long_lines_wrap_at_a_space_or_at_the_width() {
        assert_eq!(pieces("", 10.0), vec![""]);
        assert_eq!(pieces("curto", 10.0), vec!["curto"]);
        // A word that does not fit goes whole to the next line.
        assert_eq!(pieces("one small cat sleeping", 10.0), vec!["one small ", "cat ", "sleeping"]);
        // Without a space, it cuts at the width.
        assert_eq!(pieces("abcdefghijklmnop", 6.0), vec!["abcdef", "ghijkl", "mnop"]);
        // Accented letters count as one character and are not split in the middle.
        assert_eq!(pieces("áéíóúçãõ", 3.0), vec!["áéí", "óúç", "ãõ"]);
        // Wide characters (CJK, emoji) take what they measure: nothing goes past the edge.
        assert_eq!(pieces("ab中中cd", 4.0), vec!["ab中", "中cd"]);
        assert_eq!(pieces("🙂🙂🙂", 3.0), vec!["🙂", "🙂", "🙂"]);
    }

    #[test]
    fn the_file_header_names_the_file() {
        assert_eq!(file_title("diff --git a/src/x.rs b/src/x.rs").as_deref(), Some("src/x.rs"));
        assert_eq!(file_title("diff --git a/old.md b/new.md").as_deref(), Some("old.md → new.md"));
        assert_eq!(file_title("diff --cc conflito.rs").as_deref(), Some("conflito.rs"));
        assert_eq!(file_title("index 123..456 100644"), None);
    }

    #[test]
    fn several_files_get_headers_and_lose_the_git_plumbing() {
        let meta = |t| line(DiffKind::Meta, t);
        let s = |l: &DiffLine, headers| shape(l, W, true, headers, &mut cells);
        assert_eq!(s(&meta("diff --git a/a b/a"), true), Shape::File("a".into()));
        assert_eq!(s(&meta("index 1..2"), true), Shape::Hidden);
        assert_eq!(s(&meta("+++ b/a"), true), Shape::Hidden);
        assert_eq!(rows(s(&meta("new file mode 100644"), true)), 1);
        // A single file: the path is already in the title, no header.
        assert_eq!(s(&meta("diff --git a/a b/a"), false), Shape::Hidden);
        assert_eq!(s(&meta("--- a/a"), false), Shape::Hidden);
    }

    #[test]
    fn the_marker_stays_out_of_the_wrapped_text_and_the_hunk_header_wraps_beside_the_buttons() {
        let add = line(DiffKind::Add, "+abcdefgh");
        assert_eq!(split_marker(&add, true), ("+", "abcdefgh"));
        assert_eq!(body_of(&line(DiffKind::Add, "+\tx"), true), "    x");
        // 8 characters in a width of 4 (the marker is already out of the count): two lines.
        assert_eq!(rows(shape(&add, Widths { body: 4.0, hunk: 40.0 }, true, false, &mut cells)), 2);
        // Untracked file: the text is the file, with no marker.
        assert_eq!(split_marker(&line(DiffKind::Add, "+x"), false), ("", "+x"));
        // The hunk header wraps at the width left beside the buttons.
        let hunk = line(DiffKind::Hunk, &"@".repeat(30));
        assert_eq!(rows(shape(&hunk, Widths { body: 40.0, hunk: 10.0 }, true, false, &mut cells)), 3);
    }

    #[test]
    fn heights_add_up_and_the_first_header_has_no_gap() {
        let diff = GitDiff {
            lines: vec![line(DiffKind::Meta, "diff --git a/a b/a"), line(DiffKind::Meta, "index 1"), line(DiffKind::Add, "+one line")],
            ..Default::default()
        };
        let lay = layout(&diff, W, true, true, &mut cells);
        assert_eq!(lay.tops, vec![0.0, FILE_H, FILE_H, FILE_H + ROW_H]);
    }

    #[test]
    fn the_fingerprint_follows_the_content() {
        let a = GitDiff { lines: vec![line(DiffKind::Add, "+ab")], ..Default::default() };
        let b = GitDiff { lines: vec![line(DiffKind::Add, "+ba")], ..Default::default() };
        // Same size, different content: another measurement.
        assert_ne!(content_hash(&a), content_hash(&b));
        assert_eq!(content_hash(&a), content_hash(&a.clone()));
    }

    fn diff_of(lines: &[(DiffKind, &str)]) -> GitDiff {
        GitDiff { lines: lines.iter().map(|(k, t)| line(*k, t)).collect(), ..Default::default() }
    }

    #[test]
    fn each_file_of_the_diff_is_highlighted_in_its_own_language() {
        let diff = diff_of(&[
            (DiffKind::Meta, "diff --git a/a.rs b/a.rs"),
            (DiffKind::Hunk, "@@ -1 +1 @@"),
            (DiffKind::Add, "+let x = 1; // fim"),
            (DiffKind::Meta, "diff --git a/a.py b/a.py"),
            (DiffKind::Hunk, "@@ -1 +1 @@"),
            (DiffKind::Add, "+def f(): pass"),
            (DiffKind::Meta, "diff --git a/nota.txt b/nota.txt"),
            (DiffKind::Hunk, "@@ -1 +1 @@"),
            (DiffKind::Add, "+let x = 1;"),
        ]);
        let spans = diff_spans(&diff, true, None);
        let tok = |line: usize, n: usize| spans[line].get(n).map(|s| s.tok);
        assert_eq!(tok(2, 0), Some(highlight::Tok::Keyword), "let in Rust");
        assert_eq!(spans[2].last().map(|s| s.tok), Some(highlight::Tok::Comment));
        assert_eq!(tok(5, 0), Some(highlight::Tok::Keyword), "def in Python");
        assert!(spans[8].is_empty(), "plain text gets no color");
        // Without a git header (untracked file), the language comes from the given path.
        let untracked = diff_of(&[(DiffKind::Add, "+let x = 1;")]);
        assert!(diff_spans(&untracked, false, None)[0].is_empty());
        assert_eq!(diff_spans(&untracked, false, Some("src/a.rs"))[0].first().map(|s| s.tok), Some(highlight::Tok::Keyword));
    }

    #[test]
    fn a_block_comment_carries_over_diff_lines_of_the_same_side_only() {
        let diff = diff_of(&[
            (DiffKind::Meta, "diff --git a/a.rs b/a.rs"),
            (DiffKind::Hunk, "@@ -1,3 +1,3 @@"),
            (DiffKind::Add, "+/* abre"),
            (DiffKind::Del, "-let old = 1;"),
            (DiffKind::Add, "+still one comment */ let x = 1;"),
        ]);
        let spans = diff_spans(&diff, true, None);
        assert_eq!(spans[2].first().map(|s| s.tok), Some(highlight::Tok::Comment));
        // The removed line is on the other side: it does not inherit the comment opened on the new side.
        assert_eq!(spans[3].first().map(|s| s.tok), Some(highlight::Tok::Keyword));
        assert_eq!(spans[4].first().map(|s| (s.tok, s.range.clone())), Some((highlight::Tok::Comment, 0..20)));
    }

    #[test]
    fn the_search_looks_only_at_the_text_that_shows() {
        let diff = diff_of(&[
            (DiffKind::Meta, "diff --git a/foo.rs b/foo.rs"),
            (DiffKind::Meta, "index 1..2"),
            (DiffKind::Meta, "--- a/foo.rs"),
            (DiffKind::Meta, "+++ b/foo.rs"),
            (DiffKind::Hunk, "@@ -1 +1 @@"),
            (DiffKind::Add, "+fn FOO() {}"),
            (DiffKind::Context, " nothing"),
            (DiffKind::Del, "-foo"),
        ]);
        assert_eq!(search(&diff, "foo"), [5, 7], "the header path does not count");
        assert!(search(&diff, "").is_empty());
    }

    #[test]
    fn copying_gives_the_code_without_markers_and_with_the_tabs_back() {
        let diff = diff_of(&[
            (DiffKind::Meta, "diff --git a/a.go b/a.go"),
            (DiffKind::Hunk, "@@ -1,2 +1,2 @@"),
            (DiffKind::Add, "+\tone"),
            (DiffKind::Context, " \ttwo"),
            (DiffKind::Del, "-three"),
        ]);
        let all = Selected { start: Pos { line: 0, col: 0 }, end: Pos { line: 4, col: 5 } };
        assert_eq!(selection_text(&diff, true, all), "@@ -1,2 +1,2 @@\n\tone\n\ttwo\nthree");
        // A piece of a single line (the body has the tab as four spaces).
        let part = Selected { start: Pos { line: 2, col: 4 }, end: Pos { line: 2, col: 7 } };
        assert_eq!(selection_text(&diff, true, part), "one");
        assert_eq!(unexpanded("\ta\tb", 0), 0);
        assert_eq!(unexpanded("\ta\tb", 4), 1);
        assert_eq!(unexpanded("\ta\tb", 6), 2, "in the middle of the second tab, its start");
        assert_eq!(unexpanded("\ta\tb", 99), 4);
    }

    #[test]
    fn a_piece_of_a_line_keeps_the_colors_and_the_marks_inside_it() {
        let body = "let x = 1; // fim";
        let spans = highlight::highlight_lines(Lang::Rust, body).remove(0);
        let font = theme::mono(FONT);
        let marks = highlight::find_all(body, "x");
        let job = piece_job(body, &(4..14), &spans, &marks, false, theme::pal().text, &font);
        // The piece is only what fits in the screen line, and the search mark stays inside it.
        assert_eq!(job.text, "x = 1; // ");
        assert_eq!(job.sections.first().map(|s| (usize::from(s.byte_range.start), usize::from(s.byte_range.end))), Some((0, 1)));
        assert_ne!(job.sections[0].format.background, egui::Color32::TRANSPARENT);
        assert_eq!(job.sections.last().map(|s| usize::from(s.byte_range.end)), Some(job.text.len()));
    }
}
