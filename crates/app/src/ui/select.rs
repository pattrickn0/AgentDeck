//! Seleção de texto nas telas pintadas à mão (o diff e o arquivo grande, em leitura). O egui só seleciona o que é
//! `Label` ou `TextEdit`, e estas telas só pintam as linhas que aparecem (`show_viewport`/`show_rows`): aqui a seleção é
//! um par de posições (linha, byte dentro do texto da linha) guardado entre quadros, e quem pinta pergunta, linha a
//! linha, o que dela está selecionado ([`Selected::on_line`]).
//!
//! Arrastar seleciona (rolando a tela quando o ponteiro passa da borda), clique limpa, Ctrl+A seleciona tudo e Ctrl+C
//! copia. O arquivo pequeno não passa por aqui: o `TextEdit` do egui já faz tudo isso.

use std::ops::Range;

use egui::{Event, Id, Key, Modifiers, Pos2, Rect, Sense, Ui, Vec2};

/// Uma posição no texto: a linha e o byte dentro do texto dela.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

/// A seleção em andamento: onde o arrasto começou e onde está agora (a cabeça pode vir antes da âncora).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Drag {
    anchor: Pos,
    head: Pos,
}

/// Uma seleção não vazia, com o começo antes do fim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selected {
    pub start: Pos,
    pub end: Pos,
}

impl Selected {
    /// O que da linha `line`, de `len` bytes, está selecionado.
    pub fn on_line(&self, line: usize, len: usize) -> Option<Range<usize>> {
        if line < self.start.line || line > self.end.line {
            return None;
        }
        let a = if line == self.start.line { self.start.col.min(len) } else { 0 };
        let b = if line == self.end.line { self.end.col.min(len) } else { len };
        Some(a..b.max(a))
    }
}

fn ordered(d: Drag) -> Option<Selected> {
    (d.anchor != d.head).then(|| Selected { start: d.anchor.min(d.head), end: d.anchor.max(d.head) })
}

/// Em quantos pixels da borda o arrasto começa a rolar a tela, e quanto rola por quadro no máximo.
const EDGE: f32 = 24.0;
const SCROLL_STEP: f32 = 22.0;

/// Trata o mouse e o teclado da área `rect` (a parte visível, em coordenadas da tela) e devolve a seleção do quadro.
/// `hit` traduz um ponto da tela numa posição do texto (qualquer ponto, mesmo fora da área: o arrasto passa da borda);
/// `end` é o fim do texto (para Ctrl+A); `text` devolve o texto de uma seleção (para Ctrl+C).
/// Deve vir antes de qualquer botão que se desenhe por cima da área, para ele ganhar o clique.
pub fn drive(
    ui: &mut Ui,
    id: Id,
    rect: Rect,
    end: Pos,
    hit: impl Fn(Pos2) -> Pos,
    text: impl FnOnce(Selected) -> String,
) -> Option<Selected> {
    let ctx = ui.ctx().clone();
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let mut d: Drag = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    if resp.drag_started() {
        if let Some(origin) = ctx.input(|i| i.pointer.press_origin()) {
            d.anchor = hit(origin);
            d.head = d.anchor;
        }
        resp.request_focus();
    } else if resp.clicked() {
        d = Drag::default();
        resp.request_focus();
    }
    if resp.dragged() {
        if let Some(p) = ctx.input(|i| i.pointer.interact_pos()) {
            d.head = hit(p);
            let dy = if p.y < rect.min.y + EDGE {
                (p.y - (rect.min.y + EDGE)).max(-SCROLL_STEP * 3.0) / 3.0
            } else if p.y > rect.max.y - EDGE {
                (p.y - (rect.max.y - EDGE)).min(SCROLL_STEP * 3.0) / 3.0
            } else {
                0.0
            };
            if dy != 0.0 {
                // `scroll_with_delta` anda no sentido do conteúdo: arrastar para baixo da borda rola para baixo.
                ui.scroll_with_delta(Vec2::new(0.0, -dy));
                ctx.request_repaint();
            }
        }
    }
    if resp.has_focus() {
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::A)) {
            d = Drag { anchor: Pos::default(), head: end };
        }
        let copy = ctx.input(|i| i.events.iter().any(|e| matches!(e, Event::Copy)));
        if copy {
            if let Some(sel) = ordered(d) {
                ctx.copy_text(text(sel));
            }
        }
    }
    ctx.data_mut(|w| w.insert_temp(id, d));
    ordered(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(line: usize, col: usize) -> Pos {
        Pos { line, col }
    }

    #[test]
    fn the_selection_is_ordered_whatever_the_drag_direction() {
        let forward = ordered(Drag { anchor: p(1, 2), head: p(3, 0) }).expect("seleção");
        let backward = ordered(Drag { anchor: p(3, 0), head: p(1, 2) }).expect("seleção");
        assert_eq!(forward, backward);
        assert_eq!((forward.start, forward.end), (p(1, 2), p(3, 0)));
        // Um clique sem arrastar não seleciona nada.
        assert_eq!(ordered(Drag { anchor: p(2, 4), head: p(2, 4) }), None);
    }

    #[test]
    fn each_line_reports_its_selected_part() {
        let sel = Selected { start: p(1, 2), end: p(3, 4) };
        assert_eq!(sel.on_line(0, 10), None);
        assert_eq!(sel.on_line(1, 10), Some(2..10));
        assert_eq!(sel.on_line(2, 10), Some(0..10));
        assert_eq!(sel.on_line(2, 0), Some(0..0));
        assert_eq!(sel.on_line(3, 10), Some(0..4));
        assert_eq!(sel.on_line(4, 10), None);
        // O fim além do tamanho da linha (a linha ficou mais curta) é cortado.
        assert_eq!(sel.on_line(3, 2), Some(0..2));
        // Começo e fim na mesma linha.
        let one = Selected { start: p(5, 1), end: p(5, 3) };
        assert_eq!(one.on_line(5, 10), Some(1..3));
        assert_eq!(one.on_line(5, 2), Some(1..2));
        assert_eq!(one.on_line(5, 0), Some(0..0));
    }
}
