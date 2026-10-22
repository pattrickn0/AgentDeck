//! Visões da aba Git além da lista de alterações: Histórico, Stash e Conflitos. Como em `git.rs`,
//! o desenho só *coleta* o que foi clicado ([`Act`]); quem aplica é `git::apply`.

use egui::{Align2, Id, Key, Pos2, Rect, Sense, Ui, Vec2};

use agentdeck_core::git::{ConflictBlock, ConflictChoice, EntryKind, GitStash, GitStatus, RepoState};

use super::git::{diff_lines, Act, BTN_H};
use super::{clickable, frame, galley, galley_spaced, galley_trunc, place, text_in, tip};
use crate::deck::Deck;
use crate::git_ui::{GitSubView, OpenCommit, OpenConflict, OpenStashDiff};
use crate::theme;

const HEADER_H: f32 = 28.0;
const COMMIT_ROW_H: f32 = 42.0;
const STASH_ROW_H: f32 = 46.0;
const FILE_ROW_H: f32 = 18.0;
const CODE_ROW_H: f32 = 16.0;
/// Linhas de cada lado mostradas num bloco de conflito (o resto vira "+N linhas").
const BLOCK_LINES: usize = 10;

/// Aparência de um botão de texto.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Look {
    Normal,
    Primary,
    Danger,
}

/// Botão de texto (pílula) encostado em `right`; devolve o clique e recua `right`.
#[allow(clippy::too_many_arguments)]
pub(super) fn pill(ui: &Ui, right: &mut f32, y: f32, h: f32, label: &str, tooltip: &str, look: Look, enabled: bool, id: Id) -> bool {
    let painter = ui.painter();
    let w = galley(painter, label, theme::ui_semibold(11.5), theme::pal().text).size().x + 18.0;
    let r = Rect::from_min_size(Pos2::new(*right - w, y), Vec2::new(w, h));
    *right = r.min.x - 6.0;
    let resp = if enabled { clickable(ui, r, id) } else { ui.interact(r, id, Sense::hover()) };
    let hot = enabled && resp.hovered();
    let p = theme::pal();
    let (bg, border, fg) = match look {
        Look::Primary => {
            if enabled {
                let fill = if hot { theme::brighten(theme::accent(), 1.08) } else { theme::accent() };
                (fill, fill, p.on_accent)
            } else {
                (p.hover, p.separator, theme::with_opacity(p.text, 0.4))
            }
        }
        Look::Danger => {
            let fill = if hot { theme::with_alpha(p.danger, 0.18) } else { theme::with_alpha(p.danger, 0.10) };
            let border = theme::with_alpha(p.danger, 0.35);
            let fg = if enabled { p.danger_text } else { theme::with_opacity(p.danger_text, 0.4) };
            (fill, border, fg)
        }
        Look::Normal => {
            let fill = if hot { theme::with_alpha(p.text_strong, 0.08) } else { theme::with_alpha(p.text_strong, 0.04) };
            let border = theme::with_alpha(p.text_strong, 0.14);
            let fg = if enabled { p.text } else { theme::with_opacity(p.text, 0.4) };
            (fill, border, fg)
        }
    };
    frame(painter, r, 5.0, bg, border);
    text_in(painter, r, Align2::CENTER_CENTER, label, theme::ui_semibold(11.5), fg);
    tip(resp, tooltip.to_owned()).clicked() && enabled
}

/// Largura de uma pílula (para alinhar à esquerda ou calcular o espaço).
fn pill_w(ui: &Ui, label: &str) -> f32 {
    galley(ui.painter(), label, theme::ui_semibold(11.5), theme::pal().text).size().x + 18.0
}

/// Cabeçalho com "←", o título e o espaço livre à direita (`right`) para botões. Devolve (cabeçalho, voltar clicado, `right`).
fn header(ui: &Ui, body: Rect, id: &str, title: &str, hint: &str, reserved_right: f32) -> (Rect, bool, f32) {
    let painter = ui.painter_at(body);
    let head = Rect::from_min_size(body.min, Vec2::new(body.width(), HEADER_H));
    painter.rect_filled(head, 0.0, theme::pal().chip);
    let back = Rect::from_min_size(Pos2::new(head.min.x + 6.0, head.min.y + 3.0), Vec2::new(26.0, 22.0));
    let br = clickable(ui, back, Id::new(("git-back", id)));
    if br.hovered() {
        painter.rect_filled(back, 5.0, theme::pal().separator);
    }
    text_in(&painter, back, Align2::CENTER_CENTER, "←", theme::ui(15.0), theme::pal().text);
    let back_clicked = tip(br, "Voltar").clicked();
    let right = head.max.x - 6.0 - reserved_right;
    let tr = Rect::from_min_max(Pos2::new(back.max.x + 8.0, head.min.y), Pos2::new(right, head.max.y));
    let g = galley_trunc(&painter, title, theme::ui(12.0), theme::pal().text, tr.width());
    place(&painter, tr, Align2::LEFT_CENTER, g, theme::pal().text);
    tip(ui.interact(tr, Id::new(("git-head-title", id)), Sense::hover()), hint.to_owned());
    (head, back_clicked, head.max.x - 6.0)
}

