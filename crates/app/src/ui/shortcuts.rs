//! Keyboard shortcuts modal (Ctrl+/).
//!
//! Shows the real shortcuts documented in the README and closes with Esc, a click
//! outside or the close button.

use egui::{Align2, Id, Key, Order, Pos2, Rect, Sense, Stroke, Vec2};

use super::{button, button_width, clickable, frame, galley, galley_trunc, place, separator, shadow, text_in, text_w, ButtonKind};
use crate::deck::Deck;
use crate::theme;
use holt_core::t;

const ID_SHORTCUTS_OPEN: &str = "shortcuts-modal-open";

/// One item of the shortcut list.
pub struct ShortcutItem {
    pub keys: Vec<&'static str>,
    pub desc: &'static str,
}

/// The Holt shortcuts (as documented in the README). A function, not a constant: the text goes through `t!`.
pub fn shortcuts() -> Vec<ShortcutItem> {
    vec![
        ShortcutItem { keys: vec!["Ctrl+T"], desc: t!("New tab of the same agent (collapsed bar: choose)") },
        ShortcutItem { keys: vec!["Ctrl+Tab", "Ctrl+Shift+Tab"], desc: t!("Switch tabs (or documents, in Documents mode)") },
        ShortcutItem { keys: vec![t!("Ctrl+Alt+arrows")], desc: t!("Go to the neighboring pane (split view)") },
        ShortcutItem { keys: vec!["Ctrl+B"], desc: t!("Show or hide the sidebar *") },
        ShortcutItem { keys: vec!["Ctrl+Shift+D"], desc: t!("Switch the center: Agents / Documents") },
        ShortcutItem { keys: vec!["Ctrl+S"], desc: t!("Save the file being edited (Documents mode)") },
        ShortcutItem { keys: vec!["Ctrl+F"], desc: t!("Search the open document (Documents mode)") },
        ShortcutItem { keys: vec!["Ctrl+Shift+M"], desc: t!("Dictate by voice in the active tab (nothing is sent)") },
        ShortcutItem { keys: vec![t!("Double click")], desc: t!("Rename tab (model@role@name)") },
        ShortcutItem { keys: vec!["Ctrl+/"], desc: t!("Show or close these shortcuts") },
    ]
}

/// Note for the shortcuts marked with `*`, following the "App shortcuts above the terminal" option.
pub fn terminal_note(app_first: bool) -> &'static str {
    if app_first {
        t!("* Also with the terminal focused (option in ☰ Settings).")
    } else {
        t!("* With the terminal focused, it goes to the terminal (tmux). Change it in ☰ Settings.")
    }
}

/// Tells whether the shortcuts modal is open.
#[allow(dead_code)]
pub fn is_open(ctx: &egui::Context) -> bool {
    let id = Id::new(ID_SHORTCUTS_OPEN);
    ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false)
}

/// Changes the visibility of the shortcuts modal.
#[allow(dead_code)]
pub fn set_open(ctx: &egui::Context, open: bool) {
    let id = Id::new(ID_SHORTCUTS_OPEN);
    ctx.data_mut(|d| d.insert_temp(id, open));
}

/// Toggles the visibility of the shortcuts modal.
#[allow(dead_code)]
pub fn toggle(ctx: &egui::Context) -> bool {
    let new_state = !is_open(ctx);
    set_open(ctx, new_state);
    new_state
}

