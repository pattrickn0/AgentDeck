//! Aba "Git" da barra direita: branch e sincronização no topo, alterações em seções (ou o diff de
//! um arquivo) no meio e a caixa de commit embaixo. O estado e as operações ficam em `git_ui.rs`.
//!
//! O desenho só *coleta* o que foi clicado ([`Act`]); as ações são aplicadas no fim, com o estado
//! inteiro de volta no `Deck`.

use egui::{Align2, Area, Color32, Id, Key, Modifiers, Order, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use commanddeck_core::git::{valid_branch_name, DiffKind, EntryKind, GitBranch, GitDiff, GitEntry, GitStatus, RepoState};

use super::{clickable, frame, galley, galley_trunc, place, shadow, text_in, tip};
use crate::deck::Deck;
use crate::git_ui::{sections, GitView, OpenDiff, Section};
use crate::icons::Icon;
use crate::theme;

const HEAD_H: f32 = 34.0;
const BANNER_H: f32 = 24.0;
const ROW_H: f32 = 22.0;
const DIFF_ROW_H: f32 = 17.0;
const DIFF_FONT: f32 = 12.0;
const MSG_FONT: f32 = 13.0;
const MSG_ROWS: f32 = 3.0;
const BTN_H: f32 = 28.0;
pub const COMMIT_ID: &str = "git-commit-msg";

/// O que foi clicado neste quadro.
enum Act {
    Refresh,
    Init,
    Copy(String),
    Fetch,
    Pull,
    Push,
    BranchMenu,
    Stage(Vec<String>),
    Unstage(Vec<String>),
    StageAll,
    UnstageAll,
    AskDiscard(GitEntry),
    OpenDiff(GitEntry, bool),
    CloseDiff,
    OpenFile(String),
    Commit,
    ToggleAmend,
    Toggle(Section),
    DismissNotice,
}

fn letter_color(letter: char, section: Section) -> Color32 {
    match (section, letter) {
        (Section::Conflicts, _) => theme::RED_TEXT,
        (_, 'M') | (_, 'T') => theme::OLLAMA,
        (_, 'A') | (Section::Untracked, _) => theme::BADGE_JSON,
        (_, 'D') => theme::RED_TEXT,
        (_, 'R') | (_, 'C') => theme::SEND_DEFAULT,
        _ => theme::MUTED,
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
    ui.painter().rect_filled(Rect::from_min_max(Pos2::new(rect.min.x, rect.max.y - 1.0), rect.max), 0.0, theme::BORDER);
    let rect = Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.max.y - 1.0));
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
        Act::BranchMenu => deck.git_open_branch_menu(),
        Act::Stage(p) => deck.git_stage(p),
        Act::Unstage(p) => deck.git_unstage(p),
        Act::StageAll => deck.git_stage_all(),
        Act::UnstageAll => deck.git_unstage_all(),
        Act::AskDiscard(e) => deck.git.confirm = Some(e),
        Act::OpenDiff(e, staged) => deck.git_open_diff(e, staged),
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
    }
}