fn note(ui: &Ui, area: Rect, text: &str, color: egui::Color32) {
    let p = ui.painter_at(area);
    let g = p.layout(text.to_owned(), theme::ui(12.5), color, area.width() - 28.0);
    p.galley(area.min + Vec2::new(14.0, 14.0), g, color);
}

fn scroll_child(ui: &mut Ui, area: Rect) -> Ui {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::top_down(egui::Align::Min)));
    child.set_clip_rect(area);
    child
}

// ---------- Histórico ----------

pub(super) fn history(deck: &mut Deck, ui: &mut Ui, body: Rect, acts: &mut Vec<Act>) {
    if let Some(open) = deck.git.commit_detail.take() {
        commit_detail(ui, body, &open, acts);
        deck.git.commit_detail = Some(open);
        return;
    }
    if deck.git.log.is_empty() {
        let text = if deck.git.log_loading { "Lendo o histórico…" } else { "Nenhum commit ainda nesta branch." };
        note(ui, body, text, theme::pal().text_2);
        return;
    }
    let log = &deck.git.log;
    let (has_more, loading) = (deck.git.log_has_more, deck.git.log_loading);
    let n = log.len() + usize::from(has_more);
    let mut child = scroll_child(ui, body);
    egui::ScrollArea::vertical().id_salt("git-log").auto_shrink([false, false]).show_rows(&mut child, COMMIT_ROW_H, n, |ui, range| {
        for i in range {
            let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), COMMIT_ROW_H), Sense::click());
            let p = ui.painter();
            let Some(c) = log.get(i) else {
                // "Carregar mais" no fim da lista.
                let label = if loading { "Carregando…" } else { "Carregar mais commits" };
                let hot = resp.hovered() && !loading;
                if hot {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                text_in(
                    p,
                    r,
                    Align2::CENTER_CENTER,
                    label,
                    theme::ui_semibold(12.0),
                    if hot { theme::pal().text } else { theme::pal().text_2 },
                );
                if resp.clicked() && !loading {
                    acts.push(Act::MoreLog);
                }
                continue;
            };
            if resp.hovered() {
                p.rect_filled(r.shrink2(Vec2::new(4.0, 1.0)), 4.0, theme::pal().hover);
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let w = r.width() - 28.0;
            let g = galley_trunc(p, &c.subject, theme::ui(12.5), theme::pal().text, w);
            place(
                p,
                Rect::from_min_size(Pos2::new(r.min.x + 14.0, r.min.y + 5.0), Vec2::new(w, 17.0)),
                Align2::LEFT_CENTER,
                g,
                theme::pal().text,
            );
            // Segunda linha: hash (mono) · autor · data.
            let hg = galley(p, &c.short_hash, theme::mono(11.0), theme::pal().info);
            let hw = hg.size().x;
            let y2 = r.min.y + 24.0;
            place(p, Rect::from_min_size(Pos2::new(r.min.x + 14.0, y2), Vec2::new(hw, 14.0)), Align2::LEFT_CENTER, hg, theme::pal().info);
            let meta = format!("{} · {}", c.author, c.date);
            let mg = galley_trunc(p, &meta, theme::ui(11.0), theme::pal().text_2, (w - hw - 8.0).max(20.0));
            place(
                p,
                Rect::from_min_size(Pos2::new(r.min.x + 14.0 + hw + 8.0, y2), Vec2::new(w - hw - 8.0, 14.0)),
                Align2::LEFT_CENTER,
                mg,
                theme::pal().text_2,
            );
            if tip(resp, format!("{}\n{} · {} · {}", c.subject, c.short_hash, c.author, c.date)).clicked() {
                acts.push(Act::OpenCommit(c.clone()));
            }
        }
    });
}

