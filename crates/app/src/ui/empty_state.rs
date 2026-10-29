//! `#terminals-empty`: "No agent open" card with the buttons to open agents.

use egui::text::{LayoutJob, TextFormat};
use egui::{Align2, Id, Pos2, Rect, Ui, Vec2};

use super::{button, galley, place, ButtonKind};
use crate::deck::Deck;
use crate::icons::Icon;
use crate::theme;
use agentdeck_core::t;

/// "Open folder…" button of the card shown without a project (an internal marker, not user-facing).
const NEW_PROJECT: &str = "#new-project";

/// Marker the empty-state sentence uses to point at the bold piece (a character the text never has).
const MARKER: &str = "\u{1}";

pub fn draw(deck: &mut Deck, ui: &mut Ui, rect: Rect) {
    let painter = ui.painter_at(rect);

    // Without a project there is nowhere to open an agent: the card asks for a folder.
    let no_project = deck.active_project.is_empty();
    let title = if no_project { t!("No project open") } else { t!("No agent open") };
    let title = galley(&painter, title, theme::ui_semibold(16.0), theme::pal().text);
    // <p> with the "+ New agent" in bold, wrapping up to 420px (less in a narrow window).
    let mut job = LayoutJob::default();
    job.wrap.max_width = 420.0f32.min(rect.width() - 48.0).max(120.0);
    job.halign = egui::Align::Center;
    let fmt = TextFormat { font_id: theme::ui(13.0), color: theme::pal().text_2, line_height: Some(13.0 * 1.5), ..Default::default() };
    if no_project {
        job.append(t!("Open a folder as a project: each project has its own agents."), 0.0, fmt);
    } else {
        // With the sidebar collapsed the "+ New agent" row is not in sight: the shortcut is Ctrl+T.
        // The whole sentence is one key; the `{key}` placeholder marks the bold piece.
        let (key, text) = if deck.sidebar_collapsed {
            ("Ctrl+T", t!("Select an agent below or press {key} to start a session:", key = MARKER))
        } else {
            (t!("+ New agent"), t!("Select an agent below or use {key} in the sidebar to start a session:", key = MARKER))
        };
        let (before, after) = text.split_once(MARKER).unwrap_or((text.as_str(), ""));
        job.append(before, 0.0, fmt.clone());
        job.append(key, 0.0, TextFormat { font_id: theme::ui_bold(13.0), color: theme::pal().text, ..fmt.clone() });
        job.append(after, 0.0, fmt);
    }
    let para = painter.layout_job(job);

    // Buttons (.empty-agent-btn): padding 7px 14px, 12px/500, gap 10px, line wrapping.
    let items = if no_project {
        vec![crate::agents::MenuItem { kind: NEW_PROJECT.into(), label: t!("Open folder…").into(), color: theme::pal().text_2 }]
    } else {
        deck.agents.empty_actions.clone()
    };
    let font = theme::ui(12.0);
    let btn_h = 12.0 * theme::LINE_HEIGHT + 14.0 + 2.0;
    // Icon on the left: the model logo or, without one, the agent's dot; "Open folder…" has no icon.
    let lead = |m: &crate::agents::MenuItem| if m.kind == NEW_PROJECT { 0.0 } else { 8.0 + 8.0 };
    let widths: Vec<f32> = items
        .iter()
        .map(|m| 14.0 + lead(m) + 6.0 + galley(&painter, &m.label, font.clone(), theme::pal().text).size().x + 14.0 + 2.0)
        .collect();
    let max_row = 420.0f32.min(rect.width() - 48.0);
    let mut rows: Vec<Vec<usize>> = vec![Vec::new()];
    let mut row_w = 0.0;
    for (i, w) in widths.iter().enumerate() {
        let need = if rows.last().is_some_and(|r| r.is_empty()) { *w } else { row_w + 10.0 + w };
        if need > max_row && !rows.last().is_some_and(|r| r.is_empty()) {
            rows.push(vec![i]);
            row_w = *w;
        } else {
            if let Some(r) = rows.last_mut() {
                r.push(i);
            }
            row_w = need;
        }
    }
    let actions_h = rows.len() as f32 * btn_h + (rows.len().saturating_sub(1)) as f32 * 10.0;

    let total_h = 58.0 + 12.0 + title.size().y + 12.0 + para.size().y + 12.0 + 8.0 + actions_h;
    let mut y = rect.center().y - total_h / 2.0;
    let cx = rect.center().x;

    // The app mark (56 px) in place of the old terminal icon
    let icon_box = Rect::from_min_size(Pos2::new(cx - 29.0, y), Vec2::splat(58.0));
    deck.icons.paint(&painter, Icon::AgentDeck, icon_box.center(), 56.0, theme::pal().text_strong, 0.0);
    y = icon_box.max.y + 12.0;

    let tr = Rect::from_min_size(Pos2::new(rect.min.x, y), Vec2::new(rect.width(), title.size().y));
    place(&painter, tr, Align2::CENTER_CENTER, title, theme::pal().text);
    y = tr.max.y + 12.0;
    // With `halign: Center` the lines get centered at x = 0 of the galley.
    let para_h = para.size().y;
    painter.galley(Pos2::new(cx, y), para, theme::pal().text_2);
    y += para_h + 12.0 + 8.0;

    let mut open = None;
    for row in rows {
        let rw: f32 = row.iter().map(|&i| widths[i]).sum::<f32>() + 10.0 * row.len().saturating_sub(1) as f32;
        let mut x = cx - rw / 2.0;
        for i in row {
            let m = &items[i];
            let r = Rect::from_min_size(Pos2::new(x, y), Vec2::new(widths[i], btn_h));
            x += widths[i] + 10.0;
            // The base secondary button (border, background and hover); the agent's dot and name go on top.
            let resp = button(ui, r, Id::new(("empty-btn", &m.kind)), "", ButtonKind::Secondary);
            if m.kind != NEW_PROJECT {
                super::projects::paint_kind_mark(
                    &mut deck.icons,
                    &painter,
                    Pos2::new(r.min.x + 15.0 + 4.0, r.center().y),
                    &m.kind,
                    m.color,
                );
            }
            let g = galley(&painter, &m.label, font.clone(), theme::pal().text);
            place(
                &painter,
                Rect::from_min_max(Pos2::new(r.min.x + 15.0 + lead(m) + 6.0, r.min.y), r.max),
                Align2::LEFT_CENTER,
                g,
                theme::pal().text,
            );
            if resp.clicked() {
                open = Some(m.kind.clone());
            }
        }
        y += btn_h + 10.0;
    }
    match open.as_deref() {
        Some(NEW_PROJECT) => deck.new_project_dialog(),
        Some(kind) => deck.open_tab(kind, false),
        None => {}
    }
}
