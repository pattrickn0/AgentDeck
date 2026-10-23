//! Desenho de um diff, o mesmo na barra direita e no modo Documentos (como no GitHub e no VS Code com
//! quebra de linha): as linhas quebram na largura visível, com a continuação recuada depois do `+`/`−`, sem
//! rolagem horizontal. As linhas técnicas do git (`diff --git`, `index`, `---`, `+++`) somem; num diff de vários
//! arquivos, cada um ganha um cabeçalho com o caminho.

use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

use egui::text::{CCursor, LayoutJob};
use egui::{Align2, Id, Pos2, Rect, TextFormat, Ui, Vec2};

use agentdeck_core::git::{DiffKind, DiffLine, GitDiff};

use super::select::{self, Pos, Selected};
use crate::highlight::{self, Carry, Lang, Mark, Span};
use crate::theme;

/// Altura de uma linha de texto do diff.
pub const ROW_H: f32 = 17.0;
/// Corpo da fonte monoespaçada do diff.
pub const FONT: f32 = 12.0;
/// Cabeçalho de arquivo e o respiro acima dele (menos no primeiro).
const FILE_H: f32 = 26.0;
const FILE_GAP: f32 = 10.0;
/// Margem da direita do texto (a barra de rolagem fica ali).
const RIGHT_PAD: f32 = 14.0;
/// Tabulação vira espaços.
const TAB: &str = "    ";

/// O que o diff sabe além das linhas: o arquivo (para o realce, num diff sem `diff --git`, como o de um arquivo não
/// rastreado) e a busca do modo Documentos.
#[derive(Debug, Clone, Copy, Default)]
pub struct View<'a> {
    /// O caminho do arquivo do diff, se é um só.
    pub path: Option<&'a str>,
    /// O que se busca (vazio = nada), as linhas que o têm (em ordem) e a linha da ocorrência atual.
    pub query: &'a str,
    pub found: &'a [usize],
    pub current: Option<usize>,
    /// A linha do diff para onde rolar neste quadro.
    pub jump: Option<usize>,
}

/// Como uma linha do diff aparece na tela.
#[derive(Debug, Clone, PartialEq)]
enum Shape {
    /// Cabeçalho técnico do git que o cabeçalho do arquivo já resume.
    Hidden,
    /// Início de um arquivo: o caminho (ou "antigo → novo").
    File(String),
    /// Texto em linhas da tela: os pedaços (intervalos de bytes do texto já sem tabulação, ver [`body_of`]).
    Rows(Vec<Range<usize>>),
}

/// O que se mede uma vez por diff e largura: a forma e a altura acumulada de cada linha.
struct Layout {
    shapes: Vec<Shape>,
    /// `tops[i]` é o y da linha `i`; `tops[n]`, a altura total.
    tops: Vec<f32>,
}

/// As larguras disponíveis para o texto, em px: a linha comum (depois do marcador) e o cabeçalho do trecho (que
/// divide a primeira linha com os botões).
#[derive(Debug, Clone, Copy)]
struct Widths {
    body: f32,
    hunk: f32,
}

/// Divide `text` em pedaços que cabem em `max_w` px, medindo cada caractere com `width` (a fonte do diff:
/// emoji e CJK são mais largos). Quebra depois do último espaço do pedaço, se houver, para a palavra ir inteira
/// para a linha seguinte. Devolve intervalos de bytes, sempre em fronteira de caractere; texto vazio = um pedaço.
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
            // O que já passou do corte volta a contar no pedaço novo; o caractere atual é medido de novo.
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

/// O título do arquivo numa linha `diff --git a/x b/y` (ou `diff --cc x`): `y`, ou `x → y` se mudou de nome.
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

/// O marcador (`+`, `-`, espaço) e o resto da linha. Só diff do git tem marcador: o de um arquivo não
/// rastreado é o arquivo puro.
fn split_marker(line: &DiffLine, marked: bool) -> (&str, &str) {
    let text = line.text.trim_end_matches('\r');
    let has = marked && matches!(line.kind, DiffKind::Add | DiffKind::Del | DiffKind::Context);
    match text.chars().next().filter(|_| has) {
        Some(c) => text.split_at(c.len_utf8()),
        None => ("", text),
    }
}

