//! Faixa de uso dos agentes na barra de título: logo + círculo (sessão) + anel (semana) + %.

use commanddeck_core::AgentUsage;
use egui::{Align2, Color32, Id, Pos2, Rect, Sense, Stroke, Ui};

use crate::deck::Deck;
use crate::icons::Icon;
use crate::theme;

/// Cor âmbar para o arco semanal entre 80% e 99%.
pub const AMBER: Color32 = Color32::from_rgb(0xf5, 0x9e, 0x0b);

/// Tamanho do logo do agente em pontos (14 px).
pub const LOGO_SIZE: f32 = 14.0;
/// Espaçamento entre o logo e o círculo de uso.
pub const LOGO_GAP: f32 = 6.0;
/// Raio do círculo interno (sessão de uso). Diâmetro ≈ 13,6 px.
pub const CIRCLE_R: f32 = 6.8;
/// Distância do anel externo em relação ao círculo (1,7 px fora).
pub const RING_OFFSET: f32 = 1.7;
/// Raio do anel externo (semana).
pub const RING_R: f32 = CIRCLE_R + RING_OFFSET;
/// Diâmetro da área do anel externo.
pub const RING_DIAMETER: f32 = 2.0 * RING_R;
/// Espessura do traço do anel externo.
pub const RING_STROKE: f32 = 1.5;
/// Espaçamento entre o anel e o texto de porcentagem.
pub const TEXT_GAP: f32 = 5.0;
/// Espaçamento entre itens de agentes diferentes.
pub const ITEM_GAP: f32 = 10.0;

/// Mapeia o identificador do modelo para o ícone correspondente.
pub fn agent_icon(model: &str) -> Option<Icon> {
    match model {
        "claude" => Some(Icon::ClaudeLogo),
        "antigravity" => Some(Icon::AntigravityLogo),
        "ollama" => Some(Icon::OllamaLogo),
        "codex" => Some(Icon::CodexLogo),
        _ => None,
    }
}

/// Define a cor do anel semanal com base na fração consumida.
pub fn ring_color(used: f32) -> Color32 {
    if used >= 1.0 {
        theme::RED
    } else if used >= 0.8 {
        AMBER
    } else {
        // Mais claro que o MUTED: o arco precisa se destacar do trilho em BORDER.
        theme::CHIP_TEXT
    }
}

/// Calcula os vértices do polígono do segmento circular que representa o nível de líquido
/// subindo de baixo para cima dentro do círculo de raio `r`.
pub fn liquid_polygon(center: Pos2, r: f32, used: f32) -> Vec<Pos2> {
    let used = used.clamp(0.0, 1.0);
    if used <= 0.0 {
        return Vec::new();
    }
    if used >= 1.0 {
        const STEPS: usize = 32;
        return (0..STEPS)
            .map(|i| {
                let th = i as f32 * std::f32::consts::TAU / STEPS as f32;
                Pos2::new(center.x + r * th.cos(), center.y + r * th.sin())
            })
            .collect();
    }
    // O nível sobe de y = center.y + r (used = 0) até y = center.y - r (used = 1).
    let dy = r * (1.0 - 2.0 * used);
    let th_right = (dy / r).clamp(-1.0, 1.0).asin();
    let th_left = std::f32::consts::PI - th_right;
    const STEPS: usize = 24;
    let mut poly = Vec::with_capacity(STEPS + 2);
    let dx = (r * r - dy * dy).max(0.0).sqrt();
    poly.push(Pos2::new(center.x + dx, center.y + dy));
    for i in 1..STEPS {
        let t = i as f32 / STEPS as f32;
        let th = th_right + t * (th_left - th_right);
        poly.push(Pos2::new(center.x + r * th.cos(), center.y + r * th.sin()));
    }
    poly.push(Pos2::new(center.x - dx, center.y + dy));
    poly
}

