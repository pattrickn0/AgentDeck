//! Menus suspensos: nova aba (+) e configurações (☰).

use egui::{Align2, Area, Color32, Id, Key, Order, Pos2, Rect, Sense, Stroke, Vec2};

use agentdeck_core::models;

use super::{button, button_danger, button_width, dot, frame, section_label, separator, shadow, text_in, text_w, ButtonKind};
use crate::deck::Deck;
use crate::dictation::Setup;
use crate::theme;

/// Posição do canto superior esquerdo para um menu `size` não sair da tela (pelos lados e por
/// baixo: ancorado embaixo na barra lateral, um menu comprido sobe).
/// (Não usar `Area::constrain`: ele desloca o menu depois que `closes_on_outside` já calculou o
/// retângulo, e o clique num item passa a contar como "fora", fechando o menu antes de escolher.)
pub(super) fn keep_on_screen(ctx: &egui::Context, pos: Pos2, size: Vec2) -> Pos2 {
    let screen = ctx.content_rect();
    Pos2::new(
        pos.x.min(screen.max.x - size.x - 4.0).max(screen.min.x + 4.0),
        pos.y.min(screen.max.y - size.y - 4.0).max(screen.min.y + 4.0),
    )
}

/// Fecha o menu se houve clique fora dele e da âncora (`document.addEventListener('click')`).
pub(super) fn closes_on_outside(ctx: &egui::Context, menu: Rect, anchor: Rect) -> bool {
    ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !menu.contains(p) && !anchor.contains(p)))
}

/// O agente deixa escolher o modelo no menu de novo agente (o Codex não tem lista para mostrar).
pub fn picks_model(kind: &str) -> bool {
    matches!(kind, "claude" | "antigravity" | "opencode")
}

/// Aviso no lugar da lista quando não há modelos para mostrar.
fn models_status(loading: bool, empty: bool) -> Option<&'static str> {
    match (loading, empty) {
        (true, true) => Some("Carregando modelos…"),
        (false, true) => Some("Sem lista de modelos: só o padrão"),
        _ => None,
    }
}

/// Altura máxima da lista de modelos (a do OpenCode tem dezenas).
const MODELS_MAX_H: f32 = 300.0;
/// Largura da seta "›" que abre a escolha de modelo na linha do agente.
const CHEVRON_W: f32 = 28.0;

/// `#new-tab-menu`: os agentes; nos que escolhem modelo, a seta à direita abre o segundo passo
/// (padrão + modelos conhecidos). Clicar no nome abre o agente com o modelo padrão.
pub fn new_tab_menu(deck: &mut Deck, ctx: &egui::Context, anchor: Rect) {
    if !deck.new_tab_menu_open {
        deck.new_tab_models_for = None;
        deck.new_tab_provider = None;
        return;
    }
    if let Some(kind) = deck.new_tab_models_for.clone() {
        return models_step(deck, ctx, anchor, &kind);
    }
    let items = deck.agents.new_tab_menu.clone();
    let items = if items.is_empty() {
        // Antes da primeira listagem: só o terminal (fallback do renderer).
        vec![crate::agents::MenuItem { kind: "shell".into(), label: crate::agents::shell_label(None), color: theme::SHELL }]
    } else {
        items
    };
    let with_arrow: Vec<bool> =
        items.iter().map(|m| picks_model(&m.kind) && deck.agents.available.iter().any(|a| a.id == m.kind && a.installed)).collect();
    let font = theme::ui(12.0);
    let painter = ctx.layer_painter(egui::LayerId::new(Order::Foreground, Id::new("measure")));
    let content_w = items
        .iter()
        .zip(&with_arrow)
        .map(|(m, &arrow)| 8.0 + 8.0 + text_w(&painter, &m.label, font.clone()) + 20.0 + if arrow { CHEVRON_W } else { 0.0 })
        .fold(0.0, f32::max);
    let w = (content_w + 8.0 + 2.0).max(180.0);
    let item_h = 26.0;
    let h = items.len() as f32 * item_h + 8.0;
    let pos = keep_on_screen(ctx, Pos2::new(anchor.min.x, anchor.max.y), Vec2::new(w, h));
    let menu_rect = Rect::from_min_size(pos, Vec2::new(w, h));
    let mut chosen = None;
    let mut pick_model_of = None;
    Area::new(Id::new("new-tab-menu")).order(Order::Foreground).fixed_pos(pos).constrain(false).show(ctx, |ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
        let p = ui.painter();
        let pal = theme::pal();
        shadow(p, rect, 8, 8, 24, pal.shadow);
        frame(p, rect, 8.0, pal.popup, pal.glass_border);
        let mut y = rect.min.y + 4.0;
        for (m, &arrow) in items.iter().zip(&with_arrow) {
            let r = Rect::from_min_size(Pos2::new(rect.min.x + 4.0, y), Vec2::new(w - 8.0, item_h));
            y += item_h;
            let name_r = if arrow { Rect::from_min_max(r.min, Pos2::new(r.max.x - CHEVRON_W, r.max.y)) } else { r };
            let resp = super::clickable(ui, name_r, Id::new(("new-tab-item", &m.kind)));
            let arrow_resp = arrow.then(|| {
                let ar = Rect::from_min_max(Pos2::new(r.max.x - CHEVRON_W, r.min.y), r.max);
                super::clickable(ui, ar, Id::new(("new-tab-models", &m.kind)))
            });
            let arrow_hot = arrow_resp.as_ref().is_some_and(|a| a.hovered());
            if resp.hovered() || arrow_hot {
                p.rect_filled(r, 5.0, pal.hover);
            }
            super::projects::paint_kind_mark(&mut deck.icons, p, Pos2::new(r.min.x + 10.0 + 4.0, r.center().y), &m.kind, m.color);
            text_in(
                p,
                Rect::from_min_max(Pos2::new(r.min.x + 10.0 + 16.0, r.min.y), r.max),
                Align2::LEFT_CENTER,
                &m.label,
                font.clone(),
                pal.text,
            );
            if let Some(a) = arrow_resp {
                let ar = Rect::from_min_max(Pos2::new(r.max.x - CHEVRON_W, r.min.y), r.max);
                text_in(p, ar, Align2::CENTER_CENTER, "›", theme::ui(14.0), if arrow_hot { pal.text } else { pal.text_2 });
                if super::tip(a, format!("Escolher o modelo de {}", m.label)).clicked() {
                    pick_model_of = Some(m.kind.clone());
                }
            }
            if resp.clicked() {
                chosen = Some(m.kind.clone());
            }
        }
    });
    if let Some(kind) = pick_model_of {
        deck.request_models(&kind);
        deck.new_tab_models_for = Some(kind);
        deck.new_tab_provider = None;
    } else if let Some(kind) = chosen {
        deck.new_tab_menu_open = false;
        deck.new_tab_provider = None;
        deck.open_tab(&kind, false);
    } else if closes_on_outside(ctx, menu_rect, anchor) {
        deck.new_tab_menu_open = false;
        deck.new_tab_provider = None;
    }
}