/// O texto que se quebra e se pinta, sem o marcador e com a tabulação trocada por espaços.
fn body_of(line: &DiffLine, marked: bool) -> String {
    split_marker(line, marked).1.replace('\t', TAB)
}

/// A forma de cada linha. As linhas `index`/`---`/`+++` somem; a `diff --git` vira o cabeçalho do arquivo com
/// `headers` (diff de vários arquivos) e some sem ele (o caminho já está no título).
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

/// Quantas colunas tem cada número do gutter e quais aparecem (antigo, novo).
fn gutter_cols(diff: &GitDiff) -> (usize, bool, bool) {
    let max = diff.lines.iter().flat_map(|l| [l.old, l.new]).flatten().max().unwrap_or(0);
    let digits = max.to_string().len().max(3);
    (digits, diff.lines.iter().any(|l| l.old.is_some()), diff.lines.iter().any(|l| l.new.is_some()))
}

/// Impressão digital do conteúdo do diff: a medida guardada só vale para este texto.
fn content_hash(diff: &GitDiff) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for l in &diff.lines {
        std::mem::discriminant(&l.kind).hash(&mut h);
        l.text.hash(&mut h);
    }
    h.finish()
}

/// A linguagem de um diff de um arquivo só, pelo caminho do cabeçalho (`b/…`, ou o novo nome numa renomeação).
fn lang_of_header(meta: &str) -> Option<Lang> {
    let title = file_title(meta)?;
    let path = title.rsplit_once(" → ").map_or(title.as_str(), |(_, new)| new);
    Some(Lang::from_path(path))
}

/// O realce de cada linha do diff, sobre o texto que se pinta (`body_of`: sem o marcador, tabulação em espaços). Cada
/// lado do diff (o antigo: contexto e `−`; o novo: contexto e `+`) leva o seu estado de uma linha para a seguinte
/// (comentário de bloco, texto de várias linhas), zerado a cada trecho e a cada arquivo.
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

/// Esta linha do diff se pinta como texto (e a busca olha)? Não, se é um cabeçalho técnico ou o título de um arquivo.
fn is_text_line(line: &DiffLine) -> bool {
    line.kind != DiffKind::Meta
        || (file_title(&line.text).is_none() && !["index ", "--- ", "+++ "].iter().any(|p| line.text.starts_with(p)))
}

/// As linhas do diff com `query` no texto que se vê (sem diferença de maiúsculas e minúsculas ASCII).
pub fn search(diff: &GitDiff, query: &str) -> Vec<usize> {
    let marked = diff.lines.iter().any(|l| l.kind == DiffKind::Hunk);
    let bodies: Vec<String> = diff.lines.iter().map(|l| if is_text_line(l) { body_of(l, marked) } else { String::new() }).collect();
    highlight::find_lines(bodies.iter().map(String::as_str), query)
}

/// O byte de `orig` (o texto da linha com tabulações) que corresponde ao byte `at` do texto já com elas trocadas por
/// espaços; no meio de uma tabulação, o começo dela.
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

/// O texto de uma seleção, para copiar: só o código (sem número de linha nem `+`/`−`), com as tabulações de volta.
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

/// O trecho da linha de texto `body` que a peça `piece` mostra, com o realce de `spans` e as marcas da busca, numa só
/// peça de texto.
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
        let background = match p.mark {
            Mark::None => egui::Color32::TRANSPARENT,
            Mark::Found => theme::with_alpha(pal.warning, 0.30),
            Mark::Current => theme::with_alpha(pal.warning, 0.65),
        };
        let background = if current && p.mark == Mark::Found { theme::with_alpha(pal.warning, 0.65) } else { background };
        job.append(
            &text[p.range],
            0.0,
            TextFormat { font_id: font.clone(), color: pal.syn(p.tok, plain), background, ..Default::default() },
        );
    }
    job
}

