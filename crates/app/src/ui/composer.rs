//! Compositor (embaixo da barra lateral direita): chips de anexo, campo de texto na largura toda
//! e, embaixo dele, microfone/anexo à esquerda e Enviar à direita. Envia sempre para a aba ativa.

use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use egui::{Align2, Id, Key, Modifiers, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use super::{clickable, frame, galley_trunc, place, text_in, text_w, tip};
use crate::audio;
use crate::deck::Deck;
use crate::icons::Icon;
use crate::theme;
use crate::util;

pub const COMPOSER_INPUT_ID: &str = "composer-input";

const INPUT_FONT: f32 = 13.5;
const INPUT_LINE: f32 = INPUT_FONT * theme::LINE_HEIGHT;
const INPUT_MAX_H: f32 = 200.0;
const CHIP_H: f32 = 21.0;
const BTN: f32 = 31.0;
const PLACEHOLDER: &str = "Mensagem…  (Enter envia, Shift+Enter quebra linha)";

struct Layout {
    chips: Vec<Rect>,
    row: Rect,
    input: Rect,
    cancel: Option<Rect>,
    mic: Rect,
    attach: Rect,
    send: Rect,
    total_h: f32,
}

fn chip_width(painter: &egui::Painter, reference: &str) -> f32 {
    let t = text_w(painter, &util::chip_label(reference), theme::ui(12.0)).min(240.0);
    10.0 + t + 4.0 + (8.0 + text_w(painter, "×", theme::ui(13.0))) + 4.0 + 2.0
}

fn input_height(deck: &Deck, painter: &egui::Painter, width: f32) -> f32 {
    if deck.input.is_empty() {
        return 2.0 * INPUT_LINE + 8.0;
    }
    let mut job = input_job(&deck.input);
    job.wrap.max_width = (width - 12.0).max(10.0);
    let g = painter.layout_job(job);
    let lines = g.rows.len().max(1) as f32;
    let mut h = lines * INPUT_LINE + 8.0;
    if deck.input.ends_with('\n') {
        h = h.max((lines + 0.0) * INPUT_LINE + 8.0);
    }
    h.clamp(2.0 * INPUT_LINE + 8.0, INPUT_MAX_H)
}

fn input_job(text: &str) -> LayoutJob {
    LayoutJob::single_section(
        text.to_owned(),
        TextFormat { font_id: theme::ui(INPUT_FONT), color: theme::INPUT_TEXT, line_height: Some(INPUT_LINE), ..Default::default() },
    )
}

fn layout(deck: &Deck, painter: &egui::Painter, rect: Rect) -> Layout {
    // padding: 8px 10px 10px
    let x0 = rect.min.x + 10.0;
    let x1 = rect.max.x - 10.0;
    let mut y = rect.min.y + 8.0;

    // ---------- Chips de anexo (só quando há anexos; quebram linha) ----------
    let mut chips = Vec::new();
    let mut cx = x0;
    for (i, a) in deck.attachments.iter().enumerate() {
        let w = chip_width(painter, a).min(x1 - x0);
        let gap = if i == 0 { 0.0 } else { 6.0 };
        if cx + gap + w > x1 && cx > x0 {
            y += CHIP_H + 6.0;
            cx = x0;
        } else {
            cx += gap;
        }
        chips.push(Rect::from_min_size(Pos2::new(cx, y), Vec2::new(w, CHIP_H)));
        cx += w;
    }
    if !chips.is_empty() {
        y += CHIP_H + 6.0;
    }

    // ---------- Moldura: campo de texto na largura toda; botões numa linha embaixo dele ----------
    let recording = deck.is_recording();
    let input_x = x0 + 1.0 + 6.0;
    let input_w = (x1 - 1.0 - 6.0 - input_x).max(40.0);
    let ih = input_height(deck, painter, input_w);
    let input = Rect::from_min_size(Pos2::new(input_x, y + 1.0 + 2.0), Vec2::new(input_w, ih));
    let by = input.max.y + 4.0;
    let mut left = input_x;
    let cancel = recording.then(|| {
        let r = Rect::from_min_size(Pos2::new(left, by), Vec2::splat(BTN));
        left = r.max.x + 6.0;
        r
    });
    let mic = Rect::from_min_size(Pos2::new(left, by), Vec2::splat(BTN));
    let attach = Rect::from_min_size(Pos2::new(mic.max.x + 6.0, by), Vec2::splat(BTN));
    let send_w = text_w(painter, deck.send_label(), theme::ui_semibold(12.0)) + 26.0;
    let send = Rect::from_min_size(Pos2::new(input.max.x - send_w, by), Vec2::new(send_w, BTN));
    let row = Rect::from_min_max(Pos2::new(x0, y), Pos2::new(x1, send.max.y + 6.0 + 1.0));
    let total_h = (row.max.y + 10.0) - rect.min.y;
    Layout { chips, row, input, cancel, mic, attach, send, total_h }
}

/// Altura total do compositor para a largura dada.
pub fn measure(deck: &Deck, ui: &Ui, width: f32) -> f32 {
    let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(width, 1000.0));
    layout(deck, ui.painter(), rect).total_h
}

