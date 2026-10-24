//! `ask-agent --outline <arquivo>...`: o esqueleto de um arquivo (assinaturas com o número da linha, sem os corpos), para o
//! agente ler só o trecho de que precisa em vez do arquivo inteiro.
//!
//! O léxico é o de [`crate::highlight`] (o mesmo que pinta o modo Documentos): as faixas de texto entre aspas e de comentário
//! saem do código antes de contar chaves e de procurar assinaturas, então `"{"`, `'}'`, `r#"fn x() {"#` e `// fn y() {` não
//! enganam. Não é um analisador de verdade: acha os itens de topo (e os membros de `impl`/`trait`/`mod`/`class`) pela
//! profundidade das chaves e pela palavra que abre a linha, o que basta para um índice.
//!
//! Nada aqui pode dar pânico com texto qualquer (o arquivo é do usuário): os cortes de texto passam por `get`/`chars`, e o
//! teste `any_text_gives_an_outline_without_panicking` confere.

use std::path::{Path, PathBuf};

use crate::highlight::{highlight_lines, Lang, Span, Tok};

/// Caracteres da resposta de um arquivo e da chamada inteira (como o `LEARN_BUDGET` do `--learn`).
pub const FILE_BUDGET: usize = 6_000;
pub const OUTLINE_BUDGET: usize = 16_000;
/// Passou disto o arquivo não é lido: vira uma linha de aviso.
const MAX_BYTES: u64 = 1024 * 1024;
/// Largura máxima do texto de um item (sem o número da linha).
const MAX_ITEM: usize = 150;
/// Linhas olhadas à procura do fim de uma assinatura que quebra em várias.
const MAX_SIGNATURE_LINES: usize = 30;
/// Reserva para a linha "… e mais N itens".
const TAIL_RESERVE: usize = 70;

/// Uma linha do resumo.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Item {
    /// Linha do arquivo, de 1 em diante.
    line: usize,
    /// Nível de aninhamento (cada um recua dois espaços).
    depth: usize,
    text: String,
}

/// O texto separado em linhas, com duas versões de cada: `code` sem texto entre aspas nem comentário (para achar chaves e
/// palavras) e `shown` só sem comentário (para mostrar). Têm o mesmo tamanho em bytes, então um índice de uma vale na outra.
struct Source<'a> {
    raw: Vec<&'a str>,
    code: Vec<String>,
    shown: Vec<String>,
}