/// Estado vazio: título, explicação e botões.
fn message(ui: &mut Ui, rect: Rect, title: &str, detail: Option<&str>, buttons: &[(&str, Act)], acts: &mut Vec<Act>) {
    let painter = ui.painter_at(rect);
    let w = (rect.width() - 40.0).max(80.0);
    let mut y = rect.min.y + 40.0;
    let g = painter.layout(title.to_owned(), theme::ui_semibold(14.0), theme::TEXT, w);
    painter.galley(Pos2::new(rect.min.x + 20.0, y), g.clone(), theme::TEXT);
    y += g.size().y + 8.0;
    if let Some(d) = detail {
        let g = painter.layout(d.to_owned(), theme::ui(12.5), theme::MUTED, w);
        painter.galley(Pos2::new(rect.min.x + 20.0, y), g.clone(), theme::MUTED);
        y += g.size().y + 14.0;
    }
    let mut x = rect.min.x + 20.0;
    for (i, (label, act)) in buttons.iter().enumerate() {
        let bw = galley(&painter, label, theme::ui_semibold(12.0), theme::TEXT).size().x + 24.0;
        let r = Rect::from_min_size(Pos2::new(x, y), Vec2::new(bw, BTN_H));
        let resp = clickable(ui, r, Id::new(("git-msg-btn", i)));
        let primary = i == 0;
        let bg = match (primary, resp.hovered()) {
            (true, true) => theme::brighten(theme::ACCENT, 1.15),
            (true, false) => theme::ACCENT,
            (false, true) => theme::BORDER,
            (false, false) => theme::PANEL_2,
        };
        painter.rect_filled(r, 7.0, bg);
        text_in(&painter, r, Align2::CENTER_CENTER, label, theme::ui_semibold(12.0), if primary { theme::WHITE } else { theme::TEXT });
        if resp.clicked() {
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
        painter.rect_filled(r, 6.0, theme::BORDER);
    }
    let color = if enabled {
        if resp.hovered() {
            theme::TEXT
        } else {
            theme::MUTED
        }
    } else {
        theme::with_opacity(theme::MUTED, 0.45)
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
        galley(painter, label, theme::ui(12.0), theme::TEXT).size().x + 28.0
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
        painter.rect_filled(branch_r, 6.0, theme::PANEL_2);
    }
    deck.icons.paint(&painter, Icon::GitBranch, Pos2::new(branch_r.min.x + 12.0, branch_r.center().y), 14.0, theme::TEXT, 0.0);
    let name = st.branch.clone().unwrap_or_else(|| match &st.head {
        Some(h) => format!("{} (destacado)", h.chars().take(7).collect::<String>()),
        None => "(sem commits)".into(),
    });
    let count = st.entries.len();
    let label = if count > 0 { format!("{name} · {count}  ▾") } else { format!("{name}  ▾") };
    let g = galley_trunc(&painter, &label, theme::ui_semibold(12.5), theme::TEXT, branch_r.width() - 30.0);
    place(
        &painter,
        Rect::from_min_max(Pos2::new(branch_r.min.x + 24.0, branch_r.min.y), branch_r.max),
        Align2::LEFT_CENTER,
        g,
        theme::TEXT,
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
    painter.rect_filled(Rect::from_min_max(Pos2::new(head.min.x, head.max.y - 1.0), head.max), 0.0, theme::BORDER);

    // ---------- Faixas ----------
    let mut y = head.max.y;
    if let Some(label) = deck.git.busy {
        banner(ui, rect, &mut y, label, theme::TEXT, false, acts);
        busy_bar(ui, Rect::from_min_max(Pos2::new(rect.min.x, y - 2.0), Pos2::new(rect.max.x, y)));
    }
    match st.state {
        RepoState::Normal => {}
        RepoState::Merging => {
            banner(ui, rect, &mut y, "Mesclagem em andamento: resolva os conflitos, prepare e faça o commit.", theme::OLLAMA, false, acts)
        }
        RepoState::Rebasing => banner(
            ui,
            rect,
            &mut y,
            "Rebase em andamento: continue ou aborte pelo terminal (git rebase --continue / --abort).",
            theme::OLLAMA,
            false,
            acts,
        ),
        RepoState::CherryPicking => {
            banner(ui, rect, &mut y, "Cherry-pick em andamento: resolva e faça o commit.", theme::OLLAMA, false, acts)
        }
        RepoState::Reverting => banner(ui, rect, &mut y, "Revert em andamento: resolva e faça o commit.", theme::OLLAMA, false, acts),
    }
    if !st.prefix.is_empty() {
        let text = format!("Repositório em {} (a pasta de trabalho é a subpasta {})", st.root.display(), st.prefix.trim_end_matches('/'));
        banner(ui, rect, &mut y, &text, theme::MUTED, false, acts);
    }
    if st.truncated {
        banner(ui, rect, &mut y, "Alterações demais: mostrando as primeiras 5000.", theme::OLLAMA, false, acts);
    }
    if let Some((text, error)) = deck.git.notice.clone() {
        banner(ui, rect, &mut y, &text, if error { theme::RED_TEXT } else { theme::MUTED }, true, acts);
    }

    // ---------- Rodapé: commit ----------
    let msg_h = MSG_ROWS * MSG_FONT * theme::LINE_HEIGHT + 14.0;
    let footer_h = 8.0 + msg_h + 6.0 + BTN_H + 10.0;
    let footer = Rect::from_min_max(Pos2::new(rect.min.x, rect.max.y - footer_h), rect.max);
    let staged = st.entries.iter().filter(|e| e.staged()).count();
    commit_box(deck, ui, footer, msg_h, staged, busy, acts);

    // ---------- Meio: diff ou lista ----------
    let body = Rect::from_min_max(Pos2::new(rect.min.x, y), Pos2::new(rect.max.x, footer.min.y));
    if body.height() < 20.0 {
        return;
    }
    if let Some(open) = deck.git.diff.clone() {
        diff_view(deck, ui, body, &open, busy, acts);
    } else {
        lists(deck, ui, body, st, busy, acts);
    }

    branch_menu(deck, ui.ctx(), branch_r, busy);
}

/// Faixa de uma linha sob o topo (aviso, estado do repositório, erro com ×). Avança `y`.
fn banner(ui: &mut Ui, rect: Rect, y: &mut f32, text: &str, color: Color32, dismiss: bool, acts: &mut Vec<Act>) {
    let r = Rect::from_min_size(Pos2::new(rect.min.x, *y), Vec2::new(rect.width(), BANNER_H));
    *y = r.max.y;
    let p = ui.painter_at(r);
    p.rect_filled(r, 0.0, theme::PANEL);
    let text_r = Rect::from_min_max(Pos2::new(r.min.x + 12.0, r.min.y), Pos2::new(r.max.x - if dismiss { 30.0 } else { 12.0 }, r.max.y));
    let g = galley_trunc(&p, text, theme::ui(11.5), color, text_r.width());
    place(&p, text_r, Align2::LEFT_CENTER, g, color);
    let hover = ui.interact(text_r, Id::new(("git-banner", r.min.y as i32)), Sense::hover());
    tip(hover, text.to_owned());
    if dismiss {
        let x = Rect::from_min_size(Pos2::new(r.max.x - 26.0, r.min.y + 2.0), Vec2::splat(BANNER_H - 4.0));
        let xr = clickable(ui, x, Id::new("git-notice-x"));
        text_in(&p, x, Align2::CENTER_CENTER, "×", theme::ui(14.0), if xr.hovered() { theme::TEXT } else { theme::MUTED });
        if xr.clicked() {
            acts.push(Act::DismissNotice);
        }
    }
}

/// Faixa indeterminada (vai e volta) de operação em andamento.
fn busy_bar(ui: &Ui, r: Rect) {
    let t = ui.ctx().input(|i| i.time);
    let phase = ((t * 0.9) % 2.0) as f32;
    let pos = if phase < 1.0 { phase } else { 2.0 - phase };
    let w = r.width() * 0.3;
    let bar = Rect::from_min_size(Pos2::new(r.min.x + (r.width() - w) * pos, r.min.y), Vec2::new(w, r.height()));
    ui.painter().rect_filled(bar, 1.0, theme::ACCENT);
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
            let g = ui.painter().layout(text.to_owned(), theme::ui(12.5), theme::MUTED, r.width() - 28.0);
            ui.painter().galley(Pos2::new(r.min.x + 14.0, r.min.y + 14.0), g, theme::MUTED);
        }
        for (section, rows) in &groups {
            section_header(deck, ui, *section, rows.len(), busy, acts);
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

fn section_header(deck: &mut Deck, ui: &mut Ui, section: Section, count: usize, busy: bool, acts: &mut Vec<Act>) {
    let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H + 2.0), Sense::click());
    let painter = ui.painter();
    let collapsed = deck.git.collapsed.contains(&section);
    deck.icons.paint(
        painter,
        Icon::TreeChevron,
        Pos2::new(r.min.x + 12.0, r.center().y),
        10.0,
        theme::TREE_ICON,
        if collapsed { 0.0 } else { std::f32::consts::FRAC_PI_2 },
    );
    let title = format!("{}  {count}", section.title());
    text_in(
        painter,
        Rect::from_min_max(Pos2::new(r.min.x + 22.0, r.min.y), r.max),
        Align2::LEFT_CENTER,
        &title,
        theme::ui_bold(11.0),
        theme::MUTED,
    );
    // Ação em lote à direita.
    let (label, tooltip, act) = match section {
        Section::Staged => ("−", "Tirar tudo da preparação", Some(Act::UnstageAll)),
        Section::Changes | Section::Untracked => ("+", "Preparar tudo", Some(Act::StageAll)),
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
            painter.rect_filled(b, 5.0, theme::BORDER);
        }
        text_in(
            painter,
            b,
            Align2::CENTER_CENTER,
            label,
            theme::ui_semibold(15.0),
            if busy { theme::with_opacity(theme::MUTED, 0.4) } else { theme::MUTED },
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
        painter.rect_filled(r.shrink2(Vec2::new(4.0, 0.0)), 4.0, theme::PANEL_2);
    }
    // Ações (aparecem no hover).
    let mut buttons: Vec<(&str, &str, Act)> = Vec::new();
    let path = row.entry.path.clone();
    match section {
        Section::Conflicts => buttons.push(("+", "Marcar como resolvido (preparar)", Act::Stage(vec![path.clone()]))),
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
                painter.rect_filled(b, 5.0, theme::BORDER);
            }
            if label == "↶" {
                deck.icons.paint(&painter, Icon::Undo, b.center(), 13.0, theme::TEXT, 0.0);
            } else {
                text_in(&painter, b, Align2::CENTER_CENTER, label, theme::ui_semibold(15.0), theme::TEXT);
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
    let ng = galley_trunc(&painter, &name_shown, theme::ui(12.5), theme::TEXT, avail);
    let nw = ng.size().x;
    place(&painter, Rect::from_min_max(Pos2::new(text_x, r.min.y), Pos2::new(text_x + nw, r.max.y)), Align2::LEFT_CENTER, ng, theme::TEXT);
    if !dir.is_empty() && avail - nw > 30.0 {
        let dg = galley_trunc(&painter, dir, theme::ui(11.5), theme::MUTED, avail - nw - 8.0);
        place(
            &painter,
            Rect::from_min_max(Pos2::new(text_x + nw + 8.0, r.min.y), Pos2::new(bx, r.max.y)),
            Align2::LEFT_CENTER,
            dg,
            theme::MUTED,
        );
    }
    let resp = tip(resp, row.entry.path.clone());
    if !clicked_button {
        if resp.double_clicked() {
            acts.push(Act::OpenFile(row.entry.path.clone()));
        } else if resp.clicked() {
            acts.push(Act::OpenDiff(row.entry.clone(), row.staged));
        }
    }
}

fn diff_view(deck: &mut Deck, ui: &mut Ui, body: Rect, open: &OpenDiff, busy: bool, acts: &mut Vec<Act>) {
    let painter = ui.painter_at(body);
    // Cabeçalho: voltar, caminho, ações.
    let head = Rect::from_min_size(body.min, Vec2::new(body.width(), 28.0));
    painter.rect_filled(head, 0.0, theme::PANEL);
    let back = Rect::from_min_size(Pos2::new(head.min.x + 6.0, head.min.y + 3.0), Vec2::new(26.0, 22.0));
    let br = clickable(ui, back, Id::new("git-diff-back"));
    if br.hovered() {
        painter.rect_filled(back, 5.0, theme::BORDER);
    }
    text_in(&painter, back, Align2::CENTER_CENTER, "←", theme::ui(15.0), theme::TEXT);
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
            painter.rect_filled(b, 5.0, theme::BORDER);
        }
        let color = if busy { theme::with_opacity(theme::MUTED, 0.4) } else { theme::TEXT };
        if label == "↶" {
            deck.icons.paint(&painter, Icon::Undo, b.center(), 13.0, color, 0.0);
        } else {
            text_in(&painter, b, Align2::CENTER_CENTER, label, theme::ui_semibold(15.0), color);
        }
        if tip(r, tooltip).clicked() && !busy {
            acts.push(act);
        }
    }
    let side = if open.staged { "preparado" } else { "não preparado" };
    let title = format!("{}  · {side}", entry.path);
    let g = galley_trunc(&painter, &title, theme::ui(12.0), theme::TEXT, right - back.max.x - 12.0);
    place(
        &painter,
        Rect::from_min_max(Pos2::new(back.max.x + 8.0, head.min.y), Pos2::new(right, head.max.y)),
        Align2::LEFT_CENTER,
        g,
        theme::TEXT,
    );
    let hover = ui.interact(
        Rect::from_min_max(Pos2::new(back.max.x + 8.0, head.min.y), Pos2::new(right, head.max.y)),
        Id::new("git-diff-title"),
        Sense::click(),
    );
    if tip(hover, format!("{}\n(duplo clique abre o arquivo)", entry.path)).double_clicked() {
        acts.push(Act::OpenFile(entry.path.clone()));
    }

    let area = Rect::from_min_max(Pos2::new(body.min.x, head.max.y), body.max);
    match &open.diff {
        None => {
            text_in(&ui.painter_at(area), area.shrink(14.0), Align2::LEFT_TOP, "Carregando o diff…", theme::ui(12.5), theme::MUTED);
        }
        Some(Err(e)) => {
            let g = ui.painter().layout(e.clone(), theme::ui(12.5), theme::RED_TEXT, area.width() - 28.0);
            ui.painter_at(area).galley(area.min + Vec2::splat(14.0), g, theme::RED_TEXT);
        }
        Some(Ok(diff)) => diff_lines(ui, area, diff),
    }
}

