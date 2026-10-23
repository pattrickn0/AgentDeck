//! Uso das assinaturas na barra de título: para cada agente com cota, o logo do modelo e duas barras
//! empilhadas (a sessão de 5 h, fina, em cima; a semana, grossa, embaixo) com as porcentagens. Passar o mouse
//! abre o painel de uso, com todos os agentes em detalhe (quanto foi usado e quando renova). Locais (Ollama)
//! não têm barra; aparecem só no rodapé do painel.

use std::collections::HashMap;

use agentdeck_core::usage::UsageWindow;
use agentdeck_core::AgentUsage;
use egui::{Align2, Color32, Id, Order, Painter, Pos2, Rect, Sense, Ui, Vec2};

use crate::deck::Deck;
use crate::theme;

/// Largura das barras e a espessura de cada uma (a sessão fina, a semana grossa).
const BAR_W: f32 = 40.0;
const SESSION_H: f32 = 2.0;
const WEEKLY_H: f32 = 4.0;
/// Linha de cada porcentagem (as duas empilhadas, 1 px entre elas).
const LINE_H: f32 = 10.0;
/// Coluna das porcentagens ("100%").
const PCT_W: f32 = 22.0;
/// Recuo, altura do item (o fundo do hover) e espaço entre os itens.
const ITEM_PAD: f32 = 6.0;
const ITEM_H: f32 = 26.0;
const ITEM_GAP: f32 = 8.0;
/// Largura do painel de uso.
const PANEL_W: f32 = 340.0;
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

fn local_time(ms: u64) -> Option<chrono::DateTime<chrono::Local>> {
    use chrono::TimeZone;
    match chrono::Local.timestamp_millis_opt(ms as i64) {
        chrono::LocalResult::Single(t) | chrono::LocalResult::Ambiguous(t, _) => Some(t),
        chrono::LocalResult::None => None,
    }
}

/// A hora local de um instante (ex.: "16:40").
pub fn format_clock(ms: u64) -> Option<String> {
    local_time(ms).map(|t| t.format("%H:%M").to_string())
}

/// Formata o dia da semana e hora local da renovação semanal (ex.: "seg 14:00").
pub fn format_weekday_time(ms: u64) -> Option<String> {
    use chrono::{Datelike, Weekday};
    let dt = local_time(ms)?;
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

/// Há quanto tempo o uso foi lido (ex.: "lido há 1 min").
pub fn format_read(ms: u64) -> String {
    let secs = ms / 1000;
    if secs < 60 {
        "lido agora".to_owned()
    } else if secs < 3600 {
        format!("lido há {} min", secs / 60)
    } else if secs < 48 * 3600 {
        format!("lido há {} h", secs / 3600)
    } else {
        format!("lido há {} dias", secs / 86_400)
    }
}

/// Janela que já renovou (`resets_at` no passado) sem leitura nova depois disso: a % lida é de
/// antes da renovação e não vale mais.
pub fn expired(w: &UsageWindow, updated_ms: u64, now_ms: u64) -> bool {
    w.resets_at_ms.is_some_and(|r| r <= now_ms && updated_ms < r)
}

/// O uso como vale agora: janelas vencidas zeradas (e sem data de renovação, que já passou).
/// Devolve também quais venceram (sessão, semana), para esmaecer e explicar no painel.
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

/// Tem cota para mostrar (não é local e a fonte informou ao menos uma janela).
pub fn has_quota(usage: &AgentUsage) -> bool {
    !usage.local && (usage.session.is_some() || usage.weekly.is_some())
}

/// A ordem dos agentes na barra: a do menu de agentes (`ollama:x` conta como `ollama`) e, depois, os que o menu
/// não tem, em ordem alfabética.
fn ordered_models(available: &[String], usage: &HashMap<String, AgentUsage>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for id in available {
        let key = crate::split::group_key(id);
        if usage.contains_key(key) && !out.iter().any(|k| k == key) {
            out.push(key.to_owned());
        }
    }
    let mut rest: Vec<String> = usage.keys().filter(|k| !out.contains(k)).cloned().collect();
    rest.sort();
    out.extend(rest);
    out
}

/// Uma janela como aparece agora: a fração usada (0..=1), quando renova e se renovou sem leitura nova
/// (esmaecida). `None` quando a fonte não informa essa janela.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bar {
    pub used: f32,
    pub resets_at_ms: Option<u64>,
    pub faded: bool,
}