/// Pinta e trata o compositor.
pub fn draw(deck: &mut Deck, ui: &mut Ui, rect: Rect) {
    let painter = ui.painter_at(rect);
    let l = layout(deck, &painter, rect);
    let ctx = ui.ctx().clone();

    // ---------- Chips de anexo ----------
    let mut remove = None;
    for (i, (r, a)) in l.chips.iter().zip(deck.attachments.iter()).enumerate() {
        frame(&painter, *r, 12.0, theme::PANEL_2, theme::BORDER);
        let xw = 8.0 + text_w(&painter, "×", theme::ui(13.0));
        let x_rect = Rect::from_min_size(Pos2::new(r.max.x - 1.0 - 4.0 - xw, r.min.y + 1.0), Vec2::new(xw, r.height() - 2.0));
        let tr = Rect::from_min_max(Pos2::new(r.min.x + 11.0, r.min.y), Pos2::new(x_rect.min.x - 4.0, r.max.y));
        let g = galley_trunc(&painter, &util::chip_label(a), theme::ui(12.0), theme::CHIP_TEXT, tr.width());
        place(&painter, tr, Align2::LEFT_CENTER, g, theme::CHIP_TEXT);
        let chip_resp = ui.interact(*r, Id::new(("chip", i)), Sense::hover());
        tip(chip_resp, a.clone());
        let xr = clickable(ui, x_rect, Id::new(("chip-x", i)));
        text_in(&painter, x_rect, Align2::CENTER_CENTER, "×", theme::ui(13.0), if xr.hovered() { theme::TEXT } else { theme::MUTED });
        if tip(xr, "Remover").clicked() {
            remove = Some(i);
        }
    }
    if let Some(i) = remove {
        deck.attachments.remove(i);
    }

    // ---------- Linha do compositor ----------
    let border = if deck.needs_text { theme::RED } else { theme::BORDER };
    frame(&painter, l.row, 10.0, theme::PANEL, border);
    input_field(deck, ui, l.input, &ctx);

    // Microfone / cancelar
    let recording = deck.is_recording();
    if let Some(c) = l.cancel {
        let r = clickable(ui, c, Id::new("mic-cancel"));
        let (bg, fg) = if r.hovered() { (theme::RED, theme::WHITE) } else { (theme::rgba(229, 72, 77, 0.15), theme::RED_TEXT) };
        painter.rect(
            c,
            7.0,
            bg,
            Stroke::new(1.0, if r.hovered() { theme::RED } else { theme::rgba(229, 72, 77, 0.35) }),
            egui::StrokeKind::Inside,
        );
        deck.icons.paint(&painter, Icon::Close, c.center(), 15.0, fg, 0.0);
        if tip(r, "Cancelar gravação").clicked() {
            deck.cancel_recording();
        }
    }
    let busy = deck.transcribing;
    let mr = if busy { ui.interact(l.mic, Id::new("mic"), Sense::hover()) } else { clickable(ui, l.mic, Id::new("mic")) };
    let (mut bg, mut fg) = if mr.hovered() && !busy { (theme::BORDER, theme::TEXT) } else { (theme::PANEL_2, theme::MUTED) };
    let mut opacity = 1.0;
    if recording {
        bg = theme::RED;
        fg = theme::WHITE;
        // animation: blink 1.2s infinite
        opacity = super::right_panel::blink_alpha(ctx.input(|i| i.time), 1.2);
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }
    if busy || !audio::AVAILABLE {
        opacity = 0.5;
    }
    painter.rect_filled(l.mic, 7.0, theme::with_opacity(bg, opacity));
    deck.icons.paint(&painter, Icon::Mic, l.mic.center(), 17.0, theme::with_opacity(fg, opacity), 0.0);
    let mic_title = if recording {
        "Parar gravação".to_owned()
    } else if !audio::AVAILABLE {
        "Gravar áudio (indisponível: compilado sem o recurso de áudio)".to_owned()
    } else {
        format!("Gravar áudio (transcrição: {})", if deck.engine_label.is_empty() { "…" } else { &deck.engine_label })
    };
    if tip(mr, mic_title).clicked() && !busy {
        deck.toggle_mic();
    }

    // Anexar
    let ar = clickable(ui, l.attach, Id::new("attach"));
    let (bg, fg) = if ar.hovered() { (theme::BORDER, theme::TEXT) } else { (theme::PANEL_2, theme::MUTED) };
    painter.rect_filled(l.attach, 7.0, bg);
    deck.icons.paint(&painter, Icon::Attach, l.attach.center(), 17.0, fg, 0.0);
    if tip(ar, "Anexar arquivos").clicked() {
        deck.pick_attachments();
    }

    // Enviar / Enviando…
    let ready = deck.send_ready();
    let sr = if ready { clickable(ui, l.send, Id::new("send")) } else { ui.interact(l.send, Id::new("send"), Sense::click()) };
    let (mut bg, fg) = if ready {
        (deck.send_color(), theme::WHITE)
    } else {
        (theme::PANEL_2, theme::SEND_IDLE_TEXT)
    };
    if ready && sr.hovered() {
        bg = theme::brighten(bg, 1.15);
    }
    painter.rect_filled(l.send, 7.0, bg);
    text_in(&painter, l.send, Align2::CENTER_CENTER, deck.send_label(), theme::ui_semibold(12.0), fg);
    if sr.clicked() {
        deck.send_message();
    }
}

