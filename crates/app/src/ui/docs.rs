//! Modo Documentos do centro (ver `docs.rs`): a chave Agentes | Documentos da barra de título, a faixa de
//! abas de documento (o mesmo desenho de `tabs.rs`) e o documento com a largura toda do centro.
//! Diff, arquivo de texto e markdown; tudo só para leitura.

use egui::{Align2, Id, Painter, Pos2, Rect, Sense, Ui, Vec2};

use super::panes::HEAD_H;
use super::tabs::{self, Item, Lead, StripClick, StripKind};
use super::{clickable, frame, galley, galley_trunc, place, text_in, text_w, tip};
use crate::deck::Deck;
use crate::docs::{file_name, CenterView, DiffDoc, DiffFile, DocKind, DocState, DocTab, TextDoc};
use crate::theme;

/// Altura da linha com o título do documento e as ações dele.
const SUBHEAD_H: f32 = 28.0;
/// Altura de uma linha da lista de arquivos de um diff e do cabeçalho dela.
const FILE_ROW_H: f32 = 24.0;
const FILE_HEAD_H: f32 = 26.0;
/// Abaixo desta largura a lista de arquivos do diff passa para cima do diff.
const NARROW_W: f32 = 560.0;
/// Altura de uma linha do arquivo de texto e o tamanho da fonte (os mesmos do diff).
const TEXT_ROW_H: f32 = 17.0;
const TEXT_FONT: f32 = 12.0;
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

const SWITCH_H: f32 = 24.0;
const SEG_PAD: f32 = 12.0;

