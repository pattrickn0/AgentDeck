//! Aba "Git" da barra direita: branch e sincronização no topo, alterações em seções (ou o diff de
//! um arquivo) no meio e a caixa de commit embaixo. O estado e as operações ficam em `git_ui.rs`.
//!
//! O desenho só *coleta* o que foi clicado ([`Act`]); as ações são aplicadas no fim, com o estado
//! inteiro de volta no `Deck`.

use egui::{Align2, Area, Color32, Id, Key, Modifiers, Order, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use agentdeck_core::git::{
    valid_branch_name, ConflictChoice, DiffKind, EntryKind, GitBranch, GitCommit, GitDiff, GitEntry, GitStash, GitStatus, RepoState,
};

use super::git_views;
use super::{
    button, button_danger, button_width, clickable, frame, galley, galley_spaced, galley_trunc, place, section_label, segmented, separator,
    shadow, text_in, tip, ButtonKind,
};
use crate::deck::Deck;
use crate::git_ui::{sections, BranchMenuMode, ConfirmAction, GitSubView, GitView, OpenDiff, Row, Section};
use crate::icons::Icon;
use crate::theme;

const HEAD_H: f32 = 34.0;
const BANNER_H: f32 = 24.0;
const MODE_H: f32 = 24.0;
const ROW_H: f32 = 22.0;
const DIFF_ROW_H: f32 = 17.0;
/// O botão "Abrir ao centro" (largura aproximada do texto de 11,5 px com o recuo) que leva o diff ao modo Documentos.
const OPEN_CENTER: &str = "Abrir ao centro";
const OPEN_CENTER_W: f32 = 104.0;
const DIFF_FONT: f32 = 12.0;
const MSG_FONT: f32 = 13.0;
const MSG_ROWS: f32 = 3.0;
pub(super) const BTN_H: f32 = 28.0;
pub const COMMIT_ID: &str = "git-commit-msg";

/// O que foi clicado neste quadro.
pub(super) enum Act {
    Refresh,
    Init,
    Copy(String),
    Fetch,
    Pull,
    Push,
    BranchMenu,
    Stage(Vec<String>),
    Unstage(Vec<String>),
    UnstageAll,
    AskDiscard(GitEntry),
    OpenDiff(GitEntry, bool),
    /// "Abrir ao centro": o diff do arquivo, só leitura, no modo Documentos.
    OpenDiffCenter(GitEntry, bool),
    /// "Abrir ao centro" no detalhe de um commit: hash, hash curto e assunto.
    OpenCommitCenter(String, String, String),
    CloseDiff,
    OpenFile(String),
    Commit,
    ToggleAmend,
    Toggle(Section),
    DismissNotice,
    SetSub(GitSubView),
    /// Prepara/tira da preparação o trecho (hunk) `n` do diff aberto.
    HunkStage(usize),
    HunkDiscard(usize),
    Continue,
    AskAbort(RepoState),
    MoreLog,
    OpenCommit(GitCommit),
    CloseCommit,
    StashPush,
    StashToggleUntracked,
    OpenStash(GitStash),
    CloseStash,
    StashApply(usize),
    StashPop(usize),
    AskDropStash(GitStash),
    OpenConflict(String),
    CloseConflict,
    ResolveFile(String, ConflictChoice),
    ResolveBlock(String, usize, ConflictChoice),
}

fn letter_color(letter: char, section: Section) -> Color32 {
    match (section, letter) {
        (Section::Conflicts, _) => theme::pal().danger_text,
        (_, 'M') | (_, 'T') => theme::pal().warning,
        (_, 'A') | (Section::Untracked, _) => theme::pal().success,
        (_, 'D') => theme::pal().danger_text,
        (_, 'R') | (_, 'C') => theme::pal().info,
        _ => theme::pal().text_2,
    }
}

/// Nome do arquivo e a pasta (`crates/app/x.rs` → `x.rs`, `crates/app`).
fn split_path(path: &str) -> (&str, &str) {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(i) => (&path[i + 1..], &trimmed[..i]),
        None => (path, ""),
    }
}

pub fn draw(deck: &mut Deck, ui: &mut Ui, rect: Rect) {
    let mut acts = Vec::new();
    // O status sai do Deck enquanto desenha (evita clonar milhares de entradas por quadro).
    let view = std::mem::replace(&mut deck.git.view, GitView::Loading);
    match &view {
        GitView::Loading => message(ui, rect, "Lendo o repositório…", None, &[], &mut acts),
        GitView::NoGit => message(
            ui,
            rect,
            "O git não foi encontrado.",
            Some("Instale em git-scm.com e clique em Tentar de novo."),
            &[("Tentar de novo", Act::Refresh)],
            &mut acts,
        ),
        GitView::NotRepo => message(
            ui,
            rect,
            "Esta pasta não é um repositório git.",
            Some("Crie um repositório aqui, ou clone um projeto pelo terminal (git clone …) e abra a pasta dele."),
            &[("Inicializar repositório", Act::Init)],
            &mut acts,
        ),
        GitView::Unsafe(path) => {
            let cmd = format!("git config --global --add safe.directory \"{path}\"");
            message(
                ui,
                rect,
                "O git não confia nesta pasta (ela pertence a outro usuário).",
                Some(&format!("Para liberar, rode no terminal:\n{cmd}")),
                &[("Copiar comando", Act::Copy(cmd.clone())), ("Tentar de novo", Act::Refresh)],
                &mut acts,
            );
        }
        GitView::Error(e) => message(ui, rect, "Não consegui ler o repositório.", Some(e), &[("Tentar de novo", Act::Refresh)], &mut acts),
        GitView::Ready(st) => ready(deck, ui, rect, st, &mut acts),
    }
    deck.git.view = view;
    for act in acts {
        apply(deck, ui.ctx(), act);
    }
    confirm_dialog(deck, ui.ctx(), rect);
}