/// Desenha `diff` em `area`, rolando só na vertical. `id` identifica a rolagem; `view_opts`, o que o diff sabe além das
/// linhas (a linha para onde rolar neste quadro, a busca). `hunk_reserve` é a largura que os botões ocupam à direita do cabeçalho de cada trecho
/// (o texto dele quebra antes); `on_hunk(ui, linha, n)` os desenha por cima do cabeçalho do trecho `n`.
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
    // A barra de rolagem vertical fica por cima da margem da direita; o marcador ocupa uma célula.
    let text_w = (area.width() - gutter - RIGHT_PAD).max(8.0 * cell);
    let widths = Widths { body: text_w - if marked { cell } else { 0.0 }, hunk: (text_w - hunk_reserve).max(8.0 * cell) };

    // A medida só muda com o conteúdo ou com a largura: fica guardada entre os quadros.
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

    // O realce só muda com o conteúdo (não com a largura): guardado à parte.
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

/// Quantos caracteres há antes do byte `byte` de `text` (o `CCursor` do egui conta caracteres).
fn char_index(text: &str, byte: usize) -> usize {
    text.get(..byte).map_or_else(|| text.chars().count(), |t| t.chars().count())
}

/// O byte do caractere de número `chars` de `text`.
fn byte_index(text: &str, chars: usize) -> usize {
    text.char_indices().nth(chars).map_or(text.len(), |(i, _)| i)
}

