//! Drop-down menus: new tab (+) and settings (☰).

use egui::{Align2, Area, Color32, Id, Key, Order, Pos2, Rect, Sense, Stroke, Vec2};

use holt_core::i18n::{self, Lang};
use holt_core::{models, t};

use super::{button, button_danger, button_width, dot, frame, section_label, separator, shadow, text_in, text_w, ButtonKind};
use crate::deck::Deck;
use crate::dictation::Setup;
use crate::theme;

/// Top-left corner that keeps a menu of `size` on screen (on the sides and at the bottom: anchored
/// low in the sidebar, a long menu goes up).
/// (Do not use `Area::constrain`: it moves the menu after `closes_on_outside` has computed the
/// rectangle, and a click on an item then counts as "outside", closing the menu before the choice.)
pub(super) fn keep_on_screen(ctx: &egui::Context, pos: Pos2, size: Vec2) -> Pos2 {
    let screen = ctx.content_rect();
    Pos2::new(
        pos.x.min(screen.max.x - size.x - 4.0).max(screen.min.x + 4.0),
        pos.y.min(screen.max.y - size.y - 4.0).max(screen.min.y + 4.0),
    )
}

/// Closes the menu on a click outside it and outside the anchor (`document.addEventListener('click')`).
pub(super) fn closes_on_outside(ctx: &egui::Context, menu: Rect, anchor: Rect) -> bool {
    ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !menu.contains(p) && !anchor.contains(p)))
}

/// The agent lets the new-agent menu pick its model (Codex has no list to show).
pub fn picks_model(kind: &str) -> bool {
    matches!(kind, "claude" | "antigravity" | "opencode")
}

/// Notice in place of the list when there are no models to show.
fn models_status(loading: bool, empty: bool) -> Option<&'static str> {
    match (loading, empty) {
        (true, true) => Some(t!("Loading models…")),
        (false, true) => Some(t!("No model list: default only")),
        _ => None,
    }
}

/// Maximum height of the model list (OpenCode's has dozens).
const MODELS_MAX_H: f32 = 300.0;
/// Width of the "›" arrow that opens the model choice on the agent's row.
const CHEVRON_W: f32 = 28.0;

/// `#new-tab-menu`: the agents; on those that pick a model, the arrow on the right opens the second step
/// (default + known models). Clicking the name opens the agent with the default model.
pub fn new_tab_menu(deck: &mut Deck, ctx: &egui::Context, anchor: Rect) {
    if !deck.new_tab_menu_open {
        deck.new_tab_models_for = None;
        deck.new_tab_provider = None;
        return;
    }
    if let Some(kind) = deck.new_tab_models_for.clone() {
        return models_step(deck, ctx, anchor, &kind);
    }
    let items = deck.agents.new_tab_menu.clone();
    let items = if items.is_empty() {
        // Before the first listing: only the terminal (the renderer's fallback).
        vec![crate::agents::MenuItem { kind: "shell".into(), label: crate::agents::shell_label(None), color: theme::SHELL }]
    } else {
        items
    };
    let with_arrow: Vec<bool> =
        items.iter().map(|m| picks_model(&m.kind) && deck.agents.available.iter().any(|a| a.id == m.kind && a.installed)).collect();
    let font = theme::ui(12.0);
    let painter = ctx.layer_painter(egui::LayerId::new(Order::Foreground, Id::new("measure")));
    let content_w = items
        .iter()
        .zip(&with_arrow)
        .map(|(m, &arrow)| 8.0 + 8.0 + text_w(&painter, &m.label, font.clone()) + 20.0 + if arrow { CHEVRON_W } else { 0.0 })
        .fold(0.0, f32::max);
    let w = (content_w + 8.0 + 2.0).max(180.0);
    let item_h = 26.0;
    let h = items.len() as f32 * item_h + 8.0;
    let pos = keep_on_screen(ctx, Pos2::new(anchor.min.x, anchor.max.y), Vec2::new(w, h));
    let menu_rect = Rect::from_min_size(pos, Vec2::new(w, h));
    let mut chosen = None;
    let mut pick_model_of = None;
    Area::new(Id::new("new-tab-menu")).order(Order::Foreground).fixed_pos(pos).constrain(false).show(ctx, |ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
        let p = ui.painter();
        let pal = theme::pal();
        shadow(p, rect, 8, 8, 24, pal.shadow);
        frame(p, rect, 8.0, pal.popup, pal.glass_border);
        let mut y = rect.min.y + 4.0;
        for (m, &arrow) in items.iter().zip(&with_arrow) {
            let r = Rect::from_min_size(Pos2::new(rect.min.x + 4.0, y), Vec2::new(w - 8.0, item_h));
            y += item_h;
            let name_r = if arrow { Rect::from_min_max(r.min, Pos2::new(r.max.x - CHEVRON_W, r.max.y)) } else { r };
            let resp = super::clickable(ui, name_r, Id::new(("new-tab-item", &m.kind)));
            let arrow_resp = arrow.then(|| {
                let ar = Rect::from_min_max(Pos2::new(r.max.x - CHEVRON_W, r.min.y), r.max);
                super::clickable(ui, ar, Id::new(("new-tab-models", &m.kind)))
            });
            let arrow_hot = arrow_resp.as_ref().is_some_and(|a| a.hovered());
            if resp.hovered() || arrow_hot {
                p.rect_filled(r, 5.0, pal.hover);
            }
            super::projects::paint_kind_mark(&mut deck.icons, p, Pos2::new(r.min.x + 10.0 + 4.0, r.center().y), &m.kind, m.color);
            text_in(
                p,
                Rect::from_min_max(Pos2::new(r.min.x + 10.0 + 16.0, r.min.y), r.max),
                Align2::LEFT_CENTER,
                &m.label,
                font.clone(),
                pal.text,
            );
            if let Some(a) = arrow_resp {
                let ar = Rect::from_min_max(Pos2::new(r.max.x - CHEVRON_W, r.min.y), r.max);
                text_in(p, ar, Align2::CENTER_CENTER, "›", theme::ui(14.0), if arrow_hot { pal.text } else { pal.text_2 });
                if super::tip(a, t!("Choose the model for {agent}", agent = &m.label)).clicked() {
                    pick_model_of = Some(m.kind.clone());
                }
            }
            if resp.clicked() {
                chosen = Some(m.kind.clone());
            }
        }
    });
    if let Some(kind) = pick_model_of {
        deck.request_models(&kind);
        deck.new_tab_models_for = Some(kind);
        deck.new_tab_provider = None;
    } else if let Some(kind) = chosen {
        deck.new_tab_menu_open = false;
        deck.new_tab_provider = None;
        deck.open_tab(&kind, false);
    } else if closes_on_outside(ctx, menu_rect, anchor) {
        deck.new_tab_menu_open = false;
        deck.new_tab_provider = None;
    }
}

