//! Pintura da aba "GitHub" na barra direita (~330 px):
//! - cabeçalho com `dono/repo` (abre no navegador), estado do login e botão Atualizar;
//! - sub-abas PRs / Issues / Actions;
//! - lista no alto e, ao selecionar, o detalhe embaixo: checks e jobs e, para o job vermelho, o fim
//!   do log (rola dentro da barra, que mantém a largura, como o diff do Git);
//! - ações "Abrir no navegador" e "Enviar para agente" (menu com os agentes do projeto e um cargo).
//!
//! O estado e as chamadas ao core ficam em `github_ui.rs`; aqui o desenho só coleta as ações e as
//! aplica no fim, fora das closures que seguram o `Deck`.

use agentdeck_core::github::{
    format_failed_job_prompt, format_issue_prompt, format_pr_prompt, CheckStatus, GithubError, GithubOverview, Issue, JobLog, PullRequest,
    WorkflowJob, WorkflowRun,
};
use egui::{Align2, Area, Color32, Id, Key, Order, Pos2, Rect, ScrollArea, Sense, TextEdit, Ui, Vec2};

use super::{clickable, dot, frame, galley_trunc, place, shadow, text_in, text_w, tip};
use crate::deck::Deck;
use crate::github_ui::{instance_spec, normalize_role, GithubTab, GithubView};
use crate::icons::Icon;
use crate::theme;

/// Verde de "aprovado" e âmbar de "em andamento" (o vermelho é `theme::pal().danger`).

const PAD: f32 = 10.0;
const REPO_BAR_H: f32 = 34.0;
const SUBTABS_H: f32 = 28.0;
const ROW_H: f32 = 42.0;
const HEAD_H: f32 = 26.0;
const ACT_H: f32 = 28.0;
const LOG_LINE_H: f32 = 16.0;

pub fn draw(deck: &mut Deck, ui: &mut Ui, rect: Rect) {
    ui.painter().rect_filled(rect, 0.0, theme::pal().bg);
    let view = deck.github.view.clone();
    match view {
        GithubView::Loading => draw_loading(ui, rect),
        GithubView::NoProject => {
            let msg = "Abra uma pasta com um repositório do GitHub para ver Pull Requests, Issues e Actions.";
            draw_empty_state(deck, ui, rect, "GitHub", msg, false);
        }
        GithubView::Error(error) => {
            let (title, mut detail) = match &error {
                GithubError::NoGit | GithubError::NotARepo | GithubError::NoRemote | GithubError::NotGithubRemote { .. } => {
                    ("Sem repositório no GitHub".to_owned(), error.to_string())
                }
                _ => (deck.github.repo.as_ref().map_or_else(|| "GitHub".to_owned(), |r| r.to_string()), error.to_string()),
            };
            if matches!(error, GithubError::NoRemote | GithubError::NotARepo) {
                detail.push_str("\nQuando a pasta tiver um remoto \"origin\" do GitHub, os PRs, issues e execuções aparecem aqui.");
            }
            draw_empty_state(deck, ui, rect, &title, &detail, true);
        }
        GithubView::Ready(overview) => draw_ready(deck, ui, rect, &overview),
    }
    let ctx = ui.ctx().clone();
    draw_agent_menu(deck, &ctx);
}

fn draw_loading(ui: &Ui, rect: Rect) {
    let p = ui.painter_at(rect);
    text_in(&p, rect, Align2::CENTER_CENTER, "Consultando o GitHub…", theme::ui(12.0), theme::pal().text_2);
}

/// Barra de título fixa + mensagem centralizada (sem projeto, sem remoto, erro sem dados antigos).
fn draw_empty_state(deck: &mut Deck, ui: &mut Ui, rect: Rect, title: &str, detail: &str, can_retry: bool) {
    let p = ui.painter_at(rect);
    let top = Rect::from_min_size(rect.min, Vec2::new(rect.width(), REPO_BAR_H));
    p.rect_filled(top, 0.0, theme::pal().chip);
    p.rect_filled(Rect::from_min_max(Pos2::new(top.min.x, top.max.y - 1.0), top.max), 0.0, theme::pal().separator);
    deck.icons.paint(&p, Icon::GitHub, Pos2::new(top.min.x + PAD + 7.0, top.center().y), 14.0, theme::pal().text_2, 0.0);
    let btn_w = if can_retry { 34.0 } else { 0.0 };
    let title_rect = Rect::from_min_max(Pos2::new(top.min.x + PAD + 22.0, top.min.y), Pos2::new(top.max.x - PAD - btn_w, top.max.y));
    let g = galley_trunc(&p, title, theme::ui_bold(12.0), theme::pal().text, title_rect.width());
    place(&p, title_rect, Align2::LEFT_CENTER, g, theme::pal().text);

    if can_retry {
        let btn = Rect::from_min_size(Pos2::new(top.max.x - PAD - 24.0, top.center().y - 12.0), Vec2::splat(24.0));
        if icon_btn(deck, ui, &p, btn, "gh-retry-top", Icon::Refresh, "Tentar de novo", !deck.github.refreshing) {
            deck.github_refresh_now();
        }
    }

    let body = Rect::from_min_max(Pos2::new(rect.min.x + 18.0, top.max.y + 28.0), Pos2::new(rect.max.x - 18.0, rect.max.y - 18.0));
    let mut job = egui::text::LayoutJob::simple(detail.to_owned(), theme::ui(12.0), theme::pal().text_2, body.width().max(80.0));
    job.halign = egui::Align::Center;
    let g = p.layout_job(job);
    // Com `halign` centralizado, a âncora é o meio da linha.
    p.galley(Pos2::new(body.center().x, body.min.y), g, theme::pal().text_2);
}