fn diff_lines(ui: &mut Ui, area: Rect, diff: &GitDiff) {
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
        text_in(child.painter(), r.shrink2(Vec2::new(12.0, 0.0)), Align2::LEFT_CENTER, note, theme::ui(12.0), theme::OLLAMA);
    }
    let n = diff.lines.len();
    egui::ScrollArea::both().id_salt("git-diff").auto_shrink([false, false]).show_rows(&mut child, DIFF_ROW_H, n, |ui, range| {
        for line in &diff.lines[range] {
            let (r, _) = ui.allocate_exact_size(Vec2::new(content_w, DIFF_ROW_H), Sense::hover());
            let p = ui.painter();
            let (bg, fg) = match line.kind {
                DiffKind::Add => (Some(theme::rgba(74, 222, 128, 0.12)), theme::TEXT),
                DiffKind::Del => (Some(theme::rgba(229, 72, 77, 0.14)), theme::TEXT),
                DiffKind::Hunk => (Some(theme::rgba(79, 140, 247, 0.10)), theme::SEND_DEFAULT),
                DiffKind::Meta => (None, theme::MUTED),
                DiffKind::Context => (None, theme::TREE_TEXT),
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
                theme::with_opacity(theme::MUTED, 0.7),
            );
            p.text(
                Pos2::new(gx + cell * 9.0, r.center().y),
                Align2::RIGHT_CENTER,
                num(line.new),
                font.clone(),
                theme::with_opacity(theme::MUTED, 0.7),
            );
            p.text(Pos2::new(r.min.x + gutter, r.center().y), Align2::LEFT_CENTER, &line.text, font.clone(), fg);
        }
    });
}