/// One row of a step of the model menu.
struct StepRow {
    label: String,
    /// Model with no per-token cost ("free", in green).
    free: bool,
    /// How many models the provider has ("N models", in gray); 0 = nothing.
    count: usize,
    /// Opens the next step ("›").
    submenu: bool,
    enabled: bool,
}

impl StepRow {
    fn new(label: impl Into<String>) -> Self {
        Self { label: label.into(), free: false, count: 0, submenu: false, enabled: true }
    }

    /// The quiet text on the right and its color.
    fn detail(&self) -> Option<(String, Color32)> {
        if self.count > 0 {
            let count = self.count;
            let text = if count == 1 { t!("1 model").to_owned() } else { t!("{count} models", count) };
            Some((text, theme::pal().text_2))
        } else if self.free {
            Some((t!("free").to_owned(), theme::pal().success))
        } else {
            None
        }
    }
}

/// What happened in a menu step: a row was chosen, it went back to the previous step, everything closed, or
/// nothing (the menu stays open).
enum StepOutcome {
    Chosen(usize),
    Back,
    Closed,
    Pending,
}

/// One menu step: the "‹ title" header goes back to the previous step and the rows are the choices.
fn step_menu(ctx: &egui::Context, anchor: Rect, id: &str, color: Color32, title: &str, rows: &[StepRow]) -> StepOutcome {
    let font = theme::ui(12.0);
    let painter = ctx.layer_painter(egui::LayerId::new(Order::Foreground, Id::new("measure")));
    let widest = rows
        .iter()
        .map(|row| {
            let detail = row.detail().map_or(0.0, |(text, _)| 8.0 + text_w(&painter, &text, theme::ui(11.0)));
            text_w(&painter, &row.label, font.clone()) + detail + if row.submenu { 16.0 } else { 0.0 }
        })
        .fold(text_w(&painter, title, font.clone()) + 54.0, f32::max);
    let w = (widest + 8.0 + 2.0 + 10.0 * 2.0).clamp(200.0, 380.0);
    let row_h = 26.0;
    let head_h = 26.0;
    let list_h = (rows.len() as f32 * row_h).min(MODELS_MAX_H);
    let h = 4.0 + head_h + 1.0 + list_h + 4.0;
    let pos = keep_on_screen(ctx, Pos2::new(anchor.min.x, anchor.max.y), Vec2::new(w, h));
    let menu_rect = Rect::from_min_size(pos, Vec2::new(w, h));
    let mut back = false;
    let mut chosen = None;
    let pal = theme::pal();
    Area::new(Id::new(id)).order(Order::Foreground).fixed_pos(pos).constrain(false).show(ctx, |ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
        shadow(ui.painter(), rect, 8, 8, 24, pal.shadow);
        frame(ui.painter(), rect, 8.0, pal.popup, pal.glass_border);
        // Header: "‹ title" goes back one step.
        let head = Rect::from_min_size(Pos2::new(rect.min.x + 4.0, rect.min.y + 4.0), Vec2::new(w - 8.0, head_h));
        let resp = super::clickable(ui, head, Id::new((id, "back")));
        if resp.hovered() {
            ui.painter().rect_filled(head, 5.0, pal.hover);
        }
        text_in(
            ui.painter(),
            Rect::from_min_size(head.min, Vec2::new(22.0, head_h)),
            Align2::CENTER_CENTER,
            "‹",
            theme::ui(14.0),
            pal.text_2,
        );
        dot(ui.painter(), Pos2::new(head.min.x + 26.0, head.center().y), color);
        text_in(
            ui.painter(),
            Rect::from_min_max(Pos2::new(head.min.x + 36.0, head.min.y), head.max),
            Align2::LEFT_CENTER,
            title,
            theme::ui_semibold(12.0),
            pal.text_strong,
        );
        if super::tip(resp, t!("Back")).clicked() {
            back = true;
        }
        separator(ui.painter(), Pos2::new(rect.min.x + 8.0, head.max.y + 0.5), Pos2::new(rect.max.x - 8.0, head.max.y + 0.5));
        let list = Rect::from_min_size(Pos2::new(rect.min.x + 4.0, head.max.y + 1.0), Vec2::new(w - 8.0, list_h));
        let mut lc = ui.new_child(egui::UiBuilder::new().max_rect(list));
        lc.set_clip_rect(list);
        egui::ScrollArea::vertical().id_salt((id, "list")).auto_shrink([false, false]).show(&mut lc, |ui| {
            for (i, row) in rows.iter().enumerate() {
                let (r, _) = ui.allocate_exact_size(Vec2::new(list.width(), row_h), Sense::hover());
                let resp =
                    if row.enabled { super::clickable(ui, r, Id::new((id, i))) } else { ui.interact(r, Id::new((id, i)), Sense::hover()) };
                if row.enabled && resp.hovered() {
                    ui.painter().rect_filled(r, 5.0, pal.hover);
                }
                let text_color = if row.enabled { pal.text } else { pal.text_3 };
                let mut right = r.max.x - 10.0;
                if row.submenu {
                    text_in(
                        ui.painter(),
                        Rect::from_min_max(Pos2::new(right - 12.0, r.min.y), Pos2::new(right, r.max.y)),
                        Align2::CENTER_CENTER,
                        "›",
                        theme::ui(13.0),
                        pal.text_2,
                    );
                    right -= 16.0;
                }
                if let Some((text, dcolor)) = row.detail() {
                    let dw = text_w(ui.painter(), &text, theme::ui(11.0));
                    let dr = Rect::from_min_max(Pos2::new(right - dw, r.min.y), Pos2::new(right, r.max.y));
                    text_in(ui.painter(), dr, Align2::RIGHT_CENTER, &text, theme::ui(11.0), dcolor);
                    right -= dw + 8.0;
                }
                let g = super::galley_trunc(ui.painter(), &row.label, font.clone(), text_color, (right - r.min.x - 10.0).max(10.0));
                super::place(
                    ui.painter(),
                    Rect::from_min_max(Pos2::new(r.min.x + 10.0, r.min.y), Pos2::new(right, r.max.y)),
                    Align2::LEFT_CENTER,
                    g,
                    text_color,
                );
                if row.enabled && resp.clicked() {
                    chosen = Some(i);
                }
            }
        });
    });
    if let Some(i) = chosen {
        return StepOutcome::Chosen(i);
    }
    if back {
        return StepOutcome::Back;
    }
    if closes_on_outside(ctx, menu_rect, anchor) || ctx.input(|i| i.key_pressed(Key::Escape)) {
        return StepOutcome::Closed;
    }
    StepOutcome::Pending
}