fn draw_ready(deck: &mut Deck, ui: &mut Ui, rect: Rect, ov: &GithubOverview) {
    let mut y = rect.min.y;

    let repo_bar = Rect::from_min_size(Pos2::new(rect.min.x, y), Vec2::new(rect.width(), REPO_BAR_H));
    draw_repo_bar(deck, ui, repo_bar, ov);
    y = repo_bar.max.y;

    // Sem credencial: orientação curta (o tooltip explica o caminho).
    if !ov.authenticated {
        let strip = Rect::from_min_size(Pos2::new(rect.min.x, y), Vec2::new(rect.width(), 22.0));
        let p = ui.painter_at(strip);
        p.rect_filled(strip, 0.0, theme::pal().hover);
        p.rect_filled(Rect::from_min_max(Pos2::new(strip.min.x, strip.max.y - 1.0), strip.max), 0.0, theme::pal().separator);
        let txt = match ov.rate_limit {
            Some(rl) if rl.remaining.is_some() && rl.limit.is_some() => {
                format!("Sem login ({} de {} por hora) · faça um push na aba Git", rl.remaining.unwrap_or(0), rl.limit.unwrap_or(0))
            }
            _ => "Sem login (60 por hora) · faça um push na aba Git".to_owned(),
        };
        let inner = strip.shrink2(Vec2::new(PAD, 0.0));
        let g = galley_trunc(&p, &txt, theme::ui(10.5), theme::pal().text_2, inner.width());
        place(&p, inner, Align2::LEFT_CENTER, g, theme::pal().text_2);
        let r = ui.interact(strip, Id::new("gh-anon-strip"), Sense::hover());
        tip(
            r,
            "Repositório público lido sem token (limite de 60 consultas por hora; o log dos jobs exige login).\nPara entrar sem guardar token no app: faça um push pela aba Git (abre o login do Git Credential Manager) ou rode \"gh auth login\".",
        );
        y = strip.max.y;
    }

    // Aviso ou erro recente (a lista anterior continua na tela).
    if let Some((msg, is_err)) = deck.github.notice.clone() {
        let strip = Rect::from_min_size(Pos2::new(rect.min.x, y), Vec2::new(rect.width(), 24.0));
        let p = ui.painter_at(strip);
        let (bg, fg) =
            if is_err { (theme::with_alpha(theme::pal().danger, 0.14), theme::pal().danger_text) } else { (theme::pal().hover, theme::pal().text) };
        p.rect_filled(strip, 0.0, bg);
        p.rect_filled(Rect::from_min_max(Pos2::new(strip.min.x, strip.max.y - 1.0), strip.max), 0.0, theme::pal().separator);
        let close_r = Rect::from_min_size(Pos2::new(strip.max.x - 22.0, strip.min.y + 2.0), Vec2::splat(20.0));
        let txt_r = Rect::from_min_max(Pos2::new(strip.min.x + PAD, strip.min.y), Pos2::new(close_r.min.x - 4.0, strip.max.y));
        let g = galley_trunc(&p, &msg, theme::ui(11.0), fg, txt_r.width());
        place(&p, txt_r, Align2::LEFT_CENTER, g, fg);
        let cr = clickable(ui, close_r, Id::new("gh-notice-close"));
        deck.icons.paint(&p, Icon::Close, close_r.center(), 12.0, if cr.hovered() { theme::pal().text } else { theme::pal().text_2 }, 0.0);
        // A mensagem inteira, quando não cabe na faixa.
        let hover = ui.interact(txt_r, Id::new("gh-notice-text"), Sense::hover());
        tip(hover, msg);
        if cr.clicked() {
            deck.github.notice = None;
        }
        y = strip.max.y;
    }

    let subtabs = Rect::from_min_size(Pos2::new(rect.min.x, y), Vec2::new(rect.width(), SUBTABS_H));
    draw_subtabs(deck, ui, subtabs, ov);
    y = subtabs.max.y;

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
    p.rect_filled(rect, 0.0, theme::pal().chip);
    p.rect_filled(Rect::from_min_max(Pos2::new(rect.min.x, rect.max.y - 1.0), rect.max), 0.0, theme::pal().separator);

    let refresh_r = Rect::from_min_size(Pos2::new(rect.max.x - PAD - 24.0, rect.center().y - 12.0), Vec2::splat(24.0));
    let refresh_tip = match ov.rate_limit {
        Some(rl) if rl.remaining.is_some() && rl.limit.is_some() => {
            format!("Atualizar ({} de {} consultas por hora restantes)", rl.remaining.unwrap_or(0), rl.limit.unwrap_or(0))
        }
        _ => "Atualizar os dados do GitHub".to_owned(),
    };
    if icon_btn(deck, ui, &p, refresh_r, "gh-refresh", Icon::Refresh, &refresh_tip, !deck.github.refreshing) {
        deck.github_refresh_now();
    }

    let open_w = 58.0;
    let open_r = Rect::from_min_size(Pos2::new(refresh_r.min.x - 4.0 - open_w, rect.center().y - 11.0), Vec2::new(open_w, 22.0));
    if small_btn(ui, &p, open_r, "gh-open-repo", "Abrir ↗", &format!("Abrir {} no navegador", ov.repo.html_url()), false) {
        deck.open_github_browser(&ov.repo.html_url());
    }

    let slug_r = Rect::from_min_max(Pos2::new(rect.min.x + PAD, rect.min.y + 4.0), Pos2::new(open_r.min.x - 6.0, rect.max.y - 4.0));
    let sr = ui.interact(slug_r, Id::new("gh-repo-slug"), Sense::hover());
    dot(&p, Pos2::new(slug_r.min.x + 4.0, slug_r.center().y), if ov.authenticated { theme::pal().success } else { theme::pal().text_2 });
    let txt_r = Rect::from_min_max(Pos2::new(slug_r.min.x + 14.0, slug_r.min.y), slug_r.max);
    let g = galley_trunc(&p, &ov.repo.to_string(), theme::ui_bold(12.0), theme::pal().text, txt_r.width());
    place(&p, txt_r, Align2::LEFT_CENTER, g, theme::pal().text);
    let auth = if ov.authenticated { "com login (Git Credential Manager ou gh)" } else { "sem login: só repositórios públicos" };
    tip(sr, format!("{} · {auth}", ov.repo));
}

fn draw_subtabs(deck: &mut Deck, ui: &mut Ui, rect: Rect, ov: &GithubOverview) {
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, theme::pal().chip);
    p.rect_filled(Rect::from_min_max(Pos2::new(rect.min.x, rect.max.y - 1.0), rect.max), 0.0, theme::pal().separator);

    let items = [
        (GithubTab::Prs, format!("PRs ({})", ov.prs.len())),
        (GithubTab::Issues, format!("Issues ({})", ov.issues.len())),
        (GithubTab::Actions, format!("Actions ({})", ov.runs.len())),
    ];
    let tab_w = (rect.width() - PAD * 2.0) / items.len() as f32;
    let active_color = deck.accent();
    for (i, (tab, label)) in items.iter().enumerate() {
        let r = Rect::from_min_size(Pos2::new(rect.min.x + PAD + i as f32 * tab_w, rect.min.y), Vec2::new(tab_w, rect.height() - 1.0));
        let active = deck.github.tab == *tab;
        let resp = clickable(ui, r, Id::new(("gh-subtab", i)));
        if active || resp.hovered() {
            p.rect_filled(r.shrink2(Vec2::new(2.0, 3.0)), 4.0, if active { theme::pal().hover } else { theme::pal().bg });
        }
        let fg = if active || resp.hovered() { theme::pal().text } else { theme::pal().text_2 };
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

/// `v` entre `lo` e `hi`; se a área é tão pequena que `hi < lo`, vale `hi` (`f32::clamp` entraria em pânico).
fn clamp_between(v: f32, lo: f32, hi: f32) -> f32 {
    v.min(hi).max(lo.min(hi))
}

/// Divide a área em lista (em cima) e detalhe (embaixo) quando há seleção.
fn split_list_detail(rect: Rect, has_detail: bool, frac: f32, min_detail: f32) -> (Rect, Option<Rect>) {
    if !has_detail {
        return (rect, None);
    }
    let detail_h = clamp_between(rect.height() * frac, min_detail, (rect.height() - 86.0).max(rect.height() * 0.5));
    let split_y = (rect.max.y - detail_h).max(rect.min.y + 44.0);
    (Rect::from_min_max(rect.min, Pos2::new(rect.max.x, split_y)), Some(Rect::from_min_max(Pos2::new(rect.min.x, split_y), rect.max)))
}

/// Área rolável dentro de `rect` (recortada), como as listas da aba Git.
fn child_ui(ui: &mut Ui, rect: Rect) -> Ui {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(egui::Align::Min)));
    child.set_clip_rect(rect);
    child
}

fn empty_msg(ui: &Ui, rect: Rect, msg: &str) {
    let p = ui.painter_at(rect);
    text_in(&p, rect.shrink(PAD), Align2::CENTER_CENTER, msg, theme::ui(12.0), theme::pal().text_2);
}

