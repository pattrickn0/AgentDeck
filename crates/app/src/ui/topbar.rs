//! `#topbar`: botão da barra lateral, "+", tela dividida, branch, pasta de trabalho e configurações.

use egui::{Align2, Id, Pos2, Rect, Ui, Vec2};

use super::{clickable, galley, galley_trunc, place, text_w, tip};
use crate::deck::Deck;
use crate::icons::Icon;
use crate::theme;

pub struct Anchors {
    pub new_tab_anchor: Rect,
    pub settings_anchor: Rect,
}

const TAB_FONT: f32 = 13.0;

pub fn draw(deck: &mut Deck, ui: &mut Ui, rect: Rect) -> Anchors {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::PANEL);
    // border-bottom: 1px
    let inner = Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.max.y - 1.0));
    painter.rect_filled(Rect::from_min_max(Pos2::new(rect.min.x, rect.max.y - 1.0), rect.max), 0.0, theme::BORDER);
    let ctx = ui.ctx().clone();

    // ---------- #sidebar-toggle-btn (20px) ----------
    let toggle = Rect::from_min_size(inner.min, Vec2::new(20.0, inner.height()));
    let r = clickable(ui, toggle, Id::new("sidebar-toggle"));
    let hovered = r.hovered();
    painter.rect_filled(
        Rect::from_min_max(toggle.min, Pos2::new(toggle.max.x - 1.0, toggle.max.y)),
        0.0,
        if hovered { theme::PANEL_2 } else { theme::PANEL },
    );
    painter.rect_filled(Rect::from_min_max(Pos2::new(toggle.max.x - 1.0, toggle.min.y), toggle.max), 0.0, theme::BORDER);
    let t = ctx.animate_bool_with_time(Id::new("sidebar-chevron"), deck.sidebar_collapsed, 0.15);
    deck.icons.paint(
        &painter,
        Icon::SidebarChevron,
        Pos2::new(toggle.center().x - 0.5, toggle.center().y),
        9.0,
        if hovered { theme::TEXT } else { theme::MUTED },
        t * std::f32::consts::PI,
    );
    let title = if deck.sidebar_collapsed { "Expandir barra lateral (Ctrl+B)" } else { "Recolher barra lateral (Ctrl+B)" };
    if tip(r, title).clicked() {
        deck.toggle_sidebar();
    }

    // ---------- Lado direito: engrenagem e pasta ----------
    let settings = Rect::from_min_max(Pos2::new(inner.max.x - 38.0, inner.min.y), inner.max);
    let ws_text = format!("📁 {}", deck.workspace.display());
    let ws_font = theme::ui(TAB_FONT);
    // Relativo à topbar (não à janela): ela divide a largura com as duas barras laterais.
    let ws_max = inner.width() * 0.30;
    let ws_w = (text_w(&painter, &ws_text, ws_font.clone()) + 28.0).min(ws_max);
    let ws_rect = Rect::from_min_max(Pos2::new(settings.min.x - ws_w, inner.min.y), Pos2::new(settings.min.x, inner.max.y));
    // Chip da branch (antes da pasta), só quando a pasta é um repositório git.
    let chip = deck.git.status().map(crate::git_ui::chip_text);
    let chip_rect = chip.as_ref().map(|text| {
        let w = (text_w(&painter, text, theme::ui(12.0)) + 34.0).min(160.0);
        Rect::from_min_max(Pos2::new(ws_rect.min.x - w, inner.min.y + 6.0), Pos2::new(ws_rect.min.x - 2.0, inner.max.y - 6.0))
    });
    let right_edge = chip_rect.map_or(ws_rect.min.x, |r| r.min.x);

    // ---------- #new-tab (+) (as abas ficam no cabeçalho de cada janela; ver `panes.rs`) ----------
    let plus_w = 38.0;
    let plus = Rect::from_min_size(Pos2::new(toggle.max.x, inner.min.y), Vec2::new(plus_w, inner.height()));
    let r = clickable(ui, plus, Id::new("new-tab"));
    let hot = r.hovered() || deck.new_tab_menu_open;
    if r.hovered() {
        painter.rect_filled(plus, 0.0, theme::PANEL_2);
    }
    let pg = galley(&painter, "+", theme::ui(18.0), theme::MUTED);
    place(&painter, plus, Align2::CENTER_CENTER, pg, if hot { theme::TEXT } else { theme::MUTED });
    if tip(r, "Nova aba (Ctrl+T)").clicked() {
        deck.new_tab_menu_open = !deck.new_tab_menu_open;
        deck.settings_open = false;
        if deck.new_tab_menu_open {
            deck.refresh_agents();
        }
    }

    // ---------- Tela dividida (liga/desliga) ----------
    let split_btn = Rect::from_min_max(Pos2::new(right_edge - 36.0, inner.min.y), Pos2::new(right_edge - 4.0, inner.max.y));
    if split_btn.min.x > plus.max.x {
        let r = clickable(ui, split_btn, Id::new("split-toggle"));
        let on = deck.split_enabled;
        if r.hovered() {
            painter.rect_filled(split_btn.shrink2(Vec2::new(0.0, 6.0)), 6.0, theme::PANEL_2);
        }
        let c = if on {
            deck.active_color()
        } else if r.hovered() {
            theme::TEXT
        } else {
            theme::MUTED
        };
        deck.icons.paint(&painter, Icon::Split, split_btn.center(), 16.0, c, 0.0);
        let title = if on {
            "Tela dividida: ligada\nCada tipo de agente numa janela · clique para ver uma aba por vez"
        } else {
            "Tela dividida: desligada\nClique para abrir uma janela por tipo de agente"
        };
        if tip(r, title).clicked() {
            deck.toggle_split();
        }
    }

    // ---------- Chip da branch ----------
    if let (Some(text), Some(cr)) = (chip, chip_rect) {
        let r = clickable(ui, cr, Id::new("git-chip"));
        if r.hovered() {
            painter.rect_filled(cr, 6.0, theme::PANEL_2);
        }
        let c = if r.hovered() { theme::TEXT } else { theme::MUTED };
        deck.icons.paint(&painter, crate::icons::Icon::GitBranch, Pos2::new(cr.min.x + 13.0, cr.center().y), 13.0, c, 0.0);
        let g = galley_trunc(&painter, &text, theme::ui(12.0), c, cr.width() - 30.0);
        place(&painter, Rect::from_min_max(Pos2::new(cr.min.x + 24.0, cr.min.y), cr.max), Align2::LEFT_CENTER, g, c);
        let detail = match deck.git.status() {
            Some(st) => format!(
                "Branch {}{}\n{} alteração(ões) · {} à frente · {} atrás\nClique para abrir o Git",
                st.branch.as_deref().unwrap_or("(destacado)"),
                st.upstream.as_deref().map(|u| format!(" → {u}")).unwrap_or_default(),
                st.entries.len(),
                st.ahead,
                st.behind
            ),
            None => String::new(),
        };
        if tip(r, detail).clicked() {
            deck.set_right_view(crate::deck::RightView::Git);
        }
    }

    // ---------- #workspace ----------
    let r = clickable(ui, ws_rect, Id::new("workspace"));
    let wc = if r.hovered() { theme::TEXT } else { theme::MUTED };
    let g = galley_trunc(&painter, &ws_text, ws_font, wc, ws_rect.width() - 28.0);
    place(&painter, ws_rect.shrink2(Vec2::new(14.0, 0.0)), Align2::LEFT_CENTER, g, wc);
    let ws_title = format!("{}\nClique para trocar (vale para as próximas abas)", deck.workspace.display());
    if tip(r, ws_title).clicked() {
        deck.pick_workspace();
    }

    // ---------- #settings-btn ----------
    let r = clickable(ui, settings, Id::new("settings-btn"));
    let hot = r.hovered();
    if hot {
        painter.rect_filled(settings, 0.0, theme::PANEL_2);
    }
    deck.icons.paint(&painter, Icon::Gear, settings.center(), 18.0, if hot { theme::TEXT } else { theme::MUTED }, 0.0);
    if tip(r, "Configurações").clicked() {
        deck.settings_open = !deck.settings_open;
        deck.new_tab_menu_open = false;
    }

    Anchors { new_tab_anchor: plus, settings_anchor: settings }
}
