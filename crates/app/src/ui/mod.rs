//! Pintura da interface (equivalente ao `index.html` + `styles.css`).
//! O layout é calculado à mão para reproduzir as medidas do CSS ao pixel.

mod background;
mod docs;
mod empty_state;
mod git;
mod git_views;
mod github;
mod menus;
mod overlay;
mod panes;
mod projects;
mod right_panel;
mod shortcuts;
mod sidebar;
mod tabs;
mod tasks;
mod titlebar;
mod usage;

use std::sync::Arc;

use egui::{Align2, Color32, CursorIcon, FontId, Galley, Id, Order, Painter, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2};

use crate::deck::Deck;
use crate::theme;

pub use panes::pane_body;

/// Largura mínima que sobra para o terminal ao lado da barra direita.
const TERM_MIN_W: f32 = 360.0;

/// Largura efetiva da barra direita e o máximo que a janela permite agora. O valor salvo não é
/// alterado: se a janela crescer de novo, a barra volta à largura escolhida.
pub fn right_width(saved: f32, win_w: f32, left: f32) -> (f32, f32) {
    let max = theme::RIGHT_MAX_W.min(win_w * 0.5).min(win_w - left - TERM_MIN_W).max(260.0);
    (saved.max(theme::RIGHT_MIN_W).min(max), max)
}

/// Desenha a janela inteira.
pub fn draw(deck: &mut Deck, ui: &mut Ui) {
    let window = ui.max_rect();
    // A cor de destaque do quadro (a do agente em foco) vale para tudo que for desenhado abaixo.
    theme::set_accent(deck.accent_opt());
    background::draw(ui.painter(), window, deck.accent());
    theme::sync_selection(ui.ctx(), deck.accent());
    let ctx = ui.ctx().clone();
    // Cada modal desenhado neste quadro volta a marcar (ver `modal_backdrop`).
    deck.modal_open = false;

    // ---------- Layout: barra de título em cima; embaixo, as ilhas (barra esquerda | centro | barra direita) ----------
    let gap = theme::ISLAND_GAP;
    let titlebar = Rect::from_min_size(window.min, Vec2::new(window.width(), theme::TITLEBAR_H));
    let body = Rect::from_min_max(Pos2::new(window.min.x + gap, titlebar.max.y), Pos2::new(window.max.x - gap, window.max.y - gap));
    let collapsed = deck.sidebar_collapsed;
    let sw = deck.sidebar_w.clamp(theme::SIDEBAR_MIN_W, theme::SIDEBAR_MAX_W);
    let left = if collapsed { 0.0 } else { sw + gap };
    let (right_w, right_max) = right_width(deck.right_w, body.width(), left + gap);
    let center = Rect::from_min_max(Pos2::new(body.min.x + left, body.min.y), Pos2::new(body.max.x - right_w - gap, body.max.y));
    let side = Rect::from_min_size(body.min, Vec2::new(sw, body.height()));
    // A barra tem sempre a mesma largura: as listas e o diff do Git rolam dentro dela.
    let panel = Rect::from_min_max(Pos2::new(body.max.x - right_w, body.min.y), body.max);
    deck.right_panel_rect = Some(panel);

    // ---------- Sombras das ilhas (todas antes de qualquer fundo, só fora das ilhas) ----------
    deck.docs.sync(&deck.active_project);
    let docs_mode = deck.docs.view == crate::docs::CenterView::Docs;
    let mut islands = if docs_mode { vec![center] } else { panes::island_rects(deck, center) };
    islands.push(panel);
    if !collapsed {
        islands.push(side);
    }
    island_shadows(ui.painter(), Rect::from_min_max(Pos2::new(window.min.x, titlebar.max.y), window.max), &islands);

    // ---------- Centro: os terminais (tela dividida por tipo de agente) ou os documentos ----------
    if docs_mode {
        // Os terminais continuam rodando escondidos, e o PTY não é redimensionado: sem `fit_tab` aqui.
        deck.terminal_rect = Some(panes::pane_body(center));
        deck.panes.clear();
        deck.pane_drag = None;
        island(ui, center, IslandKind::Well);
        docs::draw(deck, ui, center);
        island_edge(ui, center, IslandKind::Well);
    } else {
        panes::draw(deck, ui, center);
    }

    // ---------- Barra lateral ----------
    if !collapsed {
        island(ui, side, IslandKind::Glass);
        sidebar::draw(deck, ui, side);
        island_edge(ui, side, IslandKind::Glass);
    }

    // ---------- Barra de título ----------
    let menus = titlebar::draw(deck, ui, titlebar);

    // ---------- Divisores (por último no fundo: ganham o clique na fresta entre as ilhas) ----------
    if !collapsed {
        sidebar::splitter(deck, ui, Rect::from_min_size(Pos2::new(side.max.x, body.min.y), Vec2::new(gap, body.height())), body);
    }
    let line = Rect::from_min_size(Pos2::new(center.max.x, body.min.y), Vec2::new(gap, body.height()));
    right_panel::splitter(deck, ui, line, body, right_w, right_max);

    // ---------- Barra direita (numa camada acima dos terminais) ----------
    egui::Area::new(Id::new("right-panel")).order(Order::Middle).fixed_pos(panel.min).show(&ctx, |ui| {
        let (rect, _) = ui.allocate_exact_size(panel.size(), Sense::hover());
        island(ui, rect, IslandKind::Glass);
        right_panel::draw_panel(deck, ui, rect);
        island_edge(ui, rect, IslandKind::Glass);
    });

    // ---------- Menus suspensos e overlay ----------
    menus::new_tab_menu(deck, &ctx, projects::new_agent_anchor(&ctx, menus.fallback_anchor));
    menus::settings_menu(deck, &ctx, menus.settings_anchor);
    menus::tab_context_menu(deck, &ctx);
    menus::rename_tab_modal(deck, &ctx);
    shortcuts::shortcuts_modal(deck, &ctx);
    overlay::drop_overlay(deck, &ctx, window);

    // Um campo de texto do egui (ex.: formulário de tarefa ou commit) tem foco de teclado → o terminal não.
    let egui_focus = ctx.memory(|m| m.focused());
    if egui_focus.is_some() {
        deck.term_focus = false;
        deck.ime_preedit.clear();
    }
}