/// A seleção de texto do diff: arrastar, Ctrl+A e Ctrl+C (ver `select.rs`). Posição = linha do diff e byte do texto que se vê.
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

    /// Fonte de teste: uma célula por caractere, duas para CJK e emoji.
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
        // A palavra que não cabe vai inteira para a linha seguinte.
        assert_eq!(pieces("uma frase com palavras", 10.0), vec!["uma frase ", "com ", "palavras"]);
        // Sem espaço, corta na largura.
        assert_eq!(pieces("abcdefghijklmnop", 6.0), vec!["abcdef", "ghijkl", "mnop"]);
        // Acentos contam como um caractere e não partem no meio.
        assert_eq!(pieces("áéíóúçãõ", 3.0), vec!["áéí", "óúç", "ãõ"]);
        // Caracteres largos (CJK, emoji) ocupam o que medem: nada passa da borda.
        assert_eq!(pieces("ab中中cd", 4.0), vec!["ab中", "中cd"]);
        assert_eq!(pieces("🙂🙂🙂", 3.0), vec!["🙂", "🙂", "🙂"]);
    }

    #[test]
    fn the_file_header_names_the_file() {
        assert_eq!(file_title("diff --git a/src/x.rs b/src/x.rs").as_deref(), Some("src/x.rs"));
        assert_eq!(file_title("diff --git a/velho.md b/novo.md").as_deref(), Some("velho.md → novo.md"));
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
        // Um arquivo só: o caminho já está no título, sem cabeçalho.
        assert_eq!(s(&meta("diff --git a/a b/a"), false), Shape::Hidden);
        assert_eq!(s(&meta("--- a/a"), false), Shape::Hidden);
    }

    #[test]
    fn the_marker_stays_out_of_the_wrapped_text_and_the_hunk_header_wraps_beside_the_buttons() {
        let add = line(DiffKind::Add, "+abcdefgh");
        assert_eq!(split_marker(&add, true), ("+", "abcdefgh"));
        assert_eq!(body_of(&line(DiffKind::Add, "+\tx"), true), "    x");
        // 8 caracteres em 4 de largura (o marcador já saiu da conta): duas linhas.
        assert_eq!(rows(shape(&add, Widths { body: 4.0, hunk: 40.0 }, true, false, &mut cells)), 2);
        // Arquivo não rastreado: o texto é o arquivo, sem marcador.
        assert_eq!(split_marker(&line(DiffKind::Add, "+x"), false), ("", "+x"));
        // O cabeçalho do trecho quebra na largura que sobra ao lado dos botões.
        let hunk = line(DiffKind::Hunk, &"@".repeat(30));
        assert_eq!(rows(shape(&hunk, Widths { body: 40.0, hunk: 10.0 }, true, false, &mut cells)), 3);
    }

    #[test]
    fn heights_add_up_and_the_first_header_has_no_gap() {
        let diff = GitDiff {
            lines: vec![line(DiffKind::Meta, "diff --git a/a b/a"), line(DiffKind::Meta, "index 1"), line(DiffKind::Add, "+uma linha")],
            ..Default::default()
        };
        let lay = layout(&diff, W, true, true, &mut cells);
        assert_eq!(lay.tops, vec![0.0, FILE_H, FILE_H, FILE_H + ROW_H]);
    }

    #[test]
    fn the_fingerprint_follows_the_content() {
        let a = GitDiff { lines: vec![line(DiffKind::Add, "+ab")], ..Default::default() };
        let b = GitDiff { lines: vec![line(DiffKind::Add, "+ba")], ..Default::default() };
        // Mesmo tamanho, conteúdo diferente: outra medida.
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
        assert_eq!(tok(2, 0), Some(highlight::Tok::Keyword), "let em Rust");
        assert_eq!(spans[2].last().map(|s| s.tok), Some(highlight::Tok::Comment));
        assert_eq!(tok(5, 0), Some(highlight::Tok::Keyword), "def em Python");
        assert!(spans[8].is_empty(), "texto puro não leva cor");
        // Sem cabeçalho do git (arquivo não rastreado), a linguagem vem do caminho dado.
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
            (DiffKind::Del, "-let velho = 1;"),
            (DiffKind::Add, "+ainda comentário */ let x = 1;"),
        ]);
        let spans = diff_spans(&diff, true, None);
        assert_eq!(spans[2].first().map(|s| s.tok), Some(highlight::Tok::Comment));
        // A linha removida é do outro lado: não herda o comentário aberto do lado novo.
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
            (DiffKind::Context, " nada"),
            (DiffKind::Del, "-foo"),
        ]);
        assert_eq!(search(&diff, "foo"), [5, 7], "o caminho do cabeçalho não conta");
        assert!(search(&diff, "").is_empty());
    }

    #[test]
    fn copying_gives_the_code_without_markers_and_with_the_tabs_back() {
        let diff = diff_of(&[
            (DiffKind::Meta, "diff --git a/a.go b/a.go"),
            (DiffKind::Hunk, "@@ -1,2 +1,2 @@"),
            (DiffKind::Add, "+\tum"),
            (DiffKind::Context, " \tdois"),
            (DiffKind::Del, "-tres"),
        ]);
        let all = Selected { start: Pos { line: 0, col: 0 }, end: Pos { line: 4, col: 4 } };
        assert_eq!(selection_text(&diff, true, all), "@@ -1,2 +1,2 @@\n\tum\n\tdois\ntres");
        // Um pedaço de uma linha só (o corpo tem a tabulação como quatro espaços).
        let part = Selected { start: Pos { line: 2, col: 4 }, end: Pos { line: 2, col: 6 } };
        assert_eq!(selection_text(&diff, true, part), "um");
        assert_eq!(unexpanded("\ta\tb", 0), 0);
        assert_eq!(unexpanded("\ta\tb", 4), 1);
        assert_eq!(unexpanded("\ta\tb", 6), 2, "no meio da segunda tabulação, o começo dela");
        assert_eq!(unexpanded("\ta\tb", 99), 4);
    }

    #[test]
    fn a_piece_of_a_line_keeps_the_colors_and_the_marks_inside_it() {
        let body = "let x = 1; // fim";
        let spans = highlight::highlight_lines(Lang::Rust, body).remove(0);
        let font = theme::mono(FONT);
        let marks = highlight::find_all(body, "x");
        let job = piece_job(body, &(4..14), &spans, &marks, false, theme::pal().text, &font);
        // O pedaço é só o que cabe na linha da tela, e a marca da busca fica dentro dele.
        assert_eq!(job.text, "x = 1; // ");
        assert_eq!(job.sections.first().map(|s| (usize::from(s.byte_range.start), usize::from(s.byte_range.end))), Some((0, 1)));
        assert_ne!(job.sections[0].format.background, egui::Color32::TRANSPARENT);
        assert_eq!(job.sections.last().map(|s| usize::from(s.byte_range.end)), Some(job.text.len()));
    }
}
