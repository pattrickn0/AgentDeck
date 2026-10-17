//! Pintura da aba "GitHub" na barra direita (~330 px):
//! - Cabeçalho com `dono/repo` (abre no navegador), indicador de autenticação/rate-limit e botão Atualizar;
//! - Sub-abas `PRs`, `Issues` e `Actions`;
//! - Detalhe de PR com checks/jobs, fim do log do job que falhou (rolagem horizontal e vertical,
//!   como o diff do Git) e ações "Abrir no navegador" e "Enviar para agente ▾".

use commanddeck_core::github::{
    format_failed_job_prompt, format_issue_prompt, format_pr_prompt, CheckStatus, GithubOverview, Issue, PullRequest, WorkflowJob,
    WorkflowRun,
};
use egui::{Align2, Area, Color32, Id, Key, Order, Pos2, Rect, ScrollArea, Sense, Stroke, TextEdit, Ui, Vec2};

use super::{clickable, dot, frame, galley, galley_trunc, place, shadow, text_in, text_w, tip};
use crate::deck::Deck;
use crate::github_ui::{GithubTab, GithubView};
use crate::icons::Icon;
use crate::theme;

const PAD: f32 = 10.0;
const REPO_BAR_H: f32 = 34.0;
const SUBTABS_H: f32 = 28.0;
const ROW_H: f32 = 42.0;

pub fn draw(deck: &mut Deck, ui: &mut Ui, rect: Rect) {
    ui.painter().rect_filled(rect, 0.0, theme::BG);
    let view = deck.github.view.clone();
    match view {
        GithubView::Loading => {
            draw_top_Placeholder(deck, ui, rect, "Consultando repositório GitHub…");
        }
        GithubView::NoProject => {
            draw_empty_state(
                deck,
                ui,
                rect,
                "Nenhum projeto aberto",
                "Abra uma pasta com repositório do GitHub para ver Pull Requests, Issues e Actions.",
                false,
            );
        }
        GithubView::NotGithub { reason } => {
            draw_empty_state(deck, ui, rect, "Sem remoto do GitHub", &reason, true);
        }
        GithubView::Error { repo, error } => {
            let title = repo.as_ref().map(|r| r.to_string()).unwrap_or_else(|| "GitHub".to_owned());
            draw_empty_state(deck, ui, rect, &title, &error.message_pt(), true);
        }
        GithubView::Ready(overview) => {
            draw_ready(deck, ui, rect, &overview);
        }
    }

    let ctx = ui.ctx().clone();
    draw_agent_menu(deck, &ctx);
}

#[allow(non_snake_case)]
fn draw_top_Placeholder(deck: &mut Deck, ui: &mut Ui, rect: Rect, msg: &str) {
    let p = ui.painter_at(rect);
    text_in(&p, rect, Align2::CENTER_CENTER, msg, theme::ui(12.0), theme::MUTED);
    let _ = deck;
}

fn draw_empty_state(deck: &mut Deck, ui: &mut Ui, rect: Rect, title: &str, detail: &str, can_retry: bool) {
    let p = ui.painter_at(rect);
    let top = Rect::from_min_size(rect.min, Vec2::new(rect.width(), REPO_BAR_H));
    p.rect_filled(top, 0.0, theme::PANEL);
    p.rect_filled(Rect::from_min_max(Pos2::new(top.min.x, top.max.y - 1.0), top.max), 0.0, theme::BORDER);
    deck.icons.paint(&p, Icon::GitHub, Pos2::new(top.min.x + PAD + 7.0, top.center().y), 14.0, theme::MUTED, 0.0);
    let title_rect = Rect::from_min_max(Pos2::new(top.min.x + PAD + 20.0, top.min.y), Pos2::new(top.max.x - 36.0, top.max.y));
    let g = galley_trunc(&p, title, theme::ui_bold(12.0), theme::TEXT, title_rect.width());
    place(&p, title_rect, Align2::LEFT_CENTER, g, theme::TEXT);

    if can_retry {
        let btn = Rect::from_min_size(Pos2::new(top.max.x - PAD - 24.0, top.center().y - 12.0), Vec2::splat(24.0));
        if icon_btn(deck, ui, &p, btn, "gh-retry-top", Icon::Refresh, "Tentar novamente", !deck.github.refreshing) {
            deck.github_refresh();
        }
    }

    let body = Rect::from_min_max(Pos2::new(rect.min.x + 18.0, top.max.y + 28.0), Pos2::new(rect.max.x - 18.0, rect.max.y - 18.0));
    let mut job = egui::text::LayoutJob::simple(detail.to_owned(), theme::ui(12.0), theme::MUTED, body.width().max(80.0));
    job.halign = egui::Align::Center;
    let g = p.layout_job(job);
    let msg_rect = Align2::CENTER_TOP.align_size_within_rect(g.size(), body);
    p.galley(msg_rect.min, g, theme::MUTED);
}

fn draw_ready(deck: &mut Deck, ui: &mut Ui, rect: Rect, ov: &GithubOverview) {
    let mut y = rect.min.y;

    // 1. Cabeçalho do repositório (dono/repo + status de login + atualizar)
    let repo_bar = Rect::from_min_size(Pos2::new(rect.min.x, y), Vec2::new(rect.width(), REPO_BAR_H));
    draw_repo_bar(deck, ui, repo_bar, ov);
    y = repo_bar.max.y;

    // 2. Faixa de orientação quando não há credencial (acesso público de 60/h)
    if !ov.authenticated {
        let auth_h = 22.0;
        let strip = Rect::from_min_size(Pos2::new(rect.min.x, y), Vec2::new(rect.width(), auth_h));
        let p = ui.painter_at(strip);
        p.rect_filled(strip, 0.0, theme::PANEL_2);
        p.rect_filled(Rect::from_min_max(Pos2::new(strip.min.x, strip.max.y - 1.0), strip.max), 0.0, theme::BORDER);
        let txt = match &ov.rate_limit {
            Some(rl) => format!("Sem login ({}/{}/h) · Faça push no Git ou use gh auth", rl.remaining, rl.limit),
            None => "Sem login (60/h) · Faça push no Git ou instale o gh".to_owned(),
        };
        let inner = strip.shrink2(Vec2::new(PAD, 0.0));
        let g = galley_trunc(&p, &txt, theme::ui(10.5), theme::MUTED, inner.width());
        place(&p, inner, Align2::LEFT_CENTER, g, theme::MUTED);
        let r = ui.interact(strip, Id::new("gh-anon-strip"), Sense::hover());
        tip(
            r,
            "Repositório público acessado sem token (limite de 60 req/h).\nPara entrar sem guardar token no app: faça um push pela aba Git (abre o Git Credential Manager) ou autentique o GitHub CLI (gh auth login).",
        );
        y = strip.max.y;
    }

    // 3. Faixa de aviso / erro recente
    if let Some((msg, is_err)) = deck.github.notice.clone() {
        let notice_h = 24.0;
        let strip = Rect::from_min_size(Pos2::new(rect.min.x, y), Vec2::new(rect.width(), notice_h));
        let p = ui.painter_at(strip);
        let bg = if is_err { theme::rgba(0xef, 0x44, 0x44, 0.14) } else { theme::PANEL_2 };
        let fg = if is_err { theme::RED_TEXT } else { theme::TEXT };
        p.rect_filled(strip, 0.0, bg);
        p.rect_filled(Rect::from_min_max(Pos2::new(strip.min.x, strip.max.y - 1.0), strip.max), 0.0, theme::BORDER);
        let close_r = Rect::from_min_size(Pos2::new(strip.max.x - 22.0, strip.min.y + 2.0), Vec2::splat(20.0));
        let txt_r = Rect::from_min_max(Pos2::new(strip.min.x + PAD, strip.min.y), Pos2::new(close_r.min.x - 4.0, strip.max.y));
        let g = galley_trunc(&p, &msg, theme::ui(11.0), fg, txt_r.width());
        place(&p, txt_r, Align2::LEFT_CENTER, g, fg);
        let cr = clickable(ui, close_r, Id::new("gh-notice-close"));
        deck.icons.paint(&p, Icon::Close, close_r.center(), 12.0, if cr.hovered() { theme::TEXT } else { theme::MUTED }, 0.0);
        if cr.clicked() {
            deck.github.notice = None;
        }
        y = strip.max.y;
    }

    // 4. Sub-abas: PRs | Issues | Actions
    let subtabs = Rect::from_min_size(Pos2::new(rect.min.x, y), Vec2::new(rect.width(), SUBTABS_H));
    draw_subtabs(deck, ui, subtabs, ov);
    y = subtabs.max.y;

    // 5. Corpo da sub-aba ativa
    let body = Rect::from_min_max(Pos2::new(rect.min.x, y), rect.max);
    if body.height() <= 4.0 {
        return;
    }
    match deck.github.tab {
        GithubTab::Prs => draw_prs_tab(deck, ui, body, ov),
        GithubTab::Issues => draw_issues_tab(deck, ui, body, ov),
        GithubTab::Actions => draw_actions_tab(deck, ui, body, ov),
    }
}