/// Uma linha de um passo do menu de modelo.
struct StepRow {
    label: String,
    /// Modelo sem custo por token ("grátis", em verde).
    free: bool,
    /// Quantos modelos o provedor tem ("N modelos", em cinza); 0 = nada.
    count: usize,
    /// Abre o passo seguinte ("›").
    submenu: bool,
    enabled: bool,
}

impl StepRow {
    fn new(label: impl Into<String>) -> Self {
        Self { label: label.into(), free: false, count: 0, submenu: false, enabled: true }
    }

    /// O texto discreto à direita e a cor.
    fn detail(&self) -> Option<(String, Color32)> {
        if self.count > 0 {
            Some((format!("{} modelo{}", self.count, if self.count == 1 { "" } else { "s" }), theme::pal().text_2))
        } else if self.free {
            Some(("grátis".to_owned(), theme::pal().success))
        } else {
            None
        }
    }
}

/// O que aconteceu num passo do menu: escolheu uma linha, voltou ao passo anterior, fechou tudo ou
/// nada (o menu continua aberto).
enum StepOutcome {
    Chosen(usize),
    Back,
    Closed,
    Pending,
}

/// Um passo do menu: o cabeçalho "‹ título" volta ao passo anterior e as linhas são as escolhas.
fn step_menu(ctx: &egui::Context, anchor: Rect, id: &str, color: Color32, title: &str, rows: &[StepRow]) -> StepOutcome {
    let font = theme::ui(12.0);
    let painter = ctx.layer_painter(egui::LayerId::new(Order::Foreground, Id::new("measure")));
    let widest = rows
        .iter()
        .map(|row| {
            let detail = row.detail().map_or(0.0, |(text, _)| 8.0 + text_w(&painter, &text, theme::ui(11.0)));
            text_w(&painter, &row.label, font.clone()) + detail + if row.submenu { 16.0 } else { 0.0 }
        })
        .fold(text_w(&painter, title, font.clone()) + 54.0, f32::max);
    let w = (widest + 8.0 + 2.0 + 10.0 * 2.0).clamp(200.0, 380.0);
    let row_h = 26.0;
    let head_h = 26.0;
    let list_h = (rows.len() as f32 * row_h).min(MODELS_MAX_H);
    let h = 4.0 + head_h + 1.0 + list_h + 4.0;
    let pos = keep_on_screen(ctx, Pos2::new(anchor.min.x, anchor.max.y), Vec2::new(w, h));
    let menu_rect = Rect::from_min_size(pos, Vec2::new(w, h));
    let mut back = false;
    let mut chosen = None;
    let pal = theme::pal();
    Area::new(Id::new(id)).order(Order::Foreground).fixed_pos(pos).constrain(false).show(ctx, |ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
        shadow(ui.painter(), rect, 8, 8, 24, pal.shadow);
        frame(ui.painter(), rect, 8.0, pal.popup, pal.glass_border);
        // Cabeçalho: "‹ título" volta um passo.
        let head = Rect::from_min_size(Pos2::new(rect.min.x + 4.0, rect.min.y + 4.0), Vec2::new(w - 8.0, head_h));
        let resp = super::clickable(ui, head, Id::new((id, "back")));
        if resp.hovered() {
            ui.painter().rect_filled(head, 5.0, pal.hover);
        }
        text_in(
            ui.painter(),
            Rect::from_min_size(head.min, Vec2::new(22.0, head_h)),
            Align2::CENTER_CENTER,
            "‹",
            theme::ui(14.0),
            pal.text_2,
        );
        dot(ui.painter(), Pos2::new(head.min.x + 26.0, head.center().y), color);
        text_in(
            ui.painter(),
            Rect::from_min_max(Pos2::new(head.min.x + 36.0, head.min.y), head.max),
            Align2::LEFT_CENTER,
            title,
            theme::ui_semibold(12.0),
            pal.text_strong,
        );
        if super::tip(resp, "Voltar").clicked() {
            back = true;
        }
        separator(ui.painter(), Pos2::new(rect.min.x + 8.0, head.max.y + 0.5), Pos2::new(rect.max.x - 8.0, head.max.y + 0.5));
        let list = Rect::from_min_size(Pos2::new(rect.min.x + 4.0, head.max.y + 1.0), Vec2::new(w - 8.0, list_h));
        let mut lc = ui.new_child(egui::UiBuilder::new().max_rect(list));
        lc.set_clip_rect(list);
        egui::ScrollArea::vertical().id_salt((id, "list")).auto_shrink([false, false]).show(&mut lc, |ui| {
            for (i, row) in rows.iter().enumerate() {
                let (r, _) = ui.allocate_exact_size(Vec2::new(list.width(), row_h), Sense::hover());
                let resp =
                    if row.enabled { super::clickable(ui, r, Id::new((id, i))) } else { ui.interact(r, Id::new((id, i)), Sense::hover()) };
                if row.enabled && resp.hovered() {
                    ui.painter().rect_filled(r, 5.0, pal.hover);
                }
                let text_color = if row.enabled { pal.text } else { pal.text_3 };
                let mut right = r.max.x - 10.0;
                if row.submenu {
                    text_in(
                        ui.painter(),
                        Rect::from_min_max(Pos2::new(right - 12.0, r.min.y), Pos2::new(right, r.max.y)),
                        Align2::CENTER_CENTER,
                        "›",
                        theme::ui(13.0),
                        pal.text_2,
                    );
                    right -= 16.0;
                }
                if let Some((text, dcolor)) = row.detail() {
                    let dw = text_w(ui.painter(), &text, theme::ui(11.0));
                    let dr = Rect::from_min_max(Pos2::new(right - dw, r.min.y), Pos2::new(right, r.max.y));
                    text_in(ui.painter(), dr, Align2::RIGHT_CENTER, &text, theme::ui(11.0), dcolor);
                    right -= dw + 8.0;
                }
                let g = super::galley_trunc(ui.painter(), &row.label, font.clone(), text_color, (right - r.min.x - 10.0).max(10.0));
                super::place(
                    ui.painter(),
                    Rect::from_min_max(Pos2::new(r.min.x + 10.0, r.min.y), Pos2::new(right, r.max.y)),
                    Align2::LEFT_CENTER,
                    g,
                    text_color,
                );
                if row.enabled && resp.clicked() {
                    chosen = Some(i);
                }
            }
        });
    });
    if let Some(i) = chosen {
        return StepOutcome::Chosen(i);
    }
    if back {
        return StepOutcome::Back;
    }
    if closes_on_outside(ctx, menu_rect, anchor) || ctx.input(|i| i.key_pressed(Key::Escape)) {
        return StepOutcome::Closed;
    }
    StepOutcome::Pending
}