/// A sessão e a semana de um agente, como valem agora.
pub fn bars(usage: &AgentUsage, now_ms: u64) -> [Option<Bar>; 2] {
    let (fresh, session_gone, weekly_gone) = current(usage, now_ms);
    let bar = |w: Option<UsageWindow>, faded| w.map(|w| Bar { used: w.used.clamp(0.0, 1.0), resets_at_ms: w.resets_at_ms, faded });
    [bar(fresh.session, session_gone), bar(fresh.weekly, weekly_gone)]
}

/// A linha de baixo de cada janela no painel: quando renova ("renova em 2h10 · às 16:40" para a sessão,
/// "renova seg 14:00" para a semana) ou por que não há número.
pub fn reset_text(bar: Option<Bar>, weekly: bool, now_ms: u64) -> String {
    let Some(bar) = bar else { return "a fonte não informa".to_owned() };
    if bar.faded {
        return "renovou · aguardando leitura nova".to_owned();
    }
    let Some(at) = bar.resets_at_ms.filter(|&r| r > now_ms) else { return String::new() };
    if weekly {
        format_weekday_time(at).map(|t| format!("renova {t}")).unwrap_or_default()
    } else {
        match format_clock(at) {
            Some(clock) => format!("renova em {} · às {clock}", format_duration(at - now_ms)),
            None => format!("renova em {}", format_duration(at - now_ms)),
        }
    }
}

/// A etiqueta de quem esgotou a sessão ("bloqueado até 15:10").
pub fn blocked_text(session: Option<Bar>, now_ms: u64) -> Option<String> {
    let s = session.filter(|s| s.used >= 1.0 && !s.faded)?;
    Some(match s.resets_at_ms.filter(|&r| r > now_ms).and_then(format_clock) {
        Some(clock) => format!("bloqueado até {clock}"),
        None => "bloqueado".to_owned(),
    })
}

/// O texto da porcentagem ("28%", ou "–" sem dado).
fn pct_text(bar: Option<Bar>) -> String {
    bar.map(|b| format!("{}%", (b.used * 100.0).round() as i32)).unwrap_or_else(|| "–".to_owned())
}

/// O logo do modelo (o ponto, sem logo) em `size` px.
fn paint_mark(deck: &mut Deck, p: &Painter, center: Pos2, kind: &str, color: Color32, size: f32) {
    match super::projects::agent_icon(kind) {
        Some(icon) => deck.icons.paint(p, icon, center, size, color, 0.0),
        None => super::dot(p, center, color),
    }
}

/// Uma barra (trilho e preenchimento) de `w`×`h` com o centro vertical em `cy`.
fn paint_bar(p: &Painter, x: f32, cy: f32, w: f32, h: f32, bar: Option<Bar>, agent: Color32) {
    let track = Rect::from_min_size(Pos2::new(x, cy - h / 2.0), Vec2::new(w, h));
    p.rect_filled(track, h / 2.0, theme::pal().separator);
    if let Some(b) = bar.filter(|b| b.used > 0.0) {
        let alpha = if b.faded { 0.5 } else { 1.0 };
        let fill = Rect::from_min_size(track.min, Vec2::new((w * b.used).max(h), h));
        p.rect_filled(fill, h / 2.0, theme::with_opacity(bar_color(b.used, agent), alpha));
    }
}

/// A cor de uma porcentagem: vermelha em 100%, esmaecida se a janela renovou sem leitura nova.
fn pct_color(bar: Option<Bar>, base: Color32) -> Color32 {
    match bar {
        Some(b) if b.used >= 1.0 && !b.faded => theme::pal().danger_text,
        Some(b) if b.faded => theme::with_opacity(base, 0.5),
        Some(_) => base,
        None => theme::pal().text_3,
    }
}