fn draw_repo_bar(deck: &mut Deck, ui: &mut Ui, rect: Rect, ov: &GithubOverview) {
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, theme::PANEL);
    p.rect_filled(Rect::from_min_max(Pos2::new(rect.min.x, rect.max.y - 1.0), rect.max), 0.0, theme::BORDER);

    // Botão Atualizar à direita
    let refresh_r = Rect::from_min_size(Pos2::new(rect.max.x - PAD - 24.0, rect.center().y - 12.0), Vec2::splat(24.0));
    let refresh_tip = match &ov.rate_limit {
        Some(rl) => format!("Atualizar dados do GitHub ({}/{} req/h restantes)", rl.remaining, rl.limit),
        None => "Atualizar dados do GitHub".to_owned(),
    };
    if icon_btn(deck, ui, &p, refresh_r, "gh-refresh", Icon::Refresh, &refresh_tip, !deck.github.refreshing) {
        deck.github_refresh();
    }

    // Botão "Abrir repositório no navegador"
    let open_w = 54.0;
    let open_r = Rect::from_min_size(Pos2::new(refresh_r.min.x - 4.0 - open_w, rect.center().y - 11.0), Vec2::new(open_w, 22.0));
    if small_btn(ui, &p, open_r, "gh-open-repo", "Abrir ↗", &format!("Abrir {} no navegador", ov.repo.html_url())) {
        deck.open_github_browser(&ov.repo.html_url());
    }

    // Nome dono/repo à esquerda (também clicável)
    let left_max = open_r.min.x - 6.0;
    let slug_r = Rect::from_min_max(Pos2::new(rect.min.x + PAD, rect.min.y + 4.0), Pos2::new(left_max, rect.max.y - 4.0));
    let sr = clickable(ui, slug_r, Id::new("gh-repo-slug"));
    let icon_color = if ov.authenticated { theme::GREEN } else { theme::MUTED };
    dot(&p, Pos2::new(slug_r.min.x + 4.0, slug_r.center().y), icon_color);
    let txt_r = Rect::from_min_max(Pos2::new(slug_r.min.x + 12.0, slug_r.min.y), slug_r.max);
    let slug_txt = ov.repo.to_string();
    let g = galley_trunc(&p, &slug_txt, theme::ui_bold(12.0), if sr.hovered() { theme::WHITE } else { theme::TEXT }, txt_r.width());
    place(&p, txt_r, Align2::LEFT_CENTER, g, theme::TEXT);
    let auth_label = if ov.authenticated { "Autenticado via GCM/gh" } else { "Modo público (sem token)" };
    if tip(sr, format!("{} · {auth_label}\nClique para abrir no navegador", ov.repo.html_url())).clicked() {
        deck.open_github_browser(&ov.repo.html_url());
    }
}

fn draw_subtabs(deck: &mut Deck, ui: &mut Ui, rect: Rect, ov: &GithubOverview) {
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, theme::PANEL);
    p.rect_filled(Rect::from_min_max(Pos2::new(rect.min.x, rect.max.y - 1.0), rect.max), 0.0, theme::BORDER);

    let items = [
        (GithubTab::Prs, format!("PRs ({})", ov.pulls.len())),
        (GithubTab::Issues, format!("Issues ({})", ov.issues.len())),
        (GithubTab::Actions, format!("Actions ({})", ov.runs.len())),
    ];
    let tab_w = (rect.width() - PAD * 2.0) / items.len() as f32;
    let active_color = deck.active_color();
    for (i, (tab, label)) in items.iter().enumerate() {
        let r = Rect::from_min_size(Pos2::new(rect.min.x + PAD + i as f32 * tab_w, rect.min.y), Vec2::new(tab_w, rect.height() - 1.0));
        let active = deck.github.tab == *tab;
        let resp = clickable(ui, r, Id::new(("gh-subtab", i)));
        if active || resp.hovered() {
            p.rect_filled(r.shrink2(Vec2::new(2.0, 3.0)), 4.0, if active { theme::PANEL_2 } else { theme::BG });
        }
        let fg = if active || resp.hovered() { theme::TEXT } else { theme::MUTED };
        let font = if active { theme::ui_bold(11.5) } else { theme::ui(11.5) };
        text_in(&p, r, Align2::CENTER_CENTER, label, font, fg);
        if active {
            let line = Rect::from_min_max(Pos2::new(r.min.x + 8.0, r.max.y - 2.0), Pos2::new(r.max.x - 8.0, r.max.y));
            p.rect_filled(line, 1.0, active_color);
        }
        if resp.clicked() {
            deck.github.tab = *tab;
        }
    }
}

// ---------------------------------------------------------------------------
// Sub-aba 1: Pull Requests + Detalhe de PR (checks + log do job vermelho)
// ---------------------------------------------------------------------------