/// Segundo passo do menu de novo agente: "Padrão" e os modelos conhecidos de `kind`. Quando o CLI
/// mistura provedores (`opencode models`), o primeiro passo é o provedor e o segundo, os modelos dele;
/// com um provedor só, os modelos aparecem direto.
fn models_step(deck: &mut Deck, ctx: &egui::Context, anchor: Rect, kind: &str) {
    // A lista chega de uma thread: pede de novo se ainda não veio (ou venceu).
    deck.request_models(kind);
    let (models, loading) = deck.models_of(kind);
    let models: Vec<String> = models.to_vec();
    let groups = models::by_provider(&models);
    let name = deck.agents.name(kind);
    let color = deck.agents.tab_color(kind);
    let close_all = |deck: &mut Deck| {
        deck.new_tab_menu_open = false;
        deck.new_tab_models_for = None;
        deck.new_tab_provider = None;
    };

    // Claude e Antigravity: a lista de sempre ("Padrão" + modelos).
    if groups.is_empty() {
        let flat = models::free_first(&models);
        let mut rows = vec![StepRow::new("Padrão")];
        rows.extend(flat.iter().map(|m| StepRow { free: models::is_free(m), ..StepRow::new(m.as_str()) }));
        if let Some(text) = models_status(loading, models.is_empty()) {
            rows.push(StepRow { enabled: false, ..StepRow::new(text) });
        }
        match step_menu(ctx, anchor, "new-tab-models", color, &format!("{name} · modelo"), &rows) {
            StepOutcome::Chosen(0) => {
                close_all(deck);
                deck.open_tab_with_model(kind, None, false);
            }
            StepOutcome::Chosen(i) => {
                let model = flat[i - 1].clone();
                close_all(deck);
                deck.open_tab_with_model(kind, Some(&model), false);
            }
            StepOutcome::Back => deck.new_tab_models_for = None,
            StepOutcome::Closed => close_all(deck),
            StepOutcome::Pending => {}
        }
        return;
    }

    // OpenCode: o provedor escolhido fica no estado do menu; sem ele, o passo é a lista de provedores.
    // Com um provedor só não há o que escolher: os modelos dele aparecem direto (sem "‹ Provedores").
    let sole = models::sole_provider(&groups).map(str::to_owned);
    let chosen = deck.new_tab_provider.clone().filter(|p| groups.iter().any(|(g, _)| g == p)).or(sole.clone());
    if let Some(provider) = chosen {
        let list = groups.iter().find(|(g, _)| *g == provider).map(|(_, l)| l.as_slice()).unwrap_or_default();
        let rows: Vec<StepRow> =
            list.iter().map(|m| StepRow { free: models::is_free(m), ..StepRow::new(models::short_model(m)) }).collect();
        match step_menu(ctx, anchor, "new-tab-provider-models", color, &provider, &rows) {
            StepOutcome::Chosen(i) => {
                let model = list[i].clone();
                close_all(deck);
                deck.open_tab_with_model(kind, Some(&model), false);
            }
            StepOutcome::Back => {
                if sole.is_some() {
                    // Não houve passo dos provedores: o cabeçalho volta à lista de agentes.
                    deck.new_tab_models_for = None;
                } else {
                    deck.new_tab_provider = None;
                }
            }
            StepOutcome::Closed => close_all(deck),
            StepOutcome::Pending => {}
        }
        return;
    }
    let rows: Vec<StepRow> =
        groups.iter().map(|(provider, list)| StepRow { count: list.len(), submenu: true, ..StepRow::new(provider.as_str()) }).collect();
    match step_menu(ctx, anchor, "new-tab-providers", color, &format!("{name} · provedor"), &rows) {
        StepOutcome::Chosen(i) => deck.new_tab_provider = Some(groups[i].0.clone()),
        StepOutcome::Back => deck.new_tab_models_for = None,
        StepOutcome::Closed => close_all(deck),
        StepOutcome::Pending => {}
    }
}