/// Second step of the new-agent menu: "Default" and the known models of `kind`. When the CLI
/// mixes providers (`opencode models`), the first step is the provider and the second, its models;
/// with a single provider, the models show up directly.
fn models_step(deck: &mut Deck, ctx: &egui::Context, anchor: Rect, kind: &str) {
    // The list comes from a thread: ask again if it has not arrived yet (or expired).
    deck.request_models(kind);
    let (models, loading) = deck.models_of(kind);
    let models: Vec<String> = models.to_vec();
    let groups = models::by_provider(&models);
    let name = deck.agents.name(kind);
    let color = deck.agents.tab_color(kind);
    let close_all = |deck: &mut Deck| {
        deck.new_tab_menu_open = false;
        deck.new_tab_models_for = None;
        deck.new_tab_provider = None;
    };

    // Claude and Antigravity: the usual list ("Default" + models).
    if groups.is_empty() {
        let flat = models::free_first(&models);
        let mut rows = vec![StepRow::new(t!("Default"))];
        rows.extend(flat.iter().map(|m| StepRow { free: models::is_free(m), ..StepRow::new(m.as_str()) }));
        if let Some(text) = models_status(loading, models.is_empty()) {
            rows.push(StepRow { enabled: false, ..StepRow::new(text) });
        }
        match step_menu(ctx, anchor, "new-tab-models", color, &t!("{agent} · model", agent = name), &rows) {
            StepOutcome::Chosen(0) => {
                close_all(deck);
                deck.open_tab_with_model(kind, None, false);
            }
            StepOutcome::Chosen(i) => {
                let model = flat[i - 1].clone();
                close_all(deck);
                deck.open_tab_with_model(kind, Some(&model), false);
            }
            StepOutcome::Back => deck.new_tab_models_for = None,
            StepOutcome::Closed => close_all(deck),
            StepOutcome::Pending => {}
        }
        return;
    }

    // OpenCode: the chosen provider lives in the menu state; without it, the step is the list of providers.
    // With a single provider there is nothing to choose: its models show up directly (no "‹ Providers").
    let sole = models::sole_provider(&groups).map(str::to_owned);
    let chosen = deck.new_tab_provider.clone().filter(|p| groups.iter().any(|(g, _)| g == p)).or(sole.clone());
    if let Some(provider) = chosen {
        let list = groups.iter().find(|(g, _)| *g == provider).map(|(_, l)| l.as_slice()).unwrap_or_default();
        let rows: Vec<StepRow> =
            list.iter().map(|m| StepRow { free: models::is_free(m), ..StepRow::new(models::short_model(m)) }).collect();
        match step_menu(ctx, anchor, "new-tab-provider-models", color, &provider, &rows) {
            StepOutcome::Chosen(i) => {
                let model = list[i].clone();
                close_all(deck);
                deck.open_tab_with_model(kind, Some(&model), false);
            }
            StepOutcome::Back => {
                if sole.is_some() {
                    // There was no providers step: the header goes back to the list of agents.
                    deck.new_tab_models_for = None;
                } else {
                    deck.new_tab_provider = None;
                }
            }
            StepOutcome::Closed => close_all(deck),
            StepOutcome::Pending => {}
        }
        return;
    }
    let rows: Vec<StepRow> =
        groups.iter().map(|(provider, list)| StepRow { count: list.len(), submenu: true, ..StepRow::new(provider.as_str()) }).collect();
    match step_menu(ctx, anchor, "new-tab-providers", color, &t!("{agent} · provider", agent = name), &rows) {
        StepOutcome::Chosen(i) => deck.new_tab_provider = Some(groups[i].0.clone()),
        StepOutcome::Back => deck.new_tab_models_for = None,
        StepOutcome::Closed => close_all(deck),
        StepOutcome::Pending => {}
    }
}