fn draw_prs_tab(deck: &mut Deck, ui: &mut Ui, rect: Rect, ov: &GithubOverview) {
    if ov.pulls.is_empty() {
        let p = ui.painter_at(rect);
        text_in(&p, rect, Align2::CENTER_CENTER, "Nenhum pull request aberto.", theme::ui(12.0), theme::MUTED);
        return;
    }

    let selected = deck.github.selected_pr.and_then(|num| ov.pulls.iter().find(|pr| pr.number == num)).cloned();
    let (list_rect, detail_rect) = match &selected {
        Some(_) => {
            // Divide verticalmente: lista compacta em cima e detalhe do PR (checks + log) embaixo.
            let detail_h = (rect.height() * 0.62).clamp(180.0, (rect.height() - 86.0).max(140.0));
            let split_y = (rect.max.y - detail_h).max(rect.min.y + 44.0);
            (
                Rect::from_min_max(rect.min, Pos2::new(rect.max.x, split_y)),
                Some(Rect::from_min_max(Pos2::new(rect.min.x, split_y), rect.max)),
            )
        }
        None => (rect, None),
    };

    // Lista de PRs abertos
    let mut clicked_pr = None;
    let active_num = deck.github.selected_pr;
    ui.scope_builder(egui::UiBuilder::new().max_rect(list_rect), |ui| {
        ScrollArea::vertical().id_salt("gh-prs-scroll").auto_shrink([false, false]).show(ui, |ui| {
            for pr in &ov.pulls {
                let (row, resp) = ui.allocate_exact_size(Vec2::new(list_rect.width(), ROW_H), Sense::click());
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                let p = ui.painter_at(row);
                let is_sel = active_num == Some(pr.number);
                if is_sel || resp.hovered() {
                    p.rect_filled(row, 0.0, if is_sel { theme::PANEL_2 } else { theme::PANEL });
                }
                p.rect_filled(Rect::from_min_max(Pos2::new(row.min.x, row.max.y - 1.0), row.max), 0.0, theme::BORDER);
                draw_pr_row(&p, row, pr, is_sel);
                if resp.clicked() {
                    clicked_pr = Some(pr.number);
                }
            }
        });
    });

    if let Some(num) = clicked_pr {
        deck.github_select_pr(num);
    }

    if let (Some(pr), Some(drect)) = (selected, detail_rect) {
        draw_pr_detail(deck, ui, drect, ov, &pr);
    }
}

fn draw_pr_row(p: &egui::Painter, row: Rect, pr: &PullRequest, selected: bool) {
    let top_y = row.min.y + 4.0;
    let bot_y = row.min.y + 22.0;

    // Bolinha / ícone de status dos checks
    paint_check_badge(p, Pos2::new(row.min.x + PAD + 5.0, top_y + 8.0), pr.checks);

    // #número
    let num_txt = format!("#{}", pr.number);
    let num_font = theme::mono(11.0);
    let num_w = text_w(p, &num_txt, num_font.clone());
    let num_r = Rect::from_min_size(Pos2::new(row.min.x + PAD + 14.0, top_y), Vec2::new(num_w, 16.0));
    text_in(p, num_r, Align2::LEFT_CENTER, &num_txt, num_font, theme::MUTED);

    let mut x = num_r.max.x + 6.0;
    if pr.draft {
        let pill_w = 54.0;
        let pill = Rect::from_min_size(Pos2::new(x, top_y + 1.0), Vec2::new(pill_w, 14.0));
        frame(p, pill, 3.0, theme::PANEL_2, theme::BORDER);
        text_in(p, pill, Align2::CENTER_CENTER, "RASCUNHO", theme::ui_bold(9.0), theme::MUTED);
        x = pill.max.x + 5.0;
    }

    // Título do PR (truncado para ~330 px)
    let title_r = Rect::from_min_max(Pos2::new(x, top_y), Pos2::new(row.max.x - PAD, top_y + 16.0));
    let title_font = if selected { theme::ui_bold(12.0) } else { theme::ui(12.0) };
    let g = galley_trunc(p, &pr.title, title_font, theme::TEXT, title_r.width());
    place(p, title_r, Align2::LEFT_CENTER, g, theme::TEXT);

    // Linha 2: branch `head → base` à esquerda, `@autor` à direita
    let author_txt = format!("@{}", pr.author);
    let author_font = theme::ui(10.5);
    let author_w = text_w(p, &author_txt, author_font.clone());
    let author_r = Rect::from_min_size(Pos2::new(row.max.x - PAD - author_w, bot_y), Vec2::new(author_w, 15.0));
    text_in(p, author_r, Align2::RIGHT_CENTER, &author_txt, author_font, theme::MUTED);

    let branch_txt = format!("{} → {}", pr.head_ref, pr.base_ref);
    let branch_r = Rect::from_min_max(Pos2::new(row.min.x + PAD + 14.0, bot_y), Pos2::new(author_r.min.x - 6.0, bot_y + 15.0));
    let gb = galley_trunc(p, &branch_txt, theme::mono(10.5), theme::MUTED, branch_r.width());
    place(p, branch_r, Align2::LEFT_CENTER, gb, theme::MUTED);
}

fn draw_pr_detail(deck: &mut Deck, ui: &mut Ui, rect: Rect, ov: &GithubOverview, pr: &PullRequest) {
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, theme::PANEL);
    p.rect_filled(Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.min.y + 1.0)), 0.0, theme::BORDER);

    // Cabeçalho do detalhe: PR #N + título + fechar
    let head_h = 26.0;
    let head = Rect::from_min_size(Pos2::new(rect.min.x, rect.min.y + 1.0), Vec2::new(rect.width(), head_h));
    p.rect_filled(head, 0.0, theme::PANEL_2);
    p.rect_filled(Rect::from_min_max(Pos2::new(head.min.x, head.max.y - 1.0), head.max), 0.0, theme::BORDER);

    let close_r = Rect::from_min_size(Pos2::new(head.max.x - PAD - 20.0, head.center().y - 10.0), Vec2::splat(20.0));
    let cr = clickable(ui, close_r, Id::new("gh-pr-detail-close"));
    deck.icons.paint(&p, Icon::Close, close_r.center(), 13.0, if cr.hovered() { theme::TEXT } else { theme::MUTED }, 0.0);
    if tip(cr, "Fechar detalhe do PR").clicked() {
        deck.github.selected_pr = None;
        return;
    }

    paint_check_badge(&p, Pos2::new(head.min.x + PAD + 5.0, head.center().y), pr.checks);
    let head_txt = format!("PR #{} · {}", pr.number, pr.title);
    let head_r = Rect::from_min_max(Pos2::new(head.min.x + PAD + 14.0, head.min.y), Pos2::new(close_r.min.x - 4.0, head.max.y));
    let gh = galley_trunc(&p, &head_txt, theme::ui_bold(11.5), theme::TEXT, head_r.width());
    place(&p, head_r, Align2::LEFT_CENTER, gh, theme::TEXT);

    // Barra de ações do PR: "Abrir no navegador" e "Enviar para agente ▾"
    let act_h = 28.0;
    let act = Rect::from_min_size(Pos2::new(rect.min.x, head.max.y), Vec2::new(rect.width(), act_h));
    p.rect_filled(act, 0.0, theme::PANEL);
    p.rect_filled(Rect::from_min_max(Pos2::new(act.min.x, act.max.y - 1.0), act.max), 0.0, theme::BORDER);

    let half_w = (act.width() - PAD * 2.0 - 6.0) * 0.5;
    let btn_open = Rect::from_min_size(Pos2::new(act.min.x + PAD, act.center().y - 11.0), Vec2::new(half_w, 22.0));
    let btn_agent = Rect::from_min_size(Pos2::new(btn_open.max.x + 6.0, act.center().y - 11.0), Vec2::new(half_w, 22.0));

    if small_btn(ui, &p, btn_open, ("gh-pr-open", pr.number), "Abrir no navegador", &pr.html_url) {
        deck.open_github_browser(&pr.html_url);
    }
    if small_btn_accent(ui, &p, btn_agent, ("gh-pr-agent", pr.number), "Enviar p/ agente ▾", "Enviar PR pronto para revisão ou ajuste por um agente") {
        let checks_summary = deck.github.pr_details.get(&pr.number).map(|d| summarize_jobs(&d.jobs));
        let prompt = format_pr_prompt(&ov.repo, pr, checks_summary.as_deref());
        deck.open_github_agent_menu(btn_agent, format!("PR #{}", pr.number), "review", prompt);
    }

    // Área inferior do detalhe: lista de checks/jobs e (se houver job falho aberto) o log com rolagem dupla
    let content = Rect::from_min_max(Pos2::new(rect.min.x, act.max.y), rect.max);
    if content.height() <= 8.0 {
        return;
    }

    let detail = deck.github.pr_details.get(&pr.number).cloned();
    let loading = deck.github.pr_loading == Some(pr.number);
    let jobs = detail.as_ref().map(|d| d.jobs.as_slice()).unwrap_or(&[]);

    draw_jobs_and_log(deck, ui, content, ov, Some(pr.number), jobs, loading);
}

