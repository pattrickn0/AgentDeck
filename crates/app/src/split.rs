//! Tela dividida por tipo de agente: cada grupo (`claude`, `antigravity`, `ollama`…) ganha uma
//! janela, e as janelas se organizam numa grade automática (Dividida) ou numa janela grande com as
//! outras empilhadas ao lado (Destaque). Só cálculo (sem pintura), com testes.

use egui::{Pos2, Rect};

/// Tamanho mínimo de uma janela (cabeçalho de abas + algumas linhas de terminal).
pub const MIN_W: f32 = 260.0;
pub const MIN_H: f32 = 140.0;
/// Fresta entre as janelas (ilhas): o divisor arrastável mora nela.
pub const GAP: f32 = crate::theme::ISLAND_GAP;

/// Grupo (janela) de uma aba: todos os modelos do Ollama ficam juntos; o resto é o próprio tipo
/// (`claude@dev@ana` é `kind = "claude"`, então fica no grupo do Claude).
pub fn group_key(kind: &str) -> &str {
    if kind == "ollama" || kind.starts_with("ollama:") {
        "ollama"
    } else {
        kind
    }
}

/// Quantas janelas por linha para `n` grupos numa área `w`×`h`. Até 2 grupos lado a lado; 3 =
/// 2 em cima + 1 largo embaixo; 4 = 2×2; 5 = 3+2… Respeita o tamanho mínimo: se não cabem
/// todos, devolve menos janelas (os grupos que sobram vão como abas na última).
pub fn grid_shape(n: usize, w: f32, h: f32) -> Vec<usize> {
    if n == 0 {
        return Vec::new();
    }
    let max_cols = ((w + GAP) / (MIN_W + GAP)).floor().max(1.0) as usize;
    let max_rows = ((h + GAP) / (MIN_H + GAP)).floor().max(1.0) as usize;
    let n = n.min(max_cols * max_rows);
    let ideal = (n as f32).sqrt().ceil() as usize;
    let cols = ideal.max(n.div_ceil(max_rows)).min(max_cols).max(1);
    let rows = n.div_ceil(cols);
    (0..rows).map(|r| if r + 1 < rows { cols } else { n - cols * (rows - 1) }).collect()
}

/// Fração da largura que a janela em destaque ocupa por padrão (2/3 do centro).
pub const HIGHLIGHT_RATIO: f32 = 2.0 / 3.0;

/// A proporção do Destaque que vale: a gravada (0 = nenhuma) se for uma fração de verdade.
pub fn highlight_ratio(saved: f32) -> f32 {
    if saved.is_finite() && saved > 0.0 && saved < 1.0 {
        saved
    } else {
        HIGHLIGHT_RATIO
    }
}

/// Quantas janelas o Destaque desenha para `n` grupos numa área `w`×`h`: a grande e as pequenas
/// empilhadas ao lado, cada uma com a altura mínima. Se a área é estreita demais para duas colunas,
/// é uma só; as pequenas que não cabem deixam os grupos que sobram como abas na última (`assign`).
pub fn highlight_windows(n: usize, w: f32, h: f32) -> usize {
    if n <= 1 || w < 2.0 * MIN_W + GAP {
        return n.min(1);
    }
    let max_small = ((h + GAP) / (MIN_H + GAP)).floor().max(1.0) as usize;
    1 + (n - 1).min(max_small)
}

/// Retângulos do Destaque para `windows` janelas: a grande à esquerda (`ratio` da largura) e as
/// outras na coluna da direita, com a mesma altura e a fresta de sempre. Uma janela só ocupa a área.
pub fn highlight_layout(area: Rect, windows: usize, ratio: f32) -> Vec<Rect> {
    if windows <= 1 {
        return vec![area; windows];
    }
    let avail = (area.width() - GAP).max(0.0);
    let want = avail * highlight_ratio(ratio);
    let big = if avail >= 2.0 * MIN_W { want.clamp(MIN_W, avail - MIN_W) } else { want };
    let big_x1 = (area.min.x + big).round();
    let mut out = vec![Rect::from_min_max(area.min, Pos2::new(big_x1, area.max.y))];
    let x0 = big_x1 + GAP;
    let small = windows - 1;
    let heights = sizes(area.height(), &equal(small), MIN_H);
    let mut y = area.min.y;
    for (i, h) in heights.iter().enumerate() {
        let y1 = if i + 1 == small { area.max.y } else { (y + h).round() };
        out.push(Rect::from_min_max(Pos2::new(x0, y.round()), Pos2::new(area.max.x, y1)));
        y = y1 + GAP;
    }
    out
}

