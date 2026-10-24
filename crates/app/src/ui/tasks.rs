//! Aba "Tarefas" da barra direita: a lista de tarefas em andamento e o histórico de encerradas, o
//! detalhe de uma tarefa (pedido, relatório, alterações, histórico e os botões de cada estado) e o
//! formulário de nova tarefa. O estado e as chamadas ao core ficam em `tasks_ui.rs`; aqui só se
//! pinta e se *coleta* o que foi clicado ([`Act`]), aplicado no fim do quadro com o `Deck` inteiro.

use egui::{Align2, Area, Color32, Id, Key, Margin, Modifiers, Order, Pos2, Rect, Sense, Stroke, Ui, UiBuilder, Vec2};

use agentdeck_core::models::{self, Effort};
use agentdeck_core::{FeedKind, MergeBlock, Task, TaskMessage, TaskStatus};

use super::{
    button as ui_button, button_danger, button_width, clickable, frame, galley, galley_spaced, galley_trunc, place, section_label,
    segmented, separator as line_sep, shadow, text_in, text_w, tip, ButtonKind,
};
use crate::deck::Deck;
use crate::icons::Icon;
use crate::tasks_ui::{
    self, actions_for, active_tasks, can_show_diff, card_state_label, clip_text, events_newest_first, history_tasks, is_focused_card,
    parse_diff_stat, status_age, status_name, task_age, task_date_ddmm, task_step, Confirm, FormMenu, HistoryFilter, Section, TaskAct,
    TasksView,
};
use crate::theme;
use crate::util;

const HEAD_H: f32 = theme::TOPBAR_H;
const ROW_H: f32 = 22.0;
const BTN_H: f32 = 28.0;
const CHIP_H: f32 = 21.0;
const PAD: f32 = 12.0;
const LIST_PAD: f32 = 8.0;
const FOOTER_H: f32 = 45.0;
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
    New,
    Back,
    Open(String),
    /// Abre o detalhe da tarefa já com o campo de devolver/reprovar aberto.
    ReturnFromCard(String),
    ToggleHistoryMenu,
    CloseHistoryMenu,
    SetHistoryFilter(HistoryFilter),
    ToggleSection(Section),
    /// Abre ou recolhe uma mensagem da linha do tempo (id da tarefa, hora da mensagem).
    ToggleMessage(String, u64),
    Task(String, TaskAct),
    /// "Ver diff completo" / "Ver o que foi mesclado": abre o diff no centro.
    OpenDiff(String),
    /// "Abrir em Documentos" no pedido (`false`) ou no relatório (`true`).
    OpenText(String, bool),
    DismissNotice,
    /// "Guardar minhas alterações e mesclar" no aviso da recusa.
    MergeStash(String),
    /// "Pedir ao agente para fazer o commit" / "…resolver o conflito".
    AskAgent(String),
    /// "Ver o que está sem commit" / "Esconder a lista".
    ToggleNoticeFiles,
    ConfirmYes,
    ConfirmNo,
    SendReturn(String),
    CancelReturn,
    ToggleMenu(FormMenu),
    CloseMenu,
    SetKind(String),
    SetModel(Option<String>),
    SetEffort(Option<Effort>),
    /// Passo do menu de modelo: o provedor cujos modelos estão à mostra (`None` = lista de provedores).
    SetProvider(Option<String>),
    ToggleStartNow,
    PickAttachments,
    RemoveAttachment(usize),
    Submit,
}

pub fn draw(deck: &mut Deck, ui: &mut Ui, rect: Rect) {
    let mut acts = Vec::new();
    let now = agentdeck_core::util::now_ms();
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
        Act::New => deck.tasks_open_new(),
        Act::Back => deck.tasks_back(),
        Act::Open(id) => deck.tasks_open_detail(&id),
        Act::ReturnFromCard(id) => {
            deck.tasks_open_detail(&id);
            deck.tasks.return_open = true;
        }
        Act::ToggleHistoryMenu => deck.tasks.history_menu = !deck.tasks.history_menu,
        Act::CloseHistoryMenu => deck.tasks.history_menu = false,
        Act::SetHistoryFilter(f) => {
            deck.tasks.history_filter = f;
            deck.tasks.history_menu = false;
        }
        Act::ToggleSection(s) => {
            if !deck.tasks.closed_sections.remove(&s) {
                deck.tasks.closed_sections.insert(s);
            }
        }
        Act::ToggleMessage(id, at_ms) => {
            if !deck.tasks.open_messages.remove(&(id.clone(), at_ms)) {
                deck.tasks.open_messages.insert((id, at_ms));
            }
        }
        Act::Task(id, act) => deck.tasks_act(&id, act),
        Act::OpenDiff(id) => deck.docs_open_task_diff(&id),
        Act::OpenText(id, report) => deck.docs_open_task_text(&id, report),
        Act::DismissNotice => deck.tasks_clear_notice(),
        Act::MergeStash(id) => deck.tasks_merge_stash(&id),
        Act::AskAgent(id) => deck.tasks_ask_agent(&id),
        Act::ToggleNoticeFiles => deck.tasks.notice_files = !deck.tasks.notice_files,
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
        Act::SetEffort(e) => deck.tasks.form.effort = e,
        Act::ToggleStartNow => deck.tasks.form.start_now = !deck.tasks.form.start_now,
        Act::PickAttachments => deck.tasks_pick_attachments(),
        Act::RemoveAttachment(i) => deck.tasks_remove_attachment(i),
        Act::Submit => deck.tasks_submit_form(),
    }
}

// ---------------------------------------------------------------------------
// Peças comuns
// ---------------------------------------------------------------------------