/// Modal that lists the application's keyboard shortcuts.
pub fn shortcuts_modal(deck: &mut Deck, ctx: &egui::Context) {
    let id = Id::new(ID_SHORTCUTS_OPEN);
    let mut open = ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);

    // Opens / closes with Ctrl+/ (or Ctrl+? depending on the keyboard layout)
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

    // Ctrl+/ with the terminal focused arrives through the app's keyboard (`Deck::handle_key`), not through egui.
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

    let items = shortcuts();
    let note = terminal_note(deck.app_shortcuts_first);
    let w = 560.0;
    let h = 150.0 + items.len() as f32 * 26.0;
    let center = screen.center();
    let rect = Rect::from_center_size(center, Vec2::new(w, h));
    let mut close = false;

    // Closes on a click outside the modal rectangle
    if ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !rect.contains(p))) {
        close = true;
    }
    // Closes on Esc
    if ctx.input(|i| i.key_pressed(Key::Escape)) {
        close = true;
    }

    egui::Area::new(Id::new("shortcuts-modal")).order(Order::Tooltip).fixed_pos(rect.min).show(ctx, |ui| {
        let (r, _) = ui.allocate_exact_size(rect.size(), Sense::hover());
        let p = ui.painter().clone();
        let pal = theme::pal();
        shadow(&p, r, 10, 8, 24, pal.shadow);
        frame(&p, r, 10.0, pal.popup, pal.glass_border);

        let mut y = r.min.y + 14.0;

        // Title (13 px 600)
        let title_g = galley(&p, t!("Keyboard Shortcuts"), theme::ui_semibold(13.0), pal.text_strong);
        place(
            &p,
            Rect::from_min_size(Pos2::new(r.min.x + 20.0, y), Vec2::new(w - 70.0, 20.0)),
            Align2::LEFT_CENTER,
            title_g,
            pal.text_strong,
        );

        // Close button (✕)
        let close_x_r = Rect::from_min_size(Pos2::new(r.max.x - 36.0, y), Vec2::new(20.0, 20.0));
        let cr_x = clickable(ui, close_x_r, Id::new("shortcuts-close-x"));
        if cr_x.hovered() {
            p.rect_filled(close_x_r, 5.0, pal.hover);
        }
        let x_color = if cr_x.hovered() { pal.text } else { pal.text_2 };
        text_in(&p, close_x_r, Align2::CENTER_CENTER, "✕", theme::ui(12.0), x_color);
        if cr_x.clicked() {
            close = true;
        }
        y += 22.0;

        // Explanation
        let hint_g = galley(&p, t!("Main Holt shortcuts and actions"), theme::ui(11.5), pal.text_2);
        place(&p, Rect::from_min_size(Pos2::new(r.min.x + 20.0, y), Vec2::new(w - 40.0, 16.0)), Align2::LEFT_CENTER, hint_g, pal.text_2);
        y += 22.0;

        // Divider line
        separator(&p, Pos2::new(r.min.x + 20.0, y + 0.5), Pos2::new(r.max.x - 20.0, y + 0.5));
        y += 8.0;

        // Shortcut list
        for item in &items {
            let mut kx = r.min.x + 20.0;
            for (idx, key) in item.keys.iter().enumerate() {
                if idx > 0 {
                    let sep_g = galley(&p, "/", theme::ui(11.0), pal.text_2);
                    place(&p, Rect::from_min_size(Pos2::new(kx, y), Vec2::new(12.0, 22.0)), Align2::CENTER_CENTER, sep_g, pal.text_2);
                    kx += 12.0;
                }
                let kw = text_w(&p, key, theme::mono_bold(11.0)) + 12.0;
                let badge_r = Rect::from_min_size(Pos2::new(kx, y + 1.0), Vec2::new(kw, 20.0));
                p.rect_filled(badge_r, 4.0, pal.recess);
                p.rect_stroke(badge_r, 4.0, Stroke::new(1.0, pal.separator), egui::StrokeKind::Inside);
                text_in(&p, badge_r, Align2::CENTER_CENTER, key, theme::mono_bold(11.0), pal.text);
                kx += kw + 2.0;
            }

            let desc_g = galley_trunc(&p, item.desc, theme::ui(12.0), pal.text, w - 235.0);
            place(
                &p,
                Rect::from_min_size(Pos2::new(r.min.x + 215.0, y), Vec2::new(w - 235.0, 22.0)),
                Align2::LEFT_CENTER,
                desc_g,
                pal.text,
            );
            y += 26.0;
        }

        let note_g = galley_trunc(&p, note, theme::ui(11.5), pal.text_2, w - 40.0);
        place(&p, Rect::from_min_size(Pos2::new(r.min.x + 20.0, y), Vec2::new(w - 40.0, 18.0)), Align2::LEFT_CENTER, note_g, pal.text_2);
        y += 22.0;
        // Divider line before the footer
        separator(&p, Pos2::new(r.min.x + 20.0, y + 0.5), Pos2::new(r.max.x - 20.0, y + 0.5));
        y += 10.0;

        // Footer
        let footer_hint = galley(&p, t!("Press Esc or click outside to close"), theme::ui(11.5), pal.text_2);
        place(&p, Rect::from_min_size(Pos2::new(r.min.x + 20.0, y), Vec2::new(280.0, 26.0)), Align2::LEFT_CENTER, footer_hint, pal.text_2);

        let btn_w = button_width(&p, t!("Close"), ButtonKind::Secondary);
        let btn_h = 26.0;
        let close_btn_r = Rect::from_min_size(Pos2::new(r.max.x - 20.0 - btn_w, y), Vec2::new(btn_w, btn_h));
        if button(ui, close_btn_r, Id::new("shortcuts-close-btn"), t!("Close"), ButtonKind::Secondary).clicked() {
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
    use holt_core::i18n;

    #[test]
    fn shortcuts_list_contains_readme_items() {
        i18n::english(|| {
            let keys_joined: Vec<String> = shortcuts().iter().map(|s| s.keys.join(" ")).collect();
            let all_keys = keys_joined.join(" | ");

            assert!(all_keys.contains("Ctrl+T"), "must contain Ctrl+T");
            assert!(all_keys.contains("Ctrl+Tab"), "must contain Ctrl+Tab");
            assert!(all_keys.contains("Ctrl+Shift+Tab"), "must contain Ctrl+Shift+Tab");
            assert!(all_keys.contains("Ctrl+B"), "must contain Ctrl+B");
            assert!(all_keys.contains("Ctrl+Shift+M"), "must contain Ctrl+Shift+M");
            assert!(all_keys.contains("Ctrl+Shift+D"), "must contain Ctrl+Shift+D");
            assert!(all_keys.contains("Double click"), "must contain Double click");
            assert!(all_keys.contains("Ctrl+/"), "must contain Ctrl+/");
            assert!(!all_keys.contains("Ctrl+L"), "must not contain Ctrl+L (not an app shortcut)");
            assert!(terminal_note(false).contains("terminal") && terminal_note(true).contains('*'));
        });
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