fn apply(deck: &mut Deck, ctx: &egui::Context, act: Act) {
    match act {
        Act::Refresh => {
            deck.git.notice = None;
            deck.git_refresh();
        }
        Act::Init => deck.git_init(),
        Act::Copy(text) => ctx.copy_text(text),
        Act::Fetch => deck.git_fetch(),
        Act::Pull => deck.git_pull(),
        Act::Push => deck.git_push(),
        Act::BranchMenu => deck.git_open_branch_menu(BranchMenuMode::Switch),
        Act::Stage(p) => deck.git_stage(p),
        Act::Unstage(p) => deck.git_unstage(p),
        Act::UnstageAll => deck.git_unstage_all(),
        Act::AskDiscard(e) => deck.git.confirm = Some(ConfirmAction::DiscardFile(e)),
        Act::OpenDiff(e, staged) => deck.git_open_diff(e, staged),
        Act::OpenDiffCenter(e, staged) => deck.docs_open_file_diff(&e, staged),
        Act::OpenCommitCenter(hash, short, subject) => deck.docs_open_commit(&hash, &short, &subject),
        Act::CloseDiff => deck.git.diff = None,
        Act::OpenFile(p) => deck.git_open_file(&p),
        Act::Commit => deck.git_commit(),
        Act::ToggleAmend => deck.git_toggle_amend(),
        Act::Toggle(s) => {
            if !deck.git.collapsed.remove(&s) {
                deck.git.collapsed.insert(s);
            }
        }
        Act::DismissNotice => deck.git.notice = None,
        Act::SetSub(v) => deck.git_set_subview(v),
        Act::HunkStage(n) => deck.git_stage_or_unstage_hunk(n),
        Act::HunkDiscard(n) => deck.git_ask_discard_hunk(n),
        Act::Continue => deck.git_continue_op(),
        Act::AskAbort(state) => deck.git.confirm = Some(ConfirmAction::AbortOp(state)),
        Act::MoreLog => deck.git_load_log(true),
        Act::OpenCommit(c) => deck.git_open_commit(c),
        Act::CloseCommit => deck.git.commit_detail = None,
        Act::StashPush => deck.git_stash_push(),
        Act::StashToggleUntracked => deck.git.stash_include_untracked = !deck.git.stash_include_untracked,
        Act::OpenStash(s) => deck.git_open_stash_diff(s),
        Act::CloseStash => deck.git.stash_diff = None,
        Act::StashApply(i) => deck.git_stash_apply(i),
        Act::StashPop(i) => deck.git_stash_pop(i),
        Act::AskDropStash(s) => deck.git.confirm = Some(ConfirmAction::DropStash(s)),
        Act::OpenConflict(p) => deck.git_open_conflict(p),
        Act::CloseConflict => deck.git.conflict_open = None,
        Act::ResolveFile(p, c) => deck.git_resolve_conflict_file(p, c),
        Act::ResolveBlock(p, i, c) => deck.git_resolve_conflict_block(p, i, c),
    }
}

/// Estado vazio: título, explicação e botões.
fn message(ui: &mut Ui, rect: Rect, title: &str, detail: Option<&str>, buttons: &[(&str, Act)], acts: &mut Vec<Act>) {
    let painter = ui.painter_at(rect);
    let p = theme::pal();
    let w = (rect.width() - 40.0).max(80.0);
    let mut y = rect.min.y + 32.0;
    let g = painter.layout(title.to_owned(), theme::ui_semibold(13.0), p.text_strong, w);
    painter.galley(Pos2::new(rect.min.x + 20.0, y), g.clone(), p.text_strong);
    y += g.size().y + 6.0;
    if let Some(d) = detail {
        let g = painter.layout(d.to_owned(), theme::ui(12.0), p.text_2, w);
        painter.galley(Pos2::new(rect.min.x + 20.0, y), g.clone(), p.text_2);
        y += g.size().y + 14.0;
    }
    let mut x = rect.min.x + 20.0;
    for (i, (label, act)) in buttons.iter().enumerate() {
        let kind = if i == 0 { ButtonKind::Primary } else { ButtonKind::Secondary };
        let bw = button_width(&painter, label, kind);
        let r = Rect::from_min_size(Pos2::new(x, y), Vec2::new(bw, BTN_H));
        if button(ui, r, Id::new(("git-msg-btn", i)), label, kind).clicked() {
            acts.push(match act {
                Act::Refresh => Act::Refresh,
                Act::Init => Act::Init,
                Act::Copy(t) => Act::Copy(t.clone()),
                _ => Act::Refresh,
            });
        }
        x = r.max.x + 8.0;
    }
}

/// Botão pequeno com ícone (e texto opcional à direita do ícone).
fn icon_button(deck: &mut Deck, ui: &Ui, r: Rect, icon: Icon, label: &str, tooltip: &str, enabled: bool) -> bool {
    let resp = if enabled {
        clickable(ui, r, Id::new(("git-btn", tooltip)))
    } else {
        ui.interact(r, Id::new(("git-btn", tooltip)), Sense::hover())
    };
    let painter = ui.painter();
    if enabled && resp.hovered() {
        painter.rect_filled(r, 6.0, theme::pal().separator);
    }
    let color = if enabled {
        if resp.hovered() {
            theme::pal().text
        } else {
            theme::pal().text_2
        }
    } else {
        theme::with_opacity(theme::pal().text_2, 0.45)
    };
    if label.is_empty() {
        deck.icons.paint(painter, icon, r.center(), 15.0, color, 0.0);
    } else {
        deck.icons.paint(painter, icon, Pos2::new(r.min.x + 12.0, r.center().y), 14.0, color, 0.0);
        text_in(painter, Rect::from_min_max(Pos2::new(r.min.x + 21.0, r.min.y), r.max), Align2::LEFT_CENTER, label, theme::ui(12.0), color);
    }
    tip(resp, tooltip).clicked() && enabled
}

fn icon_button_w(painter: &egui::Painter, label: &str) -> f32 {
    if label.is_empty() {
        28.0
    } else {
        galley(painter, label, theme::ui(12.0), theme::pal().text).size().x + 28.0
    }
}