/// Cor monocromática do estado no detalhe: `accent` quando pede ação do humano, cor do agente em
/// execução, `danger` em falha, e `text_2` / `text_3` nos demais.
fn status_color(deck: &Deck, task: &Task) -> Color32 {
    let p = theme::pal();
    match task.status {
        TaskStatus::Draft | TaskStatus::Merged => p.text_2,
        TaskStatus::Running => deck.agents.tab_color(&task.kind),
        TaskStatus::Review | TaskStatus::Validating => theme::accent(),
        TaskStatus::Discarded => p.text_3,
        TaskStatus::Failed => p.danger,
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

/// Faixa do topo (detalhe e formulário): borda inferior, "←" (opcional) e o título.
fn head_bar(ui: &Ui, rect: Rect, back: Option<&str>, title: &str, right: f32, acts: &mut Vec<Act>) -> Rect {
    let head = Rect::from_min_size(rect.min, Vec2::new(rect.width(), HEAD_H));
    let p = ui.painter_at(head);
    let pal = theme::pal();
    line_sep(&p, Pos2::new(head.min.x, head.max.y - 0.5), Pos2::new(head.max.x, head.max.y - 0.5));
    let mut x = head.min.x + PAD;
    if let Some(tooltip) = back {
        let b = Rect::from_min_size(Pos2::new(head.min.x + 6.0, head.center().y - 11.0), Vec2::new(24.0, 22.0));
        let r = clickable(ui, b, Id::new("tasks-back"));
        if r.hovered() {
            p.rect_filled(b, 5.0, pal.hover);
        }
        text_in(&p, b, Align2::CENTER_CENTER, "←", theme::ui(14.0), if r.hovered() { pal.text } else { pal.text_2 });
        if tip(r, tooltip).clicked() {
            acts.push(Act::Back);
        }
        x = b.max.x + 6.0;
    }
    let g = galley_trunc(&p, title, theme::ui_semibold(12.5), pal.text_strong, (head.max.x - right - x).max(20.0));
    place(
        &p,
        Rect::from_min_max(Pos2::new(x, head.min.y), Pos2::new(head.max.x - right, head.max.y - 1.0)),
        Align2::LEFT_CENTER,
        g,
        pal.text_strong,
    );
    head
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BtnKind {
    Primary,
    Secondary,
    Danger,
}

fn btn_width(painter: &egui::Painter, label: &str, kind: BtnKind) -> f32 {
    let font = if matches!(kind, BtnKind::Primary | BtnKind::Danger) { theme::ui_semibold(11.5) } else { theme::ui(11.5) };
    galley(painter, label, font, theme::pal().text).size().x + 22.0
}

/// Botão com texto (cantos 5). Desligado, fica apagado e não reage. Devolve se foi clicado.
fn button(ui: &Ui, rect: Rect, id: Id, label: &str, kind: BtnKind, enabled: bool, tooltip: &str) -> bool {
    let r = if enabled { clickable(ui, rect, id) } else { ui.interact(rect, id, Sense::hover()) };
    let hot = enabled && r.hovered();
    let p = theme::pal();
    let painter = ui.painter();
    if !enabled {
        frame(painter, rect, 5.0, theme::with_alpha(p.text_strong, 0.02), p.separator);
        let g = galley_trunc(painter, label, theme::ui(11.5), p.text_3, rect.width() - 12.0);
        place(painter, rect, Align2::CENTER_CENTER, g, p.text_3);
    } else {
        match kind {
            BtnKind::Primary => {
                let fill = if hot { theme::brighten(theme::accent(), 1.1) } else { theme::accent() };
                painter.rect_filled(rect, 5.0, fill);
                let g = galley_trunc(painter, label, theme::ui_semibold(11.5), p.on_accent, rect.width() - 12.0);
                place(painter, rect, Align2::CENTER_CENTER, g, p.on_accent);
            }
            BtnKind::Secondary => {
                let fill = theme::with_alpha(p.text_strong, if hot { 0.08 } else { 0.04 });
                frame(painter, rect, 5.0, fill, theme::with_alpha(p.text_strong, 0.14));
                let g = galley_trunc(painter, label, theme::ui(11.5), p.text, rect.width() - 12.0);
                place(painter, rect, Align2::CENTER_CENTER, g, p.text);
            }
            BtnKind::Danger => {
                let fill = theme::with_alpha(p.danger, if hot { 0.18 } else { 0.10 });
                frame(painter, rect, 5.0, fill, theme::with_alpha(p.danger, 0.35));
                let g = galley_trunc(painter, label, theme::ui_semibold(11.5), p.danger_text, rect.width() - 12.0);
                place(painter, rect, Align2::CENTER_CENTER, g, p.danger_text);
            }
        }
    }
    let r = if tooltip.is_empty() { r } else { tip(r, tooltip) };
    enabled && r.clicked()
}

/// Botões em linha que quebram para a linha de baixo quando não cabem. Devolve o índice do clicado.
fn button_flow(ui: &mut Ui, buttons: &[(String, BtnKind, bool, &str)], id_salt: &str) -> Option<usize> {
    let avail = ui.available_width();
    let max_w = (avail - 2.0 * PAD).max(60.0);
    let widths: Vec<f32> = buttons.iter().map(|(l, k, ..)| btn_width(ui.painter(), l, *k).min(max_w)).collect();
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
    paragraph(ui, text, theme::ui(12.0), theme::pal().text_2);
}

/// Faixa vermelha com o erro (e o "×" opcional). Devolve se o "×" foi clicado.
fn error_box(ui: &mut Ui, text: &str, dismiss: bool) -> bool {
    notice_box(ui, text, dismiss, true)
}

/// Faixa do aviso: vermelha (`danger`) para erro, âmbar (`warning`) para o que deu certo mas o usuário precisa saber.
fn notice_box(ui: &mut Ui, text: &str, dismiss: bool, danger: bool) -> bool {
    let (tone, text_color) =
        if danger { (theme::pal().danger, theme::pal().danger_text) } else { (theme::pal().warning, theme::pal().text) };
    let w = ui.available_width() - 2.0 * PAD;
    let g = ui.painter().layout(text.to_owned(), theme::ui(12.0), text_color, (w - 22.0 - if dismiss { 18.0 } else { 0.0 }).max(40.0));
    let h = g.size().y + 16.0;
    let (outer, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h + 12.0), Sense::hover());
    let r = Rect::from_min_size(Pos2::new(outer.min.x + PAD, outer.min.y + 3.0), Vec2::new(w, h));
    ui.painter().rect_filled(r, 7.0, theme::with_alpha(tone, 0.12));
    ui.painter().rect_stroke(r, 7.0, Stroke::new(1.0, theme::with_alpha(tone, 0.45)), egui::StrokeKind::Inside);
    ui.painter().galley(Pos2::new(r.min.x + 10.0, r.min.y + 8.0), g, text_color);
    let mut closed = false;
    if dismiss {
        let x = Rect::from_min_size(Pos2::new(r.max.x - 24.0, r.min.y + 2.0), Vec2::splat(22.0));
        let xr = clickable(ui, x, Id::new(("tasks-error-x", text.len())));
        text_in(
            ui.painter(),
            x,
            Align2::CENTER_CENTER,
            "×",
            theme::ui(15.0),
            if xr.hovered() { theme::pal().text } else { theme::pal().text_2 },
        );
        closed = tip(xr, "Fechar o aviso").clicked();
    }
    closed
}

/// Campo de texto numa moldura de 1 px (fundo `recess`, borda `separator` e `accent` no foco); a moldura cresce com o texto.
fn text_field(ui: &mut Ui, id: Id, text: &mut String, hint: &str, rows: Option<usize>) -> egui::Response {
    let focused = ui.ctx().memory(|m| m.has_focus(id));
    let p = theme::pal();
    egui::Frame::new()
        .fill(p.recess)
        .stroke(Stroke::new(1.0, if focused { theme::accent() } else { p.separator }))
        .corner_radius(6)
        .inner_margin(Margin { left: 9, right: 9, top: 6, bottom: 6 })
        .show(ui, |ui| {
            let te = match rows {
                Some(n) => egui::TextEdit::multiline(text).desired_rows(n),
                None => egui::TextEdit::singleline(text),
            };
            te.id(id)
                .frame(egui::Frame::NONE)
                .font(theme::ui(12.0))
                .text_color(p.text)
                .hint_text(egui::RichText::new(hint).font(theme::ui(12.0)).color(p.text_3))
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

/// Cabeçalho de seção recolhível: seta, título em IBM Plex Mono Medium 10 px MAIÚSCULAS e um detalhe à direita.
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
    let p = theme::pal();
    deck.icons.paint(
        ui.painter(),
        Icon::TreeChevron,
        Pos2::new(r.min.x + PAD, r.center().y),
        10.0,
        p.text_3,
        if open { std::f32::consts::FRAC_PI_2 } else { 0.0 },
    );
    let g = galley_spaced(ui.painter(), &title.to_uppercase(), theme::label(10.0), p.text_3, 10.0 * theme::LABEL_SPACING_EM);
    let t = place(ui.painter(), Rect::from_min_max(Pos2::new(r.min.x + 22.0, r.min.y), r.max), Align2::LEFT_CENTER, g, p.text_3);
    if let Some((text, color)) = right {
        let left = t.max.x + 10.0;
        let g = galley_trunc(ui.painter(), &text, theme::meta(10.5), color, (r.max.x - PAD - left).max(10.0));
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
// Lista ("Tarefas" em andamento + "HISTÓRICO" + rodapé "+ Nova tarefa")
// ---------------------------------------------------------------------------

fn list_view(deck: &mut Deck, ui: &mut Ui, rect: Rect, now: u64, acts: &mut Vec<Act>) {
    // Rodapé fixo embaixo: separator + botão primário "+ Nova tarefa" de largura total e 28 px de altura.
    let footer = Rect::from_min_max(Pos2::new(rect.min.x, (rect.max.y - FOOTER_H).max(rect.min.y)), rect.max);
    let body = Rect::from_min_max(rect.min, Pos2::new(rect.max.x, footer.min.y));

    if deck.tasks.tasks.is_empty() {
        // Sem projeto aberto ninguém lê a lista: não fica em "Lendo as tarefas…" para sempre.
        empty_state(ui, body, deck.tasks.loaded || deck.active_project.is_empty());
        draw_footer(ui, footer, acts);
        return;
    }

    let tasks = deck.tasks.tasks.clone();
    let active = active_tasks(&tasks);
    let has_any_history = tasks.iter().any(|t| matches!(t.status, TaskStatus::Merged | TaskStatus::Discarded));
    let filter = deck.tasks.history_filter;
    let history = history_tasks(&tasks, filter);

    let (active_rect, hist_rect) = if has_any_history {
        let rows = history.len().max(1) as f32;
        let wanted_h = (33.0 + rows * 24.0 + 8.0).min(156.0).min((body.height() * 0.45).max(80.0));
        let split_y = (body.max.y - wanted_h).max(body.min.y + 80.0);
        (Rect::from_min_max(body.min, Pos2::new(body.max.x, split_y)), Some(Rect::from_min_max(Pos2::new(body.min.x, split_y), body.max)))
    } else {
        (body, None)
    };

    // 1. Tarefas em andamento (no topo).
    let mut active_child = ui.new_child(UiBuilder::new().max_rect(active_rect).layout(egui::Layout::top_down(egui::Align::Min)));
    active_child.set_clip_rect(active_rect);
    egui::ScrollArea::vertical().id_salt("tasks-list").auto_shrink([false, false]).show(&mut active_child, |ui| {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        ui.add_space(LIST_PAD);
        if active.is_empty() {
            muted_line(ui, "Nenhuma tarefa em andamento.");
        } else {
            for (i, task) in active.iter().enumerate() {
                if i > 0 {
                    ui.add_space(6.0);
                }
                task_card(deck, ui, task, now, acts);
            }
        }
        ui.add_space(LIST_PAD);
    });

    // 2. Seção HISTÓRICO (abaixo das em andamento, com rolagem própria).
    let mut filter_anchor = None;
    if let Some(hrect) = hist_rect {
        filter_anchor = Some(draw_history_section(deck, ui, hrect, &history, filter, acts));
    }

    // 3. Rodapé "+ Nova tarefa".
    draw_footer(ui, footer, acts);

    // Menu suspenso do filtro do histórico.
    if deck.tasks.history_menu {
        if let Some(anchor) = filter_anchor {
            let items: Vec<MenuItem> = HistoryFilter::ALL
                .iter()
                .map(|&f| MenuItem {
                    label: f.label().to_owned(),
                    dot: None,
                    selected: f == filter,
                    enabled: true,
                    detail: None,
                    submenu: false,
                })
                .collect();
            let menu_w = 186.0_f32.max(anchor.width());
            let menu_anchor = Rect::from_min_size(Pos2::new(anchor.max.x - menu_w, anchor.min.y), Vec2::new(menu_w, anchor.height()));
            if let Some(i) = dropdown_menu(ui.ctx(), "tasks-history-filter-menu", menu_anchor, &items, acts, true) {
                acts.push(Act::SetHistoryFilter(HistoryFilter::ALL[i]));
            }
        }
    }
}

fn draw_footer(ui: &Ui, footer: Rect, acts: &mut Vec<Act>) {
    let painter = ui.painter_at(footer);
    line_sep(&painter, Pos2::new(footer.min.x, footer.min.y + 0.5), Pos2::new(footer.max.x, footer.min.y + 0.5));
    let btn_rect =
        Rect::from_min_size(Pos2::new(footer.min.x + LIST_PAD, footer.min.y + 8.5), Vec2::new(footer.width() - 2.0 * LIST_PAD, BTN_H));
    if tip(ui_button(ui, btn_rect, Id::new("tasks-new"), "+ Nova tarefa", ButtonKind::Primary), "Criar uma tarefa para um agente").clicked()
    {
        acts.push(Act::New);
    }
}

fn empty_state(ui: &mut Ui, body: Rect, loaded: bool) {
    let painter = ui.painter_at(body);
    let w = (body.width() - 2.0 * PAD - 8.0).max(80.0);
    let mut y = body.min.y + 24.0;
    let x = body.min.x + PAD + 4.0;
    if !loaded {
        text_in(
            &painter,
            Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, 20.0)),
            Align2::LEFT_CENTER,
            "Lendo as tarefas…",
            theme::ui(12.0),
            theme::pal().text_2,
        );
        return;
    }
    let title = painter.layout("Nenhuma tarefa ainda".to_owned(), theme::ui_semibold(13.0), theme::pal().text_strong, w);
    painter.galley(Pos2::new(x, y), title.clone(), theme::pal().text_strong);
    y += title.size().y + 6.0;
    let text = "Uma tarefa é um pedido que um agente faz numa cópia isolada do projeto (uma branch com a própria pasta). \
Quando ele termina, você revisa o relatório e o diff, mescla, testa e valida. Se algo falhar, devolve ou reprova com o motivo.";
    let g = painter.layout(text.to_owned(), theme::ui(12.0), theme::pal().text_2, w);
    painter.galley(Pos2::new(x, y), g, theme::pal().text_2);
}

/// Cartão de uma tarefa em andamento:
/// - Linha 1: `id` (10,5 px mono, `text_3`), `state` (10,5 px 600, `text_2` ou `accent` no em foco), espaço, `when` (`age_text`, 10,5 px, `text_3`).
/// - Linha 2: `title` (12,5 px 600, `text`, até 2 linhas).
/// - Linha 3: ponto de 6 px na cor do agente + `agente · modelo · esforço` (11 px, `text_2`), espaço e barra de 5 segmentos (3 px × 70 px).
/// - Linha 4 (só `Review` / `Validating`): ações de 24 px (`Primary`, `Secondary`, `Ghost`).
fn task_card(deck: &mut Deck, ui: &mut Ui, task: &Task, now: u64, acts: &mut Vec<Act>) {
    let p = theme::pal();
    let avail_w = ui.available_width();
    let card_w = (avail_w - 2.0 * LIST_PAD).max(120.0);
    let inner_w = (card_w - 20.0).max(80.0);
    let focused = is_focused_card(task.status);
    let has_actions = matches!(task.status, TaskStatus::Review | TaskStatus::Validating);

    // Prepara o título (até 2 linhas, 12,5 px 600).
    let mut title_job = egui::text::LayoutJob {
        wrap: egui::text::TextWrapping { max_width: inner_w, max_rows: 2, break_anywhere: false, overflow_character: Some('…') },
        ..Default::default()
    };
    title_job.append(
        &task.title,
        0.0,
        egui::TextFormat { font_id: theme::ui_semibold(12.5), color: p.text, line_height: Some(16.25), ..Default::default() },
    );
    let title_g = ui.painter().layout_job(title_job);
    let title_h = title_g.size().y.max(16.0);

    let l1_h = 15.0;
    let l3_h = 15.0;
    let actions_h = if has_actions { 6.0 + 24.0 } else { 0.0 };
    let card_h = 8.0 + l1_h + 6.0 + title_h + 6.0 + l3_h + actions_h + 8.0;

    let (outer, _) = ui.allocate_exact_size(Vec2::new(avail_w, card_h), Sense::hover());
    let rect = Rect::from_min_size(Pos2::new(outer.min.x + LIST_PAD, outer.min.y), Vec2::new(card_w, card_h));
    let card_resp = clickable(ui, rect, Id::new(("task-card", &task.id)));
    let hot = card_resp.hovered();

    let (fill, border) = if focused {
        (theme::accent_wash(theme::accent()), theme::accent_edge(theme::accent()))
    } else if hot {
        (theme::with_alpha(p.text_strong, 0.05), theme::with_alpha(p.text_strong, 0.10))
    } else {
        (theme::with_alpha(p.text_strong, 0.03), theme::with_alpha(p.text_strong, 0.06))
    };
    frame(ui.painter(), rect, 8.0, fill, border);

    let painter = ui.painter().clone();
    let x0 = rect.min.x + 10.0;
    let right = rect.max.x - 10.0;

    // Linha 1: id + estado + tempo.
    let y1 = rect.min.y + 8.0;
    let r1 = Rect::from_min_size(Pos2::new(x0, y1), Vec2::new(inner_w, l1_h));
    let id_g = galley(&painter, &task.id, theme::meta(10.5), p.text_3);
    let id_w = id_g.size().x;
    place(&painter, Rect::from_min_size(r1.min, Vec2::new(id_w, l1_h)), Align2::LEFT_CENTER, id_g, p.text_3);

    let state_color = if focused {
        theme::accent()
    } else if task.status == TaskStatus::Failed {
        p.danger_text
    } else {
        p.text_2
    };
    let state_g = galley(&painter, card_state_label(task.status), theme::ui_semibold(10.5), state_color);
    place(
        &painter,
        Rect::from_min_size(Pos2::new(x0 + id_w + 6.0, y1), Vec2::new(state_g.size().x, l1_h)),
        Align2::LEFT_CENTER,
        state_g,
        state_color,
    );

    let when = task_age(task, now);
    let when_g = galley(&painter, &when, theme::ui(10.5), p.text_3);
    place(&painter, r1, Align2::RIGHT_CENTER, when_g, p.text_3);

    // Linha 2: título (até 2 linhas).
    let y2 = y1 + l1_h + 6.0;
    painter.galley(Pos2::new(x0, y2), title_g, p.text);

    // Linha 3: ponto de 6 px na cor do agente + agente · modelo · esforço + barra de 5 etapas (70 px × 3 px).
    let y3 = y2 + title_h + 6.0;
    let r3 = Rect::from_min_size(Pos2::new(x0, y3), Vec2::new(inner_w, l3_h));
    let agent_color = deck.agents.tab_color(&task.kind);
    painter.circle_filled(Pos2::new(x0 + 3.0, r3.center().y), 3.0, agent_color);

    let mut who = deck.agents.name(&task.kind);
    if let Some(m) = tasks_ui::model_effort_text(task) {
        who.push_str(" · ");
        who.push_str(&m);
    }
    let steps_w = 70.0;
    let who_max_w = (right - steps_w - 8.0 - (x0 + 11.0)).max(20.0);
    let who_g = galley_trunc(&painter, &who, theme::ui(11.0), p.text_2, who_max_w);
    place(&painter, Rect::from_min_size(Pos2::new(x0 + 11.0, y3), Vec2::new(who_max_w, l3_h)), Align2::LEFT_CENTER, who_g, p.text_2);

    // Barra de 5 segmentos de 3 px de altura em 70 px de largura (gap de 2 px).
    let step = task_step(task.status);
    let seg_gap = 2.0;
    let seg_w = (steps_w - 4.0 * seg_gap) / 5.0;
    let seg_y = r3.center().y - 1.5;
    let seg_x0 = right - steps_w;
    let active_seg_color = if focused {
        theme::accent()
    } else if task.status == TaskStatus::Failed {
        p.danger
    } else {
        p.text_soft
    };
    for s in 0..5 {
        let sx = seg_x0 + s as f32 * (seg_w + seg_gap);
        let seg_rect = Rect::from_min_size(Pos2::new(sx, seg_y), Vec2::new(seg_w, 3.0));
        let seg_color = if s < step { active_seg_color } else { p.separator };
        painter.rect_filled(seg_rect, 1.5, seg_color);
    }

    // Linha 4: botões de 24 px para tarefas em revisão ou validação.
    let mut action_clicked = false;
    if has_actions {
        let y4 = y3 + l3_h + 6.0;
        let (prim_label, prim_act, prim_tip, sec_label, sec_tip) = if task.status == TaskStatus::Validating {
            ("Validar", TaskAct::Validate, TaskAct::Validate.tip(), "Reprovar", TaskAct::Reject.tip())
        } else {
            ("Mesclar", TaskAct::Merge, TaskAct::Merge.tip(), "Devolver", TaskAct::Return.tip())
        };
        let mut bx = x0;
        let pw = button_width(&painter, prim_label, ButtonKind::Primary);
        let pr = Rect::from_min_size(Pos2::new(bx, y4), Vec2::new(pw, 24.0));
        if tip(ui_button(ui, pr, Id::new(("card-prim", &task.id)), prim_label, ButtonKind::Primary), prim_tip).clicked() {
            acts.push(Act::Task(task.id.clone(), prim_act));
            action_clicked = true;
        }
        bx = pr.max.x + 6.0;

        let sw = button_width(&painter, sec_label, ButtonKind::Secondary);
        let sr = Rect::from_min_size(Pos2::new(bx, y4), Vec2::new(sw, 24.0));
        if tip(ui_button(ui, sr, Id::new(("card-sec", &task.id)), sec_label, ButtonKind::Secondary), sec_tip).clicked() {
            acts.push(Act::ReturnFromCard(task.id.clone()));
            action_clicked = true;
        }
        bx = sr.max.x + 6.0;

        if can_show_diff(task) {
            let gw = button_width(&painter, "Ver diff", ButtonKind::Ghost);
            if bx + gw <= right + 2.0 {
                let gr = Rect::from_min_size(Pos2::new(bx, y4), Vec2::new(gw, 24.0));
                if tip(ui_button(ui, gr, Id::new(("card-diff", &task.id)), "Ver diff", ButtonKind::Ghost), tasks_ui::diff_button_tip(task))
                    .clicked()
                {
                    acts.push(Act::OpenDiff(task.id.clone()));
                    action_clicked = true;
                }
            }
        }
    }

    let tooltip = format!("{} · {}\n{} → {}", task.id, task.title, task.branch, task.base);
    if !action_clicked && tip(card_resp, tooltip).clicked() {
        acts.push(Act::Open(task.id.clone()));
    }
}

/// Seção HISTÓRICO abaixo das tarefas em andamento. Devolve o retângulo do botão de filtro.
fn draw_history_section(deck: &mut Deck, ui: &mut Ui, rect: Rect, history: &[&Task], filter: HistoryFilter, acts: &mut Vec<Act>) -> Rect {
    let p = theme::pal();
    let painter = ui.painter_at(rect);
    line_sep(&painter, Pos2::new(rect.min.x, rect.min.y + 0.5), Pos2::new(rect.max.x, rect.min.y + 0.5));

    // Cabeçalho: section_label("HISTÓRICO") à esquerda e filtro "Concluídas e descartadas ▾" à direita.
    let head = Rect::from_min_size(Pos2::new(rect.min.x, rect.min.y + 6.0), Vec2::new(rect.width(), super::SECTION_LABEL_H));
    section_label(&painter, head, "HISTÓRICO");

    let filter_text = format!("{} ▾", filter.label());
    let fg_measure = galley(&painter, &filter_text, theme::ui(11.0), p.text_2);
    let fw = fg_measure.size().x + 12.0;
    let filter_rect = Rect::from_min_size(Pos2::new(head.max.x - LIST_PAD - fw, head.center().y - 10.0), Vec2::new(fw, 20.0));
    let f_resp = clickable(ui, filter_rect, Id::new("tasks-history-filter"));
    let f_hot = f_resp.hovered() || deck.tasks.history_menu;
    if f_hot {
        painter.rect_filled(filter_rect, 4.0, p.hover);
    }
    text_in(&painter, filter_rect, Align2::CENTER_CENTER, &filter_text, theme::ui(11.0), if f_hot { p.text } else { p.text_2 });
    if tip(f_resp, "Filtrar o histórico de tarefas encerradas").clicked() {
        acts.push(Act::ToggleHistoryMenu);
    }

    // Lista rolável de linhas de 24 px.
    let list_rect = Rect::from_min_max(Pos2::new(rect.min.x, head.max.y + 2.0), Pos2::new(rect.max.x, rect.max.y - 4.0));
    let mut child = ui.new_child(UiBuilder::new().max_rect(list_rect).layout(egui::Layout::top_down(egui::Align::Min)));
    child.set_clip_rect(list_rect);
    egui::ScrollArea::vertical().id_salt("tasks-history").auto_shrink([false, false]).show(&mut child, |ui| {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        if history.is_empty() {
            let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.0), Sense::hover());
            text_in(
                ui.painter(),
                Rect::from_min_max(Pos2::new(r.min.x + 10.0, r.min.y), r.max),
                Align2::LEFT_CENTER,
                "Nenhuma tarefa neste filtro.",
                theme::ui(11.5),
                p.text_3,
            );
        } else {
            for task in history {
                history_row(deck, ui, task, acts);
            }
        }
    });

    filter_rect
}

/// Uma linha de 24 px no HISTÓRICO: ponto de 6 px na cor do agente, `id` (10,5 px mono, `text_3`),
/// `title` truncado (11,5 px, `text`; se `Discarded`, riscado em `text_3`), `state` (10,5 px, `text_2`)
/// e data `dd/mm` (10 px mono, `text_3`, alinhada à direita).
fn history_row(deck: &mut Deck, ui: &mut Ui, task: &Task, acts: &mut Vec<Act>) {
    let p = theme::pal();
    let avail_w = ui.available_width();
    let (outer, _) = ui.allocate_exact_size(Vec2::new(avail_w, 24.0), Sense::hover());
    let row = outer.shrink2(Vec2::new(LIST_PAD, 0.0));
    let resp = clickable(ui, row, Id::new(("task-hist", &task.id)));
    if resp.hovered() {
        ui.painter().rect_filled(row, 5.0, p.hover);
    }
    let painter = ui.painter().clone();
    let cy = row.center().y;

    // 1. Ponto de 6 px na cor do agente.
    painter.circle_filled(Pos2::new(row.min.x + 6.0 + 3.0, cy), 3.0, deck.agents.tab_color(&task.kind));

    // 2. id (10,5 px mono, text_3, 24 px de largura).
    let id_rect = Rect::from_min_size(Pos2::new(row.min.x + 18.0, row.min.y), Vec2::new(24.0, 24.0));
    text_in(&painter, id_rect, Align2::LEFT_CENTER, &task.id, theme::meta(10.5), p.text_3);

    // 3. Data dd/mm à direita (34 px) e estado antes dela.
    let date_rect = Rect::from_min_size(Pos2::new(row.max.x - 6.0 - 34.0, row.min.y), Vec2::new(34.0, 24.0));
    text_in(&painter, date_rect, Align2::RIGHT_CENTER, &task_date_ddmm(task), theme::meta(10.0), p.text_3);

    let state_txt = card_state_label(task.status);
    let state_g = galley(&painter, state_txt, theme::ui(10.5), p.text_2);
    let state_w = state_g.size().x;
    let state_rect = Rect::from_min_size(Pos2::new(date_rect.min.x - 6.0 - state_w, row.min.y), Vec2::new(state_w, 24.0));
    place(&painter, state_rect, Align2::RIGHT_CENTER, state_g, p.text_2);

    // 4. Título truncado (riscado em text_3 quando Discarded).
    let title_x0 = id_rect.max.x + 6.0;
    let title_w = (state_rect.min.x - 6.0 - title_x0).max(20.0);
    let discarded = task.status == TaskStatus::Discarded;
    let title_color = if discarded { p.text_3 } else { p.text };
    let mut job = egui::text::LayoutJob {
        wrap: egui::text::TextWrapping { max_width: title_w, max_rows: 1, break_anywhere: true, overflow_character: Some('…') },
        ..Default::default()
    };
    job.append(
        &task.title,
        0.0,
        egui::TextFormat {
            font_id: theme::ui(11.5),
            color: title_color,
            strikethrough: if discarded { Stroke::new(1.0, p.text_3) } else { Stroke::NONE },
            ..Default::default()
        },
    );
    let title_g = painter.layout_job(job);
    place(
        &painter,
        Rect::from_min_size(Pos2::new(title_x0, row.min.y), Vec2::new(title_w, 24.0)),
        Align2::LEFT_CENTER,
        title_g,
        title_color,
    );

    let tooltip = format!("{} · {}\n{}", task.id, task.title, status_name(task.status));
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
    let notice_info = deck.tasks.notice_info;
    let notice_block = deck.tasks.notice_block.clone();
    let notice_stash = deck.tasks.notice_stash.clone();
    let notice_files = deck.tasks.notice_files;
    let return_open = deck.tasks.return_open && matches!(task.status, TaskStatus::Review | TaskStatus::Validating);
    let body = Rect::from_min_max(Pos2::new(rect.min.x, head.max.y), rect.max);
    let mut child = ui.new_child(UiBuilder::new().max_rect(body).layout(egui::Layout::top_down(egui::Align::Min)));
    child.set_clip_rect(body);
    let id_salt = format!("tasks-detail-{}", task.id);
    egui::ScrollArea::vertical().id_salt(id_salt).auto_shrink([false, false]).show(&mut child, |ui| {
        let p = theme::pal();
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        ui.add_space(10.0);
        // Título.
        let w = ui.available_width() - 2.0 * PAD;
        let g = ui.painter().layout(task.title.clone(), theme::ui_semibold(14.0), p.text_strong, w);
        let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), g.size().y + 8.0), Sense::hover());
        ui.painter().galley(Pos2::new(r.min.x + PAD, r.min.y), g, p.text_strong);

        // Chip do estado e o tempo.
        let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::hover());
        let name = status_name(task.status);
        let cw = galley(ui.painter(), name, theme::ui_semibold(11.0), color).size().x + 24.0;
        let chip = Rect::from_min_size(Pos2::new(r.min.x + PAD, r.min.y), Vec2::new(cw, 20.0));
        ui.painter().rect_filled(chip, 5.0, p.chip);
        status_dot(ui, Pos2::new(chip.min.x + 10.0, chip.center().y), color, task.status == TaskStatus::Running);
        text_in(
            ui.painter(),
            Rect::from_min_max(Pos2::new(chip.min.x + 19.0, chip.min.y), chip.max),
            Align2::LEFT_CENTER,
            name,
            theme::ui_semibold(11.0),
            color,
        );
        let when = status_age(task, now);
        let g = galley_trunc(ui.painter(), &when, theme::ui(11.5), p.text_2, (r.max.x - PAD - chip.max.x - 10.0).max(10.0));
        place(
            ui.painter(),
            Rect::from_min_max(Pos2::new(chip.max.x + 10.0, r.min.y), Pos2::new(r.max.x - PAD, r.max.y - 2.0)),
            Align2::LEFT_CENTER,
            g,
            p.text_2,
        );
        ui.add_space(8.0);

        // `branch → base` e o agente com o modelo.
        padded(ui, |ui| {
            let line = tasks_ui::branch_line(task);
            let g = ui.painter().layout(line, theme::mono(11.0), p.text_2, ui.available_width() - 20.0);
            let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), g.size().y + 4.0), Sense::hover());
            deck.icons.paint(ui.painter(), Icon::GitBranch, r.min + Vec2::new(6.0, 8.5), 12.0, p.text_2, 0.0);
            ui.painter().galley(Pos2::new(r.min.x + 20.0, r.min.y + 1.0), g, p.text_2);
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
            if let Some(m) = tasks_ui::model_effort_text(task) {
                who.push_str(" · ");
                who.push_str(&m);
            }
            let g = galley_trunc(ui.painter(), &who, theme::ui(12.0), p.text, (r.max.x - x).max(10.0));
            place(ui.painter(), Rect::from_min_max(Pos2::new(x, r.min.y), r.max), Align2::LEFT_CENTER, g, p.text);
        });
        ui.add_space(6.0);

        // O erro da tarefa (Failed) e o da última ação.
        if let Some(err) = task.error.as_deref().filter(|_| task.status == TaskStatus::Failed) {
            error_box(ui, err, false);
        }
        if let Some(n) = &notice {
            // Recusa da mesclagem com saída: a explicação em linguagem comum no lugar do texto do git, e o botão da saída.
            // O aviso do stash vai junto: é a única frase que diz onde ficaram as alterações do usuário.
            let shown = match (&notice_block, &notice_stash) {
                (Some(b), Some(note)) => format!("{} {note}", b.explain(task)),
                (Some(b), None) => b.explain(task),
                (None, _) => n.clone(),
            };
            if notice_box(ui, &shown, true, !notice_info) {
                acts.push(Act::DismissNotice);
            }
            if let Some(block) = &notice_block {
                refusal_buttons(ui, task, block, notice_files, !working, acts);
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
                .color(p.text_2)
                .paint_at(ui, Rect::from_center_size(Pos2::new(r.min.x + PAD + 7.0, r.center().y), Vec2::splat(14.0)));
            text_in(
                ui.painter(),
                Rect::from_min_max(Pos2::new(r.min.x + PAD + 22.0, r.min.y), r.max),
                Align2::LEFT_CENTER,
                label,
                theme::ui(12.0),
                p.text_2,
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
                paragraph(ui, &clip_text(task.brief.trim(), TEXT_MAX), theme::ui(12.0), p.text_2);
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
                            paragraph(ui, &clip_text(report.trim(), TEXT_MAX), theme::ui(12.0), p.text_2);
                        });
                    open_center_button(ui, "report", task, true, acts);
                }
                None if task.status == TaskStatus::Running => muted_line(ui, "O agente ainda não entregou o relatório."),
                None => muted_line(ui, "Sem relatório."),
            }
        }
        // Alterações.
        let summary = task.diff_stat.as_deref().and_then(parse_diff_stat);
        let right = summary.map(|s| (s.text(), p.text_3));
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
        // Mensagens.
        let messages = tasks_ui::messages_in_order(task);
        if section_header(deck, ui, Section::Messages, "Mensagens", Some((messages.len().to_string(), p.text_3)), acts) {
            timeline(deck, ui, task, &messages, acts);
        }
        // Histórico.
        let events = events_newest_first(task);
        if section_header(deck, ui, Section::History, "Histórico", Some((events.len().to_string(), p.text_3)), acts) {
            history(ui, &events);
        }
        ui.add_space(14.0);
    });
}

