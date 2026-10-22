//! Seção PROJETOS da barra lateral: lista de projetos, agentes de cada um (em árvore: quem abriu
//! quem) e ações rápidas.

use std::path::{Path, PathBuf};

use agentdeck_core::models::short_model;

use egui::{Align2, Color32, CursorIcon, Id, Order, Painter, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use super::{clickable, dot, frame, galley, galley_trunc, menus, place, shadow, text_in, text_w, tip};
use crate::deck::{AgentBadge, Deck};
use crate::icons::Icon;
use crate::theme;

pub const ROW_H: f32 = 22.0;
pub const SECTION_H: f32 = 24.0;
const LOGO_SIZE: f32 = 12.0;
const LOGO_GAP: f32 = 4.0;
/// Centro do logo de um agente da raiz (a partir da borda esquerda da linha).
const AGENT_X: f32 = 28.0;
/// Recuo de cada nível da árvore de agentes.
const INDENT: f32 = 14.0;
/// Níveis além deste não recuam mais (a barra é estreita).
const MAX_DEPTH: usize = 5;

/// Onde a linha "+ Novo agente" foi pintada no último quadro (âncora do menu de agentes; `None` se invisível).
fn anchor_key() -> Id {
    Id::new("new-agent-anchor")
}

/// Âncora do menu de novo agente: a linha "+ Novo agente" da barra lateral ou, se ela não está à vista
/// (barra recolhida, lista rolada), `fallback`.
pub fn new_agent_anchor(ctx: &egui::Context, fallback: Rect) -> Rect {
    ctx.data(|d| d.get_temp::<Option<Rect>>(anchor_key())).flatten().unwrap_or(fallback)
}

/// Mapeia o identificador do modelo para o ícone correspondente.
pub(super) fn agent_icon(kind: &str) -> Option<Icon> {
    let model = kind.split(':').next().unwrap_or(kind);
    match model {
        "claude" => Some(Icon::ClaudeLogo),
        "antigravity" => Some(Icon::AntigravityLogo),
        "ollama" => Some(Icon::OllamaLogo),
        "codex" => Some(Icon::CodexLogo),
        _ => None,
    }
}

/// Abre uma pasta no explorador de arquivos do sistema operacional.
fn open_project_folder(deck: &Deck, path: &Path) {
    if deck.workspace == path && deck.core.open_path(".").is_ok() {
        return;
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(path).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(path).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    }
}

/// Título de seção da barra lateral (MAIÚSCULAS, 11 px, MUTED).
fn section_title(painter: &egui::Painter, head: Rect, title: &str) {
    let g = galley_trunc(painter, title, theme::ui_semibold(11.0), theme::pal().text_2, head.width() - 60.0);
    place(
        painter,
        Rect::from_min_max(Pos2::new(head.min.x + 12.0, head.min.y), Pos2::new(head.max.x - 52.0, head.max.y)),
        Align2::LEFT_CENTER,
        g,
        theme::pal().text_2,
    );
}

// ---------------------------------------------------------------------------
// Árvore de agentes
// ---------------------------------------------------------------------------

/// Uma linha da árvore de agentes de um projeto.
#[derive(Debug, Clone, PartialEq)]
pub struct TreeRow {
    /// Índice em `ProjectView::agents`.
    pub idx: usize,
    pub depth: usize,
    /// Para cada ancestral de profundidade 1 até `depth - 1`: se ele ainda tem irmãos abaixo (a
    /// linha-guia daquele nível continua por esta linha).
    pub rails: Vec<bool>,
    /// É o último filho do pai (o "cotovelo" para no meio da linha).
    pub last: bool,
    /// Tem filhos logo abaixo (a linha-guia desce do logo).
    pub has_kids: bool,
}

/// Ordena os agentes em árvore: pai antes dos filhos, irmãos na ordem das abas. Quem não tem pai
/// ou tem um pai que não está na lista fica na raiz; num ciclo, o primeiro da lista vira raiz.
pub fn agent_tree(agents: &[AgentBadge]) -> Vec<TreeRow> {
    let n = agents.len();
    let parent: Vec<Option<usize>> =
        (0..n).map(|i| agents[i].parent.as_deref().and_then(|p| agents.iter().position(|a| a.id == p)).filter(|&p| p != i)).collect();
    let mut kids = vec![Vec::new(); n];
    for (i, p) in parent.iter().enumerate() {
        if let Some(p) = p {
            kids[*p].push(i);
        }
    }
    let mut out = Vec::with_capacity(n);
    let mut seen = vec![false; n];
    for i in (0..n).filter(|&i| parent[i].is_none()) {
        walk(i, 0, Vec::new(), true, &kids, &mut seen, &mut out);
    }
    for i in 0..n {
        walk(i, 0, Vec::new(), true, &kids, &mut seen, &mut out);
    }
    out
}

fn walk(i: usize, depth: usize, rails: Vec<bool>, last: bool, kids: &[Vec<usize>], seen: &mut [bool], out: &mut Vec<TreeRow>) {
    if seen[i] {
        return;
    }
    seen[i] = true;
    let mine: Vec<usize> = kids[i].iter().copied().filter(|&k| !seen[k]).collect();
    out.push(TreeRow { idx: i, depth, rails: rails.clone(), last, has_kids: !mine.is_empty() });
    let mut next = rails;
    if depth >= 1 {
        next.push(!last);
    }
    for (n, &k) in mine.iter().enumerate() {
        walk(k, depth + 1, next.clone(), n + 1 == mine.len(), kids, seen, out);
    }
}

/// X do logo de um agente na profundidade `depth`.
fn agent_x(row: Rect, depth: usize) -> f32 {
    row.min.x + AGENT_X + depth.min(MAX_DEPTH) as f32 * INDENT
}

/// Linhas-guia da árvore (como no explorador de arquivos). `gap` é o espaço entre as linhas.
fn paint_guides(p: &Painter, row: Rect, gap: f32, node: &TreeRow) {
    let stroke = Stroke::new(1.0, theme::pal().separator);
    let y = row.center().y;
    if node.has_kids {
        p.vline(agent_x(row, node.depth), (y + LOGO_SIZE / 2.0 + 2.0)..=row.max.y, stroke);
    }
    if node.depth == 0 {
        return;
    }
    for (k, &on) in node.rails.iter().enumerate() {
        if on {
            p.vline(agent_x(row, k), (row.min.y - gap)..=row.max.y, stroke);
        }
    }
    let x = agent_x(row, node.depth - 1);
    let bottom = if node.last { y } else { row.max.y };
    p.vline(x, (row.min.y - gap)..=bottom, stroke);
    p.hline(x..=(agent_x(row, node.depth) - LOGO_SIZE / 2.0 - 3.0), y, stroke);
}

/// Logo do modelo do agente (ponto, se não há logo), com o estado: halo pulsando trabalhando,
/// anel com turno concluído não visto, apagado se o processo encerrou.
fn paint_agent_mark(deck: &mut Deck, ctx: &egui::Context, p: &Painter, center: Pos2, ag: &AgentBadge, is_active_tab: bool) {
    if ag.busy {
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
        let t = ctx.input(|inp| inp.time);
        let wave = (t * 4.8).sin() as f32 * 0.5 + 0.5;
        p.circle_filled(center, 7.0 + 2.8 * wave, theme::with_opacity(ag.color, 0.30 * (1.0 - wave)));
    } else if ag.done && !is_active_tab {
        p.circle_stroke(center, 8.5, Stroke::new(1.2, theme::brighten(ag.color, 1.25)));
    }
    let color = if ag.alive { ag.color } else { theme::with_opacity(ag.color, 0.4) };
    match agent_icon(&ag.kind) {
        Some(icon) => deck.icons.paint(p, icon, center, LOGO_SIZE, color, 0.0),
        None => dot(p, center, color),
    }
}

/// Seta de expandir/recolher (▸ fechada, ▾ aberta).
fn paint_chevron(p: &Painter, center: Pos2, open: bool, color: Color32) {
    let s = Stroke::new(1.3, color);
    let (a, b, c) = if open {
        (center + Vec2::new(-3.5, -1.5), center + Vec2::new(0.0, 2.0), center + Vec2::new(3.5, -1.5))
    } else {
        (center + Vec2::new(-1.5, -3.5), center + Vec2::new(2.0, 0.0), center + Vec2::new(-1.5, 3.5))
    };
    p.line_segment([a, b], s);
    p.line_segment([b, c], s);
}

/// Duas setas empilhadas: para cima = recolher todos, para baixo = expandir todos.
fn paint_double_chevron(p: &Painter, center: Pos2, up: bool, color: Color32) {
    let s = Stroke::new(1.3, color);
    let dir = if up { -1.0 } else { 1.0 };
    for dy in [-2.5, 2.5] {
        let c = center + Vec2::new(0.0, dy);
        p.line_segment([c + Vec2::new(-3.5, -1.75 * dir), c + Vec2::new(0.0, 1.75 * dir)], s);
        p.line_segment([c + Vec2::new(0.0, 1.75 * dir), c + Vec2::new(3.5, -1.75 * dir)], s);
    }
}

/// `target` é `dragged` ou está na equipe dele (soltar ali criaria um ciclo).
fn in_subtree(agents: &[AgentBadge], target: &str, dragged: &str) -> bool {
    let mut cur = Some(target);
    for _ in 0..agents.len() + 2 {
        match cur {
            Some(id) if id == dragged => return true,
            Some(id) => cur = agents.iter().find(|a| a.id == id).and_then(|a| a.parent.as_deref()),
            None => return false,
        }
    }
    true
}

/// Agente sendo arrastado na lista: (projeto, id, nome).
type AgentDrag = Option<(String, String, String)>;

fn drag_key() -> Id {
    Id::new("agent-drag")
}

/// Onde o agente arrastado cai: sob outro agente (`Some(id)`) ou na raiz (`None`); `ok` = sem ciclo.
struct DropTarget {
    boss: Option<String>,
    ok: bool,
}

/// Contorno do alvo de soltar (cor do projeto se vale, vermelho se criaria um ciclo).
fn paint_drop_target(p: &Painter, r: Rect, ok: bool, color: Color32) {
    let c = if ok { color } else { theme::pal().danger };
    p.rect_filled(r, 4.0, theme::with_opacity(c, 0.12));
    p.rect_stroke(r.shrink(0.5), 4.0, Stroke::new(1.2, c), egui::StrokeKind::Inside);
}

/// Clique num agente da lista.
struct AgentClick {
    project: String,
    id: String,
    tab: Option<usize>,
}

/// Desenha a seção PROJETOS e avança `top_y`.
pub fn draw(deck: &mut Deck, ui: &mut Ui, top_y: &mut f32, rect: Rect, dragging: bool) {
    let ctx = ui.ctx().clone();

    if deck.sidebar_collapsed {
        ctx.data_mut(|d| {
            d.insert_temp(anchor_key(), None::<Rect>);
            d.insert_temp(drag_key(), None::<(String, String, String)>);
            d.insert_temp(Id::new("proj-ctx-menu"), None::<(String, PathBuf, String, Pos2)>);
            d.insert_temp(Id::new("proj-rename-modal"), None::<(String, String)>);
        });
        return;
    }

    let painter = ui.painter_at(rect);
    let projects = deck.project_views();

    // ---------- Cabeçalho da seção PROJETOS ----------
    let head_rect = Rect::from_min_size(Pos2::new(rect.min.x, *top_y + 4.0), Vec2::new(rect.width(), SECTION_H));
    *top_y = head_rect.max.y;
    section_title(&painter, head_rect, "PROJETOS");

    // Botão "+" para novo projeto
    let plus_r = Rect::from_min_size(Pos2::new(head_rect.max.x - 24.0, head_rect.min.y + 2.0), Vec2::new(20.0, 20.0));
    let plus_resp = clickable(ui, plus_r, Id::new("proj-plus-btn"));
    let plus_hov = plus_resp.hovered() && !dragging;
    if plus_hov {
        painter.rect_filled(plus_r, 4.0, theme::pal().hover);
        ctx.set_cursor_icon(CursorIcon::PointingHand);
    }
    let plus_color = if plus_hov { theme::pal().text } else { theme::pal().text_2 };
    text_in(&painter, plus_r, Align2::CENTER_CENTER, "+", theme::ui_semibold(14.0), plus_color);
    if tip(plus_resp, "Novo projeto…").clicked() {
        deck.new_project_dialog();
    }

    // Expandir/recolher a lista de agentes de todos os projetos
    let any_open = projects.iter().any(|p| p.expanded);
    let all_r = plus_r.translate(Vec2::new(-22.0, 0.0));
    let all_resp = clickable(ui, all_r, Id::new("proj-all-btn"));
    let all_hov = all_resp.hovered() && !dragging;
    if all_hov {
        painter.rect_filled(all_r, 4.0, theme::pal().hover);
        ctx.set_cursor_icon(CursorIcon::PointingHand);
    }
    paint_double_chevron(&painter, all_r.center(), any_open, if all_hov { theme::pal().text } else { theme::pal().text_2 });
    let all_tip = if any_open { "Recolher os agentes de todos os projetos" } else { "Mostrar os agentes de todos os projetos" };
    if tip(all_resp, all_tip).clicked() {
        deck.set_all_projects_expanded(!any_open);
    }

    // ---------- Lista de projetos e agentes ----------
    // Só a altura do conteúdo, até 40 % da barra (aí a lista rola).
    let rows = projects.len() + projects.iter().filter(|p| p.expanded).map(|p| p.agents.len() + usize::from(p.active)).sum::<usize>();
    let gap = ui.spacing().item_spacing.y;
    let content_h = rows as f32 * (ROW_H + gap);
    let max_list_h = content_h.min((rect.height() * 0.40).round());
    let list_top = *top_y + 2.0;
    let list_rect = Rect::from_min_max(Pos2::new(rect.min.x, list_top), Pos2::new(rect.max.x, list_top + max_list_h));

    let mut ui_list = ui.new_child(egui::UiBuilder::new().max_rect(list_rect).layout(egui::Layout::top_down(egui::Align::Min)));
    ui_list.set_clip_rect(list_rect);

    let mut switch_to_id = None;
    let mut toggle_id = None;
    let mut agent_click: Option<AgentClick> = None;
    let mut open_ctx = None;
    let mut new_agent_click = false;
    let mut new_agent_anchor: Option<Rect> = None;
    let active_color = deck.accent();
    let deck_active = deck.active;
    let mut agent_drag: AgentDrag = ctx.data(|d| d.get_temp::<AgentDrag>(drag_key())).flatten();
    let pointer = ctx.input(|i| i.pointer.interact_pos());
    let mut drop_target: Option<DropTarget> = None;

    let scroll_output = egui::ScrollArea::vertical().id_salt("proj-list-scroll").max_height(max_list_h).auto_shrink([false, true]).show(
        &mut ui_list,
        |ui| {
            let row_w = ui.available_width();
            let p = ui.painter().clone();

            for proj in &projects {
                let (row_r, resp) = ui.allocate_exact_size(Vec2::new(row_w, ROW_H), Sense::click());
                // A seta fica por cima da linha (criada depois, ganha o clique).
                let can_expand = proj.active || !proj.agents.is_empty();
                let chev_r = Rect::from_min_size(Pos2::new(row_r.min.x + 2.0, row_r.min.y), Vec2::new(16.0, ROW_H));
                let chev_resp = can_expand.then(|| ui.interact(chev_r, Id::new(("proj-chevron", &proj.id)), Sense::click()));
                let chev_hov = chev_resp.as_ref().is_some_and(|r| r.hovered()) && !dragging;
                let hovered = (resp.hovered() || chev_hov) && !dragging;

                // Fundo: ativo = PANEL_2, hover = PANEL_2
                if proj.active {
                    p.rect_filled(row_r, 0.0, theme::pal().hover);
                } else if hovered {
                    p.rect_filled(row_r, 4.0, theme::pal().hover);
                }

                if hovered {
                    ctx.set_cursor_icon(CursorIcon::PointingHand);
                }

                // Indicador de projeto ativo: borda esquerda
                if proj.active {
                    let ind_bar = Rect::from_min_max(Pos2::new(row_r.min.x, row_r.min.y), Pos2::new(row_r.min.x + 2.5, row_r.max.y));
                    p.rect_filled(ind_bar, 0.0, active_color);
                }
                // Soltar sobre o próprio projeto devolve o agente à raiz (responde ao usuário).
                // Só o que está à vista na lista conta (as linhas roladas para fora ficam por baixo de outras seções).
                let clip = ui.clip_rect();
                let visible = |r: Rect| pointer.is_some_and(|ptr| r.contains(ptr) && clip.contains(ptr));
                if agent_drag.as_ref().is_some_and(|(dp, ..)| *dp == proj.id) && visible(row_r) {
                    paint_drop_target(&p, row_r, true, active_color);
                    drop_target = Some(DropTarget { boss: None, ok: true });
                }
                if can_expand {
                    let c = if chev_hov {
                        theme::pal().text
                    } else if proj.active {
                        active_color
                    } else {
                        theme::pal().text_2
                    };
                    paint_chevron(&p, Pos2::new(row_r.min.x + 10.0, row_r.center().y), proj.expanded, c);
                }

                // Lado direito: contagem e logos dos agentes
                let count = proj.agents.len();
                let count_font = theme::ui(11.0);
                let count_str = if count > 0 { count.to_string() } else { String::new() };
                let count_w = if count > 0 { text_w(&p, &count_str, count_font.clone()) } else { 0.0 };
                let count_right_x = row_r.max.x - 10.0;

                if count > 0 {
                    text_in(
                        &p,
                        Rect::from_min_max(Pos2::new(count_right_x - count_w, row_r.min.y), Pos2::new(count_right_x, row_r.max.y)),
                        Align2::RIGHT_CENTER,
                        &count_str,
                        count_font,
                        theme::pal().text_2,
                    );
                }

                // Logos dos modelos quando a lista está recolhida
                let mut unique_models: Vec<(&str, Color32)> = Vec::new();
                if !proj.expanded {
                    for ag in &proj.agents {
                        let model = ag.kind.split(':').next().unwrap_or(&ag.kind);
                        if !unique_models.iter().any(|(m, _)| *m == model) {
                            unique_models.push((model, ag.color));
                        }
                    }
                }

                let logos_total_w = if !unique_models.is_empty() {
                    unique_models.len() as f32 * LOGO_SIZE + (unique_models.len().saturating_sub(1) as f32) * LOGO_GAP
                } else {
                    0.0
                };

                let logos_end_x = if count > 0 { count_right_x - count_w - 6.0 } else { count_right_x };
                let logos_start_x = logos_end_x - logos_total_w;

                if !unique_models.is_empty() {
                    let mut cur_logo_x = logos_start_x;
                    for (model, color) in &unique_models {
                        let center = Pos2::new(cur_logo_x + LOGO_SIZE / 2.0, row_r.center().y);
                        if let Some(icon) = agent_icon(model) {
                            deck.icons.paint(&p, icon, center, LOGO_SIZE, *color, 0.0);
                        } else {
                            let ch = model.chars().next().unwrap_or('?').to_uppercase().to_string();
                            p.text(center, Align2::CENTER_CENTER, ch, theme::ui_semibold(9.5), *color);
                        }
                        cur_logo_x += LOGO_SIZE + LOGO_GAP;
                    }
                }

                // Nome do projeto (com truncamento para não sobrepor logos/contagem)
                let name_x = row_r.min.x + 20.0;
                let name_right_limit = if !unique_models.is_empty() {
                    logos_start_x - 6.0
                } else if count > 0 {
                    count_right_x - count_w - 6.0
                } else {
                    row_r.max.x - 10.0
                };
                let max_name_w = (name_right_limit - name_x).max(1.0);
                let name_font = if proj.active { theme::ui_semibold(12.5) } else { theme::ui(12.5) };
                let name_color = if proj.active || hovered { theme::pal().text } else { theme::pal().text_2 };

                let name_g = galley_trunc(&p, &proj.name, name_font, name_color, max_name_w);
                place(
                    &p,
                    Rect::from_min_max(Pos2::new(name_x, row_r.min.y), Pos2::new(name_right_limit, row_r.max.y)),
                    Align2::LEFT_CENTER,
                    name_g,
                    name_color,
                );

                // Tooltip com caminho completo
                let resp = tip(resp, proj.path.display().to_string());

                // Interações de clique
                if let Some(chev) = chev_resp {
                    let label = if proj.expanded { "Recolher agentes" } else { "Mostrar agentes" };
                    if tip(chev, label).clicked() {
                        toggle_id = Some(proj.id.clone());
                    }
                }
                if resp.secondary_clicked() {
                    let p_pos = ui.input(|i| i.pointer.interact_pos()).unwrap_or(row_r.center());
                    open_ctx = Some((proj.id.clone(), proj.path.clone(), proj.name.clone(), p_pos));
                } else if resp.clicked() && !proj.active {
                    switch_to_id = Some(proj.id.clone());
                }

                if !proj.expanded {
                    continue;
                }

                // Agentes do projeto, em árvore (quem abriu quem)
                for node in agent_tree(&proj.agents) {
                    let ag = &proj.agents[node.idx];
                    let (tab_r, _) = ui.allocate_exact_size(Vec2::new(row_w, ROW_H), Sense::hover());
                    // Id fixo (projeto + instância): o arrasto continua na mesma linha entre os quadros.
                    let tab_resp = ui.interact(tab_r, Id::new(("agent-row", &proj.id, &ag.id)), Sense::click_and_drag());
                    let tab_hov = tab_resp.hovered() && !dragging;
                    let is_active_tab = ag.tab.is_some() && deck_active == ag.tab;
                    if tab_resp.drag_started() {
                        agent_drag = Some((proj.id.clone(), ag.id.clone(), ag.label.clone()));
                    }
                    let being_dragged = agent_drag.as_ref().is_some_and(|(dp, di, _)| *dp == proj.id && *di == ag.id);

                    if is_active_tab {
                        p.rect_filled(tab_r, 4.0, theme::pal().separator);
                    } else if tab_hov {
                        p.rect_filled(tab_r, 4.0, theme::pal().hover);
                    }
                    // Alvo de soltar: vira o chefe do arrastado (no mesmo projeto, sem ciclo).
                    if let Some((_, dragged, _)) = agent_drag.as_ref().filter(|(dp, ..)| *dp == proj.id) {
                        if !being_dragged && visible(tab_r) {
                            let ok = !in_subtree(&proj.agents, &ag.id, dragged);
                            paint_drop_target(&p, tab_r, ok, active_color);
                            drop_target = Some(DropTarget { boss: Some(ag.id.clone()), ok });
                        }
                    }
                    if being_dragged {
                        p.rect_filled(tab_r, 4.0, theme::with_opacity(theme::pal().hover, 0.6));
                    }

                    if tab_hov {
                        ctx.set_cursor_icon(CursorIcon::PointingHand);
                    }

                    paint_guides(&p, tab_r, gap, &node);
                    let logo_x = agent_x(tab_r, node.depth);
                    paint_agent_mark(deck, &ctx, &p, Pos2::new(logo_x, tab_r.center().y), ag, is_active_tab);

                    let tab_text_x = logo_x + LOGO_SIZE / 2.0 + 6.0;
                    let max_tab_w = (tab_r.max.x - 10.0 - tab_text_x).max(1.0);
                    let tab_font = if ag.done || is_active_tab { theme::ui_semibold(12.0) } else { theme::ui(12.0) };
                    let tab_color = if !ag.alive {
                        theme::with_opacity(theme::pal().text_2, 0.6)
                    } else if is_active_tab || ag.done {
                        theme::pal().text_strong
                    } else if tab_hov {
                        theme::pal().text
                    } else {
                        theme::pal().text_2
                    };

                    let tab_g = galley_trunc(&p, &ag.label, tab_font, tab_color, max_tab_w);
                    let placed_r = place(
                        &p,
                        Rect::from_min_max(Pos2::new(tab_text_x, tab_r.min.y), Pos2::new(tab_r.max.x - 10.0, tab_r.max.y)),
                        Align2::LEFT_CENTER,
                        tab_g,
                        tab_color,
                    );

                    if !ag.alive && ag.tab.is_some() {
                        p.hline(placed_r.min.x..=placed_r.max.x, placed_r.center().y + 0.5, Stroke::new(1.0, tab_color));
                    }
                    // Modelo de LLM em cinza depois do nome, se ainda sobra espaço na linha.
                    if let Some(model) = ag.model.as_deref() {
                        let x0 = placed_r.max.x + 6.0;
                        let room = tab_r.max.x - 10.0 - x0;
                        if room >= 24.0 {
                            let mcolor = theme::with_opacity(theme::pal().text_2, if ag.alive { 0.75 } else { 0.45 });
                            let mg = galley_trunc(&p, short_model(model), theme::ui(11.0), mcolor, room);
                            place(
                                &p,
                                Rect::from_min_max(Pos2::new(x0, tab_r.min.y), Pos2::new(tab_r.max.x - 10.0, tab_r.max.y)),
                                Align2::LEFT_CENTER,
                                mg,
                                mcolor,
                            );
                        }
                    }

                    let mut tooltip = if ag.busy {
                        format!("{} (trabalhando…)", ag.label)
                    } else if ag.done {
                        format!("{} (concluído)", ag.label)
                    } else if ag.tab.is_none() {
                        format!("{} (abre ao selecionar o projeto)", ag.label)
                    } else if !ag.alive {
                        format!("{} (encerrado)", ag.label)
                    } else {
                        ag.label.clone()
                    };
                    if let Some(model) = &ag.model {
                        tooltip.push_str(&format!("\nModelo: {model}"));
                    }
                    if let Some(parent) = ag.parent.as_ref().filter(|_| node.depth > 0) {
                        tooltip.push_str(&format!("\nResponde a {parent}"));
                    }
                    tooltip.push_str("\nArraste sobre outro agente para pô-lo na equipe dele");
                    if tip(tab_resp, tooltip).clicked() {
                        agent_click = Some(AgentClick { project: proj.id.clone(), id: ag.id.clone(), tab: ag.tab });
                    }
                }

                if !proj.active {
                    continue;
                }

                // "+ Novo agente" fecha a lista do projeto e abre o menu de agentes (o mesmo do Ctrl+T).
                let (add_r, add_resp) = ui.allocate_exact_size(Vec2::new(row_w, ROW_H), Sense::click());
                let add_hov = (add_resp.hovered() || deck.new_tab_menu_open) && !dragging;
                if add_hov {
                    p.rect_filled(add_r, 4.0, theme::pal().hover);
                    ctx.set_cursor_icon(CursorIcon::PointingHand);
                }
                let add_color = if add_hov { theme::pal().text } else { theme::pal().text_2 };
                text_in(
                    &p,
                    Rect::from_center_size(Pos2::new(add_r.min.x + AGENT_X, add_r.center().y), Vec2::splat(ROW_H)),
                    Align2::CENTER_CENTER,
                    "+",
                    theme::ui(14.0),
                    add_color,
                );
                text_in(
                    &p,
                    Rect::from_min_max(
                        Pos2::new(add_r.min.x + AGENT_X + LOGO_SIZE / 2.0 + 6.0, add_r.min.y),
                        Pos2::new(add_r.max.x - 10.0, add_r.max.y),
                    ),
                    Align2::LEFT_CENTER,
                    "Novo agente",
                    theme::ui(12.0),
                    add_color,
                );
                if ui.is_rect_visible(add_r) {
                    new_agent_anchor = Some(add_r);
                }
                if tip(add_resp, "Escolher um agente para abrir neste projeto").clicked() {
                    new_agent_click = true;
                }
            }
        },
    );

    *top_y = list_top + scroll_output.inner_rect.height().min(max_list_h);

    // Separador com margem de 8 px
    *top_y += 6.0;
    painter.rect_filled(
        Rect::from_min_max(Pos2::new(rect.min.x + 8.0, *top_y), Pos2::new(rect.max.x - 8.0, *top_y + 1.0)),
        0.0,
        theme::pal().separator,
    );
    *top_y += 1.0;

    // Arrasto de agente: "fantasma" junto ao ponteiro; ao soltar, muda o chefe.
    let mut set_boss = None;
    if let Some((project, id, label)) = agent_drag.clone() {
        if let Some(ptr) = pointer {
            ctx.set_cursor_icon(if drop_target.as_ref().is_some_and(|t| !t.ok) { CursorIcon::NoDrop } else { CursorIcon::Grabbing });
            let gp = ctx.layer_painter(egui::LayerId::new(Order::Tooltip, Id::new("agent-drag-ghost")));
            let hint = match &drop_target {
                Some(DropTarget { boss: Some(b), ok: true }) => format!("{label} → equipe de {b}"),
                Some(DropTarget { boss: Some(_), ok: false }) => format!("{label}: não pode ficar sob a própria equipe"),
                Some(DropTarget { boss: None, .. }) => format!("{label} → responde ao usuário"),
                None => label.clone(),
            };
            let g = galley(&gp, &hint, theme::ui(12.0), theme::pal().text);
            let r = Rect::from_min_size(ptr + Vec2::new(14.0, 10.0), g.size() + Vec2::new(16.0, 8.0));
            frame(&gp, r, 6.0, theme::pal().hover, theme::pal().separator);
            place(&gp, r, Align2::CENTER_CENTER, g, theme::pal().text);
        }
        let released = ctx.input(|i| i.pointer.any_released());
        if released || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if let Some(DropTarget { boss, ok: true }) = drop_target.filter(|_| released) {
                set_boss = Some((project, id, boss));
            }
            agent_drag = None;
        }
    }
    ctx.data_mut(|d| d.insert_temp(drag_key(), agent_drag));

    // Ações executadas fora das closures
    if let Some((project, id, boss)) = set_boss {
        deck.set_agent_boss(&project, &id, boss.as_deref());
    }
    ctx.data_mut(|d| d.insert_temp(anchor_key(), new_agent_anchor));
    if new_agent_click {
        deck.new_tab_menu_open = !deck.new_tab_menu_open;
        deck.settings_open = false;
        if deck.new_tab_menu_open {
            deck.refresh_agents();
        }
    }
    if let Some(id) = toggle_id {
        deck.toggle_project_expanded(&id);
    } else if let Some(id) = switch_to_id {
        deck.switch_project(&id);
    }
    match agent_click {
        Some(AgentClick { tab: Some(i), .. }) => {
            deck.show_agents();
            deck.activate(i);
        }
        Some(AgentClick { project, id, tab: None }) => deck.focus_agent(&project, &id),
        None => {}
    }
    if let Some((id, path, name, pos)) = open_ctx {
        ctx.data_mut(|d| {
            d.insert_temp(Id::new("proj-ctx-menu"), Some((id, path, name, pos)));
        });
    }

    // Menus e Modais
    show_context_menu(deck, &ctx);
    show_rename_modal(deck, &ctx);
}