/// `#settings-menu`
pub fn settings_menu(deck: &mut Deck, ctx: &egui::Context, anchor: Rect) {
    if !deck.settings_open {
        // A confirmação de remover o ditado vale só enquanto o menu está aberto.
        deck.dictation_confirm_remove = false;
        return;
    }
    let w = 260.0;
    let title_h = super::SECTION_LABEL_H;
    let item_h = 26.0;
    let btn_h = 28.0;
    // O ditado por voz é opcional: uma linha de estado e um botão para baixar ou remover.
    let h = 6.0 + title_h + 2.0 + item_h * 3.0 + 13.0 + btn_h + 13.0 + item_h + btn_h + 8.0;
    // O botão ☰ fica no canto esquerdo da barra de título: o menu abre alinhado a ele.
    let pos = keep_on_screen(ctx, Pos2::new(anchor.min.x + 4.0, anchor.max.y), Vec2::new(w, h));
    let menu_rect = Rect::from_min_size(pos, Vec2::new(w, h));
    let mut toggle = false;
    let mut toggle_tabs = false;
    let mut toggle_keys = false;
    let mut test = false;
    let mut dictation_click = false;
    let setup = deck.dictation_setup();
    let confirm_remove = deck.dictation_confirm_remove;
    let sound = deck.sound_enabled;
    let auto_tabs = deck.auto_open_agent_tabs;
    let app_keys = deck.app_shortcuts_first;
    let pal = theme::pal();
    Area::new(Id::new("settings-menu")).order(Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
        let p = ui.painter();
        shadow(p, rect, 8, 8, 24, pal.shadow);
        frame(p, rect, 8.0, pal.popup, pal.glass_border);
        let inner = rect.shrink2(Vec2::new(6.0, 6.0));
        section_label(p, Rect::from_min_size(Pos2::new(rect.min.x, inner.min.y), Vec2::new(rect.width(), title_h)), "CONFIGURAÇÕES");

        // Item 1: Sons de notificação
        let item = Rect::from_min_size(Pos2::new(inner.min.x, inner.min.y + title_h + 2.0), Vec2::new(inner.width(), item_h));
        let r = super::clickable(ui, item, Id::new("setting-sound"));
        if r.hovered() {
            p.rect_filled(item, 5.0, pal.hover);
        }
        text_in(p, item.shrink2(Vec2::new(8.0, 0.0)), Align2::LEFT_CENTER, "Sons de notificação", theme::ui(12.0), pal.text);
        let cb = Rect::from_center_size(Pos2::new(item.max.x - 15.0, item.center().y), Vec2::splat(14.0));
        checkbox(p, cb, sound);
        if r.clicked() {
            toggle = true;
        }

        // Item 2: Abrir abas dos agentes
        let item2 = Rect::from_min_size(Pos2::new(inner.min.x, item.max.y), Vec2::new(inner.width(), item_h));
        let r = super::clickable(ui, item2, Id::new("setting-auto-tabs"));
        if r.hovered() {
            p.rect_filled(item2, 5.0, pal.hover);
        }
        text_in(p, item2.shrink2(Vec2::new(8.0, 0.0)), Align2::LEFT_CENTER, "Abrir abas dos agentes", theme::ui(12.0), pal.text);
        let cb2 = Rect::from_center_size(Pos2::new(item2.max.x - 15.0, item2.center().y), Vec2::splat(14.0));
        checkbox(p, cb2, auto_tabs);
        if r.clicked() {
            toggle_tabs = true;
        }

        // Item 3: Atalhos do app acima do terminal
        let item3 = Rect::from_min_size(Pos2::new(inner.min.x, item2.max.y), Vec2::new(inner.width(), item_h));
        let r = super::clickable(ui, item3, Id::new("setting-app-keys"));
        if r.hovered() {
            p.rect_filled(item3, 5.0, pal.hover);
        }
        text_in(p, item3.shrink2(Vec2::new(8.0, 0.0)), Align2::LEFT_CENTER, "Atalhos do app acima do terminal", theme::ui(12.0), pal.text);
        let cb3 = Rect::from_center_size(Pos2::new(item3.max.x - 15.0, item3.center().y), Vec2::splat(14.0));
        checkbox(p, cb3, app_keys);
        if r.clicked() {
            toggle_keys = true;
        }
        super::tip(r, "Ligado: Ctrl+B é do app mesmo com o terminal focado. Desligado: vai para o terminal (prefixo do tmux).");

        separator(p, Pos2::new(inner.min.x + 4.0, item3.max.y + 6.5), Pos2::new(inner.max.x - 4.0, item3.max.y + 6.5));

        // ▶ Testar som
        let btn = Rect::from_min_size(Pos2::new(inner.min.x + 2.0, item3.max.y + 13.0), Vec2::new(inner.width() - 4.0, btn_h));
        if button(ui, btn, Id::new("setting-test-sound"), "▶ Testar som", ButtonKind::Secondary).clicked() {
            test = true;
        }

        // Ditado por voz (Whisper local): baixar depois, ou apagar para liberar o espaço.
        separator(p, Pos2::new(inner.min.x + 4.0, btn.max.y + 6.5), Pos2::new(inner.max.x - 4.0, btn.max.y + 6.5));
        let row = Rect::from_min_size(Pos2::new(inner.min.x, btn.max.y + 13.0), Vec2::new(inner.width(), item_h));
        text_in(p, row.shrink2(Vec2::new(8.0, 0.0)), Align2::LEFT_CENTER, "Ditado por voz", theme::ui(12.0), pal.text);
        let (status, status_color) = match setup {
            Setup::Ready => ("instalado", pal.success),
            Setup::Downloading => ("baixando…", pal.text_3),
            Setup::NotInstalled => ("não instalado", pal.text_3),
        };
        text_in(p, row.shrink2(Vec2::new(8.0, 0.0)), Align2::RIGHT_CENTER, status, theme::meta(10.5), status_color);
        let action = Rect::from_min_size(Pos2::new(inner.min.x + 2.0, row.max.y), Vec2::new(inner.width() - 4.0, btn_h));
        let id = Id::new("setting-dictation");
        let size = crate::dictation::DOWNLOAD_SIZE;
        match setup {
            Setup::Downloading => {
                text_in(
                    p,
                    action.shrink2(Vec2::new(6.0, 0.0)),
                    Align2::LEFT_CENTER,
                    "O progresso está na barra direita.",
                    theme::ui(11.5),
                    pal.text_3,
                );
            }
            Setup::NotInstalled => {
                let r = button(ui, action, id, &format!("Baixar ({size})"), ButtonKind::Secondary);
                dictation_click |= super::tip(r, "Baixa o Whisper para ditar por voz; o áudio fica no computador.").clicked();
            }
            Setup::Ready if confirm_remove => {
                let r = button_danger(ui, action, id, "Confirmar: apagar do disco");
                dictation_click |= super::tip(r, "Apaga o modelo e o motor CUDA. Dá para baixar de novo depois.").clicked();
            }
            Setup::Ready => {
                let r = button(ui, action, id, "Remover do disco", ButtonKind::Secondary);
                dictation_click |=
                    super::tip(r, format!("Apaga o Whisper ({size}) para liberar espaço; dá para baixar de novo.")).clicked();
            }
        }
    });
    if toggle {
        deck.sound_enabled = !deck.sound_enabled;
        let v = deck.sound_enabled;
        deck.core.update_config(|c| c.sound_enabled = v);
    }
    if toggle_tabs {
        deck.auto_open_agent_tabs = !deck.auto_open_agent_tabs;
        let v = deck.auto_open_agent_tabs;
        deck.core.update_config(|c| c.auto_open_agent_tabs = v);
    }
    if toggle_keys {
        deck.app_shortcuts_first = !deck.app_shortcuts_first;
        let v = deck.app_shortcuts_first;
        deck.core.update_config(|c| c.app_shortcuts_first = v);
    }
    if test {
        deck.play_sound(true);
    }
    if dictation_click {
        match setup {
            Setup::NotInstalled => deck.dictation_accept(),
            Setup::Ready if confirm_remove => deck.dictation_remove(),
            Setup::Ready => deck.dictation_confirm_remove = true,
            Setup::Downloading => {}
        }
    }
    if closes_on_outside(ctx, menu_rect, anchor) {
        deck.settings_open = false;
    }
}

