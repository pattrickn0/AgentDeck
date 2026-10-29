//! Syntax highlighting for Documents mode: the lexicon (`Tok`, `Span`, `Lang`, `highlight_lines`...) lives in the core
//! (`agentdeck_core::highlight`, also used by `ask-agent --outline`) and is re-exported here; what stays here is the UI:
//! the search in the text, the composition of the highlight with the search marks ([`pieces`]) and the cache per version
//! of the text ([`Cache`]). See `memory/aprendizados/editor-de-documentos.md`.

use std::ops::Range;

pub use agentdeck_core::highlight::{highlight_line, highlight_lines, Carry, Lang, Span, Tok};

// ---------------------------------------------------------------------------
// Search and composition
// ---------------------------------------------------------------------------

/// Where `query` appears in `text` (ASCII case-insensitive), in bytes. An empty query or one with a line break finds
/// nothing.
pub fn find_all(text: &str, query: &str) -> Vec<Range<usize>> {
    if query.is_empty() || query.contains('\n') {
        return Vec::new();
    }
    let hay = text.to_ascii_lowercase();
    let needle = query.to_ascii_lowercase();
    hay.match_indices(&needle).map(|(i, m)| i..i + m.len()).collect()
}

/// The lines (numbers starting at 0) that have `query`.
pub fn find_lines<'a>(lines: impl IntoIterator<Item = &'a str>, query: &str) -> Vec<usize> {
    if query.is_empty() {
        return Vec::new();
    }
    let needle = query.to_ascii_lowercase();
    lines.into_iter().enumerate().filter(|(_, l)| l.to_ascii_lowercase().contains(&needle)).map(|(n, _)| n).collect()
}

/// Search mark over a piece of text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    None,
    Found,
    Current,
}

/// A piece of `text` with a single color and a single mark (what becomes one section of the `LayoutJob`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    pub range: Range<usize>,
    pub tok: Tok,
    pub mark: Mark,
}

/// Joins the ranges per line (`lines`, from `highlight_lines` over the same `text`) and the searches (`matches`, in bytes
/// of `text`, in order; `current` is the position, in `matches`, of the current one) into contiguous pieces covering the
/// whole text.
pub fn pieces(text: &str, lines: &[Vec<Span>], matches: &[Range<usize>], current: Option<usize>) -> Vec<Piece> {
    // Colored ranges in bytes of `text`.
    let mut toks: Vec<(Range<usize>, Tok)> = Vec::new();
    let mut offset = 0;
    for (n, line) in text.split('\n').enumerate() {
        if let Some(spans) = lines.get(n) {
            toks.extend(spans.iter().map(|s| (offset + s.range.start..offset + s.range.end, s.tok)));
        }
        offset += line.len() + 1;
    }
    let mut cuts: Vec<usize> = vec![0, text.len()];
    cuts.extend(toks.iter().flat_map(|(r, _)| [r.start, r.end]));
    cuts.extend(matches.iter().flat_map(|r| [r.start, r.end]));
    cuts.retain(|&c| c <= text.len() && text.is_char_boundary(c));
    cuts.sort_unstable();
    cuts.dedup();
    let (mut t, mut m) = (0, 0);
    let mut out: Vec<Piece> = Vec::new();
    for w in cuts.windows(2) {
        let (a, z) = (w[0], w[1]);
        while t < toks.len() && toks[t].0.end <= a {
            t += 1;
        }
        while m < matches.len() && matches[m].end <= a {
            m += 1;
        }
        let tok = toks.get(t).filter(|(r, _)| r.start <= a).map_or(Tok::Plain, |(_, tok)| *tok);
        let mark = match matches.get(m).filter(|r| r.start <= a) {
            Some(_) if Some(m) == current => Mark::Current,
            Some(_) => Mark::Found,
            None => Mark::None,
        };
        match out.last_mut() {
            Some(last) if last.tok == tok && last.mark == mark => last.range.end = z,
            _ => out.push(Piece { range: a..z, tok, mark }),
        }
    }
    out
}

/// Fingerprint of a text: the key for what is kept per version of it (highlight, galley).
pub fn text_hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// The highlight of a text, kept by fingerprint and language: the editor asks every frame, and the file is only walked
/// again when the text changes (typing), not on every repaint.
#[derive(Debug, Clone, Default)]
pub struct Cache {
    key: Option<(u64, Lang)>,
    lines: Vec<Vec<Span>>,
    /// How many times the text was walked (for the test).
    pub runs: usize,
}

impl Cache {
    /// The ranges of each line of `text` (whose fingerprint is `hash`).
    pub fn lines(&mut self, lang: Lang, text: &str, hash: u64) -> &[Vec<Span>] {
        if self.key != Some((hash, lang)) {
            self.lines = highlight_lines(lang, text);
            self.key = Some((hash, lang));
            self.runs += 1;
        }
        &self.lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_ignores_ascii_case_and_returns_byte_ranges() {
        assert_eq!(find_all("Foo foo FOO", "foo"), [0..3, 4..7, 8..11]);
        assert_eq!(find_all("naïve naïve", "ï"), [2..4, 9..11]);
        assert!(find_all("abc", "").is_empty() && find_all("a\nb", "a\nb").is_empty());
        assert_eq!(find_lines(["one", "Two", "three"], "TW"), [1]);
    }

    #[test]
    fn pieces_cover_the_text_and_mix_colors_with_matches() {
        let text = "let x = 1;\nlet y";
        let lines = highlight_lines(Lang::Rust, text);
        let m = find_all(text, "let");
        let ps = pieces(text, &lines, &m, Some(1));
        // They cover the whole text, with no gap or overlap.
        assert_eq!(ps.first().map(|p| p.range.start), Some(0));
        assert_eq!(ps.last().map(|p| p.range.end), Some(text.len()));
        for w in ps.windows(2) {
            assert_eq!(w[0].range.end, w[1].range.start);
        }
        let at = |b: usize| ps.iter().find(|p| p.range.contains(&b)).expect("piece");
        assert_eq!((at(0).tok, at(0).mark), (Tok::Keyword, Mark::Found));
        assert_eq!((at(11).tok, at(11).mark), (Tok::Keyword, Mark::Current));
        assert_eq!((at(8).tok, at(8).mark), (Tok::Number, Mark::None));
        // With no search and no highlight, it is a single piece.
        assert_eq!(pieces("abc", &[], &[], None).len(), 1);
        assert!(pieces("", &[], &[], None).is_empty());
    }

    #[test]
    fn the_highlight_is_computed_once_per_version_of_the_text() {
        let mut cache = Cache::default();
        let text = "fn a() {}\n// end";
        let hash = text_hash(text);
        let first = cache.lines(Lang::Rust, text, hash).to_vec();
        assert_eq!(cache.lines(Lang::Rust, text, hash), first.as_slice());
        assert_eq!(cache.runs, 1, "the same text is not walked again");
        let edited = "fn b() {}\n// end";
        cache.lines(Lang::Rust, edited, text_hash(edited));
        assert_eq!(cache.runs, 2);
        // Another language for the same text recomputes it too.
        cache.lines(Lang::Python, edited, text_hash(edited));
        assert_eq!(cache.runs, 3);
        assert_ne!(text_hash("a"), text_hash("b"));
    }
}
