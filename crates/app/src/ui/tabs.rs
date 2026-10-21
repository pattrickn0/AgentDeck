//! Faixa de abas do cabeçalho de cada janela da tela dividida (antes ficava na topbar).

use egui::{Align2, Color32, Id, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use commanddeck_core::models::short_model;

use super::{dot, galley_trunc, place, text_in, text_w, tip};
use crate::audio;
use crate::deck::Deck;
use crate::dictation::{self, ButtonVisual, MicTarget};
use crate::icons::Icon;
use crate::theme;

pub enum StripClick {
    Activate(usize),
    Close(usize),
    Rename(usize),
    ContextMenu(usize, Pos2),
    Mic(u64),
}

const TAB_FONT: f32 = 13.0;
const LABEL_MAX: f32 = 140.0;
/// Largura máxima do modelo, em cinza depois do nome (`Claude · Dev  sonnet`).
const MODEL_MAX: f32 = 80.0;
const MODEL_FONT: f32 = 11.5;
const MODEL_GAP: f32 = 6.0;
const MIC_BTN: f32 = 21.0;

/// Pinta as abas `tabs` (índices de `deck.tabs`) em `rect`, com rolagem horizontal quando não
/// cabem. `shown` é a aba visível da janela; `focused`, se a janela tem o foco.
pub fn strip(deck: &mut Deck, ui: &Ui, rect: Rect, tabs: &[usize], shown: usize, focused: bool, scroll_id: Id) -> Option<StripClick> {
    let ctx = ui.ctx().clone();
    let painter = ui.painter_at(rect);
    let font = theme::ui(TAB_FONT);
    let close_w = 12.0 + text_w(&painter, "×", font.clone());
    let widths: Vec<f32> = tabs
        .iter()
        .map(|&i| {
            let tab = &deck.tabs[i];
            let lw = text_w(&painter, &tab.label, font.clone()).min(LABEL_MAX);
            let mw =
                tab.model.as_deref().map_or(0.0, |m| MODEL_GAP + text_w(&painter, short_model(m), theme::ui(MODEL_FONT)).min(MODEL_MAX));
            let status_txt = match dictation::button_visual(deck.mic_phase, MicTarget::Tab(tab.uid)) {
                ButtonVisual::Opening => Some("Abrindo…"),
                ButtonVisual::Transcribing => Some("Transcrevendo…"),
                _ => None,
            };
            let sw = status_txt.map_or(0.0, |s| 6.0 + text_w(&painter, s, theme::ui(MODEL_FONT)));
            12.0 + 8.0 + 8.0 + lw + mw + sw + 4.0 + MIC_BTN + 2.0 + close_w + 6.0 + 1.0
        })
        .collect();
    let total: f32 = widths.iter().sum();

    // Rolagem horizontal (sem barra visível); a aba visível fica sempre à vista.
    let mut scroll = ctx.data(|d| d.get_temp::<f32>(scroll_id)).unwrap_or(0.0);
    if total > rect.width() {
        let over = ui.interact(rect, scroll_id.with("area"), Sense::hover());
        if over.hovered() {
            let d = ui.input(|i| i.smooth_scroll_delta);
            scroll -= d.x + d.y;
        }
        if let Some(a) = tabs.iter().position(|&i| i == shown) {
            let start: f32 = widths[..a].iter().sum();
            let end = start + widths[a];
            if start < scroll {
                scroll = start;
            } else if end > scroll + rect.width() {
                scroll = end - rect.width();
            }
        }
        scroll = scroll.clamp(0.0, total - rect.width());
    } else {
        scroll = 0.0;
    }
    ctx.data_mut(|d| d.insert_temp(scroll_id, scroll));

    let mut x = rect.min.x - scroll;
    let mut click = None;
    for (n, &i) in tabs.iter().enumerate() {
        let tab = &deck.tabs[i];
        let w = widths[n];
        let r = Rect::from_min_size(Pos2::new(x, rect.min.y), Vec2::new(w - 1.0, rect.height()));
        x += w;
        if r.max.x < rect.min.x || r.min.x > rect.max.x {
            continue;
        }
        let vis = r.intersect(rect);
        let is_shown = i == shown;
        let resp = ui.interact(vis, Id::new(("tab", tab.uid)), Sense::click());
        let close_rect = Rect::from_min_size(Pos2::new(r.max.x - 6.0 - close_w, r.center().y - 10.5), Vec2::new(close_w, 21.0));
        let mic_rect = Rect::from_min_size(Pos2::new(close_rect.min.x - 2.0 - MIC_BTN, r.center().y - 10.5), Vec2::splat(MIC_BTN));
        let visual = dictation::button_visual(deck.mic_phase, MicTarget::Tab(tab.uid));
        let mic_busy = visual.is_busy() || !tab.alive;
        let mic_resp =
            ui.interact(mic_rect.intersect(rect), Id::new(("tab-mic", tab.uid)), if mic_busy { Sense::hover() } else { Sense::click() });
        let close_resp = ui.interact(close_rect.intersect(rect), Id::new(("tab-close", tab.uid)), Sense::click());

        let bg = if is_shown {
            Some(theme::BG)
        } else if resp.hovered() || mic_resp.hovered() || close_resp.hovered() {
            Some(theme::PANEL_2)
        } else {
            None
        };
        if let Some(bg) = bg {
            painter.rect_filled(r, 0.0, bg);
        }
        if is_shown {
            // Linha superior na cor do agente (esmaecida se a janela não tem o foco).
            let c = if focused { tab.color } else { theme::with_opacity(tab.color, 0.45) };
            painter.rect_filled(Rect::from_min_size(r.min, Vec2::new(r.width(), 2.0)), 0.0, c);
        }
        painter.rect_filled(Rect::from_min_size(Pos2::new(r.max.x, r.min.y), Vec2::new(1.0, r.height())), 0.0, theme::BORDER);

        let color = if is_shown && focused { theme::TEXT } else { theme::MUTED };
        let dot_pos = Pos2::new(r.min.x + 12.0 + 4.0, r.center().y);
        let is_active = deck.active == Some(i) || (is_shown && focused);
        if tab.busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(33));
            let t = ctx.input(|inp| inp.time);
            let wave = (t * 4.8).sin() as f32 * 0.5 + 0.5;
            let halo_r = 4.0 + 2.8 * wave;
            let halo_alpha = 0.35 * (1.0 - wave);
            painter.circle_filled(dot_pos, halo_r, theme::with_opacity(tab.color, halo_alpha));
            painter.circle_filled(dot_pos, 3.2 + 1.0 * wave, theme::with_opacity(tab.color, 0.75 + 0.25 * wave));
        } else if tab.done && !is_active {
            dot(&painter, dot_pos, tab.color);
            painter.circle_stroke(dot_pos, 6.2, Stroke::new(1.2, theme::brighten(tab.color, 1.25)));
            painter.circle_filled(dot_pos, 1.2, theme::with_opacity(Color32::WHITE, 0.75));
        } else {
            dot(&painter, dot_pos, tab.color);
        }
        let label_x = r.min.x + 12.0 + 8.0 + 8.0;
        let g = galley_trunc(&painter, &tab.label, font.clone(), color, LABEL_MAX);
        let lr = place(
            &painter,
            Rect::from_min_max(Pos2::new(label_x, r.min.y), Pos2::new(label_x + LABEL_MAX, r.max.y)),
            Align2::LEFT_CENTER,
            g,
            color,
        );
        if !tab.alive {
            painter.hline(lr.min.x..=lr.max.x, lr.center().y + 0.5, Stroke::new(1.0, color));
        }
        let mut after_x = lr.max.x;
        if let Some(model) = tab.model.as_deref() {
            let mcolor = theme::with_opacity(theme::MUTED, if tab.alive { 0.75 } else { 0.45 });
            let mg = galley_trunc(&painter, short_model(model), theme::ui(MODEL_FONT), mcolor, MODEL_MAX);
            let x0 = after_x + MODEL_GAP;
            let mr = place(
                &painter,
                Rect::from_min_max(Pos2::new(x0, r.min.y), Pos2::new(x0 + MODEL_MAX, r.max.y)),
                Align2::LEFT_CENTER,
                mg,
                mcolor,
            );
            after_x = mr.max.x;
        }
        let status_txt = match visual {
            ButtonVisual::Opening => Some("Abrindo…"),
            ButtonVisual::Transcribing => Some("Transcrevendo…"),
            _ => None,
        };
        if let Some(st) = status_txt {
            let scolor = theme::MUTED;
            let sg = galley_trunc(&painter, st, theme::ui(MODEL_FONT), scolor, (mic_rect.min.x - after_x - 8.0).max(10.0));
            let x0 = after_x + 6.0;
            place(
                &painter,
                Rect::from_min_max(Pos2::new(x0, r.min.y), Pos2::new(mic_rect.min.x - 2.0, r.max.y)),
                Align2::LEFT_CENTER,
                sg,
                scolor,
            );
        }

        // Botão de microfone da aba
        let mic_hot = mic_resp.hovered() && !mic_busy;
        let mut mic_opacity = 1.0;
        if visual == ButtonVisual::Recording {
            mic_opacity = super::right_panel::blink_alpha(ctx.input(|inp| inp.time), 1.2);
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
            painter.rect_filled(mic_rect, 4.0, theme::with_opacity(theme::RED, mic_opacity));
        } else if mic_hot {
            painter.rect_filled(mic_rect, 4.0, theme::BORDER);
        }
        if mic_busy || !audio::AVAILABLE || visual == ButtonVisual::Opening {
            mic_opacity = 0.5;
        }
        let mic_fg = if visual == ButtonVisual::Recording {
            theme::WHITE
        } else if mic_hot {
            theme::TEXT
        } else {
            theme::MUTED
        };
        deck.icons.paint(&painter, Icon::Mic, mic_rect.center(), 13.5, theme::with_opacity(mic_fg, mic_opacity), 0.0);

        let close_hot = close_resp.hovered();
        if close_hot {
            painter.rect_filled(close_rect, 4.0, theme::BORDER);
        }
        if close_hot || mic_hot || resp.hovered() {
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        text_in(&painter, close_rect, Align2::CENTER_CENTER, "×", font.clone(), if close_hot { theme::TEXT } else { theme::MUTED });

        let mic_tip = if !tab.alive {
            "Ditado indisponível (processo encerrado)".to_owned()
        } else {
            dictation::mic_tooltip(visual, audio::AVAILABLE, &deck.engine_label, Some("Ctrl+Shift+M"))
        };
        let tab_uid = tab.uid;
        let tab_label = tab.label.clone();
        let tab_model = tab.model.clone();
        if tip(close_resp, "Fechar").clicked() {
            click = Some(StripClick::Close(i));
        } else if tip(mic_resp, mic_tip).clicked() && !mic_busy {
            click = Some(StripClick::Mic(tab_uid));
        } else if resp.double_clicked() {
            click = Some(StripClick::Rename(i));
        } else if resp.secondary_clicked() {
            let pos = ui.input(|inp| inp.pointer.latest_pos()).unwrap_or_else(|| r.center());
            click = Some(StripClick::ContextMenu(i, pos));
        } else if tip(resp, tab_tip(&tab_label, tab_model.as_deref())).clicked() {
            click = Some(StripClick::Activate(i));
        }
    }
    click
}

/// Dica da aba: o nome, o modelo (inteiro) e como renomear.
fn tab_tip(label: &str, model: Option<&str>) -> String {
    let model = model.map(|m| format!("\nModelo: {m}")).unwrap_or_default();
    format!("{label}{model}\nDuplo clique ou botão direito para renomear")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tip_names_the_full_model() {
        assert_eq!(tab_tip("Claude · Dev", None), "Claude · Dev\nDuplo clique ou botão direito para renomear");
        assert_eq!(
            tab_tip("OpenCode", Some("opencode/glm-5.3-flash-free")),
            "OpenCode\nModelo: opencode/glm-5.3-flash-free\nDuplo clique ou botão direito para renomear"
        );
    }
}