// ---------------------------------------------------------------------------
// Sub-aba 2: Issues (sem PRs)
// ---------------------------------------------------------------------------

fn draw_issues_tab(deck: &mut Deck, ui: &mut Ui, rect: Rect, ov: &GithubOverview) {
    if ov.issues.is_empty() {
        let p = ui.painter_at(rect);
        text_in(&p, rect, Align2::CENTER_CENTER, "Nenhuma issue aberta.", theme::ui(12.0), theme::MUTED);
        return;
    }

    let selected = deck.github.selected_issue.and_then(|num| ov.issues.iter().find(|i| i.number == num)).cloned();
    let (list_rect, detail_rect) = match &selected {
        Some(_) => {
            let detail_h = (rect.height() * 0.45).clamp(130.0, (rect.height() - 80.0).max(110.0));
            let split_y = (rect.max.y - detail_h).max(rect.min.y + 44.0);
            (
                Rect::from_min_max(rect.min, Pos2::new(rect.max.x, split_y)),
                Some(Rect::from_min_max(Pos2::new(rect.min.x, split_y), rect.max)),
            )
        }
        None => (rect, None),
    };

    let mut clicked_issue = None;
    let active_num = deck.github.selected_issue;
    ui.scope_builder(egui::UiBuilder::new().max_rect(list_rect), |ui| {
        ScrollArea::vertical().id_salt("gh-issues-scroll").auto_shrink([false, false]).show(ui, |ui| {
            for issue in &ov.issues {
                let (row, resp) = ui.allocate_exact_size(Vec2::new(list_rect.width(), ROW_H), Sense::click());
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                let p = ui.painter_at(row);
                let is_sel = active_num == Some(issue.number);
                if is_sel || resp.hovered() {
                    p.rect_filled(row, 0.0, if is_sel { theme::PANEL_2 } else { theme::PANEL });
                }
                p.rect_filled(Rect::from_min_max(Pos2::new(row.min.x, row.max.y - 1.0), row.max), 0.0, theme::BORDER);
                draw_issue_row(&p, row, issue, is_sel);
                if resp.clicked() {
                    clicked_issue = Some(issue.number);
                }
            }
        });
    });

    if let Some(num) = clicked_issue {
        deck.github.selected_issue = if deck.github.selected_issue == Some(num) { None } else { Some(num) };
    }

    if let (Some(issue), Some(drect)) = (selected, detail_rect) {
        draw_issue_detail(deck, ui, drect, ov, &issue);
    }
}

fn draw_issue_row(p: &egui::Painter, row: Rect, issue: &Issue, selected: bool) {
    let top_y = row.min.y + 4.0;
    let bot_y = row.min.y + 22.0;

    dot(p, Pos2::new(row.min.x + PAD + 5.0, top_y + 8.0), theme::GREEN);

    let num_txt = format!("#{}", issue.number);
    let num_font = theme::mono(11.0);
    let num_w = text_w(p, &num_txt, num_font.clone());
    let num_r = Rect::from_min_size(Pos2::new(row.min.x + PAD + 14.0, top_y), Vec2::new(num_w, 16.0));
    text_in(p, num_r, Align2::LEFT_CENTER, &num_txt, num_font, theme::MUTED);

    let title_r = Rect::from_min_max(Pos2::new(num_r.max.x + 6.0, top_y), Pos2::new(row.max.x - PAD, top_y + 16.0));
    let font = if selected { theme::ui_bold(12.0) } else { theme::ui(12.0) };
    let g = galley_trunc(p, &issue.title, font, theme::TEXT, title_r.width());
    place(p, title_r, Align2::LEFT_CENTER, g, theme::TEXT);

    let mut sub = format!("@{}", issue.author);
    if !issue.labels.is_empty() {
        sub.push_str(" · ");
        sub.push_str(&issue.labels.join(", "));
    }
    if issue.comments > 0 {
        sub.push_str(&format!(" · {} coment.", issue.comments));
    }
    let sub_r = Rect::from_min_max(Pos2::new(row.min.x + PAD + 14.0, bot_y), Pos2::new(row.max.x - PAD, bot_y + 15.0));
    let gs = galley_trunc(p, &sub, theme::ui(10.5), theme::MUTED, sub_r.width());
    place(p, sub_r, Align2::LEFT_CENTER, gs, theme::MUTED);
}

fn draw_issue_detail(deck: &mut Deck, ui: &mut Ui, rect: Rect, ov: &GithubOverview, issue: &Issue) {
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, theme::PANEL);
    p.rect_filled(Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.min.y + 1.0)), 0.0, theme::BORDER);

    let head_h = 26.0;
    let head = Rect::from_min_size(Pos2::new(rect.min.x, rect.min.y + 1.0), Vec2::new(rect.width(), head_h));
    p.rect_filled(head, 0.0, theme::PANEL_2);
    p.rect_filled(Rect::from_min_max(Pos2::new(head.min.x, head.max.y - 1.0), head.max), 0.0, theme::BORDER);

    let close_r = Rect::from_min_size(Pos2::new(head.max.x - PAD - 20.0, head.center().y - 10.0), Vec2::splat(20.0));
    let cr = clickable(ui, close_r, Id::new("gh-issue-close"));
    deck.icons.paint(&p, Icon::Close, close_r.center(), 13.0, if cr.hovered() { theme::TEXT } else { theme::MUTED }, 0.0);
    if cr.clicked() {
        deck.github.selected_issue = None;
        return;
    }

    let head_txt = format!("Issue #{} · {}", issue.number, issue.title);
    let head_r = Rect::from_min_max(Pos2::new(head.min.x + PAD, head.min.y), Pos2::new(close_r.min.x - 4.0, head.max.y));
    let gh = galley_trunc(&p, &head_txt, theme::ui_bold(11.5), theme::TEXT, head_r.width());
    place(&p, head_r, Align2::LEFT_CENTER, gh, theme::TEXT);

    let act_h = 28.0;
    let act = Rect::from_min_size(Pos2::new(rect.min.x, head.max.y), Vec2::new(rect.width(), act_h));
    p.rect_filled(act, 0.0, theme::PANEL);
    p.rect_filled(Rect::from_min_max(Pos2::new(act.min.x, act.max.y - 1.0), act.max), 0.0, theme::BORDER);

    let half_w = (act.width() - PAD * 2.0 - 6.0) * 0.5;
    let btn_open = Rect::from_min_size(Pos2::new(act.min.x + PAD, act.center().y - 11.0), Vec2::new(half_w, 22.0));
    let btn_agent = Rect::from_min_size(Pos2::new(btn_open.max.x + 6.0, act.center().y - 11.0), Vec2::new(half_w, 22.0));

    if small_btn(ui, &p, btn_open, ("gh-issue-open", issue.number), "Abrir no navegador", &issue.html_url) {
        deck.open_github_browser(&issue.html_url);
    }
    if small_btn_accent(ui, &p, btn_agent, ("gh-issue-agent", issue.number), "Enviar p/ agente ▾", "Enviar issue pronta para um agente analisar ou resolver") {
        let prompt = format_issue_prompt(&ov.repo, issue);
        deck.open_github_agent_menu(btn_agent, format!("Issue #{}", issue.number), "dev", prompt);
    }

    let body_r = Rect::from_min_max(Pos2::new(rect.min.x + PAD, act.max.y + 4.0), Pos2::new(rect.max.x - PAD, rect.max.y - 4.0));
    if body_r.height() > 8.0 {
        let desc = if issue.body.trim().is_empty() { "Sem descrição." } else { issue.body.trim() };
        ui.scope_builder(egui::UiBuilder::new().max_rect(body_r), |ui| {
            ScrollArea::vertical().id_salt(("gh-issue-body", issue.number)).auto_shrink([false, false]).show(ui, |ui| {
                ui.label(egui::RichText::new(desc).font(theme::ui(11.5)).color(theme::TEXT));
            });
        });
    }
}