/// Linha de lista (fundo de seleção/hover e divisor). Devolve a resposta do clique.
fn list_row(ui: &mut Ui, width: f32, selected: bool) -> (Rect, egui::Response) {
    let (row, resp) = ui.allocate_exact_size(Vec2::new(width, ROW_H), Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let p = ui.painter_at(row);
    if selected || resp.hovered() {
        p.rect_filled(row, 0.0, if selected { theme::pal().hover } else { theme::pal().chip });
    }
    p.rect_filled(Rect::from_min_max(Pos2::new(row.min.x, row.max.y - 1.0), row.max), 0.0, theme::pal().separator);
    (row, resp)
}

// ---------------------------------------------------------------------------
// Pull requests
// ---------------------------------------------------------------------------

fn draw_prs_tab(deck: &mut Deck, ui: &mut Ui, rect: Rect, ov: &GithubOverview) {
    if ov.prs.is_empty() {
        empty_msg(ui, rect, "Nenhum pull request aberto.");
        return;
    }
    let selected = deck.github.selected_pr.and_then(|num| ov.prs.iter().find(|pr| pr.number == num)).cloned();
    let (list_rect, detail_rect) = split_list_detail(rect, selected.is_some(), 0.62, 190.0);

    let mut clicked = None;
    let active = deck.github.selected_pr;
    let mut child = child_ui(ui, list_rect);
    ScrollArea::vertical().id_salt("gh-prs-scroll").auto_shrink([false, false]).show(&mut child, |ui| {
        for pr in &ov.prs {
            let (row, resp) = list_row(ui, list_rect.width(), active == Some(pr.number));
            draw_pr_row(&ui.painter_at(row), row, pr, active == Some(pr.number));
            if tip(resp, format!("PR #{} · checks: {}", pr.number, pr.checks.label())).clicked() {
                clicked = Some(pr.number);
            }
        }
    });
    if let Some(num) = clicked {
        deck.github_select_pr(num);
    }
    if let (Some(pr), Some(drect)) = (selected, detail_rect) {
        draw_pr_detail(deck, ui, drect, ov, &pr);
    }
}

fn draw_pr_row(p: &egui::Painter, row: Rect, pr: &PullRequest, selected: bool) {
    let top_y = row.min.y + 4.0;
    let bot_y = row.min.y + 22.0;
    paint_check_badge(p, Pos2::new(row.min.x + PAD + 5.0, top_y + 8.0), pr.checks);

    let num_txt = format!("#{}", pr.number);
    let num_font = theme::mono(11.0);
    let num_w = text_w(p, &num_txt, num_font.clone());
    let num_r = Rect::from_min_size(Pos2::new(row.min.x + PAD + 14.0, top_y), Vec2::new(num_w, 16.0));
    text_in(p, num_r, Align2::LEFT_CENTER, &num_txt, num_font, theme::pal().text_2);

    let mut x = num_r.max.x + 6.0;
    if pr.draft {
        let pill = Rect::from_min_size(Pos2::new(x, top_y + 1.0), Vec2::new(60.0, 14.0));
        frame(p, pill, 3.0, theme::pal().hover, theme::pal().separator);
        text_in(p, pill, Align2::CENTER_CENTER, "RASCUNHO", theme::ui_bold(9.0), theme::pal().text_2);
        x = pill.max.x + 5.0;
    }
    let title_r = Rect::from_min_max(Pos2::new(x, top_y), Pos2::new(row.max.x - PAD, top_y + 16.0));
    let font = if selected { theme::ui_bold(12.0) } else { theme::ui(12.0) };
    let g = galley_trunc(p, &pr.title, font, theme::pal().text, title_r.width());
    place(p, title_r, Align2::LEFT_CENTER, g, theme::pal().text);

    let author_txt = format!("@{}", pr.author);
    let author_font = theme::ui(10.5);
    let author_w = text_w(p, &author_txt, author_font.clone()).min(row.width() * 0.35);
    let author_r = Rect::from_min_size(Pos2::new(row.max.x - PAD - author_w, bot_y), Vec2::new(author_w, 15.0));
    let ga = galley_trunc(p, &author_txt, author_font, theme::pal().text_2, author_w);
    place(p, author_r, Align2::RIGHT_CENTER, ga, theme::pal().text_2);

    let branch_txt = format!("{} → {}", pr.head_branch, pr.base_branch);
    let branch_r = Rect::from_min_max(Pos2::new(row.min.x + PAD + 14.0, bot_y), Pos2::new(author_r.min.x - 6.0, bot_y + 15.0));
    let gb = galley_trunc(p, &branch_txt, theme::mono(10.5), theme::pal().text_2, branch_r.width());
    place(p, branch_r, Align2::LEFT_CENTER, gb, theme::pal().text_2);
}

fn draw_pr_detail(deck: &mut Deck, ui: &mut Ui, rect: Rect, ov: &GithubOverview, pr: &PullRequest) {
    let title = format!("PR #{} · {}", pr.number, pr.title);
    let Some(act) =
        detail_header(deck, ui, rect, Some(pr.checks), &title, "gh-pr-detail-close", &|d: &mut Deck| d.github.selected_pr = None)
    else {
        return;
    };
    let (open, agent) = action_bar(ui, act, pr.number, "Enviar o PR com uma mensagem pronta de revisão");
    if open {
        deck.open_github_browser(&pr.html_url);
    }
    if let Some(anchor) = agent {
        let prompt = format_pr_prompt(&ov.repo, pr);
        deck.open_github_agent_menu(anchor, format!("PR #{}", pr.number), "review", prompt);
    }

    let content = Rect::from_min_max(Pos2::new(rect.min.x, act.max.y), rect.max);
    let detail = deck.github.pr_details.get(&pr.number);
    let jobs: Vec<WorkflowJob> = detail.map(|d| d.jobs.clone()).unwrap_or_default();
    let runs: Vec<WorkflowRun> = detail.map(|d| d.runs.clone()).unwrap_or_default();
    let loading = deck.github.pr_loading == Some(pr.number);
    let loaded = detail.is_some();
    draw_jobs_and_log(deck, ui, content, ov, Some(pr), &runs, &jobs, loading, loaded);
}

// ---------------------------------------------------------------------------
// Issues
// ---------------------------------------------------------------------------

fn draw_issues_tab(deck: &mut Deck, ui: &mut Ui, rect: Rect, ov: &GithubOverview) {
    if ov.issues.is_empty() {
        empty_msg(ui, rect, "Nenhuma issue aberta.");
        return;
    }
    let selected = deck.github.selected_issue.and_then(|num| ov.issues.iter().find(|i| i.number == num)).cloned();
    let (list_rect, detail_rect) = split_list_detail(rect, selected.is_some(), 0.45, 150.0);

    let mut clicked = None;
    let active = deck.github.selected_issue;
    let mut child = child_ui(ui, list_rect);
    ScrollArea::vertical().id_salt("gh-issues-scroll").auto_shrink([false, false]).show(&mut child, |ui| {
        for issue in &ov.issues {
            let (row, resp) = list_row(ui, list_rect.width(), active == Some(issue.number));
            draw_issue_row(&ui.painter_at(row), row, issue, active == Some(issue.number));
            if resp.clicked() {
                clicked = Some(issue.number);
            }
        }
    });
    if let Some(num) = clicked {
        deck.github.selected_issue = if active == Some(num) { None } else { Some(num) };
    }
    if let (Some(issue), Some(drect)) = (selected, detail_rect) {
        draw_issue_detail(deck, ui, drect, ov, &issue);
    }
}

fn draw_issue_row(p: &egui::Painter, row: Rect, issue: &Issue, selected: bool) {
    let top_y = row.min.y + 4.0;
    let bot_y = row.min.y + 22.0;
    dot(p, Pos2::new(row.min.x + PAD + 5.0, top_y + 8.0), theme::pal().success);

    let num_txt = format!("#{}", issue.number);
    let num_font = theme::mono(11.0);
    let num_w = text_w(p, &num_txt, num_font.clone());
    let num_r = Rect::from_min_size(Pos2::new(row.min.x + PAD + 14.0, top_y), Vec2::new(num_w, 16.0));
    text_in(p, num_r, Align2::LEFT_CENTER, &num_txt, num_font, theme::pal().text_2);

    let title_r = Rect::from_min_max(Pos2::new(num_r.max.x + 6.0, top_y), Pos2::new(row.max.x - PAD, top_y + 16.0));
    let font = if selected { theme::ui_bold(12.0) } else { theme::ui(12.0) };
    let g = galley_trunc(p, &issue.title, font, theme::pal().text, title_r.width());
    place(p, title_r, Align2::LEFT_CENTER, g, theme::pal().text);

    let sub_r = Rect::from_min_max(Pos2::new(row.min.x + PAD + 14.0, bot_y), Pos2::new(row.max.x - PAD, bot_y + 15.0));
    let gs = galley_trunc(p, &issue_meta(issue), theme::ui(10.5), theme::pal().text_2, sub_r.width());
    place(p, sub_r, Align2::LEFT_CENTER, gs, theme::pal().text_2);
}

/// `@autor · rótulos · N coment.`
fn issue_meta(issue: &Issue) -> String {
    let mut sub = format!("@{}", issue.author);
    if !issue.labels.is_empty() {
        sub.push_str(" · ");
        sub.push_str(&issue.labels.join(", "));
    }
    if issue.comments > 0 {
        sub.push_str(&format!(" · {} coment.", issue.comments));
    }
    sub
}

fn draw_issue_detail(deck: &mut Deck, ui: &mut Ui, rect: Rect, ov: &GithubOverview, issue: &Issue) {
    let title = format!("Issue #{} · {}", issue.number, issue.title);
    let Some(act) = detail_header(deck, ui, rect, None, &title, "gh-issue-close", &|d: &mut Deck| d.github.selected_issue = None) else {
        return;
    };
    let (open, agent) = action_bar(ui, act, issue.number, "Enviar a issue com uma mensagem pronta para um agente resolver");
    if open {
        deck.open_github_browser(&issue.html_url);
    }
    if let Some(anchor) = agent {
        deck.open_github_agent_menu(anchor, format!("issue #{}", issue.number), "dev", format_issue_prompt(&ov.repo, issue));
    }

    let body_r = Rect::from_min_max(Pos2::new(rect.min.x + PAD, act.max.y + 6.0), Pos2::new(rect.max.x - PAD, rect.max.y - 4.0));
    if body_r.height() <= 8.0 {
        return;
    }
    let mut child = child_ui(ui, body_r);
    ScrollArea::vertical().id_salt(("gh-issue-body", issue.number)).auto_shrink([false, false]).show(&mut child, |ui| {
        ui.set_width(body_r.width());
        ui.label(egui::RichText::new(issue_meta(issue)).font(theme::ui(10.5)).color(theme::pal().text_2));
        ui.add_space(4.0);
        let desc = if issue.body_preview.trim().is_empty() { "Sem descrição." } else { issue.body_preview.trim() };
        ui.label(egui::RichText::new(desc).font(theme::ui(11.5)).color(theme::pal().text));
    });
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

fn draw_actions_tab(deck: &mut Deck, ui: &mut Ui, rect: Rect, ov: &GithubOverview) {
    if ov.runs.is_empty() {
        empty_msg(ui, rect, "Nenhuma execução do Actions encontrada.");
        return;
    }
    let selected = deck.github.selected_run.and_then(|id| ov.runs.iter().find(|r| r.id == id)).cloned();
    let (list_rect, detail_rect) = split_list_detail(rect, selected.is_some(), 0.58, 180.0);

    let mut clicked = None;
    let mut open_url = None;
    let active = deck.github.selected_run;
    let mut child = child_ui(ui, list_rect);
    ScrollArea::vertical().id_salt("gh-runs-scroll").auto_shrink([false, false]).show(&mut child, |ui| {
        for run in &ov.runs {
            let (row, resp) = list_row(ui, list_rect.width(), active == Some(run.id));
            let p = ui.painter_at(row);
            let btn_r = Rect::from_min_size(Pos2::new(row.max.x - PAD - 22.0, row.center().y - 10.0), Vec2::new(22.0, 20.0));
            draw_run_row(&p, Rect::from_min_max(row.min, Pos2::new(btn_r.min.x - 4.0, row.max.y)), run, active == Some(run.id));
            if small_btn(ui, &p, btn_r, ("gh-run-open", run.id), "↗", "Abrir a execução no navegador", false) {
                open_url = Some(run.html_url.clone());
            } else if tip(resp, format!("{} · {}", run.name, run.status.label())).clicked() {
                clicked = Some(run.id);
            }
        }
    });
    if let Some(url) = open_url {
        deck.open_github_browser(&url);
    }
    if let Some(id) = clicked {
        deck.github_select_run(id);
    }

    if let (Some(run), Some(drect)) = (selected, detail_rect) {
        let title = format!("{} · {}", run.name, run.head_branch);
        let Some(head_bottom) =
            detail_header(deck, ui, drect, Some(run.status), &title, "gh-run-close", &|d: &mut Deck| d.github.selected_run = None)
        else {
            return;
        };
        let content = Rect::from_min_max(Pos2::new(drect.min.x, head_bottom.min.y), drect.max);
        let jobs = deck.github.run_jobs.get(&run.id).cloned().unwrap_or_default();
        let loaded = deck.github.run_jobs.contains_key(&run.id);
        let loading = deck.github.run_loading == Some(run.id);
        let pr = ov.prs.iter().find(|p| !p.head_sha.is_empty() && p.head_sha == run.head_sha);
        draw_jobs_and_log(deck, ui, content, ov, pr, std::slice::from_ref(&run), &jobs, loading, loaded);
    }
}

fn draw_run_row(p: &egui::Painter, row: Rect, run: &WorkflowRun, selected: bool) {
    let top_y = row.min.y + 4.0;
    let bot_y = row.min.y + 22.0;
    paint_check_badge(p, Pos2::new(row.min.x + PAD + 5.0, top_y + 8.0), run.status);

    let title_r = Rect::from_min_max(Pos2::new(row.min.x + PAD + 14.0, top_y), Pos2::new(row.max.x - 2.0, top_y + 16.0));
    let font = if selected { theme::ui_bold(12.0) } else { theme::ui(12.0) };
    let g = galley_trunc(p, &run.display_title, font, theme::pal().text, title_r.width());
    place(p, title_r, Align2::LEFT_CENTER, g, theme::pal().text);

    let sub = format!("{} · {} · {}", run.name, run.event, run.head_branch);
    let sub_r = Rect::from_min_max(Pos2::new(row.min.x + PAD + 14.0, bot_y), Pos2::new(row.max.x - 2.0, bot_y + 15.0));
    let gs = galley_trunc(p, &sub, theme::mono(10.5), theme::pal().text_2, sub_r.width());
    place(p, sub_r, Align2::LEFT_CENTER, gs, theme::pal().text_2);
}

// ---------------------------------------------------------------------------
// Detalhe: cabeçalho, ações, jobs e log
// ---------------------------------------------------------------------------

/// Fundo do detalhe + faixa de título com o botão de fechar. Devolve a faixa logo abaixo do título
/// (para a barra de ações ou o conteúdo), ou `None` quando o detalhe foi fechado neste quadro.
fn detail_header(
    deck: &mut Deck,
    ui: &mut Ui,
    rect: Rect,
    status: Option<CheckStatus>,
    title: &str,
    close_id: &str,
    close: &dyn Fn(&mut Deck),
) -> Option<Rect> {
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, theme::pal().chip);
    p.rect_filled(Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.min.y + 1.0)), 0.0, theme::pal().separator);

    let head = Rect::from_min_size(Pos2::new(rect.min.x, rect.min.y + 1.0), Vec2::new(rect.width(), HEAD_H));
    p.rect_filled(head, 0.0, theme::pal().hover);
    p.rect_filled(Rect::from_min_max(Pos2::new(head.min.x, head.max.y - 1.0), head.max), 0.0, theme::pal().separator);

    let close_r = Rect::from_min_size(Pos2::new(head.max.x - PAD - 20.0, head.center().y - 10.0), Vec2::splat(20.0));
    let cr = clickable(ui, close_r, Id::new(close_id));
    deck.icons.paint(&p, Icon::Close, close_r.center(), 13.0, if cr.hovered() { theme::pal().text } else { theme::pal().text_2 }, 0.0);
    if tip(cr, "Fechar o detalhe").clicked() {
        close(deck);
        return None;
    }

    let mut x = head.min.x + PAD;
    if let Some(st) = status {
        paint_check_badge(&p, Pos2::new(x + 5.0, head.center().y), st);
        x += 14.0;
    }
    let head_r = Rect::from_min_max(Pos2::new(x, head.min.y), Pos2::new(close_r.min.x - 4.0, head.max.y));
    let g = galley_trunc(&p, title, theme::ui_bold(11.5), theme::pal().text, head_r.width());
    place(&p, head_r, Align2::LEFT_CENTER, g, theme::pal().text);
    Some(Rect::from_min_size(Pos2::new(rect.min.x, head.max.y), Vec2::new(rect.width(), ACT_H)))
}

