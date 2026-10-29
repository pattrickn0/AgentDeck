//! Right sidebar (fixed, does not collapse): "Tasks | Git | GitHub" tabs, the download strip and the body
//! of the active tab. (System notices live in the title bar's bell, see `ui/notices.rs`.)

use std::time::Duration;

use egui::{Align2, CursorIcon, Id, Pos2, Rect, Sense, Ui, Vec2};

use super::{galley, galley_trunc, paint_splitter, place, separator, text_in, text_w, tip};
use crate::deck::{Deck, RightView};
use crate::theme;
use holt_core::i18n::{self, Lang};
use holt_core::{t, DownloadProgress, DownloadState};

/// Height of the download strip (below the header, only during a download).
const DOWNLOAD_H: f32 = 22.0;

/// Draws the bar in `rect` (header as tall as the topbar and body of the active tab).
pub fn draw_panel(deck: &mut Deck, ui: &mut Ui, rect: Rect) {
    let head = Rect::from_min_size(rect.min, Vec2::new(rect.width(), theme::TOPBAR_H));
    draw_head(deck, ui, head);
    let mut body_top = head.max.y;
    if let Some((p, _)) = &deck.download {
        let strip = Rect::from_min_size(Pos2::new(rect.min.x, body_top), Vec2::new(rect.width(), DOWNLOAD_H));
        ui.painter().rect_filled(strip, 0.0, theme::pal().chip);
        draw_download(ui, ui.painter(), p, strip, strip.min.x + 12.0, strip.max.x - 12.0);
        body_top = strip.max.y;
    }
    let body = Rect::from_min_max(Pos2::new(rect.min.x, body_top), rect.max);
    if body.height() > 1.0 {
        match deck.right_view {
            RightView::Tasks => super::tasks::draw(deck, ui, body),
            RightView::Git => super::git::draw(deck, ui, body),
            RightView::GitHub => super::github::draw(deck, ui, body),
        }
    }
}

/// One tab of the header: label, hint (tooltip) and optional counter.
struct HeadTab<'a> {
    id: &'static str,
    label: &'static str,
    tip: &'a str,
    badge: u32,
}

impl HeadTab<'_> {
    fn font(active: bool) -> egui::FontId {
        if active {
            theme::ui_semibold(11.5)
        } else {
            theme::ui(11.5)
        }
    }

    fn badge(&self, active: bool) -> Option<(String, egui::FontId)> {
        let text = if self.badge > 99 { "99+".to_owned() } else { self.badge.to_string() };
        (self.badge > 0).then(|| (text, if active { theme::ui_bold(10.0) } else { theme::meta(10.0) }))
    }

    /// Width of the badge and of the space before it (0 without a badge).
    fn badge_w(&self, painter: &egui::Painter, active: bool) -> f32 {
        self.badge(active).map_or(0.0, |(t, font)| 5.0 + (text_w(painter, &t, font) + 10.0).max(14.0))
    }
}

/// Paints a tab as a pill (22 px tall, corners 5) in the header of the right bar, with `pad` of inner space
/// (summed on both sides) and the label up to `label_max` px. Returns `(width, clicked)`.
fn head_tab(ui: &Ui, painter: &egui::Painter, at: Pos2, tab: HeadTab<'_>, active: bool, pad: f32, label_max: f32) -> (f32, bool) {
    let p = theme::pal();
    let font = HeadTab::font(active);
    let badge_w = tab.badge_w(painter, active);
    let w = pad + label_max + badge_w;
    let rect = Rect::from_min_size(at, Vec2::new(w, 22.0));
    let r = super::clickable(ui, rect, Id::new(("right-tab", tab.id)));
    let hovered = r.hovered();
    if active {
        painter.rect_filled(rect, 5.0, p.selected);
    } else if hovered {
        painter.rect_filled(rect, 5.0, p.hover);
    }
    let fg = if active {
        p.text_strong
    } else if hovered {
        p.text
    } else {
        p.text_2
    };
    let text_rect = Rect::from_min_size(Pos2::new(rect.min.x + pad / 2.0, rect.min.y), Vec2::new(label_max, rect.height()));
    place(painter, text_rect, Align2::LEFT_CENTER, galley_trunc(painter, tab.label, font, fg, label_max + 0.5), fg);
    if let Some((t, badge_font)) = tab.badge(active) {
        let pill = Rect::from_min_size(Pos2::new(text_rect.max.x + 5.0, rect.center().y - 7.0), Vec2::new(badge_w - 5.0, 14.0));
        let (pill_fill, pill_fg) = if active { (theme::accent(), p.on_accent) } else { (p.chip, p.text_2) };
        painter.rect_filled(pill, 7.0, pill_fill);
        text_in(painter, pill, Align2::CENTER_CENTER, &t, badge_font, pill_fg);
    }
    let tip_text = if label_max + 0.5 < text_w(painter, tab.label, HeadTab::font(active)) {
        format!("{}\n{}", tab.label, tab.tip)
    } else {
        tab.tip.to_owned()
    };
    (w, tip(r, tip_text).clicked())
}

