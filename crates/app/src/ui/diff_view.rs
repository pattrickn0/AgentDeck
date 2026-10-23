//! Desenho de um diff, o mesmo na barra direita e no modo Documentos (como no GitHub e no VS Code com
//! quebra de linha): as linhas quebram na largura visível, com a continuação recuada depois do `+`/`−`, sem
//! rolagem horizontal. As linhas técnicas do git (`diff --git`, `index`, `---`, `+++`) somem; num diff de vários
//! arquivos, cada um ganha um cabeçalho com o caminho.

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
/// Tabulação vira espaços (o desenho conta colunas de largura fixa).
const TAB: &str = "    ";

/// Como uma linha do diff aparece na tela.
#[derive(Debug, Clone, PartialEq)]
enum Shape {
    /// Cabeçalho técnico do git que o cabeçalho do arquivo já resume.
    Hidden,
    /// Início de um arquivo: o caminho (ou "antigo → novo").
    File(String),
    /// Texto em `n` linhas da tela.
    Rows(usize),
}

/// O que se mede uma vez por diff e largura: a forma e a altura acumulada de cada linha.
struct Layout {
    shapes: Vec<Shape>,
    /// `tops[i]` é o y da linha `i`; `tops[n]`, a altura total.
    tops: Vec<f32>,
}