fn commit_box(deck: &mut Deck, ui: &mut Ui, footer: Rect, msg_h: f32, staged: usize, busy: bool, acts: &mut Vec<Act>) {
    let painter = ui.painter_at(footer);
    painter.rect_filled(footer, 0.0, theme::BG);
    painter.rect_filled(Rect::from_min_size(footer.min, Vec2::new(footer.width(), 1.0)), 0.0, theme::BORDER);
    let x0 = footer.min.x + 10.0;
    let x1 = footer.max.x - 10.0;
    let msg_r = Rect::from_min_max(Pos2::new(x0, footer.min.y + 8.0), Pos2::new(x1, footer.min.y + 8.0 + msg_h));
    let id = Id::new(COMMIT_ID);
    let focused = ui.ctx().memory(|m| m.has_focus(id));
    frame(&painter, msg_r, 8.0, theme::PANEL, if focused { theme::MUTED } else { theme::BORDER });
    // Ctrl+Enter faz o commit.
    let ctrl_enter = focused && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(msg_r.shrink(1.0)));
    child.set_clip_rect(msg_r.shrink(1.0));
    egui::ScrollArea::vertical().id_salt("git-msg-scroll").auto_shrink([false, false]).show(&mut child, |ui| {
        let te = egui::TextEdit::multiline(&mut deck.git.message)
            .id(id)
            .frame(egui::Frame::NONE)
            .margin(egui::Margin { left: 9, right: 9, top: 6, bottom: 6 })
            .font(theme::ui(MSG_FONT))
            .text_color(theme::INPUT_TEXT)
            .hint_text(egui::RichText::new("Mensagem do commit (Ctrl+Enter)").font(theme::ui(MSG_FONT)).color(theme::PLACEHOLDER))
            .desired_width(ui.available_width())
            .desired_rows(MSG_ROWS as usize)
            .min_size(Vec2::new(ui.available_width(), msg_r.height() - 2.0));
        te.show(ui);
    });
    let by = msg_r.max.y + 6.0;
    // "Corrigir último commit".
    let box_r = Rect::from_min_size(Pos2::new(x0 + 2.0, by + (BTN_H - 14.0) / 2.0), Vec2::splat(14.0));
    let label_w = galley(&painter, "Corrigir último commit", theme::ui(12.0), theme::MUTED).size().x;
    let amend_r = Rect::from_min_max(Pos2::new(x0, by), Pos2::new(box_r.max.x + 8.0 + label_w, by + BTN_H));
    let ar = clickable(ui, amend_r, Id::new("git-amend"));
    painter.rect(
        box_r,
        3.0,
        if deck.git.amend { theme::ACCENT } else { theme::PANEL_2 },
        Stroke::new(1.0, theme::MUTED),
        egui::StrokeKind::Inside,
    );
    if deck.git.amend {
        text_in(&painter, box_r, Align2::CENTER_CENTER, "✓", theme::ui_bold(11.0), theme::WHITE);
    }
    text_in(
        &painter,
        Rect::from_min_max(Pos2::new(box_r.max.x + 8.0, by), Pos2::new(amend_r.max.x, by + BTN_H)),
        Align2::LEFT_CENTER,
        "Corrigir último commit",
        theme::ui(12.0),
        if ar.hovered() { theme::TEXT } else { theme::MUTED },
    );
    if tip(ar, "Junta as alterações preparadas ao último commit e permite mudar a mensagem (git commit --amend)").clicked() {
        acts.push(Act::ToggleAmend);
    }
    // Botão Commit.
    let label = if staged > 0 { format!("Commit ({staged})") } else { "Commit".into() };
    let bw = galley(&painter, &label, theme::ui_semibold(12.0), theme::WHITE).size().x + 26.0;
    let b = Rect::from_min_size(Pos2::new(x1 - bw, by), Vec2::new(bw, BTN_H));
    let ready = !busy && !deck.git.message.trim().is_empty() && (staged > 0 || deck.git.amend);
    let r = if ready { clickable(ui, b, Id::new("git-commit")) } else { ui.interact(b, Id::new("git-commit"), Sense::hover()) };
    let mut bg = if ready { deck.send_color() } else { theme::PANEL_2 };
    if ready && r.hovered() {
        bg = theme::brighten(bg, 1.15);
    }
    painter.rect_filled(b, 7.0, bg);
    text_in(&painter, b, Align2::CENTER_CENTER, &label, theme::ui_semibold(12.0), if ready { theme::WHITE } else { theme::SEND_IDLE_TEXT });
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
    let can_create = !filter.is_empty() && valid_branch_name(&filter) && !deck.git.branches.iter().any(|b| b.name == filter);
    let row = 24.0;
    let rows = locals.len() + remotes.len() + usize::from(!locals.is_empty()) + usize::from(!remotes.is_empty()) + usize::from(can_create);
    let list_h = (rows.max(1) as f32 * row).min(300.0);
    let h = 8.0 + 26.0 + 6.0 + list_h + 6.0;
    let pos = super::menus::keep_on_screen(ctx, Pos2::new(anchor.min.x, anchor.max.y + 2.0), Vec2::new(w, h));
    let menu_rect = Rect::from_min_size(pos, Vec2::new(w, h));
    let mut chosen: Option<GitBranch> = None;
    let mut create = false;
    Area::new(Id::new("git-branch-menu")).order(Order::Foreground).fixed_pos(pos).constrain(false).show(ctx, |ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
        shadow(ui.painter(), rect, 8, 8, 24, theme::rgba(0, 0, 0, 0.5));
        frame(ui.painter(), rect, 8.0, theme::PANEL_2, theme::BORDER);
        // Filtro / nome da branch nova.
        let fr = Rect::from_min_size(rect.min + Vec2::new(8.0, 8.0), Vec2::new(w - 16.0, 26.0));
        frame(ui.painter(), fr, 6.0, theme::PANEL, theme::BORDER);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(fr.shrink(1.0)));
        let out = egui::TextEdit::singleline(&mut deck.git.branch_filter)
            .id(Id::new("git-branch-filter"))
            .frame(egui::Frame::NONE)
            .margin(egui::Margin { left: 8, right: 8, top: 4, bottom: 4 })
            .font(theme::ui(12.5))
            .hint_text(egui::RichText::new("Buscar ou criar branch…").font(theme::ui(12.5)).color(theme::PLACEHOLDER))
            .desired_width(fr.width() - 2.0)
            .show(&mut child);
        if !out.response.has_focus() && deck.git.branch_filter.is_empty() {
            out.response.request_focus();
        }
        let enter = out.response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        let list = Rect::from_min_size(Pos2::new(rect.min.x + 4.0, fr.max.y + 6.0), Vec2::new(w - 8.0, list_h));
        let mut lc = ui.new_child(egui::UiBuilder::new().max_rect(list));
        lc.set_clip_rect(list);
        egui::ScrollArea::vertical().id_salt("git-branch-list").auto_shrink([false, false]).show(&mut lc, |ui| {
            let item = |ui: &mut Ui, text: &str, color: Color32, enabled: bool, id: (&str, &str)| -> bool {
                let (r, resp) = ui.allocate_exact_size(Vec2::new(list.width(), row), if enabled { Sense::click() } else { Sense::hover() });
                if enabled && resp.hovered() {
                    ui.painter().rect_filled(r, 5.0, theme::BORDER);
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                let g = galley_trunc(ui.painter(), text, theme::ui(12.5), color, r.width() - 20.0);
                place(ui.painter(), r.shrink2(Vec2::new(10.0, 0.0)), Align2::LEFT_CENTER, g, color);
                let _ = id;
                enabled && resp.clicked()
            };
            if can_create && item(ui, &format!("+ Criar branch \"{filter}\""), theme::SEND_DEFAULT, !busy, ("create", "")) {
                create = true;
            }
            for (title, list) in [("LOCAIS", &locals), ("REMOTAS", &remotes)] {
                if list.is_empty() {
                    continue;
                }
                let (r, _) = ui.allocate_exact_size(Vec2::new(w - 8.0, row), Sense::hover());
                text_in(ui.painter(), r.shrink2(Vec2::new(10.0, 0.0)), Align2::LEFT_CENTER, title, theme::ui_bold(10.5), theme::MUTED);
                for b in list {
                    let text = if b.current { format!("✓ {}", b.name) } else { format!("   {}", b.name) };
                    let color = if b.current { theme::TEXT } else { theme::SELECT_TEXT };
                    if item(ui, &text, color, !b.current && !busy, ("b", &b.name)) {
                        chosen = Some(b.clone());
                    }
                }
            }
            if rows == 0 {
                let (r, _) = ui.allocate_exact_size(Vec2::new(w - 8.0, row), Sense::hover());
                let text = if filter.is_empty() { "Carregando…" } else { "Nome de branch inválido" };
                text_in(ui.painter(), r.shrink2(Vec2::new(10.0, 0.0)), Align2::LEFT_CENTER, text, theme::ui(12.0), theme::MUTED);
            }
        });
        if enter && can_create && !busy {
            create = true;
        }
    });
    if let Some(b) = chosen {
        deck.git_checkout(b);
    } else if create {
        deck.git_create_branch(filter);
    } else if ctx.input(|i| i.key_pressed(Key::Escape)) || super::menus::closes_on_outside(ctx, menu_rect, anchor) {
        deck.git.branch_menu = false;
    }
}