/// Checkbox com `accent-color` dinâmico.
fn checkbox(p: &egui::Painter, r: Rect, checked: bool) {
    let pal = theme::pal();
    if checked {
        p.rect_filled(r, 3.0, theme::accent());
        let s = Stroke::new(1.8, pal.on_accent);
        let a = Pos2::new(r.min.x + 3.0, r.center().y + 0.5);
        let b = Pos2::new(r.min.x + 6.0, r.max.y - 3.5);
        let c = Pos2::new(r.max.x - 3.0, r.min.y + 3.5);
        p.line_segment([a, b], s);
        p.line_segment([b, c], s);
    } else {
        p.rect(r, 3.0, pal.recess, Stroke::new(1.0, pal.separator), egui::StrokeKind::Inside);
    }
}

/// Menu de contexto ao clicar com botão direito numa aba.
pub fn tab_context_menu(deck: &mut Deck, ctx: &egui::Context) {
    let Some((idx, pos)) = deck.tab_context_menu else { return };
    if idx >= deck.tabs.len() {
        deck.tab_context_menu = None;
        return;
    }
    // Quando a instância tem modelo de LLM, ele aparece no alto (só leitura: trocar com a aba aberta
    // exigiria reiniciar o agente).
    let model = deck.tabs[idx].model.clone();
    let w = if model.is_some() { 220.0 } else { 130.0 };
    let item_h = 26.0;
    let h = item_h * (2.0 + if model.is_some() { 1.0 } else { 0.0 }) + 8.0;
    let pos = keep_on_screen(ctx, pos, Vec2::new(w, h));
    let menu_rect = Rect::from_min_size(pos, Vec2::new(w, h));
    let mut action = None;
    let pal = theme::pal();
    Area::new(Id::new("tab-context-menu")).order(Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
        let p = ui.painter();
        shadow(p, rect, 8, 8, 24, pal.shadow);
        frame(p, rect, 8.0, pal.popup, pal.glass_border);
        let font = theme::ui(12.0);

        let mut top = rect.min.y + 4.0;
        if let Some(m) = &model {
            let r0 = Rect::from_min_size(Pos2::new(rect.min.x + 4.0, top), Vec2::new(w - 8.0, item_h));
            top += item_h;
            let g = super::galley_trunc(p, &format!("Modelo: {m}"), theme::meta(10.5), pal.text_3, r0.width() - 16.0);
            super::place(p, r0.shrink2(Vec2::new(8.0, 0.0)), Align2::LEFT_CENTER, g, pal.text_3);
            separator(p, Pos2::new(r0.min.x + 4.0, r0.max.y - 0.5), Pos2::new(r0.max.x - 4.0, r0.max.y - 0.5));
        }
        // "Renomear"
        let r1 = Rect::from_min_size(Pos2::new(rect.min.x + 4.0, top), Vec2::new(w - 8.0, item_h));
        let resp1 = super::clickable(ui, r1, Id::new("tab-ctx-rename"));
        if resp1.hovered() {
            p.rect_filled(r1, 5.0, pal.hover);
        }
        text_in(p, r1.shrink2(Vec2::new(8.0, 0.0)), Align2::LEFT_CENTER, "Renomear", font.clone(), pal.text);
        if resp1.clicked() {
            action = Some(1);
        }

        // "Fechar"
        let r2 = Rect::from_min_size(Pos2::new(rect.min.x + 4.0, r1.max.y), Vec2::new(w - 8.0, item_h));
        let resp2 = super::clickable(ui, r2, Id::new("tab-ctx-close"));
        if resp2.hovered() {
            p.rect_filled(r2, 5.0, pal.hover);
        }
        text_in(p, r2.shrink2(Vec2::new(8.0, 0.0)), Align2::LEFT_CENTER, "Fechar", font, pal.text);
        if resp2.clicked() {
            action = Some(2);
        }
    });

    match action {
        Some(1) => {
            deck.tab_context_menu = None;
            deck.start_rename_tab(idx);
        }
        Some(2) => {
            deck.tab_context_menu = None;
            deck.close_tab(idx);
        }
        _ => {
            if ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !menu_rect.contains(p))) {
                deck.tab_context_menu = None;
            }
        }
    }
}