/// Ordem dos grupos no Destaque: o do agente selecionado primeiro (a janela grande) e os outros na
/// ordem das janelas, que não muda quando o destaque troca.
pub fn highlight_order(order: &[String], featured: Option<&str>) -> Vec<String> {
    let Some(f) = featured.and_then(|f| order.iter().find(|g| g.as_str() == f)) else { return order.to_vec() };
    std::iter::once(f.clone()).chain(order.iter().filter(|g| *g != f).cloned()).collect()
}

/// Arrasta o divisor entre a janela grande e a coluna em `delta` pixels (de `total`, a largura do
/// centro), mantendo as duas com a largura mínima.
pub fn drag_highlight(ratio: &mut f32, delta: f32, total: f32) {
    let first = highlight_ratio(*ratio);
    let mut pair = [first, 1.0 - first];
    drag(&mut pair, 0, delta, total - GAP, MIN_W);
    *ratio = pair[0];
}

/// Proporções iguais para `n` partes.
pub fn equal(n: usize) -> Vec<f32> {
    vec![1.0 / n as f32; n]
}

/// Proporções das linhas e das colunas de cada linha. Voltam a iguais quando o formato da grade
/// muda (entra ou sai um grupo).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Ratios {
    pub rows: Vec<f32>,
    pub cols: Vec<Vec<f32>>,
}

impl Ratios {
    pub fn equal(shape: &[usize]) -> Self {
        Self { rows: equal(shape.len()), cols: shape.iter().map(|&c| equal(c)).collect() }
    }

    pub fn fits(&self, shape: &[usize]) -> bool {
        self.rows.len() == shape.len() && self.cols.iter().map(Vec::len).eq(shape.iter().copied())
    }

    /// Ajusta ao formato atual (mantém as proporções se o formato não mudou).
    pub fn conform(&mut self, shape: &[usize]) {
        if !self.fits(shape) {
            *self = Self::equal(shape);
        }
    }

    /// As proporções para desenhar o formato atual, sem gravar nada: as salvas se servem, senão
    /// iguais. Um formato passageiro (janela estreitada por um instante) não apaga as do usuário.
    pub fn fitted(&self, shape: &[usize]) -> Self {
        if self.fits(shape) {
            self.clone()
        } else {
            Self::equal(shape)
        }
    }
}

/// Tamanhos das partes de `total` (descontados os divisores) segundo `ratios`, sem deixar nenhuma
/// abaixo de `min` quando há espaço para todas.
pub fn sizes(total: f32, ratios: &[f32], min: f32) -> Vec<f32> {
    let n = ratios.len();
    if n == 0 {
        return Vec::new();
    }
    let avail = (total - GAP * (n - 1) as f32).max(0.0);
    let sum: f32 = ratios.iter().sum();
    let mut out: Vec<f32> = ratios.iter().map(|r| if sum > 0.0 { avail * r / sum } else { avail / n as f32 }).collect();
    if avail >= min * n as f32 {
        // Quem ficou abaixo do mínimo sobe; a diferença sai, em proporção, de quem tem folga.
        let deficit: f32 = out.iter().map(|s| (min - s).max(0.0)).sum();
        if deficit > 0.0 {
            let slack: f32 = out.iter().map(|s| (s - min).max(0.0)).sum();
            for s in &mut out {
                *s = if *s <= min { min } else { *s - deficit * (*s - min) / slack };
            }
        }
    }
    out
}

