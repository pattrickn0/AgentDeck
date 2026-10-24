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

/// Caracteres da resposta de um arquivo e da chamada inteira (como o `LEARN_BUDGET` do `--learn`). Um arquivo tem
/// pelo menos `FILE_BUDGET`; pedido sozinho (ou com poucos), divide a chamada inteira com os outros.
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

/// A assinatura que começa na linha `from`: o texto até o primeiro `stops` fora de parênteses, colchetes e chaves
/// (que podem quebrar linha). Devolve o texto limpo, a linha onde parou e qual `stops` achou (`None` se não achou até o
/// limite).
fn signature(src: &Source, from: usize, stops: &[&str]) -> (String, usize, Option<usize>) {
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
    }
    (collapse(&text), last.saturating_sub(1).max(from), None)
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

/// Quantos `#[test]` (ou `#[tokio::test]`...) o arquivo tem.
fn count_tests(code: &[String]) -> usize {
    code.iter()
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
    // Os `#[test]` do arquivo inteiro: a linha do `mod tests` diz quantos são, sem casar o aninhamento dos módulos.
    let tests = count_tests(&src.code);
    let mut out = Vec::new();
    // Profundidade (de chaves) dos itens `impl`/`trait`/`mod` abertos: os membros moram um nível abaixo.
    let mut stack: Vec<usize> = Vec::new();
    let mut attrs: Vec<String> = Vec::new();
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
        if t.is_empty() {
            if raw.is_empty() {
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
            i += 1;
            continue;
        };
        let stops: &[&str] = if kind == Kind::Use { &[";"] } else { &["{", ";"] };
        let (sig, j, stop) = signature(src, i, stops);
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
            out.push(item(i, indent, text));
        }
        if container {
            stack.push(d);
        }
        attrs.clear();
        i = j + 1;
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

// ---------- Resumo de um arquivo ----------

/// Os itens de `text` na linguagem de `lang`; `None` se não há resumo para ela.
fn items(lang: Lang, text: &str) -> Option<Vec<Item>> {
    match lang {
        Lang::Rust => Some(rust_items(&Source::new(lang, text))),
        Lang::Markdown => Some(markdown_items(&Source::new(lang, text))),
        _ => None,
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
    let limit = budget.saturating_sub(TAIL_RESERVE);
    let keep = fit_members(&head, &items, limit);
    let mut out = head;
    let mut shown = 0;
    let mut cut = None;
    for (i, it) in items.iter().enumerate() {
        if !keep[i] {
            continue;
        }
        let mut line = format!("\n{:>5}  {}{}", it.line, "  ".repeat(it.depth), it.text);
        let hidden = items[i + 1..].iter().take_while(|m| m.depth > it.depth).zip(&keep[i + 1..]).filter(|(_, k)| !**k).count();
        if hidden > 0 {
            line.push_str(&format!(" (+{hidden} {} dentro)", if hidden == 1 { "item" } else { "itens" }));
        }
        if out.chars().count() + line.chars().count() > limit && shown > 0 {
            cut = Some(i);
            break;
        }
        out.push_str(&line);
        shown += 1;
    }
    if let Some(cut) = cut {
        let rest = &items[cut..];
        let (first, last) = (rest[0].line, rest[rest.len() - 1].line);
        out.push_str(&format!(
            "\n      … e mais {} {} (linhas {first}–{last})",
            rest.len(),
            if rest.len() == 1 { "item" } else { "itens" }
        ));
    }
    out
}

/// Reserva, por item de topo com membros, para a nota "(+N itens dentro)".
const HIDDEN_NOTE: usize = 20;

/// Quais itens entram quando nem todos cabem em `limit`: os de topo (a menor profundidade do arquivo) sempre, e os
/// membros (de `impl`, `mod`, seção) em ordem enquanto sobra lugar. Assim o orçamento estourado tira detalhe do meio,
/// não o fim do arquivo (o `impl Core` e os testes, que costumam vir por último). Se nem os de topo cabem, `outline`
/// corta o fim como antes.
fn fit_members(head: &str, items: &[Item], limit: usize) -> Vec<bool> {
    let size = |it: &Item| format!("{:>5}  {}{}", it.line, "  ".repeat(it.depth), it.text).chars().count() + 1;
    if head.chars().count() + items.iter().map(size).sum::<usize>() <= limit {
        return vec![true; items.len()];
    }
    let top = items.iter().map(|it| it.depth).min().unwrap_or(0);
    let mut keep: Vec<bool> = items.iter().map(|it| it.depth == top).collect();
    let fixed: usize = items
        .iter()
        .enumerate()
        .filter(|(_, it)| it.depth == top)
        .map(|(i, it)| size(it) + if items.get(i + 1).is_some_and(|m| m.depth > top) { HIDDEN_NOTE } else { 0 })
        .sum();
    let mut left = limit.saturating_sub(head.chars().count() + fixed);
    for (i, it) in items.iter().enumerate().filter(|(_, it)| it.depth > top) {
        let s = size(it);
        if s > left {
            break;
        }
        left -= s;
        keep[i] = true;
    }
    keep
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
        let share = (OUTLINE_BUDGET / paths.len().max(1)).max(FILE_BUDGET);
        for p in paths {
            let rel = crate::learnings::relativize(p, &roots);
            if rel.is_empty() {
                continue;
            }
            if left < 400 {
                parts.push(format!("{rel} — fora do orçamento desta resposta; peça este arquivo sozinho"));
                continue;
            }
            let part = outline_path(&work_root, &canonical_root, &rel, share.min(left));
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
                "5:pub fn work(x: u32) -> u32",
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
                "7:  pub(crate) async fn read(&self, p: &Path) -> String",
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
    fn the_tests_are_counted_in_the_whole_file() {
        let src = "#[test]
fn outside() {}

#[cfg(test)]
mod tests {
    mod nested {
        #[test]
        fn a() {}
    }
    #[test]
    fn b() {}
}
";
        assert_eq!(rust(src), ["5:#[cfg(test)] mod tests — 3 testes"]);
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
    fn markdown_gives_the_headings_with_their_lines_and_ignores_code_fences() {
        let src = "---\ntitulo: x\n# nao e titulo\n---\n# Um\n\ntexto\n\n## Dois\n\n```sh\n# comentario\n```\n\n### Tres\n\n#### Quatro\n####Sem espaco\n";
        let got = texts(&items(Lang::Markdown, src).expect("Markdown tem resumo"));
        assert_eq!(got, ["5:# Um", "9:  ## Dois", "15:    ### Tres"]);
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
        let out = outline(
            "a.py",
            "def f():
    pass
",
            FILE_BUDGET,
        );
        assert_eq!(
            out,
            "a.py — 2 linhas, Python
      (sem resumo para Python; leia o arquivo, são 2 linhas)"
        );
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
    fn over_budget_the_members_go_before_the_end_of_the_file() {
        let members: String = (1..=40).map(|i| format!("    pub fn metodo_numero_{i}(&self) {{}}\n")).collect();
        let text = format!(
            "pub struct Core;\n\nimpl Core {{\n{members}}}\n\npub fn ultima() {{}}\n\n#[cfg(test)]\nmod tests {{\n    #[test]\n    fn t() {{}}\n}}\n"
        );
        let out = outline("a.rs", &text, 600);
        assert!(out.chars().count() <= 600, "{}", out.chars().count());
        assert!(out.contains("impl Core (+") && out.contains("itens dentro)"), "{out}");
        assert!(out.contains("pub fn ultima()") && out.contains("mod tests — 1 teste"), "{out}");
        assert!(out.contains("metodo_numero_1(") && !out.contains("… e mais"), "{out}");
        // Markdown sem título de nível 1: os `##` são o topo.
        let md: String = (1..=30).map(|i| format!("## Seção {i}\n\n### Detalhe longo da seção número {i}\n\n")).collect();
        let out = outline("a.md", &md, 1500);
        assert!(out.contains("## Seção 30") && !out.contains("… e mais"), "{out}");
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