/// Modal para renomear um agente / instância (formato modelo@cargo@nome).
pub fn rename_tab_modal(deck: &mut Deck, ctx: &egui::Context) {
    let Some((idx, mut buffer, error)) = deck.rename_tab.clone() else { return };
    if idx >= deck.tabs.len() {
        deck.rename_tab = None;
        return;
    }
    let screen = ctx.content_rect();
    super::modal_backdrop(deck, ctx, "rename-backdrop");

    let w = 360.0;
    let h = if error.is_some() { 162.0 } else { 142.0 };
    let center = screen.center();
    let rect = Rect::from_center_size(center, Vec2::new(w, h));
    let mut close = false;
    let mut apply = false;
    let pal = theme::pal();

    Area::new(Id::new("rename-tab-modal")).order(Order::Tooltip).fixed_pos(rect.min).show(ctx, |ui| {
        let (r, _) = ui.allocate_exact_size(rect.size(), Sense::hover());
        let p = ui.painter().clone();
        shadow(&p, r, 10, 8, 24, pal.shadow);
        frame(&p, r, 10.0, pal.popup, pal.glass_border);

        let mut y = r.min.y + 14.0;
        // Título (13 px 600)
        let title_g = super::galley(&p, "Renomear Agente", theme::ui_semibold(13.0), pal.text_strong);
        super::place(
            &p,
            Rect::from_min_size(Pos2::new(r.min.x + 16.0, y), Vec2::new(w - 32.0, 18.0)),
            Align2::LEFT_CENTER,
            title_g,
            pal.text_strong,
        );
        y += 22.0;

        // Explicação
        let hint_g = super::galley(&p, "Formato: modelo@cargo@nome (ex: claude@dev ou antigravity@lead)", theme::ui(11.5), pal.text_2);
        super::place(
            &p,
            Rect::from_min_size(Pos2::new(r.min.x + 16.0, y), Vec2::new(w - 32.0, 16.0)),
            Align2::LEFT_CENTER,
            hint_g,
            pal.text_2,
        );
        y += 22.0;

        // Caixa de texto (recess, borda separator e accent no foco)
        let input_id = Id::new("rename-tab-input");
        let focused = ui.ctx().memory(|m| m.has_focus(input_id));
        let input_r = Rect::from_min_size(Pos2::new(r.min.x + 16.0, y), Vec2::new(w - 32.0, 28.0));
        p.rect_filled(input_r, 6.0, pal.recess);
        p.rect_stroke(input_r, 6.0, Stroke::new(1.0, if focused { theme::accent() } else { pal.separator }), egui::StrokeKind::Inside);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(input_r.shrink2(Vec2::new(8.0, 4.0))));
        let te = egui::TextEdit::singleline(&mut buffer)
            .id(input_id)
            .frame(egui::Frame::NONE)
            .font(theme::mono(12.0))
            .text_color(pal.text)
            .desired_width(input_r.width() - 16.0);
        let out = te.show(&mut child);
        out.response.request_focus();
        if out.response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            apply = true;
        }
        if ui.input(|i| i.key_pressed(Key::Enter)) {
            apply = true;
        }
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            close = true;
        }
        y += 34.0;

        // Erro se houver
        if let Some(err) = &error {
            let err_g = super::galley_trunc(&p, err, theme::ui(11.5), pal.danger_text, w - 32.0);
            super::place(
                &p,
                Rect::from_min_size(Pos2::new(r.min.x + 16.0, y), Vec2::new(w - 32.0, 16.0)),
                Align2::LEFT_CENTER,
                err_g,
                pal.danger_text,
            );
            y += 20.0;
        }

        // Botões Cancelar e Salvar
        let btn_h = 26.0;
        let save_w = button_width(&p, "Salvar", ButtonKind::Primary);
        let cancel_w = button_width(&p, "Cancelar", ButtonKind::Secondary);
        let save_r = Rect::from_min_size(Pos2::new(r.max.x - 16.0 - save_w, y), Vec2::new(save_w, btn_h));
        let cancel_r = Rect::from_min_size(Pos2::new(save_r.min.x - 8.0 - cancel_w, y), Vec2::new(cancel_w, btn_h));

        if button(ui, cancel_r, Id::new("rename-cancel-btn"), "Cancelar", ButtonKind::Secondary).clicked() {
            close = true;
        }
        if button(ui, save_r, Id::new("rename-save-btn"), "Salvar", ButtonKind::Primary).clicked() {
            apply = true;
        }
    });

    if close {
        deck.rename_tab = None;
    } else if apply {
        match deck.apply_rename_tab(idx, &buffer) {
            Ok(()) => {}
            Err(e) => {
                deck.rename_tab = Some((idx, buffer, Some(e)));
            }
        }
    } else {
        deck.rename_tab = Some((idx, buffer, error));
    }
}