/// Retângulos das janelas, linha por linha.
pub fn layout(area: Rect, shape: &[usize], ratios: &Ratios) -> Vec<Rect> {
    let heights = sizes(area.height(), &ratios.rows, MIN_H);
    let mut out = Vec::new();
    let mut y = area.min.y;
    for (r, &cols) in shape.iter().enumerate() {
        let h = heights[r];
        let widths = sizes(area.width(), &ratios.cols[r], MIN_W);
        let mut x = area.min.x;
        for (c, w) in widths.iter().enumerate() {
            // A última de cada linha/coluna encosta na borda (sem sobra de arredondamento).
            let x1 = if c + 1 == cols { area.max.x } else { (x + w).round() };
            let y1 = if r + 1 == shape.len() { area.max.y } else { (y + h).round() };
            out.push(Rect::from_min_max(Pos2::new(x.round(), y.round()), Pos2::new(x1, y1)));
            x = x1 + GAP;
        }
        y = (y + h).round() + GAP;
    }
    out
}

/// Arrasta o divisor entre as partes `i` e `i + 1` em `delta` pixels (de `total`), respeitando o
/// mínimo das duas.
pub fn drag(ratios: &mut [f32], i: usize, delta: f32, total: f32, min: f32) {
    if i + 1 >= ratios.len() || total <= 0.0 {
        return;
    }
    let pair = ratios[i] + ratios[i + 1];
    let min_r = (min / total).min(pair / 2.0);
    let a = (ratios[i] + delta / total).clamp(min_r, pair - min_r);
    ratios[i] = a;
    ratios[i + 1] = pair - a;
}

/// Divide os grupos em janelas: um grupo por janela e, se não couberem todos, os que sobram vão
/// juntos na última.
pub fn assign<T: Clone>(groups: &[T], panes: usize) -> Vec<Vec<T>> {
    if groups.is_empty() || panes == 0 {
        return Vec::new();
    }
    let panes = panes.min(groups.len());
    let mut out: Vec<Vec<T>> = groups[..panes - 1].iter().map(|g| vec![g.clone()]).collect();
    out.push(groups[panes - 1..].to_vec());
    out
}

/// Mantém a ordem das janelas igual aos grupos abertos (`kinds`, na ordem das abas): um grupo
/// novo entra no fim e sai quando a última aba dele fecha.
pub fn sync_order<'a>(order: &mut Vec<String>, kinds: impl Iterator<Item = &'a str>) {
    let mut present: Vec<&str> = Vec::new();
    for k in kinds {
        let g = group_key(k);
        if !present.contains(&g) {
            present.push(g);
        }
    }
    order.retain(|g| present.contains(&g.as_str()));
    for g in present {
        if !order.iter().any(|o| o == g) {
            order.push(g.to_owned());
        }
    }
}

/// Abas de uma janela com os grupos `groups`, na ordem das abas (`keys[i]` é o grupo da aba `i`).
pub fn pane_tabs(keys: &[&str], groups: &[String]) -> Vec<usize> {
    groups.iter().flat_map(|g| (0..keys.len()).filter(|&i| keys[i] == g).collect::<Vec<_>>()).collect()
}

/// A aba ativada mais recentemente entre `candidates` (`shown[i]` é o contador da aba `i`;
/// empate: a primeira).
pub fn most_recent(shown: &[u64], candidates: impl Iterator<Item = usize>) -> Option<usize> {
    candidates.max_by_key(|&i| (shown[i], std::cmp::Reverse(i)))
}

/// Depois de fechar a aba ativa (do grupo `closed`): a aba do mesmo grupo usada por último, para
/// o foco ficar na mesma janela; sem nenhuma, a última usada de todas.
pub fn after_close(kinds: &[&str], shown: &[u64], closed: &str) -> Option<usize> {
    let group = group_key(closed);
    most_recent(shown, (0..kinds.len()).filter(|&i| group_key(kinds[i]) == group)).or_else(|| most_recent(shown, 0..kinds.len()))
}

