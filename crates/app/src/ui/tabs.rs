//! Faixa de abas do cabeçalho de cada janela da tela dividida (antes ficava na topbar).

use egui::{Align2, Area, Color32, Id, Order, Painter, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use commanddeck_core::models::short_model;

use super::menus::{closes_on_outside, keep_on_screen};
use super::{clickable, dot, frame, galley_trunc, place, shadow, text_in, text_w, tip};
use crate::deck::Deck;
use crate::dictation::{self, ButtonVisual, MicTarget};
use crate::theme;

pub enum StripClick {
    Activate(usize),
    Close(usize),
    Rename(usize),
    ContextMenu(usize, Pos2),
}

const TAB_FONT: f32 = 13.0;
const LABEL_MAX: f32 = 140.0;
/// Largura máxima do modelo, em cinza depois do nome (`Claude · Dev  sonnet`).
const MODEL_MAX: f32 = 80.0;
const MODEL_FONT: f32 = 11.5;
const MODEL_GAP: f32 = 6.0;
/// Espaço do indicador vermelho pulsante quando a aba está gravando áudio.
const REC_INDICATOR_W: f32 = 14.0;
/// Seta de rolagem das abas escondidas: botão 20×20 (como os do cabeçalho) sobre fundo sólido,
/// seguido do degradê que o liga às abas.
const ARROW_BTN: f32 = 20.0;
const ARROW_ZONE_W: f32 = 24.0;
const FADE_W: f32 = 18.0;
/// Espaço do botão "▾" (todas as abas), reservado no fim da faixa quando ela rola.
const LIST_W: f32 = 26.0;
const LIST_MENU_W: f32 = 260.0;
const LIST_ITEM_H: f32 = 26.0;
const LIST_MAX_ROWS: usize = 14;
/// Folga ao decidir se uma aba está inteira à vista (evita seta por causa de fração de pixel).
const EDGE_EPS: f32 = 0.5;

/// Abas que não aparecem inteiras em cada lado da faixa (posição na lista da janela).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Hidden {
    /// A mais próxima da borda esquerda.
    left: Option<usize>,
    /// A mais próxima da borda direita.
    right: Option<usize>,
}

/// Quais abas estão cortadas ou fora da área de largura `view_w`, com a faixa rolada em `scroll`.
/// De um lado que tem aba escondida entra a seta, que cobre `edge` px da borda: ali a aba que só
/// aparece debaixo dela também conta como escondida.
fn hidden_tabs(widths: &[f32], scroll: f32, view_w: f32, edge: f32) -> Hidden {
    let (mut cut_left, mut cut_right) = (false, false);
    let mut start = 0.0;
    for w in widths {
        cut_left |= start < scroll - EDGE_EPS;
        cut_right |= start + w > scroll + view_w + EDGE_EPS;
        start += w;
    }
    let mut hidden = Hidden::default();
    let mut start = 0.0;
    for (n, w) in widths.iter().enumerate() {
        let end = start + w;
        if cut_left && start < scroll + edge - EDGE_EPS {
            hidden.left = Some(n);
        }
        if cut_right && hidden.right.is_none() && end > scroll + view_w - edge + EDGE_EPS {
            hidden.right = Some(n);
        }
        start = end;
    }
    hidden
}

