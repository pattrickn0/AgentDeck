//! Aba "Tarefas" da barra direita: a lista agrupada por estado, o detalhe de uma tarefa (pedido,
//! relatório, alterações, histórico e os botões de cada estado) e o formulário de nova tarefa. O
//! estado e as chamadas ao core ficam em `tasks_ui.rs`; aqui só se pinta e se *coleta* o que foi
//! clicado ([`Act`]), aplicado no fim do quadro com o `Deck` inteiro de volta.

use egui::{Align2, Area, Color32, Id, Key, Margin, Modifiers, Order, Pos2, Rect, Sense, Stroke, Ui, UiBuilder, Vec2};

use commanddeck_core::{models, Task, TaskStatus};

use super::{clickable, frame, galley, galley_trunc, place, shadow, text_in, text_w, tip};
use crate::deck::Deck;
use crate::icons::Icon;
use crate::tasks_ui::{
    self, actions_for, can_show_diff, clip_text, events_newest_first, group_tasks, parse_diff_stat, status_age, status_name, Confirm,
    FormMenu, Group, Section, TaskAct, TasksView,
};
use crate::theme;
use crate::util;

const HEAD_H: f32 = 34.0;
const ROW_H: f32 = 22.0;
const BTN_H: f32 = 28.0;
const CHIP_H: f32 = 21.0;
const PAD: f32 = 12.0;
/// Quantos caracteres do pedido e do relatório vão para a tela.
const TEXT_MAX: usize = 20_000;
/// Altura máxima da área rolável do relatório.
const REPORT_MAX_H: f32 = 240.0;
/// Linhas do `git diff --stat` mostradas no detalhe.
const STAT_MAX_LINES: usize = 40;
/// Eventos do histórico mostrados no detalhe.
const EVENTS_MAX: usize = 50;

/// O que foi clicado neste quadro.
enum Act {
    Refresh,
    New,
    Back,
    Open(String),
    ToggleGroup(Group),
    ToggleSection(Section),
    Task(String, TaskAct),
    /// "Ver diff completo" / "Ver o que foi mesclado": abre o diff no centro.
    OpenDiff(String),
    /// "Abrir ao centro" no pedido (`false`) ou no relatório (`true`).
    OpenText(String, bool),
    DismissNotice,
    ConfirmYes,
    ConfirmNo,
    SendReturn(String),
    CancelReturn,
    ToggleMenu(FormMenu),
    CloseMenu,
    SetKind(String),
    SetModel(Option<String>),
    /// Passo do menu de modelo: o provedor cujos modelos estão à mostra (`None` = lista de provedores).
    SetProvider(Option<String>),
    ToggleStartNow,
    PickAttachments,
    RemoveAttachment(usize),
    Submit,
}

pub fn draw(deck: &mut Deck, ui: &mut Ui, rect: Rect) {
    ui.painter().rect_filled(Rect::from_min_max(Pos2::new(rect.min.x, rect.max.y - 1.0), rect.max), 0.0, theme::BORDER);
    let rect = Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.max.y - 1.0));
    let mut acts = Vec::new();
    let now = commanddeck_core::util::now_ms();
    match deck.tasks.view.clone() {
        TasksView::List => list_view(deck, ui, rect, now, &mut acts),
        TasksView::Detail(id) => match deck.tasks.task(&id).cloned() {
            Some(task) => detail_view(deck, ui, rect, &task, now, &mut acts),
            None => list_view(deck, ui, rect, now, &mut acts),
        },
        TasksView::New => form_view(deck, ui, rect, &mut acts),
    }
    confirm_dialog(deck, ui.ctx(), rect, &mut acts);
    // Os tempos ("há 5 min") andam sozinhos.
    ui.ctx().request_repaint_after(std::time::Duration::from_secs(20));
    for act in acts {
        apply(deck, act);
    }
}

fn apply(deck: &mut Deck, act: Act) {
    match act {
        Act::Refresh => deck.tasks_refresh(),
        Act::New => deck.tasks_open_new(),
        Act::Back => deck.tasks_back(),
        Act::Open(id) => deck.tasks_open_detail(&id),
        Act::ToggleGroup(g) => {
            if !deck.tasks.collapsed.remove(&g) {
                deck.tasks.collapsed.insert(g);
            }
        }
        Act::ToggleSection(s) => {
            if !deck.tasks.closed_sections.remove(&s) {
                deck.tasks.closed_sections.insert(s);
            }
        }
        Act::Task(id, act) => deck.tasks_act(&id, act),
        Act::OpenDiff(id) => deck.docs_open_task_diff(&id),
        Act::OpenText(id, report) => deck.docs_open_task_text(&id, report),
        Act::DismissNotice => deck.tasks.notice = None,
        Act::ConfirmYes => deck.tasks_confirm_accepted(),
        Act::ConfirmNo => deck.tasks.confirm = None,
        Act::SendReturn(id) => deck.tasks_send_return(&id),
        Act::CancelReturn => {
            deck.tasks.return_open = false;
            deck.tasks.feedback.clear();
        }
        Act::ToggleMenu(m) => {
            let open = deck.tasks.form.menu != Some(m);
            deck.tasks.form.menu = open.then_some(m);
            if open && m == FormMenu::Model {
                // O menu abre no provedor do modelo escolhido; sem modelo, na lista de provedores.
                deck.tasks.form.provider = deck.tasks.form.model.as_deref().and_then(models::provider).map(str::to_owned);
            }
        }
        Act::CloseMenu => {
            deck.tasks.form.menu = None;
            deck.tasks.form.provider = None;
        }
        Act::SetKind(k) => deck.tasks_form_set_kind(k),
        Act::SetProvider(p) => deck.tasks.form.provider = p,
        Act::SetModel(m) => {
            deck.tasks.form.provider = None;
            deck.tasks.form.model = m;
            deck.tasks.form.menu = None;
        }
        Act::ToggleStartNow => deck.tasks.form.start_now = !deck.tasks.form.start_now,
        Act::PickAttachments => deck.tasks_pick_attachments(),
        Act::RemoveAttachment(i) => deck.tasks_remove_attachment(i),
        Act::Submit => deck.tasks_submit_form(),
    }
}

// ---------------------------------------------------------------------------
// Peças comuns
// ---------------------------------------------------------------------------

/// Cor do ponto de estado: o agente trabalhando (pulsa), o âmbar de "precisa de você" na revisão.
fn status_color(deck: &Deck, task: &Task) -> Color32 {
    match task.status {
        TaskStatus::Draft => theme::MUTED,
        TaskStatus::Running => deck.agents.tab_color(&task.kind),
        TaskStatus::Review => theme::OLLAMA,
        TaskStatus::Validating => theme::SEND_DEFAULT,
        TaskStatus::Merged => theme::BADGE_JSON,
        TaskStatus::Discarded => theme::with_opacity(theme::MUTED, 0.6),
        TaskStatus::Failed => theme::RED,
    }
}

/// Ponto de estado; a tarefa em andamento ganha o halo pulsante dos agentes trabalhando.
fn status_dot(ui: &Ui, center: Pos2, color: Color32, pulse: bool) {
    let p = ui.painter();
    if pulse {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
        let wave = (ui.ctx().input(|i| i.time) * 4.8).sin() as f32 * 0.5 + 0.5;
        p.circle_filled(center, 6.0 + 2.5 * wave, theme::with_opacity(color, 0.30 * (1.0 - wave)));
    }
    super::dot(p, center, color);
}