/// Barra com "Abrir no navegador" e "Enviar para agente". Devolve `(abrir, âncora do menu)`.
fn action_bar(ui: &mut Ui, bar: Rect, key: u64, agent_tip: &str) -> (bool, Option<Rect>) {
    let p = ui.painter_at(bar);
    p.rect_filled(bar, 0.0, theme::pal().chip);
    p.rect_filled(Rect::from_min_max(Pos2::new(bar.min.x, bar.max.y - 1.0), bar.max), 0.0, theme::pal().separator);
    let half_w = (bar.width() - PAD * 2.0 - 6.0) * 0.5;
    let btn_open = Rect::from_min_size(Pos2::new(bar.min.x + PAD, bar.center().y - 11.0), Vec2::new(half_w, 22.0));
    let btn_agent = Rect::from_min_size(Pos2::new(btn_open.max.x + 6.0, bar.center().y - 11.0), Vec2::new(half_w, 22.0));
    let open = small_btn(ui, &p, btn_open, ("gh-act-open", key), "Abrir no navegador", "Abrir esta página no navegador padrão", false);
    let agent = small_btn(ui, &p, btn_agent, ("gh-act-agent", key), "Enviar para agente ▾", agent_tip, true);
    (open, agent.then_some(btn_agent))
}

/// Lista de checks/jobs e, embaixo, o log do job aberto.
#[allow(clippy::too_many_arguments)]
fn draw_jobs_and_log(
    deck: &mut Deck,
    ui: &mut Ui,
    rect: Rect,
    ov: &GithubOverview,
    pr: Option<&PullRequest>,
    runs: &[WorkflowRun],
    jobs: &[WorkflowJob],
    loading: bool,
    loaded: bool,
) {
    if rect.height() <= 8.0 {
        return;
    }
    let pr_number = pr.map(|p| p.number);
    let active_log = deck.github.open_log.clone().filter(|(job_id, _, _)| jobs.iter().any(|j| j.id == *job_id));
    let (jobs_rect, log_rect) = match &active_log {
        Some(_) => {
            let log_h = clamp_between(rect.height() * 0.56, 96.0, (rect.height() - 52.0).max(rect.height() * 0.5));
            let split_y = (rect.max.y - log_h).max(rect.min.y + 42.0);
            (
                Rect::from_min_max(rect.min, Pos2::new(rect.max.x, split_y)),
                Some(Rect::from_min_max(Pos2::new(rect.min.x, split_y), rect.max)),
            )
        }
        None => (rect, None),
    };

    if jobs.is_empty() {
        let msg = if loading || !loaded {
            "Carregando checks e jobs…"
        } else if runs.is_empty() {
            "Nenhuma execução do Actions para este commit."
        } else {
            "Esta execução não tem jobs."
        };
        let p = ui.painter_at(jobs_rect);
        text_in(&p, jobs_rect, Align2::CENTER_CENTER, msg, theme::ui(11.5), theme::pal().text_2);
    } else {
        let mut req_log: Option<(u64, String)> = None;
        let mut req_agent: Option<(Rect, WorkflowJob)> = None;
        let mut open_url: Option<String> = None;
        let current_log = active_log.as_ref().map(|(id, _, _)| *id);

        let mut child = child_ui(ui, jobs_rect);
        ScrollArea::vertical().id_salt(("gh-jobs-scroll", pr_number.unwrap_or(0), jobs[0].run_id)).auto_shrink([false, false]).show(
            &mut child,
            |ui| {
                for job in jobs {
                    let failed = job.status == CheckStatus::Failure;
                    let row_h = if failed && !job.failed_steps.is_empty() { 40.0 } else { 26.0 };
                    let (row, _) = ui.allocate_exact_size(Vec2::new(jobs_rect.width(), row_h), Sense::hover());
                    let p = ui.painter_at(row);
                    p.rect_filled(Rect::from_min_max(Pos2::new(row.min.x, row.max.y - 1.0), row.max), 0.0, theme::pal().separator);

                    let top_y = row.min.y + 4.0;
                    paint_check_badge(&p, Pos2::new(row.min.x + PAD + 5.0, top_y + 9.0), job.status);

                    let mut right_x = row.max.x - PAD;
                    if !job.html_url.is_empty() {
                        let b = Rect::from_min_size(Pos2::new(right_x - 20.0, top_y), Vec2::new(20.0, 18.0));
                        if small_btn(ui, &p, b, ("gh-job-open", job.id), "↗", "Abrir o job no navegador", false) {
                            open_url = Some(job.html_url.clone());
                        }
                        right_x = b.min.x - 4.0;
                    }
                    if failed {
                        let b_fix = Rect::from_min_size(Pos2::new(right_x - 62.0, top_y), Vec2::new(62.0, 18.0));
                        if small_btn(
                            ui,
                            &p,
                            b_fix,
                            ("gh-job-fix", job.id),
                            "Corrigir ▾",
                            "Enviar a falha e o fim do log para um agente corrigir",
                            true,
                        ) {
                            req_agent = Some((b_fix, job.clone()));
                        }
                        right_x = b_fix.min.x - 4.0;
                        let b_log = Rect::from_min_size(Pos2::new(right_x - 34.0, top_y), Vec2::new(34.0, 18.0));
                        if small_btn(
                            ui,
                            &p,
                            b_log,
                            ("gh-job-log", job.id),
                            "Log",
                            "Ver o fim do log deste job",
                            current_log == Some(job.id),
                        ) {
                            req_log = Some((job.id, job.name.clone()));
                        }
                        right_x = b_log.min.x - 4.0;
                    }

                    let name_r = Rect::from_min_max(Pos2::new(row.min.x + PAD + 14.0, top_y), Pos2::new(right_x, top_y + 18.0));
                    let name_color = if failed { theme::pal().danger_text } else { theme::pal().text };
                    let gn = galley_trunc(&p, &job.name, theme::ui(11.5), name_color, name_r.width());
                    place(&p, name_r, Align2::LEFT_CENTER, gn, name_color);

                    if failed && !job.failed_steps.is_empty() {
                        let step_txt = format!("falhou em: {}", job.failed_steps.join(", "));
                        let step_r = Rect::from_min_max(
                            Pos2::new(row.min.x + PAD + 14.0, top_y + 18.0),
                            Pos2::new(row.max.x - PAD, row.max.y - 2.0),
                        );
                        let gs = galley_trunc(&p, &step_txt, theme::mono(10.0), theme::pal().danger_text, step_r.width());
                        place(&p, step_r, Align2::LEFT_CENTER, gs, theme::pal().danger_text);
                    }
                }
            },
        );

        if let Some(url) = open_url {
            deck.open_github_browser(&url);
        }
        if let Some((job_id, job_name)) = req_log {
            deck.github_request_job_log(job_id, job_name, pr_number);
        }
        if let Some((anchor, job)) = req_agent {
            // Garante o log (a mensagem leva o fim dele); se ainda não veio, a mensagem sai sem ele.
            if !deck.github.job_logs.contains_key(&job.id) {
                deck.github_request_job_log(job.id, job.name.clone(), pr_number);
            }
            let log = deck.github.job_logs.get(&job.id).and_then(|r| r.as_ref().ok());
            let run = runs.iter().find(|r| r.id == job.run_id).or_else(|| ov.runs.iter().find(|r| r.id == job.run_id));
            let prompt = format_failed_job_prompt(&ov.repo, pr, run, &job, log);
            deck.open_github_agent_menu(anchor, format!("job \"{}\"", job.name), "ci", prompt);
        }
    }

    if let (Some((job_id, job_name, _)), Some(lrect)) = (active_log, log_rect) {
        let job = jobs.iter().find(|j| j.id == job_id).cloned();
        draw_job_log_pane(deck, ui, lrect, ov, pr, runs, job_id, &job_name, job);
    }
}