/// `#settings-menu`
pub fn settings_menu(deck: &mut Deck, ctx: &egui::Context, anchor: Rect) {
    if !deck.settings_open {
        // The confirmation to remove dictation only lasts while the menu is open.
        deck.dictation_confirm_remove = false;
        return;
    }
    let w = 260.0;
    let title_h = super::SECTION_LABEL_H;
    let item_h = 26.0;
    let btn_h = 28.0;
    // Four rows (three switches and the language), then the test button; voice dictation is optional: a
    // status row and a button to download or remove it.
    // The Dictation section sits between the language and the test button: label, 3 rows of language chips, the
    // translate switch and a hint.
    let chip_h = 24.0;
    let dict_h = 13.0 + title_h + chip_h * 3.0 + 4.0 + item_h + 36.0;
    let h = 6.0 + title_h + 2.0 + item_h * 4.0 + dict_h + 13.0 + btn_h + 13.0 + item_h + btn_h + 8.0;
    // The ☰ button sits in the left corner of the title bar: the menu opens aligned with it.
    let pos = keep_on_screen(ctx, Pos2::new(anchor.min.x + 4.0, anchor.max.y), Vec2::new(w, h));
    let menu_rect = Rect::from_min_size(pos, Vec2::new(w, h));
    let mut toggle = false;
    let mut toggle_tabs = false;
    let mut toggle_keys = false;
    let mut test = false;
    let mut language = None;
    let mut spoken = None;
    let mut toggle_translate = false;
    let (whisper_lang, translate) = {
        let c = deck.core.config();
        (c.whisper_lang, c.whisper_translate)
    };
    let engine_label = deck.engine_label.clone();
    let mut dictation_click = false;
    let setup = deck.dictation_setup();
    let confirm_remove = deck.dictation_confirm_remove;
    let sound = deck.sound_enabled;
    let auto_tabs = deck.auto_open_agent_tabs;
    let app_keys = deck.app_shortcuts_first;
    let current_lang = i18n::lang();
    let pal = theme::pal();
    Area::new(Id::new("settings-menu")).order(Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
        let p = ui.painter();
        shadow(p, rect, 8, 8, 24, pal.shadow);
        frame(p, rect, 8.0, pal.popup, pal.glass_border);
        let inner = rect.shrink2(Vec2::new(6.0, 6.0));
        section_label(p, Rect::from_min_size(Pos2::new(rect.min.x, inner.min.y), Vec2::new(rect.width(), title_h)), t!("SETTINGS"));

        // Item 1: notification sounds
        let item = Rect::from_min_size(Pos2::new(inner.min.x, inner.min.y + title_h + 2.0), Vec2::new(inner.width(), item_h));
        let r = super::clickable(ui, item, Id::new("setting-sound"));
        if r.hovered() {
            p.rect_filled(item, 5.0, pal.hover);
        }
        text_in(p, item.shrink2(Vec2::new(8.0, 0.0)), Align2::LEFT_CENTER, t!("Notification sounds"), theme::ui(12.0), pal.text);
        let cb = Rect::from_center_size(Pos2::new(item.max.x - 15.0, item.center().y), Vec2::splat(14.0));
        checkbox(p, cb, sound);
        if r.clicked() {
            toggle = true;
        }

        // Item 2: open agent tabs
        let item2 = Rect::from_min_size(Pos2::new(inner.min.x, item.max.y), Vec2::new(inner.width(), item_h));
        let r = super::clickable(ui, item2, Id::new("setting-auto-tabs"));
        if r.hovered() {
            p.rect_filled(item2, 5.0, pal.hover);
        }
        text_in(p, item2.shrink2(Vec2::new(8.0, 0.0)), Align2::LEFT_CENTER, t!("Open agent tabs"), theme::ui(12.0), pal.text);
        let cb2 = Rect::from_center_size(Pos2::new(item2.max.x - 15.0, item2.center().y), Vec2::splat(14.0));
        checkbox(p, cb2, auto_tabs);
        if r.clicked() {
            toggle_tabs = true;
        }

        // Item 3: app shortcuts above the terminal
        let item3 = Rect::from_min_size(Pos2::new(inner.min.x, item2.max.y), Vec2::new(inner.width(), item_h));
        let r = super::clickable(ui, item3, Id::new("setting-app-keys"));
        if r.hovered() {
            p.rect_filled(item3, 5.0, pal.hover);
        }
        text_in(
            p,
            item3.shrink2(Vec2::new(8.0, 0.0)),
            Align2::LEFT_CENTER,
            t!("App shortcuts above the terminal"),
            theme::ui(12.0),
            pal.text,
        );
        let cb3 = Rect::from_center_size(Pos2::new(item3.max.x - 15.0, item3.center().y), Vec2::splat(14.0));
        checkbox(p, cb3, app_keys);
        if r.clicked() {
            toggle_keys = true;
        }
        super::tip(r, t!("On: Ctrl+B belongs to the app even when the terminal is focused. Off: it goes to the terminal (tmux prefix)."));

        // Item 4: language. The language names stay in their own language, so anyone can find theirs.
        let item4 = Rect::from_min_size(Pos2::new(inner.min.x, item3.max.y), Vec2::new(inner.width(), item_h));
        text_in(p, item4.shrink2(Vec2::new(8.0, 0.0)), Align2::LEFT_CENTER, t!("Language"), theme::ui(12.0), pal.text);
        let seg =
            Rect::from_min_max(Pos2::new(item4.max.x - 4.0 - 140.0, item4.min.y + 2.0), Pos2::new(item4.max.x - 4.0, item4.max.y - 2.0));
        const LANGS: [Lang; 2] = [Lang::En, Lang::PtBr];
        let selected = LANGS.iter().position(|&l| l == current_lang).unwrap_or(0);
        if let Some(i) = super::segmented(ui, seg, Id::new("setting-language"), &["English", "Português"], selected, false) {
            language = Some(LANGS[i]);
        }

        // Dictation: the language Whisper hears, and translating into English.
        separator(p, Pos2::new(inner.min.x + 4.0, item4.max.y + 6.5), Pos2::new(inner.max.x - 4.0, item4.max.y + 6.5));
        let dict = Rect::from_min_size(Pos2::new(inner.min.x, item4.max.y + 13.0), Vec2::new(inner.width(), title_h));
        section_label(p, Rect::from_min_size(Pos2::new(rect.min.x, dict.min.y), Vec2::new(rect.width(), title_h)), t!("DICTATION"));
        let cell_w = (inner.width() - 4.0) / 3.0;
        let grid_top = dict.max.y;
        for (i, (code, name)) in SPOKEN.iter().enumerate() {
            let cell = Rect::from_min_size(
                Pos2::new(inner.min.x + 2.0 + cell_w * (i % 3) as f32, grid_top + chip_h * (i / 3) as f32),
                Vec2::new(cell_w, chip_h),
            )
            .shrink2(Vec2::new(1.0, 1.0));
            let on = whisper_lang == *code;
            let r = super::clickable(ui, cell, Id::new(("setting-spoken", *code)));
            if on {
                p.rect_filled(cell, 5.0, theme::accent_soft(theme::accent()));
            } else if r.hovered() {
                p.rect_filled(cell, 5.0, pal.hover);
            }
            let label = match *code {
                "auto" => t!("Auto-detect"),
                "ja" => t!("Japanese"),
                "zh" => t!("Chinese"),
                _ => name,
            };
            text_in(p, cell, Align2::CENTER_CENTER, label, theme::ui(11.5), if on { pal.text_strong } else { pal.text });
            if r.clicked() {
                spoken = Some(*code);
            }
        }
        let tr_row = Rect::from_min_size(Pos2::new(inner.min.x, grid_top + chip_h * 3.0 + 4.0), Vec2::new(inner.width(), item_h));
        let r = super::clickable(ui, tr_row, Id::new("setting-translate"));
        if r.hovered() {
            p.rect_filled(tr_row, 5.0, pal.hover);
        }
        text_in(p, tr_row.shrink2(Vec2::new(8.0, 0.0)), Align2::LEFT_CENTER, t!("Translate to English"), theme::ui(12.0), pal.text);
        checkbox(p, Rect::from_center_size(Pos2::new(tr_row.max.x - 15.0, tr_row.center().y), Vec2::splat(14.0)), translate);
        if r.clicked() {
            toggle_translate = true;
        }
        super::tip(r, t!("Whisper only translates into English."));
        let engine = if engine_label.is_empty() { t!("unavailable").to_owned() } else { engine_label };
        let hint = Rect::from_min_size(Pos2::new(inner.min.x + 8.0, tr_row.max.y + 2.0), Vec2::new(inner.width() - 16.0, 26.0));
        text_in(
            p,
            hint.with_max_y(hint.min.y + 13.0),
            Align2::LEFT_CENTER,
            t!("Speak in any language, get English text"),
            theme::ui(10.5),
            pal.text_3,
        );
        let engine_line = t!("Engine: {engine} · {model}", engine, model = holt_core::whisper::model_name());
        text_in(p, hint.with_min_y(hint.min.y + 13.0), Align2::LEFT_CENTER, &engine_line, theme::ui(10.5), pal.text_3);

        separator(p, Pos2::new(inner.min.x + 4.0, item4.max.y + dict_h - 6.5), Pos2::new(inner.max.x - 4.0, item4.max.y + dict_h - 6.5));

        // ▶ Test sound
        let btn = Rect::from_min_size(Pos2::new(inner.min.x + 2.0, item4.max.y + dict_h), Vec2::new(inner.width() - 4.0, btn_h));
        if button(ui, btn, Id::new("setting-test-sound"), t!("▶ Test sound"), ButtonKind::Secondary).clicked() {
            test = true;
        }

        // Voice dictation (local Whisper): download it later, or delete it to free the space.
        separator(p, Pos2::new(inner.min.x + 4.0, btn.max.y + 6.5), Pos2::new(inner.max.x - 4.0, btn.max.y + 6.5));
        let row = Rect::from_min_size(Pos2::new(inner.min.x, btn.max.y + 13.0), Vec2::new(inner.width(), item_h));
        text_in(p, row.shrink2(Vec2::new(8.0, 0.0)), Align2::LEFT_CENTER, t!("Voice dictation"), theme::ui(12.0), pal.text);
        let (status, status_color) = match setup {
            Setup::Ready => (t!("installed"), pal.success),
            Setup::Downloading => (t!("downloading…"), pal.text_3),
            Setup::NotInstalled => (t!("not installed"), pal.text_3),
        };
        text_in(p, row.shrink2(Vec2::new(8.0, 0.0)), Align2::RIGHT_CENTER, status, theme::meta(10.5), status_color);
        let action = Rect::from_min_size(Pos2::new(inner.min.x + 2.0, row.max.y), Vec2::new(inner.width() - 4.0, btn_h));
        let id = Id::new("setting-dictation");
        let size = crate::dictation::download_size();
        match setup {
            Setup::Downloading => {
                text_in(
                    p,
                    action.shrink2(Vec2::new(6.0, 0.0)),
                    Align2::LEFT_CENTER,
                    t!("Progress is shown in the right bar."),
                    theme::ui(11.5),
                    pal.text_3,
                );
            }
            Setup::NotInstalled => {
                let r = button(ui, action, id, &t!("Download ({size})", size), ButtonKind::Secondary);
                dictation_click |= super::tip(r, t!("Downloads Whisper for voice dictation; the audio stays on this computer.")).clicked();
            }
            Setup::Ready if confirm_remove => {
                let r = button_danger(ui, action, id, t!("Confirm: delete from disk"));
                dictation_click |= super::tip(r, t!("Deletes the model and the CUDA engine. You can download them again later.")).clicked();
            }
            Setup::Ready => {
                let r = button(ui, action, id, t!("Remove from disk"), ButtonKind::Secondary);
                dictation_click |=
                    super::tip(r, t!("Deletes Whisper ({size}) to free up space; you can download it again.", size)).clicked();
            }
        }
    });
    if toggle {
        deck.sound_enabled = !deck.sound_enabled;
        let v = deck.sound_enabled;
        deck.core.update_config(|c| c.sound_enabled = v);
    }
    if toggle_tabs {
        deck.auto_open_agent_tabs = !deck.auto_open_agent_tabs;
        let v = deck.auto_open_agent_tabs;
        deck.core.update_config(|c| c.auto_open_agent_tabs = v);
    }
    if toggle_keys {
        deck.app_shortcuts_first = !deck.app_shortcuts_first;
        let v = deck.app_shortcuts_first;
        deck.core.update_config(|c| c.app_shortcuts_first = v);
    }
    if let Some(lang) = language {
        // `update_config` applies the language at once: the next frame is already in it. The dictation language
        // follows it unless the user chose another one.
        deck.core.update_config(|c| {
            if c.whisper_lang == c.language.whisper_code() {
                c.whisper_lang = lang.whisper_code().into();
            }
            c.language = lang;
        });
    }
    // Read from the config at each dictation: the change applies to the next one.
    if let Some(code) = spoken {
        deck.core.update_config(|c| c.whisper_lang = code.into());
    }
    if toggle_translate {
        deck.core.update_config(|c| c.whisper_translate = !c.whisper_translate);
    }
    if test {
        deck.play_sound(true);
    }
    if dictation_click {
        match setup {
            Setup::NotInstalled => deck.dictation_accept(),
            Setup::Ready if confirm_remove => deck.dictation_remove(),
            Setup::Ready => deck.dictation_confirm_remove = true,
            Setup::Downloading => {}
        }
    }
    if closes_on_outside(ctx, menu_rect, anchor) {
        deck.settings_open = false;
    }
}