fn draw_head(deck: &mut Deck, ui: &mut Ui, rect: Rect) {
    let painter = ui.painter_at(rect);
    separator(&painter, Pos2::new(rect.min.x, rect.max.y - 0.5), Pos2::new(rect.max.x, rect.max.y - 0.5));
    let view = deck.right_view;
    let git_st = deck.git.status();
    let git_badge = git_st.as_ref().map(|s| s.entries.len() as u32).unwrap_or(0);
    let git_tip = match &git_st {
        Some(st) => t!("Git · {status}", status = crate::git_ui::chip_text(st)),
        None => "Git".to_string(),
    };
    let gh_badge = deck.github.open_prs_count();
    let gh_tip = deck.github.tab_tooltip();
    let (review, validating) = (deck.tasks.review_count() as u32, deck.tasks.validating_count() as u32);
    let review_badge = deck.tasks.attention_count() as u32;
    let tasks_tip = match (review, validating) {
        (0, 0) => t!("Tasks").to_string(),
        (r, 0) => t!("Tasks · {count} in review", count = r),
        (0, v) => t!("Tasks · {count} awaiting validation", count = v),
        (r, v) => t!("Tasks · {review} in review, {validating} awaiting validation", review = r, validating = v),
    };
    let tabs = [
        (RightView::Tasks, HeadTab { id: "tasks", label: t!("Tasks"), tip: &tasks_tip, badge: review_badge }),
        (RightView::Git, HeadTab { id: "git", label: "Git", tip: &git_tip, badge: git_badge }),
        (RightView::GitHub, HeadTab { id: "github", label: "GitHub", tip: &gh_tip, badge: gh_badge }),
    ];
    let y = rect.min.y + (rect.height() - 1.0 - 22.0) * 0.5;
    // In the narrow bar the tabs squeeze the inner space and, if needed, shorten the labels (never the active one's).
    let texts: Vec<f32> = tabs.iter().map(|(t, tab)| text_w(&painter, tab.label, HeadTab::font(view == *t))).collect();
    let fixed: Vec<f32> = tabs.iter().map(|(t, tab)| tab.badge_w(&painter, view == *t)).collect();
    let keep = tabs.iter().position(|(t, _)| *t == view);
    let (pad, label_max) = super::fit_pills(&texts, &fixed, rect.width() - 12.0, (10.0, 16.0), 2.0, keep);
    let mut x = rect.min.x + 6.0;
    for ((target, tab), label_max) in tabs.into_iter().zip(label_max) {
        let (w, clicked) = head_tab(ui, &painter, Pos2::new(x, y), tab, view == target, pad, label_max);
        if clicked {
            deck.set_right_view(target);
        }
        x += w + 2.0;
    }
}

/// Draggable divider on the left edge of the bar (changes the width; the bar never collapses).
/// `cur_w` is the effective width now; `max_w`, the maximum the window allows.
pub fn splitter(deck: &mut Deck, ui: &mut Ui, line: Rect, full: Rect, cur_w: f32, max_w: f32) {
    let hit = line;
    let r = ui.interact(hit, Id::new("right-splitter"), Sense::drag());
    if r.hovered() || r.dragged() {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
    }
    if r.drag_started() {
        if let Some(p) = r.interact_pointer_pos() {
            deck.right_dragging = Some((p.x, cur_w));
        }
    }
    if r.dragged() {
        if let (Some((x0, w0)), Some(p)) = (deck.right_dragging, ui.input(|i| i.pointer.latest_pos())) {
            deck.right_w = (w0 + x0 - p.x).clamp(theme::RIGHT_MIN_W, max_w.max(theme::RIGHT_MIN_W));
        }
    }
    if r.drag_stopped() {
        deck.right_dragging = None;
        deck.persist_layout();
    }
    let hot = r.hovered() || deck.right_dragging.is_some();
    let painter = ui.painter_at(full);
    paint_splitter(&painter, line, hot, deck.accent());
    tip(r, t!("Drag to resize the panel"));
}