/// Faixa do topo: fundo, borda inferior, "←" (opcional) e o título. `right`: espaço reservado à direita.
fn head_bar(ui: &Ui, rect: Rect, back: Option<&str>, title: &str, right: f32, acts: &mut Vec<Act>) -> Rect {
    let head = Rect::from_min_size(rect.min, Vec2::new(rect.width(), HEAD_H));
    let p = ui.painter_at(head);
    p.rect_filled(head, 0.0, theme::PANEL);
    p.rect_filled(Rect::from_min_max(Pos2::new(head.min.x, head.max.y - 1.0), head.max), 0.0, theme::BORDER);
    let mut x = head.min.x + PAD;
    if let Some(tooltip) = back {
        let b = Rect::from_min_size(Pos2::new(head.min.x + 6.0, head.min.y + 5.0), Vec2::new(26.0, 22.0));
        let r = clickable(ui, b, Id::new("tasks-back"));
        if r.hovered() {
            p.rect_filled(b, 5.0, theme::BORDER);
        }
        text_in(&p, b, Align2::CENTER_CENTER, "←", theme::ui(15.0), theme::TEXT);
        if tip(r, tooltip).clicked() {
            acts.push(Act::Back);
        }
        x = b.max.x + 6.0;
    }
    let g = galley_trunc(&p, title, theme::ui_semibold(13.0), theme::TEXT, (head.max.x - right - x).max(20.0));
    place(
        &p,
        Rect::from_min_max(Pos2::new(x, head.min.y), Pos2::new(head.max.x - right, head.max.y - 1.0)),
        Align2::LEFT_CENTER,
        g,
        theme::TEXT,
    );
    head
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BtnKind {
    Primary,
    Secondary,
    Danger,
}

fn btn_width(painter: &egui::Painter, label: &str) -> f32 {
    galley(painter, label, theme::ui_semibold(12.0), theme::TEXT).size().x + 26.0
}

/// Botão com texto. Desligado, fica apagado e não reage. Devolve se foi clicado.
fn button(ui: &Ui, rect: Rect, id: Id, label: &str, kind: BtnKind, enabled: bool, tooltip: &str) -> bool {
    let r = if enabled { clickable(ui, rect, id) } else { ui.interact(rect, id, Sense::hover()) };
    let hot = enabled && r.hovered();
    let (bg, fg) = if !enabled {
        (theme::PANEL_2, theme::SEND_IDLE_TEXT)
    } else {
        match kind {
            BtnKind::Primary => (if hot { theme::brighten(theme::ACCENT, 1.15) } else { theme::ACCENT }, theme::WHITE),
            BtnKind::Secondary => (if hot { theme::BORDER } else { theme::PANEL_2 }, theme::TEXT),
            BtnKind::Danger => (if hot { theme::BORDER } else { theme::PANEL_2 }, theme::RED_TEXT),
        }
    };
    ui.painter().rect_filled(rect, 7.0, bg);
    let g = galley_trunc(ui.painter(), label, theme::ui_semibold(12.0), fg, rect.width() - 14.0);
    place(ui.painter(), rect, Align2::CENTER_CENTER, g, fg);
    let r = if tooltip.is_empty() { r } else { tip(r, tooltip) };
    enabled && r.clicked()
}

/// Botões em linha que quebram para a linha de baixo quando não cabem. Devolve o índice do clicado.
fn button_flow(ui: &mut Ui, buttons: &[(String, BtnKind, bool, &str)], id_salt: &str) -> Option<usize> {
    let avail = ui.available_width();
    let max_w = (avail - 2.0 * PAD).max(60.0);
    let widths: Vec<f32> = buttons.iter().map(|(l, ..)| btn_width(ui.painter(), l).min(max_w)).collect();
    let mut clicked = None;
    let mut i = 0;
    while i < buttons.len() {
        // Quantos cabem nesta linha.
        let mut end = i;
        let mut w = 0.0;
        while end < buttons.len() {
            let next = w + widths[end] + if end > i { 6.0 } else { 0.0 };
            if end > i && next > max_w {
                break;
            }
            w = next;
            end += 1;
        }
        let (row, _) = ui.allocate_exact_size(Vec2::new(avail, BTN_H + 6.0), Sense::hover());
        let mut x = row.min.x + PAD;
        for (j, (label, kind, enabled, tooltip)) in buttons.iter().enumerate().take(end).skip(i) {
            let r = Rect::from_min_size(Pos2::new(x, row.min.y), Vec2::new(widths[j], BTN_H));
            x = r.max.x + 6.0;
            if button(ui, r, Id::new(("tasks-btn", id_salt, j)), label, *kind, *enabled, tooltip) {
                clicked = Some(j);
            }
        }
        i = end;
    }
    clicked
}

/// Parágrafo com quebra de linha, recuado `PAD` dos dois lados. Devolve o retângulo ocupado.
fn paragraph(ui: &mut Ui, text: &str, font: egui::FontId, color: Color32) -> Rect {
    let w = (ui.available_width() - 2.0 * PAD).max(40.0);
    let g = ui.painter().layout(text.to_owned(), font, color, w);
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), g.size().y + 6.0), Sense::hover());
    ui.painter().galley(Pos2::new(r.min.x + PAD, r.min.y + 2.0), g, color);
    r
}

/// Mensagem curta e muda (estados vazios dentro de uma seção).
fn muted_line(ui: &mut Ui, text: &str) {
    paragraph(ui, text, theme::ui(12.0), theme::MUTED);
}

/// Faixa vermelha com o erro (e o "×" opcional). Devolve se o "×" foi clicado.
fn error_box(ui: &mut Ui, text: &str, dismiss: bool) -> bool {
    let w = ui.available_width() - 2.0 * PAD;
    let g = ui.painter().layout(text.to_owned(), theme::ui(12.0), theme::RED_TEXT, (w - 22.0 - if dismiss { 18.0 } else { 0.0 }).max(40.0));
    let h = g.size().y + 16.0;
    let (outer, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h + 12.0), Sense::hover());
    let r = Rect::from_min_size(Pos2::new(outer.min.x + PAD, outer.min.y + 3.0), Vec2::new(w, h));
    ui.painter().rect_filled(r, 7.0, theme::rgba(229, 72, 77, 0.12));
    ui.painter().rect_stroke(r, 7.0, Stroke::new(1.0, theme::rgba(229, 72, 77, 0.45)), egui::StrokeKind::Inside);
    ui.painter().galley(Pos2::new(r.min.x + 10.0, r.min.y + 8.0), g, theme::RED_TEXT);
    let mut closed = false;
    if dismiss {
        let x = Rect::from_min_size(Pos2::new(r.max.x - 24.0, r.min.y + 2.0), Vec2::splat(22.0));
        let xr = clickable(ui, x, Id::new(("tasks-error-x", text.len())));
        text_in(ui.painter(), x, Align2::CENTER_CENTER, "×", theme::ui(15.0), if xr.hovered() { theme::TEXT } else { theme::MUTED });
        closed = tip(xr, "Fechar o aviso").clicked();
    }
    closed
}

/// Campo de texto numa moldura de 1 px (a borda clareia com o foco); a moldura cresce com o texto.
fn text_field(ui: &mut Ui, id: Id, text: &mut String, hint: &str, rows: Option<usize>) -> egui::Response {
    let focused = ui.ctx().memory(|m| m.has_focus(id));
    egui::Frame::new()
        .fill(theme::PANEL)
        .stroke(Stroke::new(1.0, if focused { theme::MUTED } else { theme::BORDER }))
        .corner_radius(8)
        .inner_margin(Margin { left: 9, right: 9, top: 6, bottom: 6 })
        .show(ui, |ui| {
            let te = match rows {
                Some(n) => egui::TextEdit::multiline(text).desired_rows(n),
                None => egui::TextEdit::singleline(text),
            };
            te.id(id)
                .frame(egui::Frame::NONE)
                .font(theme::ui(12.5))
                .text_color(theme::INPUT_TEXT)
                .hint_text(egui::RichText::new(hint).font(theme::ui(12.5)).color(theme::PLACEHOLDER))
                .desired_width(ui.available_width())
                .show(ui);
        })
        .response
}

/// Executa `f` numa coluna recuada `PAD` dos dois lados e avança o cursor do `ui` pelo que ela ocupou.
fn padded<R>(ui: &mut Ui, f: impl FnOnce(&mut Ui) -> R) -> R {
    let avail = ui.available_rect_before_wrap();
    let inner = Rect::from_min_max(Pos2::new(avail.min.x + PAD, avail.min.y), Pos2::new(avail.max.x - PAD, avail.max.y));
    let mut child = ui.new_child(UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
    child.spacing_mut().item_spacing = Vec2::ZERO;
    let out = f(&mut child);
    let used = child.min_rect();
    ui.allocate_rect(Rect::from_min_max(Pos2::new(avail.min.x, used.min.y), Pos2::new(avail.max.x, used.max.y)), Sense::hover());
    out
}

/// Cabeçalho de seção recolhível: seta, título em maiúsculas e um detalhe à direita.
fn section_header(
    deck: &mut Deck,
    ui: &mut Ui,
    section: Section,
    title: &str,
    right: Option<(String, Color32)>,
    acts: &mut Vec<Act>,
) -> bool {
    let open = !deck.tasks.closed_sections.contains(&section);
    let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H + 4.0), Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    deck.icons.paint(
        ui.painter(),
        Icon::TreeChevron,
        Pos2::new(r.min.x + PAD, r.center().y),
        10.0,
        theme::TREE_ICON,
        if open { std::f32::consts::FRAC_PI_2 } else { 0.0 },
    );
    let t = text_in(
        ui.painter(),
        Rect::from_min_max(Pos2::new(r.min.x + 22.0, r.min.y), r.max),
        Align2::LEFT_CENTER,
        &title.to_uppercase(),
        theme::ui_bold(11.0),
        theme::MUTED,
    );
    if let Some((text, color)) = right {
        let left = t.max.x + 10.0;
        let g = galley_trunc(ui.painter(), &text, theme::ui(11.5), color, (r.max.x - PAD - left).max(10.0));
        place(
            ui.painter(),
            Rect::from_min_max(Pos2::new(left, r.min.y), Pos2::new(r.max.x - PAD, r.max.y)),
            Align2::RIGHT_CENTER,
            g,
            color,
        );
    }
    if resp.clicked() {
        acts.push(Act::ToggleSection(section));
    }
    open
}