/// Menu de contexto do projeto (clique direito).
fn show_context_menu(deck: &mut Deck, ctx: &egui::Context) {
    let ctx_data: Option<(String, PathBuf, String, Pos2)> = ctx.data(|d| d.get_temp(Id::new("proj-ctx-menu")).flatten());
    let Some((id, path, name, pos)) = ctx_data else { return };

    let can_remove = true;
    let w = 170.0;
    let item_h = 26.0;
    let h = item_h * 3.0 + 8.0;
    let pos = menus::keep_on_screen(ctx, pos, Vec2::new(w, h));
    let menu_rect = Rect::from_min_size(pos, Vec2::new(w, h));

    let mut action_rename = false;
    let mut action_open = false;
    let mut action_remove = false;

    egui::Area::new(Id::new("proj-ctx-menu-area")).order(Order::Foreground).fixed_pos(pos).constrain(false).show(ctx, |ui| {
        let (rect, _) = ui.allocate_exact_size(menu_rect.size(), Sense::hover());
        let p = ui.painter();
        shadow(p, rect, 8, 8, 24, theme::rgba(0, 0, 0, 0.55));
        frame(p, rect, 6.0, theme::pal().hover, theme::pal().separator);
        let font = theme::ui(12.5);

        let mut item_y = rect.min.y + 4.0;

        // Renomear
        let r_ren = Rect::from_min_size(Pos2::new(rect.min.x + 4.0, item_y), Vec2::new(w - 8.0, item_h));
        item_y += item_h;
        let resp_ren = clickable(ui, r_ren, Id::new("proj-ctx-ren"));
        if resp_ren.hovered() {
            p.rect_filled(r_ren, 4.0, theme::pal().separator);
        }
        text_in(p, r_ren.shrink2(Vec2::new(8.0, 0.0)), Align2::LEFT_CENTER, "Renomear", font.clone(), theme::pal().text);
        if resp_ren.clicked() {
            action_rename = true;
        }

        // Abrir pasta no sistema
        let r_open = Rect::from_min_size(Pos2::new(rect.min.x + 4.0, item_y), Vec2::new(w - 8.0, item_h));
        item_y += item_h;
        let resp_open = clickable(ui, r_open, Id::new("proj-ctx-open"));
        if resp_open.hovered() {
            p.rect_filled(r_open, 4.0, theme::pal().separator);
        }
        text_in(p, r_open.shrink2(Vec2::new(8.0, 0.0)), Align2::LEFT_CENTER, "Abrir pasta no sistema", font.clone(), theme::pal().text);
        if resp_open.clicked() {
            action_open = true;
        }

        // Remover
        let r_rem = Rect::from_min_size(Pos2::new(rect.min.x + 4.0, item_y), Vec2::new(w - 8.0, item_h));
        let resp_rem = if can_remove {
            clickable(ui, r_rem, Id::new("proj-ctx-rem"))
        } else {
            ui.interact(r_rem, Id::new("proj-ctx-rem"), Sense::hover())
        };
        if resp_rem.hovered() && can_remove {
            p.rect_filled(r_rem, 4.0, theme::pal().separator);
        }
        let rem_color = if !can_remove {
            theme::with_opacity(theme::pal().text_2, 0.5)
        } else if resp_rem.hovered() {
            theme::pal().danger_text
        } else {
            theme::pal().text
        };
        text_in(p, r_rem.shrink2(Vec2::new(8.0, 0.0)), Align2::LEFT_CENTER, "Remover", font, rem_color);
        if can_remove && resp_rem.clicked() {
            action_remove = true;
        }
    });

    if action_rename {
        ctx.data_mut(|d| {
            d.insert_temp(Id::new("proj-ctx-menu"), None::<(String, PathBuf, String, Pos2)>);
            d.insert_temp(Id::new("proj-rename-modal"), Some((id, name)));
        });
    } else if action_open {
        open_project_folder(deck, &path);
        ctx.data_mut(|d| d.insert_temp(Id::new("proj-ctx-menu"), None::<(String, PathBuf, String, Pos2)>));
    } else if action_remove {
        ctx.data_mut(|d| d.insert_temp(Id::new("proj-ctx-menu"), None::<(String, PathBuf, String, Pos2)>));
        deck.confirm_remove_project(id, name);
    } else if ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !menu_rect.contains(p)))
        || ctx.input(|i| i.key_pressed(egui::Key::Escape))
    {
        ctx.data_mut(|d| d.insert_temp(Id::new("proj-ctx-menu"), None::<(String, PathBuf, String, Pos2)>));
    }
}