fn commit_detail(ui: &mut Ui, body: Rect, open: &OpenCommit, acts: &mut Vec<Act>) {
    let title = format!("{}  {}", open.short_hash, open.subject);
    let center_w = pill_w(ui, "Abrir ao centro") + 6.0;
    let (head, back, mut right) = header(ui, body, "commit", &title, &open.subject, center_w);
    if back {
        acts.push(Act::CloseCommit);
    }
    let tip_text = "Abre o diff deste commit, só para ler, com a largura toda do centro";
    if pill(ui, &mut right, head.min.y + 3.0, 22.0, "Abrir ao centro", tip_text, Look::Normal, true, Id::new("git-commit-center")) {
        acts.push(Act::OpenCommitCenter(open.hash.clone(), open.short_hash.clone(), open.subject.clone()));
    }
    let area = Rect::from_min_max(Pos2::new(body.min.x, head.max.y), body.max);
    let d = match &open.detail {
        None => return note(ui, area, "Carregando o commit…", theme::pal().text_2),
        Some(Err(e)) => return note(ui, area, e, theme::pal().danger_text),
        Some(Ok(d)) => d,
    };
    let p = ui.painter_at(area);
    let mut y = area.min.y + 8.0;
    let w = area.width() - 28.0;
    let meta = format!("{} <{}> · {}", d.author, d.email, d.date);
    let g = galley_trunc(&p, &meta, theme::ui(11.5), theme::pal().text_2, w);
    place(&p, Rect::from_min_size(Pos2::new(area.min.x + 14.0, y), Vec2::new(w, 16.0)), Align2::LEFT_CENTER, g, theme::pal().text_2);
    y += 20.0;
    // A mensagem inteira, no máximo 5 linhas de altura.
    let mut job = egui::text::LayoutJob::simple(d.message.clone(), theme::ui(12.5), theme::pal().text, w);
    job.wrap.max_rows = 5;
    job.wrap.overflow_character = Some('…');
    let mg = p.layout_job(job);
    let mh = mg.size().y;
    p.galley(Pos2::new(area.min.x + 14.0, y), mg, theme::pal().text);
    y += mh + 8.0;
    // Arquivos alterados (até 5 linhas).
    let title = format!("{} ARQUIVO(S) ALTERADO(S)", d.files.len());
    let g = galley_spaced(&p, &title, theme::label(10.0), theme::pal().text_2, 0.8);
    place(&p, Rect::from_min_size(Pos2::new(area.min.x + 14.0, y), Vec2::new(w, 16.0)), Align2::LEFT_CENTER, g, theme::pal().text_2);
    y += 18.0;
    for (letter, path) in d.files.iter().take(5) {
        let color = match letter {
            'A' => theme::pal().success,
            'D' => theme::pal().danger_text,
            'M' => theme::pal().warning,
            _ => theme::pal().info,
        };
        text_in(
            &p,
            Rect::from_min_size(Pos2::new(area.min.x + 14.0, y), Vec2::new(12.0, FILE_ROW_H)),
            Align2::CENTER_CENTER,
            &letter.to_string(),
            theme::mono_bold(11.5),
            color,
        );
        let g = galley_trunc(&p, path, theme::ui(12.0), theme::pal().text, w - 20.0);
        place(
            &p,
            Rect::from_min_size(Pos2::new(area.min.x + 34.0, y), Vec2::new(w - 20.0, FILE_ROW_H)),
            Align2::LEFT_CENTER,
            g,
            theme::pal().text,
        );
        y += FILE_ROW_H;
    }
    if d.files.len() > 5 {
        text_in(
            &p,
            Rect::from_min_size(Pos2::new(area.min.x + 34.0, y), Vec2::new(w, FILE_ROW_H)),
            Align2::LEFT_CENTER,
            &format!("+{} outros (veja no diff abaixo)", d.files.len() - 5),
            theme::ui(11.5),
            theme::pal().text_2,
        );
        y += FILE_ROW_H;
    }
    y += 6.0;
    p.rect_filled(Rect::from_min_size(Pos2::new(area.min.x, y), Vec2::new(area.width(), 1.0)), 0.0, theme::pal().separator);
    let diff_area = Rect::from_min_max(Pos2::new(area.min.x, y + 1.0), area.max);
    if diff_area.height() > 40.0 {
        diff_lines(ui, diff_area, &d.diff, None, acts);
    }
}

// ---------- Stash ----------