/// Gera pontos ao longo do arco do anel começando às 12h (topo, -pi/2) em sentido horário.
pub fn ring_arc_points(center: Pos2, r: f32, used: f32) -> Vec<Pos2> {
    let used = used.clamp(0.0, 1.0);
    if used <= 0.0 {
        return Vec::new();
    }
    let steps = ((used * 32.0).ceil() as usize).max(2);
    (0..=steps)
        .map(|i| {
            let frac = i as f32 / steps as f32 * used;
            let th = -std::f32::consts::FRAC_PI_2 + frac * std::f32::consts::TAU;
            Pos2::new(center.x + r * th.cos(), center.y + r * th.sin())
        })
        .collect()
}

/// Desenha o anel tracejado cinza para agentes sem dados de cota.
fn draw_dashed_ring(painter: &egui::Painter, center: Pos2, r: f32, color: Color32) {
    const DASHES: usize = 8;
    for i in 0..DASHES {
        let t0 = i as f32 / DASHES as f32;
        let t1 = (i as f32 + 0.6) / DASHES as f32;
        let th0 = -std::f32::consts::FRAC_PI_2 + t0 * std::f32::consts::TAU;
        let th1 = -std::f32::consts::FRAC_PI_2 + t1 * std::f32::consts::TAU;
        let th_mid = (th0 + th1) * 0.5;
        let pts = vec![
            Pos2::new(center.x + r * th0.cos(), center.y + r * th0.sin()),
            Pos2::new(center.x + r * th_mid.cos(), center.y + r * th_mid.sin()),
            Pos2::new(center.x + r * th1.cos(), center.y + r * th1.sin()),
        ];
        painter.add(egui::Shape::line(pts, Stroke::new(RING_STROKE, color)));
    }
}

/// Formata a duração até renovação da sessão (ex.: "2h10", "45 min").
pub fn format_duration(ms: u64) -> String {
    let secs = ms / 1000;
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    if hours > 0 {
        format!("{hours}h{mins:02}")
    } else {
        format!("{} min", mins.max(1))
    }
}

/// Formata o dia da semana e hora local da renovação semanal (ex.: "seg 14:00").
pub fn format_weekday_time(ms: u64) -> Option<String> {
    use chrono::{Datelike, TimeZone, Weekday};
    let dt = match chrono::Local.timestamp_millis_opt(ms as i64) {
        chrono::LocalResult::Single(t) | chrono::LocalResult::Ambiguous(t, _) => t,
        chrono::LocalResult::None => return None,
    };
    let dia = match dt.weekday() {
        Weekday::Mon => "seg",
        Weekday::Tue => "ter",
        Weekday::Wed => "qua",
        Weekday::Thu => "qui",
        Weekday::Fri => "sex",
        Weekday::Sat => "sáb",
        Weekday::Sun => "dom",
    };
    Some(format!("{dia} {}", dt.format("%H:%M")))
}

/// Formata o tempo decorrido desde a última leitura (ex.: "atualizado há 1 min").
pub fn format_updated(ms: u64) -> String {
    let secs = ms / 1000;
    if secs < 60 {
        "atualizado agora".to_owned()
    } else {
        let mins = secs / 60;
        if mins < 60 {
            format!("atualizado há {mins} min")
        } else {
            let hours = mins / 60;
            format!("atualizado há {hours} h")
        }
    }
}