fn input_field(deck: &mut Deck, ui: &mut Ui, r: Rect, ctx: &egui::Context) {
    let id = Id::new(COMPOSER_INPUT_ID);
    let focused = ctx.memory(|m| m.has_focus(id));
    // Enter envia, Shift+Enter quebra linha.
    let send = focused && ui.input_mut(|i| !i.modifiers.shift && !i.modifiers.ctrl && i.consume_key(Modifiers::NONE, Key::Enter));

    if std::mem::take(&mut deck.focus_composer) {
        ctx.memory_mut(|m| m.request_focus(id));
    }
    if std::mem::take(&mut deck.composer_cursor_end) {
        if let Some(mut state) = egui::TextEdit::load_state(ctx, id) {
            let end = deck.input.chars().count();
            state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(end))));
            egui::TextEdit::store_state(ctx, id, state);
        }
    }

    let placeholder = if deck.transcribing { "Transcrevendo áudio…" } else { PLACEHOLDER };
    let ph_color = if deck.needs_text { theme::RED } else { theme::PLACEHOLDER };
    let before = deck.input.clone();
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
    child.set_clip_rect(r);
    let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, wrap: f32| {
        let mut job = input_job(buf.as_str());
        job.wrap.max_width = wrap;
        ui.fonts_mut(|f| f.layout_job(job))
    };
    egui::ScrollArea::vertical().id_salt("composer-scroll").auto_shrink([false, false]).max_height(r.height()).show(&mut child, |ui| {
        let te = egui::TextEdit::multiline(&mut deck.input)
            .id(id)
            .frame(egui::Frame::NONE)
            .margin(egui::Margin { left: 10, right: 10, top: 7, bottom: 7 })
            .hint_text(egui::RichText::new(placeholder).font(theme::ui(INPUT_FONT)).color(ph_color))
            .desired_width(ui.available_width())
            .desired_rows(2)
            .min_size(Vec2::new(ui.available_width(), r.height()))
            .layouter(&mut layouter);
        let out = te.show(ui);
        if out.response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        }
    });
    if deck.input != before {
        deck.needs_text = false;
    }
    if send {
        deck.send_message();
    }
}
