//! Split screen by agent type: each group (`claude`, `antigravity`, `codex`…) gets a
//! window, and windows organize into an automatic grid (Split) or one large window with
//! the others stacked beside it (Highlight). Calculation only (no painting), with tests.

use egui::{Pos2, Rect};

/// Minimum size of a window (tab header + several terminal lines).
pub const MIN_W: f32 = 260.0;
pub const MIN_H: f32 = 140.0;
/// Gap between windows (islands): draggable divider lives in it.
pub const GAP: f32 = crate::theme::ISLAND_GAP;

/// Group (window) of a tab: its own kind (`claude@dev@ana` has `kind = "claude"`, so stays in
/// Claude's group).
pub fn group_key(kind: &str) -> &str {
    kind
}

/// How many windows per row for `n` groups in an area `w`×`h`. Up to 2 groups side by side; 3 =
/// 2 on top + 1 wide below; 4 = 2×2; 5 = 3+2… Respects minimum size: if not all fit,
/// yields fewer windows (leftover groups become tabs in the last one).
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

/// Fraction of width occupied by highlighted window by default (2/3 of center).
pub const HIGHLIGHT_RATIO: f32 = 2.0 / 3.0;

/// The effective Highlight ratio: the saved one (0 = none) if a valid fraction.
pub fn highlight_ratio(saved: f32) -> f32 {
    if saved.is_finite() && saved > 0.0 && saved < 1.0 {
        saved
    } else {
        HIGHLIGHT_RATIO
    }
}

/// How many windows Highlight draws for `n` groups in area `w`×`h`: the big one and the small
/// ones stacked beside it, each with minimum height. If area is too narrow for two columns,
/// it is one; small ones that do not fit leave leftover groups as tabs in the last one (`assign`).
pub fn highlight_windows(n: usize, w: f32, h: f32) -> usize {
    if n <= 1 || w < 2.0 * MIN_W + GAP {
        return n.min(1);
    }
    let max_small = ((h + GAP) / (MIN_H + GAP)).floor().max(1.0) as usize;
    1 + (n - 1).min(max_small)
}

/// Highlight rectangles for `windows` windows: the big one on the left (`ratio` of width) and the
/// others in the right column, with equal height and standard gap. A single window occupies whole area.
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

/// Order of groups in Highlight: selected agent's group first (the large window) and others in
/// window order, which does not change when the highlight switches.
pub fn highlight_order(order: &[String], featured: Option<&str>) -> Vec<String> {
    let Some(f) = featured.and_then(|f| order.iter().find(|g| g.as_str() == f)) else { return order.to_vec() };
    std::iter::once(f.clone()).chain(order.iter().filter(|g| *g != f).cloned()).collect()
}

/// Drags divider between big window and column by `delta` pixels (out of `total`, center width),
/// keeping both at minimum width.
pub fn drag_highlight(ratio: &mut f32, delta: f32, total: f32) {
    let first = highlight_ratio(*ratio);
    let mut pair = [first, 1.0 - first];
    drag(&mut pair, 0, delta, total - GAP, MIN_W);
    *ratio = pair[0];
}

/// Equal proportions for `n` parts.
pub fn equal(n: usize) -> Vec<f32> {
    vec![1.0 / n as f32; n]
}

/// Row and column proportions for each row. Resets to equal when grid shape changes
/// (a group is added or removed).
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

    /// Adjusts to current shape (keeps proportions if shape hasn't changed).
    pub fn conform(&mut self, shape: &[usize]) {
        if !self.fits(shape) {
            *self = Self::equal(shape);
        }
    }

    /// Proportions to draw current shape without saving: saved ones if they fit, otherwise equal.
    /// A temporary shape (window narrowed for a moment) does not wipe user settings.
    pub fn fitted(&self, shape: &[usize]) -> Self {
        if self.fits(shape) {
            self.clone()
        } else {
            Self::equal(shape)
        }
    }
}