/// Text "1.51 GB" / "648.5 MB" / "12 KB". Portuguese writes the decimal separator as a comma
/// (pt-BR: "1,51 GB").
fn format_bytes(n: f64) -> String {
    const KB: f64 = 1024.0;
    let s = if n >= KB * KB * KB {
        format!("{:.2} GB", n / (KB * KB * KB))
    } else if n >= KB * KB {
        format!("{:.1} MB", n / (KB * KB))
    } else {
        format!("{} KB", (n / KB).round().max(1.0))
    };
    s.replace('.', decimal_separator())
}

/// The decimal separator of the current language.
fn decimal_separator() -> &'static str {
    if i18n::lang() == Lang::PtBr {
        ","
    } else {
        "."
    }
}

/// Filled fraction (`None` = indeterminate) and the status text of a download.
fn download_view(p: &DownloadProgress) -> (Option<f32>, String) {
    let total = p.total.filter(|t| *t > 0);
    let pct = total.map(|t| (p.received as f64 / t as f64).clamp(0.0, 1.0));
    let pct_txt = pct.map(|f| format!("{}%", (f * 100.0).floor() as u32));
    match p.state {
        DownloadState::Downloading => {
            let mut info = match (total, &pct_txt) {
                (Some(t), Some(pc)) => format!("{pc} · {} / {}", format_bytes(p.received as f64), format_bytes(t as f64)),
                _ => format_bytes(p.received as f64),
            };
            if p.speed > 0.0 {
                info += &format!(" · {}/s", format_bytes(p.speed));
            }
            (pct.map(|f| f as f32), info)
        }
        DownloadState::Verifying => (
            pct.map(|f| f as f32),
            match pct_txt {
                Some(pc) => t!("verifying integrity (SHA-256) · {pct}", pct = pc),
                None => t!("verifying integrity (SHA-256)…").to_owned(),
            },
        ),
        DownloadState::Retrying => (
            pct.map(|f| f as f32),
            t!(
                "unstable connection, retrying {attempt}/{max} in {seconds}s",
                attempt = p.attempt,
                max = p.max_attempts,
                seconds = (p.retry_in_ms as f64 / 1000.0).round().max(1.0)
            ),
        ),
        DownloadState::Extracting => (None, t!("extracting…").to_owned()),
        DownloadState::Finalizing => (None, t!("finalizing…").to_owned()),
        DownloadState::Error => (Some(0.0), p.message.clone().unwrap_or_else(|| t!("download failed").to_owned())),
        DownloadState::Done => (Some(1.0), String::new()),
    }
}

fn draw_download(ui: &Ui, painter: &egui::Painter, p: &DownloadProgress, head: Rect, left: f32, limit: f32) {
    if limit - left < 160.0 {
        return;
    }
    let painter = painter.with_clip_rect(Rect::from_min_max(Pos2::new(left, head.min.y), Pos2::new(limit, head.max.y)));
    let (frac, info) = download_view(p);
    let error = p.state == DownloadState::Error;
    let font = theme::ui(11.0);
    let cy = head.center().y;

    let label_w = galley(&painter, &p.label, font.clone(), theme::pal().text).size().x;
    text_in(
        &painter,
        Rect::from_min_size(Pos2::new(left, head.min.y), Vec2::new(label_w, head.height())),
        Align2::LEFT_CENTER,
        &p.label,
        font.clone(),
        if error { theme::pal().danger_text } else { theme::pal().text },
    );

    // ×6 track (up to 140px, less in the narrow bar) and the fill (indeterminate: it comes and goes).
    let track_w = ((limit - left) * 0.25).clamp(60.0, 140.0);
    let track = Rect::from_min_size(Pos2::new(left + label_w + 8.0, cy - 3.0), Vec2::new(track_w, 6.0));
    painter.rect_filled(track, 3.0, theme::pal().hover);
    let fill = match frac {
        Some(f) => Some(Rect::from_min_size(track.min, Vec2::new(track.width() * f, track.height()))),
        None => {
            let t = ui.ctx().input(|i| i.time);
            let phase = ((t * 0.9) % 2.0) as f32;
            let pos = if phase < 1.0 { phase } else { 2.0 - phase };
            let w = track.width() * 0.35;
            ui.ctx().request_repaint_after(Duration::from_millis(33));
            Some(Rect::from_min_size(Pos2::new(track.min.x + (track.width() - w) * pos, track.min.y), Vec2::new(w, track.height())))
        }
    };
    if let Some(r) = fill.filter(|r| r.width() > 0.5) {
        painter.rect_filled(r, 3.0, theme::accent());
    }
    let info_rect = Rect::from_min_max(Pos2::new(track.max.x + 8.0, head.min.y), Pos2::new(limit, head.max.y));
    // In the narrow bar the detail ends in ellipsis instead of being cut in the middle.
    let color = if error { theme::pal().danger_text } else { theme::pal().text_2 };
    let g = super::galley_trunc(&painter, &info, font, color, info_rect.width());
    super::place(&painter, info_rect, Align2::LEFT_CENTER, g, color);
}