/// Fim do log do job: rolagem nos dois sentidos dentro da barra.
#[allow(clippy::too_many_arguments)]
fn draw_job_log_pane(
    deck: &mut Deck,
    ui: &mut Ui,
    rect: Rect,
    ov: &GithubOverview,
    pr: Option<&PullRequest>,
    runs: &[WorkflowRun],
    job_id: u64,
    job_name: &str,
    job: Option<WorkflowJob>,
) {
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, theme::pal().bg);
    p.rect_filled(Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.min.y + 1.0)), 0.0, theme::pal().separator);

    let head = Rect::from_min_size(Pos2::new(rect.min.x, rect.min.y + 1.0), Vec2::new(rect.width(), 22.0));
    p.rect_filled(head, 0.0, theme::pal().hover);
    p.rect_filled(Rect::from_min_max(Pos2::new(head.min.x, head.max.y - 1.0), head.max), 0.0, theme::pal().separator);

    let close_r = Rect::from_min_size(Pos2::new(head.max.x - PAD - 18.0, head.center().y - 9.0), Vec2::splat(18.0));
    let cr = clickable(ui, close_r, Id::new("gh-log-close"));
    deck.icons.paint(&p, Icon::Close, close_r.center(), 12.0, if cr.hovered() { theme::pal().text } else { theme::pal().text_2 }, 0.0);
    if tip(cr, "Fechar o log").clicked() {
        deck.github.open_log = None;
        return;
    }

    let log_state = deck.github.job_logs.get(&job_id).cloned();
    let send_r = Rect::from_min_size(Pos2::new(close_r.min.x - 4.0 - 84.0, head.center().y - 9.0), Vec2::new(84.0, 18.0));
    if small_btn(ui, &p, send_r, ("gh-log-send", job_id), "Enviar ▾", "Enviar a falha e este trecho do log para um agente corrigir", true)
    {
        let fallback = || WorkflowJob {
            id: job_id,
            run_id: 0,
            name: job_name.to_owned(),
            status: CheckStatus::Failure,
            conclusion: Some("failure".into()),
            html_url: String::new(),
            failed_steps: Vec::new(),
        };
        let job_ref = job.clone().unwrap_or_else(fallback);
        let log = log_state.as_ref().and_then(|r| r.as_ref().ok());
        let run = runs.iter().find(|r| r.id == job_ref.run_id).or_else(|| ov.runs.iter().find(|r| r.id == job_ref.run_id));
        let prompt = format_failed_job_prompt(&ov.repo, pr, run, &job_ref, log);
        deck.open_github_agent_menu(send_r, format!("o log de \"{job_name}\""), "ci", prompt);
    }

    let title_r = Rect::from_min_max(Pos2::new(head.min.x + PAD, head.min.y), Pos2::new(send_r.min.x - 4.0, head.max.y));
    let g = galley_trunc(&p, &format!("Log · {job_name}"), theme::mono(10.5), theme::pal().danger_text, title_r.width());
    place(&p, title_r, Align2::LEFT_CENTER, g, theme::pal().danger_text);

    let body = Rect::from_min_max(Pos2::new(rect.min.x, head.max.y), rect.max);
    match log_state {
        None => {
            let msg = if deck.github.log_loading == Some(job_id) { "Baixando o fim do log…" } else { "Log indisponível." };
            text_in(&p, body, Align2::CENTER_CENTER, msg, theme::ui(11.0), theme::pal().text_2);
        }
        Some(Err(e)) => draw_wrapped(&p, body.shrink(PAD), &e.to_string()),
        Some(Ok(log)) => log_lines(ui, body, &log, job_id),
    }
}