/// Um agente com cota, como a barra e o painel o desenham.
struct Row {
    model: String,
    name: String,
    color: Color32,
    bars: [Option<Bar>; 2],
    updated_ms: u64,
}

/// Desenha um item por agente com cota (dados de `Deck::usage`, com ou sem janela aberta), alinhados à direita até `rect.max.x` e
/// centralizados na altura de `rect`, e o painel de uso enquanto o mouse está num item ou no painel. Devolve o x
/// mais à esquerda ocupado (`rect.max.x` se não desenhou nada), para a barra saber onde acaba a área de arrastar.
pub fn draw(deck: &mut Deck, ui: &Ui, rect: Rect) -> f32 {
    let usage_map = deck.usage();
    let now_ms = agentdeck_core::util::now_ms();
    let mut rows = Vec::new();
    let mut locals = Vec::new();
    let available: Vec<String> = deck.agents.available.iter().map(|a| a.id.clone()).collect();
    for model in ordered_models(&available, &usage_map) {
        let Some(raw) = usage_map.get(&model) else { continue };
        if raw.local {
            locals.push(deck.agents.name(&model));
            continue;
        }
        if !has_quota(raw) {
            continue;
        }
        if let Some(at) = next_reset(raw, now_ms) {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(at - now_ms + 50));
        }
        rows.push(Row {
            name: deck.agents.name(&model),
            color: deck.agents.tab_color(&model),
            bars: bars(raw, now_ms),
            updated_ms: raw.updated_ms,
            model,
        });
    }
    if rows.is_empty() || rect.width() <= 0.0 {
        return rect.max.x;
    }

    let painter = ui.painter();
    let p = theme::pal();
    let item_w = ITEM_PAD + 12.0 + 5.0 + BAR_W + 4.0 + PCT_W + ITEM_PAD;
    let total = rows.len() as f32 * item_w + (rows.len() - 1) as f32 * ITEM_GAP;
    let start_x = rect.max.x - total;
    let cy = rect.center().y;
    let (row1, row2) = (cy - (LINE_H + 1.0) / 2.0, cy + (LINE_H + 1.0) / 2.0);
    let panel_key = Id::new("usage-panel");
    // O painel aberto no quadro anterior: (agente em destaque, retângulo do painel).
    let open_before = ui.ctx().data(|d| d.get_temp::<(String, Rect)>(panel_key));
    let mut hovered: Option<String> = None;
    let mut x = start_x;
    for row in &rows {
        let item = Rect::from_min_size(Pos2::new(x, cy - ITEM_H / 2.0), Vec2::new(item_w, ITEM_H));
        let resp = ui.interact(item, Id::new(("usage-item", &row.model)), Sense::hover());
        let highlighted = resp.hovered() || open_before.as_ref().is_some_and(|(m, _)| *m == row.model);
        if resp.hovered() {
            hovered = Some(row.model.clone());
        }
        if highlighted {
            painter.rect_filled(item, 5.0, p.hover);
        }
        let mut ix = item.min.x + ITEM_PAD;
        paint_mark(deck, painter, Pos2::new(ix + 6.0, cy), &row.model, row.color, 12.0);
        ix += 12.0 + 5.0;
        paint_bar(painter, ix, row1, BAR_W, SESSION_H, row.bars[0], row.color);
        paint_bar(painter, ix, row2, BAR_W, WEEKLY_H, row.bars[1], row.color);
        ix += BAR_W + 4.0;
        for (bar, y) in [(row.bars[0], row1), (row.bars[1], row2)] {
            let r = Rect::from_center_size(Pos2::new(ix + PCT_W / 2.0, y), Vec2::new(PCT_W, LINE_H));
            super::text_in(painter, r, Align2::RIGHT_CENTER, &pct_text(bar), theme::meta(8.0), pct_color(bar, p.text_soft));
        }
        x += item_w + ITEM_GAP;
    }
    let group = Rect::from_min_max(Pos2::new(start_x, cy - ITEM_H / 2.0), Pos2::new(rect.max.x, cy + ITEM_H / 2.0));

    // O painel fica aberto enquanto o mouse está num item, no painel ou no vão entre os dois.
    let pointer = ui.ctx().pointer_hover_pos();
    let keep = open_before.as_ref().is_some_and(|(_, panel)| pointer.is_some_and(|ptr| group.union(*panel).contains(ptr)));
    let focus = hovered.or_else(|| open_before.filter(|_| keep).map(|(m, _)| m));
    let focus = focus.or_else(|| deck.debug_usage_panel.then(|| rows[0].model.clone()));
    match focus {
        Some(model) => {
            let panel = draw_panel(deck, ui.ctx(), group, &rows, &locals, &model, now_ms);
            ui.ctx().data_mut(|d| d.insert_temp(panel_key, (model, panel)));
        }
        None => ui.ctx().data_mut(|d| d.remove::<(String, Rect)>(panel_key)),
    }
    start_x.max(rect.min.x)
}

