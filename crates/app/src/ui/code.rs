//! Os arquivos de texto do modo Documentos como código: o editor (`docs::Source`) e a leitura de um arquivo grande
//! demais para o editor (`docs::TextDoc`).
//!
//! O arquivo que cabe no editor é um `TextEdit` do egui: ele já dá cursor, seleção, copiar/colar e desfazer, na leitura
//! (sobre um `&str`, que não muda) e na edição (sobre a `String`). O realce entra pelo `layouter`, que devolve o galley
//! montado por `docs::Shaped` (guardado por versão do texto: digitar refaz, repintar não). As linhas numeradas ficam
//! numa coluna à parte, alinhadas às linhas do galley. O arquivo grande é pintado linha a linha, só as que aparecem, com
//! a seleção própria de `select.rs`.

use egui::text::CCursor;
use egui::text_edit::TextEditState;
use egui::text_selection::CCursorRange;
use egui::{Align, Align2, Frame, Id, Key, Margin, Modifiers, Pos2, Rect, TextBuffer, Ui, Vec2};

use super::diff_view;
use super::select::{self, Pos, Selected};
use crate::docs::{Find, Shaped, Source, TextDoc, TEXT_FONT, TEXT_ROW_H};
use crate::highlight::{self, Lang};
use crate::theme;

/// Margens do texto dentro do editor: a da direita deixa a barra de rolagem livre; a de baixo deixa rolar além do fim.
const MARGIN: Margin = Margin { left: 6, right: 18, top: 4, bottom: 40 };
/// Quantos espaços vale uma tabulação digitada.
const TAB: usize = 4;

fn gutter_width(cell: f32, lines: usize) -> f32 {
    cell * lines.to_string().len().max(3) as f32 + 22.0
}

/// O arquivo como código. `area` é o corpo todo (a coluna dos números e o texto); com `src.editing` o texto se edita.
pub fn source_body(ui: &mut Ui, area: Rect, uid: u64, src: &mut Source, find: &mut Find, shaped: &mut Shaped) {
    let p = theme::pal();
    let font = theme::mono(TEXT_FONT);
    let cell = ui.fonts_mut(|f| f.glyph_width(&font, 'M')).max(1.0);
    let lines = 1 + src.text.bytes().filter(|b| *b == b'\n').count();
    let gutter = gutter_width(cell, lines);
    let edit = src.editing && src.can_edit();
    if edit {
        ui.painter_at(area).rect_filled(area, 0.0, p.recess);
    }
    if find.open {
        find.refresh(src.version, |q| (highlight::find_all(&src.text, q), Vec::new()));
    }
    let id = Id::new(("doc-src", uid));
    if edit && ui.memory(|m| m.has_focus(id)) {
        indent_key(ui, id, src);
    }

    let view = Rect::from_min_max(Pos2::new(area.min.x + gutter, area.min.y), area.max);
    let lang = src.lang;
    let mut shown = None;
    let mut changed = false;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(view).layout(egui::Layout::top_down(Align::Min)));
    child.set_clip_rect(view);
    egui::ScrollArea::vertical().id_salt(("doc-src-scroll", uid)).auto_shrink([false, false]).show(&mut child, |ui| {
        let size = Vec2::new(view.width(), view.height());
        let out = if edit {
            run_editor(ui, &mut src.text, id, lang, shaped, find, size)
        } else {
            let mut read_only: &str = src.text.as_str();
            run_editor(ui, &mut read_only, id, lang, shaped, find, size)
        };
        changed = edit && out.response.response.changed();
        if find.jump {
            find.jump = false;
            if let Some(r) = find.current_range() {
                let chars = src.text.get(..r.start).map_or(0, |t| t.chars().count());
                let at = out.galley.pos_from_cursor(CCursor::new(chars));
                let rect = Rect::from_min_size(out.galley_pos + at.min.to_vec2(), Vec2::new(2.0, TEXT_ROW_H));
                ui.scroll_to_rect(rect, Some(Align::Center));
            }
        }
        shown = Some((out.galley_pos, out.galley.clone()));
    });
    if changed {
        src.touched();
    }

    // A coluna dos números: uma por linha do texto (as linhas que quebram só levam o número na primeira).
    if let Some((origin, galley)) = shown {
        let gutter_area = Rect::from_min_max(area.min, Pos2::new(area.min.x + gutter, area.max.y));
        let painter = ui.painter_at(gutter_area);
        let color = theme::with_opacity(p.text_2, 0.7);
        let mut number = 0;
        for (i, row) in galley.rows.iter().enumerate() {
            if i > 0 && !galley.rows[i - 1].ends_with_newline {
                continue;
            }
            number += 1;
            let top = origin.y + row.pos.y;
            if top + row.row.size.y < area.min.y || top > area.max.y {
                continue;
            }
            painter.text(
                Pos2::new(gutter_area.max.x - 12.0, top + row.row.size.y / 2.0),
                Align2::RIGHT_CENTER,
                number.to_string(),
                font.clone(),
                color,
            );
        }
    }
}