/// Modal para renomear projeto (modal de texto simples).
fn show_rename_modal(deck: &mut Deck, ctx: &egui::Context) {
    let modal_data: Option<(String, String)> = ctx.data(|d| d.get_temp(Id::new("proj-rename-modal")).flatten());
    let Some((id, mut buffer)) = modal_data else { return };

    let screen = ctx.content_rect();
    super::modal_backdrop(deck, ctx, "proj-rename-backdrop");

    let w = 360.0;
    let h = 145.0;
    let center = screen.center();
    let rect = Rect::from_center_size(center, Vec2::new(w, h));
    let mut close = false;
    let mut apply = false;

    egui::Area::new(Id::new("proj-rename-modal-area")).order(Order::Tooltip).fixed_pos(rect.min).show(ctx, |ui| {
        let (r, _) = ui.allocate_exact_size(rect.size(), Sense::hover());
        let p = ui.painter().clone();
        shadow(&p, r, 0, 8, 30, theme::rgba(0, 0, 0, 0.7));
        frame(&p, r, 10.0, theme::pal().hover, theme::pal().separator);

        let mut y = r.min.y + 16.0;
        // Título
        let title_g = galley(&p, "Renomear Projeto", theme::ui_semibold(15.0), theme::pal().text);
        place(
            &p,
            Rect::from_min_size(Pos2::new(r.min.x + 20.0, y), Vec2::new(w - 40.0, 20.0)),
            Align2::LEFT_CENTER,
            title_g,
            theme::pal().text,
        );
        y += 24.0;

        // Explicação
        let hint_g = galley(&p, "Altera o nome do projeto exibido na barra lateral", theme::ui(11.5), theme::pal().text_2);
        place(
            &p,
            Rect::from_min_size(Pos2::new(r.min.x + 20.0, y), Vec2::new(w - 40.0, 16.0)),
            Align2::LEFT_CENTER,
            hint_g,
            theme::pal().text_2,
        );
        y += 22.0;

        // Caixa de texto
        let input_r = Rect::from_min_size(Pos2::new(r.min.x + 20.0, y), Vec2::new(w - 40.0, 28.0));
        p.rect_filled(input_r, 6.0, theme::pal().bg);
        p.rect_stroke(input_r, 6.0, Stroke::new(1.0, theme::pal().separator), egui::StrokeKind::Inside);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(input_r.shrink2(Vec2::new(8.0, 4.0))));
        let input_id = Id::new("proj-rename-input");
        let te = egui::TextEdit::singleline(&mut buffer)
            .id(input_id)
            .frame(egui::Frame::NONE)
            .font(theme::ui(13.0))
            .text_color(theme::pal().text)
            .desired_width(input_r.width() - 16.0);
        let out = te.show(&mut child);
        out.response.request_focus();
        if (out.response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            apply = true;
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }
        y += 34.0;

        // Botões Cancelar e Salvar
        let btn_w = 80.0;
        let btn_h = 26.0;
        let save_r = Rect::from_min_size(Pos2::new(r.max.x - 20.0 - btn_w, y), Vec2::new(btn_w, btn_h));
        let cancel_r = Rect::from_min_size(Pos2::new(save_r.min.x - 10.0 - btn_w, y), Vec2::new(btn_w, btn_h));

        let cr = clickable(ui, cancel_r, Id::new("proj-rename-cancel-btn"));
        if cr.hovered() {
            p.rect_filled(cancel_r, 6.0, theme::pal().separator);
        } else {
            p.rect_filled(cancel_r, 6.0, theme::pal().chip);
        }
        text_in(&p, cancel_r, Align2::CENTER_CENTER, "Cancelar", theme::ui(12.5), theme::pal().text_2);
        if cr.clicked() {
            close = true;
        }

        let sr = clickable(ui, save_r, Id::new("proj-rename-save-btn"));
        if sr.hovered() {
            p.rect_filled(save_r, 6.0, theme::brighten(theme::accent(), 1.15));
        } else {
            p.rect_filled(save_r, 6.0, theme::accent());
        }
        text_in(&p, save_r, Align2::CENTER_CENTER, "Salvar", theme::ui_semibold(12.5), theme::pal().text_strong);
        if sr.clicked() {
            apply = true;
        }
    });

    if close {
        ctx.data_mut(|d| d.insert_temp(Id::new("proj-rename-modal"), None::<(String, String)>));
    } else if apply {
        let trimmed = buffer.trim();
        if !trimmed.is_empty() {
            deck.rename_project(&id, trimmed);
        }
        ctx.data_mut(|d| d.insert_temp(Id::new("proj-rename-modal"), None::<(String, String)>));
    } else {
        ctx.data_mut(|d| d.insert_temp(Id::new("proj-rename-modal"), Some((id, buffer))));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ag(id: &str, parent: Option<&str>) -> AgentBadge {
        AgentBadge {
            id: id.into(),
            parent: parent.map(Into::into),
            kind: "claude".into(),
            label: id.into(),
            model: None,
            color: Color32::WHITE,
            busy: false,
            done: false,
            alive: true,
            tab: None,
        }
    }

    /// (índice, profundidade) de cada linha.
    fn shape(rows: &[TreeRow]) -> Vec<(usize, usize)> {
        rows.iter().map(|r| (r.idx, r.depth)).collect()
    }

    #[test]
    fn filhos_vem_logo_abaixo_do_pai() {
        // claude, antigravity (aberto pelo claude), claude@dev (aberto pelo claude), ollama (pelo claude@dev), codex (raiz)
        let agents = [
            ag("claude", None),
            ag("antigravity", Some("claude")),
            ag("codex", None),
            ag("claude@dev", Some("claude")),
            ag("ollama", Some("claude@dev")),
        ];
        let rows = agent_tree(&agents);
        assert_eq!(shape(&rows), [(0, 0), (1, 1), (3, 1), (4, 2), (2, 0)]);
        assert!(rows[0].has_kids && !rows[1].has_kids);
        assert!(!rows[1].last && rows[2].last && rows[3].last);
        // O ollama está sob o último filho do claude: a guia do nível 1 não continua.
        assert_eq!(rows[3].rails, [false]);
    }

    #[test]
    fn guia_continua_quando_o_ancestral_tem_irmaos_abaixo() {
        let agents = [ag("a", None), ag("b", Some("a")), ag("c", Some("b")), ag("d", Some("a"))];
        let rows = agent_tree(&agents);
        assert_eq!(shape(&rows), [(0, 0), (1, 1), (2, 2), (3, 1)]);
        assert_eq!(rows[2].rails, [true]);
    }

    #[test]
    fn nao_se_solta_um_agente_dentro_da_propria_equipe() {
        let agents = [ag("claude", None), ag("dev", Some("claude")), ag("ollama", Some("dev")), ag("review", None)];
        assert!(in_subtree(&agents, "ollama", "claude"), "neto");
        assert!(in_subtree(&agents, "dev", "dev"), "ele mesmo");
        assert!(!in_subtree(&agents, "review", "claude"));
        assert!(!in_subtree(&agents, "claude", "ollama"), "subir para o avô pode");
    }

    #[test]
    fn pai_ausente_ou_ciclo_vira_raiz() {
        let agents = [ag("a", Some("sumiu")), ag("b", Some("c")), ag("c", Some("b")), ag("d", Some("d"))];
        let rows = agent_tree(&agents);
        assert_eq!(rows.len(), 4, "ninguém some da lista");
        assert_eq!(shape(&rows), [(0, 0), (3, 0), (1, 0), (2, 1)]);
    }
}
