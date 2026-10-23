//! Desenho de um diff, o mesmo na barra direita e no modo Documentos (como no GitHub e no VS Code com
//! quebra de linha): as linhas quebram na largura visível, com a continuação recuada depois do `+`/`−`, sem
//! rolagem horizontal. As linhas técnicas do git (`diff --git`, `index`, `---`, `+++`) somem; num diff de vários
//! arquivos, cada um ganha um cabeçalho com o caminho.

use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

use egui::{Align2, Id, Pos2, Rect, Ui, Vec2};

use agentdeck_core::git::{DiffKind, DiffLine, GitDiff};

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

/// Desenha `diff` em `area`, rolando só na vertical. `id` identifica a rolagem; `jump`, a linha do diff para onde
/// rolar neste quadro. `hunk_reserve` é a largura que os botões ocupam à direita do cabeçalho de cada trecho
/// (o texto dele quebra antes); `on_hunk(ui, linha, n)` os desenha por cima do cabeçalho do trecho `n`.
pub fn draw(
    ui: &mut Ui,
    area: Rect,
    diff: &GitDiff,
    id: Id,
    jump: Option<usize>,
    hunk_reserve: f32,
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

    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::top_down(egui::Align::Min)));
    child.set_clip_rect(area);
    let mut scroll = egui::ScrollArea::vertical().id_salt(id).auto_shrink([false, false]);
    if let Some(line) = jump {
        scroll = scroll.vertical_scroll_offset(lay.tops[line.min(diff.lines.len())]);
    }
    let total = *lay.tops.last().unwrap_or(&0.0);
    scroll.show_viewport(&mut child, |ui, view| {
        ui.set_min_height(total + 8.0);
        let origin = ui.max_rect().min;
        let width = ui.max_rect().width();
        let first = lay.tops.partition_point(|&t| t <= view.min.y).saturating_sub(1);
        let mut hunk_no = diff.lines[..first].iter().filter(|l| l.kind == DiffKind::Hunk).count();
        let pal = theme::pal();
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
                    for (k, range) in rows.iter().enumerate() {
                        let piece = body.get(range.clone()).unwrap_or("");
                        p.text(Pos2::new(tx + mw, y0 + k as f32 * ROW_H), Align2::LEFT_CENTER, piece, font.clone(), fg);
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
}