/// O `TextEdit` do arquivo sobre `buf` (a `String`, para editar, ou um `&str`, para só ler).
fn run_editor(
    ui: &mut Ui,
    buf: &mut dyn TextBuffer,
    id: Id,
    lang: Lang,
    shaped: &mut Shaped,
    find: &Find,
    size: Vec2,
) -> egui::text_edit::TextEditOutput {
    let ctx = ui.ctx().clone();
    let mut layouter = |_: &Ui, text: &dyn TextBuffer, wrap: f32| shaped.galley(&ctx, text.as_str(), lang, wrap, find);
    egui::TextEdit::multiline(buf)
        .id(id)
        .font(theme::mono(TEXT_FONT))
        .frame(Frame::NONE.inner_margin(MARGIN))
        .desired_width(size.x)
        .min_size(size)
        .lock_focus(true)
        .layouter(&mut layouter)
        .show(ui)
}

// ---------------------------------------------------------------------------
// Tab e Shift+Tab
// ---------------------------------------------------------------------------

/// Tab indenta e Shift+Tab desfaz o recuo (a tecla não chega ao `TextEdit`, que inseriria uma tabulação).
fn indent_key(ui: &Ui, id: Id, src: &mut Source) {
    let back = ui.input_mut(|i| i.consume_key(Modifiers::SHIFT, Key::Tab));
    if !back && !ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Tab)) {
        return;
    }
    let mut state = TextEditState::load(ui.ctx(), id).unwrap_or_default();
    let sel = state.cursor.char_range().map_or((0, 0), |r| {
        let (a, b): (usize, usize) = (r.primary.index.into(), r.secondary.index.into());
        (a.min(b), a.max(b))
    });
    if let Some((text, (a, b))) = indent(&src.text, sel, back) {
        src.text = text;
        src.touched();
        state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(a), CCursor::new(b))));
        state.store(ui.ctx(), id);
    }
}

/// Recuo de `TAB` espaços. Sem seleção, Tab insere espaços até a próxima parada; com seleção, indenta as linhas
/// tocadas; Shift+Tab tira até `TAB` espaços (ou uma tabulação) do começo delas. `sel` e o resultado são em caracteres.
/// `None` se nada mudou.
pub fn indent(text: &str, sel: (usize, usize), back: bool) -> Option<(String, (usize, usize))> {
    let chars: Vec<char> = text.chars().collect();
    let (a, b) = (sel.0.min(chars.len()), sel.1.min(chars.len()));
    let line_start = |at: usize| chars[..at].iter().rposition(|c| *c == '\n').map_or(0, |i| i + 1);
    if a == b && !back {
        let col = a - line_start(a);
        let n = TAB - col % TAB;
        let mut out: String = chars[..a].iter().collect();
        out.push_str(&" ".repeat(n));
        out.extend(&chars[a..]);
        return Some((out, (a + n, a + n)));
    }
    // As linhas tocadas: do começo da linha de `a` até a de `b` (a seleção que termina no começo de uma linha não a inclui).
    let first = line_start(a);
    let last_at = if b > a && b > 0 && chars[b - 1] == '\n' { b - 1 } else { b };
    let end = chars[last_at..].iter().position(|c| *c == '\n').map_or(chars.len(), |i| last_at + i);
    let mut out: String = chars[..first].iter().collect();
    let (mut na, mut nb) = (a as isize, b as isize);
    let mut at = first;
    for (n, line) in chars[first..end].split(|c| *c == '\n').enumerate() {
        if n > 0 {
            out.push('\n');
        }
        let delta: isize = if back {
            let drop = if line.first() == Some(&'\t') { 1 } else { line.iter().take(TAB).take_while(|c| **c == ' ').count() };
            out.extend(&line[drop..]);
            -(drop as isize)
        } else if line.is_empty() {
            0
        } else {
            out.push_str(&" ".repeat(TAB));
            out.extend(line);
            TAB as isize
        };
        if delta != 0 {
            // O começo da seleção só anda se estava depois do ponto onde a linha mudou.
            if at + usize::from(back) <= a {
                na += if back { delta.max(-((a - at) as isize)) } else { delta };
            }
            if at < b || (a == b && at <= b) {
                nb += if back { delta.max(-((b.saturating_sub(at)) as isize)) } else { delta };
            }
        }
        at += line.len() + 1;
    }
    out.extend(&chars[end..]);
    (out != text).then(|| (out, (na.max(0) as usize, nb.max(0) as usize)))
}