/// Pinta a chave **Agentes | Documentos (n)** com o canto esquerdo em `origin` (`bar_h` é a altura da
/// barra onde ela fica) e devolve o retângulo que ocupou. O selo de "Agentes" mostra o que acontece nas
/// abas enquanto se lê: quantas terminaram um turno sem ser vistas e, lendo documentos, se alguma
/// trabalha agora. Um clique troca o centro.
pub fn switch(deck: &mut Deck, ui: &Ui, painter: &Painter, origin: Pos2, bar_h: f32) -> Rect {
    let n_docs = deck.docs.count(&deck.active_project);
    let badge = deck.agents_badge();
    let view = deck.docs.view;
    let font = theme::ui_semibold(12.0);
    let agents_label = "Agentes";
    let docs_label = format!("Documentos ({n_docs})");
    // O selo de "Agentes": um número sobre fundo de destaque; com agentes só trabalhando, um ponto que pulsa.
    let pill_text = (badge.done > 0).then(|| if badge.done > 99 { "99+".to_owned() } else { badge.done.to_string() });
    let pill_w = pill_text.as_ref().map_or(0.0, |t| (text_w(painter, t, theme::ui(10.0)) + 8.0).max(14.0));
    let pulse = badge.done == 0 && badge.busy > 0 && view == CenterView::Docs;
    let selo_w = if pill_text.is_some() {
        pill_w + 6.0
    } else if pulse {
        14.0
    } else {
        0.0
    };
    let agents_w = text_w(painter, agents_label, font.clone()) + selo_w + 2.0 * SEG_PAD;
    let docs_w = text_w(painter, &docs_label, font.clone()) + 2.0 * SEG_PAD;

    let outer = Rect::from_min_size(Pos2::new(origin.x, origin.y + (bar_h - SWITCH_H) / 2.0), Vec2::new(agents_w + docs_w + 4.0, SWITCH_H));
    frame(painter, outer, 6.0, theme::pal().recess, theme::pal().glass_border);
    let inner = outer.shrink(2.0);
    let agents_r = Rect::from_min_size(inner.min, Vec2::new(agents_w, inner.height()));
    let docs_r = Rect::from_min_size(Pos2::new(agents_r.max.x, inner.min.y), Vec2::new(docs_w, inner.height()));

    let docs_enabled = n_docs > 0;
    let mut picked = None;
    let segments = [
        (agents_r, Switched::Agents, view == CenterView::Agents, true, "center-agents"),
        (docs_r, Switched::Docs, view == CenterView::Docs, docs_enabled, "center-docs"),
    ];
    for (r, which, active, enabled, id) in segments {
        let resp = if enabled { clickable(ui, r, Id::new(id)) } else { ui.interact(r, Id::new(id), Sense::hover()) };
        if active {
            painter.rect_filled(r, 4.0, theme::pal().separator);
        } else if resp.hovered() && enabled {
            painter.rect_filled(r, 4.0, theme::pal().hover);
        }
        let color = match (active, enabled, resp.hovered()) {
            (true, ..) => theme::pal().text,
            (false, true, true) => theme::pal().text,
            (false, true, false) => theme::pal().text_2,
            (false, false, _) => theme::with_opacity(theme::pal().text_2, 0.5),
        };
        let text_r = if which == Switched::Agents {
            Rect::from_min_max(Pos2::new(r.min.x + SEG_PAD, r.min.y), Pos2::new(r.max.x - SEG_PAD - selo_w, r.max.y))
        } else {
            r
        };
        let (label, align) =
            if which == Switched::Agents { (agents_label, Align2::LEFT_CENTER) } else { (docs_label.as_str(), Align2::CENTER_CENTER) };
        text_in(painter, text_r, align, label, font.clone(), color);
        if which == Switched::Agents {
            let x = r.max.x - SEG_PAD - selo_w + 6.0;
            if let Some(t) = &pill_text {
                let pill = Rect::from_min_size(Pos2::new(x, r.center().y - 7.0), Vec2::new(pill_w, 14.0));
                painter.rect_filled(pill, 7.0, theme::accent());
                text_in(painter, pill, Align2::CENTER_CENTER, t, theme::ui(10.0), theme::pal().on_accent);
            } else if pulse {
                let t = ui.ctx().input(|i| i.time);
                let wave = (t * 4.8).sin() as f32 * 0.5 + 0.5;
                let c = Pos2::new(x + 4.0, r.center().y);
                painter.circle_filled(c, 4.0 + 2.8 * wave, theme::with_opacity(deck.accent(), 0.35 * (1.0 - wave)));
                painter.circle_filled(c, 3.2 + wave, theme::with_opacity(deck.accent(), 0.75 + 0.25 * wave));
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
            }
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
    outer
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
            model: None,
            color: accent,
            alive: true,
            busy: false,
            done: false,
            mic: crate::dictation::ButtonVisual::Idle,
            lead: Lead::Glyph(glyph(&t.kind)),
            tip: t.subtitle.clone(),
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
    match tabs::strip_items(ui, strip_rect, &items, current, true, Id::new("doc-tabs"), StripKind { scope: "doc-tab", tab_menu: false }) {
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
            Act::Close(uid) => deck.docs_close(uid),
            Act::Reload(uid) => deck.docs_reload(uid),
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

/// A linha de título do documento (com as ações) e o corpo dele.
fn document(ui: &mut Ui, area: Rect, tab: &mut DocTab, cache: &mut egui_commonmark::CommonMarkCache, acts: &mut Vec<Act>) {
    let painter = ui.painter_at(area);
    let sub = Rect::from_min_size(area.min, Vec2::new(area.width(), SUBHEAD_H));
    painter.rect_filled(Rect::from_min_max(Pos2::new(sub.min.x, sub.max.y - 1.0), sub.max), 0.0, theme::pal().separator);

    // Ações à direita, da última para a primeira.
    let mut right = sub.max.x - 8.0;
    let cy = sub.min.y + (SUBHEAD_H - 1.0) / 2.0;
    if let Some(path) = tab.kind.file_path() {
        let label = "Abrir no programa padrão";
        if action_button(
            ui,
            &painter,
            &mut right,
            cy,
            label,
            "Abre o arquivo no programa que o Windows associou a ele",
            Id::new(("doc-open", tab.uid)),
        ) {
            acts.push(Act::OpenExternal(path.to_owned()));
        }
    }
    if tab.kind.reloadable() {
        let loading = matches!(tab.state, DocState::Loading);
        if action_button(ui, &painter, &mut right, cy, "Recarregar", "Lê o documento de novo", Id::new(("doc-reload", tab.uid))) && !loading
        {
            acts.push(Act::Reload(tab.uid));
        }
    }
    let title_r = Rect::from_min_max(Pos2::new(sub.min.x + 14.0, sub.min.y), Pos2::new(right - 8.0, sub.max.y - 1.0));
    if title_r.width() > 20.0 {
        let g = galley_trunc(&painter, &tab.subtitle, theme::ui(12.0), theme::pal().text, title_r.width());
        place(&painter, title_r, Align2::LEFT_CENTER, g, theme::pal().text);
    }

    let body = Rect::from_min_max(Pos2::new(area.min.x, sub.max.y), area.max);
    let uid = tab.uid;
    match &mut tab.state {
        DocState::Loading => note(&painter, body, "Carregando…", theme::pal().text_2),
        DocState::Failed(e) => note(&painter, body, e, theme::pal().danger_text),
        DocState::Unreadable(why) => note(&painter, body, why, theme::pal().text_2),
        DocState::Diff(doc) => diff_body(ui, body, uid, doc, acts),
        DocState::Text(doc) => text_body(ui, body, uid, doc),
        DocState::Markdown(text) => markdown_body(ui, body, uid, text, cache),
    }
}

/// Um botão de texto no fim da linha de título; `right` anda para a esquerda. Devolve se foi clicado.
fn action_button(ui: &Ui, painter: &Painter, right: &mut f32, cy: f32, label: &str, tooltip: &str, id: Id) -> bool {
    let g = galley(painter, label, theme::ui(11.5), theme::pal().text);
    let r = Rect::from_min_size(Pos2::new(*right - g.size().x - 16.0, cy - 10.0), Vec2::new(g.size().x + 16.0, 20.0));
    *right = r.min.x - 6.0;
    let resp = clickable(ui, r, id);
    painter.rect_filled(r, 4.0, if resp.hovered() { theme::pal().separator } else { theme::pal().hover });
    place(painter, r, Align2::CENTER_CENTER, g, theme::pal().text);
    tip(resp, tooltip).clicked()
}

/// Uma mensagem no alto do corpo (carregando, erro, arquivo que não vira texto), com quebra de linha.
fn note(painter: &Painter, body: Rect, text: &str, color: egui::Color32) {
    let g = painter.layout(text.to_owned(), theme::ui(12.5), color, (body.width() - 28.0).max(40.0));
    painter.galley(body.min + Vec2::splat(14.0), g, color);
}

// ---------------------------------------------------------------------------
// Diff
// ---------------------------------------------------------------------------

fn diff_body(ui: &mut Ui, body: Rect, uid: u64, doc: &mut DiffDoc, acts: &mut Vec<Act>) {
    let id = Id::new(("doc-diff", uid));
    if doc.files.is_empty() {
        let jump = doc.jump.take();
        super::git::diff_lines_at(ui, body, &doc.diff, None, &mut Vec::new(), id, jump);
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
    painter.rect_filled(list, 0.0, theme::pal().chip);
    let sep = if narrow {
        Rect::from_min_size(Pos2::new(list.min.x, list.max.y), Vec2::new(list.width(), 1.0))
    } else {
        Rect::from_min_size(Pos2::new(list.max.x, list.min.y), Vec2::new(1.0, list.height()))
    };
    painter.rect_filled(sep, 0.0, theme::pal().separator);
    file_list(ui, list, uid, doc, acts);
    let jump = doc.jump.take();
    super::git::diff_lines_at(ui, view, &doc.diff, None, &mut Vec::new(), id, jump);
}

/// A lista de arquivos do diff: letra do que mudou, nome e `+`/`−`. Clicar rola o diff até o arquivo.
fn file_list(ui: &mut Ui, area: Rect, uid: u64, doc: &DiffDoc, acts: &mut Vec<Act>) {
    let painter = ui.painter_at(area);
    let (added, removed) = doc.files.iter().fold((0, 0), |(a, r), f| (a + f.added, r + f.removed));
    let title = format!("{} ARQUIVO(S)", doc.files.len());
    let head = Rect::from_min_size(area.min, Vec2::new(area.width(), FILE_HEAD_H));
    text_in(&painter, head.shrink2(Vec2::new(12.0, 0.0)), Align2::LEFT_CENTER, &title, theme::ui_bold(10.5), theme::pal().text_2);
    let totals = format!("+{added} −{removed}");
    text_in(&painter, head.shrink2(Vec2::new(12.0, 0.0)), Align2::RIGHT_CENTER, &totals, theme::mono(11.0), theme::pal().text_2);

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
    if selected {
        p.rect_filled(r.shrink2(Vec2::new(4.0, 1.0)), 4.0, theme::pal().separator);
    } else if hovered {
        p.rect_filled(r.shrink2(Vec2::new(4.0, 1.0)), 4.0, theme::pal().hover);
    }
    let letter_color = match file.status {
        'A' => theme::BADGE_JSON,
        'D' => theme::pal().danger_text,
        'R' => theme::pal().info,
        _ => theme::OLLAMA,
    };
    let letter = Rect::from_min_size(Pos2::new(r.min.x + 12.0, r.min.y), Vec2::new(12.0, r.height()));
    text_in(p, letter, Align2::CENTER_CENTER, &file.status.to_string(), theme::mono_bold(11.5), letter_color);
    let mut right = r.max.x - 12.0;
    for (text, color) in [(format!("−{}", file.removed), theme::pal().danger_text), (format!("+{}", file.added), theme::BADGE_JSON)] {
        if text.len() <= 2 && text.ends_with('0') {
            continue;
        }
        let g = galley(p, &text, theme::mono(11.0), color);
        let w = g.size().x;
        place(p, Rect::from_min_size(Pos2::new(right - w, r.min.y), Vec2::new(w, r.height())), Align2::LEFT_CENTER, g, color);
        right -= w + 8.0;
    }
    let name_x = r.min.x + 32.0;
    let name = file_name(&file.path);
    let font = if selected { theme::ui_semibold(12.0) } else { theme::ui(12.0) };
    let g = galley_trunc(p, name, font, theme::pal().text, (right - name_x - 6.0).max(20.0));
    place(p, Rect::from_min_max(Pos2::new(name_x, r.min.y), Pos2::new(right, r.max.y)), Align2::LEFT_CENTER, g, theme::pal().text);
}

// ---------------------------------------------------------------------------
// Arquivo de texto
// ---------------------------------------------------------------------------

/// O arquivo em monoespaçado, com número de linha, rolagem nos dois sentidos e só para leitura.
fn text_body(ui: &mut Ui, body: Rect, uid: u64, doc: &TextDoc) {
    let font = theme::mono(TEXT_FONT);
    let cell = ui.fonts_mut(|f| f.glyph_width(&font, 'M')).max(1.0);
    let digits = doc.lines.len().max(1).to_string().len().max(3);
    let gutter = cell * digits as f32 + 22.0;
    let content_w = (gutter + doc.max_chars as f32 * cell + 24.0).max(body.width());
    if doc.lines.is_empty() {
        note(&ui.painter_at(body), body, "O arquivo está vazio.", theme::pal().text_2);
        return;
    }
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body).layout(egui::Layout::top_down(egui::Align::Min)));
    child.set_clip_rect(body);
    egui::ScrollArea::both().id_salt(("doc-text", uid)).auto_shrink([false, false]).show_rows(
        &mut child,
        TEXT_ROW_H,
        doc.lines.len(),
        |ui, range| {
            for n in range {
                let (r, _) = ui.allocate_exact_size(Vec2::new(content_w, TEXT_ROW_H), Sense::hover());
                let p = ui.painter();
                p.text(
                    Pos2::new(r.min.x + gutter - 12.0, r.center().y),
                    Align2::RIGHT_CENTER,
                    (n + 1).to_string(),
                    font.clone(),
                    theme::with_opacity(theme::pal().text_2, 0.7),
                );
                p.text(Pos2::new(r.min.x + gutter, r.center().y), Align2::LEFT_CENTER, &doc.lines[n], font.clone(), theme::pal().text_2);
            }
        },
    );
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