impl<'a> Source<'a> {
    fn new(lang: Lang, text: &'a str) -> Source<'a> {
        let raw: Vec<&str> = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
        let spans = highlight_lines(lang, text);
        let blank = |line: &str, spans: &[Span], strings: bool| {
            let mut bytes = line.as_bytes().to_vec();
            for s in spans {
                if s.tok == Tok::Comment || (strings && s.tok == Tok::String) {
                    for b in bytes.get_mut(s.range.clone()).into_iter().flatten() {
                        *b = b' ';
                    }
                }
            }
            String::from_utf8_lossy(&bytes).into_owned()
        };
        let code = raw.iter().zip(&spans).map(|(l, s)| blank(l, s, true)).collect();
        let shown = raw.iter().zip(&spans).map(|(l, s)| blank(l, s, false)).collect();
        Source { raw, code, shown }
    }

    fn len(&self) -> usize {
        self.raw.len()
    }
}

// ---------- Texto ----------

/// Junta e limpa o que veio de várias linhas: espaços repetidos, `( a` e `a, )`.
fn collapse(s: &str) -> String {
    let joined = s.split_whitespace().collect::<Vec<_>>().join(" ");
    joined
        .replace("( ", "(")
        .replace(", )", ")")
        .replace(" )", ")")
        .replace("[ ", "[")
        .replace(" ]", "]")
        .trim_end_matches([',', ' '])
        .to_owned()
}

/// `s` com no máximo `max` caracteres; o que passa vira `…`.
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// A palavra (letras, dígitos e `_`) que abre `s`.
fn first_word(s: &str) -> &str {
    let end = s.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(s.len());
    &s[..end]
}

/// Profundidade de chaves no começo de cada linha (só no código: sem aspas nem comentário).
fn brace_depths(code: &[String]) -> Vec<usize> {
    let mut depth = 0usize;
    let mut out = Vec::with_capacity(code.len());
    for line in code {
        out.push(depth);
        for b in line.bytes() {
            match b {
                b'{' => depth += 1,
                b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    out
}

/// Profundidade de `(`, `[` e `{` no começo de cada linha (Python: linha dentro de parênteses é continuação).
fn bracket_depths(code: &[String]) -> Vec<usize> {
    let mut depth = 0usize;
    let mut out = Vec::with_capacity(code.len());
    for line in code {
        out.push(depth);
        for b in line.bytes() {
            match b {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    out
}

/// A assinatura que começa na linha `from`: o texto até o primeiro `stops` fora de parênteses, colchetes e chaves
/// (que podem quebrar linha). Devolve o texto limpo, a linha onde parou e qual `stops` achou (`None` se não achou até o
/// limite). Com `eol`, o fim da linha também para quando nada ficou aberto e a linha não termina num operador (JS sem `;`).
fn signature(src: &Source, from: usize, stops: &[&str], eol: bool) -> (String, usize, Option<usize>) {
    let mut text = String::new();
    let mut depth = 0usize;
    let last = (from + MAX_SIGNATURE_LINES).min(src.len());
    for k in from..last {
        let code = src.code[k].as_bytes();
        let shown = &src.shown[k];
        let mut i = 0;
        while i < code.len() {
            if depth == 0 {
                if let Some(s) = stops.iter().position(|s| code[i..].starts_with(s.as_bytes())) {
                    text.push_str(shown.get(..i).unwrap_or(""));
                    return (collapse(&text), k, Some(s));
                }
            }
            match code[i] {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
            i += 1;
        }
        text.push_str(shown);
        text.push(' ');
        if eol
            && depth == 0
            && !src.code[k].trim().is_empty()
            && !src.code[k].trim_end().ends_with([',', '=', '(', '+', '-', '*', '/', '&', '|', '?', ':', '.', '<'])
        {
            return (collapse(&text), k, None);
        }
    }
    (collapse(&text), last.saturating_sub(1).max(from), None)
}

/// Primeira frase do comentário de documentação (`///`), curta.
fn first_sentence(doc: &str) -> String {
    let doc = doc.trim();
    let cut = doc.find(". ").map_or(doc.len(), |i| i + 1);
    doc[..cut].trim_end_matches('.').trim().to_owned()
}

/// Acrescenta `// doc` ao fim de `text` se couber no que sobra da largura.
fn with_doc(text: String, doc: Option<&str>) -> String {
    let Some(doc) = doc.filter(|d| !d.is_empty()) else { return text };
    let room = MAX_ITEM.saturating_sub(text.chars().count() + 4);
    if room < 24 {
        return text;
    }
    format!("{text}  // {}", clip(doc, room.min(80)))
}

fn item(line: usize, depth: usize, text: String) -> Item {
    Item { line: line + 1, depth, text: clip(&text, MAX_ITEM) }
}

// ---------- Rust ----------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Fn,
    Struct,
    Enum,
    Trait,
    Impl,
    Mod,
    Type,
    Const,
    Static,
    Union,
    Macro,
    Use,
}

/// O tipo de item que a linha abre (`code` já sem aspas e comentários) e se é `pub`.
fn item_kind(t: &str) -> Option<(Kind, bool)> {
    if t.starts_with("macro_rules!") {
        return Some((Kind::Macro, false));
    }
    let mut rest = t;
    let mut public = false;
    loop {
        let w = first_word(rest);
        match w {
            "pub" => {
                public = true;
                rest = rest[3..].trim_start();
                if rest.starts_with('(') {
                    rest = rest[rest.find(')')? + 1..].trim_start();
                }
            }
            "unsafe" | "async" | "default" | "extern" => rest = rest[w.len()..].trim_start(),
            "const" => {
                let after = rest[5..].trim_start();
                if matches!(first_word(after), "fn" | "unsafe" | "async" | "extern") {
                    rest = after;
                } else {
                    break;
                }
            }
            _ => break,
        }
    }
    let kind = match first_word(rest) {
        "fn" => Kind::Fn,
        "struct" => Kind::Struct,
        "enum" => Kind::Enum,
        "trait" => Kind::Trait,
        "impl" => Kind::Impl,
        "mod" => Kind::Mod,
        "type" => Kind::Type,
        "const" => Kind::Const,
        "static" => Kind::Static,
        "union"
            if rest[5..].starts_with(|c: char| c.is_whitespace()) && rest[5..].trim_start().starts_with(|c: char| c.is_alphabetic()) =>
        {
            Kind::Union
        }
        "use" if public => Kind::Use,
        _ => return None,
    };
    Some((kind, public))
}

/// Quantos `#[test]` (ou `#[tokio::test]`...) há entre as linhas `from` e `to` (exclusive).
fn count_tests(code: &[String], from: usize, to: usize) -> usize {
    code.get(from..to.min(code.len()))
        .unwrap_or(&[])
        .iter()
        .filter(|l| {
            let t = l.trim();
            t.strip_prefix("#[").is_some_and(|a| {
                let name = a.split([']', '(']).next().unwrap_or("").trim();
                name.rsplit("::").next() == Some("test")
            })
        })
        .count()
}

/// Os nomes de campo de uma `struct` ou de variante de `enum` que moram nas linhas `from..` (até a chave fechar).
fn member_names(src: &Source, depths: &[usize], from: usize, body_depth: usize, variants: bool) -> Vec<String> {
    let mut names = Vec::new();
    for (code, &depth) in src.code.iter().zip(depths).skip(from) {
        if depth < body_depth {
            break;
        }
        if depth != body_depth {
            continue;
        }
        let mut t = code.trim();
        if t.starts_with('#') {
            continue;
        }
        if let Some(r) = t.strip_prefix("pub") {
            t = r.trim_start();
            if t.starts_with('(') {
                t = t.find(')').map_or("", |e| t[e + 1..].trim_start());
            }
        }
        let name = first_word(t);
        let after = t[name.len()..].trim_start();
        let ok = !name.is_empty()
            && !name.starts_with(|c: char| c.is_ascii_digit())
            && if variants { after.is_empty() || after.starts_with([',', '(', '{', '=']) } else { after.starts_with(':') };
        if ok {
            names.push(name.to_owned());
        }
        if names.len() >= 60 {
            break;
        }
    }
    names
}

/// `header { a, b, c }`, com os nomes cortados para a linha caber.
fn with_members(header: &str, names: &[String]) -> String {
    if names.is_empty() {
        return header.to_owned();
    }
    let room = MAX_ITEM.saturating_sub(header.chars().count() + 5);
    let mut list = String::new();
    for (n, name) in names.iter().enumerate() {
        let sep = if n == 0 { 0 } else { 2 };
        if list.chars().count() + sep + name.chars().count() > room.saturating_sub(3) {
            list.push_str(", …");
            break;
        }
        if n > 0 {
            list.push_str(", ");
        }
        list.push_str(name);
    }
    format!("{header} {{ {list} }}")
}

fn rust_items(src: &Source) -> Vec<Item> {
    let n = src.len();
    let depths = brace_depths(&src.code);
    let mut out = Vec::new();
    // Profundidade (de chaves) dos itens `impl`/`trait`/`mod` abertos: os membros moram um nível abaixo.
    let mut stack: Vec<usize> = Vec::new();
    let mut attrs: Vec<String> = Vec::new();
    let mut doc: Option<String> = None;
    let mut i = 0;
    while i < n {
        let d = depths[i];
        while stack.last().is_some_and(|&c| d <= c) {
            stack.pop();
        }
        let scope = stack.last().map_or(0, |c| c + 1);
        if d != scope {
            i += 1;
            continue;
        }
        let t = src.code[i].trim();
        let raw = src.raw[i].trim_start();
        if raw.starts_with("///") && !raw.starts_with("////") {
            if doc.is_none() {
                doc = Some(first_sentence(&raw[3..]));
            }
            i += 1;
            continue;
        }
        if t.is_empty() {
            if raw.is_empty() {
                doc = None;
                attrs.clear();
            }
            i += 1;
            continue;
        }
        if t.starts_with("#[") || t.starts_with("#![") {
            let inner = t.starts_with("#!");
            let mut text = String::new();
            let mut open = 0i32;
            let mut k = i;
            while k < n {
                text.push_str(src.code[k].trim());
                open += src.code[k].bytes().map(|b| i32::from(b == b'[') - i32::from(b == b']')).sum::<i32>();
                if open <= 0 || k + 1 - i >= MAX_SIGNATURE_LINES {
                    break;
                }
                k += 1;
            }
            if !inner {
                attrs.push(collapse(&text));
            }
            i = k + 1;
            continue;
        }
        let Some((kind, _)) = item_kind(t) else {
            attrs.clear();
            doc = None;
            i += 1;
            continue;
        };
        let stops: &[&str] = if kind == Kind::Use { &[";"] } else { &["{", ";"] };
        let (sig, j, stop) = signature(src, i, stops, false);
        let opens_body = stop == Some(0);
        let indent = stack.len();
        let is_test_fn = attrs.iter().any(|a| a == "#[test]" || a.ends_with("::test]"));
        let cfg_test = attrs.iter().any(|a| a.replace(' ', "") == "#[cfg(test)]");
        let name_is_tests = sig.split_whitespace().last() == Some("tests");
        let mut text = sig.clone();
        let mut container = false;
        match kind {
            Kind::Fn if is_test_fn => text.clear(),
            Kind::Const | Kind::Static => {
                if let Some((head, _)) = sig.split_once('=') {
                    text = head.trim().to_owned();
                }
            }
            Kind::Struct | Kind::Union if opens_body => {
                let names = member_names(src, &depths, j + 1, d + 1, false);
                text = with_members(&sig, &names);
            }
            Kind::Enum if opens_body => {
                let names = member_names(src, &depths, j + 1, d + 1, true);
                text = with_members(&sig, &names);
            }
            Kind::Mod if opens_body && (cfg_test || name_is_tests) => {
                let end = (j + 1..n).find(|&k| depths[k] <= d).unwrap_or(n);
                let tests = count_tests(&src.code, j + 1, end);
                text = format!(
                    "{}{sig} — {tests} {}",
                    if cfg_test { "#[cfg(test)] " } else { "" },
                    if tests == 1 { "teste" } else { "testes" }
                );
            }
            Kind::Mod | Kind::Impl | Kind::Trait if opens_body => container = true,
            _ => {}
        }
        if !text.is_empty() {
            out.push(item(i, indent, with_doc(text, doc.as_deref())));
        }
        if container {
            stack.push(d);
        }
        attrs.clear();
        doc = None;
        i = j + 1;
    }
    out
}

// ---------- JS / TS ----------

const JS_NOT_METHODS: &[&str] =
    &["if", "for", "while", "switch", "catch", "return", "function", "with", "else", "do", "try", "new", "await"];
const JS_MODIFIERS: &[&str] =
    &["export", "default", "declare", "abstract", "public", "private", "protected", "static", "readonly", "override"];

fn js_items(src: &Source) -> Vec<Item> {
    let n = src.len();
    let depths = brace_depths(&src.code);
    let mut out = Vec::new();
    // Profundidade dos `class` (true) e `namespace` (false) abertos.
    let mut stack: Vec<(usize, bool)> = Vec::new();
    let mut i = 0;
    while i < n {
        let d = depths[i];
        while stack.last().is_some_and(|&(c, _)| d <= c) {
            stack.pop();
        }
        let scope = stack.last().map_or(0, |(c, _)| c + 1);
        let t = src.code[i].trim();
        if d != scope || t.is_empty() || t.starts_with('@') {
            i += 1;
            continue;
        }
        let in_class = stack.last().is_some_and(|&(_, class)| class);
        let indent = stack.len();
        // Palavras que abrem a linha, sem os modificadores.
        let mut rest = t;
        let mut exported = false;
        loop {
            let w = first_word(rest);
            if JS_MODIFIERS.contains(&w) && !w.is_empty() && !rest[w.len()..].trim_start().starts_with(['(', '=', ':', ';', '<']) {
                exported |= w == "export";
                rest = rest[w.len()..].trim_start();
            } else {
                break;
            }
        }
        let body = match rest.strip_prefix("async") {
            Some(r) if r.starts_with(char::is_whitespace) => r.trim_start(),
            _ => rest,
        };
        let word = first_word(body);
        if in_class {
            // Método (ou propriedade com função) dentro de uma classe.
            let name = body.trim_start_matches(['*', '#']);
            let ident = first_word(name);
            let after = name[ident.len()..].trim_start();
            let method = !ident.is_empty() && !JS_NOT_METHODS.contains(&ident) && after.starts_with(['(', '<']);
            let arrow_prop = !ident.is_empty() && after.starts_with('=') && !after.starts_with("==");
            if method || arrow_prop {
                let (sig, j, stop) = signature(src, i, &["{", ";", "=>"], true);
                if method || stop == Some(2) || sig.contains("function") {
                    out.push(item(i, indent, if stop == Some(2) { format!("{sig} =>") } else { sig }));
                }
                i = j + 1;
                continue;
            }
            i += 1;
            continue;
        }
        let container = matches!(word, "class" | "namespace" | "module");
        let header = matches!(word, "function" | "class" | "interface" | "enum" | "namespace" | "module")
            || (word == "type" && body[4..].trim_start().starts_with(|c: char| c.is_alphabetic()));
        if header {
            if word == "type" {
                out.push(item(i, indent, collapse(&src.shown[i])));
                i += 1;
                continue;
            }
            let (sig, j, stop) = signature(src, i, &["{", ";"], true);
            let mut j = j;
            let mut stop = stop;
            if stop.is_none()
                && (j + 1..n.min(j + 3))
                    .find(|&k| !src.code[k].trim().is_empty())
                    .is_some_and(|k| src.code[k].trim_start().starts_with('{'))
            {
                // `{` na linha de baixo.
                j = (j + 1..n).find(|&k| !src.code[k].trim().is_empty()).unwrap_or(j);
                stop = Some(0);
            }
            out.push(item(i, indent, sig));
            if container && stop == Some(0) {
                stack.push((d, word == "class"));
            }
            i = j + 1;
            continue;
        }
        if matches!(word, "const" | "let" | "var") {
            let (sig, j, stop) = signature(src, i, &["=>", "{", ";"], true);
            let name = first_word(body[word.len()..].trim_start());
            let init = sig.split_once('=').map_or("", |(_, r)| r.trim_start());
            let function_like = stop == Some(0) || init.starts_with("function") || init.starts_with("async function");
            if !name.is_empty() && (exported || function_like) {
                let text = if function_like {
                    if stop == Some(0) {
                        format!("{sig} =>")
                    } else {
                        sig
                    }
                } else {
                    sig.split_once('=').map_or(sig.clone(), |(h, _)| h.trim().to_owned())
                };
                out.push(item(i, indent, text));
                i = j + 1;
            } else {
                i += 1;
            }
            continue;
        }
        if exported {
            // `export { a, b }`, `export * from "x"`, `export default x`.
            out.push(item(i, indent, collapse(&src.shown[i])));
        } else if t.contains(" = function") || t.contains(" = async function") {
            // `module.exports.x = function`, `Foo.prototype.bar = function`.
            let (sig, j, _) = signature(src, i, &["{", ";"], true);
            out.push(item(i, indent, sig));
            i = j + 1;
            continue;
        }
        i += 1;
    }
    out
}

// ---------- Python ----------

fn indent_of(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ' || *c == '\t').map(|c| if c == '\t' { 4 } else { 1 }).sum()
}

fn python_items(src: &Source) -> Vec<Item> {
    let n = src.len();
    let nest = bracket_depths(&src.code);
    let mut out = Vec::new();
    // (recuo, é função) do que está aberto.
    let mut stack: Vec<(usize, bool)> = Vec::new();
    let mut i = 0;
    while i < n {
        let t = src.code[i].trim();
        if t.is_empty() || nest[i] > 0 {
            i += 1;
            continue;
        }
        let ind = indent_of(&src.code[i]);
        while stack.last().is_some_and(|&(c, _)| ind <= c) {
            stack.pop();
        }
        let body = match t.strip_prefix("async ") {
            Some(r) => r.trim_start(),
            None => t,
        };
        let is_def = body.starts_with("def ");
        if is_def || body.starts_with("class ") {
            let (sig, j, _) = signature(src, i, &[":"], false);
            if !stack.iter().any(|&(_, def)| def) {
                out.push(item(i, stack.len(), sig));
            }
            stack.push((ind, is_def));
            i = j + 1;
            continue;
        }
        i += 1;
    }
    out
}

// ---------- Markdown ----------

fn markdown_items(src: &Source) -> Vec<Item> {
    let mut out = Vec::new();
    let mut fence: Option<char> = None;
    let mut front = src.raw.first().is_some_and(|l| l.trim_end() == "---");
    for (i, line) in src.raw.iter().enumerate() {
        let t = line.trim_start();
        if front {
            if i > 0 && line.trim_end() == "---" {
                front = false;
            }
            continue;
        }
        if let Some(c) = t.chars().next().filter(|c| matches!(c, '`' | '~')) {
            if t.starts_with(&c.to_string().repeat(3)) {
                match fence {
                    Some(f) if f == c => fence = None,
                    None => fence = Some(c),
                    _ => {}
                }
                continue;
            }
        }
        if fence.is_some() || line.len() - t.len() > 3 {
            continue;
        }
        let level = t.chars().take_while(|c| *c == '#').count();
        if (1..=3).contains(&level) && t[level..].starts_with(' ') {
            out.push(item(i, level - 1, t.trim_end().to_owned()));
        }
    }
    out
}

// ---------- PowerShell / shell ----------

fn script_items(src: &Source, powershell: bool) -> Vec<Item> {
    let mut out = Vec::new();
    for (i, code) in src.code.iter().enumerate() {
        let t = code.trim();
        let lower = t.to_ascii_lowercase();
        let keyword = if lower.starts_with("function ") {
            Some(8)
        } else if powershell && lower.starts_with("filter ") {
            Some(6)
        } else {
            None
        };
        if let Some(k) = keyword {
            let cut = t.find('{').unwrap_or(t.len());
            let shown = src.shown[i].trim();
            let shown = shown.get(..cut.min(shown.len())).unwrap_or(shown);
            if !t[k..].trim().is_empty() {
                out.push(item(i, 0, collapse(shown)));
            }
        } else if !powershell {
            // `nome() {` do shell.
            if let Some(pos) = t.find("()") {
                let name = t[..pos].trim();
                if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-')) {
                    out.push(item(i, 0, format!("{name}()")));
                }
            }
        }
    }
    out
}

// ---------- Resumo de um arquivo ----------

/// Os itens de `text` na linguagem de `lang`; `None` se não há resumo para ela.
fn items(lang: Lang, text: &str) -> Option<Vec<Item>> {
    match lang {
        Lang::Rust => Some(rust_items(&Source::new(lang, text))),
        Lang::JsTs => Some(js_items(&Source::new(lang, text))),
        Lang::Python => Some(python_items(&Source::new(lang, text))),
        Lang::Markdown => Some(markdown_items(&Source::new(lang, text))),
        Lang::PowerShell => Some(script_items(&Source::new(lang, text), true)),
        Lang::Shell => Some(script_items(&Source::new(lang, text), false)),
        Lang::Plain | Lang::Toml | Lang::Json | Lang::Yaml => None,
    }
}

/// O resumo de um arquivo: o cabeçalho (caminho, linhas, linguagem) e os itens com o número da linha, até `budget`
/// caracteres; o que passa vira "… e mais N itens (linhas X–Y)".
pub fn outline(path: &str, text: &str, budget: usize) -> String {
    let lang = Lang::from_path(path);
    let lines = text.split('\n').count() - usize::from(text.ends_with('\n'));
    let head = format!("{path} — {lines} linha{}, {}", if lines == 1 { "" } else { "s" }, lang.label());
    let Some(items) = items(lang, text) else {
        let what = if lang == Lang::Plain { "este tipo de arquivo".to_owned() } else { lang.label().to_owned() };
        return format!("{head}\n      (sem resumo para {what}; leia o arquivo, são {lines} linhas)");
    };
    if items.is_empty() {
        return format!("{head}\n      (nenhum item encontrado)");
    }
    let mut out = head;
    let limit = budget.saturating_sub(TAIL_RESERVE);
    let mut shown = 0;
    for it in &items {
        let line = format!("\n{:>5}  {}{}", it.line, "  ".repeat(it.depth), it.text);
        if out.chars().count() + line.chars().count() > limit && shown > 0 {
            break;
        }
        out.push_str(&line);
        shown += 1;
    }
    if shown < items.len() {
        let rest = &items[shown..];
        let (first, last) = (rest[0].line, rest[rest.len() - 1].line);
        out.push_str(&format!(
            "\n      … e mais {} {} (linhas {first}–{last})",
            rest.len(),
            if rest.len() == 1 { "item" } else { "itens" }
        ));
    }
    out
}

// ---------- Core ----------

/// `rel` já normalizado (barras `/`) sem `..` e dentro da raiz? (`..`, caminho absoluto e letra de unidade saem.)
fn escapes_root(rel: &str) -> bool {
    rel.split('/').any(|c| c == "..") || rel.starts_with('/') || rel.as_bytes().get(1) == Some(&b':')
}

/// O resumo de `rel` dentro de `root`, ou a linha de aviso do porquê não deu.
fn outline_path(root: &Path, canonical_root: &Path, rel: &str, budget: usize) -> String {
    if escapes_root(rel) {
        return format!("{rel} — recusado: o caminho sai da raiz do projeto");
    }
    let full: PathBuf = root.join(rel);
    let Ok(canonical) = full.canonicalize() else { return format!("{rel} — não existe") };
    if !canonical.starts_with(canonical_root) {
        return format!("{rel} — recusado: o caminho sai da raiz do projeto");
    }
    let Ok(meta) = std::fs::metadata(&canonical) else { return format!("{rel} — não consegui ler") };
    if !meta.is_file() {
        return format!("{rel} — é uma pasta, não um arquivo");
    }
    if meta.len() > MAX_BYTES {
        return format!("{rel} — grande demais para resumir ({} KB; o limite é {} KB)", meta.len() / 1024, MAX_BYTES / 1024);
    }
    let Ok(raw) = std::fs::read(&canonical) else { return format!("{rel} — não consegui ler") };
    if raw.iter().take(8192).any(|b| *b == 0) && !raw.starts_with(&[0xFF, 0xFE]) && !raw.starts_with(&[0xFE, 0xFF]) {
        return format!("{rel} — arquivo binário");
    }
    outline(rel, &crate::util::decode_text(&raw), budget)
}

impl crate::Core {
    /// Resposta de `ask-agent --outline`: o esqueleto de cada arquivo de `paths`. Lê da worktree da tarefa de `instance`
    /// (onde o agente trabalha) ou, sem tarefa, do repositório do projeto. Arquivo que não existe, binário, grande demais
    /// ou fora da raiz vira uma linha de aviso, não erro.
    pub fn outline(&self, project: &str, instance: &str, paths: &[String]) -> Result<String, String> {
        if paths.is_empty() {
            return Err("informe ao menos um arquivo: ask-agent --outline <arquivo>...".into());
        }
        let project_root = self.project_path(project).and_then(|p| self.git_repo_at(&p).ok()).map(|r| r.root);
        let work_root = self.task_cwd(project, instance).or_else(|| project_root.clone());
        let Some(work_root) = work_root else {
            return Err("não achei o repositório do projeto: o resumo lê os arquivos dele".into());
        };
        let roots: Vec<PathBuf> = [Some(work_root.clone()), project_root].into_iter().flatten().collect();
        let canonical_root = work_root.canonicalize().unwrap_or_else(|_| work_root.clone());
        let mut parts = Vec::new();
        let mut left = OUTLINE_BUDGET;
        for p in paths {
            let rel = crate::learnings::relativize(p, &roots);
            if rel.is_empty() {
                continue;
            }
            if left < 400 {
                parts.push(format!("{rel} — fora do orçamento desta resposta; peça este arquivo sozinho"));
                continue;
            }
            let part = outline_path(&work_root, &canonical_root, &rel, FILE_BUDGET.min(left));
            left = left.saturating_sub(part.chars().count() + 2);
            parts.push(part);
        }
        Ok(parts.join("\n\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(items: &[Item]) -> Vec<String> {
        items.iter().map(|i| format!("{}:{}{}", i.line, "  ".repeat(i.depth), i.text)).collect()
    }

    fn rust(text: &str) -> Vec<String> {
        texts(&items(Lang::Rust, text).expect("Rust tem resumo"))
    }

    #[test]
    fn rust_top_level_items_have_signatures_and_lines_without_bodies() {
        let src = "use std::io;\npub use crate::a::{b, c};\n\n/// Faz a coisa. Detalhes.\npub fn work(x: u32) -> u32 {\n    let y = x + 1;\n    fn inner() {}\n    y\n}\n\npub const MAX: usize = 10;\nstatic NAME: &str = \"a { b\";\ntype Alias = Vec<u8>;\nmacro_rules! m {\n    () => {};\n}\n";
        assert_eq!(
            rust(src),
            [
                "2:pub use crate::a::{b, c}",
                "5:pub fn work(x: u32) -> u32  // Faz a coisa",
                "11:pub const MAX: usize",
                "12:static NAME: &str",
                "13:type Alias = Vec<u8>",
                "14:macro_rules! m",
            ]
        );
    }

    #[test]
    fn braces_inside_strings_comments_chars_and_raw_strings_do_not_count() {
        let src = "fn a() {\n    let s = \"}\";\n    let c = '}';\n    // }\n    /* } */\n    let r = r#\"}\n}\"#;\n}\nfn b() {}\nfn c() {\n    let lt: &'static str = \"{\";\n}\n";
        assert_eq!(rust(src), ["1:fn a()", "9:fn b()", "10:fn c()"]);
    }

    #[test]
    fn a_signature_over_several_lines_becomes_one_line() {
        let src = "pub fn long(\n    a: u32,\n    b: &str, // dois\n) -> Result<(), String>\nwhere\n    T: Clone,\n{\n    todo!()\n}\n";
        assert_eq!(rust(src), ["1:pub fn long(a: u32, b: &str) -> Result<(), String> where T: Clone"]);
    }

    #[test]
    fn impl_and_trait_members_are_indented_under_them() {
        let src = "impl Core {\n    pub fn new() -> Self {\n        Self\n    }\n\n    /// Lê.\n    pub(crate) async fn read(&self, p: &Path) -> String {\n        String::new()\n    }\n    const ID: u8 = 1;\n}\n\npub trait Tr {\n    type Out;\n    fn go(&self);\n}\n\nimpl Tr for Core {\n    fn go(&self) {}\n}\n";
        assert_eq!(
            rust(src),
            [
                "1:impl Core",
                "2:  pub fn new() -> Self",
                "7:  pub(crate) async fn read(&self, p: &Path) -> String  // Lê",
                "10:  const ID: u8",
                "13:pub trait Tr",
                "14:  type Out",
                "15:  fn go(&self)",
                "18:impl Tr for Core",
                "19:  fn go(&self)",
            ]
        );
    }

    #[test]
    fn struct_fields_and_enum_variants_are_listed_by_name() {
        let src = "pub struct Learning {\n    pub file: String,\n    pub(crate) titulo: String,\n    touches: Vec<String>,\n}\n\npub enum Kind {\n    A,\n    B(u32),\n    C { x: u8 },\n    D = 4,\n}\n\nstruct Unit;\nstruct Tuple(u8, u8);\nstruct Empty {}\n";
        assert_eq!(
            rust(src),
            [
                "1:pub struct Learning { file, titulo, touches }",
                "7:pub enum Kind { A, B, C, D }",
                "14:struct Unit",
                "15:struct Tuple(u8, u8)",
                "16:struct Empty",
            ]
        );
    }

    #[test]
    fn a_long_list_of_fields_is_cut_to_fit_the_line() {
        let fields: String = (0..60).map(|i| format!("    campo_numero_{i}: u8,\n")).collect();
        let lines = rust(&format!("struct Big {{\n{fields}}}\n"));
        assert_eq!(lines.len(), 1);
        assert!(lines[0].chars().count() <= MAX_ITEM + 4 && lines[0].ends_with(", … }"), "{lines:?}");
    }

    #[test]
    fn cfg_test_module_becomes_one_line_with_the_number_of_tests() {
        let src = "fn a() {}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn one() {\n        assert!(true);\n    }\n\n    #[test]\n    fn two() {}\n\n    #[tokio::test]\n    async fn three() {}\n\n    fn helper() {}\n}\n";
        assert_eq!(rust(src), ["1:fn a()", "4:#[cfg(test)] mod tests — 3 testes"]);
        assert_eq!(rust("#[cfg(test)]\nmod tests {\n    #[test]\n    fn only() {}\n}\n"), ["2:#[cfg(test)] mod tests — 1 teste"]);
    }

    #[test]
    fn inline_modules_list_their_members_and_attributes_over_several_lines_are_skipped() {
        let src = "#[derive(\n    Debug,\n    Clone,\n)]\npub struct S {\n    a: u8,\n}\n\npub mod inner {\n    pub fn f() {}\n}\n\nmod external;\n\n#[test]\nfn top_level_test() {}\n";
        assert_eq!(rust(src), ["5:pub struct S { a }", "9:pub mod inner", "10:  pub fn f()", "13:mod external"]);
    }

    #[test]
    fn code_after_a_container_goes_back_to_the_top_level() {
        let src = "impl A {\n    fn x() {}\n}\nfn after() {}\nimpl B {}\nfn after_empty() {}\n";
        assert_eq!(rust(src), ["1:impl A", "2:  fn x()", "4:fn after()", "5:impl B", "6:fn after_empty()"]);
    }

    #[test]
    fn python_lists_defs_and_classes_by_indentation_without_nested_functions() {
        let src = "import os\n\n@decorator\ndef top(a, b=1):\n    def inner():\n        pass\n    return 1\n\nclass Foo(Base):\n    \"\"\"def fake(self):\"\"\"\n    x = 1\n\n    def method(self,\n               y: int) -> int:\n        return y\n\n    async def amethod(self):\n        pass\n\n    class Nested:\n        def deep(self): pass\n\ndef last(): ...\n";
        let got = texts(&items(Lang::Python, src).expect("Python tem resumo"));
        assert_eq!(
            got,
            [
                "4:def top(a, b=1)",
                "9:class Foo(Base)",
                "13:  def method(self, y: int) -> int",
                "17:  async def amethod(self)",
                "20:  class Nested",
                "21:    def deep(self)",
                "23:def last()",
            ]
        );
    }

    #[test]
    fn markdown_gives_the_headings_with_their_lines_and_ignores_code_fences() {
        let src = "---\ntitulo: x\n# nao e titulo\n---\n# Um\n\ntexto\n\n## Dois\n\n```sh\n# comentario\n```\n\n### Tres\n\n#### Quatro\n####Sem espaco\n";
        let got = texts(&items(Lang::Markdown, src).expect("Markdown tem resumo"));
        assert_eq!(got, ["5:# Um", "9:  ## Dois", "15:    ### Tres"]);
    }

    #[test]
    fn javascript_lists_functions_classes_methods_and_exports() {
        let src = "import x from \"y\";\n\nexport async function load(a, b) {\n  const s = \"}\";\n  return s;\n}\n\nexport const arrow = async (a) => {\n  return a;\n};\nconst small = x => x + 1;\nconst data = { a: 1 };\nexport const VALUE = 42;\n\nclass Foo extends Bar {\n  constructor(a) {\n    super(a);\n  }\n\n  static create() {\n    if (x) {\n      return 1;\n    }\n  }\n\n  handler = (e) => {\n    e.stop();\n  };\n}\n\nexport interface Opts {\n  a: string;\n}\n\nexport type Mode = \"a\" | \"b\";\n\nfunction* gen() {}\n";
        let got = texts(&items(Lang::JsTs, src).expect("JS tem resumo"));
        assert_eq!(
            got,
            [
                "3:export async function load(a, b)",
                "8:export const arrow = async (a) =>",
                "11:const small = x =>",
                "13:export const VALUE",
                "15:class Foo extends Bar",
                "16:  constructor(a)",
                "20:  static create()",
                "26:  handler = (e) =>",
                "31:export interface Opts",
                "35:export type Mode = \"a\" | \"b\";",
                "37:function* gen()",
            ]
        );
    }

    #[test]
    fn powershell_and_shell_list_functions() {
        let ps = "function Get-Thing {\n    param($a)\n}\n# function Fake {\nFunction Other($x) { 1 }\n";
        assert_eq!(texts(&items(Lang::PowerShell, ps).expect("PS tem resumo")), ["1:function Get-Thing", "5:Function Other($x)"]);
        let sh = "foo() {\n  echo hi\n}\nfunction bar {\n  :\n}\nbaz () { :; }\necho \"x()\"\n";
        assert_eq!(texts(&items(Lang::Shell, sh).expect("Shell tem resumo")), ["1:foo()", "4:function bar", "7:baz()"]);
    }

    #[test]
    fn a_language_without_outline_says_so_with_the_number_of_lines() {
        let out = outline("Cargo.toml", "[package]\nname = \"x\"\n", FILE_BUDGET);
        assert!(
            out.starts_with("Cargo.toml — 2 linhas, TOML") && out.contains("sem resumo para TOML") && out.contains("2 linhas"),
            "{out}"
        );
        let out = outline("a.html", "<p>x</p>", FILE_BUDGET);
        assert!(out.contains("sem resumo para este tipo de arquivo"), "{out}");
    }

    #[test]
    fn the_outline_has_a_header_and_numbered_lines() {
        let out = outline("src/a.rs", "pub fn a() {}\n\nfn b() {}\n", FILE_BUDGET);
        assert_eq!(out, "src/a.rs — 3 linhas, Rust\n    1  pub fn a()\n    3  fn b()");
        assert_eq!(outline("src/a.rs", "", FILE_BUDGET), "src/a.rs — 1 linha, Rust\n      (nenhum item encontrado)");
    }

    #[test]
    fn the_budget_cuts_the_items_and_reports_what_was_left_out() {
        let text: String = (1..=40).map(|i| format!("fn funcao_numero_{i}() {{}}\n")).collect();
        let out = outline("a.rs", &text, 600);
        assert!(out.chars().count() <= 600, "{}", out.chars().count());
        let tail = out.lines().last().expect("última linha");
        assert!(tail.contains("… e mais ") && tail.contains("itens (linhas ") && tail.ends_with("–40)"), "{tail}");
        // Dentro do orçamento, nada é cortado.
        assert!(!outline("a.rs", &text, 10_000).contains("… e mais"));
    }

    #[test]
    fn doc_comments_only_fit_when_the_line_has_room() {
        let long = format!("/// Uma frase.\npub fn {}() {{}}\n", "n".repeat(135));
        assert!(!rust(&long)[0].contains("//"), "{:?}", rust(&long));
        assert!(rust("/// Uma frase. Outra.\nfn a() {}\n")[0].ends_with("fn a()  // Uma frase"));
    }

    #[test]
    fn any_text_gives_an_outline_without_panicking() {
        let nasty = [
            "",
            "ção \"não fechada 中文 🙂 /* x",
            "'é' 'ab 'ç' r#\"ü\" \\ \"\\",
            "fn é(",
            "pub fn 🙂(a: \"x\" {{{{ } } }",
            "impl<'a> X for Y<'a> {",
            "struct S { é: u8, 🙂",
            "enum E { 中 }",
            "#[cfg(test)] mod tests { #[test",
            "#[derive(",
            "class é {\n  ç(\n",
            "def é(\n    x,",
            "```\n# x\n---\n#",
            "function* (\n",
            "const é = (",
            "}}}}\n)))\n]]]]",
            "pub(\npub(crate\nconst\nunion\nunion é\nextern",
            "\u{feff}fn x() {}\r\nfn y() {}\r\n",
        ];
        let paths = ["a.rs", "a.md", "a.py", "a.ts", "a.ps1", "a.sh", "a.toml", "a.txt"];
        for path in paths {
            let all = nasty.join("\n");
            for text in nasty.iter().copied().chain([all.as_str()]) {
                let _ = outline(path, text, 300);
                let _ = outline(path, text, FILE_BUDGET);
            }
        }
        // Todas as fatias de um texto multibyte.
        let text = "fn ação(é: &str) -> 🙂 {\n    let s = \"ü{\";\n}\nstruct Ç { ã: u8 }\n";
        for end in 0..=text.len() {
            if text.is_char_boundary(end) {
                for path in ["a.rs", "a.py", "a.ts"] {
                    let _ = outline(path, &text[..end], FILE_BUDGET);
                }
            }
        }
    }
}
