//! Barra lateral direita (fixa, não recolhe): abas "Conversa | Mapa", faixa de download,
//! mensagens da Conversa integrada (ou o mapa de agentes) e o compositor colado embaixo.

use std::time::Duration;

use egui::text::{LayoutJob, TextFormat};
use egui::{Align, Align2, Color32, CursorIcon, Id, Pos2, Rect, Sense, Ui, Vec2};

use super::{composer, galley, map, paint_splitter, place, text_in, tip};
use crate::deck::{Deck, FeedItem, RightView};
use crate::icons::Icon;
use crate::theme;
use commanddeck_core::{DownloadProgress, DownloadState, FeedKind};

/// Altura da faixa de download (abaixo do cabeçalho, só durante um download).
const DOWNLOAD_H: f32 = 22.0;

/// Desenha a barra em `rect` (cabeçalho da altura da topbar, corpo e compositor).
pub fn draw_panel(deck: &mut Deck, ui: &mut Ui, rect: Rect, comp_h: f32) {
    ui.painter().rect_filled(rect, 0.0, theme::BG);
    let head = Rect::from_min_size(rect.min, Vec2::new(rect.width(), theme::TOPBAR_H));
    draw_head(deck, ui, head);
    let mut body_top = head.max.y;
    if let Some((p, _)) = &deck.download {
        let strip = Rect::from_min_size(Pos2::new(rect.min.x, body_top), Vec2::new(rect.width(), DOWNLOAD_H));
        ui.painter().rect_filled(strip, 0.0, theme::PANEL);
        draw_download(ui, ui.painter(), p, strip, strip.min.x + 12.0, strip.max.x - 12.0);
        body_top = strip.max.y;
    }
    let comp = Rect::from_min_max(Pos2::new(rect.min.x, rect.max.y - comp_h), rect.max);
    let body = Rect::from_min_max(Pos2::new(rect.min.x, body_top), Pos2::new(rect.max.x, comp.min.y));
    if body.height() > 1.0 {
        match deck.right_view {
            RightView::Conversa => draw_feed(deck, ui, body),
            RightView::Mapa => map::draw(deck, ui, body),
            RightView::Git => super::git::draw(deck, ui, body),
            RightView::GitHub => super::github::draw(deck, ui, body),
        }
    }
    if comp_h > 0.0 {
        composer::draw(deck, ui, comp);
    }
}

/// Uma aba do cabeçalho: ícone, dica (tooltip) e o número de não vistas.
struct HeadTab<'a> {
    id: &'static str,
    icon: Icon,
    tip: &'a str,
    badge: u32,
}

/// Largura de cada aba do cabeçalho (fixa: o número de não vistas fica sobre o ícone).
const HEAD_TAB_W: f32 = 36.0;

/// Pinta uma aba do cabeçalho. Devolve se foi clicada.
fn head_tab(deck: &mut Deck, ui: &Ui, painter: &egui::Painter, rect: Rect, tab: HeadTab<'_>, active: bool) -> bool {
    let r = super::clickable(ui, rect, Id::new(("right-tab", tab.id)));
    let hovered = r.hovered();
    if active || hovered {
        painter.rect_filled(rect.shrink2(Vec2::new(2.0, 6.0)), 6.0, if active { theme::PANEL_2 } else { theme::PANEL });
    }
    let color = if active || hovered { theme::TEXT } else { theme::MUTED };
    let center = rect.center();
    deck.icons.paint(painter, tab.icon, center, 16.0, color, 0.0);
    if tab.badge > 0 {
        // Pílula sobre o canto superior direito do ícone.
        let txt = if tab.badge > 99 { "99+".to_owned() } else { tab.badge.to_string() };
        let g = galley(painter, &txt, theme::ui(10.0), theme::WHITE);
        let size = Vec2::new((g.size().x + 8.0).max(14.0), 14.0);
        let b = Rect::from_min_size(Pos2::new(center.x + 3.0, center.y - 13.0), size);
        painter.rect_filled(b.expand(1.5), 8.0, theme::PANEL);
        painter.rect_filled(b, 7.0, theme::ACCENT);
        place(painter, b, Align2::CENTER_CENTER, g, theme::WHITE);
    }
    if active {
        // Sublinhado na cor do agente ativo.
        let line = Rect::from_min_max(Pos2::new(rect.min.x + 6.0, rect.max.y - 2.0), Pos2::new(rect.max.x - 6.0, rect.max.y));
        painter.rect_filled(line, 1.0, deck.active_color());
    }
    tip(r, tab.tip).clicked()
}