fn ready(deck: &mut Deck, ui: &mut Ui, rect: Rect, st: &GitStatus, acts: &mut Vec<Act>) {
    let painter = ui.painter_at(rect);
    let busy = deck.git.busy.is_some();

    // ---------- Topo: branch e sincronização ----------
    let head = Rect::from_min_size(rect.min, Vec2::new(rect.width(), HEAD_H));
    let mut right = head.max.x - 6.0;
    let mut place_btn = |label: &str| {
        let w = icon_button_w(&painter, label);
        let r = Rect::from_min_size(Pos2::new(right - w, head.min.y + 3.0), Vec2::new(w, HEAD_H - 6.0));
        right = r.min.x - 2.0;
        r
    };
    let (push_label, push_tip) = if st.upstream.is_none() {
        ("Publicar".to_owned(), "Publicar a branch no remoto (git push -u)".to_owned())
    } else if st.ahead > 0 {
        (st.ahead.to_string(), format!("Enviar {} commit(s) (git push)", st.ahead))
    } else {
        (String::new(), "Enviar (git push)".to_owned())
    };
    let push_r = place_btn(&push_label);
    let pull_label = if st.behind > 0 { st.behind.to_string() } else { String::new() };
    let pull_r = place_btn(&pull_label);
    let fetch_r = place_btn("");
    let can_sync = !busy && st.branch.is_some();
    if icon_button(deck, ui, push_r, Icon::ArrowUp, &push_label, &push_tip, can_sync) {
        acts.push(Act::Push);
    }
    let pull_tip =
        if st.behind > 0 { format!("Puxar {} commit(s) (git pull --ff-only)", st.behind) } else { "Puxar (git pull --ff-only)".into() };
    if icon_button(deck, ui, pull_r, Icon::ArrowDown, &pull_label, &pull_tip, can_sync && st.upstream.is_some()) {
        acts.push(Act::Pull);
    }
    if icon_button(deck, ui, fetch_r, Icon::Refresh, "", "Buscar do remoto (git fetch)", !busy) {
        acts.push(Act::Fetch);
    }
    // Botão da branch (abre o menu).
    let branch_r = Rect::from_min_max(Pos2::new(head.min.x + 6.0, head.min.y + 3.0), Pos2::new(right - 4.0, head.max.y - 3.0));
    let resp = clickable(ui, branch_r, Id::new("git-branch-btn"));
    if resp.hovered() || deck.git.branch_menu {
        painter.rect_filled(branch_r, 6.0, theme::pal().hover);
    }
    deck.icons.paint(&painter, Icon::GitBranch, Pos2::new(branch_r.min.x + 12.0, branch_r.center().y), 14.0, theme::pal().text, 0.0);
    let name = st.branch.clone().unwrap_or_else(|| match &st.head {
        Some(h) => format!("{} (destacado)", h.chars().take(7).collect::<String>()),
        None => "(sem commits)".into(),
    });
    let count = st.entries.len();
    let label = if count > 0 { format!("{name} · {count}  ▾") } else { format!("{name}  ▾") };
    let g = galley_trunc(&painter, &label, theme::ui_semibold(12.5), theme::pal().text, branch_r.width() - 30.0);
    place(
        &painter,
        Rect::from_min_max(Pos2::new(branch_r.min.x + 24.0, branch_r.min.y), branch_r.max),
        Align2::LEFT_CENTER,
        g,
        theme::pal().text,
    );
    let tip_text = match &st.upstream {
        Some(u) => {
            format!("Trocar ou criar branch\n{name} · {} alteração(ões)\nAcompanha {u} · {} à frente, {} atrás", count, st.ahead, st.behind)
        }
        None => format!("Trocar ou criar branch\n{name} · {} alteração(ões)\nSem upstream (ainda não publicada)", count),
    };
    if tip(resp, tip_text).clicked() && !busy {
        acts.push(Act::BranchMenu);
    }
    painter.rect_filled(Rect::from_min_max(Pos2::new(head.min.x, head.max.y - 1.0), head.max), 0.0, theme::pal().separator);

    // ---------- Faixas ----------
    let mut y = head.max.y;
    if let Some(label) = deck.git.busy {
        banner(ui, rect, &mut y, label, theme::pal().text, false, acts);
        busy_bar(ui, Rect::from_min_max(Pos2::new(rect.min.x, y - 2.0), Pos2::new(rect.max.x, y)));
    }
    y += git_views::op_banner(ui, rect, y, st, busy, acts);
    if !st.prefix.is_empty() {
        let text = format!("Repositório em {} (a pasta de trabalho é a subpasta {})", st.root.display(), st.prefix.trim_end_matches('/'));
        banner(ui, rect, &mut y, &text, theme::pal().text_2, false, acts);
    }
    if st.truncated {
        banner(ui, rect, &mut y, "Alterações demais: mostrando as primeiras 5000.", theme::pal().warning, false, acts);
    }
    if let Some((text, error)) = deck.git.notice.clone() {
        banner(ui, rect, &mut y, &text, if error { theme::pal().danger_text } else { theme::pal().text_2 }, true, acts);
    }

    y += git_views::tab_strip(deck, ui, rect, y, st, acts);

    // ---------- Rodapé: commit (só em Alterações) ----------
    let changes_view = deck.git.subview == GitSubView::Changes;
    let msg_h = MSG_ROWS * MSG_FONT * theme::LINE_HEIGHT + 14.0;
    let footer_h = if changes_view { 8.0 + msg_h + 6.0 + BTN_H + 10.0 } else { 0.0 };
    let footer = Rect::from_min_max(Pos2::new(rect.min.x, rect.max.y - footer_h), rect.max);
    if changes_view {
        let staged = st.entries.iter().filter(|e| e.staged()).count();
        commit_box(deck, ui, footer, msg_h, staged, busy, acts);
    }

    // ---------- Meio: a visão escolhida ----------
    let body = Rect::from_min_max(Pos2::new(rect.min.x, y), Pos2::new(rect.max.x, footer.min.y));
    if body.height() >= 20.0 {
        match deck.git.subview {
            GitSubView::Changes => {
                if let Some(open) = deck.git.diff.take() {
                    diff_view(deck, ui, body, &open, busy, acts);
                    deck.git.diff = Some(open);
                } else {
                    lists(deck, ui, body, st, busy, acts);
                }
            }
            GitSubView::History => git_views::history(deck, ui, body, acts),
            GitSubView::Stash => git_views::stash(deck, ui, body, st, busy, acts),
            GitSubView::Conflicts => git_views::conflicts(deck, ui, body, st, busy, acts),
        }
    }

    branch_menu(deck, ui.ctx(), branch_r, busy);
}

/// Faixa de uma linha sob o topo (aviso, estado do repositório, erro com ×). Avança `y`.
fn banner(ui: &mut Ui, rect: Rect, y: &mut f32, text: &str, color: Color32, dismiss: bool, acts: &mut Vec<Act>) {
    let r = Rect::from_min_size(Pos2::new(rect.min.x, *y), Vec2::new(rect.width(), BANNER_H));
    *y = r.max.y;
    let p = ui.painter_at(r);
    p.rect_filled(r, 0.0, theme::pal().chip);
    let text_r = Rect::from_min_max(Pos2::new(r.min.x + 12.0, r.min.y), Pos2::new(r.max.x - if dismiss { 30.0 } else { 12.0 }, r.max.y));
    let g = galley_trunc(&p, text, theme::ui(11.5), color, text_r.width());
    place(&p, text_r, Align2::LEFT_CENTER, g, color);
    let hover = ui.interact(text_r, Id::new(("git-banner", r.min.y as i32)), Sense::hover());
    tip(hover, text.to_owned());
    if dismiss {
        let x = Rect::from_min_size(Pos2::new(r.max.x - 26.0, r.min.y + 2.0), Vec2::splat(BANNER_H - 4.0));
        let xr = clickable(ui, x, Id::new("git-notice-x"));
        text_in(&p, x, Align2::CENTER_CENTER, "×", theme::ui(14.0), if xr.hovered() { theme::pal().text } else { theme::pal().text_2 });
        if xr.clicked() {
            acts.push(Act::DismissNotice);
        }
    }
}

/// Faixa indeterminada (vai e volta) de operação em andamento.
pub(super) fn busy_bar(ui: &Ui, r: Rect) {
    let t = ui.ctx().input(|i| i.time);
    let phase = ((t * 0.9) % 2.0) as f32;
    let pos = if phase < 1.0 { phase } else { 2.0 - phase };
    let w = r.width() * 0.3;
    let bar = Rect::from_min_size(Pos2::new(r.min.x + (r.width() - w) * pos, r.min.y), Vec2::new(w, r.height()));
    ui.painter().rect_filled(bar, 1.0, theme::accent());
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
}

fn lists(deck: &mut Deck, ui: &mut Ui, body: Rect, st: &GitStatus, busy: bool, acts: &mut Vec<Act>) {
    let groups = sections(st);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body).layout(egui::Layout::top_down(egui::Align::Min)));
    child.set_clip_rect(body);
    egui::ScrollArea::vertical().id_salt("git-lists").auto_shrink([false, false]).show(&mut child, |ui| {
        ui.add_space(4.0);
        if groups.is_empty() {
            let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 60.0), Sense::hover());
            let text = if st.head.is_none() {
                "Nada para fazer commit ainda: crie arquivos na pasta de trabalho."
            } else {
                "Nenhuma alteração. Tudo commitado."
            };
            let g = ui.painter().layout(text.to_owned(), theme::ui(12.5), theme::pal().text_2, r.width() - 28.0);
            ui.painter().galley(Pos2::new(r.min.x + 14.0, r.min.y + 14.0), g, theme::pal().text_2);
        }
        for (section, rows) in &groups {
            section_header(deck, ui, *section, rows, busy, acts);
            if deck.git.collapsed.contains(section) {
                continue;
            }
            for (i, row) in rows.iter().enumerate() {
                file_row(deck, ui, *section, i, row, busy, acts);
            }
        }
        ui.add_space(8.0);
    });
}