/// Os botões da saída da recusa da mesclagem (e, para o que está sem commit, a lista completa de arquivos).
fn refusal_buttons(ui: &mut Ui, task: &Task, block: &MergeBlock, files_open: bool, enabled: bool, acts: &mut Vec<Act>) {
    let mergeable = matches!(task.status, TaskStatus::Review | TaskStatus::Failed);
    if !mergeable {
        return;
    }
    let (buttons, files): (Vec<(String, BtnKind, bool, &str)>, bool) = match block {
        MergeBlock::MainDirty(_) => (
            vec![
                (
                    "Guardar minhas alterações e mesclar".to_owned(),
                    BtnKind::Primary,
                    enabled,
                    "Guarda as suas alterações sem commit (git stash), mescla a tarefa e devolve as alterações; se elas se chocarem com a tarefa, ficam guardadas e o aviso diz como recuperar",
                ),
                (
                    if files_open { "Esconder a lista" } else { "Ver o que está sem commit" }.to_owned(),
                    BtnKind::Secondary,
                    true,
                    "Mostra todos os arquivos com alterações sem commit na pasta principal",
                ),
            ],
            true,
        ),
        MergeBlock::TaskDirty(_) => (
            vec![(
                "Pedir ao agente para fazer o commit".to_owned(),
                BtnKind::Primary,
                enabled,
                "Devolve a tarefa ao agente com a lista dos arquivos e o pedido de fazer o commit",
            )],
            false,
        ),
        MergeBlock::Conflict(_) => (
            vec![(
                "Pedir ao agente para resolver o conflito".to_owned(),
                BtnKind::Primary,
                enabled,
                "Devolve a tarefa ao agente pedindo para juntar a base na branch dele (git merge) e resolver os arquivos em conflito",
            )],
            false,
        ),
    };
    match button_flow(ui, &buttons, "refusal") {
        Some(0) if files => acts.push(Act::MergeStash(task.id.clone())),
        Some(0) => acts.push(Act::AskAgent(task.id.clone())),
        Some(_) => acts.push(Act::ToggleNoticeFiles),
        None => {}
    }
    if files_open && files {
        let list = block.files().join(
            "
",
        );
        paragraph(ui, &clip_text(&list, TEXT_MAX), theme::mono(11.0), theme::pal().text_2);
    }
}