/// Monta o texto descritivo do tooltip para o item de uso.
pub fn build_tooltip(name: &str, usage: Option<&AgentUsage>, now_ms: u64) -> String {
    let Some(usage) = usage else {
        return format!("{name} · sem dados de uso");
    };
    if usage.local {
        return format!("{name} · local, sem limite");
    }
    let mut parts = vec![name.to_owned()];
    if let Some(s) = usage.session {
        let pct = (s.used * 100.0).round() as i32;
        let mut s_str = format!("sessão {pct}%");
        if let Some(resets_ms) = s.resets_at_ms {
            let delta = resets_ms.saturating_sub(now_ms);
            s_str += &format!(" (renova em {})", format_duration(delta));
        }
        parts.push(s_str);
    }
    if let Some(w) = usage.weekly {
        let pct = (w.used * 100.0).round() as i32;
        let mut w_str = format!("semana {pct}%");
        if let Some(resets_ms) = w.resets_at_ms {
            if let Some(wt) = format_weekday_time(resets_ms) {
                w_str += &format!(" (renova {wt})");
            }
        }
        parts.push(w_str);
    }
    if usage.updated_ms > 0 {
        let elapsed = now_ms.saturating_sub(usage.updated_ms);
        parts.push(format_updated(elapsed));
    }
    parts.join(" · ")
}