/// Confirmação antes de descartar (não dá para desfazer).
fn confirm_dialog(deck: &mut Deck, ctx: &egui::Context, panel: Rect) {
    let Some(entry) = deck.git.confirm.clone() else { return };
    let untracked = entry.kind == EntryKind::Untracked;
    let title = if untracked { format!("Apagar {}?", entry.path) } else { format!("Descartar as alterações em {}?", entry.path) };
    let detail =
        if untracked { "O arquivo não rastreado será removido do disco." } else { "O arquivo volta ao que está preparado/commitado." };
    let w = (panel.width() - 40.0).clamp(220.0, 360.0);
    let pos = Pos2::new(panel.center().x - w / 2.0, panel.min.y + 60.0);
    let mut decided: Option<bool> = None;
    Area::new(Id::new("git-confirm")).order(Order::Foreground).fixed_pos(pos).constrain(false).show(ctx, |ui| {
        let g1 = ui.painter().layout(title.clone(), theme::ui_semibold(13.0), theme::TEXT, w - 28.0);
        let g2 = ui.painter().layout(format!("{detail} Não dá para desfazer."), theme::ui(12.0), theme::MUTED, w - 28.0);
        let h = 14.0 + g1.size().y + 6.0 + g2.size().y + 14.0 + BTN_H + 14.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
        shadow(ui.painter(), rect, 10, 10, 30, theme::rgba(0, 0, 0, 0.6));
        frame(ui.painter(), rect, 10.0, theme::PANEL_2, theme::BORDER);
        let mut y = rect.min.y + 14.0;
        ui.painter().galley(Pos2::new(rect.min.x + 14.0, y), g1.clone(), theme::TEXT);
        y += g1.size().y + 6.0;
        ui.painter().galley(Pos2::new(rect.min.x + 14.0, y), g2.clone(), theme::MUTED);
        let by = rect.max.y - 14.0 - BTN_H;
        let danger = if untracked { "Apagar" } else { "Descartar" };
        let dw = galley(ui.painter(), danger, theme::ui_semibold(12.0), theme::WHITE).size().x + 26.0;
        let d = Rect::from_min_size(Pos2::new(rect.max.x - 14.0 - dw, by), Vec2::new(dw, BTN_H));
        let cw = galley(ui.painter(), "Cancelar", theme::ui_semibold(12.0), theme::TEXT).size().x + 26.0;
        let c = Rect::from_min_size(Pos2::new(d.min.x - 8.0 - cw, by), Vec2::new(cw, BTN_H));
        let dr = clickable(ui, d, Id::new("git-confirm-yes"));
        let cr = clickable(ui, c, Id::new("git-confirm-no"));
        ui.painter().rect_filled(d, 7.0, if dr.hovered() { theme::brighten(theme::RED, 1.1) } else { theme::RED });
        text_in(ui.painter(), d, Align2::CENTER_CENTER, danger, theme::ui_semibold(12.0), theme::WHITE);
        ui.painter().rect_filled(c, 7.0, if cr.hovered() { theme::BORDER } else { theme::PANEL });
        text_in(ui.painter(), c, Align2::CENTER_CENTER, "Cancelar", theme::ui_semibold(12.0), theme::TEXT);
        if dr.clicked() {
            decided = Some(true);
        } else if cr.clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
            decided = Some(false);
        }
    });
    match decided {
        Some(true) => deck.git_discard_confirmed(),
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