// ---------------------------------------------------------------------------
// Sub-aba 3: Actions (execuções de workflow, jobs e log do job vermelho)
// ---------------------------------------------------------------------------

fn draw_actions_tab(deck: &mut Deck, ui: &mut Ui, rect: Rect, ov: &GithubOverview) {
    if ov.runs.is_empty() {
        let p = ui.painter_at(rect);
        text_in(&p, rect, Align2::CENTER_CENTER, "Nenhuma execução do GitHub Actions encontrada.", theme::ui(12.0), theme::MUTED);
        return;
    }

    let selected = deck.github.selected_run.and_then(|id| ov.runs.iter().find(|r| r.id == id)).cloned();
    let (list_rect, detail_rect) = match &selected {
        Some(_) => {
            let detail_h = (rect.height() * 0.58).clamp(170.0, (rect.height() - 84.0).max(140.0));
            let split_y = (rect.max.y - detail_h).max(rect.min.y + 44.0);
            (
                Rect::from_min_max(rect.min, Pos2::new(rect.max.x, split_y)),
                Some(Rect::from_min_max(Pos2::new(rect.min.x, split_y), rect.max)),
            )
        }
        None => (rect, None),
    };

    let mut clicked_run = None;
    let mut open_url = None;
    let active_id = deck.github.selected_run;
    ui.scope_builder(egui::UiBuilder::new().max_rect(list_rect), |ui| {
        ScrollArea::vertical().id_salt("gh-runs-scroll").auto_shrink([false, false]).show(ui, |ui| {
            for run in &ov.runs {
                let (row, resp) = ui.allocate_exact_size(Vec2::new(list_rect.width(), ROW_H), Sense::click());
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                let p = ui.painter_at(row);
                let is_sel = active_id == Some(run.id);
                if is_sel || resp.hovered() {
                    p.rect_filled(row, 0.0, if is_sel { theme::PANEL_2 } else { theme::PANEL });
                }
                p.rect_filled(Rect::from_min_max(Pos2::new(row.min.x, row.max.y - 1.0), row.max), 0.0, theme::BORDER);

                let btn_r = Rect::from_min_size(Pos2::new(row.max.x - PAD - 22.0, row.center().y - 10.0), Vec2::new(22.0, 20.0));
                if small_btn(ui, &p, btn_r, ("gh-run-open", run.id), "↗", "Abrir execução no navegador") {
                    open_url = Some(run.html_url.clone());
                } else if resp.clicked() {
                    clicked_run = Some(run.id);
                }
                let text_row = Rect::from_min_max(row.min, Pos2::new(btn_r.min.x - 4.0, row.max.y));
                draw_run_row(&p, text_row, run, is_sel);
            }
        });
    });

    if let Some(url) = open_url {
        deck.open_github_browser(&url);
    }
    if let Some(id) = clicked_run {
        deck.github_select_run(id);
    }

    if let (Some(run), Some(drect)) = (selected, detail_rect) {
        let p = ui.painter_at(drect);
        p.rect_filled(drect, 0.0, theme::PANEL);
        p.rect_filled(Rect::from_min_max(drect.min, Pos2::new(drect.max.x, drect.min.y + 1.0)), 0.0, theme::BORDER);

        let head_h = 26.0;
        let head = Rect::from_min_size(Pos2::new(drect.min.x, drect.min.y + 1.0), Vec2::new(drect.width(), head_h));
        p.rect_filled(head, 0.0, theme::PANEL_2);
        p.rect_filled(Rect::from_min_max(Pos2::new(head.min.x, head.max.y - 1.0), head.max), 0.0, theme::BORDER);

        let close_r = Rect::from_min_size(Pos2::new(head.max.x - PAD - 20.0, head.center().y - 10.0), Vec2::splat(20.0));
        let cr = clickable(ui, close_r, Id::new("gh-run-close"));
        deck.icons.paint(&p, Icon::Close, close_r.center(), 13.0, if cr.hovered() { theme::TEXT } else { theme::MUTED }, 0.0);
        if cr.clicked() {
            deck.github.selected_run = None;
            return;
        }

        paint_check_badge(&p, Pos2::new(head.min.x + PAD + 5.0, head.center().y), run.status);
        let title = format!("{} #{} · {}", run.name, run.run_number, run.head_branch);
        let head_r = Rect::from_min_max(Pos2::new(head.min.x + PAD + 14.0, head.min.y), Pos2::new(close_r.min.x - 4.0, head.max.y));
        let gh = galley_trunc(&p, &title, theme::ui_bold(11.5), theme::TEXT, head_r.width());
        place(&p, head_r, Align2::LEFT_CENTER, gh, theme::TEXT);

        let content = Rect::from_min_max(Pos2::new(drect.min.x, head.max.y), drect.max);
        let jobs = deck.github.run_jobs.get(&run.id).cloned().unwrap_or_default();
        let loading = deck.github.run_loading == Some(run.id);
        draw_jobs_and_log(deck, ui, content, ov, None, &jobs, loading);
    }
}

fn draw_run_row(p: &egui::Painter, row: Rect, run: &WorkflowRun, selected: bool) {
    let top_y = row.min.y + 4.0;
    let bot_y = row.min.y + 22.0;

    paint_check_badge(p, Pos2::new(row.min.x + PAD + 5.0, top_y + 8.0), run.status);

    let title_r = Rect::from_min_max(Pos2::new(row.min.x + PAD + 14.0, top_y), Pos2::new(row.max.x - 4.0, top_y + 16.0));
    let font = if selected { theme::ui_bold(12.0) } else { theme::ui(12.0) };
    let g = galley_trunc(p, &run.display_title, font, theme::TEXT, title_r.width());
    place(p, title_r, Align2::LEFT_CENTER, g, theme::TEXT);

    let sub = format!("{} #{} · {}", run.name, run.run_number, run.head_branch);
    let sub_r = Rect::from_min_max(Pos2::new(row.min.x + PAD + 14.0, bot_y), Pos2::new(row.max.x - 4.0, bot_y + 15.0));
    let gs = galley_trunc(p, &sub, theme::mono(10.5), theme::MUTED, sub_r.width());
    place(p, sub_r, Align2::LEFT_CENTER, gs, theme::MUTED);
}

// ---------------------------------------------------------------------------
// Lista de Checks/Jobs e visualizador de Log do job vermelho (rolagem dupla)
// ---------------------------------------------------------------------------