fn section_header(deck: &mut Deck, ui: &mut Ui, section: Section, rows: &[Row], busy: bool, acts: &mut Vec<Act>) {
    let count = rows.len();
    let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H + 2.0), Sense::click());
    let painter = ui.painter();
    let p = theme::pal();
    let collapsed = deck.git.collapsed.contains(&section);
    deck.icons.paint(
        painter,
        Icon::TreeChevron,
        Pos2::new(r.min.x + 12.0, r.center().y),
        10.0,
        p.text_3,
        if collapsed { 0.0 } else { std::f32::consts::FRAC_PI_2 },
    );
    let tg = galley_spaced(painter, &section.title().to_uppercase(), theme::label(10.0), p.text_3, 10.0 * theme::LABEL_SPACING_EM);
    let tr = place(painter, Rect::from_min_max(Pos2::new(r.min.x + 22.0, r.min.y), r.max), Align2::LEFT_CENTER, tg, p.text_3);
    text_in(
        painter,
        Rect::from_min_max(Pos2::new(tr.max.x + 6.0, r.min.y), r.max),
        Align2::LEFT_CENTER,
        &count.to_string(),
        theme::meta(10.0),
        p.text_3,
    );
    // Ação em lote à direita.
    let (label, tooltip, act) = match section {
        Section::Staged => ("−", "Tirar tudo da preparação", Some(Act::UnstageAll)),
        // Só os arquivos desta seção (o "+" das alterações não prepara os não rastreados e vice-versa).
        Section::Changes => ("+", "Preparar as alterações", Some(Act::Stage(rows.iter().map(|r| r.entry.path.clone()).collect()))),
        Section::Untracked => ("+", "Preparar os não rastreados", Some(Act::Stage(rows.iter().map(|r| r.entry.path.clone()).collect()))),
        Section::Conflicts => ("", "", None),
    };
    if let Some(act) = act {
        let b = Rect::from_min_size(Pos2::new(r.max.x - 30.0, r.min.y + 2.0), Vec2::new(24.0, ROW_H - 2.0));
        let br = if busy {
            ui.interact(b, Id::new(("git-all", section)), Sense::hover())
        } else {
            clickable(ui, b, Id::new(("git-all", section)))
        };
        if br.hovered() && !busy {
            painter.rect_filled(b, 5.0, theme::pal().separator);
        }
        text_in(
            painter,
            b,
            Align2::CENTER_CENTER,
            label,
            theme::ui_semibold(15.0),
            if busy { theme::with_opacity(theme::pal().text_2, 0.4) } else { theme::pal().text_2 },
        );
        if tip(br, tooltip).clicked() && !busy {
            acts.push(act);
            return;
        }
    }
    if resp.clicked() {
        acts.push(Act::Toggle(section));
    }
}

fn file_row(deck: &mut Deck, ui: &mut Ui, section: Section, i: usize, row: &crate::git_ui::Row, busy: bool, acts: &mut Vec<Act>) {
    let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
    let hovered = ui.rect_contains_pointer(r);
    let painter = ui.painter().clone();
    if hovered {
        painter.rect_filled(r.shrink2(Vec2::new(4.0, 0.0)), 4.0, theme::pal().hover);
    }
    // Ações (aparecem no hover).
    let mut buttons: Vec<(&str, &str, Act)> = Vec::new();
    let path = row.entry.path.clone();
    match section {
        Section::Conflicts => buttons.push(("+", "Marcar como resolvido (prepara o arquivo como está)", Act::Stage(vec![path.clone()]))),
        Section::Staged => buttons.push(("−", "Tirar da preparação", Act::Unstage(vec![path.clone()]))),
        Section::Changes => {
            buttons.push(("+", "Preparar", Act::Stage(vec![path.clone()])));
            buttons.push(("↶", "Descartar alterações", Act::AskDiscard(row.entry.clone())));
        }
        Section::Untracked => {
            buttons.push(("+", "Preparar", Act::Stage(vec![path.clone()])));
            buttons.push(("↶", "Apagar arquivo não rastreado", Act::AskDiscard(row.entry.clone())));
        }
    }
    let mut bx = r.max.x - 8.0;
    let mut clicked_button = false;
    if hovered && !busy {
        for (j, (label, tooltip, act)) in buttons.into_iter().enumerate() {
            let b = Rect::from_min_size(Pos2::new(bx - 22.0, r.min.y + 1.0), Vec2::new(22.0, ROW_H - 2.0));
            bx = b.min.x - 2.0;
            let br = clickable(ui, b, Id::new(("git-row-btn", section, i, j)));
            if br.hovered() {
                painter.rect_filled(b, 5.0, theme::pal().separator);
            }
            if label == "↶" {
                deck.icons.paint(&painter, Icon::Undo, b.center(), 13.0, theme::pal().text, 0.0);
            } else {
                text_in(&painter, b, Align2::CENTER_CENTER, label, theme::ui_semibold(15.0), theme::pal().text);
            }
            if tip(br, tooltip).clicked() {
                acts.push(act);
                clicked_button = true;
            }
        }
    }
    // Letra, nome e pasta.
    let lx = r.min.x + 14.0;
    text_in(
        &painter,
        Rect::from_min_size(Pos2::new(lx, r.min.y), Vec2::new(12.0, r.height())),
        Align2::CENTER_CENTER,
        &row.letter.to_string(),
        theme::mono_bold(12.0),
        letter_color(row.letter, section),
    );
    let (name, dir) = split_path(&row.entry.path);
    let name_shown = match &row.entry.orig_path {
        Some(orig) if row.entry.kind == EntryKind::Renamed => format!("{} ← {}", name, split_path(orig).0),
        _ => name.to_owned(),
    };
    let text_x = lx + 18.0;
    let avail = (bx - text_x - 6.0).max(20.0);
    let ng = galley_trunc(&painter, &name_shown, theme::ui(12.5), theme::pal().text, avail);
    let nw = ng.size().x;
    place(
        &painter,
        Rect::from_min_max(Pos2::new(text_x, r.min.y), Pos2::new(text_x + nw, r.max.y)),
        Align2::LEFT_CENTER,
        ng,
        theme::pal().text,
    );
    if !dir.is_empty() && avail - nw > 30.0 {
        let dg = galley_trunc(&painter, dir, theme::ui(11.5), theme::pal().text_2, avail - nw - 8.0);
        place(
            &painter,
            Rect::from_min_max(Pos2::new(text_x + nw + 8.0, r.min.y), Pos2::new(bx, r.max.y)),
            Align2::LEFT_CENTER,
            dg,
            theme::pal().text_2,
        );
    }
    let resp = tip(resp, row.entry.path.clone());
    if !clicked_button {
        if resp.double_clicked() {
            acts.push(Act::OpenFile(row.entry.path.clone()));
        } else if resp.clicked() {
            if section == Section::Conflicts {
                acts.push(Act::OpenConflict(row.entry.path.clone()));
            } else {
                acts.push(Act::OpenDiff(row.entry.clone(), row.staged));
            }
        }
    }
}

