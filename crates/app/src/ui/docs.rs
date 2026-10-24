//! Modo Documentos do centro (ver `docs.rs`): a chave Agentes | Documentos da barra de título, a faixa de
//! abas de documento (o mesmo desenho de `tabs.rs`) e o documento com a largura toda do centro.
//! Diff, arquivo de texto (que também se edita) e markdown.

use egui::{Align2, Id, Key, Painter, Pos2, Rect, Sense, Ui, Vec2};

use super::panes::HEAD_H;
use super::tabs::{self, Item, Lead, StripClick, StripKind};
use super::{
    button, button_width, clickable, code, diff_view, frame, galley, galley_trunc, place, section_label, text_in, text_w, tip, ButtonKind,
};
use crate::deck::Deck;
use crate::docs::{file_name, After, Answer, CenterView, DiffDoc, DiffFile, DocKind, DocState, DocTab, Find, Unsaved};
use crate::highlight::Lang;
use crate::theme;

/// Altura da linha com o título do documento e as ações dele.
const SUBHEAD_H: f32 = 28.0;
/// Altura de uma linha da lista de arquivos de um diff e do cabeçalho dela.
const FILE_ROW_H: f32 = 20.0;
const FILE_HEAD_H: f32 = super::SECTION_LABEL_H;
/// Abaixo desta largura a lista de arquivos do diff passa para cima do diff.
const NARROW_W: f32 = 560.0;
/// Largura máxima da coluna do markdown (acima disso o texto fica comprido demais para ler).
const MD_MAX_W: f32 = 820.0;

// ---------------------------------------------------------------------------
// A chave
// ---------------------------------------------------------------------------

/// O que um clique na chave pede.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Switched {
    Agents,
    Docs,
}

/// Altura do trilho da chave (segmentos de 24 px e 2 px de cada lado).
pub const SWITCH_H: f32 = 28.0;
/// A chave compacta da barra de título (barra lateral recolhida): segmentos de 20 px.
pub const SWITCH_COMPACT_H: f32 = 24.0;
/// O selo de "Agentes": um ponto de 6 px, 6 px depois do nome.
const SELO: f32 = 6.0;
const SELO_GAP: f32 = 6.0;

/// Largura da chave compacta: dois segmentos iguais, do tamanho do maior rótulo.
pub fn switch_compact_width(painter: &Painter, deck: &Deck) -> f32 {
    let font = theme::ui_semibold(11.5);
    let agents = text_w(painter, "Agentes", font.clone()) + SELO_GAP + SELO;
    let docs = text_w(painter, &docs_label(deck.docs.count(&deck.active_project)), font);
    2.0 * (agents.max(docs) + 20.0) + 4.0
}

/// A palavra do segundo lado da chave: inteira se ela e a contagem (`full_w`) cabem no segmento de largura
/// `seg_w` com folga, abreviada se não.
fn docs_word(full_w: f32, seg_w: f32) -> &'static str {
    if full_w + 8.0 <= seg_w {
        "Documentos"
    } else {
        "Docs"
    }
}

/// "Documentos n"; sem documento aberto, só "Documentos" (o zero é ruído).
fn docs_label(n: usize) -> String {
    if n == 0 {
        "Documentos".to_owned()
    } else {
        format!("Documentos {n}")
    }
}

