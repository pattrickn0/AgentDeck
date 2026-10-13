//! Pintura da grade do terminal no egui + mouse (seleção, rolagem, roda).

use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::CursorShape;
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};

use super::{colors, GridSize, Session};
use crate::theme;

/// `.term { padding: 6px 4px 6px 10px }`
pub const PAD_LEFT: f32 = 10.0;
pub const PAD_TOP: f32 = 6.0;
pub const PAD_RIGHT: f32 = 4.0;
pub const PAD_BOTTOM: f32 = 6.0;
/// Largura da barra de rolagem do viewport do xterm (reservada pelo FitAddon).
pub const SCROLLBAR_W: f32 = 5.0;

/// Métricas da célula para a fonte do terminal.
#[derive(Debug, Clone)]
pub struct Metrics {
    pub cell_w: f32,
    pub cell_h: f32,
    pub font: FontId,
    pub bold: FontId,
}

impl Metrics {
    pub fn measure(ctx: &egui::Context) -> Self {
        let font = theme::mono(theme::TERM_FONT_SIZE);
        let bold = theme::mono_bold(theme::TERM_FONT_SIZE);
        let (w, h) = ctx.fonts_mut(|f| (f.glyph_width(&font, 'M'), f.row_height(&font)));
        Self { cell_w: if w > 0.0 { w } else { 8.4 }, cell_h: h.ceil().max(1.0), font, bold }
    }

    /// Colunas × linhas que cabem na área (equivalente ao `FitAddon.proposeDimensions`).
    pub fn grid_size_for(&self, rect: Rect) -> GridSize {
        let w = rect.width() - PAD_LEFT - PAD_RIGHT - SCROLLBAR_W;
        let h = rect.height() - PAD_TOP - PAD_BOTTOM;
        GridSize { cols: ((w / self.cell_w).floor().max(2.0)) as usize, rows: ((h / self.cell_h).floor().max(2.0)) as usize }
    }
}

/// Estado de interação por aba.
#[derive(Default)]
pub struct ViewState {
    wheel_accum: f32,
    dragging_scrollbar: Option<(f32, usize)>,
}

/// Resultado da interação no quadro atual.
#[derive(Default)]
pub struct ViewResponse {
    /// O usuário clicou na área (deve receber o foco do teclado).
    pub focus: bool,
    /// Bytes a enviar ao processo (ex.: roda do mouse em modo de relatório/tela alternativa).
    pub to_pty: Vec<u8>,
}