fn diff_view(deck: &mut Deck, ui: &mut Ui, body: Rect, open: &OpenDiff, busy: bool, acts: &mut Vec<Act>) {
    let painter = ui.painter_at(body);
    // Cabeçalho: voltar, caminho, ações.
    let head = Rect::from_min_size(body.min, Vec2::new(body.width(), 28.0));
    painter.rect_filled(head, 0.0, theme::pal().chip);
    let back = Rect::from_min_size(Pos2::new(head.min.x + 6.0, head.min.y + 3.0), Vec2::new(26.0, 22.0));
    let br = clickable(ui, back, Id::new("git-diff-back"));
    if br.hovered() {
        painter.rect_filled(back, 5.0, theme::pal().separator);
    }
    text_in(&painter, back, Align2::CENTER_CENTER, "←", theme::ui(15.0), theme::pal().text);
    if tip(br, "Voltar às alterações").clicked() {
        acts.push(Act::CloseDiff);
    }
    let mut right = head.max.x - 6.0;
    let entry = &open.entry;
    let mut buttons: Vec<(&str, &str, Act)> = Vec::new();
    if open.staged {
        buttons.push(("−", "Tirar da preparação", Act::Unstage(vec![entry.path.clone()])));
    } else {
        buttons.push(("+", "Preparar", Act::Stage(vec![entry.path.clone()])));
        if entry.kind != EntryKind::Unmerged {
            buttons.push(("↶", "Descartar alterações", Act::AskDiscard(entry.clone())));
        }
    }
    for (j, (label, tooltip, act)) in buttons.into_iter().enumerate() {
        let b = Rect::from_min_size(Pos2::new(right - 24.0, head.min.y + 3.0), Vec2::new(24.0, 22.0));
        right = b.min.x - 2.0;
        let r = if busy {
            ui.interact(b, Id::new(("git-diff-btn", j)), Sense::hover())
        } else {
            clickable(ui, b, Id::new(("git-diff-btn", j)))
        };
        if r.hovered() && !busy {
            painter.rect_filled(b, 5.0, theme::pal().separator);
        }
        let color = if busy { theme::with_opacity(theme::pal().text_2, 0.4) } else { theme::pal().text };
        if label == "↶" {
            deck.icons.paint(&painter, Icon::Undo, b.center(), 13.0, color, 0.0);
        } else {
            text_in(&painter, b, Align2::CENTER_CENTER, label, theme::ui_semibold(15.0), color);
        }
        if tip(r, tooltip).clicked() && !busy {
            acts.push(act);
        }
    }
    // "Abrir ao centro": na mesma linha se sobra lugar para o caminho; senão, numa linha só dele.
    let center_tip = "Abre este diff, só para ler, com a largura toda do centro (o preparo por trecho continua aqui)";
    let mut extra_row = 0.0;
    if right - (back.max.x + 8.0) - (OPEN_CENTER_W + 6.0) >= 150.0 {
        let id = Id::new("git-diff-center");
        if git_views::pill(ui, &mut right, head.min.y + 3.0, 22.0, OPEN_CENTER, center_tip, git_views::Look::Normal, true, id) {
            acts.push(Act::OpenDiffCenter(entry.clone(), open.staged));
        }
    } else {
        extra_row = 26.0;
        let row = Rect::from_min_size(Pos2::new(body.min.x, head.max.y), Vec2::new(body.width(), extra_row));
        painter.rect_filled(row, 0.0, theme::pal().chip);
        let mut r = row.max.x - 6.0;
        let id = Id::new("git-diff-center");
        if git_views::pill(ui, &mut r, row.min.y + 2.0, 22.0, OPEN_CENTER, center_tip, git_views::Look::Normal, true, id) {
            acts.push(Act::OpenDiffCenter(entry.clone(), open.staged));
        }
    }
    let side = if open.staged { "preparado" } else { "não preparado" };
    let title = format!("{}  · {side}", entry.path);
    let g = galley_trunc(&painter, &title, theme::ui(12.0), theme::pal().text, right - back.max.x - 12.0);
    place(
        &painter,
        Rect::from_min_max(Pos2::new(back.max.x + 8.0, head.min.y), Pos2::new(right, head.max.y)),
        Align2::LEFT_CENTER,
        g,
        theme::pal().text,
    );
    let hover = ui.interact(
        Rect::from_min_max(Pos2::new(back.max.x + 8.0, head.min.y), Pos2::new(right, head.max.y)),
        Id::new("git-diff-title"),
        Sense::click(),
    );
    if tip(hover, format!("{}\n(duplo clique abre o arquivo)", entry.path)).double_clicked() {
        acts.push(Act::OpenFile(entry.path.clone()));
    }

    let area = Rect::from_min_max(Pos2::new(body.min.x, head.max.y + extra_row), body.max);
    match &open.diff {
        None => {
            text_in(&ui.painter_at(area), area.shrink(14.0), Align2::LEFT_TOP, "Carregando o diff…", theme::ui(12.5), theme::pal().text_2);
        }
        Some(Err(e)) => {
            let g = ui.painter().layout(e.clone(), theme::ui(12.5), theme::pal().danger_text, area.width() - 28.0);
            ui.painter_at(area).galley(area.min + Vec2::splat(14.0), g, theme::pal().danger_text);
        }
        Some(Ok(diff)) => {
            // Trechos só valem para um arquivo rastreado e sem conflito.
            let hunks =
                (!matches!(entry.kind, EntryKind::Untracked | EntryKind::Unmerged)).then_some(HunkCtx { staged: open.staged, busy });
            diff_lines(ui, area, diff, hunks, acts)
        }
    }
}

/// Botões por trecho (hunk) do diff: preparar/tirar e descartar.
#[derive(Clone, Copy)]
pub(super) struct HunkCtx {
    staged: bool,
    busy: bool,
}

pub(super) fn diff_lines(ui: &mut Ui, area: Rect, diff: &GitDiff, hunks: Option<HunkCtx>, acts: &mut Vec<Act>) {
    let id = ui.id().with(("git-diff", area.min.x as i32, area.min.y as i32));
    diff_lines_at(ui, area, diff, hunks, acts, id, None);
}