/// Pinta a chave **Agentes | Documentos n** em `rect` (o trilho, `SWITCH_H` ou `SWITCH_COMPACT_H` de
/// altura): o segmento ativo na cor de destaque. O selo de "Agentes" mostra o que acontece nas abas
/// enquanto se lê: um ponto se alguma terminou um turno sem ser vista (na cor do texto sobre o destaque
/// se o lado está ativo, na cor do agente se não) e, lendo documentos, um que pulsa se alguma trabalha
/// agora. Um clique troca o centro.
pub fn switch(deck: &mut Deck, ui: &Ui, rect: Rect, id: &str) {
    let painter = ui.painter();
    let p = theme::pal();
    let n_docs = deck.docs.count(&deck.active_project);
    let badge = deck.agents_badge();
    let view = deck.docs.view;
    let accent = theme::accent();
    let docs_enabled = n_docs > 0;
    let pulse = badge.done == 0 && badge.busy > 0 && view == CenterView::Docs;

    painter.rect_filled(rect, 7.0, p.recess);
    let inner = rect.shrink(2.0);
    let w = inner.width() / 2.0;
    let mut picked = None;
    for (i, which) in [Switched::Agents, Switched::Docs].into_iter().enumerate() {
        let seg = Rect::from_min_size(Pos2::new(inner.min.x + w * i as f32, inner.min.y), Vec2::new(w, inner.height()));
        let enabled = which == Switched::Agents || docs_enabled;
        let resp = if enabled { clickable(ui, seg, Id::new((id, i))) } else { ui.interact(seg, Id::new((id, i)), Sense::hover()) };
        let active = (which == Switched::Agents) == (view == CenterView::Agents);
        let (fill, fg, font) = if active {
            (Some(accent), p.on_accent, theme::ui_semibold(11.5))
        } else if resp.hovered() && enabled {
            (Some(p.hover), p.text, theme::ui(11.5))
        } else if enabled {
            (None, p.text_2, theme::ui(11.5))
        } else {
            (None, theme::with_opacity(p.text_2, 0.5), theme::ui(11.5))
        };
        if let Some(fill) = fill {
            painter.rect_filled(seg, 5.0, fill);
        }
        if which == Switched::Agents {
            let has_selo = badge.done > 0 || pulse;
            let g = galley(painter, "Agentes", font, fg);
            let total = g.size().x + if has_selo { SELO_GAP + SELO } else { 0.0 };
            let x = seg.center().x - total / 2.0;
            let text = place(
                painter,
                Rect::from_min_size(Pos2::new(x, seg.min.y), Vec2::new(g.size().x, seg.height())),
                Align2::LEFT_CENTER,
                g,
                fg,
            );
            let c = Pos2::new(text.max.x + SELO_GAP + SELO / 2.0, seg.center().y);
            let selo = if active { p.on_accent } else { accent };
            if badge.done > 0 {
                painter.circle_filled(c, SELO / 2.0, selo);
            } else if pulse {
                let t = ui.ctx().input(|i| i.time);
                let wave = (t * 4.8).sin() as f32 * 0.5 + 0.5;
                painter.circle_filled(c, SELO / 2.0 + 3.0 * wave, theme::with_opacity(selo, 0.35 * (1.0 - wave)));
                painter.circle_filled(c, SELO / 2.0, selo);
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
            }
        } else {
            // "Documentos" (ou "Docs", na barra lateral mais estreita) e a contagem, esta em `text_3`
            // (apagada sobre o destaque); sem documento aberto, sem contagem.
            let count = if n_docs > 0 { n_docs.to_string() } else { String::new() };
            let count_gap = if n_docs > 0 { 5.0 } else { 0.0 };
            let count_w = text_w(painter, &count, theme::ui(11.5));
            let label = docs_word(text_w(painter, "Documentos", font.clone()) + count_gap + count_w, seg.width());
            let g = galley(painter, label, font, fg);
            let count_color = if active {
                theme::with_opacity(p.on_accent, 0.7)
            } else if enabled {
                p.text_3
            } else {
                fg
            };
            let cg = galley(painter, &count, theme::ui(11.5), count_color);
            let total = g.size().x + count_gap + cg.size().x;
            let x = seg.center().x - total / 2.0;
            let gw = g.size().x;
            place(painter, Rect::from_min_size(Pos2::new(x, seg.min.y), Vec2::new(gw, seg.height())), Align2::LEFT_CENTER, g, fg);
            let cw = cg.size().x;
            place(
                painter,
                Rect::from_min_size(Pos2::new(x + gw + count_gap, seg.min.y), Vec2::new(cw, seg.height())),
                Align2::LEFT_CENTER,
                cg,
                count_color,
            );
        }
        let tip_text = switch_tip(which, n_docs, badge.done, badge.busy);
        if tip(resp, tip_text).clicked() && enabled {
            picked = Some(which);
        }
    }
    match picked {
        Some(Switched::Agents) => deck.show_agents(),
        Some(Switched::Docs) => deck.docs.view = CenterView::Docs,
        None => {}
    }
}

/// A dica de cada lado da chave (com o atalho).
fn switch_tip(which: Switched, n_docs: usize, done: usize, busy: usize) -> String {
    match which {
        Switched::Agents => {
            let mut parts = vec!["Agentes: as janelas dos terminais".to_owned()];
            if done > 0 {
                parts.push(format!("{done} terminou sem você ver"));
            }
            if busy > 0 {
                parts.push(format!("{busy} trabalhando"));
            }
            parts.push("Ctrl+Shift+D alterna".to_owned());
            parts.join("\n")
        }
        Switched::Docs if n_docs == 0 => {
            "Documentos: nada aberto neste projeto\nAbra um diff pela mesa de tarefas, um commit pela aba Git\nou um arquivo (duplo clique no explorador)".to_owned()
        }
        Switched::Docs => "Documentos: diff, arquivos e markdown, só para ler\nCtrl+Shift+D alterna".to_owned(),
    }
}

// ---------------------------------------------------------------------------
// O centro em modo Documentos
// ---------------------------------------------------------------------------

/// O que um clique no documento pede (aplicado depois de pintar).
enum Act {
    Focus(u64),
    Close(u64),
    Reload(u64),
    OpenExternal(String),
    SelectFile(u64, usize),
    Save(u64),
}