/// Pinta o terminal e trata o mouse. `focused` = recebe teclado; `blink_on` = fase do cursor.
#[allow(clippy::too_many_arguments)]
pub fn show(ui: &mut egui::Ui, rect: Rect, id: egui::Id, session: &Session, state: &mut ViewState, focused: bool, blink_on: bool, m: &Metrics) -> ViewResponse {
    let mut out = ViewResponse::default();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::TERM_BG);

    let origin = Pos2::new(rect.min.x + PAD_LEFT, rect.min.y + PAD_TOP);
    let response = ui.interact(rect, id, Sense::click_and_drag());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
    }

    let mut emu = session.shared.emu.lock();
    let rows = emu.term.screen_lines();
    let cols = emu.term.columns();
    let history = emu.term.grid().history_size();

    // ---------- Barra de rolagem ----------
    let track = Rect::from_min_max(
        Pos2::new(rect.max.x - PAD_RIGHT - SCROLLBAR_W, rect.min.y + PAD_TOP),
        Pos2::new(rect.max.x - PAD_RIGHT, rect.min.y + PAD_TOP + rows as f32 * m.cell_h),
    );
    let mut thumb = None;
    if history > 0 {
        let total = (history + rows) as f32;
        let offset = emu.term.grid().display_offset();
        let h = (rows as f32 / total * track.height()).max(20.0).min(track.height());
        let top = track.min.y + (history - offset) as f32 / history as f32 * (track.height() - h);
        thumb = Some(Rect::from_min_size(Pos2::new(track.min.x, top), Vec2::new(SCROLLBAR_W, h)));
    }
    let pointer = ui.input(|i| i.pointer.interact_pos());
    let on_scrollbar = pointer.is_some_and(|p| track.expand2(Vec2::new(1.0, 0.0)).contains(p)) && thumb.is_some();

    // ---------- Mouse ----------
    let to_point = |p: Pos2, offset: usize| -> (Point, Side) {
        let x = ((p.x - origin.x) / m.cell_w).max(0.0);
        let col = (x.floor() as usize).min(cols.saturating_sub(1));
        let side = if x.fract() < 0.5 { Side::Left } else { Side::Right };
        let row = (((p.y - origin.y) / m.cell_h).floor().max(0.0) as usize).min(rows.saturating_sub(1));
        (Point::new(Line(row as i32 - offset as i32), Column(col)), side)
    };

    if response.drag_started() || response.clicked() || response.double_clicked() || response.triple_clicked() {
        out.focus = true;
    }
    if response.drag_started() {
        if let (true, Some(p), Some(t)) = (on_scrollbar, pointer, thumb) {
            let offset = emu.term.grid().display_offset();
            let grab = if t.contains(p) { p.y - t.min.y } else { t.height() / 2.0 };
            state.dragging_scrollbar = Some((grab, offset));
        } else if let Some(p) = response.interact_pointer_pos() {
            let offset = emu.term.grid().display_offset();
            let (pt, side) = to_point(p, offset);
            emu.term.selection = Some(Selection::new(SelectionType::Simple, pt, side));
        }
    }
    if response.dragged() {
        if let (Some((grab, _)), Some(p), Some(t)) = (state.dragging_scrollbar, pointer, thumb) {
            let free = (track.height() - t.height()).max(1.0);
            let frac = ((p.y - grab - track.min.y) / free).clamp(0.0, 1.0);
            let target = history as f32 - frac * history as f32;
            let cur = emu.term.grid().display_offset() as f32;
            emu.term.scroll_display(Scroll::Delta((target - cur).round() as i32));
        } else if let Some(p) = response.interact_pointer_pos() {
            // Rolagem automática ao arrastar para fora da área.
            if p.y < rect.min.y {
                emu.term.scroll_display(Scroll::Delta(1));
            } else if p.y > rect.max.y {
                emu.term.scroll_display(Scroll::Delta(-1));
            }
            let offset = emu.term.grid().display_offset();
            let (pt, side) = to_point(p, offset);
            if let Some(sel) = emu.term.selection.as_mut() {
                sel.update(pt, side);
            }
        }
    }
    if response.drag_stopped() {
        state.dragging_scrollbar = None;
        if emu.term.selection.as_ref().is_some_and(|s| s.is_empty()) {
            emu.term.selection = None;
        }
    }
    if response.double_clicked() || response.triple_clicked() {
        if let Some(p) = response.interact_pointer_pos() {
            let offset = emu.term.grid().display_offset();
            let (pt, side) = to_point(p, offset);
            let ty = if response.triple_clicked() { SelectionType::Lines } else { SelectionType::Semantic };
            emu.term.selection = Some(Selection::new(ty, pt, side));
        }
    } else if response.clicked() && !on_scrollbar {
        emu.term.selection = None;
    }

    // Roda do mouse.
    if response.hovered() {
        let mode = *emu.term.mode();
        let events = ui.input(|i| i.events.clone());
        for ev in events {
            if let egui::Event::MouseWheel { unit, delta, .. } = ev {
                // Uma "linha" de roda ≈ 100 px no Chromium; o xterm converte pixels em linhas pela altura da célula.
                let px = match unit {
                    egui::MouseWheelUnit::Point => delta.y,
                    egui::MouseWheelUnit::Line => delta.y * 100.0,
                    egui::MouseWheelUnit::Page => delta.y * rect.height(),
                };
                state.wheel_accum += px / m.cell_h;
                let lines = state.wheel_accum.trunc() as i32;
                if lines == 0 {
                    continue;
                }
                state.wheel_accum -= lines as f32;
                if mode.intersects(TermMode::MOUSE_MODE) {
                    if let Some(p) = pointer {
                        let (pt, _) = to_point(p, 0);
                        let (c, r) = (pt.column.0 + 1, pt.line.0 + 1);
                        let button = if lines > 0 { 64 } else { 65 };
                        for _ in 0..lines.unsigned_abs().min(10) {
                            if mode.contains(TermMode::SGR_MOUSE) {
                                out.to_pty.extend(format!("\x1b[<{button};{c};{r}M").into_bytes());
                            } else if c < 224 && r < 224 {
                                out.to_pty.extend([0x1b, b'[', b'M', 32 + button as u8, 32 + c as u8, 32 + r as u8]);
                            }
                        }
                    }
                } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
                    let seq: &[u8] = match (lines > 0, mode.contains(TermMode::APP_CURSOR)) {
                        (true, true) => b"\x1bOA",
                        (true, false) => b"\x1b[A",
                        (false, true) => b"\x1bOB",
                        (false, false) => b"\x1b[B",
                    };
                    for _ in 0..lines.unsigned_abs() {
                        out.to_pty.extend_from_slice(seq);
                    }
                } else {
                    emu.term.scroll_display(Scroll::Delta(lines));
                }
            }
        }
    }

    // ---------- Pintura ----------
    let content = emu.term.renderable_content();
    let offset = content.display_offset;
    let overrides = content.colors;
    let cursor = content.cursor;
    let selection = content.selection;
    let cursor_row = cursor.point.line.0 + offset as i32;
    let cursor_visible = cursor.shape != CursorShape::Hidden && cursor_row >= 0 && (cursor_row as usize) < rows;

    let mut bgs: Vec<(Rect, Color32)> = Vec::new();
    let mut texts: Vec<(Pos2, LayoutJob, Option<f32>)> = Vec::new();
    let mut lines: Vec<(Pos2, f32, Color32, bool)> = Vec::new(); // (início, largura, cor, tachado?)
    let mut run: Option<Run> = None;
    let mut cursor_cell: Option<(char, bool, Color32)> = None;

    for indexed in content.display_iter {
        let row = indexed.point.line.0 + offset as i32;
        if row < 0 || row as usize >= rows {
            continue;
        }
        let col = indexed.point.column.0;
        let cell = indexed.cell;
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) || cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
            continue;
        }
        let width_cells = if cell.flags.contains(Flags::WIDE_CHAR) { 2.0 } else { 1.0 };
        let pos = Pos2::new(origin.x + col as f32 * m.cell_w, origin.y + row as f32 * m.cell_h);
        let (fg, bg) = colors::cell_colors(cell.fg, cell.bg, cell.flags, overrides);
        let selected = selection.as_ref().is_some_and(|s| s.contains_cell(&indexed, cursor.point, cursor.shape));
        let bg = if selected { Some(theme::TERM_SELECTION) } else { bg };
        if let Some(bg) = bg {
            bgs.push((Rect::from_min_size(pos, Vec2::new(m.cell_w * width_cells, m.cell_h)), bg));
        }
        let is_cursor = cursor_visible && indexed.point == cursor.point;
        let bold = cell.flags.contains(Flags::BOLD);
        if is_cursor {
            cursor_cell = Some((cell.c, bold, bg.unwrap_or(theme::TERM_BG)));
        }
        let underline = cell.flags.intersects(Flags::ALL_UNDERLINES);
        let strike = cell.flags.contains(Flags::STRIKEOUT);
        if underline || strike {
            lines.push((pos, m.cell_w * width_cells, fg, strike && !underline));
        }
        let c = cell.c;
        if c == ' ' || c == '\t' || c == '\0' || fg == Color32::TRANSPARENT {
            continue;
        }
        let style = RunStyle { fg, bold, italic: cell.flags.contains(Flags::ITALIC) };
        let ascii = c.is_ascii() && width_cells == 1.0;
        if let Some(r) = run.as_mut() {
            if ascii && r.ascii && r.row == row && r.style == style && r.next_col == col {
                r.text.push(c);
                r.next_col += 1;
                continue;
            }
        }
        if let Some(r) = run.take() {
            texts.push(r.into_job(m));
        }
        let mut text = String::new();
        text.push(c);
        if let Some(extra) = cell.zerowidth() {
            text.extend(extra.iter());
        }
        run = Some(Run { row, start: pos, next_col: col + 1, text, style, ascii, width_cells });
    }
    if let Some(r) = run.take() {
        texts.push(r.into_job(m));
    }

    for (r, c) in bgs {
        painter.rect_filled(r, 0.0, c);
    }
    for (pos, job, span) in texts {
        let galley = painter.layout_job(job);
        // Caracteres largos/fora do ASCII: centraliza na(s) célula(s) que ocupam.
        let dx = span.map(|w| (w - galley.size().x) / 2.0).unwrap_or(0.0);
        painter.galley(pos + Vec2::new(dx, 0.0), galley, theme::TERM_FG);
    }
    for (pos, w, color, strike) in lines {
        let y = if strike { pos.y + m.cell_h * 0.5 } else { pos.y + m.cell_h - 1.5 };
        painter.hline(pos.x..=pos.x + w, y, Stroke::new(1.0, color));
    }

    // Cursor.
    if cursor_visible {
        let pos = Pos2::new(origin.x + cursor.point.column.0 as f32 * m.cell_w, origin.y + cursor_row as f32 * m.cell_h);
        let cell_rect = Rect::from_min_size(pos, Vec2::new(m.cell_w, m.cell_h));
        if !focused {
            painter.rect_stroke(cell_rect.shrink(0.5), 0.0, Stroke::new(1.0, theme::TERM_CURSOR), egui::StrokeKind::Inside);
        } else if blink_on {
            match cursor.shape {
                CursorShape::Beam => {
                    painter.rect_filled(Rect::from_min_size(pos, Vec2::new(1.0, m.cell_h)), 0.0, theme::TERM_CURSOR);
                }
                CursorShape::Underline => {
                    painter.rect_filled(Rect::from_min_size(Pos2::new(pos.x, pos.y + m.cell_h - 2.0), Vec2::new(m.cell_w, 2.0)), 0.0, theme::TERM_CURSOR);
                }
                CursorShape::HollowBlock => {
                    painter.rect_stroke(cell_rect.shrink(0.5), 0.0, Stroke::new(1.0, theme::TERM_CURSOR), egui::StrokeKind::Inside);
                }
                _ => {
                    painter.rect_filled(cell_rect, 0.0, theme::TERM_CURSOR);
                    if let Some((c, bold, bg)) = cursor_cell {
                        if c != ' ' && c != '\0' {
                            let font = if bold { m.bold.clone() } else { m.font.clone() };
                            painter.text(pos, egui::Align2::LEFT_TOP, c, font, bg);
                        }
                    }
                }
            }
        }
    }

    if let Some(t) = thumb {
        let hot = on_scrollbar || state.dragging_scrollbar.is_some();
        painter.rect_filled(t, 3.0, theme::rgba(255, 255, 255, if hot { 0.25 } else { 0.12 }));
    }

    out
}

#[derive(Clone, Copy, PartialEq)]
struct RunStyle {
    fg: Color32,
    bold: bool,
    italic: bool,
}

struct Run {
    row: i32,
    start: Pos2,
    next_col: usize,
    text: String,
    style: RunStyle,
    ascii: bool,
    width_cells: f32,
}

impl Run {
    fn into_job(self, m: &Metrics) -> (Pos2, LayoutJob, Option<f32>) {
        let font = if self.style.bold { m.bold.clone() } else { m.font.clone() };
        let mut job = LayoutJob::default();
        job.wrap.max_width = f32::INFINITY;
        job.append(&self.text, 0.0, TextFormat { font_id: font, color: self.style.fg, italics: self.style.italic, ..Default::default() });
        let span = (!self.ascii).then_some(m.cell_w * self.width_cells);
        (self.start, job, span)
    }
}