/// O desenho do diff, com a identidade da rolagem escolhida por quem chama (cada documento do modo
/// Documentos tem a sua) e, em `jump`, a linha do diff para onde rolar neste quadro.
pub(super) fn diff_lines_at(
    ui: &mut Ui,
    area: Rect,
    diff: &GitDiff,
    hunks: Option<HunkCtx>,
    acts: &mut Vec<Act>,
    id: Id,
    jump: Option<usize>,
) {
    let mut notes: Vec<&str> = Vec::new();
    if diff.binary {
        notes.push("Arquivo binário: o conteúdo não é mostrado.");
    } else if diff.lines.is_empty() {
        notes.push("Sem diferenças para mostrar.");
    }
    if diff.truncated {
        notes.push("Diff grande: mostrando só o começo.");
    }
    let font = theme::mono(DIFF_FONT);
    let cell = ui.fonts_mut(|f| f.glyph_width(&font, 'M')).max(1.0);
    let gutter = cell * 10.0 + 10.0;
    let max_chars = diff.lines.iter().map(|l| l.text.chars().count()).max().unwrap_or(0);
    let content_w = (gutter + max_chars as f32 * cell + 24.0).max(area.width());
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::top_down(egui::Align::Min)));
    child.set_clip_rect(area);
    for note in &notes {
        let (r, _) = child.allocate_exact_size(Vec2::new(area.width(), 22.0), Sense::hover());
        text_in(child.painter(), r.shrink2(Vec2::new(12.0, 0.0)), Align2::LEFT_CENTER, note, theme::ui(12.0), theme::pal().warning);
    }
    let n = diff.lines.len();
    // Diff combinado (`@@@`, de um conflito) não vira patch de trecho.
    let hunks = hunks
        .filter(|_| !diff.binary && !diff.truncated && !diff.lines.iter().any(|l| l.kind == DiffKind::Hunk && l.text.starts_with("@@@")));
    let mut scroll = egui::ScrollArea::both().id_salt(id).auto_shrink([false, false]);
    if let Some(line) = jump {
        scroll = scroll.vertical_scroll_offset(line as f32 * DIFF_ROW_H);
    }
    scroll.show_rows(&mut child, DIFF_ROW_H, n, |ui, range| {
        let mut hunk_no = diff.lines[..range.start].iter().filter(|l| l.kind == DiffKind::Hunk).count();
        for line in &diff.lines[range] {
            let (r, _) = ui.allocate_exact_size(Vec2::new(content_w, DIFF_ROW_H), Sense::hover());
            let p = ui.painter();
            let (bg, fg) = match line.kind {
                DiffKind::Add => (Some(theme::with_alpha(theme::pal().success, 0.12)), theme::pal().text),
                DiffKind::Del => (Some(theme::with_alpha(theme::pal().danger, 0.14)), theme::pal().text),
                DiffKind::Hunk => (Some(theme::with_alpha(theme::pal().info, 0.10)), theme::pal().info),
                DiffKind::Meta => (None, theme::pal().text_2),
                DiffKind::Context => (None, theme::pal().text_2),
            };
            if let Some(bg) = bg {
                p.rect_filled(r, 0.0, bg);
            }
            let num = |n: Option<u32>| n.map(|v| v.to_string()).unwrap_or_default();
            let gx = r.min.x + 6.0;
            p.text(
                Pos2::new(gx + cell * 4.0, r.center().y),
                Align2::RIGHT_CENTER,
                num(line.old),
                font.clone(),
                theme::with_opacity(theme::pal().text_2, 0.7),
            );
            p.text(
                Pos2::new(gx + cell * 9.0, r.center().y),
                Align2::RIGHT_CENTER,
                num(line.new),
                font.clone(),
                theme::with_opacity(theme::pal().text_2, 0.7),
            );
            p.text(Pos2::new(r.min.x + gutter, r.center().y), Align2::LEFT_CENTER, line.text.trim_end_matches('\r'), font.clone(), fg);
            if line.kind == DiffKind::Hunk {
                if let Some(h) = hunks {
                    hunk_buttons(ui, area, r, hunk_no, h, acts);
                }
                hunk_no += 1;
            }
        }
    });
}

/// Botões do cabeçalho de um trecho, fixos na borda direita da área visível.
fn hunk_buttons(ui: &Ui, area: Rect, row: Rect, n: usize, h: HunkCtx, acts: &mut Vec<Act>) {
    let (stage_label, stage_tip) = if h.staged {
        ("Tirar", "Tira só este trecho da preparação")
    } else {
        ("Preparar", "Prepara só este trecho (git apply --cached)")
    };
    let mut right = area.max.x - 6.0;
    let y = row.min.y + 1.0;
    let ht = row.height() - 2.0;
    let mut items: Vec<(&str, &str, bool)> = vec![(stage_label, stage_tip, false)];
    if !h.staged {
        items.push(("Descartar", "Descarta só este trecho da cópia de trabalho", true));
    }
    // Faixa opaca atrás dos botões (com a cor da linha do trecho): o texto do cabeçalho não aparece
    // entre eles.
    let total: f32 = items.iter().map(|(l, ..)| galley(ui.painter(), l, theme::ui(11.0), theme::pal().text).size().x + 16.0).sum();
    let strip = Rect::from_min_max(Pos2::new(right - total - 6.0, row.min.y), Pos2::new(area.max.x, row.max.y));
    ui.painter().rect_filled(strip, 0.0, theme::glass_solid());
    ui.painter().rect_filled(strip, 0.0, theme::with_alpha(theme::pal().info, 0.10));
    for (label, tooltip, danger) in items {
        let g = galley(ui.painter(), label, theme::ui(11.0), theme::pal().text);
        let w = g.size().x + 12.0;
        let r = Rect::from_min_size(Pos2::new(right - w, y), Vec2::new(w, ht));
        right = r.min.x - 4.0;
        let id = Id::new(("git-hunk-btn", n, label));
        let resp = if h.busy { ui.interact(r, id, Sense::hover()) } else { clickable(ui, r, id) };
        let fg = if danger { theme::pal().danger_text } else { theme::pal().text };
        ui.painter().rect_filled(r, 4.0, if resp.hovered() && !h.busy { theme::pal().separator } else { theme::pal().hover });
        text_in(ui.painter(), r, Align2::CENTER_CENTER, label, theme::ui(11.0), if h.busy { theme::with_opacity(fg, 0.4) } else { fg });
        if tip(resp, tooltip).clicked() && !h.busy {
            acts.push(if danger { Act::HunkDiscard(n) } else { Act::HunkStage(n) });
        }
    }
}