/// Pinta os documentos do projeto ativo no lugar das janelas dos agentes.
pub fn draw(deck: &mut Deck, ui: &mut Ui, area: Rect) {
    let painter = ui.painter_at(area);
    let project = deck.active_project.clone();
    let accent = deck.accent();
    let items: Vec<Item> = deck
        .docs
        .of_project(&project)
        .map(|t| Item {
            uid: t.uid,
            label: t.title.clone(),
            color: accent,
            alive: true,
            busy: false,
            done: false,
            mic: crate::dictation::ButtonVisual::Idle,
            lead: Lead::Glyph(glyph(&t.kind)),
            tip: t.subtitle.clone(),
            dirty: t.dirty(),
        })
        .collect();
    let uids: Vec<u64> = items.iter().map(|i| i.uid).collect();
    let Some(current) = deck.docs.current_uid(&project).and_then(|c| uids.iter().position(|u| *u == c)) else {
        text_in(&painter, area, Align2::CENTER_CENTER, "Nenhum documento aberto neste projeto.", theme::ui(13.0), theme::pal().text_2);
        return;
    };

    // ---------- Faixa de abas ----------
    let head = Rect::from_min_size(area.min, Vec2::new(area.width(), HEAD_H));
    painter.rect_filled(head, 0.0, theme::pal().strip);
    painter.rect_filled(Rect::from_min_max(Pos2::new(head.min.x, head.max.y - 1.0), head.max), 0.0, theme::pal().separator);
    let strip_rect = Rect::from_min_max(head.min, Pos2::new(head.max.x, head.max.y - 1.0));
    let mut acts = Vec::new();
    match tabs::strip_items(
        ui,
        strip_rect,
        &items,
        current,
        true,
        Id::new("doc-tabs"),
        StripKind { scope: "doc-tab", tab_menu: false, colored: false },
    ) {
        Some(StripClick::Activate(n)) => acts.push(Act::Focus(uids[n])),
        Some(StripClick::Close(n)) => acts.push(Act::Close(uids[n])),
        _ => {}
    }

    // ---------- Documento em foco ----------
    let uid = uids[current];
    let below = Rect::from_min_max(Pos2::new(area.min.x, head.max.y), area.max);
    let mut cache = std::mem::take(&mut deck.docs.md_cache);
    if let Some(tab) = deck.docs.get_mut(uid) {
        document(ui, below, tab, &mut cache, &mut acts);
    }
    deck.docs.md_cache = cache;

    for act in acts {
        match act {
            Act::Focus(uid) => deck.docs.focus(uid),
            Act::Close(uid) => deck.docs_request_close(uid),
            Act::Reload(uid) => deck.docs_reload(uid),
            Act::Save(uid) => deck.docs_save(uid, After::Nothing),
            Act::OpenExternal(rel) => {
                if let Err(e) = deck.core.open_path(&rel) {
                    deck.system(format!("Não foi possível abrir {rel} no programa padrão: {e}"));
                }
            }
            Act::SelectFile(uid, n) => {
                if let Some(DocState::Diff(d)) = deck.docs.get_mut(uid).map(|t| &mut t.state) {
                    d.select(n);
                }
            }
        }
    }
}

/// O sinal do tipo de documento, antes do nome da aba.
fn glyph(kind: &DocKind) -> &'static str {
    match kind {
        DocKind::TaskDiff { .. } | DocKind::CommitDiff(_) | DocKind::FileDiff { .. } => "±",
        DocKind::File(_) => "≡",
        DocKind::Markdown(_) => "¶",
    }
}