/// Altura de um bloco de agente no painel.
const BLOCK_H: f32 = 10.0 + 18.0 + 9.0 + 42.0 + 9.0 + 42.0 + 12.0;

/// O painel de uso, embaixo do grupo de itens: um bloco por agente com cota (`focus` em destaque) e os locais
/// no rodapé. Devolve o retângulo dele.
fn draw_panel(deck: &mut Deck, ctx: &egui::Context, group: Rect, rows: &[Row], locals: &[String], focus: &str, now_ms: u64) -> Rect {
    let p = theme::pal();
    let footer_h = if locals.is_empty() { 0.0 } else { 6.0 + 1.0 + 8.0 + 16.0 };
    let h = 12.0 + super::SECTION_LABEL_H + 2.0 + rows.len() as f32 * (BLOCK_H + 2.0) + footer_h + 8.0;
    let size = Vec2::new(PANEL_W, h);
    let pos = super::menus::keep_on_screen(ctx, Pos2::new(group.max.x - PANEL_W, group.max.y + 4.0), size);
    let rect = Rect::from_min_size(pos, size);
    egui::Area::new(Id::new("usage-panel-area")).order(Order::Foreground).fixed_pos(pos).constrain(false).interactable(false).show(
        ctx,
        |ui| {
            let painter = ui.painter();
            super::shadow(painter, rect, 10, 8, 24, p.shadow);
            super::frame(painter, rect, 10.0, p.popup, p.glass_border);
            let head = Rect::from_min_size(Pos2::new(rect.min.x + 4.0, rect.min.y + 12.0), Vec2::new(PANEL_W - 4.0, super::SECTION_LABEL_H));
            super::section_label(painter, head, "USO DAS ASSINATURAS");
            if let Some(t) = local_time(now_ms) {
                let when = t.format("%d/%m · %H:%M").to_string();
                super::text_in(painter, head.shrink2(Vec2::new(14.0, 0.0)), Align2::RIGHT_CENTER, &when, theme::meta(10.0), p.text_3);
            }
            let mut y = head.max.y + 2.0;
            for row in rows {
                let block = Rect::from_min_size(Pos2::new(rect.min.x + 6.0, y), Vec2::new(PANEL_W - 12.0, BLOCK_H));
                agent_block(deck, painter, block, row, row.model == focus, now_ms);
                y += BLOCK_H + 2.0;
            }
            if !locals.is_empty() {
                y += 6.0;
                super::separator(painter, Pos2::new(rect.min.x + 14.0, y + 0.5), Pos2::new(rect.max.x - 14.0, y + 0.5));
                y += 1.0 + 8.0;
                let text = format!("{} · local, sem limite", locals.join(", "));
                let line = Rect::from_min_size(Pos2::new(rect.min.x + 14.0, y), Vec2::new(PANEL_W - 28.0, 16.0));
                let g = super::galley_trunc(painter, &text, theme::ui(12.0), p.text_3, line.width());
                super::place(painter, line, Align2::LEFT_CENTER, g, p.text_3);
            }
        },
    );
    rect
}