/// Rolagem mínima, a partir de `scroll`, que deixa a aba `n` inteira à vista, fora das setas (que
/// cobrem `edge` px de cada borda onde há mais abas); limitada ao conteúdo.
fn scroll_to_show(widths: &[f32], n: usize, scroll: f32, view_w: f32, edge: f32) -> f32 {
    let total: f32 = widths.iter().sum();
    let start: f32 = widths[..n].iter().sum();
    let end = start + widths[n];
    let left = if n > 0 { edge } else { 0.0 };
    let right = if n + 1 < widths.len() { edge } else { 0.0 };
    let target = if start - left < scroll {
        start - left
    } else if end + right > scroll + view_w {
        end + right - view_w
    } else {
        scroll
    };
    target.clamp(0.0, (total - view_w).max(0.0))
}

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
            let sw = match dictation::button_visual(deck.mic_phase, MicTarget::Tab(tab.uid)) {
                ButtonVisual::Recording => REC_INDICATOR_W,
                ButtonVisual::Opening => 6.0 + text_w(&painter, "Abrindo…", theme::ui(MODEL_FONT)),
                ButtonVisual::Transcribing => 6.0 + text_w(&painter, "Transcrevendo…", theme::ui(MODEL_FONT)),
                _ => 0.0,
            };
            12.0 + 8.0 + 8.0 + lw + mw + sw + 6.0 + close_w + 6.0 + 1.0
        })
        .collect();
    let total: f32 = widths.iter().sum();

    let overflow = total > rect.width();
    // Com a faixa rolando, o fim dela fica para o botão "▾"; as abas só aparecem em `view`.
    let view = if overflow { Rect::from_min_max(rect.min, Pos2::new(rect.max.x - LIST_W, rect.max.y)) } else { rect };

    // Rolagem horizontal (sem barra visível). A aba visível é trazida à vista quando muda (ou
    // quando a largura muda); fora isso o usuário rola à vontade, pela roda ou pelas setas.
    let mut scroll = ctx.data(|d| d.get_temp::<f32>(scroll_id)).unwrap_or(0.0);
    if overflow {
        let over = ui.interact(rect, scroll_id.with("area"), Sense::hover());
        if over.hovered() {
            let d = ui.input(|i| i.smooth_scroll_delta);
            scroll -= d.x + d.y;
        }
        let key = (shown, view.width().round() as i32);
        if ctx.data(|d| d.get_temp::<(usize, i32)>(scroll_id.with("seen"))) != Some(key) {
            ctx.data_mut(|d| d.insert_temp(scroll_id.with("seen"), key));
            if let Some(a) = tabs.iter().position(|&i| i == shown) {
                scroll = scroll_to_show(&widths, a, scroll, view.width(), ARROW_ZONE_W);
            }
        }
        scroll = scroll.clamp(0.0, (total - view.width()).max(0.0));
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
        if r.max.x < view.min.x || r.min.x > view.max.x {
            continue;
        }
        let vis = r.intersect(view);
        let is_shown = i == shown;
        let resp = ui.interact(vis, Id::new(("tab", tab.uid)), Sense::click());
        let close_rect = Rect::from_min_size(Pos2::new(r.max.x - 6.0 - close_w, r.center().y - 10.5), Vec2::new(close_w, 21.0));
        let visual = dictation::button_visual(deck.mic_phase, MicTarget::Tab(tab.uid));
        let close_resp = ui.interact(close_rect.intersect(view), Id::new(("tab-close", tab.uid)), Sense::click());

        let bg = if is_shown {
            Some(theme::BG)
        } else if resp.hovered() || close_resp.hovered() {
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
        // Indicador de estado do ditado na aba (o botão fica no cabeçalho do painel).
        match visual {
            ButtonVisual::Recording => {
                let alpha = super::right_panel::blink_alpha(ctx.input(|inp| inp.time), 1.2);
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
                let rec_pos = Pos2::new(after_x + 10.0, r.center().y);
                painter.circle_filled(rec_pos, 5.5, theme::with_opacity(theme::RED, alpha * 0.28));
                painter.circle_filled(rec_pos, 3.6, theme::with_opacity(theme::RED, alpha));
            }
            ButtonVisual::Opening | ButtonVisual::Transcribing => {
                let st = if visual == ButtonVisual::Opening { "Abrindo…" } else { "Transcrevendo…" };
                let scolor = theme::MUTED;
                let sg = galley_trunc(&painter, st, theme::ui(MODEL_FONT), scolor, (close_rect.min.x - after_x - 8.0).max(10.0));
                let x0 = after_x + 6.0;
                place(
                    &painter,
                    Rect::from_min_max(Pos2::new(x0, r.min.y), Pos2::new(close_rect.min.x - 2.0, r.max.y)),
                    Align2::LEFT_CENTER,
                    sg,
                    scolor,
                );
            }
            _ => {}
        }

        let close_hot = close_resp.hovered();
        if close_hot {
            painter.rect_filled(close_rect, 4.0, theme::BORDER);
        }
        if close_hot || resp.hovered() {
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        text_in(&painter, close_rect, Align2::CENTER_CENTER, "×", font.clone(), if close_hot { theme::TEXT } else { theme::MUTED });

        let tab_label = tab.label.clone();
        let tab_model = tab.model.clone();
        if tip(close_resp, "Fechar").clicked() {
            click = Some(StripClick::Close(i));
        } else if resp.double_clicked() {
            click = Some(StripClick::Rename(i));
        } else if resp.secondary_clicked() {
            let pos = ui.input(|inp| inp.pointer.latest_pos()).unwrap_or_else(|| r.center());
            click = Some(StripClick::ContextMenu(i, pos));
        } else if tip(resp, tab_tip(&tab_label, tab_model.as_deref())).clicked() {
            click = Some(StripClick::Activate(i));
        }
    }

    if overflow {
        let hidden = hidden_tabs(&widths, scroll, view.width(), ARROW_ZONE_W);
        let mut new_scroll = None;
        if let Some(n) = hidden.left {
            let zone = Rect::from_min_size(rect.min, Vec2::new(ARROW_ZONE_W, rect.height()));
            if edge_arrow(deck, ui, &painter, zone, Dir::Left, tabs[n], scroll_id.with("arrow-left")) {
                new_scroll = Some(scroll_to_show(&widths, n, scroll, view.width(), ARROW_ZONE_W));
            }
        }
        if let Some(n) = hidden.right {
            let zone = Rect::from_min_max(Pos2::new(view.max.x - ARROW_ZONE_W, rect.min.y), Pos2::new(view.max.x, rect.max.y));
            if edge_arrow(deck, ui, &painter, zone, Dir::Right, tabs[n], scroll_id.with("arrow-right")) {
                new_scroll = Some(scroll_to_show(&widths, n, scroll, view.width(), ARROW_ZONE_W));
            }
        }
        if let Some(s) = new_scroll {
            ctx.data_mut(|d| d.insert_temp(scroll_id, s));
            ctx.request_repaint();
        }
        let list = Rect::from_min_max(Pos2::new(view.max.x, rect.min.y), rect.max);
        if let Some(i) = tab_list(deck, ui, &painter, list, tabs, shown, scroll_id.with("list")) {
            click = Some(StripClick::Activate(i));
        }
    } else {
        ctx.data_mut(|d| d.remove::<bool>(scroll_id.with("list")));
    }
    click
}

#[derive(Clone, Copy, PartialEq)]
enum Dir {
    Left,
    Right,
    Down,
}

/// Seta fina (chevron) centrada em `c`.
fn chevron(painter: &Painter, c: Pos2, dir: Dir, color: Color32) {
    let (a, b, d) = match dir {
        Dir::Left => (Vec2::new(1.5, -4.0), Vec2::new(-1.5, 0.0), Vec2::new(1.5, 4.0)),
        Dir::Right => (Vec2::new(-1.5, -4.0), Vec2::new(1.5, 0.0), Vec2::new(-1.5, 4.0)),
        Dir::Down => (Vec2::new(-4.0, -1.5), Vec2::new(0.0, 1.5), Vec2::new(4.0, -1.5)),
    };
    let s = Stroke::new(1.5, color);
    painter.line_segment([c + a, c + b], s);
    painter.line_segment([c + b, c + d], s);
}

/// Degradê horizontal de `PANEL` opaco (em `solid_x`) a transparente (em `far_x`), entre `top` e `bottom`.
fn fade(painter: &Painter, top: f32, bottom: f32, solid_x: f32, far_x: f32) {
    let (opaque, clear) = (theme::PANEL, theme::with_opacity(theme::PANEL, 0.0));
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(Pos2::new(solid_x, top), opaque);
    mesh.colored_vertex(Pos2::new(far_x, top), clear);
    mesh.colored_vertex(Pos2::new(far_x, bottom), clear);
    mesh.colored_vertex(Pos2::new(solid_x, bottom), opaque);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(mesh);
}

/// Seta de uma borda da faixa (há abas escondidas desse lado): fundo sólido, degradê e botão.
/// Devolve se foi clicada; `target` é a aba escondida mais próxima, que a dica cita.
fn edge_arrow(deck: &Deck, ui: &Ui, painter: &Painter, zone: Rect, dir: Dir, target: usize, id: Id) -> bool {
    painter.rect_filled(zone, 0.0, theme::PANEL);
    let (solid_x, far_x) = if dir == Dir::Left { (zone.max.x, zone.max.x + FADE_W) } else { (zone.min.x, zone.min.x - FADE_W) };
    fade(painter, zone.min.y, zone.max.y, solid_x, far_x);
    let btn = Rect::from_center_size(zone.center(), Vec2::splat(ARROW_BTN));
    let resp = clickable(ui, btn, id);
    if resp.hovered() {
        painter.rect_filled(btn, 4.0, theme::PANEL_2);
    }
    chevron(painter, btn.center(), dir, if resp.hovered() { theme::TEXT } else { theme::MUTED });
    tip(resp, format!("Mostrar «{}»", deck.tabs[target].label)).clicked()
}

/// Botão "▾" no fim da faixa e o menu com todas as abas da janela. Devolve a aba escolhida.
fn tab_list(deck: &Deck, ui: &Ui, painter: &Painter, zone: Rect, tabs: &[usize], shown: usize, id: Id) -> Option<usize> {
    let ctx = ui.ctx();
    painter.rect_filled(zone, 0.0, theme::PANEL);
    let btn = Rect::from_center_size(zone.center(), Vec2::splat(ARROW_BTN));
    let resp = clickable(ui, btn, id.with("btn"));
    let mut open = ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    let hot = resp.hovered() || open;
    if hot {
        painter.rect_filled(btn, 4.0, theme::PANEL_2);
    }
    chevron(painter, btn.center(), Dir::Down, if hot { theme::TEXT } else { theme::MUTED });
    if tip(resp, "Todas as abas").clicked() {
        open = !open;
    }

    let mut chosen = None;
    if open {
        let rows = tabs.len().min(LIST_MAX_ROWS);
        let (w, h) = (LIST_MENU_W, LIST_ITEM_H * rows as f32 + 8.0);
        let pos = keep_on_screen(ctx, Pos2::new(btn.max.x - w, btn.max.y + 4.0), Vec2::new(w, h));
        let menu = Rect::from_min_size(pos, Vec2::new(w, h));
        Area::new(id.with("menu")).order(Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
            shadow(ui.painter(), rect, 8, 8, 24, theme::rgba(0, 0, 0, 0.55));
            frame(ui.painter(), rect, 6.0, theme::PANEL_2, theme::BORDER);
            let inner = rect.shrink(4.0);
            ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
                egui::ScrollArea::vertical().max_height(inner.height()).auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    for &i in tabs {
                        let tab = &deck.tabs[i];
                        let (r, _) = ui.allocate_exact_size(Vec2::new(inner.width(), LIST_ITEM_H), Sense::hover());
                        let resp = clickable(ui, r, id.with(("item", tab.uid)));
                        let p = ui.painter();
                        if resp.hovered() || i == shown {
                            p.rect_filled(r, 4.0, theme::BORDER);
                        }
                        dot(p, Pos2::new(r.min.x + 14.0, r.center().y), tab.color);
                        let color = if tab.alive { theme::TEXT } else { theme::MUTED };
                        let text_x = r.min.x + 28.0;
                        let g = galley_trunc(p, &tab.label, theme::ui(12.5), color, r.max.x - text_x - 8.0);
                        let area = Rect::from_min_max(Pos2::new(text_x, r.min.y), Pos2::new(r.max.x - 8.0, r.max.y));
                        let lr = place(p, area, Align2::LEFT_CENTER, g, color);
                        if !tab.alive {
                            p.hline(lr.min.x..=lr.max.x, lr.center().y + 0.5, Stroke::new(1.0, color));
                        }
                        if resp.clicked() {
                            chosen = Some(i);
                        }
                    }
                });
            });
        });
        if chosen.is_some() || ctx.input(|i| i.key_pressed(egui::Key::Escape)) || closes_on_outside(ctx, menu, btn) {
            open = false;
        }
    }
    ctx.data_mut(|d| d.insert_temp(id, open));
    chosen
}