/// A linha de título do documento (com as ações), os avisos e a busca; depois, o corpo dele.
fn document(ui: &mut Ui, area: Rect, tab: &mut DocTab, cache: &mut egui_commonmark::CommonMarkCache, acts: &mut Vec<Act>) {
    let painter = ui.painter_at(area);
    let sub = Rect::from_min_size(area.min, Vec2::new(area.width(), SUBHEAD_H));
    painter.rect_filled(Rect::from_min_max(Pos2::new(sub.min.x, sub.max.y - 1.0), sub.max), 0.0, theme::pal().separator);
    let uid = tab.uid;

    // Ações à direita, da última para a primeira.
    let mut right = sub.max.x - 8.0;
    let cy = sub.min.y + (SUBHEAD_H - 1.0) / 2.0;
    if let Some(path) = tab.kind.file_path() {
        let label = "Abrir no programa padrão";
        if action(
            ui,
            &painter,
            &mut right,
            cy,
            (label, "Abre o arquivo no programa que o Windows associou a ele"),
            Id::new(("doc-open", uid)),
            (ButtonKind::Secondary, true),
        ) {
            acts.push(Act::OpenExternal(path.to_owned()));
        }
    }
    if tab.kind.reloadable() {
        let loading = matches!(tab.state, DocState::Loading);
        let tooltip = "Lê o documento de novo (com edição não salva, pergunta antes)";
        if action(ui, &painter, &mut right, cy, ("Recarregar", tooltip), Id::new(("doc-reload", uid)), (ButtonKind::Secondary, !loading)) {
            acts.push(Act::Reload(uid));
        }
    }
    let mut dirty = false;
    if let DocState::Source(src) = &mut tab.state {
        dirty = src.dirty();
        let is_md = matches!(tab.kind, DocKind::Markdown(_));
        if src.can_edit() && src.editing {
            let hint = "Para de editar (o que você digitou continua na tela até gravar ou fechar)";
            if action(ui, &painter, &mut right, cy, ("Só leitura", hint), Id::new(("doc-stop", uid)), (ButtonKind::Secondary, true)) {
                src.editing = false;
            }
            let ready = src.dirty() && src.external.is_none() && !src.saving;
            let label = if src.saving { "Gravando…" } else { "Salvar" };
            let hint =
                if src.external.is_some() { "Resolva primeiro o aviso: o arquivo mudou no disco" } else { "Grava o arquivo (Ctrl+S)" };
            if action(ui, &painter, &mut right, cy, (label, hint), Id::new(("doc-save", uid)), (ButtonKind::Primary, ready)) {
                acts.push(Act::Save(uid));
            }
        } else if src.can_edit() {
            let hint = "Edita o arquivo aqui mesmo (Ctrl+S grava)";
            if action(ui, &painter, &mut right, cy, ("Editar", hint), Id::new(("doc-edit", uid)), (ButtonKind::Primary, true)) {
                src.set_editing(true);
            }
        }
        if is_md && !src.editing {
            let (label, hint) = if src.raw {
                ("Ver formatado", "Mostra o markdown formatado")
            } else {
                ("Ver código", "Mostra o texto do markdown, com realce")
            };
            if action(ui, &painter, &mut right, cy, (label, hint), Id::new(("doc-raw", uid)), (ButtonKind::Secondary, true)) {
                src.raw = !src.raw;
            }
        }
    }
    let title_r = Rect::from_min_max(Pos2::new(sub.min.x + 14.0, sub.min.y), Pos2::new(right - 8.0, sub.max.y - 1.0));
    if title_r.width() > 20.0 {
        let mut title = title_r;
        if let DocState::Source(src) = &tab.state {
            if src.lang != Lang::Plain {
                let g = galley(&painter, src.lang.label(), theme::meta(10.5), theme::pal().text_3);
                let w = g.size().x;
                place(
                    &painter,
                    Rect::from_min_size(Pos2::new(title.max.x - w, title.min.y), Vec2::new(w, title.height())),
                    Align2::LEFT_CENTER,
                    g,
                    theme::pal().text_3,
                );
                title.max.x -= w + 12.0;
            }
        }
        if dirty {
            let g = galley(&painter, "● Não salvo", theme::ui(11.5), theme::pal().warning);
            let w = g.size().x;
            place(
                &painter,
                Rect::from_min_size(Pos2::new(title.max.x - w, title.min.y), Vec2::new(w, title.height())),
                Align2::LEFT_CENTER,
                g,
                theme::pal().warning,
            );
            title.max.x -= w + 10.0;
        }
        let g = galley_trunc(&painter, &tab.subtitle, theme::ui(12.0), theme::pal().text, title.width().max(10.0));
        place(&painter, title, Align2::LEFT_CENTER, g, theme::pal().text);
    }

    let mut body = Rect::from_min_max(Pos2::new(area.min.x, sub.max.y), area.max);
    notices(ui, &mut body, tab);
    if tab.find.open {
        find_bar(ui, &mut body, uid, &mut tab.find);
    }
    let is_md = matches!(tab.kind, DocKind::Markdown(_));
    let diff_path = tab.kind.diff_path().map(str::to_owned);
    let DocTab { state, find, shaped, .. } = tab;
    match state {
        DocState::Loading => note(&painter, body, "Carregando…", theme::pal().text_2),
        DocState::Failed(e) => note(&painter, body, e, theme::pal().danger_text),
        DocState::Unreadable(why) => note(&painter, body, why, theme::pal().text_2),
        DocState::Diff(doc) => diff_body(ui, body, uid, doc, find, acts, diff_path.as_deref()),
        DocState::Text(doc) => code::text_body(ui, body, uid, doc, find),
        DocState::Source(src) if is_md && !src.raw && !src.editing => markdown_body(ui, body, uid, &src.text, cache),
        DocState::Source(src) => code::source_body(ui, body, uid, src, find, shaped),
        DocState::Markdown(text) => markdown_body(ui, body, uid, text, cache),
    }
}

/// Um botão de texto no fim da linha de título; `right` anda para a esquerda. Devolve se foi clicado (um botão
/// desligado aparece apagado e não clica).
fn action(
    ui: &Ui,
    painter: &Painter,
    right: &mut f32,
    cy: f32,
    (label, tooltip): (&str, &str),
    id: Id,
    (kind, enabled): (ButtonKind, bool),
) -> bool {
    let w = button_width(painter, label, kind);
    let r = Rect::from_min_size(Pos2::new(*right - w, cy - 11.0), Vec2::new(w, 22.0));
    *right = r.min.x - 6.0;
    if enabled {
        return tip(button(ui, r, id, label, kind), tooltip).clicked();
    }
    // Desligado: o mesmo desenho a 45%, sem clique.
    painter.rect_filled(r, 5.0, theme::with_alpha(theme::pal().text_strong, 0.03));
    text_in(painter, r, Align2::CENTER_CENTER, label, theme::ui(11.5), theme::with_opacity(theme::pal().text_2, 0.55));
    tip(ui.interact(r, id, Sense::hover()), tooltip);
    false
}

/// Uma mensagem no alto do corpo (carregando, erro, arquivo que não vira texto), com quebra de linha.
pub(super) fn note(painter: &Painter, body: Rect, text: &str, color: egui::Color32) {
    let g = painter.layout(text.to_owned(), theme::ui(12.5), color, (body.width() - 28.0).max(40.0));
    painter.galley(body.min + Vec2::splat(14.0), g, color);
}