/// Spoken languages offered for dictation: Whisper code and name. Japanese and Chinese are written in English
/// because the embedded fonts have no CJK glyphs.
const SPOKEN: [(&str, &str); 9] = [
    ("auto", "Auto-detect"),
    ("en", "English"),
    ("pt", "Português"),
    ("es", "Español"),
    ("fr", "Français"),
    ("de", "Deutsch"),
    ("it", "Italiano"),
    ("ja", "Japanese"),
    ("zh", "Chinese"),
];

/// Checkbox with a dynamic `accent-color`.
fn checkbox(p: &egui::Painter, r: Rect, checked: bool) {
    let pal = theme::pal();
    if checked {
        p.rect_filled(r, 3.0, theme::accent());
        let s = Stroke::new(1.8, pal.on_accent);
        let a = Pos2::new(r.min.x + 3.0, r.center().y + 0.5);
        let b = Pos2::new(r.min.x + 6.0, r.max.y - 3.5);
        let c = Pos2::new(r.max.x - 3.0, r.min.y + 3.5);
        p.line_segment([a, b], s);
        p.line_segment([b, c], s);
    } else {
        p.rect(r, 3.0, pal.recess, Stroke::new(1.0, pal.separator), egui::StrokeKind::Inside);
    }
}

/// Context menu on a right click on a tab.
pub fn tab_context_menu(deck: &mut Deck, ctx: &egui::Context) {
    let Some((idx, pos)) = deck.tab_context_menu else { return };
    if idx >= deck.tabs.len() {
        deck.tab_context_menu = None;
        return;
    }
    // When the instance has an LLM model, it shows at the top (read only: changing it with the tab open
    // would require restarting the agent).
    let model = deck.tabs[idx].model.clone();
    let recording = deck.tabs[idx].session.recording();
    // (action, text, hint): "Record" and "Snapshot" are the terminal diagnostics (see `terminal/record.rs`).
    let items: [(TabAction, &str, Option<&str>); 4] = [
        (TabAction::Rename, t!("Rename"), None),
        (
            TabAction::Record,
            if recording { t!("Stop recording the terminal") } else { t!("Record the terminal (diagnostics)") },
            Some(t!("Keeps what this tab's program writes to the terminal, to track down display bugs. Uses disk space while on.")),
        ),
        (
            TabAction::Snapshot,
            t!("Save a snapshot of the grid now"),
            Some(t!("Saves the text the terminal holds now (screen and history) and opens the folder. Use it when the screen goes wrong.")),
        ),
        (TabAction::Close, t!("Close"), None),
    ];
    let w = 250.0;
    let item_h = 26.0;
    let h = item_h * (items.len() as f32 + if model.is_some() { 1.0 } else { 0.0 }) + 8.0;
    let pos = keep_on_screen(ctx, pos, Vec2::new(w, h));
    let menu_rect = Rect::from_min_size(pos, Vec2::new(w, h));
    let mut action = None;
    let pal = theme::pal();
    Area::new(Id::new("tab-context-menu")).order(Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
        let p = ui.painter();
        shadow(p, rect, 8, 8, 24, pal.shadow);
        frame(p, rect, 8.0, pal.popup, pal.glass_border);
        let font = theme::ui(12.0);

        let mut top = rect.min.y + 4.0;
        if let Some(m) = &model {
            let r0 = Rect::from_min_size(Pos2::new(rect.min.x + 4.0, top), Vec2::new(w - 8.0, item_h));
            top += item_h;
            let g = super::galley_trunc(p, &t!("Model: {model}", model = m), theme::meta(10.5), pal.text_3, r0.width() - 16.0);
            super::place(p, r0.shrink2(Vec2::new(8.0, 0.0)), Align2::LEFT_CENTER, g, pal.text_3);
            separator(p, Pos2::new(r0.min.x + 4.0, r0.max.y - 0.5), Pos2::new(r0.max.x - 4.0, r0.max.y - 0.5));
        }
        for (n, (what, label, hint)) in items.iter().enumerate() {
            let r = Rect::from_min_size(Pos2::new(rect.min.x + 4.0, top + n as f32 * item_h), Vec2::new(w - 8.0, item_h));
            let mut resp = super::clickable(ui, r, Id::new(("tab-ctx", n)));
            if let Some(hint) = hint {
                resp = super::tip(resp, *hint);
            }
            if resp.hovered() {
                p.rect_filled(r, 5.0, pal.hover);
            }
            // Diagnostics on show in the item itself: danger color, like a "REC".
            let color = if *what == TabAction::Record && recording { pal.danger_text } else { pal.text };
            text_in(p, r.shrink2(Vec2::new(8.0, 0.0)), Align2::LEFT_CENTER, label, font.clone(), color);
            if resp.clicked() {
                action = Some(*what);
            }
        }
    });

    match action {
        Some(what) => {
            deck.tab_context_menu = None;
            match what {
                TabAction::Rename => deck.start_rename_tab(idx),
                TabAction::Record => deck.toggle_terminal_recording(idx),
                TabAction::Snapshot => deck.save_terminal_snapshot(idx),
                TabAction::Close => deck.close_tab(idx),
            }
        }
        None => {
            if ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !menu_rect.contains(p))) {
                deck.tab_context_menu = None;
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum TabAction {
    Rename,
    Record,
    Snapshot,
    Close,
}

/// Modal to rename an agent / instance (format model@role@name).
pub fn rename_tab_modal(deck: &mut Deck, ctx: &egui::Context) {
    let Some((idx, mut buffer, error)) = deck.rename_tab.clone() else { return };
    if idx >= deck.tabs.len() {
        deck.rename_tab = None;
        return;
    }
    let screen = ctx.content_rect();
    super::modal_backdrop(deck, ctx, "rename-backdrop");

    let w = 360.0;
    let h = if error.is_some() { 162.0 } else { 142.0 };
    let center = screen.center();
    let rect = Rect::from_center_size(center, Vec2::new(w, h));
    let mut close = false;
    let mut apply = false;
    let pal = theme::pal();

    Area::new(Id::new("rename-tab-modal")).order(Order::Tooltip).fixed_pos(rect.min).show(ctx, |ui| {
        let (r, _) = ui.allocate_exact_size(rect.size(), Sense::hover());
        let p = ui.painter().clone();
        shadow(&p, r, 10, 8, 24, pal.shadow);
        frame(&p, r, 10.0, pal.popup, pal.glass_border);

        let mut y = r.min.y + 14.0;
        // Title (13 px 600)
        let title_g = super::galley(&p, t!("Rename Agent"), theme::ui_semibold(13.0), pal.text_strong);
        super::place(
            &p,
            Rect::from_min_size(Pos2::new(r.min.x + 16.0, y), Vec2::new(w - 32.0, 18.0)),
            Align2::LEFT_CENTER,
            title_g,
            pal.text_strong,
        );
        y += 22.0;

        // Explanation
        let hint_g = super::galley(&p, t!("Format: model@role@name (e.g. claude@dev or antigravity@lead)"), theme::ui(11.5), pal.text_2);
        super::place(
            &p,
            Rect::from_min_size(Pos2::new(r.min.x + 16.0, y), Vec2::new(w - 32.0, 16.0)),
            Align2::LEFT_CENTER,
            hint_g,
            pal.text_2,
        );
        y += 22.0;

        // Text box (recess, separator border and accent on focus)
        let input_id = Id::new("rename-tab-input");
        let focused = ui.ctx().memory(|m| m.has_focus(input_id));
        let input_r = Rect::from_min_size(Pos2::new(r.min.x + 16.0, y), Vec2::new(w - 32.0, 28.0));
        p.rect_filled(input_r, 6.0, pal.recess);
        p.rect_stroke(input_r, 6.0, Stroke::new(1.0, if focused { theme::accent() } else { pal.separator }), egui::StrokeKind::Inside);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(input_r.shrink2(Vec2::new(8.0, 4.0))));
        let te = egui::TextEdit::singleline(&mut buffer)
            .id(input_id)
            .frame(egui::Frame::NONE)
            .font(theme::mono(12.0))
            .text_color(pal.text)
            .desired_width(input_r.width() - 16.0);
        let out = te.show(&mut child);
        out.response.request_focus();
        if out.response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            apply = true;
        }
        if ui.input(|i| i.key_pressed(Key::Enter)) {
            apply = true;
        }
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            close = true;
        }
        y += 34.0;

        // The error, if any
        if let Some(err) = &error {
            let err_g = super::galley_trunc(&p, err, theme::ui(11.5), pal.danger_text, w - 32.0);
            super::place(
                &p,
                Rect::from_min_size(Pos2::new(r.min.x + 16.0, y), Vec2::new(w - 32.0, 16.0)),
                Align2::LEFT_CENTER,
                err_g,
                pal.danger_text,
            );
            y += 20.0;
        }

        // Cancel and Save buttons
        let btn_h = 26.0;
        let save_w = button_width(&p, t!("Save"), ButtonKind::Primary);
        let cancel_w = button_width(&p, t!("Cancel"), ButtonKind::Secondary);
        let save_r = Rect::from_min_size(Pos2::new(r.max.x - 16.0 - save_w, y), Vec2::new(save_w, btn_h));
        let cancel_r = Rect::from_min_size(Pos2::new(save_r.min.x - 8.0 - cancel_w, y), Vec2::new(cancel_w, btn_h));

        if button(ui, cancel_r, Id::new("rename-cancel-btn"), t!("Cancel"), ButtonKind::Secondary).clicked() {
            close = true;
        }
        if button(ui, save_r, Id::new("rename-save-btn"), t!("Save"), ButtonKind::Primary).clicked() {
            apply = true;
        }
    });

    if close {
        deck.rename_tab = None;
    } else if apply {
        match deck.apply_rename_tab(idx, &buffer) {
            Ok(()) => {}
            Err(e) => {
                deck.rename_tab = Some((idx, buffer, Some(e)));
            }
        }
    } else {
        deck.rename_tab = Some((idx, buffer, error));
    }
}