// ---------------------------------------------------------------------------
// Lista
// ---------------------------------------------------------------------------

fn list_view(deck: &mut Deck, ui: &mut Ui, rect: Rect, now: u64, acts: &mut Vec<Act>) {
    // Cabeçalho: título, atualizar e "+ Nova".
    let nova_w = btn_width(ui.painter(), "+ Nova") - 4.0;
    let head = head_bar(ui, rect, None, "Tarefas", nova_w + 8.0 + 28.0 + 8.0, acts);
    let nova = Rect::from_min_size(Pos2::new(head.max.x - 8.0 - nova_w, head.min.y + 5.0), Vec2::new(nova_w, HEAD_H - 11.0));
    if button(ui, nova, Id::new("tasks-new"), "+ Nova", BtnKind::Primary, true, "Criar uma tarefa para um agente") {
        acts.push(Act::New);
    }
    let refresh = Rect::from_min_size(Pos2::new(nova.min.x - 4.0 - 24.0, head.min.y + 5.0), Vec2::splat(24.0));
    let rr = clickable(ui, refresh, Id::new("tasks-refresh"));
    if rr.hovered() {
        ui.painter().rect_filled(refresh, 6.0, theme::BORDER);
    }
    deck.icons.paint(ui.painter(), Icon::Refresh, refresh.center(), 15.0, if rr.hovered() { theme::TEXT } else { theme::MUTED }, 0.0);
    if tip(rr, "Reler as tarefas").clicked() {
        acts.push(Act::Refresh);
    }

    let body = Rect::from_min_max(Pos2::new(rect.min.x, head.max.y), rect.max);
    if deck.tasks.tasks.is_empty() {
        empty_state(ui, body, deck.tasks.loaded, acts);
        return;
    }
    let tasks = deck.tasks.tasks.clone();
    let groups = group_tasks(&tasks);
    let mut child = ui.new_child(UiBuilder::new().max_rect(body).layout(egui::Layout::top_down(egui::Align::Min)));
    child.set_clip_rect(body);
    egui::ScrollArea::vertical().id_salt("tasks-list").auto_shrink([false, false]).show(&mut child, |ui| {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        ui.add_space(4.0);
        for (group, list) in &groups {
            let collapsed = deck.tasks.collapsed.contains(group);
            group_header(deck, ui, *group, list.len(), collapsed, acts);
            if collapsed {
                continue;
            }
            for task in list {
                card(deck, ui, task, now, acts);
            }
        }
        ui.add_space(10.0);
    });
}

fn empty_state(ui: &mut Ui, body: Rect, loaded: bool, acts: &mut Vec<Act>) {
    let painter = ui.painter_at(body);
    let w = (body.width() - 2.0 * PAD - 8.0).max(80.0);
    let mut y = body.min.y + 28.0;
    let x = body.min.x + PAD + 4.0;
    if !loaded {
        text_in(
            &painter,
            Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, 20.0)),
            Align2::LEFT_CENTER,
            "Lendo as tarefas…",
            theme::ui(12.5),
            theme::MUTED,
        );
        return;
    }
    let title = painter.layout("Nenhuma tarefa ainda".to_owned(), theme::ui_semibold(14.0), theme::TEXT, w);
    painter.galley(Pos2::new(x, y), title.clone(), theme::TEXT);
    y += title.size().y + 8.0;
    let text = "Uma tarefa é um pedido que um agente faz numa cópia isolada do projeto (uma branch com a própria pasta). \
Quando ele termina, você revisa o relatório e o diff, mescla, testa e valida. Se algo falhar, devolve ou reprova com o motivo.";
    let g = painter.layout(text.to_owned(), theme::ui(12.5), theme::MUTED, w);
    painter.galley(Pos2::new(x, y), g.clone(), theme::MUTED);
    y += g.size().y + 14.0;
    let label = "+ Nova tarefa";
    let bw = btn_width(&painter, label);
    let b = Rect::from_min_size(Pos2::new(x, y), Vec2::new(bw, BTN_H));
    if button(ui, b, Id::new("tasks-empty-new"), label, BtnKind::Primary, true, "Criar uma tarefa para um agente") {
        acts.push(Act::New);
    }
}

fn group_header(deck: &mut Deck, ui: &mut Ui, group: Group, count: usize, collapsed: bool, acts: &mut Vec<Act>) {
    let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H + 2.0), Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    deck.icons.paint(
        ui.painter(),
        Icon::TreeChevron,
        Pos2::new(r.min.x + PAD, r.center().y),
        10.0,
        theme::TREE_ICON,
        if collapsed { 0.0 } else { std::f32::consts::FRAC_PI_2 },
    );
    text_in(
        ui.painter(),
        Rect::from_min_max(Pos2::new(r.min.x + 22.0, r.min.y), r.max),
        Align2::LEFT_CENTER,
        &format!("{}  {count}", group.title().to_uppercase()),
        theme::ui_bold(11.0),
        theme::MUTED,
    );
    if tip(resp, if collapsed { "Mostrar" } else { "Recolher" }).clicked() {
        acts.push(Act::ToggleGroup(group));
    }
}

/// Cartão de uma tarefa: ponto de estado, `tN` e título; agente, modelo e tempo; e, na revisão, o resumo do diff.
fn card(deck: &mut Deck, ui: &mut Ui, task: &Task, now: u64, acts: &mut Vec<Act>) {
    let summary = (task.status == TaskStatus::Review).then(|| task.diff_stat.as_deref().and_then(parse_diff_stat)).flatten();
    let h = 10.0 + 17.0 + 16.0 + if summary.is_some() { 16.0 } else { 0.0 } + 6.0;
    let (outer, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::click());
    let r = outer.shrink2(Vec2::new(6.0, 1.0));
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        ui.painter().rect_filled(r, 6.0, theme::PANEL_2);
    }
    let painter = ui.painter().clone();
    let x0 = r.min.x + 28.0;
    let right = r.max.x - 8.0;
    // Ponto de estado e primeira linha: `tN` apagado e o título.
    let l1 = r.min.y + 9.0 + 8.5;
    status_dot(ui, Pos2::new(r.min.x + 14.0, l1), status_color(deck, task), task.status == TaskStatus::Running);
    let id_g = galley(&painter, &task.id, theme::mono(11.0), theme::MUTED);
    let id_w = id_g.size().x;
    place(&painter, Rect::from_min_size(Pos2::new(x0, l1 - 7.5), Vec2::new(id_w, 17.0)), Align2::LEFT_CENTER, id_g, theme::MUTED);
    let title_g = galley_trunc(&painter, &task.title, theme::ui(12.5), theme::TEXT, (right - x0 - id_w - 6.0).max(20.0));
    place(
        &painter,
        Rect::from_min_max(Pos2::new(x0 + id_w + 6.0, l1 - 8.5), Pos2::new(right, l1 + 8.5)),
        Align2::LEFT_CENTER,
        title_g,
        theme::TEXT,
    );
    // Segunda linha: logo + agente · modelo · tempo.
    let l2 = l1 + 8.5 + 4.0 + 8.0;
    let mut tx = x0;
    if let Some(icon) = super::projects::agent_icon(&task.kind) {
        deck.icons.paint(&painter, icon, Pos2::new(x0 + 5.5, l2), 11.0, deck.agents.tab_color(&task.kind), 0.0);
        tx += 17.0;
    } else {
        super::dot(&painter, Pos2::new(x0 + 5.5, l2), deck.agents.tab_color(&task.kind));
        tx += 17.0;
    }
    let mut meta = deck.agents.name(&task.kind);
    if let Some(m) = tasks_ui::model_text(task) {
        meta.push_str(" · ");
        meta.push_str(m);
    }
    meta.push_str(" · ");
    meta.push_str(&status_age(task, now));
    let g = galley_trunc(&painter, &meta, theme::ui(11.5), theme::MUTED, (right - tx).max(20.0));
    place(&painter, Rect::from_min_max(Pos2::new(tx, l2 - 8.0), Pos2::new(right, l2 + 8.0)), Align2::LEFT_CENTER, g, theme::MUTED);
    // Revisão: o resumo do diff (arquivos, linhas novas e removidas).
    if let Some(sum) = summary {
        let l3 = l2 + 16.0;
        let mut job = egui::text::LayoutJob::default();
        let fmt = |color| egui::TextFormat { font_id: theme::ui(11.5), color, ..Default::default() };
        job.append(&sum.files_text(), 0.0, fmt(theme::TREE_TEXT));
        if sum.insertions > 0 {
            job.append(&format!("  +{}", sum.insertions), 0.0, fmt(theme::BADGE_JSON));
        }
        if sum.deletions > 0 {
            job.append(&format!("  −{}", sum.deletions), 0.0, fmt(theme::RED_TEXT));
        }
        job.wrap.max_width = (right - x0).max(20.0);
        job.wrap.max_rows = 1;
        job.wrap.overflow_character = Some('…');
        let g = painter.layout_job(job);
        place(&painter, Rect::from_min_max(Pos2::new(x0, l3 - 8.0), Pos2::new(right, l3 + 8.0)), Align2::LEFT_CENTER, g, theme::TREE_TEXT);
    }
    let tooltip = format!("{} · {}\n{} → {}", task.id, task.title, task.branch, task.base);
    if tip(resp, tooltip).clicked() {
        acts.push(Act::Open(task.id.clone()));
    }
}

