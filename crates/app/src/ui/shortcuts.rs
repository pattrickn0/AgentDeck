//! Modal de atalhos de teclado (Ctrl+/).
//!
//! Exibe os atalhos reais documentados no README e permite fechar com Esc,
//! clique fora ou pelo botão de fechar.

use egui::{Align2, Id, Key, Order, Pos2, Rect, Sense, Stroke, Vec2};

use super::{clickable, frame, galley, galley_trunc, place, shadow, text_in, text_w};
use crate::deck::Deck;
use crate::theme;

const ID_SHORTCUTS_OPEN: &str = "shortcuts-modal-open";

/// Um item da lista de atalhos.
pub struct ShortcutItem {
    pub keys: &'static [&'static str],
    pub desc: &'static str,
}

/// Lista de atalhos do CommandDeck (conforme README).
pub const SHORTCUTS: &[ShortcutItem] = &[
    ShortcutItem { keys: &["Ctrl+T"], desc: "Nova aba do mesmo agente (barra recolhida: escolher)" },
    ShortcutItem { keys: &["Ctrl+Tab", "Ctrl+Shift+Tab"], desc: "Alternar abas (ou documentos, no modo Documentos)" },
    ShortcutItem { keys: &["Ctrl+Alt+setas"], desc: "Ir para a janela vizinha (tela dividida)" },
    ShortcutItem { keys: &["Ctrl+B"], desc: "Mostrar ou ocultar barra lateral *" },
    ShortcutItem { keys: &["Ctrl+Shift+D"], desc: "Alternar o centro: Agentes / Documentos" },
    ShortcutItem { keys: &["Ctrl+Shift+M"], desc: "Ditar por voz na aba ativa (sem enviar)" },
    ShortcutItem { keys: &["Duplo clique"], desc: "Renomear aba (modelo@cargo@nome)" },
    ShortcutItem { keys: &["Ctrl+/"], desc: "Mostrar ou fechar estes atalhos" },
];

/// Nota dos atalhos marcados com `*`, conforme a opção "Atalhos do app acima do terminal".
pub fn terminal_note(app_first: bool) -> &'static str {
    if app_first {
        "* Também com o terminal focado (opção em ☰ Configurações)."
    } else {
        "* Com o terminal focado, vai para o terminal (tmux). Mude em ☰ Configurações."
    }
}

/// Informa se o modal de atalhos está aberto.
#[allow(dead_code)]
pub fn is_open(ctx: &egui::Context) -> bool {
    let id = Id::new(ID_SHORTCUTS_OPEN);
    ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false)
}

/// Altera a visibilidade do modal de atalhos.
#[allow(dead_code)]
pub fn set_open(ctx: &egui::Context, open: bool) {
    let id = Id::new(ID_SHORTCUTS_OPEN);
    ctx.data_mut(|d| d.insert_temp(id, open));
}

/// Alterna a visibilidade do modal de atalhos.
#[allow(dead_code)]
pub fn toggle(ctx: &egui::Context) -> bool {
    let new_state = !is_open(ctx);
    set_open(ctx, new_state);
    new_state
}