// ---------------------------------------------------------------------------
// Ilhas
// ---------------------------------------------------------------------------

/// O jeito de uma ilha: vidro translúcido (barras) ou o poço quase sólido do terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IslandKind {
    Glass,
    Well,
}

impl IslandKind {
    fn fill(self) -> Color32 {
        match self {
            IslandKind::Glass => theme::pal().glass,
            IslandKind::Well => theme::pal().well,
        }
    }
}

fn island_radius() -> f32 {
    theme::ISLAND_RADIUS as f32
}

/// `area` menos cada retângulo de `holes`: sobram só as frestas e margens, onde a sombra pode aparecer.
pub fn free_rects(area: Rect, holes: &[Rect]) -> Vec<Rect> {
    let mut free = vec![area];
    for hole in holes {
        free = free
            .into_iter()
            .flat_map(|r| {
                let h = r.intersect(*hole);
                if h.width() <= 0.0 || h.height() <= 0.0 {
                    return vec![r];
                }
                let mut parts = Vec::with_capacity(4);
                if h.min.y > r.min.y {
                    parts.push(Rect::from_min_max(r.min, Pos2::new(r.max.x, h.min.y)));
                }
                if h.max.y < r.max.y {
                    parts.push(Rect::from_min_max(Pos2::new(r.min.x, h.max.y), r.max));
                }
                if h.min.x > r.min.x {
                    parts.push(Rect::from_min_max(Pos2::new(r.min.x, h.min.y), Pos2::new(h.min.x, h.max.y)));
                }
                if h.max.x < r.max.x {
                    parts.push(Rect::from_min_max(Pos2::new(h.max.x, h.min.y), Pos2::new(r.max.x, h.max.y)));
                }
                parts
            })
            .collect();
    }
    free
}

