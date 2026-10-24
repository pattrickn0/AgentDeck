//! Realce de sintaxe do modo Documentos: o léxico (`Tok`, `Span`, `Lang`, `highlight_lines`...) mora no core
//! (`agentdeck_core::highlight`, também usado pelo `ask-agent --outline`) e é reexportado aqui; o que fica é da
//! interface: a busca no texto, a composição do realce com as marcas da busca ([`pieces`]) e o cache por versão do texto
//! ([`Cache`]). Ver `memory/aprendizados/editor-de-documentos.md`.

use std::ops::Range;

pub use agentdeck_core::highlight::{highlight_line, highlight_lines, Carry, Lang, Span, Tok};

// ---------------------------------------------------------------------------
// Busca e composição
// ---------------------------------------------------------------------------

/// Onde `query` aparece em `text` (sem diferença de maiúsculas e minúsculas ASCII), em bytes. Consulta vazia ou com
/// quebra de linha não acha nada.
pub fn find_all(text: &str, query: &str) -> Vec<Range<usize>> {
    if query.is_empty() || query.contains('\n') {
        return Vec::new();
    }
    let hay = text.to_ascii_lowercase();
    let needle = query.to_ascii_lowercase();
    hay.match_indices(&needle).map(|(i, m)| i..i + m.len()).collect()
}

/// As linhas (números a partir de 0) que têm `query`.
pub fn find_lines<'a>(lines: impl IntoIterator<Item = &'a str>, query: &str) -> Vec<usize> {
    if query.is_empty() {
        return Vec::new();
    }
    let needle = query.to_ascii_lowercase();
    lines.into_iter().enumerate().filter(|(_, l)| l.to_ascii_lowercase().contains(&needle)).map(|(n, _)| n).collect()
}

/// Marca de busca sobre um pedaço de texto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    None,
    Found,
    Current,
}

/// Um pedaço de `text` com uma só cor e uma só marca (o que vira uma seção do `LayoutJob`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    pub range: Range<usize>,
    pub tok: Tok,
    pub mark: Mark,
}

/// Junta as faixas por linha (`lines`, de `highlight_lines` sobre o mesmo `text`) e as buscas (`matches`, em bytes de
/// `text`, em ordem; `current` é a posição, em `matches`, da atual) em pedaços contíguos que cobrem o texto todo.
pub fn pieces(text: &str, lines: &[Vec<Span>], matches: &[Range<usize>], current: Option<usize>) -> Vec<Piece> {
    // Faixas coloridas em bytes de `text`.
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

/// Impressão digital de um texto: a chave do que se guarda por versão dele (realce, galley).
pub fn text_hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// O realce de um texto, guardado por impressão digital e linguagem: o editor pergunta a cada quadro, e o arquivo só é
/// percorrido de novo quando o texto muda (digitar) e não a cada repintura.
#[derive(Debug, Clone, Default)]
pub struct Cache {
    key: Option<(u64, Lang)>,
    lines: Vec<Vec<Span>>,
    /// Quantas vezes o texto foi percorrido (para o teste).
    pub runs: usize,
}

impl Cache {
    /// As faixas de cada linha de `text` (cuja impressão digital é `hash`).
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
        assert_eq!(find_all("ação Ação", "ão"), [3..6, 10..13]);
        assert!(find_all("abc", "").is_empty() && find_all("a\nb", "a\nb").is_empty());
        assert_eq!(find_lines(["um", "Dois", "tres"], "DO"), [1]);
    }

    #[test]
    fn pieces_cover_the_text_and_mix_colors_with_matches() {
        let text = "let x = 1;\nlet y";
        let lines = highlight_lines(Lang::Rust, text);
        let m = find_all(text, "let");
        let ps = pieces(text, &lines, &m, Some(1));
        // Cobrem o texto todo, sem buraco nem sobreposição.
        assert_eq!(ps.first().map(|p| p.range.start), Some(0));
        assert_eq!(ps.last().map(|p| p.range.end), Some(text.len()));
        for w in ps.windows(2) {
            assert_eq!(w[0].range.end, w[1].range.start);
        }
        let at = |b: usize| ps.iter().find(|p| p.range.contains(&b)).expect("pedaço");
        assert_eq!((at(0).tok, at(0).mark), (Tok::Keyword, Mark::Found));
        assert_eq!((at(11).tok, at(11).mark), (Tok::Keyword, Mark::Current));
        assert_eq!((at(8).tok, at(8).mark), (Tok::Number, Mark::None));
        // Sem busca nem realce, é um pedaço só.
        assert_eq!(pieces("abc", &[], &[], None).len(), 1);
        assert!(pieces("", &[], &[], None).is_empty());
    }

    #[test]
    fn the_highlight_is_computed_once_per_version_of_the_text() {
        let mut cache = Cache::default();
        let text = "fn a() {}\n// fim";
        let hash = text_hash(text);
        let first = cache.lines(Lang::Rust, text, hash).to_vec();
        assert_eq!(cache.lines(Lang::Rust, text, hash), first.as_slice());
        assert_eq!(cache.runs, 1, "o mesmo texto não é percorrido de novo");
        let edited = "fn b() {}\n// fim";
        cache.lines(Lang::Rust, edited, text_hash(edited));
        assert_eq!(cache.runs, 2);
        // Outra linguagem para o mesmo texto também refaz.
        cache.lines(Lang::Python, edited, text_hash(edited));
        assert_eq!(cache.runs, 3);
        assert_ne!(text_hash("a"), text_hash("b"));
    }
}