/// Um agente no painel: logo, nome, a etiqueta de bloqueado e "lido há"; a sessão e a semana, cada uma com a
/// porcentagem, uma barra de 6 px e quando renova.
fn agent_block(deck: &mut Deck, painter: &Painter, block: Rect, row: &Row, focus: bool, now_ms: u64) {
    let p = theme::pal();
    if focus {
        painter.rect_filled(block, 8.0, p.selected_soft);
    }
    let (x0, x1) = (block.min.x + 14.0, block.max.x - 14.0);
    let mut y = block.min.y + 10.0;
    // Cabeçalho do agente.
    let cy = y + 9.0;
    paint_mark(deck, painter, Pos2::new(x0 + 7.0, cy), &row.model, row.color, 14.0);
    let name_r = Rect::from_min_max(Pos2::new(x0 + 22.0, y), Pos2::new(x1, y + 18.0));
    let read = if row.updated_ms > 0 { format_read(now_ms.saturating_sub(row.updated_ms)) } else { String::new() };
    let read_w = super::text_in(painter, name_r, Align2::RIGHT_CENTER, &read, theme::meta(10.0), p.text_3).width();
    let name = super::galley_trunc(painter, &row.name, theme::ui_semibold(13.0), p.text_strong, (name_r.width() - read_w - 8.0).max(20.0));
    let name_end = super::place(painter, name_r, Align2::LEFT_CENTER, name, p.text_strong).max.x;
    if let Some(tag) = blocked_text(row.bars[0], now_ms) {
        let font = theme::ui_semibold(10.5);
        let w = super::text_w(painter, &tag, font.clone()) + 12.0;
        if name_end + 8.0 + w <= x1 - read_w - 8.0 {
            let chip = Rect::from_min_size(Pos2::new(name_end + 8.0, cy - 8.0), Vec2::new(w, 16.0));
            painter.rect_filled(chip, 4.0, theme::with_alpha(p.danger, 0.14));
            super::text_in(painter, chip, Align2::CENTER_CENTER, &tag, font, p.danger_text);
        }
    }
    y += 18.0 + 9.0;
    // Sessão e semana.
    for (i, (label, span)) in [("Sessão", "5 h"), ("Semana", "7 dias")].into_iter().enumerate() {
        let bar = row.bars[i];
        let line = Rect::from_min_max(Pos2::new(x0, y), Pos2::new(x1, y + 16.0));
        let l = super::text_in(painter, line, Align2::LEFT_CENTER, label, theme::ui(12.0), p.text_soft);
        let span_r = Rect::from_min_max(Pos2::new(l.max.x + 6.0, y), Pos2::new(x1, y + 16.0));
        super::text_in(painter, span_r, Align2::LEFT_CENTER, span, theme::meta(10.0), p.text_3);
        super::text_in(painter, line, Align2::RIGHT_CENTER, &pct_text(bar), theme::ui_semibold(13.0), pct_color(bar, p.text_strong));
        paint_bar(painter, x0, y + 16.0 + 4.0 + 3.0, x1 - x0, 6.0, bar, row.color);
        let note = Rect::from_min_max(Pos2::new(x0, y + 16.0 + 4.0 + 6.0 + 4.0), Pos2::new(x1, y + 42.0));
        super::text_in(painter, note, Align2::LEFT_CENTER, &reset_text(bar, i == 1, now_ms), theme::meta(10.0), p.text_3);
        y += 42.0 + 9.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(used: f32, resets: Option<u64>) -> Option<UsageWindow> {
        Some(UsageWindow { used, resets_at_ms: resets })
    }

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
    fn each_window_gets_its_own_bar() {
        let u = AgentUsage { session: w(0.28, Some(9_000)), weekly: w(1.7, None), updated_ms: 1, local: false };
        let [s, wk] = bars(&u, 100);
        assert_eq!(s, Some(Bar { used: 0.28, resets_at_ms: Some(9_000), faded: false }));
        // Valores fora da faixa ficam em 100%.
        assert_eq!(wk.map(|b| b.used), Some(1.0));
        // Janela que a fonte não informa.
        let only_session = AgentUsage { session: w(0.2, None), ..Default::default() };
        assert_eq!(bars(&only_session, 100)[1], None);
        assert_eq!(pct_text(None), "–");
        assert_eq!(pct_text(s), "28%");
        // Quem tem cota: não local e com ao menos uma janela.
        assert!(has_quota(&only_session));
        assert!(!has_quota(&AgentUsage { local: true, ..Default::default() }));
        assert!(!has_quota(&AgentUsage::default()));
    }

    #[test]
    fn a_window_past_its_reset_without_new_data_is_zeroed_and_faded() {
        let usage = AgentUsage { session: w(0.9, Some(1_000)), weekly: w(0.4, Some(9_000)), updated_ms: 500, local: false };
        // Antes da renovação: nada muda.
        assert_eq!(current(&usage, 900), (usage.clone(), false, false));
        // Depois: a sessão zera, perde a data e esmaece; a semana (que renova depois) fica.
        let [s, wk] = bars(&usage, 2_000);
        assert_eq!(s, Some(Bar { used: 0.0, resets_at_ms: None, faded: true }));
        assert_eq!(wk, Some(Bar { used: 0.4, resets_at_ms: Some(9_000), faded: false }));
        assert_eq!(reset_text(s, false, 2_000), "renovou · aguardando leitura nova");
        // Leitura feita depois da renovação: o dado é novo, vale como está.
        let fresh = AgentUsage { updated_ms: 1_500, ..usage.clone() };
        assert!(!current(&fresh, 2_000).1);
        assert_eq!(next_reset(&usage, 2_000), Some(9_000));
    }

    #[test]
    fn the_panel_says_when_each_window_renews_and_who_is_blocked() {
        let now = 10_000_000;
        let session = Some(Bar { used: 0.34, resets_at_ms: Some(now + 7_800_000), faded: false });
        assert!(reset_text(session, false, now).starts_with("renova em 2h10 · às "));
        let weekly = Some(Bar { used: 0.12, resets_at_ms: Some(now + 86_400_000), faded: false });
        assert!(reset_text(weekly, true, now).starts_with("renova "));
        assert_eq!(reset_text(None, true, now), "a fonte não informa");
        // Sem data de renovação: nada a dizer.
        assert_eq!(reset_text(Some(Bar { used: 0.3, resets_at_ms: None, faded: false }), false, now), "");
        // Sessão esgotada bloqueia (com a hora, se a fonte disser); esmaecida ou abaixo de 100%, não.
        let full = Some(Bar { used: 1.0, resets_at_ms: Some(now + 60_000), faded: false });
        assert!(blocked_text(full, now).is_some_and(|t| t.starts_with("bloqueado até ")));
        assert_eq!(blocked_text(Some(Bar { used: 1.0, resets_at_ms: None, faded: false }), now).as_deref(), Some("bloqueado"));
        assert_eq!(blocked_text(Some(Bar { used: 1.0, resets_at_ms: None, faded: true }), now), None);
        assert_eq!(blocked_text(session, now), None);
    }

    #[test]
    fn agents_follow_the_menu_order_and_the_unknown_come_last() {
        let usage: HashMap<String, AgentUsage> =
            ["zeta", "codex", "claude", "ollama"].into_iter().map(|k| (k.to_owned(), AgentUsage::default())).collect();
        let menu = ["claude", "antigravity", "ollama:qwen", "ollama:llama", "codex"].map(String::from);
        assert_eq!(ordered_models(&menu, &usage), ["claude", "ollama", "codex", "zeta"]);
    }

    #[test]
    fn time_formatting() {
        assert_eq!(format_duration(2 * 3600 * 1000 + 10 * 60 * 1000), "2h10");
        assert_eq!(format_duration(45 * 60 * 1000), "45 min");
        assert_eq!(format_duration(10 * 1000), "1 min");

        assert_eq!(format_read(20 * 1000), "lido agora");
        assert_eq!(format_read(60 * 1000), "lido há 1 min");
        assert_eq!(format_read(5 * 60 * 1000), "lido há 5 min");
        assert_eq!(format_read(3 * 3600 * 1000), "lido há 3 h");
        assert_eq!(format_read(3 * 86_400 * 1000), "lido há 3 dias");
    }
}