pub(super) fn stash(deck: &mut Deck, ui: &mut Ui, body: Rect, st: &GitStatus, busy: bool, acts: &mut Vec<Act>) {
    if let Some(open) = deck.git.stash_diff.take() {
        stash_diff(ui, body, &open, busy, acts);
        deck.git.stash_diff = Some(open);
        return;
    }
    let p = ui.painter_at(body);
    // Formulário: mensagem, "incluir não rastreados" e Guardar.
    let x0 = body.min.x + 10.0;
    let x1 = body.max.x - 10.0;
    let input = Rect::from_min_size(Pos2::new(x0, body.min.y + 8.0), Vec2::new(x1 - x0, 28.0));
    let id = Id::new("git-stash-msg");
    let focused = ui.ctx().memory(|m| m.has_focus(id));
    frame(&p, input, 6.0, theme::pal().recess, if focused { theme::accent() } else { theme::pal().separator });
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(input.shrink(1.0)));
    let out = egui::TextEdit::singleline(&mut deck.git.stash_message)
        .id(id)
        .frame(egui::Frame::NONE)
        .margin(egui::Margin { left: 9, right: 9, top: 5, bottom: 5 })
        .font(theme::ui(12.5))
        .text_color(theme::pal().text)
        .hint_text(egui::RichText::new("Mensagem do stash (opcional)").font(theme::ui(12.5)).color(theme::pal().text_3))
        .desired_width(input.width() - 2.0)
        .show(&mut child);
    let has_changes = !st.entries.is_empty();
    let enter = out.response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
    let by = input.max.y + 8.0;
    let mut right = x1;
    let can = has_changes && !busy;
    let why =
        if has_changes { "Guarda as alterações e deixa a pasta limpa (git stash push)" } else { "Não há alterações para guardar" };
    if pill(ui, &mut right, by, BTN_H, "Guardar", why, Look::Primary, can, Id::new("git-stash-push")) || (enter && can) {
        acts.push(Act::StashPush);
    }
    let box_r = Rect::from_min_size(Pos2::new(x0 + 2.0, by + (BTN_H - 14.0) / 2.0), Vec2::splat(14.0));
    let label = "Incluir não rastreados";
    let lw = galley(&p, label, theme::ui(12.0), theme::pal().text_2).size().x;
    let hit = Rect::from_min_max(Pos2::new(x0, by), Pos2::new(box_r.max.x + 8.0 + lw, by + BTN_H));
    let ar = clickable(ui, hit, Id::new("git-stash-untracked"));
    let on = deck.git.stash_include_untracked;
    p.rect(
        box_r,
        3.0,
        if on { theme::accent() } else { theme::pal().recess },
        egui::Stroke::new(1.0, if on { theme::accent() } else { theme::pal().separator }),
        egui::StrokeKind::Inside,
    );
    if on {
        text_in(&p, box_r, Align2::CENTER_CENTER, "✓", theme::ui_bold(11.0), theme::pal().on_accent);
    }
    text_in(
        &p,
        Rect::from_min_max(Pos2::new(box_r.max.x + 8.0, by), Pos2::new(hit.max.x, by + BTN_H)),
        Align2::LEFT_CENTER,
        label,
        theme::ui(12.0),
        if ar.hovered() { theme::pal().text } else { theme::pal().text_2 },
    );
    if tip(ar, "Também guarda (e tira da pasta) os arquivos ainda não rastreados").clicked() {
        acts.push(Act::StashToggleUntracked);
    }
    let top = by + BTN_H + 8.0;
    p.rect_filled(Rect::from_min_size(Pos2::new(body.min.x, top), Vec2::new(body.width(), 1.0)), 0.0, theme::pal().separator);
    let list = Rect::from_min_max(Pos2::new(body.min.x, top + 1.0), body.max);
    if deck.git.stashes.is_empty() {
        let text = if deck.git.stash_loading {
            "Lendo os stashes…"
        } else {
            "Nenhum stash. Guarde o trabalho em andamento para voltar a ele depois."
        };
        return note(ui, list, text, theme::pal().text_2);
    }
    let stashes = &deck.git.stashes;
    let mut child = scroll_child(ui, list);
    egui::ScrollArea::vertical().id_salt("git-stashes").auto_shrink([false, false]).show_rows(
        &mut child,
        STASH_ROW_H,
        stashes.len(),
        |ui, range| {
            for i in range {
                let s = &stashes[i];
                let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), STASH_ROW_H), Sense::click());
                let hovered = ui.rect_contains_pointer(r);
                let p = ui.painter();
                if hovered {
                    p.rect_filled(r.shrink2(Vec2::new(4.0, 1.0)), 4.0, theme::pal().hover);
                }
                let w = r.width() - 28.0;
                let g = galley_trunc(p, &s.message, theme::ui(12.5), theme::pal().text, w);
                place(
                    p,
                    Rect::from_min_size(Pos2::new(r.min.x + 14.0, r.min.y + 5.0), Vec2::new(w, 17.0)),
                    Align2::LEFT_CENTER,
                    g,
                    theme::pal().text,
                );
                // Segunda linha: índice/data à esquerda; ações à direita.
                let y2 = r.min.y + 24.0;
                let mut right = r.max.x - 12.0;
                let mut clicked_button = false;
                let h = 18.0;
                for (label, tooltip, look, act) in [
                    ("Apagar", "Apaga este stash (não dá para desfazer)", Look::Danger, Act::AskDropStash(s.clone())),
                    ("Aplicar e apagar", "Aplica as alterações e remove o stash (git stash pop)", Look::Normal, Act::StashPop(s.index)),
                    ("Aplicar", "Aplica as alterações e mantém o stash (git stash apply)", Look::Normal, Act::StashApply(s.index)),
                ] {
                    if pill_small(ui, &mut right, y2, h, label, tooltip, look, !busy, Id::new(("git-stash-btn", i, label))) {
                        acts.push(act);
                        clicked_button = true;
                    }
                }
                let meta = s.date.clone();
                let mg = galley_trunc(ui.painter(), &meta, theme::ui(11.0), theme::pal().text_2, (right - r.min.x - 20.0).max(20.0));
                place(
                    ui.painter(),
                    Rect::from_min_max(Pos2::new(r.min.x + 14.0, y2), Pos2::new(right, y2 + h)),
                    Align2::LEFT_CENTER,
                    mg,
                    theme::pal().text_2,
                );
                if tip(
                    resp,
                    format!(
                        "stash@{{{}}}
Clique para ver as alterações guardadas",
                        s.index
                    ),
                )
                .clicked()
                    && !clicked_button
                {
                    acts.push(Act::OpenStash(s.clone()));
                }
            }
        },
    );
}