fn draw_jobs_and_log(
    deck: &mut Deck,
    ui: &mut Ui,
    rect: Rect,
    ov: &GithubOverview,
    pr_number: Option<u64>,
    jobs: &[WorkflowJob],
    loading: bool,
) {
    let active_log = deck
        .github
        .open_log
        .clone()
        .filter(|(job_id, _, log_pr)| *log_pr == pr_number || jobs.iter().any(|j| j.id == *job_id));

    let (jobs_rect, log_rect) = match &active_log {
        Some(_) => {
            let log_h = (rect.height() * 0.56).clamp(96.0, (rect.height() - 52.0).max(80.0));
            let split_y = (rect.max.y - log_h).max(rect.min.y + 42.0);
            (
                Rect::from_min_max(rect.min, Pos2::new(rect.max.x, split_y)),
                Some(Rect::from_min_max(Pos2::new(rect.min.x, split_y), rect.max)),
            )
        }
        None => (rect, None),
    };

    if loading && jobs.is_empty() {
        let p = ui.painter_at(jobs_rect);
        text_in(&p, jobs_rect, Align2::CENTER_CENTER, "Carregando checks e jobs…", theme::ui(11.5), theme::MUTED);
    } else if jobs.is_empty() {
        let p = ui.painter_at(jobs_rect);
        text_in(&p, jobs_rect, Align2::CENTER_CENTER, "Nenhum check/job registrado para este commit.", theme::ui(11.5), theme::MUTED);
    } else {
        let mut req_log: Option<(u64, String)> = None;
        let mut req_agent: Option<(Rect, WorkflowJob)> = None;
        let mut open_url: Option<String> = None;
        let current_log_id = active_log.as_ref().map(|(id, _, _)| *id);

        ui.scope_builder(egui::UiBuilder::new().max_rect(jobs_rect), |ui| {
            ScrollArea::vertical().id_salt(("gh-jobs-scroll", pr_number.unwrap_or(0))).auto_shrink([false, false]).show(ui, |ui| {
                for job in jobs {
                    let failed = job.status == CheckStatus::Failure;
                    let row_h = if failed && !job.failed_steps.is_empty() { 38.0 } else { 24.0 };
                    let (row, _) = ui.allocate_exact_size(Vec2::new(jobs_rect.width(), row_h), Sense::hover());
                    let p = ui.painter_at(row);
                    p.rect_filled(Rect::from_min_max(Pos2::new(row.min.x, row.max.y - 1.0), row.max), 0.0, theme::BORDER);

                    let top_y = row.min.y + 3.0;
                    paint_check_badge(&p, Pos2::new(row.min.x + PAD + 5.0, top_y + 9.0), job.status);

                    let mut right_x = row.max.x - PAD;
                    // Botão abrir job no navegador
                    if !job.html_url.is_empty() {
                        let b = Rect::from_min_size(Pos2::new(right_x - 20.0, top_y), Vec2::new(20.0, 18.0));
                        if small_btn(ui, &p, b, ("gh-job-open", job.id), "↗", "Abrir job no navegador") {
                            open_url = Some(job.html_url.clone());
                        }
                        right_x = b.min.x - 4.0;
                    }
                    if failed {
                        let b_fix = Rect::from_min_size(Pos2::new(right_x - 62.0, top_y), Vec2::new(62.0, 18.0));
                        if small_btn_accent(ui, &p, b_fix, ("gh-job-fix", job.id), "Corrigir ▾", "Enviar falha e fim do log para um agente corrigir") {
                            req_agent = Some((b_fix, job.clone()));
                        }
                        right_x = b_fix.min.x - 4.0;

                        let log_open = current_log_id == Some(job.id);
                        let b_log = Rect::from_min_size(Pos2::new(right_x - 36.0, top_y), Vec2::new(36.0, 18.0));
                        let label = if log_open { "Log ✓" } else { "Log" };
                        if small_btn(ui, &p, b_log, ("gh-job-log", job.id), label, "Ver fim do log deste job") {
                            req_log = Some((job.id, job.name.clone()));
                        }
                        right_x = b_log.min.x - 4.0;
                    }

                    let name_r = Rect::from_min_max(Pos2::new(row.min.x + PAD + 14.0, top_y), Pos2::new(right_x, top_y + 18.0));
                    let name_color = if failed { theme::RED_TEXT } else { theme::TEXT };
                    let gn = galley_trunc(&p, &job.name, theme::ui(11.5), name_color, name_r.width());
                    place(&p, name_r, Align2::LEFT_CENTER, gn, name_color);

                    if failed && !job.failed_steps.is_empty() {
                        let step_txt = format!("↳ Falhou em: {}", job.failed_steps.join(", "));
                        let step_r = Rect::from_min_max(Pos2::new(row.min.x + PAD + 14.0, top_y + 18.0), Pos2::new(row.max.x - PAD, row.max.y - 2.0));
                        let gs = galley_trunc(&p, &step_txt, theme::mono(10.0), theme::RED_TEXT, step_r.width());
                        place(&p, step_r, Align2::LEFT_CENTER, gs, theme::RED_TEXT);
                    }
                }
            });
        });

        if let Some(url) = open_url {
            deck.open_github_browser(&url);
        }
        if let Some((job_id, job_name)) = req_log {
            deck.github_request_job_log(job_id, job_name, pr_number);
        }
        if let Some((anchor, job)) = req_agent {
            // Garante que o log está sendo carregado se ainda não estiver no cache.
            if !deck.github.job_logs.contains_key(&job.id) {
                deck.github_request_job_log(job.id, job.name.clone(), pr_number);
            }
            let log_tail = deck.github.job_logs.get(&job.id).map(|l| l.tail.as_str());
            let pr_ref = pr_number.and_then(|n| ov.pulls.iter().find(|p| p.number == n));
            let prompt = format_failed_job_prompt(&ov.repo, pr_ref, &job, log_tail);
            deck.open_github_agent_menu(anchor, format!("Job \"{}\"", job.name), "ci", prompt);
        }
    }

    // Painel de log do job que falhou (rolagem horizontal + vertical dentro dos ~330 px, como o diff do Git)
    if let (Some((job_id, job_name, _)), Some(lrect)) = (active_log, log_rect) {
        draw_job_log_pane(deck, ui, lrect, ov, pr_number, job_id, &job_name, jobs);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_job_log_pane(
    deck: &mut Deck,
    ui: &mut Ui,
    rect: Rect,
    ov: &GithubOverview,
    pr_number: Option<u64>,
    job_id: u64,
    job_name: &str,
    jobs: &[WorkflowJob],
) {
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, theme::BG);
    p.rect_filled(Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.min.y + 1.0)), 0.0, theme::BORDER);

    let head_h = 22.0;
    let head = Rect::from_min_size(Pos2::new(rect.min.x, rect.min.y + 1.0), Vec2::new(rect.width(), head_h));
    p.rect_filled(head, 0.0, theme::PANEL_2);
    p.rect_filled(Rect::from_min_max(Pos2::new(head.min.x, head.max.y - 1.0), head.max), 0.0, theme::BORDER);

    let close_r = Rect::from_min_size(Pos2::new(head.max.x - PAD - 18.0, head.center().y - 9.0), Vec2::splat(18.0));
    let cr = clickable(ui, close_r, Id::new("gh-log-close"));
    deck.icons.paint(&p, Icon::Close, close_r.center(), 12.0, if cr.hovered() { theme::TEXT } else { theme::MUTED }, 0.0);
    if cr.clicked() {
        deck.github.open_log = None;
        return;
    }

    let send_r = Rect::from_min_size(Pos2::new(close_r.min.x - 4.0 - 94.0, head.center().y - 9.0), Vec2::new(94.0, 18.0));
    if small_btn_accent(ui, &p, send_r, ("gh-log-send", job_id), "Enviar log ▾", "Enviar o erro e este trecho de log para um agente corrigir") {
        let log_tail = deck.github.job_logs.get(&job_id).map(|l| l.tail.as_str());
        let pr_ref = pr_number.and_then(|n| ov.pulls.iter().find(|p| p.number == n));
        let fallback_job = WorkflowJob {
            id: job_id,
            run_id: 0,
            name: job_name.to_owned(),
            status: CheckStatus::Failure,
            html_url: String::new(),
            failed_steps: Vec::new(),
        };
        let job_ref = jobs.iter().find(|j| j.id == job_id).unwrap_or(&fallback_job);
        let prompt = format_failed_job_prompt(&ov.repo, pr_ref, job_ref, log_tail);
        deck.open_github_agent_menu(send_r, format!("Log: {job_name}"), "ci", prompt);
    }

    let title = format!("Log: {job_name}");
    let title_r = Rect::from_min_max(Pos2::new(head.min.x + PAD, head.min.y), Pos2::new(send_r.min.x - 4.0, head.max.y));
    let gt = galley_trunc(&p, &title, theme::mono(10.5), theme::RED_TEXT, title_r.width());
    place(&p, title_r, Align2::LEFT_CENTER, gt, theme::RED_TEXT);

    let body = Rect::from_min_max(Pos2::new(rect.min.x, head.max.y), rect.max);
    if deck.github.log_loading == Some(job_id) && !deck.github.job_logs.contains_key(&job_id) {
        text_in(&p, body, Align2::CENTER_CENTER, "Baixando fim do log do job…", theme::ui(11.0), theme::MUTED);
        return;
    }
    let Some(log) = deck.github.job_logs.get(&job_id).cloned() else {
        text_in(&p, body, Align2::CENTER_CENTER, "Log indisponível.", theme::ui(11.0), theme::MUTED);
        return;
    };

    let font = theme::mono(11.0);
    let line_h = 16.0;
    let lines: Vec<&str> = log.tail.lines().collect();
    let max_w = lines
        .iter()
        .map(|l| text_w(&p, l, font.clone()) + PAD * 2.0)
        .fold(body.width(), f32::max);

    ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
        ScrollArea::both().id_salt(("gh-log-scroll", job_id)).auto_shrink([false, false]).show(ui, |ui| {
            ui.set_min_width(max_w);
            for line in &lines {
                let (r, _) = ui.allocate_exact_size(Vec2::new(max_w, line_h), Sense::hover());
                let lp = ui.painter_at(r);
                let is_err_line = line.contains("error")
                    || line.contains("Error")
                    || line.contains("FAILED")
                    || line.contains("panicked");
                let fg = if is_err_line {
                    lp.rect_filled(r, 0.0, theme::rgba(0xf8, 0x71, 0x71, 0.12));
                    theme::RED_TEXT
                } else {
                    theme::TEXT
                };
                let g = galley(&lp, line, font.clone(), fg);
                lp.galley(Pos2::new(r.min.x + 8.0, r.center().y - g.size().y * 0.5), g, fg);
            }
        });
    });
}