/// Sizes of parts of `total` (excluding dividers) according to `ratios`, without leaving any
/// below `min` when there is room for all.
pub fn sizes(total: f32, ratios: &[f32], min: f32) -> Vec<f32> {
    let n = ratios.len();
    if n == 0 {
        return Vec::new();
    }
    let avail = (total - GAP * (n - 1) as f32).max(0.0);
    let sum: f32 = ratios.iter().sum();
    let mut out: Vec<f32> = ratios.iter().map(|r| if sum > 0.0 { avail * r / sum } else { avail / n as f32 }).collect();
    if avail >= min * n as f32 {
        // Parts below minimum increase; difference is proportionally taken from those with slack.
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

/// Window rectangles, row by row.
pub fn layout(area: Rect, shape: &[usize], ratios: &Ratios) -> Vec<Rect> {
    let heights = sizes(area.height(), &ratios.rows, MIN_H);
    let mut out = Vec::new();
    let mut y = area.min.y;
    for (r, &cols) in shape.iter().enumerate() {
        let h = heights[r];
        let widths = sizes(area.width(), &ratios.cols[r], MIN_W);
        let mut x = area.min.x;
        for (c, w) in widths.iter().enumerate() {
            // Last in each row/column touches border (no rounding residue).
            let x1 = if c + 1 == cols { area.max.x } else { (x + w).round() };
            let y1 = if r + 1 == shape.len() { area.max.y } else { (y + h).round() };
            out.push(Rect::from_min_max(Pos2::new(x.round(), y.round()), Pos2::new(x1, y1)));
            x = x1 + GAP;
        }
        y = (y + h).round() + GAP;
    }
    out
}

/// Drags divider between parts `i` and `i + 1` by `delta` pixels (out of `total`), respecting
/// minimum of both.
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

/// Assigns groups to windows: one group per window and, if not all fit, leftovers go
/// together in the last one.
pub fn assign<T: Clone>(groups: &[T], panes: usize) -> Vec<Vec<T>> {
    if groups.is_empty() || panes == 0 {
        return Vec::new();
    }
    let panes = panes.min(groups.len());
    let mut out: Vec<Vec<T>> = groups[..panes - 1].iter().map(|g| vec![g.clone()]).collect();
    out.push(groups[panes - 1..].to_vec());
    out
}

/// Keeps window order matching open groups (`kinds`, in tab order): a new group
/// is added at end and removed when its last tab closes.
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

/// Tabs for a window with groups `groups`, in tab order (`keys[i]` is tab `i`'s group).
pub fn pane_tabs(keys: &[&str], groups: &[String]) -> Vec<usize> {
    groups.iter().flat_map(|g| (0..keys.len()).filter(|&i| keys[i] == g).collect::<Vec<_>>()).collect()
}

/// Most recently activated tab among `candidates` (`shown[i]` is tab `i` counter;
/// tie: first).
pub fn most_recent(shown: &[u64], candidates: impl Iterator<Item = usize>) -> Option<usize> {
    candidates.max_by_key(|&i| (shown[i], std::cmp::Reverse(i)))
}

/// After closing the active tab (of group `closed`): the most recently used tab of same group, so
/// focus stays in same window; if none, the most recently used of all tabs.
pub fn after_close(kinds: &[&str], shown: &[u64], closed: &str) -> Option<usize> {
    let group = group_key(closed);
    most_recent(shown, (0..kinds.len()).filter(|&i| group_key(kinds[i]) == group)).or_else(|| most_recent(shown, 0..kinds.len()))
}

/// Neighbor window of `from` in direction (`dx`, `dy`): closest whose center lies towards that
/// side (and overlaps on other axis, if any).
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
        // Fits only one column: stacks.
        assert_eq!(grid_shape(3, 400.0, 700.0), [1, 1, 1]);
        // Not enough height for two: single window (other groups become tabs).
        assert_eq!(grid_shape(3, 400.0, 200.0), [1]);
        // Wide and low: side by side.
        assert_eq!(grid_shape(4, 1200.0, 200.0), [4]);
        assert_eq!(grid_shape(6, 1200.0, 200.0), [4]);
        // Minimum window (820px with both bars: ~339 × 440).
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
            assert!(gaps >= 0.0 && gaps < GAP * (3.0 * a.width() + 3.0 * a.height()), "n={n} leftover {gaps}");
            for (i, r) in rects.iter().enumerate() {
                assert!(a.contains_rect(*r), "n={n} {r:?}");
                for s in &rects[i + 1..] {
                    assert!(r.intersect(*s).area() <= 0.0, "n={n} {r:?} × {s:?}");
                }
            }
        }
        // 3 groups: two on top, one wide below.
        let shape = grid_shape(3, 1001.0, 601.0);
        let r = layout(a, &shape, &Ratios::equal(&shape));
        assert_eq!(r[2].width(), 1001.0);
        assert_eq!(r[0].width() + GAP + r[1].width(), 1001.0);
        assert!((r[0].width() - r[1].width()).abs() <= 1.0);
        // (601 - 6) / 2 = 297.5 for each line, rounded.
        assert_eq!(r[0].height(), 298.0);
        assert_eq!((r[2].min.y, r[2].max.y), (304.0, 601.0));
    }

    #[test]
    fn the_highlight_stacks_the_other_windows_beside_the_big_one() {
        let (w, h) = (1600.0, 900.0);
        // 1, 2, 4 and 7 CLIs: the big one plus small ones that fit height (900 → up to 6 of 140 px).
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
            // Big one occupies 2/3 width (less gap) on left, full height.
            assert_eq!((r[0].min, r[0].height()), (Pos2::ZERO, a.height()));
            assert!((r[0].width() - (w - GAP) * 2.0 / 3.0).abs() <= 1.0, "{:?}", r[0]);
            // Small ones stack in right column with standard gap, equal heights.
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
        // At 600 px height fits 4 small ones: with 7 CLIs, last 3 share the last as tabs.
        let windows = highlight_windows(7, w, 600.0);
        assert_eq!(windows, 5);
        assert_eq!(assign(&["a", "b", "c", "d", "e", "f", "g"], windows).last().map(Vec::len), Some(3));
    }

    #[test]
    fn the_highlight_respects_the_minimum_window_size() {
        // Fits only two small ones in 300 px height (2 × 140 + 6 = 286): remainder become tabs.
        assert_eq!(highlight_windows(7, 1200.0, 300.0), 3);
        let r = highlight_layout(area(1200.0, 300.0), 3, HIGHLIGHT_RATIO);
        assert!(r[1..].iter().all(|s| s.height() >= MIN_H));
        // Not enough height for two small ones: single small one.
        assert_eq!(highlight_windows(7, 1200.0, 200.0), 2);
        // Narrow area (less than two columns): big window is only one.
        assert_eq!(highlight_windows(4, 2.0 * MIN_W + GAP - 1.0, 900.0), 1);
        assert_eq!(highlight_windows(4, 2.0 * MIN_W + GAP, 900.0), 4);
        // At limit, big one and column take minimum width, without exceeding.
        let r = highlight_layout(area(2.0 * MIN_W + GAP, 900.0), 2, HIGHLIGHT_RATIO);
        assert_eq!((r[0].width(), r[1].width()), (MIN_W, MIN_W));
        // Small width with 2/3: column does not fall below minimum.
        let r = highlight_layout(area(700.0, 900.0), 3, HIGHLIGHT_RATIO);
        assert!(r[1].width() >= MIN_W && r[0].width() >= MIN_W, "{r:?}");
        assert_eq!(highlight_windows(0, 700.0, 900.0), 0);
        assert!(highlight_layout(area(700.0, 900.0), 0, HIGHLIGHT_RATIO).is_empty());
    }

    #[test]
    fn the_featured_group_goes_first_and_the_rest_keep_their_order() {
        let order: Vec<String> = ["claude", "antigravity", "opencode", "codex"].map(String::from).to_vec();
        assert_eq!(highlight_order(&order, Some("opencode")), ["opencode", "claude", "antigravity", "codex"]);
        assert_eq!(highlight_order(&order, Some("claude")), order);
        // None selected or nonexistent group: window order.
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
        // To the end, each side maintains minimum width.
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
        // Narrowed window: single column for one frame; draws with equal and keeps saved ones.
        assert_eq!(saved.fitted(&[1, 1]), Ratios::equal(&[1, 1]));
        assert_eq!(saved.fitted(&[2]).cols[0], [0.7, 0.3]);
        assert_eq!(saved.cols[0], [0.7, 0.3]);
    }

    #[test]
    fn sizes_keep_the_minimum() {
        let half = (1001.0 - GAP) / 2.0;
        assert_eq!(sizes(1001.0, &[0.5, 0.5], 260.0), [half, half]);
        // A part dragged to almost nothing returns to minimum.
        let s = sizes(1001.0, &[0.95, 0.05], 260.0);
        assert_eq!(s[1], 260.0);
        assert!((s[0] + s[1] - (1001.0 - GAP)).abs() < 0.01);
        // Not enough space for minimum of all: proportional only.
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
        // Nonexistent divider: nothing changes.
        drag(&mut r, 1, 50.0, 1000.0, 260.0);
        assert!((r[0] + r[1] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn leftover_groups_share_the_last_window() {
        let g = ["claude", "antigravity", "opencode", "codex"];
        assert_eq!(assign(&g, 4), [vec!["claude"], vec!["antigravity"], vec!["opencode"], vec!["codex"]]);
        assert_eq!(assign(&g, 2), [vec!["claude"], vec!["antigravity", "opencode", "codex"]]);
        assert_eq!(assign(&g, 9).len(), 4);
        assert!(assign::<&str>(&[], 3).is_empty());
    }

    #[test]
    fn window_order_follows_the_open_groups() {
        let mut order = vec!["antigravity".to_owned(), "gone".to_owned()];
        sync_order(&mut order, ["claude", "antigravity", "opencode", "claude", "opencode"].into_iter());
        // Saved order holds; closed group leaves; new groups enter at end in tab order.
        assert_eq!(order, ["antigravity", "claude", "opencode"]);
        sync_order(&mut order, ["claude", "opencode"].into_iter());
        assert_eq!(order, ["claude", "opencode"]);
        sync_order(&mut order, std::iter::empty());
        assert!(order.is_empty());
    }

    #[test]
    fn tabs_of_a_window_keep_the_tab_order() {
        let keys = ["claude", "antigravity", "claude", "opencode", "codex"];
        assert_eq!(pane_tabs(&keys, &["claude".into()]), [0, 2]);
        // Window with leftover groups: by group, each in tab order.
        assert_eq!(pane_tabs(&keys, &["codex".into(), "antigravity".into()]), [4, 1]);
        assert!(pane_tabs(&keys, &["shell".into()]).is_empty());
    }

    #[test]
    fn closing_keeps_the_focus_in_the_same_window() {
        // Remaining tabs after closing a Claude: the other Claude (used longer ago) wins.
        let kinds = ["claude", "antigravity", "claude"];
        assert_eq!(after_close(&kinds, &[3, 9, 1], "claude"), Some(0));
        // Last of the group: the most recently used overall.
        let kinds = ["claude", "antigravity", "codex"];
        assert_eq!(after_close(&kinds, &[4, 7, 2], "shell"), Some(1));
        assert_eq!(after_close(&[], &[], "claude"), None);
        // None activated yet: the first.
        assert_eq!(most_recent(&[0, 0, 0], 0..3), Some(0));
    }

    #[test]
    fn neighbors_follow_the_arrow_keys() {
        // 2 on top, 1 wide below.
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