/// Modal que lista os atalhos de teclado da aplicação.
pub fn shortcuts_modal(deck: &mut Deck, ctx: &egui::Context) {
    let id = Id::new(ID_SHORTCUTS_OPEN);
    let mut open = ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);

    // Abre / fecha com Ctrl+/ (ou Ctrl+? dependendo do layout de teclado)
    let triggered = ctx.input_mut(|i| {
        i.consume_key(egui::Modifiers::COMMAND, Key::Slash)
            || i.consume_key(egui::Modifiers::CTRL, Key::Slash)
            || i.consume_key(egui::Modifiers::COMMAND, Key::Questionmark)
            || i.consume_key(egui::Modifiers::CTRL, Key::Questionmark)
            || i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::Slash)
            || i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, Key::Slash)
            || i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::Questionmark)
            || i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, Key::Questionmark)
    });

    // Ctrl+/ com o terminal focado chega pelo teclado do app (`Deck::handle_key`), não pelo egui.
    let triggered = triggered || std::mem::take(&mut deck.toggle_shortcuts);
    if triggered {
        open = !open;
        ctx.data_mut(|d| d.insert_temp(id, open));
    }

    if !open {
        return;
    }

    let screen = ctx.content_rect();
    super::modal_backdrop(deck, ctx, "shortcuts-backdrop");

    let note = terminal_note(deck.app_shortcuts_first);
    let w = 560.0;
    let h = 150.0 + SHORTCUTS.len() as f32 * 26.0;
    let center = screen.center();
    let rect = Rect::from_center_size(center, Vec2::new(w, h));
    let mut close = false;

    // Fecha ao clicar fora do retângulo do modal
    if ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !rect.contains(p))) {
        close = true;
    }
    // Fecha ao pressionar Esc
    if ctx.input(|i| i.key_pressed(Key::Escape)) {
        close = true;
    }

    egui::Area::new(Id::new("shortcuts-modal")).order(Order::Tooltip).fixed_pos(rect.min).show(ctx, |ui| {
        let (r, _) = ui.allocate_exact_size(rect.size(), Sense::hover());
        let p = ui.painter().clone();
        shadow(&p, r, 0, 8, 30, theme::rgba(0, 0, 0, 0.7));
        frame(&p, r, 10.0, theme::PANEL_2, theme::BORDER);

        let mut y = r.min.y + 16.0;

        // Título
        let title_g = galley(&p, "Atalhos de Teclado", theme::ui_semibold(15.0), theme::TEXT);
        place(&p, Rect::from_min_size(Pos2::new(r.min.x + 20.0, y), Vec2::new(w - 70.0, 20.0)), Align2::LEFT_CENTER, title_g, theme::TEXT);

        // Botão fechar (✕)
        let close_x_r = Rect::from_min_size(Pos2::new(r.max.x - 36.0, y), Vec2::new(20.0, 20.0));
        let cr_x = clickable(ui, close_x_r, Id::new("shortcuts-close-x"));
        let x_color = if cr_x.hovered() { theme::TEXT } else { theme::MUTED };
        text_in(&p, close_x_r, Align2::CENTER_CENTER, "✕", theme::ui(13.0), x_color);
        if cr_x.clicked() {
            close = true;
        }
        y += 24.0;

        // Explicação
        let hint_g = galley(&p, "Principais atalhos e ações do CommandDeck", theme::ui(11.5), theme::MUTED);
        place(&p, Rect::from_min_size(Pos2::new(r.min.x + 20.0, y), Vec2::new(w - 40.0, 16.0)), Align2::LEFT_CENTER, hint_g, theme::MUTED);
        y += 22.0;

        // Linha divisória
        p.rect_filled(Rect::from_min_size(Pos2::new(r.min.x + 20.0, y), Vec2::new(w - 40.0, 1.0)), 0.0, theme::BORDER);
        y += 8.0;

        // Lista de atalhos
        for item in SHORTCUTS {
            let mut kx = r.min.x + 20.0;
            for (idx, key) in item.keys.iter().enumerate() {
                if idx > 0 {
                    let sep_g = galley(&p, "/", theme::ui(11.0), theme::MUTED);
                    place(&p, Rect::from_min_size(Pos2::new(kx, y), Vec2::new(12.0, 22.0)), Align2::CENTER_CENTER, sep_g, theme::MUTED);
                    kx += 12.0;
                }
                let kw = text_w(&p, key, theme::mono_bold(11.0)) + 12.0;
                let badge_r = Rect::from_min_size(Pos2::new(kx, y + 1.0), Vec2::new(kw, 20.0));
                p.rect_filled(badge_r, 4.0, theme::BG);
                p.rect_stroke(badge_r, 4.0, Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Inside);
                text_in(&p, badge_r, Align2::CENTER_CENTER, key, theme::mono_bold(11.0), theme::INPUT_TEXT);
                kx += kw + 2.0;
            }

            let desc_g = galley_trunc(&p, item.desc, theme::ui(12.5), theme::TEXT, w - 235.0);
            place(
                &p,
                Rect::from_min_size(Pos2::new(r.min.x + 215.0, y), Vec2::new(w - 235.0, 22.0)),
                Align2::LEFT_CENTER,
                desc_g,
                theme::TEXT,
            );
            y += 26.0;
        }

        let note_g = galley_trunc(&p, note, theme::ui(11.5), theme::MUTED, w - 40.0);
        place(&p, Rect::from_min_size(Pos2::new(r.min.x + 20.0, y), Vec2::new(w - 40.0, 18.0)), Align2::LEFT_CENTER, note_g, theme::MUTED);
        y += 22.0;
        // Linha divisória antes do rodapé
        p.rect_filled(Rect::from_min_size(Pos2::new(r.min.x + 20.0, y), Vec2::new(w - 40.0, 1.0)), 0.0, theme::BORDER);
        y += 10.0;

        // Rodapé
        let footer_hint = galley(&p, "Pressione Esc ou clique fora para fechar", theme::ui(11.5), theme::MUTED);
        place(
            &p,
            Rect::from_min_size(Pos2::new(r.min.x + 20.0, y), Vec2::new(280.0, 26.0)),
            Align2::LEFT_CENTER,
            footer_hint,
            theme::MUTED,
        );

        let btn_w = 75.0;
        let btn_h = 26.0;
        let close_btn_r = Rect::from_min_size(Pos2::new(r.max.x - 20.0 - btn_w, y), Vec2::new(btn_w, btn_h));
        let cr = clickable(ui, close_btn_r, Id::new("shortcuts-close-btn"));
        if cr.hovered() {
            p.rect_filled(close_btn_r, 6.0, theme::BORDER);
        } else {
            p.rect_filled(close_btn_r, 6.0, theme::PANEL);
        }
        text_in(&p, close_btn_r, Align2::CENTER_CENTER, "Fechar", theme::ui(12.5), theme::MUTED);
        if cr.clicked() {
            close = true;
        }
    });

    if close {
        ctx.data_mut(|d| d.insert_temp(id, false));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_list_contains_readme_items() {
        let keys_joined: Vec<String> = SHORTCUTS.iter().map(|s| s.keys.join(" ")).collect();
        let all_keys = keys_joined.join(" | ");

        assert!(all_keys.contains("Ctrl+T"), "deve conter Ctrl+T");
        assert!(all_keys.contains("Ctrl+Tab"), "deve conter Ctrl+Tab");
        assert!(all_keys.contains("Ctrl+Shift+Tab"), "deve conter Ctrl+Shift+Tab");
        assert!(all_keys.contains("Ctrl+B"), "deve conter Ctrl+B");
        assert!(all_keys.contains("Ctrl+Shift+M"), "deve conter Ctrl+Shift+M");
        assert!(all_keys.contains("Ctrl+Shift+D"), "deve conter Ctrl+Shift+D");
        assert!(all_keys.contains("Duplo clique"), "deve conter Duplo clique");
        assert!(all_keys.contains("Ctrl+/"), "deve conter Ctrl+/");
        assert!(!all_keys.contains("Ctrl+L"), "não deve conter Ctrl+L (compositor removido)");
        assert!(terminal_note(false).contains("terminal") && terminal_note(true).contains('*'));
    }

    #[test]
    fn state_helpers() {
        let ctx = egui::Context::default();
        assert!(!is_open(&ctx));
        set_open(&ctx, true);
        assert!(is_open(&ctx));
        toggle(&ctx);
        assert!(!is_open(&ctx));
    }
}