fn draw_wrapped(p: &egui::Painter, area: Rect, text: &str) {
    let job = egui::text::LayoutJob::simple(text.to_owned(), theme::ui(11.5), theme::pal().text_2, area.width().max(80.0));
    let g = p.layout_job(job);
    p.galley(area.min, g, theme::pal().text_2);
}

fn log_lines(ui: &mut Ui, body: Rect, log: &JobLog, job_id: u64) {
    if log.lines.is_empty() {
        let p = ui.painter_at(body);
        text_in(&p, body, Align2::CENTER_CENTER, "O log deste job está vazio.", theme::ui(11.0), theme::pal().text_2);
        return;
    }
    let font = theme::mono(11.0);
    let cell = ui.fonts_mut(|f| f.glyph_width(&font, 'M')).max(1.0);
    let max_chars = log.lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let content_w = (max_chars as f32 * cell + 24.0).max(body.width());
    let mut child = child_ui(ui, body);
    let n = log.lines.len();
    // Abre no fim do log, que é onde o erro aparece.
    ScrollArea::both().id_salt(("gh-log-scroll", job_id)).auto_shrink([false, false]).stick_to_bottom(true).show_rows(
        &mut child,
        LOG_LINE_H,
        n,
        |ui, range| {
            for (i, line) in log.lines[range.clone()].iter().enumerate() {
                let (r, _) = ui.allocate_exact_size(Vec2::new(content_w, LOG_LINE_H), Sense::hover());
                let p = ui.painter();
                let first_clipped = log.truncated && range.start + i == 0;
                let is_err = is_error_line(line);
                if is_err {
                    p.rect_filled(r, 0.0, theme::with_alpha(theme::pal().danger, 0.14));
                }
                let fg = if is_err {
                    theme::pal().danger_text
                } else if first_clipped {
                    theme::pal().text_2
                } else {
                    theme::pal().text_2
                };
                p.text(Pos2::new(r.min.x + 8.0, r.center().y), Align2::LEFT_CENTER, line, font.clone(), fg);
            }
        },
    );
}