/// Sombra suave (y 8, desfoque 24) de cada ilha, só nas frestas e margens de `area`: o vidro é
/// translúcido, então uma sombra por baixo dele o escureceria (no CSS ela é recortada por dentro).
pub fn island_shadows(painter: &Painter, area: Rect, islands: &[Rect]) {
    let shape =
        |r: Rect| egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: theme::pal().shadow }.as_shape(r, theme::ISLAND_RADIUS);
    for free in free_rects(area, islands) {
        let clipped = painter.with_clip_rect(free.intersect(painter.clip_rect()));
        // Só as ilhas cuja sombra (até 32 px) alcança esta sobra.
        for r in islands.iter().filter(|r| r.expand(32.0).intersects(free)) {
            clipped.add(shape(*r));
        }
    }
}

/// Fundo de uma ilha: cantos de 10 px. Desenhe o conteúdo depois e chame [`island_edge`] por último.
pub fn island(ui: &Ui, rect: Rect, kind: IslandKind) {
    ui.painter().rect_filled(rect, island_radius(), kind.fill());
}

/// Acabamento de uma ilha, depois do conteúdo: repinta o fundo nos quatro cantos (o conteúdo é
/// retangular e vazaria pelo arredondado), a borda de 1 px e, no vidro, o filete de luz no topo interno.
pub fn island_edge(ui: &Ui, rect: Rect, kind: IslandKind) {
    let painter = ui.painter();
    let window = ui.ctx().content_rect();
    let accent = theme::accent();
    let r = island_radius();
    // Cada canto: o leque entre o vértice do quadrado e o arco do círculo de canto.
    let mut mesh = egui::epaint::Mesh::default();
    for (corner, dx, dy) in
        [(rect.left_top(), 1.0, 1.0), (rect.right_top(), -1.0, 1.0), (rect.left_bottom(), 1.0, -1.0), (rect.right_bottom(), -1.0, -1.0)]
    {
        let center = corner + Vec2::new(dx * r, dy * r);
        let mut vertex = |pos: Pos2| {
            mesh.vertices.push(egui::epaint::Vertex { pos, uv: egui::epaint::WHITE_UV, color: background::color_at(pos, window, accent) });
            mesh.vertices.len() as u32 - 1
        };
        let apex = vertex(corner);
        const STEPS: usize = 8;
        // Do ponto sobre a borda vertical até o ponto sobre a borda horizontal.
        let arc: Vec<u32> = (0..=STEPS)
            .map(|i| {
                let th = i as f32 / STEPS as f32 * std::f32::consts::FRAC_PI_2;
                vertex(center + Vec2::new(-dx * r * th.sin(), -dy * r * th.cos()))
            })
            .collect();
        for w in arc.windows(2) {
            mesh.add_triangle(apex, w[0], w[1]);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
    // Filete no topo interno, entre os cantos (só o vidro tem; o poço não).
    if kind == IslandKind::Glass {
        painter.line_segment(
            [Pos2::new(rect.min.x + r, rect.min.y + 1.0), Pos2::new(rect.max.x - r, rect.min.y + 1.0)],
            Stroke::new(1.0, theme::pal().glass_highlight),
        );
    }
    painter.rect_stroke(rect, r, Stroke::new(1.0, theme::pal().glass_border), egui::StrokeKind::Inside);
}

// ---------------------------------------------------------------------------
// Utilitários de pintura
// ---------------------------------------------------------------------------

/// Fundo escurecido de um modal. Fica numa camada acima de tudo menos do próprio modal e segura o
/// clique, o arrasto e o hover do que está atrás; marca `modal_open`, que tira o teclado do
/// terminal e dos atalhos do app enquanto o modal está aberto.
pub fn modal_backdrop(deck: &mut Deck, ctx: &egui::Context, id: &str) {
    deck.modal_open = true;
    let screen = ctx.content_rect();
    egui::Area::new(Id::new(id)).order(Order::Foreground).fixed_pos(screen.min).constrain(false).show(ctx, |ui| {
        let (r, _) = ui.allocate_exact_size(screen.size(), Sense::click_and_drag());
        ui.painter().rect_filled(r, 0.0, theme::pal().scrim);
    });
}

/// Texto numa linha (sem quebra).
pub fn galley(painter: &Painter, text: &str, font: FontId, color: Color32) -> Arc<Galley> {
    painter.layout_no_wrap(text.to_owned(), font, color)
}

/// Texto numa linha com reticências (`text-overflow: ellipsis`).
pub fn galley_trunc(painter: &Painter, text: &str, font: FontId, color: Color32, max_w: f32) -> Arc<Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping { max_width: max_w.max(1.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    painter.layout_job(job)
}

/// Texto numa linha com espaçamento extra entre as letras (`letter-spacing`, em px).
pub fn galley_spaced(painter: &Painter, text: &str, font: FontId, color: Color32, spacing: f32) -> Arc<Galley> {
    let mut job = egui::text::LayoutJob::default();
    job.append(text, 0.0, egui::TextFormat { font_id: font, color, extra_letter_spacing: spacing, ..Default::default() });
    painter.layout_job(job)
}

/// Largura do texto numa linha.
pub fn text_w(painter: &Painter, text: &str, font: FontId) -> f32 {
    galley(painter, text, font, Color32::WHITE).size().x
}

/// Pinta a galley alinhada dentro de `rect` (centralizada verticalmente) e devolve o retângulo ocupado.
pub fn place(painter: &Painter, rect: Rect, align: Align2, g: Arc<Galley>, color: Color32) -> Rect {
    let r = align.align_size_within_rect(g.size(), rect);
    painter.galley(r.min, g, color);
    r
}

/// Texto alinhado dentro de um retângulo.
pub fn text_in(painter: &Painter, rect: Rect, align: Align2, text: &str, font: FontId, color: Color32) -> Rect {
    let g = galley(painter, text, font, color);
    place(painter, rect, align, g, color)
}

/// `.dot` (8×8, círculo).
pub fn dot(painter: &Painter, center: Pos2, color: Color32) {
    painter.circle_filled(center, 4.0, color);
}

/// `box-shadow: 0 <y>px <blur>px <color>`.
pub fn shadow(painter: &Painter, rect: Rect, radius: u8, offset_y: i8, blur: u8, color: Color32) {
    let s = egui::Shadow { offset: [0, offset_y], blur, spread: 0, color };
    painter.add(s.as_shape(rect, radius));
}

/// Área clicável com cursor de mão (como `cursor: pointer`).
pub fn clickable(ui: &Ui, rect: Rect, id: Id) -> Response {
    let r = ui.interact(rect, id, Sense::click());
    if r.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    r
}

/// Tooltip no estilo do atributo `title`.
pub fn tip(resp: Response, text: impl Into<String>) -> Response {
    let text = text.into();
    resp.on_hover_ui(|ui| {
        ui.label(egui::RichText::new(text).font(theme::ui(12.0)).color(theme::pal().text));
    })
}

/// O fio de 2 px no meio de uma fresta entre ilhas (vertical ou horizontal, conforme a fresta).
fn splitter_wire(line: Rect) -> Rect {
    if line.width() < line.height() {
        Rect::from_center_size(line.center(), Vec2::new(2.0, line.height()))
    } else {
        Rect::from_center_size(line.center(), Vec2::new(line.width(), 2.0))
    }
}

/// Divisor arrastável entre as ilhas (`line` é a fresta inteira): só aparece ao passar o mouse ou
/// arrastar, como um fio que brilha na cor do agente ativo.
pub fn paint_splitter(painter: &Painter, line: Rect, hot: bool, color: Color32) {
    if !hot {
        return;
    }
    let wire = splitter_wire(line);
    // box-shadow: 0 0 3px <cor>
    for (grow, alpha) in [(2.0, 0.12), (1.0, 0.25)] {
        painter.rect_filled(wire.expand(grow), 1.0, theme::with_opacity(color, alpha));
    }
    painter.rect_filled(wire, 1.0, color);
}

/// Moldura arredondada com borda de 1px.
pub fn frame(painter: &Painter, rect: Rect, radius: f32, fill: Color32, border: Color32) {
    painter.rect(rect, radius, fill, Stroke::new(1.0, border), egui::StrokeKind::Inside);
}

// ---------------------------------------------------------------------------
// Componentes comuns (para as partes 2 e 3 do visual)
// ---------------------------------------------------------------------------

/// Linha divisória de 1 px dentro de uma ilha, de `a` até `b` (horizontal ou vertical).
pub fn separator(painter: &Painter, a: Pos2, b: Pos2) {
    painter.line_segment([a, b], Stroke::new(1.0, theme::pal().separator));
}

/// Etiqueta (branch, contador): fundo `chip`, cantos de 4 px, texto em IBM Plex Mono. `at` é a
/// ponta esquerda, centrada na vertical em `at.y`; devolve o retângulo ocupado.
pub fn chip(painter: &Painter, at: Pos2, text: &str, size: f32, color: Color32) -> Rect {
    let g = galley(painter, text, theme::meta(size), color);
    let rect = Rect::from_min_size(Pos2::new(at.x, at.y - 7.0), Vec2::new(g.size().x + 12.0, 14.0));
    painter.rect_filled(rect, 4.0, theme::pal().chip);
    place(painter, rect, Align2::CENTER_CENTER, g, color);
    rect
}

/// Altura de uma linha de [`section_label`].
pub const SECTION_LABEL_H: f32 = 22.0;

/// Rótulo de seção ("PROJETOS"): IBM Plex Mono Medium 10 px, MAIÚSCULAS, `text_3`, com 0,1 em de
/// espaçamento, 22 px de altura e 10 px de margem. `rect` é a linha (só `min` e a largura contam);
/// devolve o retângulo da linha.
pub fn section_label(painter: &Painter, rect: Rect, text: &str) -> Rect {
    let row = Rect::from_min_size(rect.min, Vec2::new(rect.width(), SECTION_LABEL_H));
    let color = theme::pal().text_3;
    let g = galley_spaced(painter, &text.to_uppercase(), theme::label(10.0), color, 10.0 * theme::LABEL_SPACING_EM);
    place(painter, Rect::from_min_max(Pos2::new(row.min.x + 10.0, row.min.y), row.max), Align2::LEFT_CENTER, g, color);
    row
}

/// Controle segmentado ("Abas | Dividida | Destaque"): trilho `recess` (cantos 7), segmentos de
/// cantos 5. O ativo leva a cor de destaque com texto `on_accent` (`accent = true`) ou o fundo
/// `selected` com texto `text_strong` (`accent = false`). Devolve o índice clicado, se houve.
#[allow(dead_code)] // Para a parte 2 (barra lateral e centro).
pub fn segmented(ui: &Ui, rect: Rect, id: Id, items: &[&str], selected: usize, accent: bool) -> Option<usize> {
    let painter = ui.painter();
    let p = theme::pal();
    painter.rect_filled(rect, 7.0, p.recess);
    let inner = rect.shrink(2.0);
    let w = inner.width() / items.len().max(1) as f32;
    let mut clicked = None;
    for (i, label) in items.iter().enumerate() {
        let seg = Rect::from_min_size(Pos2::new(inner.min.x + w * i as f32, inner.min.y), Vec2::new(w, inner.height()));
        let r = clickable(ui, seg, id.with(i));
        let on = i == selected;
        let (fill, fg, font) = if on && accent {
            (Some(theme::accent()), p.on_accent, theme::ui_semibold(11.5))
        } else if on {
            (Some(p.selected), p.text_strong, theme::ui_semibold(11.5))
        } else {
            (r.hovered().then_some(p.hover), p.text_2, theme::ui(11.5))
        };
        if let Some(fill) = fill {
            painter.rect_filled(seg, 5.0, fill);
        }
        text_in(painter, seg, Align2::CENTER_CENTER, label, font, fg);
        if r.clicked() {
            clicked = Some(i);
        }
    }
    clicked
}

/// O jeito de um botão de texto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Para as partes 2 e 3.
pub enum ButtonKind {
    /// Cor de destaque, texto `on_accent`, 600.
    Primary,
    /// Borda branca 14%, fundo branco 4%.
    Secondary,
    /// Só o texto (`text_2`); o fundo `hover` aparece ao passar o mouse.
    Ghost,
}

/// Botão de texto (cantos 5). Devolve a resposta; quem chama trata o clique.
#[allow(dead_code)] // Para as partes 2 e 3.
pub fn button(ui: &Ui, rect: Rect, id: Id, label: &str, kind: ButtonKind) -> Response {
    let r = clickable(ui, rect, id);
    let painter = ui.painter();
    let p = theme::pal();
    let hot = r.hovered();
    match kind {
        ButtonKind::Primary => {
            let fill = if hot { theme::brighten(theme::accent(), 1.1) } else { theme::accent() };
            painter.rect_filled(rect, 5.0, fill);
            text_in(painter, rect, Align2::CENTER_CENTER, label, theme::ui_semibold(11.5), p.on_accent);
        }
        ButtonKind::Secondary => {
            let fill = theme::with_alpha(p.text_strong, if hot { 0.08 } else { 0.04 });
            frame(painter, rect, 5.0, fill, theme::with_alpha(p.text_strong, 0.14));
            text_in(painter, rect, Align2::CENTER_CENTER, label, theme::ui(11.5), p.text);
        }
        ButtonKind::Ghost => {
            if hot {
                painter.rect_filled(rect, 5.0, p.hover);
            }
            text_in(painter, rect, Align2::CENTER_CENTER, label, theme::ui(11.5), if hot { p.text } else { p.text_2 });
        }
    }
    r
}

/// Largura de um botão de texto com `label` (10 px de cada lado).
#[allow(dead_code)] // Para as partes 2 e 3.
pub fn button_width(painter: &Painter, label: &str, kind: ButtonKind) -> f32 {
    let font = if kind == ButtonKind::Primary { theme::ui_semibold(11.5) } else { theme::ui(11.5) };
    text_w(painter, label, font) + 20.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_right_panel_keeps_room_for_the_terminal() {
        // Janela larga: a largura salva vale (respeitando o mínimo e o máximo).
        assert_eq!(right_width(380.0, 1280.0, 221.0), (380.0, 640.0));
        assert_eq!(right_width(100.0, 1280.0, 221.0).0, theme::RIGHT_MIN_W);
        assert_eq!(right_width(900.0, 1280.0, 221.0).0, 640.0);
        // Janela estreita: o terminal fica com pelo menos TERM_MIN_W, e a barra com pelo menos 260.
        let (w, max) = right_width(380.0, 820.0, 221.0);
        assert_eq!((w, max), (260.0, 260.0));
        assert!(820.0 - 221.0 - w >= 300.0);
        let (w, _) = right_width(380.0, 1000.0, 0.0);
        assert_eq!(w, 380.0);
    }

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
        Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x1, y1))
    }

    fn area_of(rects: &[Rect]) -> f32 {
        rects.iter().map(|r| r.width() * r.height()).sum()
    }

    #[test]
    fn the_free_space_is_the_area_minus_the_islands() {
        let area = rect(0.0, 0.0, 100.0, 50.0);
        let islands = [rect(6.0, 0.0, 40.0, 44.0), rect(46.0, 0.0, 94.0, 44.0)];
        let free = free_rects(area, &islands);
        assert_eq!(area_of(&free), 100.0 * 50.0 - 34.0 * 44.0 - 48.0 * 44.0);
        // Nada sobra dentro de uma ilha.
        for f in &free {
            assert!(islands.iter().all(|i| i.intersect(*f).width() <= 0.0 || i.intersect(*f).height() <= 0.0));
        }
        assert_eq!(free_rects(area, &[]), vec![area]);
        assert!(free_rects(area, &[area]).is_empty());
    }

    #[test]
    fn the_splitter_wire_runs_along_the_gap() {
        let vertical = splitter_wire(rect(100.0, 0.0, 106.0, 400.0));
        assert_eq!((vertical.width(), vertical.height()), (2.0, 400.0));
        assert_eq!(vertical.center(), Pos2::new(103.0, 200.0));
        let horizontal = splitter_wire(rect(0.0, 50.0, 300.0, 56.0));
        assert_eq!((horizontal.width(), horizontal.height()), (300.0, 2.0));
    }
}