/// "Abrir em Documentos" embaixo do pedido e do relatório: eles aparecem formatados no modo Documentos.
fn open_center_button(ui: &mut Ui, id_salt: &str, task: &Task, report: bool, acts: &mut Vec<Act>) {
    let tip = if report {
        "Abre o relatório formatado, com a largura toda do centro"
    } else {
        "Abre o pedido formatado, com a largura toda do centro"
    };
    let b = [("Abrir em Documentos".to_owned(), BtnKind::Secondary, true, tip)];
    if button_flow(ui, &b, id_salt).is_some() {
        acts.push(Act::OpenText(task.id.clone(), report));
    }
}

fn separator(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    line_sep(ui.painter(), Pos2::new(r.min.x + PAD, r.min.y + 0.5), Pos2::new(r.max.x - PAD, r.min.y + 0.5));
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
    let p = theme::pal();
    let shown = lines.len().min(STAT_MAX_LINES);
    let extra = lines.len() - shown;
    let row = 16.0;
    let h = (shown + usize::from(extra > 0)) as f32 * row + 12.0;
    let (outer, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h + 6.0), Sense::hover());
    let r = Rect::from_min_size(Pos2::new(outer.min.x + PAD, outer.min.y + 2.0), Vec2::new(outer.width() - 2.0 * PAD, h));
    frame(ui.painter(), r, 6.0, p.recess, p.separator);
    for (i, line) in lines.iter().take(shown).enumerate() {
        let y = r.min.y + 6.0 + i as f32 * row;
        let count = galley(ui.painter(), &line.changes, theme::meta(10.5), p.text_2);
        let cw = count.size().x;
        place(
            ui.painter(),
            Rect::from_min_size(Pos2::new(r.max.x - 8.0 - cw, y), Vec2::new(cw, row)),
            Align2::LEFT_CENTER,
            count,
            p.text_2,
        );
        let g = tail_galley(ui.painter(), &line.path, theme::mono(11.0), p.text_2, (r.width() - 16.0 - cw - 10.0).max(20.0));
        place(
            ui.painter(),
            Rect::from_min_size(Pos2::new(r.min.x + 8.0, y), Vec2::new(r.width() - 16.0 - cw, row)),
            Align2::LEFT_CENTER,
            g,
            p.text_2,
        );
    }
    if extra > 0 {
        text_in(
            ui.painter(),
            Rect::from_min_size(Pos2::new(r.min.x + 8.0, r.min.y + 6.0 + shown as f32 * row), Vec2::new(r.width() - 16.0, row)),
            Align2::LEFT_CENTER,
            &format!("… e mais {extra} arquivos"),
            theme::ui(11.0),
            p.text_2,
        );
    }
}