// ---------------------------------------------------------------------------
// Detalhe
// ---------------------------------------------------------------------------

fn detail_view(deck: &mut Deck, ui: &mut Ui, rect: Rect, task: &Task, now: u64, acts: &mut Vec<Act>) {
    let head = head_bar(ui, rect, Some("Voltar à lista de tarefas"), &format!("Tarefa {}", task.id), 0.0, acts);
    let busy = deck.tasks.busy.clone();
    let working = busy.is_some();
    let busy_here = busy.as_ref().filter(|b| b.id == task.id).map(|b| b.label);
    if busy_here.is_some() {
        super::git::busy_bar(ui, Rect::from_min_max(Pos2::new(rect.min.x, head.max.y - 2.0), Pos2::new(rect.max.x, head.max.y)));
    }
    let tab_open = deck.tasks_tab_open(task);
    let acts_for = actions_for(task, tab_open);
    let color = status_color(deck, task);
    let agent_color = deck.agents.tab_color(&task.kind);
    let agent_name = deck.agents.name(&task.kind);
    let notice = deck.tasks.notice.clone();
    let return_open = deck.tasks.return_open && matches!(task.status, TaskStatus::Review | TaskStatus::Validating);
    let body = Rect::from_min_max(Pos2::new(rect.min.x, head.max.y), rect.max);
    let mut child = ui.new_child(UiBuilder::new().max_rect(body).layout(egui::Layout::top_down(egui::Align::Min)));
    child.set_clip_rect(body);
    let id_salt = format!("tasks-detail-{}", task.id);
    egui::ScrollArea::vertical().id_salt(id_salt).auto_shrink([false, false]).show(&mut child, |ui| {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        ui.add_space(10.0);
        // Título.
        let w = ui.available_width() - 2.0 * PAD;
        let g = ui.painter().layout(task.title.clone(), theme::ui_semibold(14.5), theme::TEXT, w);
        let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), g.size().y + 8.0), Sense::hover());
        ui.painter().galley(Pos2::new(r.min.x + PAD, r.min.y), g, theme::TEXT);

        // Chip do estado e o tempo.
        let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::hover());
        let name = status_name(task.status);
        let cw = galley(ui.painter(), name, theme::ui_semibold(11.5), color).size().x + 26.0;
        let chip = Rect::from_min_size(Pos2::new(r.min.x + PAD, r.min.y), Vec2::new(cw, 20.0));
        ui.painter().rect_filled(chip, 10.0, theme::with_opacity(color, 0.16));
        status_dot(ui, Pos2::new(chip.min.x + 11.0, chip.center().y), color, task.status == TaskStatus::Running);
        text_in(
            ui.painter(),
            Rect::from_min_max(Pos2::new(chip.min.x + 20.0, chip.min.y), chip.max),
            Align2::LEFT_CENTER,
            name,
            theme::ui_semibold(11.5),
            color,
        );
        let when = status_age(task, now);
        let g = galley_trunc(ui.painter(), &when, theme::ui(11.5), theme::MUTED, (r.max.x - PAD - chip.max.x - 10.0).max(10.0));
        place(
            ui.painter(),
            Rect::from_min_max(Pos2::new(chip.max.x + 10.0, r.min.y), Pos2::new(r.max.x - PAD, r.max.y - 2.0)),
            Align2::LEFT_CENTER,
            g,
            theme::MUTED,
        );
        ui.add_space(8.0);

        // `branch → base` e o agente com o modelo.
        padded(ui, |ui| {
            let line = tasks_ui::branch_line(task);
            let g = ui.painter().layout(line, theme::mono(11.5), theme::SELECT_TEXT, ui.available_width() - 20.0);
            let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), g.size().y + 4.0), Sense::hover());
            deck.icons.paint(ui.painter(), Icon::GitBranch, r.min + Vec2::new(6.0, 8.5), 12.0, theme::MUTED, 0.0);
            ui.painter().galley(Pos2::new(r.min.x + 20.0, r.min.y + 1.0), g, theme::SELECT_TEXT);
            tip(resp, "Branch da tarefa → branch em que ela será mesclada (e o commit da mesclagem, se já houve)");
            let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::hover());
            let mut x = r.min.x;
            if let Some(icon) = super::projects::agent_icon(&task.kind) {
                deck.icons.paint(ui.painter(), icon, Pos2::new(x + 6.0, r.center().y), 12.0, agent_color, 0.0);
                x += 20.0;
            } else {
                super::dot(ui.painter(), Pos2::new(x + 6.0, r.center().y), agent_color);
                x += 20.0;
            }
            let mut who = agent_name.clone();
            if let Some(m) = tasks_ui::model_text(task) {
                who.push_str(" · ");
                who.push_str(m);
            }
            let g = galley_trunc(ui.painter(), &who, theme::ui(12.5), theme::TEXT, (r.max.x - x).max(10.0));
            place(ui.painter(), Rect::from_min_max(Pos2::new(x, r.min.y), r.max), Align2::LEFT_CENTER, g, theme::TEXT);
        });
        ui.add_space(6.0);

        // O erro da tarefa (Failed) e o da última ação.
        if let Some(err) = task.error.as_deref().filter(|_| task.status == TaskStatus::Failed) {
            error_box(ui, err, false);
        }
        if let Some(n) = &notice {
            if error_box(ui, n, true) {
                acts.push(Act::DismissNotice);
            }
        }

        // Botões do estado.
        let buttons: Vec<(String, BtnKind, bool, &str)> = acts_for
            .iter()
            .map(|a| {
                let kind = if a.is_primary() {
                    BtnKind::Primary
                } else if *a == TaskAct::Discard {
                    BtnKind::Danger
                } else {
                    BtnKind::Secondary
                };
                (a.label(task), kind, !working, a.tip())
            })
            .collect();
        if !buttons.is_empty() {
            if let Some(i) = button_flow(ui, &buttons, "detail") {
                acts.push(Act::Task(task.id.clone(), acts_for[i]));
            }
        }
        if let Some(label) = busy_here {
            let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::hover());
            egui::Spinner::new()
                .size(14.0)
                .color(theme::MUTED)
                .paint_at(ui, Rect::from_center_size(Pos2::new(r.min.x + PAD + 7.0, r.center().y), Vec2::splat(14.0)));
            text_in(
                ui.painter(),
                Rect::from_min_max(Pos2::new(r.min.x + PAD + 22.0, r.min.y), r.max),
                Align2::LEFT_CENTER,
                label,
                theme::ui(12.0),
                theme::MUTED,
            );
        }
        if return_open {
            return_form(deck, ui, task, working, acts);
        }
        ui.add_space(4.0);
        separator(ui);

        // Pedido.
        if section_header(deck, ui, Section::Brief, "Pedido", None, acts) {
            if task.brief.trim().is_empty() {
                muted_line(ui, "Sem pedido além do título: o agente recebe só o título.");
            } else {
                paragraph(ui, &clip_text(task.brief.trim(), TEXT_MAX), theme::ui(12.5), theme::TREE_TEXT);
                open_center_button(ui, "brief", task, false, acts);
            }
        }
        // Relatório.
        if section_header(deck, ui, Section::Report, "Relatório", None, acts) {
            match task.report.as_deref().filter(|r| !r.trim().is_empty()) {
                Some(report) => {
                    egui::ScrollArea::vertical()
                        .id_salt(("tasks-report", &task.id))
                        .max_height(REPORT_MAX_H)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            paragraph(ui, &clip_text(report.trim(), TEXT_MAX), theme::ui(12.5), theme::TREE_TEXT);
                        });
                    open_center_button(ui, "report", task, true, acts);
                }
                None if task.status == TaskStatus::Running => muted_line(ui, "O agente ainda não entregou o relatório."),
                None => muted_line(ui, "Sem relatório."),
            }
        }
        // Alterações.
        let summary = task.diff_stat.as_deref().and_then(parse_diff_stat);
        let right = summary.map(|s| (s.text(), theme::MUTED));
        if section_header(deck, ui, Section::Changes, "Alterações", right, acts) {
            match task.diff_stat.as_deref().filter(|s| !s.trim().is_empty()) {
                Some(stat) => stat_box(ui, stat),
                None => muted_line(ui, if can_show_diff(task) { "O resumo chega com o relatório." } else { "Sem alterações registradas." }),
            }
            if can_show_diff(task) {
                let b = [(tasks_ui::diff_button_label(task).to_owned(), BtnKind::Secondary, true, tasks_ui::diff_button_tip(task))];
                if button_flow(ui, &b, "diff").is_some() {
                    acts.push(Act::OpenDiff(task.id.clone()));
                }
            }
        }
        // Histórico.
        let events = events_newest_first(task);
        if section_header(deck, ui, Section::History, "Histórico", Some((events.len().to_string(), theme::MUTED)), acts) {
            history(ui, &events);
        }
        ui.add_space(14.0);
    });
}