/// Pílula menor (11 px) para as linhas de lista.
#[allow(clippy::too_many_arguments)]
fn pill_small(ui: &Ui, right: &mut f32, y: f32, h: f32, label: &str, tooltip: &str, look: Look, enabled: bool, id: Id) -> bool {
    let painter = ui.painter();
    let w = galley(painter, label, theme::ui(11.0), theme::pal().text).size().x + 12.0;
    let r = Rect::from_min_size(Pos2::new(*right - w, y), Vec2::new(w, h));
    *right = r.min.x - 4.0;
    let resp = if enabled { clickable(ui, r, id) } else { ui.interact(r, id, Sense::hover()) };
    let hot = enabled && resp.hovered();
    let p = theme::pal();
    let (bg, border, fg) = match look {
        Look::Danger => (
            if hot { theme::with_alpha(p.danger, 0.18) } else { theme::with_alpha(p.danger, 0.10) },
            theme::with_alpha(p.danger, 0.35),
            p.danger_text,
        ),
        _ => (
            if hot { theme::with_alpha(p.text_strong, 0.08) } else { theme::with_alpha(p.text_strong, 0.04) },
            theme::with_alpha(p.text_strong, 0.14),
            p.text,
        ),
    };
    frame(painter, r, 5.0, bg, border);
    text_in(painter, r, Align2::CENTER_CENTER, label, theme::ui(11.0), if enabled { fg } else { theme::with_opacity(fg, 0.4) });
    tip(resp, tooltip.to_owned()).clicked() && enabled
}

fn stash_diff(ui: &mut Ui, body: Rect, open: &OpenStashDiff, busy: bool, acts: &mut Vec<Act>) {
    let s: &GitStash = &open.stash;
    let title = format!("stash@{{{}}}  {}", s.index, s.message);
    let (head, back, _) = header(ui, body, "stash", &title, &s.message, 0.0);
    if back {
        acts.push(Act::CloseStash);
    }
    // Segunda linha: as ações (não cabem junto do título).
    let bar = Rect::from_min_size(Pos2::new(body.min.x, head.max.y), Vec2::new(body.width(), 30.0));
    ui.painter().rect_filled(bar, 0.0, theme::pal().chip);
    let mut right = bar.max.x - 8.0;
    let (y, h) = (bar.min.y + 3.0, 24.0);
    if pill(ui, &mut right, y, h, "Apagar", "Apaga este stash (não dá para desfazer)", Look::Danger, !busy, Id::new("git-sd-drop")) {
        acts.push(Act::AskDropStash(s.clone()));
    }
    if pill(ui, &mut right, y, h, "Aplicar e apagar", "Aplica e remove o stash (git stash pop)", Look::Normal, !busy, Id::new("git-sd-pop"))
    {
        acts.push(Act::StashPop(s.index));
    }
    if pill(ui, &mut right, y, h, "Aplicar", "Aplica e mantém o stash (git stash apply)", Look::Primary, !busy, Id::new("git-sd-apply")) {
        acts.push(Act::StashApply(s.index));
    }
    let area = Rect::from_min_max(Pos2::new(body.min.x, bar.max.y), body.max);
    match &open.diff {
        None => note(ui, area, "Carregando o stash…", theme::pal().text_2),
        Some(Err(e)) => note(ui, area, e, theme::pal().danger_text),
        Some(Ok(diff)) => diff_lines(ui, area, diff, None, acts),
    }
}

// ---------- Conflitos ----------

/// O que a dupla de letras do status quer dizer, em português.
fn conflict_kind(index: char, worktree: char) -> &'static str {
    match (index, worktree) {
        ('U', 'U') => "os dois lados alteraram",
        ('A', 'A') => "os dois lados adicionaram",
        ('D', 'D') => "os dois lados apagaram",
        ('A', 'U') | ('U', 'A') => "adicionado só por um dos lados",
        ('D', 'U') => "nós apagamos, eles alteraram",
        ('U', 'D') => "nós alteramos, eles apagaram",
        _ => "em conflito",
    }
}

pub(super) fn conflicts(deck: &mut Deck, ui: &mut Ui, body: Rect, st: &GitStatus, busy: bool, acts: &mut Vec<Act>) {
    if let Some(open) = deck.git.conflict_open.take() {
        conflict_file(ui, body, &open, st, busy, acts);
        deck.git.conflict_open = Some(open);
        return;
    }
    let files: Vec<_> = st.entries.iter().filter(|e| e.kind == EntryKind::Unmerged).collect();
    if files.is_empty() {
        let text = if st.state == RepoState::Normal {
            "Nenhum arquivo em conflito."
        } else {
            "Todos os conflitos foram resolvidos. Clique em Continuar na faixa acima para concluir."
        };
        return note(ui, body, text, theme::pal().text_2);
    }
    let mut child = scroll_child(ui, body);
    egui::ScrollArea::vertical().id_salt("git-conflicts").auto_shrink([false, false]).show(&mut child, |ui| {
        ui.add_space(6.0);
        let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), Sense::hover());
        let hint = "Abra cada arquivo e escolha, bloco a bloco, ficar com o nosso lado, o deles ou os dois.";
        let g = ui.painter().layout(hint.to_owned(), theme::ui(11.5), theme::pal().text_2, r.width() - 28.0);
        ui.painter().galley(r.min + Vec2::new(14.0, 0.0), g, theme::pal().text_2);
        for (i, e) in files.iter().enumerate() {
            let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 38.0), Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(r.shrink2(Vec2::new(4.0, 1.0)), 4.0, theme::pal().hover);
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let p = ui.painter();
            let letters = format!("{}{}", e.index, e.worktree);
            text_in(
                p,
                Rect::from_min_size(Pos2::new(r.min.x + 12.0, r.min.y), Vec2::new(22.0, r.height())),
                Align2::CENTER_CENTER,
                &letters,
                theme::mono_bold(12.0),
                theme::pal().danger_text,
            );
            let w = r.width() - 56.0;
            let g = galley_trunc(p, &e.path, theme::ui(12.5), theme::pal().text, w);
            place(
                p,
                Rect::from_min_size(Pos2::new(r.min.x + 42.0, r.min.y + 4.0), Vec2::new(w, 17.0)),
                Align2::LEFT_CENTER,
                g,
                theme::pal().text,
            );
            let g = galley_trunc(p, conflict_kind(e.index, e.worktree), theme::ui(11.0), theme::pal().text_2, w);
            place(
                p,
                Rect::from_min_size(Pos2::new(r.min.x + 42.0, r.min.y + 21.0), Vec2::new(w, 14.0)),
                Align2::LEFT_CENTER,
                g,
                theme::pal().text_2,
            );
            if tip(resp, format!("{}\nClique para resolver", e.path)).clicked() {
                acts.push(Act::OpenConflict(e.path.clone()));
            }
            let _ = i;
        }
    });
}