/// Linha que o GitHub/compilador marca como erro.
fn is_error_line(line: &str) -> bool {
    let l = line.trim_start();
    l.starts_with("##[error]") || l.starts_with("error") || l.contains("FAILED") || l.contains("panicked at") || l.starts_with("Error:")
}

// ---------------------------------------------------------------------------
// Menu "Enviar para agente"
// ---------------------------------------------------------------------------

fn draw_agent_menu(deck: &mut Deck, ctx: &egui::Context) {
    let Some(target) = deck.github.agent_menu.clone() else { return };

    // Abas de agente abertas no projeto ativo (sem terminal comum).
    let open_tabs: Vec<(String, String, Color32)> = deck
        .visible()
        .into_iter()
        .filter(|&i| deck.tabs[i].kind != "shell")
        .take(8)
        .map(|i| (deck.tabs[i].ident.id(), deck.tabs[i].label.clone(), deck.tabs[i].color))
        .collect();

    // Modelos para abrir (ou reaproveitar) uma instância com cargo, ex.: claude@review.
    let models: Vec<(String, String, Color32)> =
        deck.agents.new_tab_menu.iter().filter(|m| m.kind != "shell").map(|m| (m.kind.clone(), m.label.clone(), m.color)).collect();
    if models.is_empty() {
        // A lista de agentes ainda não chegou do core: o menu aparece no quadro seguinte.
        return;
    }
    if !models.iter().any(|(k, _, _)| *k == deck.github.new_role_kind) {
        deck.github.new_role_kind = models[0].0.clone();
    }

    let w = 276.0;
    let item_h = 26.0;
    let model_h = 22.0;
    let tabs_h = open_tabs.len().max(1) as f32 * item_h;
    let h = 8.0 + 18.0 + tabs_h + 11.0 + 18.0 + models.len() as f32 * model_h + 6.0 + 22.0 + 6.0 + 24.0 + 8.0;
    let pos = super::menus::keep_on_screen(ctx, Pos2::new(target.anchor.min.x, target.anchor.max.y + 2.0), Vec2::new(w, h));
    let menu_rect = Rect::from_min_size(pos, Vec2::new(w, h));

    let mut chosen: Option<String> = None;
    let mut close_menu = ctx.input(|i| i.key_pressed(Key::Escape));

    Area::new(Id::new("gh-agent-menu")).order(Order::Foreground).fixed_pos(pos).constrain(false).show(ctx, |ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
        shadow(ui.painter(), rect, 8, 8, 24, theme::with_alpha(theme::pal().shadow, 0.5));
        frame(ui.painter(), rect, 8.0, theme::pal().hover, theme::pal().separator);
        let p = ui.painter().clone();
        let mut y = rect.min.y + 8.0;

        let head_r = Rect::from_min_size(Pos2::new(rect.min.x + 12.0, y), Vec2::new(w - 24.0, 18.0));
        let title = format!("ENVIAR {} PARA", target.title.to_uppercase());
        let g = galley_trunc(&p, &title, theme::ui_bold(10.5), theme::pal().text_2, head_r.width());
        place(&p, head_r, Align2::LEFT_CENTER, g, theme::pal().text_2);
        y += 18.0;

        if open_tabs.is_empty() {
            let r = Rect::from_min_size(Pos2::new(rect.min.x + 12.0, y), Vec2::new(w - 24.0, item_h));
            text_in(&p, r, Align2::LEFT_CENTER, "Nenhum agente aberto neste projeto", theme::ui(12.0), theme::pal().text_2);
            y += item_h;
        }
        for (id, label, color) in &open_tabs {
            let r = Rect::from_min_size(Pos2::new(rect.min.x + 4.0, y), Vec2::new(w - 8.0, item_h));
            y += item_h;
            let resp = clickable(ui, r, Id::new(("gh-menu-tab", id)));
            if resp.hovered() {
                p.rect_filled(r, 5.0, theme::pal().separator);
            }
            dot(&p, Pos2::new(r.min.x + 14.0, r.center().y), *color);
            let tr = Rect::from_min_max(Pos2::new(r.min.x + 26.0, r.min.y), Pos2::new(r.max.x - 8.0, r.max.y));
            let g = galley_trunc(&p, label, theme::ui(12.5), theme::pal().text, tr.width());
            place(&p, tr, Align2::LEFT_CENTER, g, theme::pal().text);
            if resp.clicked() {
                chosen = Some(id.clone());
            }
        }

        y += 4.0;
        p.rect_filled(Rect::from_min_size(Pos2::new(rect.min.x + 8.0, y), Vec2::new(w - 16.0, 1.0)), 0.0, theme::pal().separator);
        y += 7.0;
        let sub_r = Rect::from_min_size(Pos2::new(rect.min.x + 12.0, y), Vec2::new(w - 24.0, 18.0));
        text_in(&p, sub_r, Align2::LEFT_CENTER, "OU ABRIR / USAR UM CARGO", theme::ui_bold(10.5), theme::pal().text_2);
        y += 18.0;

        for (kind, label, color) in &models {
            let r = Rect::from_min_size(Pos2::new(rect.min.x + 4.0, y), Vec2::new(w - 8.0, model_h));
            y += model_h;
            let active = deck.github.new_role_kind == *kind;
            let resp = clickable(ui, r, Id::new(("gh-role-model", kind)));
            if resp.hovered() {
                p.rect_filled(r, 5.0, theme::pal().separator);
            }
            let c = Pos2::new(r.min.x + 14.0, r.center().y);
            p.circle_stroke(c, 5.0, egui::Stroke::new(1.0, if active { *color } else { theme::pal().text_2 }));
            if active {
                p.circle_filled(c, 3.0, *color);
            }
            let tr = Rect::from_min_max(Pos2::new(r.min.x + 28.0, r.min.y), Pos2::new(r.max.x - 8.0, r.max.y));
            let fg = if active { theme::pal().text } else { theme::pal().text_2 };
            let g = galley_trunc(&p, label, theme::ui(12.0), fg, tr.width());
            place(&p, tr, Align2::LEFT_CENTER, g, fg);
            if resp.clicked() {
                deck.github.new_role_kind = kind.clone();
            }
        }
        y += 6.0;

        // Cargo: caixa de texto + "Enviar".
        let btn_w = 70.0;
        let input_r = Rect::from_min_size(Pos2::new(rect.min.x + 8.0, y), Vec2::new(w - 16.0 - btn_w - 6.0, 22.0));
        let send_r = Rect::from_min_size(Pos2::new(input_r.max.x + 6.0, y), Vec2::new(btn_w, 22.0));
        frame(&p, input_r, 6.0, theme::pal().chip, theme::pal().separator);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(input_r.shrink(1.0)));
        let out = TextEdit::singleline(&mut deck.github.new_role_name)
            .id(Id::new("gh-role-input"))
            .frame(egui::Frame::NONE)
            .margin(egui::Margin { left: 8, right: 8, top: 3, bottom: 3 })
            .font(theme::mono(11.5))
            .hint_text(egui::RichText::new("cargo (ex.: review)").font(theme::ui(11.5)).color(theme::pal().text_3))
            .vertical_align(egui::Align::Center)
            .desired_width(input_r.width() - 2.0)
            .show(&mut child);
        let enter = out.response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        let spec = instance_spec(&deck.github.new_role_kind, &deck.github.new_role_name);
        if small_btn(ui, &p, send_r, "gh-role-submit", "Enviar →", &format!("Enviar para {spec} (a aba é aberta se não existir)"), true)
            || enter
        {
            chosen = Some(spec);
        }
        y += 28.0;

        // Atalhos de cargo.
        let roles = ["review", "ci", "fix", "dev"];
        let rw = (w - 16.0 - 3.0 * 4.0) / 4.0;
        for (i, role) in roles.iter().enumerate() {
            let rr = Rect::from_min_size(Pos2::new(rect.min.x + 8.0 + i as f32 * (rw + 4.0), y), Vec2::new(rw, 20.0));
            let resp = clickable(ui, rr, Id::new(("gh-quick-role", role)));
            let active = normalize_role(&deck.github.new_role_name) == *role;
            frame(&p, rr, 5.0, if active { theme::pal().separator } else { theme::pal().chip }, theme::pal().separator);
            let fg = if active || resp.hovered() { theme::pal().text } else { theme::pal().text_2 };
            text_in(&p, rr, Align2::CENTER_CENTER, &format!("@{role}"), theme::mono(10.5), fg);
            if resp.clicked() {
                deck.github.new_role_name = (*role).to_owned();
            }
        }
    });

    if let Some(spec) = chosen {
        deck.send_github_prompt_to_agent(&spec, target.prompt);
    } else if close_menu || super::menus::closes_on_outside(ctx, menu_rect, target.anchor) {
        close_menu = true;
    }
    if close_menu {
        deck.github.agent_menu = None;
    }
}