/// "Abrir ao centro" embaixo do pedido e do relatório: eles aparecem formatados no modo Documentos.
fn open_center_button(ui: &mut Ui, id_salt: &str, task: &Task, report: bool, acts: &mut Vec<Act>) {
    let tip = if report { "Abre o relatório formatado, com a largura toda do centro" } else { "Abre o pedido formatado, com a largura toda do centro" };
    let b = [("Abrir ao centro".to_owned(), BtnKind::Secondary, true, tip)];
    if button_flow(ui, &b, id_salt).is_some() {
        acts.push(Act::OpenText(task.id.clone(), report));
    }
}

fn separator(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(Rect::from_min_max(Pos2::new(r.min.x + PAD, r.min.y), Pos2::new(r.max.x - PAD, r.max.y)), 0.0, theme::BORDER);
}

/// Corta o começo do texto (`…ui/tasks.rs`) para caber em `max_w`: num caminho, o fim é o que identifica.
fn tail_galley(painter: &egui::Painter, text: &str, font: egui::FontId, color: Color32, max_w: f32) -> std::sync::Arc<egui::Galley> {
    let full = galley(painter, text, font.clone(), color);
    if full.size().x <= max_w {
        return full;
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        let cut: String = std::iter::once('…').chain(chars[mid..].iter().copied()).collect();
        if galley(painter, &cut, font.clone(), color).size().x <= max_w {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    let cut: String = std::iter::once('…').chain(chars[lo..].iter().copied()).collect();
    galley(painter, &cut, font, color)
}

/// O `git diff --stat` do relatório: um arquivo por linha (o caminho, cortado pelo começo, e o número de
/// linhas mudadas à direita). O total fica no cabeçalho da seção.
fn stat_box(ui: &mut Ui, stat: &str) {
    let lines = tasks_ui::parse_stat_lines(stat);
    if lines.is_empty() {
        return;
    }
    let shown = lines.len().min(STAT_MAX_LINES);
    let extra = lines.len() - shown;
    let row = 16.0;
    let h = (shown + usize::from(extra > 0)) as f32 * row + 12.0;
    let (outer, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h + 6.0), Sense::hover());
    let r = Rect::from_min_size(Pos2::new(outer.min.x + PAD, outer.min.y + 2.0), Vec2::new(outer.width() - 2.0 * PAD, h));
    frame(ui.painter(), r, 7.0, theme::PANEL, theme::BORDER);
    for (i, line) in lines.iter().take(shown).enumerate() {
        let y = r.min.y + 6.0 + i as f32 * row;
        let count = galley(ui.painter(), &line.changes, theme::mono(11.0), theme::MUTED);
        let cw = count.size().x;
        place(
            ui.painter(),
            Rect::from_min_size(Pos2::new(r.max.x - 8.0 - cw, y), Vec2::new(cw, row)),
            Align2::LEFT_CENTER,
            count,
            theme::MUTED,
        );
        let g = tail_galley(ui.painter(), &line.path, theme::mono(11.0), theme::TREE_TEXT, (r.width() - 16.0 - cw - 10.0).max(20.0));
        place(
            ui.painter(),
            Rect::from_min_size(Pos2::new(r.min.x + 8.0, y), Vec2::new(r.width() - 16.0 - cw, row)),
            Align2::LEFT_CENTER,
            g,
            theme::TREE_TEXT,
        );
    }
    if extra > 0 {
        text_in(
            ui.painter(),
            Rect::from_min_size(Pos2::new(r.min.x + 8.0, r.min.y + 6.0 + shown as f32 * row), Vec2::new(r.width() - 16.0, row)),
            Align2::LEFT_CENTER,
            &format!("… e mais {extra} arquivos"),
            theme::ui(11.0),
            theme::MUTED,
        );
    }
}

/// Eventos (do mais novo): data e hora, e o texto ao lado.
fn history(ui: &mut Ui, events: &[&commanddeck_core::TaskEvent]) {
    if events.is_empty() {
        muted_line(ui, "Nada registrado ainda.");
        return;
    }
    let time_w = galley(ui.painter(), "00/00 00:00", theme::mono(10.5), theme::MUTED).size().x;
    for e in events.iter().take(EVENTS_MAX) {
        let text_w = (ui.available_width() - 2.0 * PAD - time_w - 8.0).max(40.0);
        let g = ui.painter().layout(clip_text(&e.text, 1_000).into_owned(), theme::ui(12.0), theme::TREE_TEXT, text_w);
        let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), g.size().y + 5.0), Sense::hover());
        let t = galley(ui.painter(), &tasks_ui::event_time(e.at_ms), theme::mono(10.5), theme::MUTED);
        ui.painter().galley(
            Pos2::new(r.min.x + PAD, r.min.y + 2.0 + (g.rows.first().map_or(14.0, |row| row.height()) - t.size().y) / 2.0),
            t,
            theme::MUTED,
        );
        ui.painter().galley(Pos2::new(r.min.x + PAD + time_w + 8.0, r.min.y + 2.0), g, theme::TREE_TEXT);
    }
    if events.len() > EVENTS_MAX {
        muted_line(ui, &format!("… e mais {} eventos mais antigos.", events.len() - EVENTS_MAX));
    }
}

/// Campo de feedback do "Devolver", com "Enviar" e "Cancelar".
fn return_form(deck: &mut Deck, ui: &mut Ui, task: &Task, working: bool, acts: &mut Vec<Act>) {
    // Em revisão devolve-se com ajustes; aguardando validação, reprova-se com o que não funcionou no teste.
    let (hint, send, send_tip, cancel_tip) = if task.status == TaskStatus::Validating {
        (
            "O que não funcionou no teste? (Ctrl+Enter envia)",
            "Reprovar e enviar",
            "Devolve a tarefa ao mesmo agente, dizendo que o trabalho já foi mesclado e o que falhou no teste",
            "Fecha sem reprovar",
        )
    } else {
        (
            "O que o agente precisa mudar? (Ctrl+Enter envia)",
            "Enviar ao agente",
            "Devolve a tarefa ao agente com este pedido de ajustes",
            "Fecha sem devolver",
        )
    };
    let id = Id::new("tasks-return-feedback");
    let focused = ui.ctx().memory(|m| m.has_focus(id));
    let ready = !working && !deck.tasks.feedback.trim().is_empty();
    let ctrl_enter = focused && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter));
    padded(ui, |ui| {
        ui.add_space(2.0);
        text_field(ui, id, &mut deck.tasks.feedback, hint, Some(4));
    });
    let b = [(send.to_owned(), BtnKind::Primary, ready, send_tip), ("Cancelar".to_owned(), BtnKind::Secondary, true, cancel_tip)];
    ui.add_space(6.0);
    match button_flow(ui, &b, "return") {
        Some(0) => acts.push(Act::SendReturn(task.id.clone())),
        Some(_) => acts.push(Act::CancelReturn),
        None if ctrl_enter && ready => acts.push(Act::SendReturn(task.id.clone())),
        None => {}
    }
}