/// The voice dictation question: on the first launch (no Whisper downloaded) and on a click on the microphone without it.
/// "Not now" (or Esc) saves the "no" and nothing is downloaded; only the download button starts the download.
pub fn dictation_modal(deck: &mut Deck, ctx: &egui::Context) {
    if !deck.dictation_prompt {
        return;
    }
    super::modal_backdrop(deck, ctx, "dictation-backdrop");
    let pal = theme::pal();
    let w = 400.0;
    let pad = 20.0;
    let size = crate::dictation::download_size();
    let intro = t!("Speak instead of typing: Holt transcribes your voice on this computer, without sending the audio over the internet.");
    let detail = t!(
        "To do that it downloads Whisper (transcription model and engine): {size}, plus about 700 MB of the CUDA engine if there is an NVIDIA card. \
         The download runs in the background and progress appears in the right bar.",
        size
    );
    let later = t!("You can download or remove dictation later, in Settings.");
    let mut accept = false;
    let mut decline = ctx.input(|i| i.key_pressed(Key::Escape));

    // The text wraps: measure it first to know the card's height.
    let measure = ctx.fonts_mut(|f| {
        let mut lay = |t: &str, font: egui::FontId| f.layout(t.to_owned(), font, pal.text_2, w - pad * 2.0).size().y;
        (lay(intro, theme::ui(12.0)), lay(&detail, theme::ui(12.0)), lay(later, theme::ui(11.5)))
    });
    let h = 14.0 + 22.0 + measure.0 + 8.0 + measure.1 + 8.0 + measure.2 + 18.0 + 28.0 + 16.0;
    let rect = Rect::from_center_size(ctx.content_rect().center(), Vec2::new(w, h));
    Area::new(Id::new("dictation-modal")).order(Order::Tooltip).fixed_pos(rect.min).show(ctx, |ui| {
        let (r, _) = ui.allocate_exact_size(rect.size(), Sense::hover());
        let p = ui.painter().clone();
        shadow(&p, r, 10, 8, 24, pal.shadow);
        frame(&p, r, 10.0, pal.popup, pal.glass_border);
        let mut y = r.min.y + 14.0;
        let title = super::galley(&p, t!("Voice dictation (optional)"), theme::ui_semibold(13.0), pal.text_strong);
        super::place(
            &p,
            Rect::from_min_size(Pos2::new(r.min.x + pad, y), Vec2::new(w - pad * 2.0, 18.0)),
            Align2::LEFT_CENTER,
            title,
            pal.text_strong,
        );
        y += 22.0;
        for (text, font, color) in
            [(intro, theme::ui(12.0), pal.text), (detail.as_str(), theme::ui(12.0), pal.text_2), (later, theme::ui(11.5), pal.text_3)]
        {
            let g = p.layout(text.to_owned(), font, color, w - pad * 2.0);
            let h = g.size().y;
            p.galley(Pos2::new(r.min.x + pad, y), g, color);
            y += h + 8.0;
        }
        y += 10.0;
        let download = t!("Download ({size})", size);
        let accept_w = button_width(&p, &download, ButtonKind::Primary);
        let decline_w = button_width(&p, t!("Not now"), ButtonKind::Secondary);
        let accept_r = Rect::from_min_size(Pos2::new(r.max.x - pad - accept_w, y), Vec2::new(accept_w, 28.0));
        let decline_r = Rect::from_min_size(Pos2::new(accept_r.min.x - 8.0 - decline_w, y), Vec2::new(decline_w, 28.0));
        if button(ui, decline_r, Id::new("dictation-decline-btn"), t!("Not now"), ButtonKind::Secondary).clicked() {
            decline = true;
        }
        if button(ui, accept_r, Id::new("dictation-accept-btn"), &download, ButtonKind::Primary).clicked() {
            accept = true;
        }
    });
    if accept {
        deck.dictation_accept();
    } else if decline {
        deck.dictation_decline();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_some_agents_offer_models_in_the_menu() {
        for k in ["claude", "antigravity", "opencode"] {
            assert!(picks_model(k), "{k}");
        }
        for k in ["codex", "shell", "aider"] {
            assert!(!picks_model(k), "{k}");
        }
    }

    #[test]
    fn the_status_line_explains_an_empty_list() {
        i18n::english(|| {
            assert_eq!(models_status(true, true), Some("Loading models…"));
            assert_eq!(models_status(false, true), Some("No model list: default only"));
            assert_eq!(models_status(false, false), None);
            assert_eq!(models_status(true, false), None, "with the old list on screen, it updates silently");
        });
    }
}