/// A pergunta do ditado por voz: na primeira abertura (sem Whisper baixado) e quando se clica no microfone sem ele.
/// "Agora não" (ou Esc) grava o "não" e nada é baixado; só o botão de baixar começa o download.
pub fn dictation_modal(deck: &mut Deck, ctx: &egui::Context) {
    if !deck.dictation_prompt {
        return;
    }
    super::modal_backdrop(deck, ctx, "dictation-backdrop");
    let pal = theme::pal();
    let w = 400.0;
    let pad = 20.0;
    let size = crate::dictation::DOWNLOAD_SIZE;
    let intro = "Fale em vez de digitar: o AgentDeck transcreve a sua voz aqui no computador, sem enviar o áudio para a internet.";
    let detail = format!(
        "Para isso ele baixa o Whisper (modelo e motor de transcrição): {size}, e mais uns 700 MB do motor CUDA se houver uma placa NVIDIA. \
         O download roda em segundo plano e o progresso aparece na barra direita."
    );
    let later = "Você pode baixar ou remover o ditado depois, em Configurações.";
    let mut accept = false;
    let mut decline = ctx.input(|i| i.key_pressed(Key::Escape));

    // O texto quebra em linhas: mede antes para saber a altura do cartão.
    let measure = ctx.fonts_mut(|f| {
        let mut lay = |t: &str, font: egui::FontId| f.layout(t.to_owned(), font, pal.text_2, w - pad * 2.0).size().y;
        (lay(intro, theme::ui(12.0)), lay(&detail, theme::ui(12.0)), lay(later, theme::ui(11.5)))
    });
    let h = 14.0 + 22.0 + measure.0 + 8.0 + measure.1 + 8.0 + measure.2 + 18.0 + 28.0 + 16.0;
    let rect = Rect::from_center_size(ctx.content_rect().center(), Vec2::new(w, h));
    Area::new(Id::new("dictation-modal")).order(Order::Tooltip).fixed_pos(rect.min).show(ctx, |ui| {
        let (r, _) = ui.allocate_exact_size(rect.size(), Sense::hover());
        let p = ui.painter().clone();
        shadow(&p, r, 10, 8, 24, pal.shadow);
        frame(&p, r, 10.0, pal.popup, pal.glass_border);
        let mut y = r.min.y + 14.0;
        let title = super::galley(&p, "Ditado por voz (opcional)", theme::ui_semibold(13.0), pal.text_strong);
        super::place(
            &p,
            Rect::from_min_size(Pos2::new(r.min.x + pad, y), Vec2::new(w - pad * 2.0, 18.0)),
            Align2::LEFT_CENTER,
            title,
            pal.text_strong,
        );
        y += 22.0;
        for (text, font, color) in
            [(intro, theme::ui(12.0), pal.text), (detail.as_str(), theme::ui(12.0), pal.text_2), (later, theme::ui(11.5), pal.text_3)]
        {
            let g = p.layout(text.to_owned(), font, color, w - pad * 2.0);
            let h = g.size().y;
            p.galley(Pos2::new(r.min.x + pad, y), g, color);
            y += h + 8.0;
        }
        y += 10.0;
        let download = format!("Baixar ({size})");
        let accept_w = button_width(&p, &download, ButtonKind::Primary);
        let decline_w = button_width(&p, "Agora não", ButtonKind::Secondary);
        let accept_r = Rect::from_min_size(Pos2::new(r.max.x - pad - accept_w, y), Vec2::new(accept_w, 28.0));
        let decline_r = Rect::from_min_size(Pos2::new(accept_r.min.x - 8.0 - decline_w, y), Vec2::new(decline_w, 28.0));
        if button(ui, decline_r, Id::new("dictation-decline-btn"), "Agora não", ButtonKind::Secondary).clicked() {
            decline = true;
        }
        if button(ui, accept_r, Id::new("dictation-accept-btn"), &download, ButtonKind::Primary).clicked() {
            accept = true;
        }
    });
    if accept {
        deck.dictation_accept();
    } else if decline {
        deck.dictation_decline();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_some_agents_offer_models_in_the_menu() {
        for k in ["claude", "antigravity", "opencode"] {
            assert!(picks_model(k), "{k}");
        }
        for k in ["codex", "shell", "ollama", "ollama:qwen3.5:9b", "aider"] {
            assert!(!picks_model(k), "{k}");
        }
    }

    #[test]
    fn the_status_line_explains_an_empty_list() {
        assert_eq!(models_status(true, true), Some("Carregando modelos…"));
        assert_eq!(models_status(false, true), Some("Sem lista de modelos: só o padrão"));
        assert_eq!(models_status(false, false), None);
        assert_eq!(models_status(true, false), None, "com a lista antiga na tela, atualiza em silêncio");
    }
}