fn commit_box(deck: &mut Deck, ui: &mut Ui, footer: Rect, msg_h: f32, staged: usize, busy: bool, acts: &mut Vec<Act>) {
    let painter = ui.painter_at(footer);
    let p = theme::pal();
    separator(&painter, Pos2::new(footer.min.x, footer.min.y + 0.5), Pos2::new(footer.max.x, footer.min.y + 0.5));
    let x0 = footer.min.x + 10.0;
    let x1 = footer.max.x - 10.0;
    let msg_r = Rect::from_min_max(Pos2::new(x0, footer.min.y + 8.0), Pos2::new(x1, footer.min.y + 8.0 + msg_h));
    let id = Id::new(COMMIT_ID);
    let focused = ui.ctx().memory(|m| m.has_focus(id));
    frame(&painter, msg_r, 6.0, p.recess, if focused { theme::accent() } else { p.separator });
    // Ctrl+Enter faz o commit.
    let ctrl_enter = focused && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(msg_r.shrink(1.0)));
    child.set_clip_rect(msg_r.shrink(1.0));
    egui::ScrollArea::vertical().id_salt("git-msg-scroll").auto_shrink([false, false]).show(&mut child, |ui| {
        let te = egui::TextEdit::multiline(&mut deck.git.message)
            .id(id)
            .frame(egui::Frame::NONE.inner_margin(egui::Margin { left: 9, right: 9, top: 6, bottom: 6 }))
            .font(theme::ui(12.0))
            .text_color(p.text)
            .hint_text(egui::RichText::new("Mensagem do commit (Ctrl+Enter)").font(theme::ui(12.0)).color(p.text_3))
            .desired_width(ui.available_width())
            .desired_rows(MSG_ROWS as usize)
            .min_size(Vec2::new(ui.available_width(), msg_r.height() - 2.0));
        te.show(ui);
    });
    let by = msg_r.max.y + 6.0;
    // "Corrigir último commit".
    let box_r = Rect::from_min_size(Pos2::new(x0 + 2.0, by + (BTN_H - 14.0) / 2.0), Vec2::splat(14.0));
    let label_w = galley(&painter, "Corrigir último commit", theme::ui(11.5), p.text_2).size().x;
    let amend_r = Rect::from_min_max(Pos2::new(x0, by), Pos2::new(box_r.max.x + 8.0 + label_w, by + BTN_H));
    let ar = clickable(ui, amend_r, Id::new("git-amend"));
    painter.rect(
        box_r,
        3.0,
        if deck.git.amend { theme::accent() } else { p.recess },
        Stroke::new(1.0, if deck.git.amend { theme::accent() } else { p.separator }),
        egui::StrokeKind::Inside,
    );
    if deck.git.amend {
        text_in(&painter, box_r, Align2::CENTER_CENTER, "✓", theme::ui_bold(11.0), p.on_accent);
    }
    text_in(
        &painter,
        Rect::from_min_max(Pos2::new(box_r.max.x + 8.0, by), Pos2::new(amend_r.max.x, by + BTN_H)),
        Align2::LEFT_CENTER,
        "Corrigir último commit",
        theme::ui(11.5),
        if ar.hovered() { p.text } else { p.text_2 },
    );
    if tip(ar, "Junta as alterações preparadas ao último commit e permite mudar a mensagem (git commit --amend)").clicked() {
        acts.push(Act::ToggleAmend);
    }
    // Botão Commit.
    let label = if staged > 0 { format!("Commit ({staged})") } else { "Commit".into() };
    let bw = button_width(&painter, &label, ButtonKind::Primary);
    let b = Rect::from_min_size(Pos2::new(x1 - bw, by), Vec2::new(bw, BTN_H));
    let ready = !busy && !deck.git.message.trim().is_empty() && (staged > 0 || deck.git.amend);
    let r = if ready { clickable(ui, b, Id::new("git-commit")) } else { ui.interact(b, Id::new("git-commit"), Sense::hover()) };
    if ready {
        let bg = if r.hovered() { theme::brighten(theme::accent(), 1.1) } else { theme::accent() };
        painter.rect_filled(b, 5.0, bg);
    } else {
        frame(&painter, b, 5.0, theme::with_alpha(p.text_strong, 0.02), p.separator);
    }
    text_in(&painter, b, Align2::CENTER_CENTER, &label, theme::ui_semibold(11.5), if ready { p.on_accent } else { p.text_3 });
    let why = if staged == 0 && !deck.git.amend {
        "Prepare (+) as alterações que vão no commit"
    } else if deck.git.message.trim().is_empty() {
        "Escreva a mensagem do commit"
    } else {
        "Fazer o commit do que está preparado"
    };
    if (tip(r, why).clicked() || ctrl_enter) && ready {
        acts.push(Act::Commit);
    }
}

/// Menu de branches (abre sob o botão da branch): filtro, locais, remotas e "criar".
fn branch_menu(deck: &mut Deck, ctx: &egui::Context, anchor: Rect, busy: bool) {
    if !deck.git.branch_menu {
        return;
    }
    let w = anchor.width().clamp(240.0, 340.0);
    let filter = deck.git.branch_filter.trim().to_owned();
    let low = filter.to_lowercase();
    let matches = |b: &&GitBranch| low.is_empty() || b.name.to_lowercase().contains(&low);
    let locals: Vec<GitBranch> = deck.git.branches.iter().filter(|b| !b.remote).filter(matches).cloned().collect();
    let remotes: Vec<GitBranch> = deck.git.branches.iter().filter(|b| b.remote).filter(matches).cloned().collect();
    let mode = deck.git.branch_menu_mode;
    let current = deck.git.status().and_then(|st| st.branch.clone());
    let can_create = mode == BranchMenuMode::Switch
        && !filter.is_empty()
        && valid_branch_name(&filter)
        && !deck.git.branches.iter().any(|b| b.name == filter);
    let row = 26.0;
    let rows = locals.len() + remotes.len() + usize::from(!locals.is_empty()) + usize::from(!remotes.is_empty()) + usize::from(can_create);
    let list_h = (rows.max(1) as f32 * row).min(300.0);
    let h = 8.0 + 26.0 + 6.0 + MODE_H + 6.0 + list_h + 6.0;
    let pos = super::menus::keep_on_screen(ctx, Pos2::new(anchor.min.x, anchor.max.y + 2.0), Vec2::new(w, h));
    let menu_rect = Rect::from_min_size(pos, Vec2::new(w, h));
    let mut chosen: Option<GitBranch> = None;
    let mut create = false;
    let mut new_mode: Option<BranchMenuMode> = None;
    let hint = match mode {
        BranchMenuMode::Switch => "Buscar ou criar branch…",
        BranchMenuMode::Merge => "Buscar a branch a mesclar aqui…",
        BranchMenuMode::Rebase => "Buscar a branch-base do rebase…",
    };
    let p = theme::pal();
    Area::new(Id::new("git-branch-menu")).order(Order::Foreground).fixed_pos(pos).constrain(false).show(ctx, |ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
        shadow(ui.painter(), rect, 8, 8, 24, p.shadow);
        frame(ui.painter(), rect, 8.0, p.popup, p.glass_border);
        // Filtro / nome da branch nova.
        let filter_id = Id::new("git-branch-filter");
        let focused = ui.ctx().memory(|m| m.has_focus(filter_id));
        let fr = Rect::from_min_size(rect.min + Vec2::new(8.0, 8.0), Vec2::new(w - 16.0, 26.0));
        frame(ui.painter(), fr, 6.0, p.recess, if focused { theme::accent() } else { p.separator });
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(fr.shrink(1.0)));
        let out = egui::TextEdit::singleline(&mut deck.git.branch_filter)
            .id(filter_id)
            .frame(egui::Frame::NONE.inner_margin(egui::Margin { left: 8, right: 8, top: 4, bottom: 4 }))
            .font(theme::ui(12.0))
            .text_color(p.text)
            .hint_text(egui::RichText::new(hint).font(theme::ui(12.0)).color(p.text_3))
            .desired_width(fr.width() - 2.0)
            .show(&mut child);
        if !out.response.has_focus() && deck.git.branch_filter.is_empty() {
            out.response.request_focus();
        }
        let enter = out.response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        // Trocar / Mesclar / Rebase (segmented control).
        let modes = [BranchMenuMode::Switch, BranchMenuMode::Merge, BranchMenuMode::Rebase];
        let cur_idx = modes.iter().position(|&m| m == mode).unwrap_or(0);
        let seg_r = Rect::from_min_size(Pos2::new(rect.min.x + 8.0, fr.max.y + 6.0), Vec2::new(w - 16.0, MODE_H));
        if let Some(idx) = segmented(ui, seg_r, Id::new("git-branch-mode"), &["Trocar", "Mesclar", "Rebase"], cur_idx, false) {
            if modes[idx] != mode {
                new_mode = Some(modes[idx]);
            }
        }
        let list = Rect::from_min_size(Pos2::new(rect.min.x + 4.0, fr.max.y + 6.0 + MODE_H + 6.0), Vec2::new(w - 8.0, list_h));
        let mut lc = ui.new_child(egui::UiBuilder::new().max_rect(list));
        lc.set_clip_rect(list);
        egui::ScrollArea::vertical().id_salt("git-branch-list").auto_shrink([false, false]).show(&mut lc, |ui| {
            let item = |ui: &mut Ui, text: &str, color: Color32, enabled: bool, id: (&str, &str)| -> bool {
                let (r, resp) = ui.allocate_exact_size(Vec2::new(list.width(), row), if enabled { Sense::click() } else { Sense::hover() });
                if enabled && resp.hovered() {
                    ui.painter().rect_filled(r, 5.0, p.hover);
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                let g = galley_trunc(ui.painter(), text, theme::ui(12.0), color, r.width() - 20.0);
                place(ui.painter(), r.shrink2(Vec2::new(10.0, 0.0)), Align2::LEFT_CENTER, g, color);
                let _ = id;
                enabled && resp.clicked()
            };
            if can_create && item(ui, &format!("+ Criar branch \"{filter}\""), p.info, !busy, ("create", "")) {
                create = true;
            }
            for (title, list) in [("LOCAIS", &locals), ("REMOTAS", &remotes)] {
                if list.is_empty() {
                    continue;
                }
                let (r, _) = ui.allocate_exact_size(Vec2::new(w - 8.0, super::SECTION_LABEL_H), Sense::hover());
                section_label(ui.painter(), r, title);
                for b in list {
                    let text = if b.current { format!("✓ {}", b.name) } else { format!("   {}", b.name) };
                    let color = if b.current { p.text } else { p.text_2 };
                    // Mesclar/rebase da branch atual nela mesma não faz sentido.
                    let same = current.as_deref() == Some(b.name.as_str());
                    if item(ui, &text, color, !b.current && !same && !busy, ("b", &b.name)) {
                        chosen = Some(b.clone());
                    }
                }
            }
            if rows == 0 {
                let (r, _) = ui.allocate_exact_size(Vec2::new(w - 8.0, row), Sense::hover());
                let text = if filter.is_empty() { "Carregando…" } else { "Nome de branch inválido" };
                text_in(ui.painter(), r.shrink2(Vec2::new(10.0, 0.0)), Align2::LEFT_CENTER, text, theme::ui(12.0), p.text_2);
            }
        });
        if enter && can_create && !busy {
            create = true;
        }
    });
    if let Some(m) = new_mode {
        deck.git_open_branch_menu_mode(m);
    } else if let Some(b) = chosen {
        match mode {
            BranchMenuMode::Switch => deck.git_checkout(b),
            BranchMenuMode::Merge => deck.git_merge_branch(b.name),
            BranchMenuMode::Rebase => deck.git_rebase_onto(b.name),
        }
    } else if create {
        deck.git_create_branch(filter);
    } else if ctx.input(|i| i.key_pressed(Key::Escape)) || super::menus::closes_on_outside(ctx, menu_rect, anchor) {
        deck.git.branch_menu = false;
    }
}