// ---------------------------------------------------------------------------
// Menu suspenso "Enviar para agente ▾" (agentes abertos + novo cargo)
// ---------------------------------------------------------------------------

fn draw_agent_menu(deck: &mut Deck, ctx: &egui::Context) {
    let Some(target) = deck.github.agent_menu.clone() else { return };

    // Agentes (abas) abertos no projeto ativo, exceto terminal comum.
    let open_tabs: Vec<(String, String, Color32)> = deck
        .visible()
        .into_iter()
        .filter(|&i| deck.tabs[i].kind != "shell")
        .map(|i| (deck.tabs[i].ident.id(), deck.tabs[i].label.clone(), deck.tabs[i].color))
        .collect();

    // Modelos instalados para abrir um cargo novo (ex.: claude@review).
    let mut models: Vec<(String, String, Color32)> = deck
        .agents
        .new_tab_menu
        .iter()
        .filter(|m| m.kind != "shell")
        .map(|m| (m.kind.clone(), m.label.clone(), m.color))
        .collect();
    if models.is_empty() {
        models.push(("claude".into(), "Claude".into(), theme::CLAUDE));
    }
    if !models.iter().any(|(k, _, _)| *k == deck.github.new_role_kind) {
        deck.github.new_role_kind = models[0].0.clone();
    }

    let w = 264.0;
    let item_h = 26.0;
    let open_count = open_tabs.len().max(1);
    let h = 32.0 + (open_count as f32 * item_h) + 116.0;
    let pos = super::menus::keep_on_screen(ctx, Pos2::new(target.anchor.min.x, target.anchor.max.y + 2.0), Vec2::new(w, h));
    let menu_rect = Rect::from_min_size(pos, Vec2::new(w, h));

    let mut chosen_spec: Option<(String, bool)> = None;
    let mut close_menu = false;

    Area::new(Id::new("gh-agent-menu")).order(Order::Foreground).fixed_pos(pos).constrain(false).show(ctx, |ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
        let p = ui.painter();
        shadow(p, rect, 8, 8, 24, theme::rgba(0, 0, 0, 0xaa as f32 / 255.0));
        frame(p, rect, 8.0, theme::PANEL_2, theme::BORDER);

        let mut y = rect.min.y + 8.0;
        let head_r = Rect::from_min_size(Pos2::new(rect.min.x + 10.0, y), Vec2::new(w - 20.0, 16.0));
        let title_txt = format!("ENVIAR {} PARA AGENTE", target.title.to_uppercase());
        let gt = galley_trunc(p, &title_txt, theme::ui_bold(10.5), theme::MUTED, head_r.width());
        place(p, head_r, Align2::LEFT_CENTER, gt, theme::MUTED);
        y += 20.0;

        if open_tabs.is_empty() {
            let r = Rect::from_min_size(Pos2::new(rect.min.x + 10.0, y), Vec2::new(w - 20.0, item_h));
            text_in(p, r, Align2::LEFT_CENTER, "Nenhuma aba aberta no projeto", theme::ui(11.5), theme::MUTED);
            y += item_h;
        } else {
            for (id, label, color) in &open_tabs {
                let r = Rect::from_min_size(Pos2::new(rect.min.x + 6.0, y), Vec2::new(w - 12.0, item_h));
                y += item_h;
                let resp = clickable(ui, r, Id::new(("gh-menu-tab", id)));
                if resp.hovered() {
                    p.rect_filled(r, 5.0, theme::BORDER);
                }
                dot(p, Pos2::new(r.min.x + 10.0, r.center().y), *color);
                let tr = Rect::from_min_max(Pos2::new(r.min.x + 20.0, r.min.y), Pos2::new(r.max.x - 6.0, r.max.y));
                let gl = galley_trunc(p, &format!("{label} ({id})"), theme::ui(12.0), theme::TEXT, tr.width());
                place(p, tr, Align2::LEFT_CENTER, gl, theme::TEXT);
                if resp.clicked() {
                    chosen_spec = Some((id.clone(), false));
                }
            }
        }

        // Divisor: abrir/usar cargo novo (`modelo@cargo`)
        y += 4.0;
        p.rect_filled(Rect::from_min_size(Pos2::new(rect.min.x + 8.0, y), Vec2::new(w - 16.0, 1.0)), 0.0, theme::BORDER);
        y += 6.0;

        let sub_r = Rect::from_min_size(Pos2::new(rect.min.x + 10.0, y), Vec2::new(w - 20.0, 15.0));
        text_in(p, sub_r, Align2::LEFT_CENTER, "ABRIR OU USAR CARGO (MODELO@CARGO)", theme::ui_bold(10.0), theme::MUTED);
        y += 18.0;

        // Pílulas de modelo (Claude, Antigravity, Ollama…)
        let pill_w = ((w - 16.0 - (models.len().saturating_sub(1) as f32 * 4.0)) / models.len().max(1) as f32).clamp(48.0, 120.0);
        for (i, (kind, label, color)) in models.iter().enumerate() {
            let pr = Rect::from_min_size(Pos2::new(rect.min.x + 8.0 + i as f32 * (pill_w + 4.0), y), Vec2::new(pill_w, 20.0));
            let active = deck.github.new_role_kind == *kind;
            let resp = clickable(ui, pr, Id::new(("gh-role-model", kind)));
            frame(
                p,
                pr,
                4.0,
                if active { theme::BG } else { theme::PANEL },
                if active { *color } else { theme::BORDER },
            );
            let short = label.split_whitespace().next().unwrap_or(label);
            let gm = galley_trunc(p, short, theme::ui(11.0), if active { theme::TEXT } else { theme::MUTED }, pr.width() - 8.0);
            place(p, pr, Align2::CENTER_CENTER, gm, theme::TEXT);
            if resp.clicked() {
                deck.github.new_role_kind = kind.clone();
            }
        }
        y += 24.0;

        // Atalhos rápidos de cargo (@review, @ci, @fix, @dev)
        let roles = ["review", "ci", "fix", "dev"];
        let rw = (w - 16.0 - 12.0) / 4.0;
        for (i, role) in roles.iter().enumerate() {
            let rr = Rect::from_min_size(Pos2::new(rect.min.x + 8.0 + i as f32 * (rw + 4.0), y), Vec2::new(rw, 19.0));
            let active = deck.github.new_role_name.trim() == *role;
            let resp = clickable(ui, rr, Id::new(("gh-quick-role", role)));
            frame(p, rr, 4.0, if active { theme::PANEL } else { theme::BG }, theme::BORDER);
            text_in(p, rr, Align2::CENTER_CENTER, &format!("@{role}"), theme::mono(10.5), if active || resp.hovered() { theme::TEXT } else { theme::MUTED });
            if resp.clicked() {
                deck.github.new_role_name = (*role).to_owned();
            }
        }
        y += 24.0;

        // Caixa de cargo + botão "Abrir/Enviar"
        let btn_w = 68.0;
        let input_r = Rect::from_min_size(Pos2::new(rect.min.x + 8.0, y), Vec2::new(w - 16.0 - btn_w - 6.0, 22.0));
        let send_r = Rect::from_min_size(Pos2::new(input_r.max.x + 6.0, y), Vec2::new(btn_w, 22.0));

        frame(p, input_r, 4.0, theme::BG, theme::BORDER);
        ui.scope_builder(egui::UiBuilder::new().max_rect(input_r.shrink2(Vec2::new(6.0, 2.0))), |ui| {
            let edit = TextEdit::singleline(&mut deck.github.new_role_name)
                .font(theme::mono(11.5))
                .text_color(theme::TEXT)
                .hint_text("cargo")
                .frame(false);
            let resp = ui.add(edit);
            if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                let role = deck.github.new_role_name.trim();
                let spec = if role.is_empty() {
                    deck.github.new_role_kind.clone()
                } else {
                    format!("{}@{role}", deck.github.new_role_kind)
                };
                chosen_spec = Some((spec, false));
            }
        });

        if small_btn_accent(ui, p, send_r, "gh-role-submit", "Enviar →", "Enviar mensagem para esta instância (abre a aba se não existir)") {
            let role = deck.github.new_role_name.trim();
            let spec = if role.is_empty() {
                deck.github.new_role_kind.clone()
            } else {
                format!("{}@{role}", deck.github.new_role_kind)
            };
            chosen_spec = Some((spec, false));
        }

        if ui.input(|i| i.key_pressed(Key::Escape)) {
            close_menu = true;
        }
    });

    if let Some((spec, force_new)) = chosen_spec {
        deck.send_github_prompt_to_agent(&spec, force_new, target.prompt);
    } else if close_menu || super::menus::closes_on_outside(ctx, menu_rect, target.anchor) {
        deck.github.agent_menu = None;
    }
}