/// Divide `text` em pedaços de até `cols` caracteres (intervalos de bytes). Quebra depois do último espaço do
/// pedaço, se houver, para a palavra ir inteira para a linha seguinte. Texto vazio = um pedaço.
pub fn wrap_ranges(text: &str, cols: usize) -> Vec<Range<usize>> {
    let cols = cols.max(1);
    let starts: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    let n = starts.len();
    let byte = |c: usize| if c < n { starts[c] } else { text.len() };
    let mut out = Vec::new();
    let mut start = 0;
    while n - start > cols {
        let mut end = start + cols;
        if let Some(space) = (start + 1..end).rev().find(|&c| text[byte(c)..].starts_with(' ')) {
            end = space + 1;
        }
        out.push(byte(start)..byte(end));
        start = end;
    }
    out.push(byte(start)..text.len());
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

/// A forma de cada linha para `cols` colunas de texto. As linhas `index`/`---`/`+++` somem; a `diff --git` vira
/// o cabeçalho do arquivo com `headers` (diff de vários arquivos) e some sem ele (o caminho já está no título).
fn shape(line: &DiffLine, cols: usize, marked: bool, headers: bool) -> Shape {
    if line.kind == DiffKind::Meta {
        if let Some(title) = file_title(&line.text) {
            return if headers { Shape::File(title) } else { Shape::Hidden };
        }
        if ["index ", "--- ", "+++ "].iter().any(|p| line.text.starts_with(p)) {
            return Shape::Hidden;
        }
    }
    if line.kind == DiffKind::Hunk {
        return Shape::Rows(1);
    }
    let (marker, body) = split_marker(line, marked);
    let body = body.replace('\t', TAB);
    Shape::Rows(wrap_ranges(&body, cols.saturating_sub(marker.chars().count()).max(1)).len())
}

fn layout(diff: &GitDiff, cols: usize, marked: bool, headers: bool) -> Layout {
    let shapes: Vec<Shape> = diff.lines.iter().map(|l| shape(l, cols, marked, headers)).collect();
    let mut tops = Vec::with_capacity(shapes.len() + 1);
    let mut y = 0.0;
    for (i, s) in shapes.iter().enumerate() {
        tops.push(y);
        y += match s {
            Shape::Hidden => 0.0,
            Shape::File(_) if i == 0 => FILE_H,
            Shape::File(_) => FILE_GAP + FILE_H,
            Shape::Rows(n) => *n as f32 * ROW_H,
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

/// Desenha `diff` em `area`, rolando só na vertical. `id` identifica a rolagem; `jump`, a linha do diff para onde
/// rolar neste quadro. `on_hunk(ui, linha, n)` desenha o que vai por cima do cabeçalho do trecho `n`.
pub fn draw(ui: &mut Ui, area: Rect, diff: &GitDiff, id: Id, jump: Option<usize>, mut on_hunk: impl FnMut(&mut Ui, Rect, usize)) {
    let font = theme::mono(FONT);
    let cell = ui.fonts_mut(|f| f.glyph_width(&font, 'M')).max(1.0);
    let (digits, has_old, has_new) = gutter_cols(diff);
    let num_w = digits as f32 * cell;
    let num_cols = usize::from(has_old) + usize::from(has_new);
    let gutter = 6.0 + num_cols as f32 * (num_w + 8.0) + 4.0;
    let marked = diff.lines.iter().any(|l| l.kind == DiffKind::Hunk);
    let headers = diff.lines.iter().filter(|l| l.kind == DiffKind::Meta && file_title(&l.text).is_some()).count() > 1;
    // A barra de rolagem vertical fica por cima da margem da direita.
    let cols = ((area.width() - gutter - RIGHT_PAD) / cell).floor().max(8.0) as usize;

    // A medida só muda com o diff ou com a largura: fica guardada entre os quadros. O endereço das linhas muda a
    // cada diff carregado (um recarregado com os mesmos tamanhos não reaproveita a medida velha).
    type Fingerprint = (usize, usize, usize, usize, bool, bool);
    let fingerprint: Fingerprint =
        (diff.lines.as_ptr() as usize, diff.lines.len(), diff.lines.iter().map(|l| l.text.len()).sum(), cols, marked, headers);
    let key = id.with("diff-layout");
    let cached = ui.ctx().data(|d| d.get_temp::<(Fingerprint, Arc<Layout>)>(key));
    let lay = match cached {
        Some((f, lay)) if f == fingerprint => lay,
        _ => {
            let lay = Arc::new(layout(diff, cols, marked, headers));
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
                Shape::Rows(_) => {
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
                    if line.kind == DiffKind::Hunk {
                        p.text(Pos2::new(tx, y0), Align2::LEFT_CENTER, line.text.trim_end_matches('\r'), font.clone(), fg);
                        on_hunk(ui, Rect::from_min_size(r.min, Vec2::new(width, ROW_H)), hunk_no);
                        hunk_no += 1;
                        continue;
                    }
                    let (marker, body) = split_marker(line, marked);
                    let mw = marker.chars().count() as f32 * cell;
                    if !marker.is_empty() {
                        p.text(Pos2::new(tx, y0), Align2::LEFT_CENTER, marker, font.clone(), fg);
                    }
                    let body = body.replace('\t', TAB);
                    let body_cols = cols.saturating_sub(marker.chars().count()).max(1);
                    for (k, range) in wrap_ranges(&body, body_cols).into_iter().enumerate() {
                        let y = y0 + k as f32 * ROW_H;
                        p.text(Pos2::new(tx + mw, y), Align2::LEFT_CENTER, &body[range], font.clone(), fg);
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

    #[test]
    fn long_lines_wrap_at_a_space_or_at_the_column() {
        let pieces = |t: &str, c| wrap_ranges(t, c).into_iter().map(|r| t[r].to_owned()).collect::<Vec<_>>();
        assert_eq!(pieces("", 10), vec![""]);
        assert_eq!(pieces("curto", 10), vec!["curto"]);
        // A palavra que não cabe vai inteira para a linha seguinte.
        assert_eq!(pieces("uma frase com palavras", 10), vec!["uma frase ", "com ", "palavras"]);
        // Sem espaço, corta na coluna.
        assert_eq!(pieces("abcdefghijklmnop", 6), vec!["abcdef", "ghijkl", "mnop"]);
        // Acentos contam como um caractere e não partem no meio.
        assert_eq!(pieces("áéíóúçãõ", 3), vec!["áéí", "óúç", "ãõ"]);
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
        assert_eq!(shape(&meta("diff --git a/a b/a"), 40, true, true), Shape::File("a".into()));
        assert_eq!(shape(&meta("index 1..2"), 40, true, true), Shape::Hidden);
        assert_eq!(shape(&meta("+++ b/a"), 40, true, true), Shape::Hidden);
        assert_eq!(shape(&meta("new file mode 100644"), 40, true, true), Shape::Rows(1));
        // Um arquivo só: o caminho já está no título, sem cabeçalho.
        assert_eq!(shape(&meta("diff --git a/a b/a"), 40, true, false), Shape::Hidden);
        assert_eq!(shape(&meta("--- a/a"), 40, true, false), Shape::Hidden);
    }

    #[test]
    fn the_marker_stays_out_of_the_wrapped_text() {
        let add = line(DiffKind::Add, "+abcdefgh");
        assert_eq!(split_marker(&add, true), ("+", "abcdefgh"));
        // 8 caracteres em 5 colunas, menos 1 do marcador: duas linhas.
        assert_eq!(shape(&add, 5, true, false), Shape::Rows(2));
        // Arquivo não rastreado: o texto é o arquivo, sem marcador.
        assert_eq!(split_marker(&line(DiffKind::Add, "+x"), false), ("", "+x"));
        // O cabeçalho do trecho nunca quebra.
        assert_eq!(shape(&line(DiffKind::Hunk, &"@".repeat(90)), 10, true, false), Shape::Rows(1));
    }

    #[test]
    fn heights_add_up_and_the_first_header_has_no_gap() {
        let diff = GitDiff {
            lines: vec![line(DiffKind::Meta, "diff --git a/a b/a"), line(DiffKind::Meta, "index 1"), line(DiffKind::Add, "+uma linha")],
            ..Default::default()
        };
        let lay = layout(&diff, 40, true, true);
        assert_eq!(lay.tops, vec![0.0, FILE_H, FILE_H, FILE_H + ROW_H]);
    }
}