/// Texto da confirmação: (título, detalhe, rótulo do botão perigoso).
fn confirm_texts(action: &ConfirmAction) -> (String, String, &'static str) {
    match action {
        ConfirmAction::DiscardFile(entry) => {
            if entry.kind == EntryKind::Untracked {
                (
                    format!("Apagar {}?", entry.path),
                    "O arquivo não rastreado será removido do disco. Não dá para desfazer.".into(),
                    "Apagar",
                )
            } else if entry.kind == EntryKind::Unmerged {
                (
                    format!("Descartar o conflito em {}?", entry.path),
                    "O arquivo volta ao que está no commit atual (as alterações dos dois lados nele se perdem). Não dá para desfazer."
                        .into(),
                    "Descartar",
                )
            } else {
                (
                    format!("Descartar as alterações em {}?", entry.path),
                    "O arquivo volta ao que está preparado/commitado. Não dá para desfazer.".into(),
                    "Descartar",
                )
            }
        }
        ConfirmAction::DiscardHunk { path, .. } => (
            format!("Descartar este trecho de {path}?"),
            "O trecho volta ao que está preparado/commitado. Não dá para desfazer.".into(),
            "Descartar",
        ),
        ConfirmAction::DropStash(s) => (
            format!("Apagar o stash@{{{}}}?", s.index),
            format!("\"{}\" será removido da lista. Não dá para desfazer.", s.message),
            "Apagar",
        ),
        ConfirmAction::AbortOp(state) => {
            let what = match state {
                RepoState::Merging => "a mesclagem",
                RepoState::Rebasing => "o rebase",
                RepoState::CherryPicking => "o cherry-pick",
                RepoState::Reverting => "o revert",
                RepoState::Normal => "a operação",
            };
            (format!("Abortar {what}?"), "A branch volta ao estado de antes e o que foi resolvido até aqui se perde.".into(), "Abortar")
        }
    }
}

/// Confirmação antes de uma ação que não dá para desfazer.
fn confirm_dialog(deck: &mut Deck, ctx: &egui::Context, panel: Rect) {
    let Some(action) = deck.git.confirm.clone() else { return };
    super::modal_backdrop(deck, ctx, "git-confirm-backdrop");
    let (title, detail, danger) = confirm_texts(&action);
    let w = (panel.width() - 40.0).clamp(220.0, 360.0);
    let pos = Pos2::new(panel.center().x - w / 2.0, panel.min.y + 60.0);
    let mut decided: Option<bool> = None;
    let p = theme::pal();
    Area::new(Id::new("git-confirm")).order(Order::Tooltip).fixed_pos(pos).constrain(false).show(ctx, |ui| {
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
        let dw = button_width(ui.painter(), danger, ButtonKind::Primary);
        let d = Rect::from_min_size(Pos2::new(rect.max.x - 14.0 - dw, by), Vec2::new(dw, BTN_H));
        let cw = button_width(ui.painter(), "Cancelar", ButtonKind::Secondary);
        let c = Rect::from_min_size(Pos2::new(d.min.x - 8.0 - cw, by), Vec2::new(cw, BTN_H));
        let dr = button_danger(ui, d, Id::new("git-confirm-yes"), danger);
        let cr = button(ui, c, Id::new("git-confirm-no"), "Cancelar", ButtonKind::Secondary);
        if dr.clicked() {
            decided = Some(true);
        } else if cr.clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
            decided = Some(false);
        }
    });
    match decided {
        Some(true) => deck.git_confirm_accepted(),
        Some(false) => deck.git.confirm = None,
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_split_into_name_and_folder() {
        assert_eq!(split_path("crates/app/src/x.rs"), ("x.rs", "crates/app/src"));
        assert_eq!(split_path("README.md"), ("README.md", ""));
        assert_eq!(split_path("docs/novo/"), ("novo/", "docs"));
    }
}