/// Linha do tempo das mensagens do `ask-agent` da tarefa, da mais antiga para a mais nova: hora, tipo, de → para, e o
/// texto (recolhido; o clique abre inteiro).
fn timeline(deck: &Deck, ui: &mut Ui, task: &Task, messages: &[&TaskMessage], acts: &mut Vec<Act>) {
    if messages.is_empty() {
        muted_line(ui, "Nenhuma pergunta nem resposta com outros agentes ainda.");
        return;
    }
    let p = theme::pal();
    let hidden = messages.len().saturating_sub(tasks_ui::MESSAGES_SHOWN);
    if hidden > 0 {
        muted_line(ui, &format!("… e mais {hidden} mensagens anteriores."));
    }
    for m in messages.iter().skip(hidden) {
        let open = deck.tasks.open_messages.contains(&(task.id.clone(), m.at_ms));
        let (preview, more) = tasks_ui::message_preview(&m.text);
        let shown = if open { clip_text(m.text.trim(), TEXT_MAX).into_owned() } else { preview };
        let body_color = if m.kind == FeedKind::Error { p.danger_text } else { p.text_2 };
        let w = ui.available_width();
        let body = ui.painter().layout(shown, theme::ui(12.0), body_color, (w - 2.0 * PAD).max(40.0));
        let head_h = 18.0;
        let foot_h = if more { 16.0 } else { 0.0 };
        let h = head_h + body.size().y + foot_h + 8.0;
        let (r, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
        if more {
            if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if resp.clicked() {
                acts.push(Act::ToggleMessage(task.id.clone(), m.at_ms));
            }
            tip(resp.clone(), if open { "Recolhe a mensagem" } else { "Abre a mensagem inteira" });
        }
        if more && resp.hovered() {
            ui.painter().rect_filled(r.shrink2(Vec2::new(4.0, 0.0)), 5.0, p.hover);
        }
        let painter = ui.painter();
        let time = galley(painter, &tasks_ui::event_time(m.at_ms), theme::meta(10.5), p.text_3);
        let time_w = time.size().x;
        place(
            painter,
            Rect::from_min_size(Pos2::new(r.min.x + PAD, r.min.y + 3.0), Vec2::new(time_w, head_h)),
            Align2::LEFT_CENTER,
            time,
            p.text_3,
        );
        let kind_color = if m.kind == FeedKind::Error { p.danger_text } else { p.text };
        let kind = galley(painter, tasks_ui::message_kind_label(m.kind), theme::ui_medium(11.0), kind_color);
        let kind_w = kind.size().x;
        let kind_x = r.min.x + PAD + time_w + 8.0;
        place(
            painter,
            Rect::from_min_size(Pos2::new(kind_x, r.min.y + 3.0), Vec2::new(kind_w, head_h)),
            Align2::LEFT_CENTER,
            kind,
            kind_color,
        );
        let route_x = kind_x + kind_w + 8.0;
        let route = galley_trunc(painter, &tasks_ui::message_route(m), theme::meta(10.5), p.text_3, (r.max.x - PAD - route_x).max(20.0));
        place(
            painter,
            Rect::from_min_max(Pos2::new(route_x, r.min.y + 3.0), Pos2::new(r.max.x - PAD, r.min.y + 3.0 + head_h)),
            Align2::LEFT_CENTER,
            route,
            p.text_3,
        );
        painter.galley(Pos2::new(r.min.x + PAD, r.min.y + 3.0 + head_h), body.clone(), body_color);
        if more {
            text_in(
                painter,
                Rect::from_min_size(Pos2::new(r.min.x + PAD, r.min.y + 3.0 + head_h + body.size().y), Vec2::new(w - 2.0 * PAD, foot_h)),
                Align2::LEFT_CENTER,
                if open { "recolher" } else { "ver tudo" },
                theme::meta(10.5),
                p.text_3,
            );
        }
    }
}

/// Eventos (do mais novo): data e hora, e o texto ao lado.
fn history(ui: &mut Ui, events: &[&agentdeck_core::TaskEvent]) {
    if events.is_empty() {
        muted_line(ui, "Nada registrado ainda.");
        return;
    }
    let p = theme::pal();
    let time_w = galley(ui.painter(), "00/00 00:00", theme::meta(10.5), p.text_3).size().x;
    for e in events.iter().take(EVENTS_MAX) {
        let text_w = (ui.available_width() - 2.0 * PAD - time_w - 8.0).max(40.0);
        let g = ui.painter().layout(clip_text(&e.text, 1_000).into_owned(), theme::ui(12.0), p.text_2, text_w);
        let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), g.size().y + 5.0), Sense::hover());
        let t = galley(ui.painter(), &tasks_ui::event_time(e.at_ms), theme::meta(10.5), p.text_3);
        ui.painter().galley(
            Pos2::new(r.min.x + PAD, r.min.y + 2.0 + (g.rows.first().map_or(14.0, |row| row.height()) - t.size().y) / 2.0),
            t,
            p.text_3,
        );
        ui.painter().galley(Pos2::new(r.min.x + PAD + time_w + 8.0, r.min.y + 2.0), g, p.text_2);
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
        let p = theme::pal();
        let g1 = ui.painter().layout(title.clone(), theme::ui_semibold(13.0), p.text_strong, w - 28.0);
        let g2 = ui.painter().layout(detail.clone(), theme::ui(12.0), p.text_2, w - 28.0);
        let h = 14.0 + g1.size().y + 6.0 + g2.size().y + 14.0 + BTN_H + 14.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
        shadow(ui.painter(), rect, 10, 8, 24, p.shadow);
        frame(ui.painter(), rect, 10.0, p.popup, p.glass_border);
        let mut y = rect.min.y + 14.0;
        ui.painter().galley(Pos2::new(rect.min.x + 14.0, y), g1.clone(), p.text_strong);
        y += g1.size().y + 6.0;
        ui.painter().galley(Pos2::new(rect.min.x + 14.0, y), g2, p.text_2);
        let by = rect.max.y - 14.0 - BTN_H;
        let dw = button_width(ui.painter(), yes, ButtonKind::Primary);
        let d = Rect::from_min_size(Pos2::new(rect.max.x - 14.0 - dw, by), Vec2::new(dw, BTN_H));
        let cw = button_width(ui.painter(), "Cancelar", ButtonKind::Secondary);
        let c = Rect::from_min_size(Pos2::new(d.min.x - 8.0 - cw, by), Vec2::new(cw, BTN_H));
        let dr = if merge {
            ui_button(ui, d, Id::new("tasks-confirm-yes"), yes, ButtonKind::Primary)
        } else {
            button_danger(ui, d, Id::new("tasks-confirm-yes"), yes)
        };
        let cr = ui_button(ui, c, Id::new("tasks-confirm-no"), "Cancelar", ButtonKind::Secondary);
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

