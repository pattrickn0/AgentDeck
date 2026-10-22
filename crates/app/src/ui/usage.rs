//! Uso das assinaturas na barra de título: para cada agente com cota, o nome, uma barra de 44×4 px
//! na cor do agente e o percentual. Agentes sem cota (locais, OpenCode gratuito) não aparecem.

use agentdeck_core::usage::UsageWindow;
use agentdeck_core::AgentUsage;
use egui::{Color32, Id, Pos2, Rect, Sense, Ui, Vec2};

use crate::deck::Deck;
use crate::theme;

/// Largura e altura da barra de uso.
const BAR_W: f32 = 44.0;
const BAR_H: f32 = 4.0;
/// Espaço entre o nome, a barra e o percentual.
const PART_GAP: f32 = 6.0;
/// Espaço entre um agente e o seguinte.
const ITEM_GAP: f32 = 14.0;
/// Acima disto a barra avisa (`warning`); em 100% fica vermelha.
const WARN_AT: f32 = 0.8;

/// A cor do preenchimento: a do agente, até o aviso e o esgotamento.
pub fn bar_color(used: f32, agent: Color32) -> Color32 {
    if used >= 1.0 {
        theme::pal().danger
    } else if used >= WARN_AT {
        theme::pal().warning
    } else {
        agent
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

/// Janela que já renovou (`resets_at` no passado) sem leitura nova depois disso: a % lida é de
/// antes da renovação e não vale mais.
pub fn expired(w: &UsageWindow, updated_ms: u64, now_ms: u64) -> bool {
    w.resets_at_ms.is_some_and(|r| r <= now_ms && updated_ms < r)
}

/// O uso como vale agora: janelas vencidas zeradas (e sem data de renovação, que já passou).
/// Devolve também quais venceram (sessão, semana), para esmaecer e explicar na dica.
pub fn current(usage: &AgentUsage, now_ms: u64) -> (AgentUsage, bool, bool) {
    let mut u = usage.clone();
    let zero = |w: &mut Option<UsageWindow>| match w {
        Some(win) if expired(win, usage.updated_ms, now_ms) => {
            *win = UsageWindow { used: 0.0, resets_at_ms: None };
            true
        }
        _ => false,
    };
    let (s, wk) = (zero(&mut u.session), zero(&mut u.weekly));
    (u, s, wk)
}

/// Próxima renovação ainda por vir (para redesenhar quando ela passar).
fn next_reset(usage: &AgentUsage, now_ms: u64) -> Option<u64> {
    [usage.session, usage.weekly].into_iter().flatten().filter_map(|w| w.resets_at_ms).filter(|&r| r > now_ms).min()
}

/// Monta o texto descritivo do tooltip para o item de uso.
pub fn build_tooltip(name: &str, usage: Option<&AgentUsage>, now_ms: u64) -> String {
    let Some(usage) = usage else {
        return format!("{name} · sem dados de uso");
    };
    if usage.local {
        return format!("{name} · local, sem limite");
    }
    let (usage, session_gone, weekly_gone) = current(usage, now_ms);
    let mut parts = vec![name.to_owned()];
    if session_gone {
        parts.push("sessão renovada (sem dados novos)".to_owned());
    } else if let Some(s) = usage.session {
        let pct = (s.used * 100.0).round() as i32;
        let mut s_str = format!("sessão {pct}%");
        if let Some(resets_ms) = s.resets_at_ms {
            let delta = resets_ms.saturating_sub(now_ms);
            s_str += &format!(" (renova em {})", format_duration(delta));
        }
        parts.push(s_str);
    }
    if weekly_gone {
        parts.push("semana renovada (sem dados novos)".to_owned());
    } else if let Some(w) = usage.weekly {
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

/// O que a barra de título mostra de um agente: a semana quando há, senão a sessão (fração 0..=1).
/// `None` para quem não tem cota (local, sem leitura).
pub fn shown_usage(usage: &AgentUsage) -> Option<f32> {
    if usage.local {
        return None;
    }
    usage.weekly.or(usage.session).map(|w| w.used.clamp(0.0, 1.0))
}

/// Desenha um item por modelo aberto que tenha cota (`Deck::open_models`, dados de `Deck::usage`),
/// alinhados à direita até `rect.max.x` e centralizados na altura de `rect`. Devolve o x mais à
/// esquerda ocupado (`rect.max.x` se não desenhou nada), para a barra saber onde acaba a área de arrastar.
pub fn draw(deck: &mut Deck, ui: &Ui, rect: Rect) -> f32 {
    let models = deck.open_models();
    if models.is_empty() || rect.width() <= 0.0 {
        return rect.max.x;
    }

    let usage_map = deck.usage();
    let now_ms = agentdeck_core::util::now_ms();
    let painter = ui.painter();
    let p = theme::pal();

    struct Item {
        model: String,
        color: Color32,
        name: std::sync::Arc<egui::Galley>,
        pct: std::sync::Arc<egui::Galley>,
        used: f32,
        tooltip: String,
        width: f32,
    }

    let mut items = Vec::with_capacity(models.len());
    for model in &models {
        let Some(raw) = usage_map.get(model) else { continue };
        if let Some(at) = next_reset(raw, now_ms) {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(at - now_ms + 50));
        }
        let (fresh, session_gone, weekly_gone) = current(raw, now_ms);
        let Some(used) = shown_usage(&fresh) else { continue };
        let gone = if fresh.weekly.is_some() { weekly_gone } else { session_gone };
        let agent_name = deck.agents.name(model);
        // Janela renovada sem leitura nova: 0% esmaecido até chegar dado novo.
        let alpha = if gone { 0.5 } else { 1.0 };
        let name = super::galley_spaced(painter, &agent_name.to_uppercase(), theme::label(10.0), theme::with_opacity(p.text_3, alpha), 0.6);
        let pct_color = if used >= 1.0 { p.danger_text } else { theme::with_opacity(p.text_soft, alpha) };
        let pct = painter.layout_no_wrap(format!("{}%", (used * 100.0).round() as i32), theme::meta(10.0), pct_color);
        let width = name.size().x + PART_GAP + BAR_W + PART_GAP + pct.size().x;
        items.push(Item {
            model: model.clone(),
            color: theme::with_opacity(bar_color(used, deck.agents.tab_color(model)), alpha),
            name,
            pct,
            used,
            tooltip: build_tooltip(&agent_name, Some(raw), now_ms),
            width,
        });
    }
    if items.is_empty() {
        return rect.max.x;
    }

    let total = items.iter().map(|it| it.width).sum::<f32>() + (items.len() - 1) as f32 * ITEM_GAP;
    let start_x = rect.max.x - total;
    let cy = rect.center().y;
    let mut x = start_x;
    for item in items {
        let item_rect = Rect::from_min_max(Pos2::new(x, rect.min.y), Pos2::new(x + item.width, rect.max.y));
        let text_h = item.name.size().y;
        painter.galley(Pos2::new(x, cy - text_h / 2.0), item.name.clone(), p.text_3);
        let bar = Rect::from_min_size(Pos2::new(x + item.name.size().x + PART_GAP, cy - BAR_H / 2.0), Vec2::new(BAR_W, BAR_H));
        painter.rect_filled(bar, 2.0, p.separator);
        if item.used > 0.0 {
            painter.rect_filled(Rect::from_min_size(bar.min, Vec2::new((BAR_W * item.used).max(BAR_H), BAR_H)), 2.0, item.color);
        }
        painter.galley(Pos2::new(bar.max.x + PART_GAP, cy - item.pct.size().y / 2.0), item.pct.clone(), p.text_soft);
        // Dica ao passar o mouse.
        let resp = ui.interact(item_rect, Id::new(("usage-item", item.model)), Sense::hover());
        super::tip(resp, item.tooltip);
        x += item.width + ITEM_GAP;
    }
    start_x.max(rect.min.x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bar_keeps_the_agent_color_until_the_quota_runs_low() {
        let agent = theme::CLAUDE;
        assert_eq!(bar_color(0.0, agent), agent);
        assert_eq!(bar_color(0.79, agent), agent);
        assert_eq!(bar_color(0.80, agent), theme::pal().warning);
        assert_eq!(bar_color(0.999, agent), theme::pal().warning);
        assert_eq!(bar_color(1.0, agent), theme::pal().danger);
        assert_eq!(bar_color(1.5, agent), theme::pal().danger);
    }

    #[test]
    fn only_agents_with_a_quota_show_a_bar() {
        let w = |used| Some(UsageWindow { used, resets_at_ms: None });
        // A semana vale mais que a sessão.
        let both = AgentUsage { session: w(0.9), weekly: w(0.3), updated_ms: 1, local: false };
        assert_eq!(shown_usage(&both), Some(0.3));
        let session_only = AgentUsage { session: w(0.9), weekly: None, updated_ms: 1, local: false };
        assert_eq!(shown_usage(&session_only), Some(0.9));
        // Local (Ollama), sem leitura e valores fora da faixa.
        assert_eq!(shown_usage(&AgentUsage { local: true, ..Default::default() }), None);
        assert_eq!(shown_usage(&AgentUsage::default()), None);
        assert_eq!(shown_usage(&AgentUsage { weekly: w(1.7), ..Default::default() }), Some(1.0));
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

    #[test]
    fn a_window_past_its_reset_without_new_data_is_zeroed() {
        let usage = AgentUsage {
            session: Some(UsageWindow { used: 0.9, resets_at_ms: Some(1_000) }),
            weekly: Some(UsageWindow { used: 0.4, resets_at_ms: Some(9_000) }),
            updated_ms: 500,
            local: false,
        };
        // Antes da renovação: nada muda.
        assert_eq!(current(&usage, 900), (usage.clone(), false, false));
        // Depois: a sessão zera e perde a data; a semana (que renova depois) fica.
        let (now, s, w) = current(&usage, 2_000);
        assert!(s && !w);
        assert_eq!(now.session, Some(UsageWindow { used: 0.0, resets_at_ms: None }));
        assert_eq!(now.weekly, usage.weekly);
        assert!(build_tooltip("Claude", Some(&usage), 2_000).contains("sessão renovada (sem dados novos) · semana 40%"));
        // Leitura feita depois da renovação: o dado é novo, vale como está.
        let fresh = AgentUsage { updated_ms: 1_500, ..usage.clone() };
        assert!(!current(&fresh, 2_000).1);
        assert_eq!(next_reset(&usage, 2_000), Some(9_000));
    }
}