fn conflict_file(ui: &mut Ui, body: Rect, open: &OpenConflict, st: &GitStatus, busy: bool, acts: &mut Vec<Act>) {
    let reserved = pill_w(ui, "Abrir") + 6.0;
    let (head, back, mut right) =
        header(ui, body, "conflict", &open.path, &format!("{}\n(Abrir: no programa do sistema)", open.path), reserved);
    if back {
        acts.push(Act::CloseConflict);
    }
    if pill(
        ui,
        &mut right,
        head.min.y + 3.0,
        HEADER_H - 6.0,
        "Abrir",
        "Abre o arquivo no programa do sistema",
        Look::Normal,
        true,
        Id::new("git-cf-open"),
    ) {
        acts.push(Act::OpenFile(open.path.clone()));
    }
    let area = Rect::from_min_max(Pos2::new(body.min.x, head.max.y), body.max);
    let file = match &open.file {
        None => return note(ui, area, "Lendo o arquivo…", theme::pal().text_2),
        Some(Err(e)) => return note(ui, area, e, theme::pal().danger_text),
        Some(Ok(f)) => f,
    };
    let still = st.entries.iter().find(|e| e.path == open.path && e.kind == EntryKind::Unmerged);
    let p = ui.painter_at(area);
    let (x0, x1) = (area.min.x + 12.0, area.max.x - 12.0);
    // Linha do arquivo inteiro: rótulo + Nosso / Deles / Ambos.
    let mut y = area.min.y + 8.0;
    let remaining = file.blocks.len();
    let status_line = if file.binary {
        "Arquivo binário: escolha um dos lados.".to_owned()
    } else if remaining == 0 {
        match still {
            Some(e) => format!("Sem marcadores no arquivo ({}).", conflict_kind(e.index, e.worktree)),
            None => "Resolvido.".to_owned(),
        }
    } else {
        format!("{remaining} bloco(s) em conflito")
    };
    text_in(
        &p,
        Rect::from_min_size(Pos2::new(x0, y), Vec2::new(x1 - x0, 16.0)),
        Align2::LEFT_CENTER,
        &status_line,
        theme::ui_semibold(12.0),
        theme::pal().text,
    );
    y += 22.0;
    let g = galley_spaced(&p, "ARQUIVO INTEIRO", theme::label(10.0), theme::pal().text_2, 0.8);
    place(&p, Rect::from_min_size(Pos2::new(x0, y), Vec2::new(110.0, 24.0)), Align2::LEFT_CENTER, g, theme::pal().text_2);
    let mut right = x1;
    for (label, choice, tooltip) in [
        ("Ambos", ConflictChoice::Both, "Junta os dois lados, o nosso primeiro"),
        ("Deles", ConflictChoice::Theirs, "Fica com a versão do outro lado (git checkout --theirs) e já prepara"),
        ("Nosso", ConflictChoice::Ours, "Fica com a nossa versão (git checkout --ours) e já prepara"),
    ] {
        let enabled = !busy && !(choice == ConflictChoice::Both && (file.binary || remaining == 0));
        if pill(ui, &mut right, y, 24.0, label, tooltip, Look::Normal, enabled, Id::new(("git-cf-all", label))) {
            acts.push(Act::ResolveFile(open.path.clone(), choice));
        }
    }
    y += 32.0;
    // Marcar como resolvido (só quando não sobrou marcador).
    let mut right = x1;
    let ready = remaining == 0 && !file.binary && still.is_some();
    let why = if ready { "Prepara o arquivo como está (git add)" } else { "Resolva todos os blocos antes de marcar" };
    if pill(ui, &mut right, y, BTN_H, "Marcar como resolvido", why, Look::Primary, ready && !busy, Id::new("git-cf-mark")) {
        acts.push(Act::Stage(vec![open.path.clone()]));
    }
    y += BTN_H + 8.0;
    p.rect_filled(Rect::from_min_size(Pos2::new(area.min.x, y), Vec2::new(area.width(), 1.0)), 0.0, theme::pal().separator);
    let list = Rect::from_min_max(Pos2::new(area.min.x, y + 1.0), area.max);
    if file.blocks.is_empty() || list.height() < 40.0 {
        return;
    }
    let mut child = scroll_child(ui, list);
    egui::ScrollArea::vertical().id_salt(("git-cf-blocks", &open.path)).auto_shrink([false, false]).show(&mut child, |ui| {
        ui.add_space(6.0);
        for (i, b) in file.blocks.iter().enumerate() {
            block_card(ui, &open.path, i, b, busy, acts);
            ui.add_space(8.0);
        }
    });
}