// ---------------------------------------------------------------------------
// O arquivo grande demais para o editor
// ---------------------------------------------------------------------------

/// O arquivo em monoespaçado, com número de linha, realce, rolagem nos dois sentidos, seleção, cópia e busca; só leitura.
pub fn text_body(ui: &mut Ui, body: Rect, uid: u64, doc: &TextDoc, find: &mut Find) {
    let p = theme::pal();
    let font = theme::mono(TEXT_FONT);
    let cell = ui.fonts_mut(|f| f.glyph_width(&font, 'M')).max(1.0);
    if doc.lines.is_empty() {
        super::docs::note(&ui.painter_at(body), body, "O arquivo está vazio.", p.text_2);
        return;
    }
    let gutter = gutter_width(cell, doc.lines.len());
    let content_w = (gutter + doc.max_chars as f32 * cell + 24.0).max(body.width());
    let total_h = doc.lines.len() as f32 * TEXT_ROW_H;
    if find.open {
        find.refresh(doc.lines.len() as u64, |q| (Vec::new(), highlight::find_lines(doc.lines.iter().map(String::as_str), q)));
    }
    // A busca fechada guarda a consulta (volta ao reabrir), mas não marca nada.
    let current = find.current_line().filter(|_| find.open);
    let sel_id = Id::new(("doc-text-sel", uid));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body).layout(egui::Layout::top_down(Align::Min)));
    child.set_clip_rect(body);
    let mut scroll = egui::ScrollArea::both().id_salt(("doc-text", uid)).auto_shrink([false, false]);
    if std::mem::take(&mut find.jump) {
        if let Some(line) = current {
            scroll = scroll.vertical_scroll_offset((line as f32 * TEXT_ROW_H - body.height() / 2.0).max(0.0));
        }
    }
    scroll.show_viewport(&mut child, |ui, view| {
        ui.set_min_size(Vec2::new(content_w, total_h + 8.0));
        let origin = ui.max_rect().min;
        let painter = ui.painter().clone();
        let end = Pos { line: doc.lines.len() - 1, col: doc.lines[doc.lines.len() - 1].len() };
        let hit = |at: Pos2| -> Pos {
            let line = (((at.y - origin.y) / TEXT_ROW_H).max(0.0) as usize).min(doc.lines.len() - 1);
            let text = &doc.lines[line];
            let g = painter.layout_no_wrap(text.clone(), font.clone(), egui::Color32::WHITE);
            let index = g.cursor_from_pos(Vec2::new((at.x - (origin.x + gutter)).max(0.0), 0.0)).index;
            Pos { line, col: text.char_indices().nth(index.into()).map_or(text.len(), |(i, _)| i) }
        };
        let sel = select::drive(ui, sel_id, body, end, hit, |s| selected_lines(&doc.lines, s));
        let first = ((view.min.y / TEXT_ROW_H) as usize).min(doc.lines.len() - 1);
        let last = ((view.max.y / TEXT_ROW_H).ceil() as usize + 1).min(doc.lines.len());
        let num_color = theme::with_opacity(p.text_2, 0.7);
        for n in first..last {
            let row = Rect::from_min_size(origin + Vec2::new(0.0, n as f32 * TEXT_ROW_H), Vec2::new(content_w, TEXT_ROW_H));
            painter.text(
                Pos2::new(row.min.x + gutter - 12.0, row.center().y),
                Align2::RIGHT_CENTER,
                (n + 1).to_string(),
                font.clone(),
                num_color,
            );
            let line = &doc.lines[n];
            let spans = doc.spans.get(n).map_or(&[][..], Vec::as_slice);
            let hits = if find.open && find.lines.binary_search(&n).is_ok() { highlight::find_all(line, &find.query) } else { Vec::new() };
            let galley = painter.layout_job(diff_view::piece_job(line, &(0..line.len()), spans, &hits, current == Some(n), p.text, &font));
            let at = Pos2::new(row.min.x + gutter, row.min.y);
            if let Some(r) = sel.and_then(|s| s.on_line(n, line.len())).filter(|r| !r.is_empty()) {
                let chars = |byte: usize| line.get(..byte).map_or(0, |t| t.chars().count());
                let (xa, xz) =
                    (galley.pos_from_cursor(CCursor::new(chars(r.start))).min.x, galley.pos_from_cursor(CCursor::new(chars(r.end))).min.x);
                painter.rect_filled(
                    Rect::from_min_max(Pos2::new(at.x + xa, at.y), Pos2::new(at.x + xz.max(xa + 2.0), at.y + TEXT_ROW_H)),
                    0.0,
                    ui.visuals().selection.bg_fill,
                );
            }
            painter.galley(Pos2::new(at.x, at.y + (TEXT_ROW_H - galley.size().y) / 2.0), galley, p.text);
        }
    });
}