// ---------------------------------------------------------------------------
// Peças visuais
// ---------------------------------------------------------------------------

/// Bolinha do estado dos checks: cheia (verde/vermelho/âmbar) ou só o anel quando não há checks.
fn paint_check_badge(p: &egui::Painter, center: Pos2, status: CheckStatus) {
    let color = match status {
        CheckStatus::Success => theme::pal().success,
        CheckStatus::Failure => theme::pal().danger,
        CheckStatus::Pending => theme::pal().warning,
        CheckStatus::Neutral => {
            p.circle_stroke(center, 3.5, egui::Stroke::new(1.0, theme::pal().text_2));
            return;
        }
    };
    p.circle_filled(center, 4.0, color);
}

#[allow(clippy::too_many_arguments)]
fn icon_btn(
    deck: &mut Deck,
    ui: &Ui,
    p: &egui::Painter,
    rect: Rect,
    id: impl std::hash::Hash + std::fmt::Debug,
    icon: Icon,
    tooltip: &str,
    enabled: bool,
) -> bool {
    let r = if enabled { clickable(ui, rect, Id::new(id)) } else { ui.interact(rect, Id::new(id), Sense::hover()) };
    if enabled && r.hovered() {
        p.rect_filled(rect, 4.0, theme::pal().hover);
    }
    let color = if !enabled {
        theme::with_opacity(theme::pal().text_2, 0.4)
    } else if r.hovered() {
        theme::pal().text
    } else {
        theme::pal().text_2
    };
    deck.icons.paint(p, icon, rect.center(), 15.0, color, 0.0);
    tip(r, tooltip).clicked() && enabled
}

/// Botão de texto pequeno. `strong` = ação principal (negrito e realce no hover).
fn small_btn(
    ui: &Ui,
    p: &egui::Painter,
    rect: Rect,
    id: impl std::hash::Hash + std::fmt::Debug,
    label: &str,
    tooltip: &str,
    strong: bool,
) -> bool {
    let r = clickable(ui, rect, Id::new(id));
    let hot = r.hovered();
    let (bg, border) = match (strong, hot) {
        (true, true) => (theme::accent(), theme::accent()),
        (false, true) => (theme::pal().separator, theme::pal().separator),
        _ => (theme::pal().hover, theme::pal().separator),
    };
    frame(p, rect, 4.0, bg, border);
    let fg = if hot { theme::pal().text_strong } else { theme::pal().text };
    let font = if strong { theme::ui_bold(11.0) } else { theme::ui(11.0) };
    let g = galley_trunc(p, label, font, fg, rect.width() - 6.0);
    place(p, rect, Align2::CENTER_CENTER, g, fg);
    tip(r, tooltip).clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_lines_are_highlighted() {
        assert!(is_error_line("error[E0308]: mismatched types"));
        assert!(is_error_line("##[error]Process completed with exit code 101."));
        assert!(is_error_line("thread 'main' panicked at src/lib.rs:1:1"));
        assert!(!is_error_line("   Compiling agentdeck v0.3.0"));
    }

    #[test]
    fn list_and_detail_share_the_area() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(330.0, 600.0));
        let (list, detail) = split_list_detail(rect, false, 0.6, 180.0);
        assert_eq!(list, rect);
        assert!(detail.is_none());
        let (list, detail) = split_list_detail(rect, true, 0.6, 180.0);
        let detail = detail.expect("detalhe");
        assert_eq!(list.max.y, detail.min.y);
        assert!(detail.height() >= 180.0 && list.height() >= 44.0);
        // Área pequena: a lista nunca some por completo.
        let small = Rect::from_min_size(Pos2::ZERO, Vec2::new(330.0, 200.0));
        let (list, detail) = split_list_detail(small, true, 0.6, 180.0);
        assert!(list.height() >= 44.0 && detail.is_some());
        // Áreas minúsculas não derrubam o app.
        for h in [0.0, 10.0, 60.0, 120.0, 150.0] {
            let r = Rect::from_min_size(Pos2::ZERO, Vec2::new(330.0, h));
            let _ = split_list_detail(r, true, 0.6, 180.0);
        }
        assert_eq!(clamp_between(50.0, 96.0, 80.0), 80.0);
        assert_eq!(clamp_between(500.0, 96.0, 200.0), 200.0);
        assert_eq!(clamp_between(10.0, 96.0, 200.0), 96.0);
    }
}