// ---------------------------------------------------------------------------
// Confirmação
// ---------------------------------------------------------------------------

/// Modal de confirmação de "Mesclar" e "Descartar", dentro do painel, sobre o fundo escurecido.
fn confirm_dialog(deck: &mut Deck, ctx: &egui::Context, panel: Rect, acts: &mut Vec<Act>) {
    let Some(confirm) = deck.tasks.confirm.clone() else { return };
    let (id, merge) = match &confirm {
        Confirm::Merge(id) => (id, true),
        Confirm::Discard(id) => (id, false),
    };
    let Some(task) = deck.tasks.task(id).cloned() else {
        deck.tasks.confirm = None;
        return;
    };
    let (title, detail, yes) = if merge {
        (
            format!("Mesclar \"{}\" em {}?", task.title, task.base),
            format!(
                "A branch {} entra em {} com um commit de mesclagem (git merge --no-ff). O agente, a worktree e a branch continuam até você testar e validar a tarefa.",
                task.branch, task.base
            ),
            "Mesclar",
        )
    } else {
        (
            format!("Descartar \"{}\"?", task.title),
            if task.status == TaskStatus::Validating {
                format!(
                    "A aba do agente fecha e a worktree e a branch da tarefa são apagadas. O que já foi mesclado continua em {}: descartar não desfaz a mesclagem.",
                    task.base
                )
            } else {
                "A aba do agente fecha e a worktree e a branch da tarefa são apagadas, com tudo o que o agente fez e não foi mesclado. Não dá para desfazer.".to_owned()
            },
            "Descartar",
        )
    };
    super::modal_backdrop(deck, ctx, "tasks-confirm-backdrop");
    let w = (panel.width() - 24.0).clamp(220.0, 360.0);
    let pos = Pos2::new(panel.center().x - w / 2.0, panel.min.y + 70.0);
    Area::new(Id::new("tasks-confirm")).order(Order::Tooltip).fixed_pos(pos).constrain(false).show(ctx, |ui| {
        let g1 = ui.painter().layout(title.clone(), theme::ui_semibold(13.0), theme::TEXT, w - 28.0);
        let g2 = ui.painter().layout(detail.clone(), theme::ui(12.0), theme::MUTED, w - 28.0);
        let h = 14.0 + g1.size().y + 6.0 + g2.size().y + 14.0 + BTN_H + 14.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
        shadow(ui.painter(), rect, 10, 10, 30, theme::rgba(0, 0, 0, 0.6));
        frame(ui.painter(), rect, 10.0, theme::PANEL_2, theme::BORDER);
        let mut y = rect.min.y + 14.0;
        ui.painter().galley(Pos2::new(rect.min.x + 14.0, y), g1.clone(), theme::TEXT);
        y += g1.size().y + 6.0;
        ui.painter().galley(Pos2::new(rect.min.x + 14.0, y), g2.clone(), theme::MUTED);
        let by = rect.max.y - 14.0 - BTN_H;
        let dw = btn_width(ui.painter(), yes);
        let d = Rect::from_min_size(Pos2::new(rect.max.x - 14.0 - dw, by), Vec2::new(dw, BTN_H));
        let cw = btn_width(ui.painter(), "Cancelar");
        let c = Rect::from_min_size(Pos2::new(d.min.x - 8.0 - cw, by), Vec2::new(cw, BTN_H));
        let dr = clickable(ui, d, Id::new("tasks-confirm-yes"));
        let fill = if merge { theme::ACCENT } else { theme::RED };
        ui.painter().rect_filled(d, 7.0, if dr.hovered() { theme::brighten(fill, 1.12) } else { fill });
        text_in(ui.painter(), d, Align2::CENTER_CENTER, yes, theme::ui_semibold(12.0), theme::WHITE);
        let cr = clickable(ui, c, Id::new("tasks-confirm-no"));
        ui.painter().rect_filled(c, 7.0, if cr.hovered() { theme::BORDER } else { theme::PANEL });
        text_in(ui.painter(), c, Align2::CENTER_CENTER, "Cancelar", theme::ui_semibold(12.0), theme::TEXT);
        if dr.clicked() {
            acts.push(Act::ConfirmYes);
        } else if cr.clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
            acts.push(Act::ConfirmNo);
        }
    });
}

// ---------------------------------------------------------------------------
// Nova tarefa
// ---------------------------------------------------------------------------

/// Rótulo pequeno de um campo do formulário.
fn field_label(ui: &mut Ui, text: &str) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.0), Sense::hover());
    text_in(
        ui.painter(),
        Rect::from_min_max(Pos2::new(r.min.x, r.min.y + 4.0), r.max),
        Align2::LEFT_CENTER,
        &text.to_uppercase(),
        theme::ui_bold(10.5),
        theme::MUTED,
    );
}

fn chip_width(painter: &egui::Painter, reference: &str) -> f32 {
    let t = text_w(painter, &util::chip_label(reference), theme::ui(12.0)).min(220.0);
    10.0 + t + 4.0 + (8.0 + text_w(painter, "×", theme::ui(13.0))) + 4.0 + 2.0
}

/// Botão "Anexar arquivos" e chips dos arquivos anexados ao pedido da nova tarefa.
fn form_attachments(deck: &mut Deck, ui: &mut Ui, acts: &mut Vec<Act>) {
    let avail = ui.available_width();
    let attachments = &deck.tasks.form.attachments;
    let busy = deck.tasks.form.busy;

    if !attachments.is_empty() {
        ui.add_space(6.0);
        let painter = ui.painter().clone();
        let mut i = 0;
        while i < attachments.len() {
            let mut end = i;
            let mut w = 0.0;
            while end < attachments.len() {
                let cw = chip_width(&painter, &attachments[end].to_string_lossy()).min(avail);
                let next = w + cw + if end > i { 6.0 } else { 0.0 };
                if end > i && next > avail {
                    break;
                }
                w = next;
                end += 1;
            }
            let (row, _) = ui.allocate_exact_size(Vec2::new(avail, CHIP_H + 4.0), Sense::hover());
            let mut cx = row.min.x;
            for (idx, a) in attachments.iter().enumerate().take(end).skip(i) {
                let cw = chip_width(&painter, &a.to_string_lossy()).min(avail);
                let r = Rect::from_min_size(Pos2::new(cx, row.min.y + 2.0), Vec2::new(cw, CHIP_H));
                cx = r.max.x + 6.0;
                frame(ui.painter(), r, 11.0, theme::PANEL_2, theme::BORDER);
                let xw = 8.0 + text_w(ui.painter(), "×", theme::ui(13.0));
                let x_rect = Rect::from_min_size(Pos2::new(r.max.x - 5.0 - xw, r.min.y + 1.0), Vec2::new(xw, r.height() - 2.0));
                let tr = Rect::from_min_max(Pos2::new(r.min.x + 10.0, r.min.y), Pos2::new(x_rect.min.x - 4.0, r.max.y));
                let g = galley_trunc(ui.painter(), &util::chip_label(&a.to_string_lossy()), theme::ui(12.0), theme::CHIP_TEXT, tr.width());
                place(ui.painter(), tr, Align2::LEFT_CENTER, g, theme::CHIP_TEXT);
                let chip_resp = ui.interact(r, Id::new(("task-chip", idx)), Sense::hover());
                tip(chip_resp, a.display().to_string());
                let xr = if !busy {
                    clickable(ui, x_rect, Id::new(("task-chip-x", idx)))
                } else {
                    ui.interact(x_rect, Id::new(("task-chip-x", idx)), Sense::hover())
                };
                text_in(
                    ui.painter(),
                    x_rect,
                    Align2::CENTER_CENTER,
                    "×",
                    theme::ui(13.0),
                    if xr.hovered() && !busy { theme::TEXT } else { theme::MUTED },
                );
                if tip(xr, "Remover anexo").clicked() && !busy {
                    acts.push(Act::RemoveAttachment(idx));
                }
            }
            i = end;
        }
    }

    ui.add_space(6.0);
    let label = "Anexar arquivos";
    let lw = text_w(ui.painter(), label, theme::ui(12.0));
    let bw = 10.0 + 14.0 + 6.0 + lw + 12.0;
    let (row, _) = ui.allocate_exact_size(Vec2::new(avail, 26.0), Sense::hover());
    let btn = Rect::from_min_size(row.min, Vec2::new(bw.min(avail), 26.0));
    let resp =
        if !busy { clickable(ui, btn, Id::new("tasks-new-attach")) } else { ui.interact(btn, Id::new("tasks-new-attach"), Sense::hover()) };
    let hot = !busy && resp.hovered();
    frame(ui.painter(), btn, 6.0, if hot { theme::BORDER } else { theme::PANEL_2 }, theme::BORDER);
    let fg = if !busy {
        if hot {
            theme::TEXT
        } else {
            theme::SELECT_TEXT
        }
    } else {
        theme::SEND_IDLE_TEXT
    };
    deck.icons.paint(ui.painter(), Icon::Attach, Pos2::new(btn.min.x + 16.0, btn.center().y), 13.5, fg, 0.0);
    text_in(
        ui.painter(),
        Rect::from_min_max(Pos2::new(btn.min.x + 28.0, btn.min.y), Pos2::new(btn.max.x - 8.0, btn.max.y)),
        Align2::LEFT_CENTER,
        label,
        theme::ui(12.0),
        fg,
    );
    if tip(resp, "Anexar arquivos ou imagens ao pedido (também pode arrastar e soltar sobre o formulário)").clicked() && !busy {
        acts.push(Act::PickAttachments);
    }
}