/// O texto de uma seleção de linhas inteiras ou parciais, para copiar.
fn selected_lines(lines: &[String], sel: Selected) -> String {
    (sel.start.line..=sel.end.line.min(lines.len().saturating_sub(1)))
        .filter_map(|n| sel.on_line(n, lines[n].len()).map(|r| lines[n][r].to_owned()))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_without_selection_inserts_spaces_up_to_the_next_stop() {
        assert_eq!(indent("ab", (1, 1), false), Some(("a   b".into(), (4, 4))));
        assert_eq!(indent("abcd", (4, 4), false), Some(("abcd    ".into(), (8, 8))));
        assert_eq!(indent("x\ny", (2, 2), false), Some(("x\n    y".into(), (6, 6))));
    }

    #[test]
    fn tab_with_selection_indents_every_touched_line() {
        let (out, sel) = indent("a\nb\nc", (0, 3), false).expect("muda");
        assert_eq!(out, "    a\n    b\nc");
        assert_eq!(sel, (4, 11));
        // A seleção que termina no começo de uma linha não a inclui.
        let (out, _) = indent("a\nb\nc", (0, 2), false).expect("muda");
        assert_eq!(out, "    a\nb\nc");
        // Linha vazia fica como está.
        let (out, _) = indent("a\n\nb", (0, 4), false).expect("muda");
        assert_eq!(out, "    a\n\n    b");
    }

    #[test]
    fn shift_tab_removes_up_to_one_indent_level() {
        assert_eq!(indent("        a", (8, 8), true), Some(("    a".into(), (4, 4))));
        assert_eq!(indent("  a", (3, 3), true), Some(("a".into(), (1, 1))));
        assert_eq!(indent("\ta", (1, 1), true), Some(("a".into(), (0, 0))));
        let (out, _) = indent("    a\n    b", (0, 11), true).expect("muda");
        assert_eq!(out, "a\nb");
        // Sem recuo, nada muda.
        assert_eq!(indent("a", (0, 1), true), None);
    }

    #[test]
    fn indenting_keeps_non_ascii_text_intact() {
        let (out, _) = indent("ação\nvocê", (0, 9), false).expect("muda");
        assert_eq!(out, "    ação\n    você");
    }

    #[test]
    fn the_selected_lines_are_copied_whole_or_in_part() {
        let lines: Vec<String> = ["um", "dois", "três"].iter().map(|s| s.to_string()).collect();
        let sel = Selected { start: Pos { line: 0, col: 1 }, end: Pos { line: 2, col: 2 } };
        assert_eq!(selected_lines(&lines, sel), "m\ndois\ntr");
    }
}