fn draw_head(deck: &mut Deck, ui: &mut Ui, rect: Rect) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::PANEL);
    // Borda inferior alinhada à da topbar.
    painter.rect_filled(Rect::from_min_max(Pos2::new(rect.min.x, rect.max.y - 1.0), rect.max), 0.0, theme::BORDER);
    let view = deck.right_view;
    let git_st = deck.git.status();
    let git_badge = git_st.as_ref().map(|s| s.entries.len() as u32).unwrap_or(0);
    let git_tip = match &git_st {
        Some(st) => format!("Git · {}", crate::git_ui::chip_text(st)),
        None => "Git".to_string(),
    };
    let gh_badge = deck.github.open_prs_count();
    let gh_tip = deck.github.tab_tooltip();
    let tabs = [
        (RightView::Conversa, HeadTab { id: "conversa", icon: Icon::Chat, tip: "Conversa integrada", badge: deck.unseen }),
        (RightView::Mapa, HeadTab { id: "mapa", icon: Icon::Diagram, tip: "Mapa de agentes", badge: 0 }),
        (RightView::Git, HeadTab { id: "git", icon: Icon::GitBranch, tip: &git_tip, badge: git_badge }),
        (RightView::GitHub, HeadTab { id: "github", icon: Icon::GitHub, tip: &gh_tip, badge: gh_badge }),
    ];
    let mut x = rect.min.x + 6.0;
    for (target, tab) in tabs {
        let r = Rect::from_min_size(Pos2::new(x, rect.min.y), Vec2::new(HEAD_TAB_W, rect.height() - 1.0));
        if head_tab(deck, ui, &painter, r, tab, view == target) {
            deck.set_right_view(target);
        }
        x = r.max.x + 2.0;
    }
    // "Limpar" à direita, só na Conversa.
    if view == RightView::Conversa {
        let font = theme::ui(13.0);
        let cw = galley(&painter, "Limpar", font.clone(), theme::MUTED).size().x + 8.0;
        let clear = Rect::from_min_size(Pos2::new(rect.max.x - 10.0 - cw, rect.min.y + 6.0), Vec2::new(cw, rect.height() - 13.0));
        let r = super::clickable(ui, clear, Id::new("feed-clear"));
        text_in(&painter, clear, Align2::CENTER_CENTER, "Limpar", font, if r.hovered() { theme::TEXT } else { theme::MUTED });
        if tip(r, "Limpar a conversa integrada").clicked() {
            deck.clear_feed();
        }
    }
}

/// Divisor arrastável na borda esquerda da barra (muda a largura; a barra nunca recolhe).
/// `cur_w` é a largura efetiva agora; `max_w`, o máximo que a janela permite.
pub fn splitter(deck: &mut Deck, ui: &mut Ui, line: Rect, full: Rect, cur_w: f32, max_w: f32) {
    let hit = line.expand2(Vec2::new(3.0, 0.0));
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
    paint_splitter(&painter, line, hot, deck.active_color());
    tip(r, "Arraste para redimensionar a conversa");
}

/// Texto "1,51 GB" / "648,5 MB" / "12 KB".
fn format_bytes(n: f64) -> String {
    const KB: f64 = 1024.0;
    let s = if n >= KB * KB * KB {
        format!("{:.2} GB", n / (KB * KB * KB))
    } else if n >= KB * KB {
        format!("{:.1} MB", n / (KB * KB))
    } else {
        format!("{} KB", (n / KB).round().max(1.0))
    };
    s.replace('.', ",")
}

/// Fração preenchida (`None` = indeterminada) e o texto de status de um download.
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
                Some(pc) => format!("verificando integridade (SHA-256) · {pc}"),
                None => "verificando integridade (SHA-256)…".into(),
            },
        ),
        DownloadState::Retrying => (
            pct.map(|f| f as f32),
            format!(
                "conexão instável, nova tentativa {}/{} em {}s",
                p.attempt,
                p.max_attempts,
                (p.retry_in_ms as f64 / 1000.0).round().max(1.0)
            ),
        ),
        DownloadState::Extracting => (None, "extraindo…".into()),
        DownloadState::Finalizing => (None, "concluindo…".into()),
        DownloadState::Error => (Some(0.0), p.message.clone().unwrap_or_else(|| "falha no download".into())),
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

    let label_w = galley(&painter, &p.label, font.clone(), theme::TEXT).size().x;
    text_in(
        &painter,
        Rect::from_min_size(Pos2::new(left, head.min.y), Vec2::new(label_w, head.height())),
        Align2::LEFT_CENTER,
        &p.label,
        font.clone(),
        if error { theme::RED_TEXT } else { theme::TEXT },
    );

    // Trilho ×6 (até 140px, menor na barra estreita) e preenchimento (indeterminado: vai e volta).
    let track_w = ((limit - left) * 0.25).clamp(60.0, 140.0);
    let track = Rect::from_min_size(Pos2::new(left + label_w + 8.0, cy - 3.0), Vec2::new(track_w, 6.0));
    painter.rect_filled(track, 3.0, theme::PANEL_2);
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
        painter.rect_filled(r, 3.0, theme::ACCENT);
    }
    let info_rect = Rect::from_min_max(Pos2::new(track.max.x + 8.0, head.min.y), Pos2::new(limit, head.max.y));
    // Na barra estreita o detalhe termina em reticências em vez de cortar no meio.
    let color = if error { theme::RED_TEXT } else { theme::MUTED };
    let g = super::galley_trunc(&painter, &info, font, color, info_rect.width());
    super::place(&painter, info_rect, Align2::LEFT_CENTER, g, color);
}