/// Rótulo de seção/campo do formulário (IBM Plex Mono Medium 10 px, MAIÚSCULAS, `text_3`, `0.1em`).
/// Os segmentos do seletor de esforço, na ordem de [`effort_of`].
const EFFORT_LABELS: [&str; 4] = ["Padrão", "Baixo", "Médio", "Alto"];

/// O segmento de um esforço (`None` = "Padrão").
fn effort_index(effort: Option<Effort>) -> usize {
    effort.map_or(0, |e| 1 + Effort::ALL.iter().position(|x| *x == e).unwrap_or(0))
}

/// O esforço de um segmento.
fn effort_of(index: usize) -> Option<Effort> {
    index.checked_sub(1).and_then(|i| Effort::ALL.get(i).copied())
}

fn field_label(ui: &mut Ui, text: &str) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.0), Sense::hover());
    let color = theme::pal().text_3;
    let g = galley_spaced(ui.painter(), &text.to_uppercase(), theme::label(10.0), color, 10.0 * theme::LABEL_SPACING_EM);
    place(ui.painter(), Rect::from_min_max(Pos2::new(r.min.x, r.min.y + 4.0), r.max), Align2::LEFT_CENTER, g, color);
}

fn chip_width(painter: &egui::Painter, reference: &str) -> f32 {
    let t = text_w(painter, &util::chip_label(reference), theme::ui(11.5)).min(220.0);
    10.0 + t + 4.0 + (8.0 + text_w(painter, "×", theme::ui(13.0))) + 4.0 + 2.0
}