/// Dica da aba: o nome, o modelo (inteiro) e como renomear.
fn tab_tip(label: &str, model: Option<&str>) -> String {
    let model = model.map(|m| format!("\nModelo: {m}")).unwrap_or_default();
    format!("{label}{model}\nDuplo clique ou botão direito para renomear")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cinco abas de 100 px.
    const W: [f32; 5] = [100.0; 5];

    #[test]
    fn nothing_is_hidden_when_the_tabs_fit() {
        assert_eq!(hidden_tabs(&W, 0.0, 500.0, 24.0), Hidden::default());
        assert_eq!(hidden_tabs(&[], 0.0, 250.0, 24.0), Hidden::default());
    }

    #[test]
    fn the_cut_tab_counts_as_hidden_on_the_side_it_is_cut() {
        // No começo: só há o que ver à direita; a terceira está cortada (200..300 em 0..250).
        assert_eq!(hidden_tabs(&W, 0.0, 250.0, 0.0), Hidden { left: None, right: Some(2) });
        // No meio (rolada 120): a segunda cortada à esquerda; a quarta, cortada à direita (300..400 em 120..370).
        assert_eq!(hidden_tabs(&W, 120.0, 250.0, 0.0), Hidden { left: Some(1), right: Some(3) });
        // No fim (rolada 250): só há o que ver à esquerda; a mais próxima é a terceira, cortada.
        assert_eq!(hidden_tabs(&W, 250.0, 250.0, 0.0), Hidden { left: Some(2), right: None });
    }

    #[test]
    fn a_fraction_of_a_pixel_does_not_make_a_tab_hidden() {
        assert_eq!(hidden_tabs(&W, 0.3, 300.0, 0.0), Hidden { left: None, right: Some(3) });
        assert_eq!(hidden_tabs(&W, 100.0, 300.2, 0.0), Hidden { left: Some(0), right: Some(4) });
        assert_eq!(hidden_tabs(&W, 200.2, 300.0, 0.0), Hidden { left: Some(1), right: None });
    }

    #[test]
    fn a_tab_under_the_arrow_counts_as_hidden() {
        // Rolada 100: a segunda começa na borda (100..200), mas a seta de 24 px a cobre; a primeira é a escondida.
        assert_eq!(hidden_tabs(&W, 100.0, 250.0, 24.0).left, Some(1));
        // Sem aba cortada do lado, não há seta e nada fica coberto: a quinta termina na borda direita.
        assert_eq!(hidden_tabs(&W, 250.0, 250.0, 24.0).right, None);
    }

    #[test]
    fn the_arrow_scrolls_to_the_nearest_hidden_tab() {
        // Para a esquerda: a aba escondida encosta na borda esquerda.
        assert_eq!(scroll_to_show(&W, 1, 120.0, 250.0, 0.0), 100.0);
        // Para a direita: a aba escondida encosta na borda direita.
        assert_eq!(scroll_to_show(&W, 2, 0.0, 250.0, 0.0), 50.0);
        // Já à vista: não mexe.
        assert_eq!(scroll_to_show(&W, 1, 50.0, 250.0, 0.0), 50.0);
        // Nunca passa do fim do conteúdo nem do começo.
        assert_eq!(scroll_to_show(&W, 4, 0.0, 250.0, 0.0), 250.0);
        assert_eq!(scroll_to_show(&W, 0, 250.0, 250.0, 0.0), 0.0);
    }

    #[test]
    fn the_shown_tab_stays_clear_of_the_arrows() {
        // Com abas dos dois lados, sobra a largura da seta junto à borda; nas pontas, não.
        assert_eq!(scroll_to_show(&W, 2, 0.0, 250.0, 24.0), 74.0);
        assert_eq!(scroll_to_show(&W, 2, 400.0, 250.0, 24.0), 176.0);
        assert_eq!(scroll_to_show(&W, 0, 250.0, 250.0, 24.0), 0.0);
        assert_eq!(scroll_to_show(&W, 4, 0.0, 250.0, 24.0), 250.0);
    }

    #[test]
    fn clicking_the_arrows_reaches_every_tab() {
        for edge in [0.0, 24.0] {
            let (mut scroll, view) = (0.0, 250.0);
            let mut clicks = 0;
            while let Some(n) = hidden_tabs(&W, scroll, view, edge).right {
                scroll = scroll_to_show(&W, n, scroll, view, edge);
                clicks += 1;
                assert!(clicks < 10);
            }
            assert_eq!(scroll, 250.0);
            while let Some(n) = hidden_tabs(&W, scroll, view, edge).left {
                scroll = scroll_to_show(&W, n, scroll, view, edge);
                clicks += 1;
                assert!(clicks < 20);
            }
            assert_eq!(scroll, 0.0);
        }
    }

    #[test]
    fn the_tip_names_the_full_model() {
        assert_eq!(tab_tip("Claude · Dev", None), "Claude · Dev\nDuplo clique ou botão direito para renomear");
        assert_eq!(
            tab_tip("OpenCode", Some("opencode/glm-5.3-flash-free")),
            "OpenCode\nModelo: opencode/glm-5.3-flash-free\nDuplo clique ou botão direito para renomear"
        );
    }
}