fn draw_feed(deck: &mut Deck, ui: &mut Ui, rect: Rect) {
    // border-bottom: 1px solid var(--border)
    ui.painter().rect_filled(Rect::from_min_max(Pos2::new(rect.min.x, rect.max.y - 1.0), rect.max), 0.0, theme::BORDER);
    let area = Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.max.y - 1.0));
    let ctx = ui.ctx().clone();

    // Se a conversa estava no fim e a área mudou de altura (compositor cresceu), mantém no fim.
    let h_id = Id::new("feed-last-h");
    let last_h = ctx.data(|d| d.get_temp::<f32>(h_id)).unwrap_or(area.height());
    if (last_h - area.height()).abs() > 0.5 && deck.feed_near_bottom {
        deck.feed_scroll_to_bottom = true;
    }
    ctx.data_mut(|d| d.insert_temp(h_id, area.height()));

    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::top_down(Align::Min)));
    child.set_clip_rect(area);
    let scroll_to_bottom = std::mem::take(&mut deck.feed_scroll_to_bottom);
    let thinking_alpha = blink_alpha(ctx.input(|i| i.time), 1.0);
    let has_thinking = deck.feed.iter().any(|f| f.kind == FeedKind::Thinking);
    let out = egui::ScrollArea::vertical().id_salt("feed").auto_shrink([false, false]).animated(false).show(&mut child, |ui| {
        let full_w = ui.available_width();
        // padding: 4px 14px 8px
        let content_w = (full_w - 28.0).max(40.0);
        ui.add_space(4.0);
        if deck.feed.is_empty() {
            empty_text(ui, content_w);
        }
        let mut last = None;
        let n = deck.feed.len();
        for (i, item) in deck.feed.iter().enumerate() {
            // Margens verticais de 8px colapsam entre irmãos.
            ui.add_space(8.0);
            let r = message(ui, item, content_w, thinking_alpha);
            last = Some(r);
            if i + 1 == n {
                ui.add_space(8.0);
            }
        }
        ui.add_space(8.0);
        if scroll_to_bottom {
            if let Some(r) = last {
                ui.scroll_to_rect(r.expand2(Vec2::new(0.0, 16.0)), Some(Align::BOTTOM));
            }
        }
    });
    let dist = out.content_size.y - out.state.offset.y - out.inner_rect.height();
    deck.feed_near_bottom = dist < 40.0;
    if has_thinking {
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }
}

/// Opacidade da animação `blink` (1 → 0,2 → 1, `ease`) num período `period` s.
pub fn blink_alpha(time: f64, period: f64) -> f32 {
    let p = (time % period) / period;
    let half = if p < 0.5 { p * 2.0 } else { (1.0 - p) * 2.0 };
    // aproximação da curva `ease`
    let e = half * half * (3.0 - 2.0 * half);
    (1.0 - 0.8 * e) as f32
}

fn empty_text(ui: &mut Ui, w: f32) {
    let mut job = LayoutJob::default();
    job.wrap.max_width = w;
    let base =
        TextFormat { font_id: theme::ui(13.0), color: theme::MUTED, line_height: Some(13.0 * theme::LINE_HEIGHT), ..Default::default() };
    job.append("Aqui aparecem as conversas entre os agentes (via ", 0.0, base.clone());
    // Espaços que não quebram: o chip não se parte entre duas linhas (o fundo ficava solto).
    job.append("\u{a0}ask-agent\u{a0}", 0.0, TextFormat { font_id: theme::mono(12.0), background: theme::PANEL_2, ..base.clone() });
    job.append(") e as mensagens que você envia.", 0.0, base);
    let g = ui.painter().layout_job(job);
    let (r, _) = ui.allocate_exact_size(Vec2::new(w + 28.0, g.size().y + 16.0), Sense::hover());
    ui.painter().galley(Pos2::new(r.min.x + 14.0, r.min.y + 8.0), g, theme::MUTED);
}