// ---------------------------------------------------------------------------
// Avisos e busca
// ---------------------------------------------------------------------------

/// Altura de uma faixa de aviso e da barra de busca.
const NOTICE_H: f32 = 30.0;
const FIND_H: f32 = 34.0;

/// O que um botão de aviso pede.
#[derive(Clone, Copy)]
enum Fix {
    KeepMine,
    TakeDisk,
    Dismiss,
}

/// Uma faixa de aviso no alto de `body` (que encolhe): o texto, na cor do tom, e os botões à direita. Devolve o botão clicado.
fn notice(ui: &Ui, body: &mut Rect, tone: egui::Color32, text: &str, buttons: &[(&str, &str, Fix)], key: Id) -> Option<Fix> {
    let r = Rect::from_min_size(body.min, Vec2::new(body.width(), NOTICE_H));
    body.min.y += NOTICE_H;
    let painter = ui.painter_at(r);
    painter.rect_filled(r, 0.0, theme::with_alpha(tone, 0.10));
    painter.rect_filled(Rect::from_min_max(Pos2::new(r.min.x, r.max.y - 1.0), r.max), 0.0, theme::pal().separator);
    let mut right = r.max.x - 8.0;
    let mut clicked = None;
    for (n, (label, hint, fix)) in buttons.iter().enumerate().rev() {
        let w = button_width(&painter, label, ButtonKind::Secondary);
        let b = Rect::from_min_size(Pos2::new(right - w, r.center().y - 11.0), Vec2::new(w, 22.0));
        right = b.min.x - 6.0;
        if tip(button(ui, b, key.with(n), label, ButtonKind::Secondary), *hint).clicked() {
            clicked = Some(*fix);
        }
    }
    let text_r = Rect::from_min_max(Pos2::new(r.min.x + 14.0, r.min.y), Pos2::new(right - 6.0, r.max.y - 1.0));
    let g = galley_trunc(&painter, text, theme::ui(12.0), theme::pal().text, text_r.width().max(10.0));
    let shown = place(&painter, text_r, Align2::LEFT_CENTER, g, theme::pal().text);
    painter.circle_filled(Pos2::new(shown.min.x - 8.0, r.center().y - 0.5), 3.0, tone);
    tip(ui.interact(text_r, key.with("text"), Sense::hover()), text.to_owned());
    clicked
}

/// Os avisos do arquivo aberto como código (e do que só abriu em leitura), do mais importante ao menos.
fn notices(ui: &Ui, body: &mut Rect, tab: &mut DocTab) {
    let p = theme::pal();
    let key = Id::new(("doc-notice", tab.uid));
    match &mut tab.state {
        DocState::Source(src) => {
            let fix = if src.external.is_some() {
                notice(
                    ui,
                    body,
                    p.warning,
                    "O arquivo mudou no disco (um agente?) enquanto você editava.",
                    &[
                        (
                            "Manter a minha",
                            "Fica com o que você digitou; a próxima gravação põe por cima do que está no disco",
                            Fix::KeepMine,
                        ),
                        ("Recarregar a do disco", "Joga fora o que você digitou e mostra o que está no disco", Fix::TakeDisk),
                    ],
                    key.with("external"),
                )
            } else if src.gone {
                notice(
                    ui,
                    body,
                    p.warning,
                    "O arquivo foi apagado ou não abre mais. O que você editou continua aqui; Ctrl+S o grava de novo.",
                    &[],
                    key.with("gone"),
                )
            } else {
                None
            };
            match fix {
                Some(Fix::KeepMine) => src.keep_mine(),
                Some(Fix::TakeDisk) => src.take_disk(),
                _ => {}
            }
            if let Some(e) = src.save_error.clone() {
                if let Some(Fix::Dismiss) = notice(
                    ui,
                    body,
                    p.danger,
                    &format!("Não consegui gravar: {e}"),
                    &[("Fechar", "Esconde o erro", Fix::Dismiss)],
                    key.with("error"),
                ) {
                    src.save_error = None;
                }
            }
            if let Some(why) = &src.locked {
                notice(ui, body, p.text_2, why, &[], key.with("locked"));
            }
        }
        DocState::Text(_) => {
            let why = "Aberto só para leitura: o arquivo é grande ou tem linha comprida demais para o editor.";
            notice(ui, body, p.text_2, why, &[], key.with("large"));
        }
        _ => {}
    }
}