/// Um bloco de conflito: os dois lados lado a lado na ordem do arquivo e as três escolhas.
fn block_card(ui: &mut Ui, path: &str, index: usize, b: &ConflictBlock, busy: bool, acts: &mut Vec<Act>) {
    let side_h = |lines: &[String]| (lines.len().clamp(1, BLOCK_LINES + 1) as f32) * CODE_ROW_H + 6.0;
    let h = 24.0 + 18.0 + side_h(&b.ours) + 18.0 + side_h(&b.theirs) + 10.0 + 24.0 + 10.0;
    let w = ui.available_width() - 20.0;
    let (r, _) = ui.allocate_exact_size(Vec2::new(w + 20.0, h), Sense::hover());
    let r = Rect::from_min_size(r.min + Vec2::new(10.0, 0.0), Vec2::new(w, h));
    let p = ui.painter().clone();
    frame(&p, r, 8.0, theme::pal().recess, theme::pal().glass_border);
    text_in(
        &p,
        Rect::from_min_size(r.min + Vec2::new(10.0, 0.0), Vec2::new(w - 20.0, 24.0)),
        Align2::LEFT_CENTER,
        &format!("Bloco {} · linha {}", index + 1, b.start_line),
        theme::ui_semibold(11.5),
        theme::pal().text,
    );
    let mut y = r.min.y + 24.0;
    let font = theme::mono(11.5);
    for (title, label, lines, tint) in [
        ("NOSSO", &b.ours_label, &b.ours, theme::with_alpha(theme::pal().success, 0.12)),
        ("DELES", &b.theirs_label, &b.theirs, theme::with_alpha(theme::pal().info, 0.14)),
    ] {
        let t = if label.is_empty() { title.to_owned() } else { format!("{title} · {label}") };
        let g = galley_spaced(&p, &t, theme::label(10.0), theme::pal().text_2, 0.8);
        place(
            &p,
            Rect::from_min_size(Pos2::new(r.min.x + 10.0, y), Vec2::new(w - 20.0, 18.0)),
            Align2::LEFT_CENTER,
            g,
            theme::pal().text_2,
        );
        y += 18.0;
        let sh = side_h(lines);
        let box_r = Rect::from_min_size(Pos2::new(r.min.x + 8.0, y), Vec2::new(w - 16.0, sh));
        p.rect_filled(box_r, 4.0, tint);
        let clip = p.with_clip_rect(box_r);
        if lines.is_empty() {
            clip.text(
                Pos2::new(box_r.min.x + 8.0, box_r.min.y + 3.0 + CODE_ROW_H / 2.0),
                Align2::LEFT_CENTER,
                "(vazio)",
                font.clone(),
                theme::pal().text_2,
            );
        }
        for (k, line) in lines.iter().take(BLOCK_LINES).enumerate() {
            let shown: String = line.chars().take(300).collect();
            clip.text(
                Pos2::new(box_r.min.x + 8.0, box_r.min.y + 3.0 + CODE_ROW_H * (k as f32 + 0.5)),
                Align2::LEFT_CENTER,
                shown,
                font.clone(),
                theme::pal().text,
            );
        }
        if lines.len() > BLOCK_LINES {
            let k = BLOCK_LINES;
            clip.text(
                Pos2::new(box_r.min.x + 8.0, box_r.min.y + 3.0 + CODE_ROW_H * (k as f32 + 0.5)),
                Align2::LEFT_CENTER,
                format!("… +{} linha(s)", lines.len() - BLOCK_LINES),
                font.clone(),
                theme::pal().text_2,
            );
        }
        y += sh;
    }
    y += 10.0;
    let mut right = r.max.x - 10.0;
    for (label, choice, tooltip) in [
        ("Ambos", ConflictChoice::Both, "Fica com os dois, o nosso primeiro"),
        ("O deles", ConflictChoice::Theirs, "Fica só com o deles neste bloco"),
        ("O nosso", ConflictChoice::Ours, "Fica só com o nosso neste bloco"),
    ] {
        if pill(ui, &mut right, y, 24.0, label, tooltip, Look::Normal, !busy, Id::new(("git-cf-block", path, index, label))) {
            acts.push(Act::ResolveBlock(path.to_owned(), index, choice));
        }
    }
}