/// `.msg` (article). Devolve o retângulo ocupado.
fn message(ui: &mut Ui, item: &FeedItem, w: f32, thinking_alpha: f32) -> Rect {
    let plain = matches!(item.kind, FeedKind::System | FeedKind::Thinking);
    let (pad_x, pad_y) = (10.0, if plain { 2.0 } else { 8.0 });
    let inner_w = (w - 3.0 - pad_x * 2.0).max(20.0);
    let painter = ui.painter().clone();

    // Cabeçalho: "quem → para" + hora à direita (12px, muted).
    let hfont = theme::ui(12.0);
    let time_g = galley(&painter, &item.time, hfont.clone(), theme::MUTED);
    let mut head = LayoutJob::default();
    head.wrap.max_width = (inner_w - time_g.size().x - 6.0).max(10.0);
    head.wrap.max_rows = 1;
    head.wrap.overflow_character = Some('…');
    let hfmt =
        TextFormat { font_id: hfont.clone(), color: theme::MUTED, line_height: Some(12.0 * theme::LINE_HEIGHT), ..Default::default() };
    if let Some(who) = &item.who {
        head.append(who, 0.0, TextFormat { font_id: theme::ui_semibold(12.0), color: item.color, ..hfmt.clone() });
    }
    if let Some(to) = &item.to {
        head.append(&format!(" → {to}"), if item.who.is_some() { 6.0 } else { 0.0 }, hfmt.clone());
    }
    let head_g = painter.layout_job(head);
    let head_h = 12.0 * theme::LINE_HEIGHT;

    // Corpo (pre-wrap, 13px, line-height 1.5).
    let body_color = match item.kind {
        FeedKind::Error => theme::RED_TEXT,
        FeedKind::System | FeedKind::Thinking => theme::MUTED,
        _ => theme::TEXT,
    };
    let mut body = LayoutJob::default();
    body.wrap.max_width = inner_w;
    let bfmt = TextFormat { font_id: theme::ui(13.0), color: body_color, line_height: Some(13.0 * 1.5), ..Default::default() };
    body.append(&item.text, 0.0, bfmt.clone());
    if item.kind == FeedKind::Thinking {
        body.append(" ●", 0.0, TextFormat { color: theme::with_opacity(body_color, thinking_alpha), ..bfmt });
    }
    let body_g = painter.layout_job(body);

    let h = pad_y * 2.0 + head_h + 4.0 + body_g.size().y.max(13.0 * 1.5);
    let (outer, _) = ui.allocate_exact_size(Vec2::new(w + 28.0, h), Sense::hover());
    let r = Rect::from_min_size(Pos2::new(outer.min.x + 14.0, outer.min.y), Vec2::new(w, h));

    if !plain {
        let border = match item.kind {
            FeedKind::Error => theme::RED,
            FeedKind::User => theme::USER,
            _ => item.color,
        };
        // Indicador colorido vertical na borda esquerda de 3px acompanhando o raio de 8px.
        painter.rect_filled(r, 8.0, border);
        let inner = Rect::from_min_max(Pos2::new(r.min.x + 3.0, r.min.y), r.max);
        painter.rect_filled(inner, egui::CornerRadius { nw: 5, sw: 5, ne: 8, se: 8 }, theme::PANEL_2);
    }
    let x = r.min.x + 3.0 + pad_x;
    let y = r.min.y + pad_y;
    let head_rect = Rect::from_min_size(Pos2::new(x, y), Vec2::new(inner_w, head_h));
    place(&painter, head_rect, Align2::LEFT_CENTER, head_g, theme::MUTED);
    place(&painter, head_rect, Align2::RIGHT_CENTER, time_g, theme::MUTED);
    painter.galley(Pos2::new(x, y + head_h + 4.0), body_g, body_color);
    let _: Color32 = body_color;
    r
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
    fn formata_bytes_em_ptbr() {
        assert_eq!(format_bytes(680_000_000.0), "648,5 MB");
        assert_eq!(format_bytes(1_624_555_275.0), "1,51 GB");
        assert_eq!(format_bytes(12.0), "1 KB");
    }

    #[test]
    fn textos_de_cada_estado() {
        let mut d = p(DownloadState::Downloading, 680_000_000, Some(1_624_555_275));
        d.speed = 12_900_000.0;
        let (f, t) = download_view(&d);
        assert_eq!(t, "41% · 648,5 MB / 1,51 GB · 12,3 MB/s");
        assert!((f.expect("fração") - 0.4186).abs() < 0.001);
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
    }
}