/// Opacity of the `blink` animation (1 → 0.2 → 1, `ease`) in a period of `period` s.
pub fn blink_alpha(time: f64, period: f64) -> f32 {
    let p = (time % period) / period;
    let half = if p < 0.5 { p * 2.0 } else { (1.0 - p) * 2.0 };
    // approximation of the `ease` curve
    let e = half * half * (3.0 - 2.0 * half);
    (1.0 - 0.8 * e) as f32
}

#[cfg(test)]
mod download_tests {
    use super::*;

    fn p(state: DownloadState, received: u64, total: Option<u64>) -> DownloadProgress {
        DownloadProgress {
            label: "x".into(),
            state,
            received,
            total,
            speed: 0.0,
            attempt: 0,
            max_attempts: 0,
            retry_in_ms: 0,
            message: None,
        }
    }

    #[test]
    fn formats_bytes_per_language() {
        assert_eq!(format_bytes(680_000_000.0), "648.5 MB");
        assert_eq!(format_bytes(1_624_555_275.0), "1.51 GB");
        assert_eq!(format_bytes(12.0), "1 KB");
        i18n::with_lang(Lang::PtBr, || {
            assert_eq!(format_bytes(680_000_000.0), "648,5 MB");
            assert_eq!(format_bytes(1_624_555_275.0), "1,51 GB");
            assert_eq!(format_bytes(12.0), "1 KB");
        });
    }

    #[test]
    fn status_text_of_each_state() {
        let mut d = p(DownloadState::Downloading, 680_000_000, Some(1_624_555_275));
        d.speed = 12_900_000.0;
        let (f, t) = download_view(&d);
        assert_eq!(t, "41% · 648.5 MB / 1.51 GB · 12.3 MB/s");
        assert!((f.expect("fraction") - 0.4186).abs() < 0.001);
        assert_eq!(download_view(&p(DownloadState::Downloading, 5_000_000, None)), (None, "4.8 MB".into()));
        assert_eq!(download_view(&p(DownloadState::Verifying, 812_277_637, Some(1_624_555_275))).1, "verifying integrity (SHA-256) · 49%");
        let mut r = p(DownloadState::Retrying, 100, Some(1000));
        (r.attempt, r.max_attempts, r.retry_in_ms) = (2, 5, 4200);
        assert_eq!(download_view(&r).1, "unstable connection, retrying 2/5 in 4s");
        assert_eq!(download_view(&p(DownloadState::Extracting, 0, None)), (None, "extracting…".into()));
        let mut e = p(DownloadState::Error, 0, None);
        e.message = Some("SHA-256 mismatch".into());
        assert_eq!(download_view(&e), (Some(0.0), "SHA-256 mismatch".into()));
    }

    #[test]
    fn status_text_of_each_state_in_portuguese() {
        i18n::with_lang(Lang::PtBr, || {
            let mut d = p(DownloadState::Downloading, 680_000_000, Some(1_624_555_275));
            d.speed = 12_900_000.0;
            let (f, t) = download_view(&d);
            assert_eq!(t, "41% · 648,5 MB / 1,51 GB · 12,3 MB/s");
            assert!((f.expect("fraction") - 0.4186).abs() < 0.001);
            assert_eq!(download_view(&p(DownloadState::Downloading, 5_000_000, None)), (None, "4,8 MB".into()));
            assert_eq!(
                download_view(&p(DownloadState::Verifying, 812_277_637, Some(1_624_555_275))).1,
                "verificando integridade (SHA-256) · 49%"
            );
            let mut r = p(DownloadState::Retrying, 100, Some(1000));
            (r.attempt, r.max_attempts, r.retry_in_ms) = (2, 5, 4200);
            assert_eq!(download_view(&r).1, "conexão instável, nova tentativa 2/5 em 4s");
            assert_eq!(download_view(&p(DownloadState::Extracting, 0, None)), (None, "extraindo…".into()));
            let mut e = p(DownloadState::Error, 0, None);
            e.message = Some("SHA-256 não confere".into());
            assert_eq!(download_view(&e), (Some(0.0), "SHA-256 não confere".into()));
        });
    }
}