/// Aba de subvisões (Alterações / Histórico / Stash / Conflitos). Devolve a altura usada.
pub(super) fn tab_strip(deck: &mut Deck, ui: &mut Ui, rect: Rect, y: f32, st: &GitStatus, acts: &mut Vec<Act>) -> f32 {
    let h = 30.0;
    let strip = Rect::from_min_size(Pos2::new(rect.min.x, y), Vec2::new(rect.width(), h));
    let p = ui.painter_at(strip);
    p.rect_filled(strip, 0.0, theme::pal().chip);
    p.rect_filled(Rect::from_min_max(Pos2::new(strip.min.x, strip.max.y - 1.0), strip.max), 0.0, theme::pal().separator);
    let conflicts = crate::git_ui::conflict_count(st);
    let changes = st.entries.len();
    let mut tabs: Vec<(GitSubView, String)> = vec![
        (GitSubView::Changes, if changes > 0 { format!("Alterações {changes}") } else { "Alterações".into() }),
        (GitSubView::History, "Histórico".into()),
        (GitSubView::Stash, if st.stash_count > 0 { format!("Stash {}", st.stash_count) } else { "Stash".into() }),
    ];
    if conflicts > 0 {
        tabs.push((GitSubView::Conflicts, format!("Conflitos {conflicts}")));
    }
    // Conflitos some quando não há mais nenhum; a visão volta para Alterações.
    if deck.git.subview == GitSubView::Conflicts && conflicts == 0 && st.state == RepoState::Normal {
        deck.git.subview = GitSubView::Changes;
    }
    let mut x = strip.min.x + 6.0;
    for (view, label) in tabs {
        let active = deck.git.subview == view;
        let danger = view == GitSubView::Conflicts;
        let color = if danger {
            theme::pal().danger_text
        } else if active {
            theme::pal().text
        } else {
            theme::pal().text_2
        };
        let font = if active { theme::ui_semibold(12.0) } else { theme::ui(12.0) };
        let w = galley(&p, &label, font.clone(), color).size().x + 16.0;
        let r = Rect::from_min_size(Pos2::new(x, strip.min.y), Vec2::new(w, h - 1.0));
        x = r.max.x + 2.0;
        let resp = clickable(ui, r, Id::new(("git-subtab", label.split(' ').next().unwrap_or(""))));
        if resp.hovered() && !active {
            p.rect_filled(r.shrink2(Vec2::new(0.0, 3.0)), 5.0, theme::pal().hover);
        }
        text_in(&p, r, Align2::CENTER_CENTER, &label, font, if resp.hovered() { theme::pal().text } else { color });
        if active {
            p.rect_filled(
                Rect::from_min_size(Pos2::new(r.min.x + 6.0, strip.max.y - 2.0), Vec2::new(w - 12.0, 2.0)),
                1.0,
                if danger { theme::pal().danger } else { deck.accent() },
            );
        }
        if resp.clicked() && !active {
            acts.push(Act::SetSub(view));
        }
    }
    h
}

/// Faixa do merge/rebase em andamento, com Continuar e Abortar. Devolve a altura usada.
pub(super) fn op_banner(ui: &mut Ui, rect: Rect, y: f32, st: &GitStatus, busy: bool, acts: &mut Vec<Act>) -> f32 {
    let (name, noun) = match st.state {
        RepoState::Normal => return 0.0,
        RepoState::Merging => ("Mesclagem", "a mesclagem"),
        RepoState::Rebasing => ("Rebase", "o rebase"),
        RepoState::CherryPicking => ("Cherry-pick", "o cherry-pick"),
        RepoState::Reverting => ("Revert", "o revert"),
    };
    let h = 32.0;
    let r = Rect::from_min_size(Pos2::new(rect.min.x, y), Vec2::new(rect.width(), h));
    let p = ui.painter_at(r);
    let warn = theme::pal().warning;
    p.rect_filled(r, 0.0, theme::with_opacity(warn, 0.10));
    p.rect_filled(Rect::from_min_size(r.min, Vec2::new(3.0, h)), 0.0, warn);
    let conflicts = crate::git_ui::conflict_count(st);
    let mut right = r.max.x - 8.0;
    let top = r.min.y + 4.0;
    if pill(
        ui,
        &mut right,
        top,
        h - 8.0,
        "Abortar",
        &format!("Cancela {noun} e volta ao estado de antes"),
        Look::Danger,
        !busy,
        Id::new("git-op-abort"),
    ) {
        acts.push(Act::AskAbort(st.state));
    }
    let can = !busy && conflicts == 0;
    let why = if conflicts > 0 { "Resolva os conflitos antes de continuar".to_owned() } else { format!("Conclui {noun}") };
    if pill(ui, &mut right, top, h - 8.0, "Continuar", &why, Look::Primary, can, Id::new("git-op-continue")) {
        acts.push(Act::Continue);
    }
    let text = if conflicts > 0 { format!("{name} · {conflicts} conflito(s)") } else { format!("{name} em andamento") };
    let tr = Rect::from_min_max(Pos2::new(r.min.x + 12.0, r.min.y), Pos2::new(right, r.max.y));
    let g = galley_trunc(&p, &text, theme::ui_semibold(12.0), warn, tr.width());
    place(&p, tr, Align2::LEFT_CENTER, g, warn);
    tip(ui.interact(tr, Id::new("git-op-text"), Sense::hover()), text);
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflict_letters_are_explained() {
        assert_eq!(conflict_kind('U', 'U'), "os dois lados alteraram");
        assert_eq!(conflict_kind('D', 'U'), "nós apagamos, eles alteraram");
        assert_eq!(conflict_kind('U', 'D'), "nós alteramos, eles apagaram");
        assert_eq!(conflict_kind('?', '?'), "em conflito");
    }
}
