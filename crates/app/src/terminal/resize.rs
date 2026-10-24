//! Mudança de tamanho da grade como o ConPTY espera (Windows).
//!
//! O `alacritty_terminal` mantém o cursor na mesma linha da tela e, para isso, **puxa linhas do histórico para o topo**
//! quando a tela cresce em linhas ou quando o reflow de uma largura maior junta linhas. O ConPTY não faz isso: o topo da
//! janela dele fica no mesmo conteúdo, as linhas que sobram ficam em branco embaixo, e logo depois da mudança ele repinta
//! a janela inteira a partir de `ESC[H` (medido com o PowerShell e com o Claude Code). A repintura cai por cima das linhas
//! que a grade puxou, e elas somem: linhas numeradas de 144 a 171 sumiam da conversa (e 28 linhas ficavam em branco
//! embaixo) numa janela que ia de 58 a 30 e de volta a 58 linhas; o mesmo acontecia ao alargar com texto quebrado.
//! Por isso depois de cada mudança o conteúdo que estava no topo da tela volta para o topo, e o que foi puxado volta para
//! o histórico. Este arquivo não depende do resto do app: `tools/pty-probe` o inclui por `#[path]` para medir.

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::{Term, TermMode};

/// Caractere sem largura (área de uso privado) que marca a primeira célula da tela durante a mudança.
const MARK: char = '\u{E000}';

/// `term.resize(size)`; com `keep_top` o conteúdo que estava na primeira linha da tela continua nela (ver o módulo).
pub fn resize<T: EventListener>(term: &mut Term<T>, size: impl Dimensions, keep_top: bool) {
    if !keep_top || term.grid().history_size() == 0 || term.mode().contains(TermMode::ALT_SCREEN) {
        term.resize(size);
        return;
    }
    let saved = term.grid()[Line(0)][Column(0)].clone();
    term.grid_mut()[Line(0)][Column(0)].push_zerowidth(MARK);
    term.resize(size);

    let grid = term.grid_mut();
    let marked = |l: i32, c: usize| grid[Line(l)][Column(c)].zerowidth().is_some_and(|z| z.contains(&MARK));
    let rows = grid.screen_lines() as i32;
    // A marca está na tela (puxada para baixo) ou logo acima dela (empurrada para o histórico).
    let top = -(grid.history_size() as i32);
    let Some((line, col)) = (top..rows).rev().find_map(|l| (0..grid.columns()).find(|&c| marked(l, c)).map(|c| (l, c))) else { return };

    // Tira a marca: a célula volta ao que era (só o `extra`, privado, é o do clone salvo).
    let cell = &mut grid[Line(line)][Column(col)];
    let mut restored = saved;
    (restored.c, restored.fg, restored.bg, restored.flags) = (cell.c, cell.fg, cell.bg, cell.flags);
    *cell = restored;

    // Marca no meio de uma linha (a continuação de uma linha quebrada foi juntada à anterior no reflow): o ConPTY começa a
    // janela na linha seguinte, então essa linha inteira vai para o histórico.
    let line = if col > 0 { line + 1 } else { line };
    if line > 0 {
        grid.scroll_up(&(Line(0)..Line(rows)), line as usize);
        grid.cursor.point.line = Line((grid.cursor.point.line.0 - line).max(0));
        grid.saved_cursor.point.line = Line((grid.saved_cursor.point.line.0 - line).max(0));
    }
}