/// Botão em forma de campo que abre um menu: texto (com a bolinha do agente, se houver) e "▾".
fn dropdown_button(ui: &mut Ui, id: Id, label: &str, dot: Option<Color32>, enabled: bool, open: bool, tooltip: &str) -> (Rect, bool) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), Sense::hover());
    let resp = if enabled { clickable(ui, r, id) } else { ui.interact(r, id, Sense::hover()) };
    let p = ui.painter();
    let hot = enabled && (resp.hovered() || open);
    frame(p, r, 8.0, if hot { theme::PANEL_2 } else { theme::PANEL }, if open { theme::MUTED } else { theme::BORDER });
    let mut x = r.min.x + 10.0;
    if let Some(c) = dot {
        super::dot(p, Pos2::new(x + 4.0, r.center().y), c);
        x += 16.0;
    }
    let color = if enabled { theme::TEXT } else { theme::SEND_IDLE_TEXT };
    let g = galley_trunc(p, label, theme::ui(12.5), color, (r.max.x - 28.0 - x).max(10.0));
    place(p, Rect::from_min_max(Pos2::new(x, r.min.y), Pos2::new(r.max.x - 26.0, r.max.y)), Align2::LEFT_CENTER, g, color);
    text_in(p, Rect::from_min_max(Pos2::new(r.max.x - 26.0, r.min.y), r.max), Align2::CENTER_CENTER, "▾", theme::ui(12.0), theme::MUTED);
    let clicked = tip(resp, tooltip).clicked() && enabled;
    (r, clicked)
}

/// Marca discreta à direita da linha de um menu do formulário.
#[derive(Clone, Copy)]
enum Detail {
    /// "grátis": modelo sem custo por token.
    Free,
    /// "N modelos": quantos o provedor tem.
    Count(usize),
}

/// Itens de um menu do formulário.
#[derive(Clone)]
struct MenuItem {
    label: String,
    dot: Option<Color32>,
    selected: bool,
    enabled: bool,
    /// Texto discreto à direita: "12 modelos" no passo do provedor, "grátis" nos modelos sem custo.
    detail: Option<Detail>,
    /// Abre outro passo: desenha a seta "›" à direita.
    submenu: bool,
}

/// Menu sob `anchor`. Devolve o índice escolhido; fecha com clique fora ou Esc.
fn dropdown_menu(ctx: &egui::Context, id: &str, anchor: Rect, items: &[MenuItem], acts: &mut Vec<Act>) -> Option<usize> {
    let row = 26.0;
    let h = (items.len() as f32 * row).min(260.0) + 8.0;
    let w = anchor.width();
    let pos = super::menus::keep_on_screen(ctx, Pos2::new(anchor.min.x, anchor.max.y + 2.0), Vec2::new(w, h));
    let menu_rect = Rect::from_min_size(pos, Vec2::new(w, h));
    let mut chosen = None;
    Area::new(Id::new(id)).order(Order::Foreground).fixed_pos(pos).constrain(false).show(ctx, |ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
        shadow(ui.painter(), rect, 8, 8, 24, theme::rgba(0, 0, 0, 0.5));
        frame(ui.painter(), rect, 8.0, theme::PANEL_2, theme::BORDER);
        let list = rect.shrink(4.0);
        let mut child = ui.new_child(UiBuilder::new().max_rect(list));
        child.set_clip_rect(list);
        egui::ScrollArea::vertical().id_salt((id, "list")).auto_shrink([false, false]).show(&mut child, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            for (i, item) in items.iter().enumerate() {
                let (r, _) = ui.allocate_exact_size(Vec2::new(list.width(), row), Sense::hover());
                let resp = if item.enabled { clickable(ui, r, Id::new((id, i))) } else { ui.interact(r, Id::new((id, i)), Sense::hover()) };
                if item.enabled && resp.hovered() {
                    ui.painter().rect_filled(r, 6.0, theme::BORDER);
                }
                let mut x = r.min.x + 10.0;
                if let Some(c) = item.dot {
                    super::dot(ui.painter(), Pos2::new(x + 4.0, r.center().y), c);
                    x += 16.0;
                }
                let color = if !item.enabled {
                    theme::MUTED
                } else if item.selected {
                    theme::TEXT
                } else {
                    theme::SELECT_TEXT
                };
                let mut right = r.max.x - 10.0;
                if item.submenu {
                    let ar = Rect::from_min_max(Pos2::new(right - 12.0, r.min.y), Pos2::new(right, r.max.y));
                    text_in(ui.painter(), ar, Align2::CENTER_CENTER, "›", theme::ui(14.0), theme::MUTED);
                    right -= 16.0;
                }
                if let Some(detail) = &item.detail {
                    let (text, dcolor) = match detail {
                        Detail::Free => ("grátis".to_owned(), theme::BADGE_JSON),
                        Detail::Count(n) => (format!("{n} modelo{}", if *n == 1 { "" } else { "s" }), theme::MUTED),
                    };
                    let g = galley(ui.painter(), &text, theme::ui(11.0), dcolor);
                    let w = g.size().x;
                    place(
                        ui.painter(),
                        Rect::from_min_max(Pos2::new(right - w, r.min.y), Pos2::new(right, r.max.y)),
                        Align2::RIGHT_CENTER,
                        g,
                        dcolor,
                    );
                    right -= w + 8.0;
                }
                let label = if item.selected { format!("✓ {}", item.label) } else { item.label.clone() };
                let g = galley_trunc(ui.painter(), &label, theme::ui(12.5), color, (right - x).max(10.0));
                place(ui.painter(), Rect::from_min_max(Pos2::new(x, r.min.y), Pos2::new(right, r.max.y)), Align2::LEFT_CENTER, g, color);
                if item.enabled && resp.clicked() {
                    chosen = Some(i);
                }
            }
        });
    });
    if chosen.is_none() && (ctx.input(|i| i.key_pressed(Key::Escape)) || super::menus::closes_on_outside(ctx, menu_rect, anchor)) {
        acts.push(Act::CloseMenu);
    }
    chosen
}

