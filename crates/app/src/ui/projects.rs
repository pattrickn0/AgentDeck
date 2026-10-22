//! Seção PROJETOS da barra lateral: lista de projetos, agentes de cada um (em árvore: quem abriu
//! quem) e ações rápidas.

use std::path::{Path, PathBuf};

use agentdeck_core::models::short_model;

use egui::{Align2, Color32, CursorIcon, Id, Order, Painter, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use super::{clickable, dot, frame, galley, galley_trunc, menus, place, shadow, text_in, tip};
use crate::deck::{AgentBadge, Deck, ProjectView};
use crate::icons::Icon;
use crate::theme;

/// Altura da linha de um projeto e da "+ Novo agente".
pub const ROW_H: f32 = 22.0;
/// Altura da linha de um agente.
const AGENT_ROW_H: f32 = 24.0;
pub const SECTION_H: f32 = 24.0;
/// Margem lateral das linhas: a seleção e o hover não encostam na borda da ilha.
const SIDE_MARGIN: f32 = 4.0;
/// O logo do modelo (12 px) mora numa caixa de 16 px.
const LOGO_SIZE: f32 = 12.0;
const MARK_BOX: f32 = 16.0;
/// Centro do logo de um agente da raiz (a partir da borda esquerda da linha).
const AGENT_X: f32 = 30.0;
/// Recuo de cada nível da árvore de agentes.
const INDENT: f32 = 12.0;
/// Espaço entre o logo e o nome, e entre o nome e o modelo.
const GAP: f32 = 7.0;
/// Largura máxima do modelo à direita do nome (o resto é do nome).
const MODEL_MAX_W: f32 = 64.0;
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

/// O modelo de LLM como cabe numa linha da barra lateral: a família (`claude-sonnet-5-5` → `sonnet`,
/// `gemini-4-argon-high` → `argon`, `opencode/deepseek-v4.1-flash-free` → `flash`). Sem família
/// conhecida, o nome sem o provedor, sem o `claude-` e sem o `-free`.
pub fn model_tag(model: &str) -> String {
    const FAMILIES: [&str; 7] = ["opus", "sonnet", "haiku", "fable", "argon", "flash", "codex"];
    let short = short_model(model).to_lowercase();
    if let Some(family) = FAMILIES.iter().find(|f| short.split(['-', '.', '_', ' ', ':']).any(|t| t == **f)) {
        return (*family).to_owned();
    }
    let short = short.strip_prefix("claude-").unwrap_or(&short);
    short.strip_suffix("-free").unwrap_or(short).to_owned()
}

/// Altura do que a lista de projetos desenha: uma linha por projeto e, nos abertos, uma por agente e, no
/// ativo, a "+ Novo agente".
fn list_height(projects: &[ProjectView]) -> f32 {
    projects
        .iter()
        .map(|p| {
            let agents = if p.expanded { p.agents.len() as f32 * AGENT_ROW_H + if p.active { ROW_H } else { 0.0 } } else { 0.0 };
            ROW_H + agents
        })
        .sum()
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
    let stroke = Stroke::new(1.0, theme::pal().guide);
    let y = row.center().y;
    if node.has_kids {
        p.vline(agent_x(row, node.depth), (y + MARK_BOX / 2.0 + 1.0)..=row.max.y, stroke);
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
    p.hline(x..=(agent_x(row, node.depth) - MARK_BOX / 2.0 - 2.0), y, stroke);
}

/// Logo do modelo do agente (ponto, se não há logo), com o estado: halo pulsando trabalhando,
/// anel com turno concluído não visto, apagado se o processo encerrou.
fn paint_agent_mark(deck: &mut Deck, ctx: &egui::Context, p: &Painter, center: Pos2, ag: &AgentBadge, is_active_tab: bool) {
    if ag.busy {
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
        let t = ctx.input(|inp| inp.time);
        let wave = (t * 4.8).sin() as f32 * 0.5 + 0.5;
        p.circle_filled(center, 7.0 + 3.5 * wave, theme::with_opacity(ag.color, 0.30 * (1.0 - wave)));
    } else if ag.done && !is_active_tab {
        // Anel de 1,5 px logo fora da caixa do logo (16 px).
        p.circle_stroke(center, MARK_BOX / 2.0 + 0.75, Stroke::new(1.5, ag.color));
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
    p.rect_filled(r, 5.0, theme::with_opacity(c, 0.12));
    p.rect_stroke(r.shrink(0.5), 5.0, Stroke::new(1.2, c), egui::StrokeKind::Inside);
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
    super::section_label(&painter, head_rect, "PROJETOS");

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
    let gap = ui.spacing().item_spacing.y;
    let content_h = list_height(&projects) + gap * projects.len() as f32;
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
                let inner = row_r.shrink2(Vec2::new(SIDE_MARGIN, 0.0));
                // A seta fica por cima da linha (criada depois, ganha o clique).
                let can_expand = proj.active || !proj.agents.is_empty();
                let chev_r = Rect::from_min_size(inner.min, Vec2::new(20.0, ROW_H));
                let chev_resp = can_expand.then(|| ui.interact(chev_r, Id::new(("proj-chevron", &proj.id)), Sense::click()));
                let chev_hov = chev_resp.as_ref().is_some_and(|r| r.hovered()) && !dragging;
                let hovered = (resp.hovered() || chev_hov) && !dragging;

                if hovered {
                    p.rect_filled(inner, 5.0, theme::pal().hover);
                    ctx.set_cursor_icon(CursorIcon::PointingHand);
                }
                // Soltar sobre o próprio projeto devolve o agente à raiz (responde ao usuário).
                // Só o que está à vista na lista conta (as linhas roladas para fora ficam por baixo de outras seções).
                let clip = ui.clip_rect();
                let visible = |r: Rect| pointer.is_some_and(|ptr| r.contains(ptr) && clip.contains(ptr));
                if agent_drag.as_ref().is_some_and(|(dp, ..)| *dp == proj.id) && visible(row_r) {
                    paint_drop_target(&p, inner, true, active_color);
                    drop_target = Some(DropTarget { boss: None, ok: true });
                }
                if can_expand {
                    let c = if chev_hov { theme::pal().text_2 } else { theme::pal().text_3 };
                    paint_chevron(&p, Pos2::new(inner.min.x + 10.5, row_r.center().y), proj.expanded, c);
                }

                // Lado direito: o anel de um agente que terminou sem ser visto (projeto de fora) e a contagem.
                let mut right = inner.max.x - 6.0;
                let unseen = proj.agents.iter().find(|a| a.done && a.alive).filter(|_| !proj.active);
                if let Some(ag) = unseen {
                    p.circle_stroke(Pos2::new(right - 3.0, row_r.center().y), 3.75, Stroke::new(1.5, ag.color));
                    right -= 6.0 + 6.0;
                }
                let count = proj.agents.len();
                if count > 0 {
                    let count_str = count.to_string();
                    let g = galley(&p, &count_str, theme::ui(10.5), theme::pal().text_3);
                    let w = g.size().x;
                    place(&p, Rect::from_min_max(Pos2::new(right - w, row_r.min.y), Pos2::new(right, row_r.max.y)), Align2::LEFT_CENTER, g, theme::pal().text_3);
                    right -= w + 6.0;
                }

                // Nome do projeto (com reticências para não encostar no que está à direita)
                let name_x = inner.min.x + 6.0 + 9.0 + 6.0;
                let name_color = if proj.active || hovered { theme::pal().text } else { theme::pal().text_2 };
                let name_g = galley_trunc(&p, &proj.name, theme::ui_semibold(12.0), name_color, (right - name_x).max(1.0));
                place(&p, Rect::from_min_max(Pos2::new(name_x, row_r.min.y), Pos2::new(right, row_r.max.y)), Align2::LEFT_CENTER, name_g, name_color);

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
                    let (tab_r, _) = ui.allocate_exact_size(Vec2::new(row_w, AGENT_ROW_H), Sense::hover());
                    let sel_r = tab_r.shrink2(Vec2::new(SIDE_MARGIN, 0.0));
                    // Id fixo (projeto + instância): o arrasto continua na mesma linha entre os quadros.
                    let tab_resp = ui.interact(tab_r, Id::new(("agent-row", &proj.id, &ag.id)), Sense::click_and_drag());
                    let tab_hov = tab_resp.hovered() && !dragging;
                    let is_active_tab = ag.tab.is_some() && deck_active == ag.tab;
                    if tab_resp.drag_started() {
                        agent_drag = Some((proj.id.clone(), ag.id.clone(), ag.label.clone()));
                    }
                    let being_dragged = agent_drag.as_ref().is_some_and(|(dp, di, _)| *dp == proj.id && *di == ag.id);

                    if is_active_tab {
                        // Fundo na cor de destaque (18%) e filete de 2 px na borda esquerda.
                        p.rect_filled(sel_r, 5.0, theme::accent_soft(active_color));
                        let edge = egui::CornerRadius { nw: 5, sw: 5, ne: 0, se: 0 };
                        p.rect_filled(Rect::from_min_size(sel_r.min, Vec2::new(2.0, sel_r.height())), edge, active_color);
                    } else if tab_hov {
                        p.rect_filled(sel_r, 5.0, theme::pal().hover);
                    }
                    // Alvo de soltar: vira o chefe do arrastado (no mesmo projeto, sem ciclo).
                    if let Some((_, dragged, _)) = agent_drag.as_ref().filter(|(dp, ..)| *dp == proj.id) {
                        if !being_dragged && visible(tab_r) {
                            let ok = !in_subtree(&proj.agents, &ag.id, dragged);
                            paint_drop_target(&p, sel_r, ok, active_color);
                            drop_target = Some(DropTarget { boss: Some(ag.id.clone()), ok });
                        }
                    }
                    if being_dragged {
                        p.rect_filled(sel_r, 5.0, theme::with_opacity(theme::pal().hover, 0.6));
                    }

                    if tab_hov {
                        ctx.set_cursor_icon(CursorIcon::PointingHand);
                    }

                    paint_guides(&p, tab_r, gap, &node);
                    let logo_x = agent_x(tab_r, node.depth);
                    paint_agent_mark(deck, &ctx, &p, Pos2::new(logo_x, tab_r.center().y), ag, is_active_tab);

                    // O modelo à direita, numa linha só; o nome é quem encolhe (com reticências).
                    let right = sel_r.max.x - 8.0;
                    let model = ag.model.as_deref().map(model_tag).filter(|m| !m.is_empty());
                    let mcolor = if is_active_tab { theme::pal().text } else { theme::pal().text_3 };
                    let model_w = model.as_deref().map_or(0.0, |m| {
                        let mg = galley_trunc(&p, m, theme::meta(10.0), mcolor, MODEL_MAX_W);
                        let w = mg.size().x;
                        place(&p, Rect::from_min_max(Pos2::new(right - w, tab_r.min.y), Pos2::new(right, tab_r.max.y)), Align2::LEFT_CENTER, mg, mcolor);
                        w
                    });

                    let name_x = logo_x + MARK_BOX / 2.0 + GAP;
                    let name_right = if model_w > 0.0 { right - model_w - GAP } else { right };
                    let tab_font = if is_active_tab { theme::ui_semibold(12.0) } else { theme::ui_medium(12.0) };
                    let tab_color = if !ag.alive {
                        theme::pal().text_3
                    } else if is_active_tab {
                        theme::pal().text_strong
                    } else {
                        theme::pal().text
                    };
                    let tab_g = galley_trunc(&p, &ag.label, tab_font, tab_color, (name_right - name_x).max(1.0));
                    let placed_r = place(
                        &p,
                        Rect::from_min_max(Pos2::new(name_x, tab_r.min.y), Pos2::new(name_right, tab_r.max.y)),
                        Align2::LEFT_CENTER,
                        tab_g,
                        tab_color,
                    );
                    if !ag.alive && ag.tab.is_some() {
                        p.hline(placed_r.min.x..=placed_r.max.x, placed_r.center().y + 0.5, Stroke::new(1.0, tab_color));
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
                    p.rect_filled(add_r.shrink2(Vec2::new(SIDE_MARGIN, 0.0)), 5.0, theme::pal().hover);
                    ctx.set_cursor_icon(CursorIcon::PointingHand);
                }
                let add_color = if add_hov { theme::pal().text_2 } else { theme::pal().text_3 };
                let add_text = Rect::from_min_max(Pos2::new(add_r.min.x + AGENT_X - MARK_BOX / 2.0, add_r.min.y), Pos2::new(add_r.max.x - 10.0, add_r.max.y));
                text_in(&p, add_text, Align2::LEFT_CENTER, "+ Novo agente", theme::ui(11.5), add_color);
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

    // Separador da largura da ilha
    *top_y += 6.0;
    super::separator(&painter, Pos2::new(rect.min.x, *top_y + 0.5), Pos2::new(rect.max.x, *top_y + 0.5));
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
            frame(&gp, r, 6.0, theme::pal().popup, theme::pal().glass_border);
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
        shadow(p, rect, 8, 8, 24, theme::with_alpha(theme::pal().shadow, 0.55));
        frame(p, rect, 6.0, theme::pal().popup, theme::pal().glass_border);
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
        shadow(&p, r, 0, 8, 30, theme::with_alpha(theme::pal().shadow, 0.7));
        frame(&p, r, 10.0, theme::pal().popup, theme::pal().glass_border);

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
        p.rect_filled(input_r, 6.0, theme::pal().recess);
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
        text_in(&p, save_r, Align2::CENTER_CENTER, "Salvar", theme::ui_semibold(12.5), theme::pal().on_accent);
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
    fn o_modelo_vira_a_familia() {
        assert_eq!(model_tag("claude-sonnet-5-5"), "sonnet");
        assert_eq!(model_tag("claude-opus-5-5"), "opus");
        assert_eq!(model_tag("gemini-4-argon-high"), "argon");
        assert_eq!(model_tag("opencode/deepseek-v4.1-flash-free"), "flash");
        assert_eq!(model_tag("opencode-go/glm-5.3-flash"), "flash");
        assert_eq!(model_tag("sonnet"), "sonnet");
        assert_eq!(model_tag("Haiku"), "haiku");
    }

    #[test]
    fn o_modelo_sem_familia_perde_so_o_enfeite() {
        assert_eq!(model_tag("opencode/big-pickle-free"), "big-pickle");
        assert_eq!(model_tag("claude-custom-1"), "custom-1");
        assert_eq!(model_tag("llama3:8b"), "llama3:8b");
        assert_eq!(model_tag(""), "");
        // Só vale a palavra inteira.
        assert_eq!(model_tag("flashlight-2"), "flashlight-2");
    }

    fn view(agents: usize, expanded: bool, active: bool) -> ProjectView {
        ProjectView {
            id: "p".into(),
            name: "p".into(),
            path: PathBuf::new(),
            active,
            expanded,
            agents: (0..agents).map(|i| ag(&format!("a{i}"), None)).collect(),
        }
    }

    #[test]
    fn a_altura_da_lista_conta_agentes_e_o_novo_agente() {
        // Fechado: só a linha do projeto.
        assert_eq!(list_height(&[view(3, false, true)]), ROW_H);
        // Aberto e ativo: projeto + 3 agentes + "+ Novo agente".
        assert_eq!(list_height(&[view(3, true, true)]), ROW_H + 3.0 * AGENT_ROW_H + ROW_H);
        // Aberto e inativo: sem o "+ Novo agente".
        assert_eq!(list_height(&[view(2, true, false), view(0, false, false)]), ROW_H + 2.0 * AGENT_ROW_H + ROW_H);
        assert_eq!(list_height(&[]), 0.0);
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