/// A barra de busca (Ctrl+F): o campo, "n de m", anterior, próxima e fechar. Enter vai para a próxima (Shift+Enter, a anterior); Esc fecha.
fn find_bar(ui: &mut Ui, body: &mut Rect, uid: u64, find: &mut Find) {
    let r = Rect::from_min_size(body.min, Vec2::new(body.width(), FIND_H));
    body.min.y += FIND_H;
    let p = theme::pal();
    let painter = ui.painter_at(r);
    painter.rect_filled(Rect::from_min_max(Pos2::new(r.min.x, r.max.y - 1.0), r.max), 0.0, p.separator);
    let field = Rect::from_min_size(Pos2::new(r.min.x + 12.0, r.min.y + 5.0), Vec2::new(240.0f32.min(r.width() - 24.0), 24.0));
    let id = Id::new(("doc-find", uid));
    let focused = ui.ctx().memory(|m| m.has_focus(id));
    frame(&painter, field, 6.0, p.recess, if focused { theme::accent() } else { p.separator });
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(field.shrink2(Vec2::new(8.0, 3.0))));
    let out = egui::TextEdit::singleline(&mut find.query)
        .id(id)
        .frame(egui::Frame::NONE)
        .font(theme::ui(12.0))
        .hint_text("Buscar no documento")
        .text_color(p.text)
        .desired_width(field.width() - 16.0)
        .show(&mut child);
    if std::mem::take(&mut find.focus) {
        out.response.response.request_focus();
    }
    let (enter, shift, esc) = ui.input(|i| (i.key_pressed(Key::Enter), i.modifiers.shift, i.key_pressed(Key::Escape)));
    if out.response.response.lost_focus() && enter {
        find.step(shift);
        out.response.response.request_focus();
    } else if focused && esc {
        find.close();
        return;
    }
    let n = find.count();
    let counter = match (find.query.is_empty(), n) {
        (true, _) => String::new(),
        (false, 0) => "Nenhum resultado".to_owned(),
        (false, n) => format!("{} de {n}", find.current.min(n - 1) + 1),
    };
    let mut x = field.max.x + 10.0;
    if !counter.is_empty() {
        let w = text_w(&painter, &counter, theme::meta(10.5));
        text_in(
            &painter,
            Rect::from_min_size(Pos2::new(x, r.min.y), Vec2::new(w, r.height() - 1.0)),
            Align2::LEFT_CENTER,
            &counter,
            theme::meta(10.5),
            p.text_2,
        );
        x += w + 10.0;
    }
    for (label, hint, act) in
        [("‹", "Ocorrência anterior (Shift+Enter)", 0), ("›", "Próxima ocorrência (Enter)", 1), ("Fechar", "Fecha a busca (Esc)", 2)]
    {
        let w = if act == 2 { button_width(&painter, label, ButtonKind::Ghost) } else { 24.0 };
        let b = Rect::from_min_size(Pos2::new(x, r.min.y + 5.0), Vec2::new(w, 24.0));
        x += w + 4.0;
        if x > r.max.x {
            break;
        }
        if tip(button(ui, b, Id::new(("doc-find-btn", uid, act)), label, ButtonKind::Ghost), hint).clicked() {
            match act {
                0 => find.step(true),
                1 => find.step(false),
                _ => find.close(),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// A pergunta sobre edição não salva
// ---------------------------------------------------------------------------

/// O modal de "Salvar as alterações?": ao fechar uma aba com edição, ao recarregar sobre ela ou ao fechar o app.
pub fn unsaved_modal(deck: &mut Deck, ctx: &egui::Context) {
    let Some(asked) = deck.docs.confirm else { return };
    let name_of = |uid: u64| deck.docs.get(uid).map_or_else(|| "o arquivo".to_owned(), |t| t.subtitle.clone());
    let (title, detail, buttons): (String, String, Vec<(&str, Answer)>) = match asked {
        Unsaved::Tab(uid) => (
            format!("Salvar as alterações de {}?", name_of(uid)),
            "Se não salvar, o que você digitou se perde.".to_owned(),
            vec![("Salvar", Answer::Save), ("Não salvar", Answer::Discard), ("Cancelar", Answer::Cancel)],
        ),
        Unsaved::Revert(uid) => (
            format!("Descartar as alterações de {}?", name_of(uid)),
            "O arquivo volta a ser lido do disco; o que você digitou se perde.".to_owned(),
            vec![("Descartar", Answer::Discard), ("Cancelar", Answer::Cancel)],
        ),
        Unsaved::Quit => {
            let files: Vec<String> = deck.docs.unsaved().iter().map(|t| t.subtitle.clone()).collect();
            let shown = files.iter().take(4).cloned().collect::<Vec<_>>().join(", ");
            let more = if files.len() > 4 { format!(" e mais {}", files.len() - 4) } else { String::new() };
            (
                "Há alterações não salvas".to_owned(),
                format!("{shown}{more}. Se sair sem salvar, o que você digitou se perde."),
                vec![("Salvar tudo e sair", Answer::Save), ("Sair sem salvar", Answer::Discard), ("Cancelar", Answer::Cancel)],
            )
        }
    };
    super::modal_backdrop(deck, ctx, "docs-unsaved-backdrop");
    let p = theme::pal();
    let w = 420.0;
    let mut answer = None;
    let screen = ctx.content_rect();
    egui::Area::new(Id::new("docs-unsaved"))
        .order(egui::Order::Tooltip)
        .fixed_pos(Pos2::new(screen.center().x - w / 2.0, screen.min.y + 120.0))
        .constrain(false)
        .show(ctx, |ui| {
            let g1 = ui.painter().layout(title.clone(), theme::ui_semibold(13.0), p.text_strong, w - 28.0);
            let g2 = ui.painter().layout(detail.clone(), theme::ui(12.0), p.text_2, w - 28.0);
            let btn_h = 28.0;
            let h = 14.0 + g1.size().y + 6.0 + g2.size().y + 14.0 + btn_h + 14.0;
            let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
            super::shadow(ui.painter(), rect, 10, 8, 24, p.shadow);
            frame(ui.painter(), rect, 10.0, p.popup, p.glass_border);
            ui.painter().galley(Pos2::new(rect.min.x + 14.0, rect.min.y + 14.0), g1.clone(), p.text_strong);
            ui.painter().galley(Pos2::new(rect.min.x + 14.0, rect.min.y + 14.0 + g1.size().y + 6.0), g2, p.text_2);
            let mut right = rect.max.x - 14.0;
            for (n, (label, which)) in buttons.iter().enumerate().rev() {
                let kind = if *which == Answer::Save { ButtonKind::Primary } else { ButtonKind::Secondary };
                let bw = button_width(ui.painter(), label, kind);
                let b = Rect::from_min_size(Pos2::new(right - bw, rect.max.y - 14.0 - btn_h), Vec2::new(bw, btn_h));
                right = b.min.x - 8.0;
                let id = Id::new(("docs-unsaved-btn", n));
                let resp = if *which == Answer::Discard { super::button_danger(ui, b, id, label) } else { button(ui, b, id, label, kind) };
                if resp.clicked() {
                    answer = Some(*which);
                }
            }
            if ui.input(|i| i.key_pressed(Key::Escape)) {
                answer = Some(Answer::Cancel);
            }
        });
    if let Some(a) = answer {
        deck.docs_answer(a);
    }
}

// ---------------------------------------------------------------------------
// Diff
// ---------------------------------------------------------------------------

fn diff_body(ui: &mut Ui, body: Rect, uid: u64, doc: &mut DiffDoc, find: &mut Find, acts: &mut Vec<Act>, path: Option<&str>) {
    let id = Id::new(("doc-diff", uid));
    if find.open {
        let version = doc.diff.lines.len() as u64;
        find.refresh(version, |q| (Vec::new(), diff_view::search(&doc.diff, q)));
    }
    let current = find.current_line();
    let mut jump = doc.jump.take();
    if std::mem::take(&mut find.jump) {
        jump = current.or(jump);
    }
    let query = if find.open { find.query.as_str() } else { "" };
    let opts = diff_view::View { path, query, found: &find.lines, current: current.filter(|_| find.open), jump };
    if doc.files.is_empty() {
        super::git::diff_lines_at(ui, body, &doc.diff, None, &mut Vec::new(), id, opts);
        return;
    }
    let narrow = body.width() < NARROW_W;
    let (list, view) = if narrow {
        let h = (FILE_HEAD_H + FILE_ROW_H * doc.files.len() as f32).min(body.height() * 0.35).max(FILE_HEAD_H + FILE_ROW_H);
        let list = Rect::from_min_size(body.min, Vec2::new(body.width(), h));
        (list, Rect::from_min_max(Pos2::new(body.min.x, list.max.y + 1.0), body.max))
    } else {
        let w = (body.width() * 0.28).clamp(190.0, 320.0);
        let list = Rect::from_min_size(body.min, Vec2::new(w, body.height()));
        (list, Rect::from_min_max(Pos2::new(list.max.x + 1.0, body.min.y), body.max))
    };
    let painter = ui.painter_at(body);
    let sep = if narrow {
        Rect::from_min_size(Pos2::new(list.min.x, list.max.y), Vec2::new(list.width(), 1.0))
    } else {
        Rect::from_min_size(Pos2::new(list.max.x, list.min.y), Vec2::new(1.0, list.height()))
    };
    painter.rect_filled(sep, 0.0, theme::pal().separator);
    file_list(ui, list, uid, doc, acts);
    super::git::diff_lines_at(ui, view, &doc.diff, None, &mut Vec::new(), id, opts);
}

/// A lista de arquivos do diff: letra do que mudou, nome e `+`/`−`. Clicar rola o diff até o arquivo.
fn file_list(ui: &mut Ui, area: Rect, uid: u64, doc: &DiffDoc, acts: &mut Vec<Act>) {
    let painter = ui.painter_at(area);
    let (added, removed) = doc.files.iter().fold((0, 0), |(a, r), f| (a + f.added, r + f.removed));
    let title = format!("{} arquivo(s)", doc.files.len());
    let head = section_label(&painter, Rect::from_min_size(area.min, Vec2::new(area.width(), FILE_HEAD_H)), &title);
    let totals = format!("+{added} −{removed}");
    text_in(&painter, head.shrink2(Vec2::new(12.0, 0.0)), Align2::RIGHT_CENTER, &totals, theme::meta(10.0), theme::pal().text_2);

    let rows = Rect::from_min_max(Pos2::new(area.min.x, head.max.y), area.max);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rows).layout(egui::Layout::top_down(egui::Align::Min)));
    child.set_clip_rect(rows);
    egui::ScrollArea::vertical().id_salt(("doc-files", uid)).auto_shrink([false, false]).show_rows(
        &mut child,
        FILE_ROW_H,
        doc.files.len(),
        |ui, range| {
            for n in range {
                let file = &doc.files[n];
                let (r, _) = ui.allocate_exact_size(Vec2::new(rows.width(), FILE_ROW_H), Sense::hover());
                let resp = clickable(ui, r, Id::new(("doc-file", uid, n)));
                file_row(ui.painter(), r, file, n == doc.selected, resp.hovered());
                if tip(resp, format!("{}\nClique para ir até ele no diff", file.path)).clicked() {
                    acts.push(Act::SelectFile(uid, n));
                }
            }
        },
    );
}

fn file_row(p: &Painter, r: Rect, file: &DiffFile, selected: bool, hovered: bool) {
    let pal = theme::pal();
    if selected {
        p.rect_filled(r.shrink2(Vec2::new(4.0, 0.0)), 4.0, pal.selected_soft);
    } else if hovered {
        p.rect_filled(r.shrink2(Vec2::new(4.0, 0.0)), 4.0, pal.hover);
    }
    let letter_color = match file.status {
        'A' => pal.success,
        'D' => pal.danger_text,
        'R' => pal.info,
        _ => pal.warning,
    };
    let letter = Rect::from_min_size(Pos2::new(r.min.x + 12.0, r.min.y), Vec2::new(12.0, r.height()));
    text_in(p, letter, Align2::CENTER_CENTER, &file.status.to_string(), theme::meta(10.0), letter_color);
    let mut right = r.max.x - 12.0;
    for (text, color) in [(format!("−{}", file.removed), pal.danger_text), (format!("+{}", file.added), pal.success)] {
        if text.len() <= 2 && text.ends_with('0') {
            continue;
        }
        let g = galley(p, &text, theme::meta(10.0), color);
        let w = g.size().x;
        place(p, Rect::from_min_size(Pos2::new(right - w, r.min.y), Vec2::new(w, r.height())), Align2::LEFT_CENTER, g, color);
        right -= w + 8.0;
    }
    let name_x = r.min.x + 32.0;
    let name = file_name(&file.path);
    let color = if selected { pal.text_strong } else { pal.text };
    let g = galley_trunc(p, name, theme::ui(11.5), color, (right - name_x - 6.0).max(20.0));
    place(p, Rect::from_min_max(Pos2::new(name_x, r.min.y), Pos2::new(right, r.max.y)), Align2::LEFT_CENTER, g, color);
}

// ---------------------------------------------------------------------------
// Markdown
// ---------------------------------------------------------------------------

/// O markdown formatado (`egui_commonmark`: títulos, listas, caixas `- [ ]`, tabelas, citações, código),
/// numa coluna centrada de até `MD_MAX_W`.
fn markdown_body(ui: &mut Ui, body: Rect, uid: u64, text: &str, cache: &mut egui_commonmark::CommonMarkCache) {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body).layout(egui::Layout::top_down(egui::Align::Min)));
    child.set_clip_rect(body);
    let side = ((body.width() - MD_MAX_W) / 2.0).max(18.0);
    let col = (body.width() - 2.0 * side).max(40.0);
    egui::ScrollArea::vertical().id_salt(("doc-md", uid)).auto_shrink([false, false]).show(&mut child, |ui| {
        ui.add_space(14.0);
        ui.horizontal_top(|ui| {
            ui.add_space(side);
            ui.allocate_ui_with_layout(Vec2::new(col, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_max_width(col);
                markdown_style(ui);
                egui_commonmark::CommonMarkViewer::new().show(ui, cache, text);
            });
        });
        ui.add_space(24.0);
    });
}