fn form_view(deck: &mut Deck, ui: &mut Ui, rect: Rect, acts: &mut Vec<Act>) {
    let head = head_bar(ui, rect, Some("Voltar à lista de tarefas"), "Nova tarefa", 0.0, acts);
    let body = Rect::from_min_max(Pos2::new(rect.min.x, head.max.y), rect.max);
    let ctx = ui.ctx().clone();

    // O que os menus e o botão precisam do Deck, antes de a closure pegar emprestados os campos.
    let kind = deck.tasks.form.kind.clone();
    let choices = deck.tasks_agent_choices();
    let agent_label = deck.agents.name(&kind);
    let agent_dot = deck.agents.tab_color(&kind);
    let picks_model = super::menus::picks_model(&kind);
    let (known, loading) = {
        let (m, l) = deck.models_of(&kind);
        (m.to_vec(), l)
    };
    let agent_items: Vec<MenuItem> = choices
        .iter()
        .map(|k| MenuItem {
            label: deck.agents.name(k),
            dot: Some(deck.agents.tab_color(k)),
            selected: *k == kind,
            enabled: true,
            detail: None,
            submenu: false,
        })
        .collect();
    // O OpenCode lista todos os provedores conectados (`opencode`, `opencode-go`, `ollama`…): o menu
    // pede o provedor antes e mostra só os modelos dele. Os outros agentes têm uma lista só.
    let groups = models::by_provider(&known);
    let sole = models::sole_provider(&groups).map(str::to_owned);
    let step = deck.tasks.form.provider.clone().filter(|p| groups.iter().any(|(name, _)| name == p)).or(sole.clone());
    let model_label = deck.tasks.form.model.clone().unwrap_or_else(|| "Padrão".to_owned());
    let rows: Vec<(MenuItem, Act)> = if let Some(provider) = &step {
        let mut rows = Vec::new();
        // Com um provedor só não há passo anterior: o menu abre direto nos modelos dele.
        if sole.is_none() {
            rows.push((
                MenuItem { label: "‹ Provedores".into(), dot: None, selected: false, enabled: true, detail: None, submenu: false },
                Act::SetProvider(None),
            ));
        }
        rows.push((
            MenuItem {
                label: "Padrão".into(),
                dot: None,
                selected: deck.tasks.form.model.is_none(),
                enabled: true,
                detail: None,
                submenu: false,
            },
            Act::SetModel(None),
        ));
        if let Some((_, list)) = groups.iter().find(|(name, _)| name == provider) {
            rows.extend(list.iter().map(|m| {
                let item = MenuItem {
                    label: models::short_model(m).to_owned(),
                    dot: None,
                    selected: deck.tasks.form.model.as_deref() == Some(m.as_str()),
                    enabled: true,
                    detail: models::is_free(m).then_some(Detail::Free),
                    submenu: false,
                };
                (item, Act::SetModel(Some(m.clone())))
            }));
        }
        rows
    } else if groups.is_empty() {
        let mut rows = vec![(
            MenuItem {
                label: "Padrão".into(),
                dot: None,
                selected: deck.tasks.form.model.is_none(),
                enabled: true,
                detail: None,
                submenu: false,
            },
            Act::SetModel(None),
        )];
        rows.extend(models::free_first(&known).iter().map(|m| {
            let item = MenuItem {
                label: m.clone(),
                dot: None,
                selected: deck.tasks.form.model.as_deref() == Some(m.as_str()),
                enabled: true,
                detail: models::is_free(m).then_some(Detail::Free),
                submenu: false,
            };
            (item, Act::SetModel(Some(m.clone())))
        }));
        if known.is_empty() {
            let text = if loading { "Carregando modelos…" } else { "Sem lista de modelos: só o padrão" };
            rows.push((
                MenuItem { label: text.into(), dot: None, selected: false, enabled: false, detail: None, submenu: false },
                Act::CloseMenu,
            ));
        }
        rows
    } else {
        let mut rows = vec![(
            MenuItem {
                label: "Padrão".into(),
                dot: None,
                selected: deck.tasks.form.model.is_none(),
                enabled: true,
                detail: None,
                submenu: false,
            },
            Act::SetModel(None),
        )];
        rows.extend(groups.iter().map(|(provider, list)| {
            let item = MenuItem {
                label: provider.clone(),
                dot: None,
                selected: false,
                enabled: true,
                detail: Some(Detail::Count(list.len())),
                submenu: true,
            };
            (item, Act::SetProvider(Some(provider.clone())))
        }));
        rows
    };
    let model_items: Vec<MenuItem> = rows.iter().map(|(item, _)| item.clone()).collect();
    let open_menu = deck.tasks.form.menu;
    let (title_id, brief_id, base_id) = (Id::new("tasks-new-title"), Id::new("tasks-new-brief"), Id::new("tasks-new-base"));
    let any_focus = [title_id, brief_id, base_id].iter().any(|id| ctx.memory(|m| m.has_focus(*id)));
    let ctrl_enter = any_focus && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter));
    let focus_title = std::mem::take(&mut deck.tasks.form.focus_title);
    let can_submit = !deck.tasks.form.busy && !deck.tasks.form.title.trim().is_empty() && !choices.is_empty();
    let start_now = deck.tasks.form.start_now;
    let busy = deck.tasks.form.busy;
    let error = deck.tasks.form.error.clone();

    let mut anchors: [Option<Rect>; 2] = [None, None];
    let mut child = ui.new_child(UiBuilder::new().max_rect(body).layout(egui::Layout::top_down(egui::Align::Min)));
    child.set_clip_rect(body);
    egui::ScrollArea::vertical().id_salt("tasks-form").auto_shrink([false, false]).show(&mut child, |ui| {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        ui.add_space(8.0);
        padded(ui, |ui| {
            field_label(ui, "Título");
            text_field(ui, title_id, &mut deck.tasks.form.title, "O que o agente deve fazer, em uma linha", None);
            if focus_title {
                ui.ctx().memory_mut(|m| m.request_focus(title_id));
            }
            ui.add_space(8.0);
            field_label(ui, "Pedido");
            text_field(ui, brief_id, &mut deck.tasks.form.brief, "Detalhes, arquivos, critérios de aceite… (opcional)", Some(6));
            form_attachments(deck, ui, acts);
            ui.add_space(8.0);
            field_label(ui, "Agente");
            let (r, click) = dropdown_button(
                ui,
                Id::new("tasks-new-agent"),
                &agent_label,
                Some(agent_dot),
                !choices.is_empty(),
                open_menu == Some(FormMenu::Agent),
                "Agente que vai executar a tarefa",
            );
            anchors[0] = Some(r);
            if click {
                acts.push(Act::ToggleMenu(FormMenu::Agent));
            }
            ui.add_space(8.0);
            field_label(ui, "Modelo");
            let (r, click) = dropdown_button(
                ui,
                Id::new("tasks-new-model"),
                &model_label,
                None,
                picks_model,
                open_menu == Some(FormMenu::Model),
                if !picks_model {
                    "Este agente não permite escolher o modelo"
                } else if groups.is_empty() || sole.is_some() {
                    "Modelo de LLM do agente"
                } else {
                    "Escolha o provedor e depois o modelo"
                },
            );
            anchors[1] = Some(r);
            if click {
                acts.push(Act::ToggleMenu(FormMenu::Model));
            }
            ui.add_space(8.0);
            field_label(ui, "Base");
            text_field(ui, base_id, &mut deck.tasks.form.base, "branch atual", None);
            ui.add_space(10.0);
            // "Iniciar agora".
            let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.0), Sense::hover());
            let rr = clickable(ui, r, Id::new("tasks-new-start"));
            let cb = Rect::from_min_size(Pos2::new(r.min.x + 1.0, r.center().y - 7.0), Vec2::splat(14.0));
            ui.painter().rect(
                cb,
                3.0,
                if start_now { theme::ACCENT } else { theme::PANEL_2 },
                Stroke::new(1.0, theme::MUTED),
                egui::StrokeKind::Inside,
            );
            if start_now {
                text_in(ui.painter(), cb, Align2::CENTER_CENTER, "✓", theme::ui_bold(11.0), theme::WHITE);
            }
            text_in(
                ui.painter(),
                Rect::from_min_max(Pos2::new(cb.max.x + 8.0, r.min.y), r.max),
                Align2::LEFT_CENTER,
                "Iniciar agora",
                theme::ui(12.5),
                if rr.hovered() { theme::TEXT } else { theme::SELECT_TEXT },
            );
            if tip(rr, "Já cria a worktree e abre o agente. Desligado, a tarefa fica em rascunho para iniciar depois").clicked() {
                acts.push(Act::ToggleStartNow);
            }
        });
        if let Some(e) = &error {
            ui.add_space(8.0);
            error_box(ui, e, false);
        }
        ui.add_space(10.0);
        let label = if busy {
            "Criando…".to_owned()
        } else if start_now {
            "Criar e iniciar".to_owned()
        } else {
            "Criar rascunho".to_owned()
        };
        let b = [
            (label, BtnKind::Primary, can_submit, if can_submit { "Cria a tarefa (Ctrl+Enter)" } else { "Escreva o título da tarefa" }),
            ("Cancelar".to_owned(), BtnKind::Secondary, !busy, "Volta à lista sem criar"),
        ];
        match button_flow(ui, &b, "form") {
            Some(0) => acts.push(Act::Submit),
            Some(_) => acts.push(Act::Back),
            None if ctrl_enter && can_submit => acts.push(Act::Submit),
            None => {}
        }
        ui.add_space(14.0);
    });

    match (open_menu, anchors) {
        (Some(FormMenu::Agent), [Some(a), _]) => {
            if let Some(i) = dropdown_menu(&ctx, "tasks-menu-agent", a, &agent_items, acts) {
                acts.push(Act::SetKind(choices[i].clone()));
            }
        }
        (Some(FormMenu::Model), [_, Some(a)]) => {
            if let Some(i) = dropdown_menu(&ctx, "tasks-menu-model", a, &model_items, acts) {
                if let Some((_, act)) = rows.into_iter().nth(i) {
                    acts.push(act);
                }
            }
        }
        _ => {}
    }
}