/// Janela vizinha de `from` na direção (`dx`, `dy`): a mais próxima cujo centro fica para aquele
/// lado (e que se sobrepõe no outro eixo, se houver alguma).
pub fn neighbor(rects: &[Rect], from: usize, dx: i32, dy: i32) -> Option<usize> {
    let a = *rects.get(from)?;
    let ahead = |b: &Rect| match (dx.signum(), dy.signum()) {
        (1, _) => b.min.x >= a.max.x,
        (-1, _) => b.max.x <= a.min.x,
        (_, 1) => b.min.y >= a.max.y,
        _ => b.max.y <= a.min.y,
    };
    let overlaps = |b: &Rect| if dx != 0 { b.min.y < a.max.y && b.max.y > a.min.y } else { b.min.x < a.max.x && b.max.x > a.min.x };
    let dist = |b: &Rect| (b.center() - a.center()).length();
    let candidates: Vec<usize> = (0..rects.len()).filter(|&i| i != from && ahead(&rects[i])).collect();
    let best = |list: &mut dyn Iterator<Item = usize>| list.min_by(|&i, &j| dist(&rects[i]).total_cmp(&dist(&rects[j])));
    best(&mut candidates.iter().copied().filter(|&i| overlaps(&rects[i]))).or_else(|| best(&mut candidates.iter().copied()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;

    fn area(w: f32, h: f32) -> Rect {
        Rect::from_min_size(Pos2::ZERO, vec2(w, h))
    }

    #[test]
    fn group_keys() {
        assert_eq!(group_key("claude"), "claude");
        assert_eq!(group_key("ollama"), "ollama");
        assert_eq!(group_key("ollama:qwen3.5:4b"), "ollama");
        assert_eq!(group_key("ollama:user/custom:latest"), "ollama");
        assert_eq!(group_key("antigravity"), "antigravity");
        assert_eq!(group_key("shell"), "shell");
    }

    #[test]
    fn the_grid_grows_side_by_side_then_in_rows() {
        let big = (1600.0, 900.0);
        let shapes: Vec<Vec<usize>> = (0..=7).map(|n| grid_shape(n, big.0, big.1)).collect();
        assert_eq!(shapes, [vec![], vec![1], vec![2], vec![2, 1], vec![2, 2], vec![3, 2], vec![3, 3], vec![3, 3, 1]]);
    }

    #[test]
    fn a_small_area_holds_fewer_windows() {
        // Só cabe uma coluna: empilha.
        assert_eq!(grid_shape(3, 400.0, 700.0), [1, 1, 1]);
        // Nem altura para duas: uma janela só (os outros grupos viram abas nela).
        assert_eq!(grid_shape(3, 400.0, 200.0), [1]);
        // Larga e baixa: lado a lado.
        assert_eq!(grid_shape(4, 1200.0, 200.0), [4]);
        assert_eq!(grid_shape(6, 1200.0, 200.0), [4]);
        // Janela mínima (820px com as duas barras: ~339 × 440).
        assert_eq!(grid_shape(2, 339.0, 440.0), [1, 1]);
        assert_eq!(grid_shape(5, 339.0, 440.0), [1, 1, 1]);
    }

    #[test]
    fn layout_fills_the_area_without_overlaps() {
        let a = area(1001.0, 601.0);
        for n in 1..=7 {
            let shape = grid_shape(n, a.width(), a.height());
            let rects = layout(a, &shape, &Ratios::equal(&shape));
            assert_eq!(rects.len(), n);
            let covered: f32 = rects.iter().map(|r| r.area()).sum();
            let gaps = a.area() - covered;
            assert!(gaps >= 0.0 && gaps < GAP * (3.0 * a.width() + 3.0 * a.height()), "n={n} sobra {gaps}");
            for (i, r) in rects.iter().enumerate() {
                assert!(a.contains_rect(*r), "n={n} {r:?}");
                for s in &rects[i + 1..] {
                    assert!(r.intersect(*s).area() <= 0.0, "n={n} {r:?} × {s:?}");
                }
            }
        }
        // 3 grupos: dois em cima, um largo embaixo.
        let shape = grid_shape(3, 1001.0, 601.0);
        let r = layout(a, &shape, &Ratios::equal(&shape));
        assert_eq!(r[2].width(), 1001.0);
        assert_eq!(r[0].width() + GAP + r[1].width(), 1001.0);
        assert!((r[0].width() - r[1].width()).abs() <= 1.0);
        // (601 - 6) / 2 = 297,5 de cada linha, arredondado.
        assert_eq!(r[0].height(), 298.0);
        assert_eq!((r[2].min.y, r[2].max.y), (304.0, 601.0));
    }

    #[test]
    fn the_highlight_stacks_the_other_windows_beside_the_big_one() {
        let (w, h) = (1600.0, 900.0);
        // 1, 2, 4 e 7 CLIs: a grande mais as pequenas que cabem na altura (900 → até 6 de 140 px).
        let counts: Vec<usize> = [0, 1, 2, 4, 7].iter().map(|&n| highlight_windows(n, w, h)).collect();
        assert_eq!(counts, [0, 1, 2, 4, 7]);
        let a = area(w, h);
        for windows in [1, 2, 4, 7] {
            let r = highlight_layout(a, windows, HIGHLIGHT_RATIO);
            assert_eq!(r.len(), windows);
            if windows == 1 {
                assert_eq!(r[0], a);
                continue;
            }
            // A grande ocupa 2/3 da largura (menos a fresta) à esquerda, em toda a altura.
            assert_eq!((r[0].min, r[0].height()), (Pos2::ZERO, a.height()));
            assert!((r[0].width() - (w - GAP) * 2.0 / 3.0).abs() <= 1.0, "{:?}", r[0]);
            // As pequenas empilham na coluna da direita com a fresta de sempre, alturas iguais.
            assert_eq!((r[1].min.x, r[1].min.y), (r[0].max.x + GAP, 0.0));
            assert_eq!(r.last().map(|s| (s.max.x, s.max.y)), Some((a.max.x, a.max.y)));
            for pair in r[1..].windows(2) {
                assert_eq!(pair[1].min.y, pair[0].max.y + GAP);
                assert!((pair[0].height() - pair[1].height()).abs() <= 1.0);
            }
            for (i, x) in r.iter().enumerate() {
                assert!(a.contains_rect(*x));
                for y in &r[i + 1..] {
                    assert!(x.intersect(*y).area() <= 0.0, "{x:?} × {y:?}");
                }
            }
        }
        // Em 600 px de altura cabem 4 pequenas: com 7 CLIs, os 3 últimos dividem a última como abas.
        let windows = highlight_windows(7, w, 600.0);
        assert_eq!(windows, 5);
        assert_eq!(assign(&["a", "b", "c", "d", "e", "f", "g"], windows).last().map(Vec::len), Some(3));
    }

    #[test]
    fn the_highlight_respects_the_minimum_window_size() {
        // Só cabem duas pequenas em 300 px de altura (2 × 140 + 6 = 286): as outras viram abas.
        assert_eq!(highlight_windows(7, 1200.0, 300.0), 3);
        let r = highlight_layout(area(1200.0, 300.0), 3, HIGHLIGHT_RATIO);
        assert!(r[1..].iter().all(|s| s.height() >= MIN_H));
        // Nem altura para duas pequenas: uma só.
        assert_eq!(highlight_windows(7, 1200.0, 200.0), 2);
        // Área estreita (menos de duas colunas): a janela grande é a única.
        assert_eq!(highlight_windows(4, 2.0 * MIN_W + GAP - 1.0, 900.0), 1);
        assert_eq!(highlight_windows(4, 2.0 * MIN_W + GAP, 900.0), 4);
        // No limite, a grande e a coluna ficam com a largura mínima, sem passar.
        let r = highlight_layout(area(2.0 * MIN_W + GAP, 900.0), 2, HIGHLIGHT_RATIO);
        assert_eq!((r[0].width(), r[1].width()), (MIN_W, MIN_W));
        // Largura pequena com 2/3: a coluna não fica abaixo do mínimo.
        let r = highlight_layout(area(700.0, 900.0), 3, HIGHLIGHT_RATIO);
        assert!(r[1].width() >= MIN_W && r[0].width() >= MIN_W, "{r:?}");
        assert_eq!(highlight_windows(0, 700.0, 900.0), 0);
        assert!(highlight_layout(area(700.0, 900.0), 0, HIGHLIGHT_RATIO).is_empty());
    }

    #[test]
    fn the_featured_group_goes_first_and_the_rest_keep_their_order() {
        let order: Vec<String> = ["claude", "antigravity", "ollama", "codex"].map(String::from).to_vec();
        assert_eq!(highlight_order(&order, Some("ollama")), ["ollama", "claude", "antigravity", "codex"]);
        assert_eq!(highlight_order(&order, Some("claude")), order);
        // Sem selecionado, ou grupo que não existe: a ordem das janelas.
        assert_eq!(highlight_order(&order, None), order);
        assert_eq!(highlight_order(&order, Some("shell")), order);
    }

    #[test]
    fn the_highlight_divider_drags_within_the_limits() {
        assert_eq!(highlight_ratio(0.0), HIGHLIGHT_RATIO);
        assert_eq!(highlight_ratio(f32::NAN), HIGHLIGHT_RATIO);
        assert_eq!(highlight_ratio(1.5), HIGHLIGHT_RATIO);
        assert_eq!(highlight_ratio(0.5), 0.5);
        let mut r = 0.0;
        drag_highlight(&mut r, -100.0, 1006.0);
        assert!((r - (HIGHLIGHT_RATIO - 0.1)).abs() < 1e-3, "{r}");
        // Até o fim, cada lado mantém a largura mínima.
        drag_highlight(&mut r, 5000.0, 1006.0);
        assert!((r * 1000.0 - (1000.0 - MIN_W)).abs() < 1.0, "{r}");
        drag_highlight(&mut r, -5000.0, 1006.0);
        assert!((r * 1000.0 - MIN_W).abs() < 1.0, "{r}");
    }

    #[test]
    fn ratios_reset_when_the_shape_changes() {
        let mut r = Ratios::equal(&[2]);
        r.cols[0] = vec![0.7, 0.3];
        r.conform(&[2]);
        assert_eq!(r.cols[0], [0.7, 0.3]);
        r.conform(&[2, 1]);
        assert_eq!(r, Ratios::equal(&[2, 1]));
        assert!(!Ratios::default().fits(&[1]));
    }

    #[test]
    fn a_passing_shape_does_not_erase_the_saved_ratios() {
        let mut saved = Ratios::equal(&[2]);
        saved.cols[0] = vec![0.7, 0.3];
        // Janela estreitada: uma coluna só por um quadro; desenha com iguais e guarda as salvas.
        assert_eq!(saved.fitted(&[1, 1]), Ratios::equal(&[1, 1]));
        assert_eq!(saved.fitted(&[2]).cols[0], [0.7, 0.3]);
        assert_eq!(saved.cols[0], [0.7, 0.3]);
    }

    #[test]
    fn sizes_keep_the_minimum() {
        let half = (1001.0 - GAP) / 2.0;
        assert_eq!(sizes(1001.0, &[0.5, 0.5], 260.0), [half, half]);
        // Uma parte arrastada para quase nada volta ao mínimo.
        let s = sizes(1001.0, &[0.95, 0.05], 260.0);
        assert_eq!(s[1], 260.0);
        assert!((s[0] + s[1] - (1001.0 - GAP)).abs() < 0.01);
        // Sem espaço para o mínimo de todas: só proporcional.
        assert_eq!(sizes(301.0, &[0.5, 0.5], 260.0), [(301.0 - GAP) / 2.0; 2]);
    }

    #[test]
    fn dragging_a_divider_respects_both_sides() {
        let mut r = vec![0.5, 0.5];
        drag(&mut r, 0, 100.0, 1000.0, 260.0);
        assert!((r[0] - 0.6).abs() < 1e-6 && (r[1] - 0.4).abs() < 1e-6);
        drag(&mut r, 0, 900.0, 1000.0, 260.0);
        assert!((r[1] - 0.26).abs() < 1e-6);
        drag(&mut r, 0, -900.0, 1000.0, 260.0);
        assert!((r[0] - 0.26).abs() < 1e-6);
        // Divisor inexistente: nada muda.
        drag(&mut r, 1, 50.0, 1000.0, 260.0);
        assert!((r[0] + r[1] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn leftover_groups_share_the_last_window() {
        let g = ["claude", "antigravity", "ollama", "codex"];
        assert_eq!(assign(&g, 4), [vec!["claude"], vec!["antigravity"], vec!["ollama"], vec!["codex"]]);
        assert_eq!(assign(&g, 2), [vec!["claude"], vec!["antigravity", "ollama", "codex"]]);
        assert_eq!(assign(&g, 9).len(), 4);
        assert!(assign::<&str>(&[], 3).is_empty());
    }

    #[test]
    fn window_order_follows_the_open_groups() {
        let mut order = vec!["antigravity".to_owned(), "gone".to_owned()];
        sync_order(&mut order, ["claude", "antigravity", "ollama:qwen3.5:4b", "claude", "ollama:qwen3.5:9b"].into_iter());
        // A ordem salva vale; o grupo fechado sai; os novos entram no fim, na ordem das abas.
        assert_eq!(order, ["antigravity", "claude", "ollama"]);
        sync_order(&mut order, ["claude", "ollama"].into_iter());
        assert_eq!(order, ["claude", "ollama"]);
        sync_order(&mut order, std::iter::empty());
        assert!(order.is_empty());
    }

    #[test]
    fn tabs_of_a_window_keep_the_tab_order() {
        let keys = ["claude", "antigravity", "claude", "ollama", "codex"];
        assert_eq!(pane_tabs(&keys, &["claude".into()]), [0, 2]);
        // Janela com os grupos que sobraram: por grupo, cada um na ordem das abas.
        assert_eq!(pane_tabs(&keys, &["codex".into(), "antigravity".into()]), [4, 1]);
        assert!(pane_tabs(&keys, &["shell".into()]).is_empty());
    }

    #[test]
    fn closing_keeps_the_focus_in_the_same_window() {
        // Abas restantes depois de fechar um Claude: o outro Claude (usado há mais tempo) ganha.
        let kinds = ["claude", "antigravity", "claude"];
        assert_eq!(after_close(&kinds, &[3, 9, 1], "claude"), Some(0));
        // Ollama: os modelos são do mesmo grupo.
        let kinds = ["ollama:a", "claude"];
        assert_eq!(after_close(&kinds, &[0, 5], "ollama:b"), Some(0));
        // Último do grupo: a usada por último de todas.
        let kinds = ["claude", "antigravity", "codex"];
        assert_eq!(after_close(&kinds, &[4, 7, 2], "shell"), Some(1));
        assert_eq!(after_close(&[], &[], "claude"), None);
        // Nenhuma ativada ainda: a primeira.
        assert_eq!(most_recent(&[0, 0, 0], 0..3), Some(0));
    }

    #[test]
    fn neighbors_follow_the_arrow_keys() {
        // 2 em cima, 1 largo embaixo.
        let a = area(1001.0, 601.0);
        let rects = layout(a, &[2, 1], &Ratios::equal(&[2, 1]));
        assert_eq!(neighbor(&rects, 0, 1, 0), Some(1));
        assert_eq!(neighbor(&rects, 1, -1, 0), Some(0));
        assert_eq!(neighbor(&rects, 0, 0, 1), Some(2));
        assert_eq!(neighbor(&rects, 1, 0, 1), Some(2));
        assert_eq!(neighbor(&rects, 2, 0, -1), Some(0));
        assert_eq!(neighbor(&rects, 2, 1, 0), None);
        assert_eq!(neighbor(&rects, 0, -1, 0), None);
    }
}