/// O estilo do texto do markdown: o egui zera o espaçamento do app, e o `egui_commonmark` precisa dele.
fn markdown_style(ui: &mut Ui) {
    let style = ui.style_mut();
    style.spacing.item_spacing = Vec2::new(6.0, 8.0);
    style.text_styles.insert(egui::TextStyle::Body, theme::ui(13.5));
    style.text_styles.insert(egui::TextStyle::Heading, theme::ui_semibold(21.0));
    style.text_styles.insert(egui::TextStyle::Monospace, theme::mono(12.5));
    style.text_styles.insert(egui::TextStyle::Small, theme::ui(11.5));
    style.visuals.override_text_color = Some(theme::pal().text);
    style.visuals.code_bg_color = theme::pal().hover;
    style.visuals.faint_bg_color = theme::pal().chip;
    style.visuals.extreme_bg_color = theme::pal().hover;
    style.visuals.hyperlink_color = theme::pal().info;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_documents_word_shrinks_in_a_narrow_segment() {
        // "Documentos 13" precisa de ~80 px mais folga.
        assert_eq!(docs_word(80.0, 100.0), "Documentos");
        assert_eq!(docs_word(80.0, 88.0), "Documentos");
        assert_eq!(docs_word(80.0, 87.0), "Docs");
        assert_eq!(docs_word(80.0, 60.0), "Docs");
    }
}