/// Botão "Anexar arquivos" e chips dos arquivos anexados ao pedido da nova tarefa.
fn form_attachments(deck: &mut Deck, ui: &mut Ui, acts: &mut Vec<Act>) {
    let avail = ui.available_width();
    let attachments = &deck.tasks.form.attachments;
    let busy = deck.tasks.form.busy;
    let p = theme::pal();

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
                frame(ui.painter(), r, 5.0, p.recess, p.separator);
                let xw = 8.0 + text_w(ui.painter(), "×", theme::ui(13.0));
                let x_rect = Rect::from_min_size(Pos2::new(r.max.x - 5.0 - xw, r.min.y + 1.0), Vec2::new(xw, r.height() - 2.0));
                let tr = Rect::from_min_max(Pos2::new(r.min.x + 10.0, r.min.y), Pos2::new(x_rect.min.x - 4.0, r.max.y));
                let g = galley_trunc(ui.painter(), &util::chip_label(&a.to_string_lossy()), theme::ui(11.5), p.text_2, tr.width());
                place(ui.painter(), tr, Align2::LEFT_CENTER, g, p.text_2);
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
                    if xr.hovered() && !busy { p.text } else { p.text_2 },
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
    let lw = text_w(ui.painter(), label, theme::ui(11.5));
    let bw = 10.0 + 14.0 + 6.0 + lw + 12.0;
    let (row, _) = ui.allocate_exact_size(Vec2::new(avail, 26.0), Sense::hover());
    let btn = Rect::from_min_size(row.min, Vec2::new(bw.min(avail), 26.0));
    let resp =
        if !busy { clickable(ui, btn, Id::new("tasks-new-attach")) } else { ui.interact(btn, Id::new("tasks-new-attach"), Sense::hover()) };
    let hot = !busy && resp.hovered();
    frame(ui.painter(), btn, 5.0, theme::with_alpha(p.text_strong, if hot { 0.08 } else { 0.04 }), theme::with_alpha(p.text_strong, 0.14));
    let fg = if !busy {
        if hot {
            p.text
        } else {
            p.text_2
        }
    } else {
        p.text_3
    };
    deck.icons.paint(ui.painter(), Icon::Attach, Pos2::new(btn.min.x + 16.0, btn.center().y), 13.5, fg, 0.0);
    text_in(
        ui.painter(),
        Rect::from_min_max(Pos2::new(btn.min.x + 28.0, btn.min.y), Pos2::new(btn.max.x - 8.0, btn.max.y)),
        Align2::LEFT_CENTER,
        label,
        theme::ui(11.5),
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
    let pal = theme::pal();
    let hot = enabled && (resp.hovered() || open);
    frame(p, r, 6.0, if hot { pal.hover } else { pal.recess }, if open { theme::accent() } else { pal.separator });
    let mut x = r.min.x + 10.0;
    if let Some(c) = dot {
        super::dot(p, Pos2::new(x + 4.0, r.center().y), c);
        x += 16.0;
    }
    let color = if enabled { pal.text } else { pal.text_3 };
    let g = galley_trunc(p, label, theme::ui(12.0), color, (r.max.x - 28.0 - x).max(10.0));
    place(p, Rect::from_min_max(Pos2::new(x, r.min.y), Pos2::new(r.max.x - 26.0, r.max.y)), Align2::LEFT_CENTER, g, color);
    text_in(p, Rect::from_min_max(Pos2::new(r.max.x - 26.0, r.min.y), r.max), Align2::CENTER_CENTER, "▾", theme::ui(11.5), pal.text_2);
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

/// Itens de um menu do formulário ou do filtro.
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

/// Menu sob `anchor` (fundo `popup` opaco, `glass_border`, cantos 8, itens de 26 px com `hover`).
/// Devolve o índice escolhido; fecha com clique fora ou Esc.
fn dropdown_menu(
    ctx: &egui::Context,
    id: &str,
    anchor: Rect,
    items: &[MenuItem],
    acts: &mut Vec<Act>,
    is_history_menu: bool,
) -> Option<usize> {
    let row = 26.0;
    let h = (items.len() as f32 * row).min(260.0) + 8.0;
    let w = anchor.width();
    let pos = super::menus::keep_on_screen(ctx, Pos2::new(anchor.min.x, anchor.max.y + 2.0), Vec2::new(w, h));
    let menu_rect = Rect::from_min_size(pos, Vec2::new(w, h));
    let mut chosen = None;
    let p = theme::pal();
    Area::new(Id::new(id)).order(Order::Foreground).fixed_pos(pos).constrain(false).show(ctx, |ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
        shadow(ui.painter(), rect, 8, 8, 24, p.shadow);
        frame(ui.painter(), rect, 8.0, p.popup, p.glass_border);
        let list = rect.shrink(4.0);
        let mut child = ui.new_child(UiBuilder::new().max_rect(list));
        child.set_clip_rect(list);
        egui::ScrollArea::vertical().id_salt((id, "list")).auto_shrink([false, false]).show(&mut child, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            for (i, item) in items.iter().enumerate() {
                let (r, _) = ui.allocate_exact_size(Vec2::new(list.width(), row), Sense::hover());
                let resp = if item.enabled { clickable(ui, r, Id::new((id, i))) } else { ui.interact(r, Id::new((id, i)), Sense::hover()) };
                if item.enabled && resp.hovered() {
                    ui.painter().rect_filled(r, 5.0, p.hover);
                }
                let mut x = r.min.x + 10.0;
                if let Some(c) = item.dot {
                    super::dot(ui.painter(), Pos2::new(x + 4.0, r.center().y), c);
                    x += 16.0;
                }
                let color = if !item.enabled {
                    p.text_3
                } else if item.selected || resp.hovered() {
                    p.text
                } else {
                    p.text_2
                };
                let mut right = r.max.x - 10.0;
                if item.submenu {
                    let ar = Rect::from_min_max(Pos2::new(right - 12.0, r.min.y), Pos2::new(right, r.max.y));
                    text_in(ui.painter(), ar, Align2::CENTER_CENTER, "›", theme::ui(13.0), p.text_2);
                    right -= 16.0;
                }
                if let Some(detail) = &item.detail {
                    let (text, dcolor) = match detail {
                        Detail::Free => ("grátis".to_owned(), p.success),
                        Detail::Count(n) => (format!("{n} modelo{}", if *n == 1 { "" } else { "s" }), p.text_2),
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
                let g = galley_trunc(ui.painter(), &label, theme::ui(12.0), color, (right - x).max(10.0));
                place(ui.painter(), Rect::from_min_max(Pos2::new(x, r.min.y), Pos2::new(right, r.max.y)), Align2::LEFT_CENTER, g, color);
                if item.enabled && resp.clicked() {
                    chosen = Some(i);
                }
            }
        });
    });
    if chosen.is_none() && (ctx.input(|i| i.key_pressed(Key::Escape)) || super::menus::closes_on_outside(ctx, menu_rect, anchor)) {
        acts.push(if is_history_menu { Act::CloseHistoryMenu } else { Act::CloseMenu });
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
    // O OpenCode lista todos os provedores conectados (`opencode`, `opencode-go`…): o menu
    // pede o provedor antes e mostra só os modelos dele. Os outros agentes têm uma lista só.
    let groups = models::by_provider(&known);
    let sole = models::sole_provider(&groups).map(str::to_owned);
    let step = deck.tasks.form.provider.clone().filter(|p| groups.iter().any(|(name, _)| name == p)).or(sole.clone());
    let model_label = deck.tasks.form.model.clone().unwrap_or_else(|| "Padrão".to_owned());
    // Esforço: o que o agente (e o modelo escolhido) permite; sem isso o seletor fica desligado, com a dica.
    let effort_off = models::effort_unavailable(&kind, deck.tasks.form.model.as_deref());
    let effort_at = if effort_off.is_some() { 0 } else { effort_index(deck.tasks.form.effort) };
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
            field_label(ui, "Esforço");
            let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), Sense::hover());
            let picked = ui
                .scope(|ui| {
                    if effort_off.is_some() {
                        ui.set_opacity(0.4);
                    }
                    segmented(ui, r, Id::new("tasks-new-effort"), &EFFORT_LABELS, effort_at, true)
                })
                .inner;
            match effort_off {
                Some(why) => {
                    // Cobre o seletor: sem clique nem destaque ao passar o mouse, só a dica.
                    tip(ui.interact(r, Id::new("tasks-new-effort-off"), Sense::hover()), why);
                }
                None => {
                    if let Some(i) = picked {
                        acts.push(Act::SetEffort(effort_of(i)));
                    }
                }
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
                if start_now { theme::accent() } else { theme::pal().recess },
                Stroke::new(1.0, if start_now { theme::accent() } else { theme::pal().separator }),
                egui::StrokeKind::Inside,
            );
            if start_now {
                text_in(ui.painter(), cb, Align2::CENTER_CENTER, "✓", theme::ui_bold(11.0), theme::pal().on_accent);
            }
            text_in(
                ui.painter(),
                Rect::from_min_max(Pos2::new(cb.max.x + 8.0, r.min.y), r.max),
                Align2::LEFT_CENTER,
                "Iniciar agora",
                theme::ui(12.0),
                if rr.hovered() { theme::pal().text } else { theme::pal().text_2 },
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
            if let Some(i) = dropdown_menu(&ctx, "tasks-menu-agent", a, &agent_items, acts, false) {
                acts.push(Act::SetKind(choices[i].clone()));
            }
        }
        (Some(FormMenu::Model), [_, Some(a)]) => {
            if let Some(i) = dropdown_menu(&ctx, "tasks-menu-model", a, &model_items, acts, false) {
                if let Some((_, act)) = rows.into_iter().nth(i) {
                    acts.push(act);
                }
            }
        }
        _ => {}
    }
}