// ---------------------------------------------------------------------------
// Helpers visuais
// ---------------------------------------------------------------------------

fn paint_check_badge(p: &egui::Painter, center: Pos2, status: CheckStatus) {
    let color = match status {
        CheckStatus::Success => theme::GREEN,
        CheckStatus::Failure => theme::RED,
        CheckStatus::Pending => Color32::from_rgb(0xfa, 0xcc, 0x15),
        CheckStatus::None => theme::MUTED,
    };
    p.circle_filled(center, 4.0, color);
}

fn summarize_jobs(jobs: &[WorkflowJob]) -> String {
    if jobs.is_empty() {
        return String::new();
    }
    jobs.iter()
        .map(|j| {
            let st = match j.status {
                CheckStatus::Success => "OK",
                CheckStatus::Failure => "FALHOU",
                CheckStatus::Pending => "PENDENTE",
                CheckStatus::None => "-",
            };
            format!("{} ({st})", j.name)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[allow(clippy::too_many_arguments)]
fn icon_btn(
    deck: &mut Deck,
    ui: &Ui,
    p: &egui::Painter,
    rect: Rect,
    id: impl std::hash::Hash,
    icon: Icon,
    tooltip: &str,
    enabled: bool,
) -> bool {
    let r = if enabled { clickable(ui, rect, Id::new(id)) } else { ui.interact(rect, Id::new(id), Sense::hover()) };
    if enabled && r.hovered() {
        p.rect_filled(rect, 5.0, theme::PANEL_2);
    }
    let color = if !enabled {
        theme::rgba(0x90, 0x90, 0xa8, 0.35)
    } else if r.hovered() {
        theme::TEXT
    } else {
        theme::MUTED
    };
    deck.icons.paint(p, icon, rect.center(), 15.0, color, 0.0);
    tip(r, tooltip).clicked() && enabled
}

fn small_btn(ui: &Ui, p: &egui::Painter, rect: Rect, id: impl std::hash::Hash, label: &str, tooltip: &str) -> bool {
    let r = clickable(ui, rect, Id::new(id));
    let bg = if r.hovered() { theme::BORDER } else { theme::PANEL_2 };
    frame(p, rect, 4.0, bg, theme::BORDER);
    let fg = if r.hovered() { theme::WHITE } else { theme::TEXT };
    let g = galley_trunc(p, label, theme::ui(11.0), fg, rect.width() - 6.0);
    place(p, rect, Align2::CENTER_CENTER, g, fg);
    tip(r, tooltip).clicked()
}

fn small_btn_accent(ui: &Ui, p: &egui::Painter, rect: Rect, id: impl std::hash::Hash, label: &str, tooltip: &str) -> bool {
    let r = clickable(ui, rect, Id::new(id));
    let bg = if r.hovered() { theme::ACCENT } else { theme::PANEL_2 };
    let border = if r.hovered() { theme::ACCENT } else { theme::BORDER };
    frame(p, rect, 4.0, bg, border);
    let fg = if r.hovered() { theme::WHITE } else { theme::TEXT };
    let g = galley_trunc(p, label, theme::ui_bold(11.0), fg, rect.width() - 6.0);
    place(p, rect, Align2::CENTER_CENTER, g, fg);
    tip(r, tooltip).clicked()
}