/// Desenha um item por modelo aberto (`Deck::open_models`, dados de `Deck::usage`), alinhados
/// à direita até `rect.max.x` e centralizados na altura de `rect`. Devolve o x mais à esquerda
/// ocupado (`rect.max.x` se não desenhou nada), para a barra saber onde acaba a área de arrastar.
pub fn draw(deck: &mut Deck, ui: &Ui, rect: Rect) -> f32 {
    let models = deck.open_models();
    if models.is_empty() || rect.width() <= 0.0 {
        return rect.max.x;
    }

    let usage_map = deck.usage();
    let now_ms = commanddeck_core::util::now_ms();
    let painter = ui.painter();

    struct ItemData {
        model: String,
        name: String,
        agent_color: Color32,
        icon: Option<Icon>,
        tooltip: String,
        has_data: bool,
        is_local: bool,
        session_used: Option<f32>,
        weekly_used: Option<f32>,
        text_galley: Option<std::sync::Arc<egui::Galley>>,
        width: f32,
    }

    let mut items = Vec::with_capacity(models.len());
    for model in &models {
        let agent_color = deck.agents.tab_color(model);
        let name = deck.agents.name(model);
        let icon = agent_icon(model);
        let usage = usage_map.get(model);
        let tooltip = build_tooltip(&name, usage, now_ms);

        let is_local = usage.map(|u| u.local).unwrap_or(false);
        let has_data = usage.is_some_and(|u| u.local || u.session.is_some() || u.weekly.is_some());
        let session_used = usage.and_then(|u| u.session).map(|s| s.used);
        let weekly_used = usage.and_then(|u| u.weekly).map(|w| w.used);

        let (text_galley, text_w) = if let Some(s_used) = session_used {
            if !is_local {
                let pct_text = format!("{}%", (s_used * 100.0).round() as i32);
                let g = painter.layout_no_wrap(pct_text, theme::ui(11.5), theme::MUTED);
                let w = g.size().x;
                (Some(g), w)
            } else {
                (None, 0.0)
            }
        } else {
            (None, 0.0)
        };

        let width =
            if text_w > 0.0 { LOGO_SIZE + LOGO_GAP + RING_DIAMETER + TEXT_GAP + text_w } else { LOGO_SIZE + LOGO_GAP + RING_DIAMETER };

        items.push(ItemData {
            model: model.clone(),
            name,
            agent_color,
            icon,
            tooltip,
            has_data,
            is_local,
            session_used,
            weekly_used,
            text_galley,
            width,
        });
    }

    let total_width = items.iter().map(|it| it.width).sum::<f32>() + (items.len().saturating_sub(1) as f32) * ITEM_GAP;

    let start_x = rect.max.x - total_width;
    let cy = rect.center().y;

    let mut cur_x = start_x;
    for item in items {
        let item_rect = Rect::from_min_max(Pos2::new(cur_x, rect.min.y), Pos2::new(cur_x + item.width, rect.max.y));

        // 1. Logo do agente
        let logo_center = Pos2::new(cur_x + LOGO_SIZE / 2.0, cy);
        if let Some(icon) = item.icon {
            deck.icons.paint(painter, icon, logo_center, LOGO_SIZE, item.agent_color, 0.0);
        } else {
            let initial = item.name.chars().next().unwrap_or('?').to_uppercase().to_string();
            painter.text(logo_center, Align2::CENTER_CENTER, initial, theme::ui_semibold(11.5), item.agent_color);
        }

        // 2. Círculo interno + anel externo
        let circle_center = Pos2::new(cur_x + LOGO_SIZE + LOGO_GAP + RING_R, cy);

        // Fundo do círculo
        painter.circle_filled(circle_center, CIRCLE_R, theme::PANEL_2);

        if !item.has_data {
            // Sem dados: anel tracejado cinza e sem preenchimento
            draw_dashed_ring(painter, circle_center, RING_R, theme::MUTED);
            painter.circle_stroke(circle_center, CIRCLE_R, Stroke::new(1.0, theme::BORDER));
        } else if item.is_local {
            // Ollama / local: anel cinza contínuo, sem %
            painter.circle_stroke(circle_center, RING_R, Stroke::new(RING_STROKE, theme::MUTED));
            painter.circle_stroke(circle_center, CIRCLE_R, Stroke::new(1.0, theme::BORDER));
        } else {
            // Preenchimento de líquido (sessão)
            if let Some(s_used) = item.session_used {
                let poly = liquid_polygon(circle_center, CIRCLE_R, s_used);
                if poly.len() >= 3 {
                    painter.add(egui::Shape::convex_polygon(poly, item.agent_color, Stroke::NONE));
                }
            }
            painter.circle_stroke(circle_center, CIRCLE_R, Stroke::new(1.0, theme::BORDER));

            // Trilho discreto do anel externo
            painter.circle_stroke(circle_center, RING_R, Stroke::new(1.0, theme::BORDER));

            // Arco semanal (anel externo)
            if let Some(w_used) = item.weekly_used {
                if w_used >= 1.0 {
                    painter.circle_stroke(circle_center, RING_R, Stroke::new(RING_STROKE, theme::RED));
                } else if w_used > 0.0 {
                    let arc_pts = ring_arc_points(circle_center, RING_R, w_used);
                    if arc_pts.len() >= 2 {
                        let c = ring_color(w_used);
                        painter.add(egui::Shape::line(arc_pts, Stroke::new(RING_STROKE, c)));
                    }
                }
            }
        }

        // 3. Texto da porcentagem da sessão
        if let Some(ref galley) = item.text_galley {
            let text_x = cur_x + LOGO_SIZE + LOGO_GAP + RING_DIAMETER + TEXT_GAP;
            let text_pos = Pos2::new(text_x, cy - galley.size().y / 2.0);
            painter.galley(text_pos, galley.clone(), theme::MUTED);
        }

        // 4. Tooltip no hover
        let resp = ui.interact(item_rect, Id::new(("usage-item", item.model)), Sense::hover());
        super::tip(resp, item.tooltip);

        cur_x += item.width + ITEM_GAP;
    }

    start_x.max(rect.min.x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use commanddeck_core::usage::UsageWindow;

    #[test]
    fn liquid_polygon_geometry() {
        let center = Pos2::new(50.0, 50.0);
        let r = 8.0;

        // used <= 0: vazio
        assert!(liquid_polygon(center, r, 0.0).is_empty());
        assert!(liquid_polygon(center, r, -0.5).is_empty());

        // used >= 1: círculo completo
        let full = liquid_polygon(center, r, 1.0);
        assert_eq!(full.len(), 32);
        for pt in &full {
            let dist = (pt.x - center.x).hypot(pt.y - center.y);
            assert!((dist - r).abs() < 1e-4);
        }

        // used = 0.5: meio círculo inferior
        let half = liquid_polygon(center, r, 0.5);
        assert!(!half.is_empty());
        // Superfície em y = center.y
        let first = half.first().unwrap();
        let last = half.last().unwrap();
        assert!((first.y - center.y).abs() < 1e-3);
        assert!((last.y - center.y).abs() < 1e-3);
        // Todos os pontos da parte inferior têm y >= center.y
        for pt in &half {
            assert!(pt.y >= center.y - 1e-4);
            let dist = (pt.x - center.x).hypot(pt.y - center.y);
            assert!(dist <= r + 1e-3);
        }

        // used = 0.25: nível do líquido a 1/4 (dy = +0.5 * r)
        let q1 = liquid_polygon(center, r, 0.25);
        let surf_y = center.y + 0.5 * r;
        assert!((q1.first().unwrap().y - surf_y).abs() < 1e-3);
        assert!((q1.last().unwrap().y - surf_y).abs() < 1e-3);

        // used = 0.75: nível do líquido a 3/4 (dy = -0.5 * r)
        let q3 = liquid_polygon(center, r, 0.75);
        let surf_y3 = center.y - 0.5 * r;
        assert!((q3.first().unwrap().y - surf_y3).abs() < 1e-3);
        assert!((q3.last().unwrap().y - surf_y3).abs() < 1e-3);
    }

    #[test]
    fn ring_arc_geometry() {
        let center = Pos2::new(100.0, 100.0);
        let r = 10.0;

        assert!(ring_arc_points(center, r, 0.0).is_empty());

        // Começa sempre às 12h: (center.x, center.y - r)
        let arc = ring_arc_points(center, r, 0.5);
        let start = arc.first().unwrap();
        assert!((start.x - center.x).abs() < 1e-3);
        assert!((start.y - (center.y - r)).abs() < 1e-3);

        // Aos 50% termina às 6h: (center.x, center.y + r)
        let end = arc.last().unwrap();
        assert!((end.x - center.x).abs() < 1e-3);
        assert!((end.y - (center.y + r)).abs() < 1e-3);
    }

    #[test]
    fn ring_color_thresholds() {
        assert_eq!(ring_color(0.0), theme::CHIP_TEXT);
        assert_eq!(ring_color(0.5), theme::CHIP_TEXT);
        assert_eq!(ring_color(0.79), theme::CHIP_TEXT);
        assert_eq!(ring_color(0.80), AMBER);
        assert_eq!(ring_color(0.95), AMBER);
        assert_eq!(ring_color(0.999), AMBER);
        assert_eq!(ring_color(1.0), theme::RED);
        assert_eq!(ring_color(1.5), theme::RED);
    }

    #[test]
    fn time_formatting() {
        assert_eq!(format_duration(2 * 3600 * 1000 + 10 * 60 * 1000), "2h10");
        assert_eq!(format_duration(45 * 60 * 1000), "45 min");
        assert_eq!(format_duration(10 * 1000), "1 min");

        assert_eq!(format_updated(20 * 1000), "atualizado agora");
        assert_eq!(format_updated(60 * 1000), "atualizado há 1 min");
        assert_eq!(format_updated(5 * 60 * 1000), "atualizado há 5 min");
        assert_eq!(format_updated(3 * 3600 * 1000), "atualizado há 3 h");
    }

    #[test]
    fn tooltips() {
        let usage = AgentUsage {
            session: Some(UsageWindow { used: 0.34, resets_at_ms: Some(10_000_000 + 7800 * 1000) }),
            weekly: Some(UsageWindow { used: 0.12, resets_at_ms: None }),
            updated_ms: 10_000_000 - 65 * 1000,
            local: false,
        };
        let tip = build_tooltip("Claude", Some(&usage), 10_000_000);
        assert!(tip.starts_with("Claude · sessão 34% (renova em 2h10) · semana 12% · atualizado há 1 min"));

        let ollama = AgentUsage { local: true, ..Default::default() };
        assert_eq!(build_tooltip("Ollama", Some(&ollama), 10_000_000), "Ollama · local, sem limite");

        assert_eq!(build_tooltip("Codex", None, 10_000_000), "Codex · sem dados de uso");
    }
}
