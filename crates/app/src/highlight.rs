//! Realce de sintaxe do modo Documentos: um tokenizador leve, escrito à mão, linha a linha.
//!
//! Escolha (t36): `syntect` (o que o `egui_extras::syntax_highlighting` usa) traz o motor de expressões regulares, as
//! definições de sintaxe e os temas embutidos: alguns MB a mais no executável e minutos a mais na compilação (a CI
//! compila nos três sistemas), para colorir nove linguagens. Aqui cada linguagem é uma tabela de palavras-chave mais
//! as regras de comentário, texto entre aspas, número e chave; o estado que passa de uma linha para outra (comentário de
//! bloco, texto de várias linhas, cerca do markdown) cabe em [`Carry`]. O resultado é só uma lista de faixas de bytes
//! por linha ([`Span`]): quem pinta escolhe a cor (`theme::pal().syn_*`) e guarda o resultado por versão do texto.
//!
//! Nada aqui pode dar pânico com texto qualquer (o arquivo é do usuário): as faixas sempre começam e terminam em
//! fronteira de caractere, e o teste `spans_are_always_valid_on_any_text` confere isso.

use std::ops::Range;

/// A categoria de um pedaço de texto; o que pinta traduz em cor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tok {
    #[default]
    Plain,
    Keyword,
    Type,
    Function,
    String,
    Number,
    Comment,
    /// Chave de JSON, TOML ou YAML; opção de linha de comando.
    Key,
    /// Atributo, macro, título do markdown, variável do shell.
    Meta,
}

/// Uma faixa colorida de uma linha (bytes dentro da linha, sem o `\n`). As linhas só guardam o que não é `Plain`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub range: Range<usize>,
    pub tok: Tok,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lang {
    #[default]
    Plain,
    Rust,
    Markdown,
    Toml,
    Json,
    Yaml,
    JsTs,
    Python,
    PowerShell,
    Shell,
}

impl Lang {
    /// A linguagem pelo caminho do arquivo (extensão ou nome conhecido); o resto é texto puro.
    pub fn from_path(path: &str) -> Lang {
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path).to_ascii_lowercase();
        match name.as_str() {
            "cargo.lock" | "rustfmt.toml" | ".rustfmt.toml" => return Lang::Toml,
            ".bashrc" | ".zshrc" | ".profile" | ".bash_profile" => return Lang::Shell,
            _ => {}
        }
        match name.rsplit_once('.').map(|(_, ext)| ext) {
            Some("rs") => Lang::Rust,
            Some("md" | "markdown") => Lang::Markdown,
            Some("toml") => Lang::Toml,
            Some("json" | "jsonc" | "json5") => Lang::Json,
            Some("yml" | "yaml") => Lang::Yaml,
            Some("js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx" | "mts" | "cts") => Lang::JsTs,
            Some("py" | "pyw") => Lang::Python,
            Some("ps1" | "psm1" | "psd1") => Lang::PowerShell,
            Some("sh" | "bash" | "zsh") => Lang::Shell,
            _ => Lang::Plain,
        }
    }

    /// A linguagem do que vem depois da cerca do markdown (```` ```rust ````).
    fn from_fence(info: &str) -> Lang {
        match info.split_whitespace().next().unwrap_or("").to_ascii_lowercase().as_str() {
            "rust" | "rs" => Lang::Rust,
            "toml" => Lang::Toml,
            "json" | "jsonc" => Lang::Json,
            "yaml" | "yml" => Lang::Yaml,
            "js" | "javascript" | "ts" | "typescript" | "jsx" | "tsx" => Lang::JsTs,
            "py" | "python" => Lang::Python,
            "powershell" | "ps1" | "pwsh" => Lang::PowerShell,
            "sh" | "bash" | "shell" | "zsh" | "console" => Lang::Shell,
            _ => Lang::Plain,
        }
    }

    /// Nome para a dica da interface.
    pub fn label(self) -> &'static str {
        match self {
            Lang::Plain => "Texto",
            Lang::Rust => "Rust",
            Lang::Markdown => "Markdown",
            Lang::Toml => "TOML",
            Lang::Json => "JSON",
            Lang::Yaml => "YAML",
            Lang::JsTs => "JavaScript/TypeScript",
            Lang::Python => "Python",
            Lang::PowerShell => "PowerShell",
            Lang::Shell => "Shell",
        }
    }
}

/// O que passa de uma linha para a seguinte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Carry {
    /// Profundidade do comentário de bloco aberto.
    block: u8,
    /// Delimitador do texto de várias linhas aberto (0 = nenhum) e se é o triplo (Python, TOML).
    quote: u8,
    triple: bool,
    /// Texto cru do Rust aberto: quantos `#` fecham.
    raw: Option<u8>,
    /// Markdown: dentro de uma cerca de código, com a linguagem dela.
    fence: Option<Lang>,
}

/// O realce de todas as linhas de `text` (separadas por `\n`; o `\r` do fim some). Uma linha a mais no fim, vazia, se o
/// texto termina em `\n`, como o editor mostra.
pub fn highlight_lines(lang: Lang, text: &str) -> Vec<Vec<Span>> {
    let mut carry = Carry::default();
    text.split('\n').map(|l| highlight_line(lang, l.strip_suffix('\r').unwrap_or(l), &mut carry)).collect()
}

/// O realce de uma linha, continuando de `carry` e deixando nele o que a próxima precisa.
pub fn highlight_line(lang: Lang, line: &str, carry: &mut Carry) -> Vec<Span> {
    let mut out = Vec::new();
    match lang {
        Lang::Plain => {}
        Lang::Markdown => markdown_line(line, carry, &mut out),
        Lang::Toml => toml_line(line, carry, &mut out),
        Lang::Yaml => yaml_line(line, carry, &mut out),
        _ => scan(&spec(lang), line, 0, carry, &mut out),
    }
    out
}

// ---------------------------------------------------------------------------
// As linguagens de código
// ---------------------------------------------------------------------------

/// As regras de uma linguagem para o tokenizador comum.
struct Spec {
    lang: Lang,
    line_comments: &'static [&'static str],
    /// Abre, fecha e se aninha.
    block: Option<(&'static str, &'static str, bool)>,
    quotes: &'static [u8],
    triple: bool,
    keywords: &'static [&'static str],
    types: &'static [&'static str],
    /// Palavra com inicial maiúscula é um tipo.
    upper_types: bool,
    /// Comparar as palavras-chave sem diferença de caixa (PowerShell).
    nocase: bool,
    /// `$var` (shell e PowerShell).
    vars: bool,
    /// `@nome` é um decorador.
    decorators: bool,
    /// O texto seguido de `:` é uma chave (JSON).
    colon_keys: bool,
    /// `-opção` e `--opção` (shell e PowerShell).
    options: bool,
}

const NONE: &[&str] = &[];

fn spec(lang: Lang) -> Spec {
    let base = Spec {
        lang,
        line_comments: NONE,
        block: None,
        quotes: b"\"",
        triple: false,
        keywords: NONE,
        types: NONE,
        upper_types: false,
        nocase: false,
        vars: false,
        decorators: false,
        colon_keys: false,
        options: false,
    };
    match lang {
        Lang::Rust => Spec {
            line_comments: &["//"],
            block: Some(("/*", "*/", true)),
            quotes: b"\"'",
            keywords: &[
                "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn", "for", "if",
                "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct",
                "super", "trait", "true", "type", "unsafe", "use", "where", "while",
            ],
            types: &["bool", "char", "str", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize", "f32", "f64"],
            upper_types: true,
            ..base
        },
        Lang::JsTs => Spec {
            line_comments: &["//"],
            block: Some(("/*", "*/", false)),
            quotes: b"\"'`",
            keywords: &[
                "abstract", "as", "async", "await", "break", "case", "catch", "class", "const", "continue", "debugger", "declare",
                "default", "delete", "do", "else", "enum", "export", "extends", "false", "finally", "for", "from", "function", "if",
                "implements", "import", "in", "instanceof", "interface", "keyof", "let", "namespace", "new", "null", "of", "private",
                "protected", "public", "readonly", "return", "static", "super", "switch", "this", "throw", "true", "try", "type",
                "typeof", "undefined", "var", "void", "while", "with", "yield",
            ],
            types: &["string", "number", "boolean", "any", "unknown", "never", "object", "symbol", "bigint"],
            upper_types: true,
            decorators: true,
            ..base
        },
        Lang::Python => Spec {
            line_comments: &["#"],
            quotes: b"\"'",
            triple: true,
            keywords: &[
                "and", "as", "assert", "async", "await", "break", "case", "class", "continue", "def", "del", "elif", "else", "except",
                "False", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "match", "None", "nonlocal", "not",
                "or", "pass", "raise", "return", "self", "True", "try", "while", "with", "yield",
            ],
            types: &["int", "str", "float", "bool", "list", "dict", "set", "tuple", "bytes"],
            upper_types: true,
            decorators: true,
            ..base
        },
        Lang::PowerShell => Spec {
            line_comments: &["#"],
            block: Some(("<#", "#>", false)),
            quotes: b"\"'",
            keywords: &[
                "begin", "break", "catch", "class", "continue", "data", "do", "dynamicparam", "else", "elseif", "end", "exit", "filter",
                "finally", "for", "foreach", "from", "function", "if", "in", "param", "process", "return", "switch", "throw", "trap",
                "try", "until", "using", "var", "while", "workflow", "true", "false", "null",
            ],
            nocase: true,
            vars: true,
            options: true,
            ..base
        },
        Lang::Shell => Spec {
            line_comments: &["#"],
            quotes: b"\"'",
            keywords: &[
                "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac", "in", "function", "select",
                "time", "return", "exit", "break", "continue", "local", "export", "readonly", "declare", "unset", "set", "shift", "source",
                "alias", "true", "false",
            ],
            vars: true,
            options: true,
            ..base
        },
        Lang::Json => Spec {
            line_comments: &["//"],
            block: Some(("/*", "*/", false)),
            keywords: &["true", "false", "null"],
            colon_keys: true,
            ..base
        },
        Lang::Toml => Spec { line_comments: &["#"], quotes: b"\"'", triple: true, keywords: &["true", "false"], ..base },
        Lang::Yaml => Spec {
            line_comments: &["#"],
            quotes: b"\"'",
            keywords: &["true", "false", "null", "yes", "no", "True", "False", "Null", "~"],
            ..base
        },
        Lang::Markdown | Lang::Plain => base,
    }
}

fn is_ident_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80
}

/// O fim (depois do delimitador) do texto que começou antes de `from`; `None` se a linha acaba antes.
fn find_close(b: &[u8], from: usize, q: u8, triple: bool, escapes: bool) -> Option<usize> {
    let mut i = from;
    while i < b.len() {
        if escapes && b[i] == b'\\' {
            i += 2;
            continue;
        }
        if b[i] == q {
            if !triple {
                return Some(i + 1);
            }
            if b[i..].starts_with(&[q, q, q]) {
                return Some(i + 3);
            }
        }
        i += 1;
    }
    None
}

fn push(out: &mut Vec<Span>, start: usize, end: usize, tok: Tok) {
    if end > start {
        out.push(Span { range: start..end, tok });
    }
}

/// Continua um comentário de bloco aberto: pinta de `start` até o fecho (ou o fim da linha) e devolve onde parou.
fn block_rest(line: &str, start: usize, scan_from: usize, sp: &Spec, carry: &mut Carry, out: &mut Vec<Span>) -> usize {
    let Some((open, close, nested)) = sp.block else { return line.len() };
    let b = line.as_bytes();
    let mut i = scan_from;
    while i < b.len() {
        if b[i..].starts_with(close.as_bytes()) {
            i += close.len();
            carry.block = carry.block.saturating_sub(1);
            if carry.block == 0 {
                break;
            }
        } else if nested && b[i..].starts_with(open.as_bytes()) {
            carry.block = carry.block.saturating_add(1);
            i += open.len();
        } else {
            i += 1;
        }
    }
    let end = i.min(b.len());
    push(out, start, end, Tok::Comment);
    end
}

/// Uma palavra de `start` em diante: o fim dela (só bytes de identificador).
fn word_end(b: &[u8], start: usize) -> usize {
    let mut j = start;
    while j < b.len() && is_ident_byte(b[j]) {
        j += 1;
    }
    j
}

/// Quantos bytes tem o caractere que começa em `i` (`i` em fronteira).
fn char_len(line: &str, i: usize) -> usize {
    line[i..].chars().next().map_or(1, char::len_utf8)
}

/// O texto cru do Rust que abre em `i` (`r"`, `r#"`, `br##"`): quantos `#` e quantos bytes tem a abertura.
fn raw_open(b: &[u8], i: usize) -> Option<(u8, usize)> {
    let mut j = i;
    if b.get(j) == Some(&b'b') {
        j += 1;
    }
    if b.get(j) != Some(&b'r') {
        return None;
    }
    j += 1;
    let mut hashes = 0u8;
    while b.get(j) == Some(&b'#') {
        hashes = hashes.checked_add(1)?;
        j += 1;
    }
    (b.get(j) == Some(&b'"')).then_some((hashes, j + 1 - i))
}

/// Fim do texto cru: `"` e `hashes` vezes `#`.
fn raw_close(b: &[u8], from: usize, hashes: u8) -> Option<usize> {
    let mut i = from;
    while i < b.len() {
        if b[i] == b'"' && b[i + 1..].len() >= hashes as usize && b[i + 1..i + 1 + hashes as usize].iter().all(|&c| c == b'#') {
            return Some(i + 1 + hashes as usize);
        }
        i += 1;
    }
    None
}

/// O tokenizador comum às linguagens de código, de `start` em diante na linha.
fn scan(sp: &Spec, line: &str, start: usize, carry: &mut Carry, out: &mut Vec<Span>) {
    let b = line.as_bytes();
    let mut i = start;
    // O que ficou aberto da linha anterior.
    if carry.block > 0 {
        i = block_rest(line, i, i, sp, carry, out);
    } else if carry.quote != 0 {
        let (q, triple) = (carry.quote, carry.triple);
        match find_close(b, i, q, triple, sp.lang != Lang::Toml || q == b'"') {
            Some(end) => {
                push(out, i, end, Tok::String);
                carry.quote = 0;
                carry.triple = false;
                i = end;
            }
            None => {
                push(out, i, b.len(), Tok::String);
                return;
            }
        }
    } else if let Some(h) = carry.raw {
        match raw_close(b, i, h) {
            Some(end) => {
                push(out, i, end, Tok::String);
                carry.raw = None;
                i = end;
            }
            None => {
                push(out, i, b.len(), Tok::String);
                return;
            }
        }
    }
    while i < b.len() {
        let c = b[i];
        let prev_ws = i == 0 || b[i - 1].is_ascii_whitespace();
        // Comentário de linha (o `#` só vale no começo de uma palavra: `$#`, `a#b`).
        if sp.line_comments.iter().any(|lc| b[i..].starts_with(lc.as_bytes()) && (*lc != "#" || prev_ws)) {
            push(out, i, b.len(), Tok::Comment);
            return;
        }
        if let Some((open, _, _)) = sp.block {
            if b[i..].starts_with(open.as_bytes()) {
                carry.block = 1;
                i = block_rest(line, i, i + open.len(), sp, carry, out);
                continue;
            }
        }
        // Texto cru do Rust.
        if sp.lang == Lang::Rust && (c == b'r' || c == b'b') && (i == 0 || !is_ident_byte(b[i - 1])) {
            if let Some((hashes, open_len)) = raw_open(b, i) {
                match raw_close(b, i + open_len, hashes) {
                    Some(end) => {
                        push(out, i, end, Tok::String);
                        i = end;
                    }
                    None => {
                        carry.raw = Some(hashes);
                        push(out, i, b.len(), Tok::String);
                        return;
                    }
                }
                continue;
            }
        }
        // Aspas simples do Rust: caractere ou lifetime.
        if sp.lang == Lang::Rust && c == b'\'' {
            let rest = &line[i + 1..];
            let next = rest.chars().next();
            if next == Some('\\') {
                let end = find_close(b, i + 1, b'\'', false, true).unwrap_or(b.len());
                push(out, i, end, Tok::String);
                i = end;
            } else if next.is_some_and(|ch| b.get(i + 1 + ch.len_utf8()) == Some(&b'\'')) {
                let end = i + 2 + next.map_or(0, char::len_utf8);
                push(out, i, end, Tok::String);
                i = end;
            } else if next.is_some_and(|ch| ch.is_alphabetic() || ch == '_') {
                let end = word_end(b, i + 1);
                push(out, i, end, Tok::Type);
                i = end;
            } else {
                i += 1;
            }
            continue;
        }
        if sp.quotes.contains(&c) {
            let triple = sp.triple && b[i..].starts_with(&[c, c, c]);
            let escapes = sp.lang != Lang::Toml || c == b'"';
            let from = if triple { i + 3 } else { i + 1 };
            match find_close(b, from, c, triple, escapes) {
                Some(end) => {
                    let mut tok = Tok::String;
                    if sp.colon_keys && b[end..].iter().find(|x| !x.is_ascii_whitespace()) == Some(&b':') {
                        tok = Tok::Key;
                    }
                    push(out, i, end, tok);
                    i = end;
                }
                None if triple || c == b'`' => {
                    carry.quote = c;
                    carry.triple = triple;
                    push(out, i, b.len(), Tok::String);
                    return;
                }
                None => {
                    push(out, i, b.len(), Tok::String);
                    return;
                }
            }
            continue;
        }
        if c == b'$' && sp.vars {
            let mut j = i + 1;
            if b.get(j) == Some(&b'{') {
                j = b[j..].iter().position(|&x| x == b'}').map_or(b.len(), |p| j + p + 1);
            } else if j < b.len() && !is_ident_byte(b[j]) && b"?#@*!$-0123456789".contains(&b[j]) {
                j += 1;
            } else {
                j = word_end(b, j);
            }
            push(out, i, j, Tok::Meta);
            i = j.max(i + 1);
            continue;
        }
        if sp.options && c == b'-' && prev_ws && b.get(i + 1).is_some_and(|x| x.is_ascii_alphabetic() || *x == b'-') {
            let mut j = i + 1;
            while j < b.len() && (is_ident_byte(b[j]) || b[j] == b'-') {
                j += 1;
            }
            push(out, i, j, Tok::Key);
            i = j;
            continue;
        }
        if sp.lang == Lang::Rust && c == b'#' && matches!(b.get(i + 1), Some(b'[') | Some(b'!')) {
            let end = b[i..].iter().position(|&x| x == b']').map_or(b.len(), |p| i + p + 1);
            push(out, i, end, Tok::Meta);
            i = end;
            continue;
        }
        if sp.decorators && c == b'@' && b.get(i + 1).is_some_and(|x| x.is_ascii_alphabetic() || *x == b'_') {
            let mut j = i + 1;
            while j < b.len() && (is_ident_byte(b[j]) || b[j] == b'.') {
                j += 1;
            }
            push(out, i, j, Tok::Meta);
            i = j;
            continue;
        }
        if c.is_ascii_digit() {
            let mut j = i + 1;
            while j < b.len() {
                let x = b[j];
                if x.is_ascii_alphanumeric() || x == b'_' || (x == b'.' && b.get(j + 1).is_some_and(u8::is_ascii_digit)) {
                    j += 1;
                } else {
                    break;
                }
            }
            push(out, i, j, Tok::Number);
            i = j;
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' || c >= 0x80 {
            let mut j = word_end(b, i);
            let mut word = &line[i..j];
            let mut cmdlet = false;
            if sp.lang == Lang::PowerShell {
                while b.get(j) == Some(&b'-') && b.get(j + 1).is_some_and(u8::is_ascii_alphabetic) {
                    j = word_end(b, j + 1);
                    cmdlet = true;
                }
                word = &line[i..j];
            }
            let is_kw = |list: &[&str]| if sp.nocase { list.iter().any(|k| k.eq_ignore_ascii_case(word)) } else { list.contains(&word) };
            let tok = if cmdlet {
                Tok::Function
            } else if is_kw(sp.keywords) {
                Tok::Keyword
            } else if is_kw(sp.types) {
                Tok::Type
            } else if sp.lang == Lang::Rust && b.get(j) == Some(&b'!') && b.get(j + 1) != Some(&b'=') {
                j += 1;
                Tok::Meta
            } else if b.get(j) == Some(&b'(') {
                Tok::Function
            } else if sp.upper_types && c.is_ascii_uppercase() {
                Tok::Type
            } else {
                Tok::Plain
            };
            push(out, i, j, tok);
            i = j;
            continue;
        }
        i += if c < 0x80 { 1 } else { char_len(line, i) };
    }
}

// ---------------------------------------------------------------------------
// Dados: TOML e YAML (a chave, e o resto como código)
// ---------------------------------------------------------------------------

fn toml_line(line: &str, carry: &mut Carry, out: &mut Vec<Span>) {
    let sp = spec(Lang::Toml);
    if carry.quote != 0 || carry.block > 0 {
        return scan(&sp, line, 0, carry, out);
    }
    let t = line.trim_start();
    let indent = line.len() - t.len();
    if t.starts_with('[') {
        let end = t.rfind(']').map_or(line.len(), |p| indent + p + 1);
        push(out, indent, end, Tok::Meta);
        return scan(&sp, line, end, carry, out);
    }
    // `chave = valor`, com a chave simples, com aspas ou pontilhada.
    let key_end = if t.starts_with('"') || t.starts_with('\'') {
        let q = t.as_bytes()[0];
        find_close(t.as_bytes(), 1, q, false, q == b'"').and_then(|e| t[e..].trim_start().starts_with('=').then_some(indent + e))
    } else {
        t.find('=').filter(|&p| t[..p].bytes().all(|c| is_ident_byte(c) || c == b'-' || c == b'.' || c == b' ' || c == b'"' || c == b'\''))
            .map(|p| indent + t[..p].trim_end().len())
    };
    match key_end {
        Some(e) => {
            push(out, indent, e, Tok::Key);
            scan(&sp, line, e, carry, out);
        }
        None => scan(&sp, line, 0, carry, out),
    }
}

fn yaml_line(line: &str, carry: &mut Carry, out: &mut Vec<Span>) {
    let sp = spec(Lang::Yaml);
    let t = line.trim_start();
    let indent = line.len() - t.len();
    if indent == 0 && (t == "---" || t == "..." || t.starts_with("--- ")) {
        push(out, 0, 3, Tok::Meta);
        return scan(&sp, line, 3, carry, out);
    }
    // Pula os `- ` da lista e acha `chave:` (seguida de espaço ou do fim).
    let mut at = indent;
    while line[at..].starts_with("- ") {
        at += 2;
        while line[at..].starts_with(' ') {
            at += 1;
        }
    }
    let rest = &line[at..];
    let key_len = if rest.starts_with('"') || rest.starts_with('\'') {
        let q = rest.as_bytes()[0];
        find_close(rest.as_bytes(), 1, q, false, q == b'"').filter(|&e| rest[e..].starts_with(':'))
    } else if rest.starts_with('#') {
        None
    } else {
        rest.find(':').filter(|&p| {
            (rest[p + 1..].is_empty() || rest[p + 1..].starts_with(' ')) && !rest[..p].contains(['#', '"', '\'', '{', '[', ',']) && p > 0
        })
    };
    match key_len {
        Some(k) => {
            push(out, at, at + k, Tok::Key);
            scan(&sp, line, at + k, carry, out);
        }
        None => scan(&sp, line, 0, carry, out),
    }
}

// ---------------------------------------------------------------------------
// Markdown
// ---------------------------------------------------------------------------

fn markdown_line(line: &str, carry: &mut Carry, out: &mut Vec<Span>) {
    let t = line.trim_start();
    let indent = line.len() - t.len();
    let fence_mark = t.starts_with("```") || t.starts_with("~~~");
    if let Some(inner) = carry.fence {
        if fence_mark {
            *carry = Carry::default();
            return push(out, indent, line.len(), Tok::Meta);
        }
        return match inner {
            Lang::Plain | Lang::Markdown => {}
            lang => {
                let spans = highlight_line(lang, line, carry);
                out.extend(spans);
            }
        };
    }
    if fence_mark {
        *carry = Carry { fence: Some(Lang::from_fence(&t[3..])), ..Carry::default() };
        return push(out, indent, line.len(), Tok::Meta);
    }
    let hashes = t.bytes().take_while(|&c| c == b'#').count();
    if (1..=6).contains(&hashes) && (t.len() == hashes || t[hashes..].starts_with(' ')) {
        return push(out, indent, line.len(), Tok::Meta);
    }
    if t.starts_with('>') {
        return push(out, indent, line.len(), Tok::Comment);
    }
    let mut at = indent;
    // Marcador de lista (`-`, `*`, `+`, `1.`) e caixa de tarefa.
    let marker = if t.starts_with(['-', '*', '+']) && t[1..].starts_with(' ') {
        Some(1)
    } else {
        let digits = t.bytes().take_while(u8::is_ascii_digit).count();
        (digits > 0 && t[digits..].starts_with(". ")).then_some(digits + 1)
    };
    if let Some(m) = marker {
        push(out, at, at + m, Tok::Keyword);
        at += m;
        let after = line[at..].trim_start();
        let sp = line.len() - after.len();
        if after.starts_with("[ ]") || after.starts_with("[x]") || after.starts_with("[X]") {
            push(out, sp, sp + 3, Tok::Number);
            at = sp + 3;
        }
    }
    inline_markdown(line, at, out);
}

/// Código entre crases, negrito e `[texto](url)`.
fn inline_markdown(line: &str, from: usize, out: &mut Vec<Span>) {
    let b = line.as_bytes();
    let mut i = from;
    while i < b.len() {
        match b[i] {
            b'`' => {
                let end = b[i + 1..].iter().position(|&c| c == b'`').map_or(b.len(), |p| i + p + 2);
                push(out, i, end, Tok::String);
                i = end;
            }
            b'*' | b'_' if b.get(i + 1) == Some(&b[i]) => {
                let mark = [b[i], b[i]];
                match b[i + 2..].windows(2).position(|w| w == mark) {
                    Some(p) => {
                        let end = i + 2 + p + 2;
                        push(out, i, end, Tok::Function);
                        i = end;
                    }
                    None => i += 2,
                }
            }
            b'[' => {
                let text_end = b[i..].iter().position(|&c| c == b']').map(|p| i + p + 1);
                match text_end.filter(|&e| b.get(e) == Some(&b'(')) {
                    Some(e) => {
                        let url_end = b[e..].iter().position(|&c| c == b')').map_or(b.len(), |p| e + p + 1);
                        push(out, i, e, Tok::Key);
                        push(out, e, url_end, Tok::Comment);
                        i = url_end;
                    }
                    None => i += 1,
                }
            }
            c if c < 0x80 => i += 1,
            _ => i += char_len(line, i),
        }
    }
}

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

    fn toks(lang: Lang, line: &str) -> Vec<(String, Tok)> {
        let mut carry = Carry::default();
        highlight_line(lang, line, &mut carry).into_iter().map(|s| (line[s.range].to_owned(), s.tok)).collect()
    }

    #[test]
    fn the_language_comes_from_the_file_name() {
        for (path, lang) in [
            ("crates/app/src/main.rs", Lang::Rust),
            ("README.md", Lang::Markdown),
            ("Cargo.toml", Lang::Toml),
            ("Cargo.lock", Lang::Toml),
            ("package.json", Lang::Json),
            (".github/workflows/ci.yml", Lang::Yaml),
            ("a.yaml", Lang::Yaml),
            ("x.tsx", Lang::JsTs),
            ("x.mjs", Lang::JsTs),
            ("a.py", Lang::Python),
            ("scripts\\gerar-icone.ps1", Lang::PowerShell),
            ("run.sh", Lang::Shell),
            ("LICENSE", Lang::Plain),
            ("sem-extensao", Lang::Plain),
        ] {
            assert_eq!(Lang::from_path(path), lang, "{path}");
        }
    }

    #[test]
    fn rust_keywords_types_strings_numbers_macros_and_comments() {
        let t = toks(Lang::Rust, "pub fn run(x: u32) -> Option<String> { println!(\"oi {x}\"); 0x1F // fim");
        let has = |s: &str, tok| t.iter().any(|(w, k)| w == s && *k == tok);
        assert!(has("pub", Tok::Keyword) && has("fn", Tok::Keyword));
        assert!(has("run", Tok::Function));
        assert!(has("u32", Tok::Type) && has("Option", Tok::Type) && has("String", Tok::Type));
        assert!(has("println!", Tok::Meta));
        assert!(has("\"oi {x}\"", Tok::String));
        assert!(has("0x1F", Tok::Number));
        assert!(has("// fim", Tok::Comment));
    }

    #[test]
    fn rust_lifetimes_chars_attributes_and_raw_strings() {
        let t = toks(Lang::Rust, "#[derive(Debug)] fn a<'a>(c: &'a str) -> char { 'x' }");
        let has = |s: &str, tok| t.iter().any(|(w, k)| w == s && *k == tok);
        assert!(has("#[derive(Debug)]", Tok::Meta));
        assert!(has("'a", Tok::Type));
        assert!(has("'x'", Tok::String));
        let raw = toks(Lang::Rust, "let s = r#\"a \" b\"#; // c");
        assert!(raw.iter().any(|(w, k)| w == "r#\"a \" b\"#" && *k == Tok::String), "{raw:?}");
        assert!(raw.iter().any(|(w, k)| w == "// c" && *k == Tok::Comment));
    }

    #[test]
    fn block_comments_and_raw_strings_carry_over_lines() {
        let lines = highlight_lines(Lang::Rust, "a /* x\n  y /* z */ w\n*/ b\nlet s = r\"\nq\";\nc");
        let words = |n: usize, text: &str| lines[n].iter().filter(|s| s.tok == Tok::Comment).map(|s| text[s.range.clone()].to_owned()).collect::<Vec<_>>();
        assert_eq!(words(0, "a /* x"), ["/* x"]);
        assert_eq!(words(1, "  y /* z */ w"), ["  y /* z */ w"], "o comentário aninha");
        assert_eq!(words(2, "*/ b"), ["*/"]);
        assert!(lines[4].iter().any(|s| s.tok == Tok::String), "o texto cru continua");
        assert!(lines[5].is_empty() || lines[5].iter().all(|s| s.tok != Tok::String));
    }

    #[test]
    fn python_triple_quotes_decorators_and_hash_comments() {
        let lines = highlight_lines(Lang::Python, "@app.route\ndef f():\n    \"\"\"doc\n    mais\"\"\"  # c\n    return 'a#b'");
        assert!(lines[0].iter().any(|s| s.tok == Tok::Meta));
        assert!(lines[1].iter().any(|s| s.tok == Tok::Keyword));
        assert!(lines[2].iter().any(|s| s.tok == Tok::String));
        assert!(lines[3].iter().any(|s| s.tok == Tok::String) && lines[3].iter().any(|s| s.tok == Tok::Comment));
        let last = &lines[4];
        assert!(last.iter().any(|s| s.tok == Tok::String) && last.iter().all(|s| s.tok != Tok::Comment), "# dentro de aspas");
    }

    #[test]
    fn json_tells_keys_from_values() {
        let t = toks(Lang::Json, "  \"nome\": \"valor\", \"n\": -12.5, \"ok\": true");
        let has = |s: &str, tok| t.iter().any(|(w, k)| w == s && *k == tok);
        assert!(has("\"nome\"", Tok::Key) && has("\"valor\"", Tok::String));
        assert!(has("12.5", Tok::Number) && has("true", Tok::Keyword));
    }

    #[test]
    fn toml_tables_keys_and_values() {
        let t = toks(Lang::Toml, "[workspace.package]");
        assert_eq!(t, [("[workspace.package]".to_owned(), Tok::Meta)]);
        let t = toks(Lang::Toml, "rust-version = \"1.89\" # mínima");
        let has = |s: &str, tok| t.iter().any(|(w, k)| w == s && *k == tok);
        assert!(has("rust-version", Tok::Key) && has("\"1.89\"", Tok::String) && has("# mínima", Tok::Comment));
        let t = toks(Lang::Toml, "egui = { version = \"0.25\", default-features = false }");
        assert!(t.iter().any(|(w, k)| w == "egui" && *k == Tok::Key));
        assert!(t.iter().any(|(w, k)| w == "false" && *k == Tok::Keyword));
    }

    #[test]
    fn yaml_keys_lists_and_comments() {
        let t = toks(Lang::Yaml, "  - name: Rodar os testes # nota");
        let has = |s: &str, tok| t.iter().any(|(w, k)| w == s && *k == tok);
        assert!(has("name", Tok::Key) && has("# nota", Tok::Comment));
        let t = toks(Lang::Yaml, "run: cargo test --workspace");
        assert!(t.iter().any(|(w, k)| w == "run" && *k == Tok::Key));
        // Uma URL não é chave.
        assert!(toks(Lang::Yaml, "- http://x.y/z").iter().all(|(_, k)| *k != Tok::Key));
        assert_eq!(toks(Lang::Yaml, "---"), [("---".to_owned(), Tok::Meta)]);
    }

    #[test]
    fn shell_and_powershell_variables_options_and_cmdlets() {
        let t = toks(Lang::Shell, "if [ \"$x\" = a ]; then echo ${HOME} --flag # c");
        let has = |s: &str, tok| t.iter().any(|(w, k)| w == s && *k == tok);
        assert!(has("if", Tok::Keyword) && has("then", Tok::Keyword) && has("${HOME}", Tok::Meta) && has("--flag", Tok::Key));
        assert!(has("# c", Tok::Comment));
        let t = toks(Lang::PowerShell, "Get-ChildItem -Recurse $env:PATH | Where-Object { $_ -ne 1 }");
        let has = |s: &str, tok| t.iter().any(|(w, k)| w == s && *k == tok);
        assert!(has("Get-ChildItem", Tok::Function) && has("-Recurse", Tok::Key) && has("Where-Object", Tok::Function));
        assert!(toks(Lang::PowerShell, "FOREACH ($a in $b) {}").iter().any(|(w, k)| w == "FOREACH" && *k == Tok::Keyword), "sem diferença de caixa");
        // `$#` não abre comentário.
        assert!(toks(Lang::Shell, "echo $#").iter().all(|(_, k)| *k != Tok::Comment));
    }

    #[test]
    fn javascript_templates_decorators_and_types() {
        let lines = highlight_lines(Lang::JsTs, "@Component()\nconst x: string = `a\n b ${c}`; // t");
        assert!(lines[0].iter().any(|s| s.tok == Tok::Meta));
        assert!(lines[1].iter().any(|s| s.tok == Tok::Keyword) && lines[1].iter().any(|s| s.tok == Tok::Type));
        assert!(lines[2].iter().any(|s| s.tok == Tok::String) && lines[2].iter().any(|s| s.tok == Tok::Comment));
    }

    #[test]
    fn markdown_headings_lists_code_and_fenced_blocks_in_their_language() {
        let text = "# Título\n- [ ] item `cod` **forte** [x](http://y)\n> cita\n```rust\nfn a() {}\n```\nfim";
        let lines = highlight_lines(Lang::Markdown, text);
        assert_eq!(lines[0], [Span { range: 0.."# Título".len(), tok: Tok::Meta }]);
        let l1: Vec<Tok> = lines[1].iter().map(|s| s.tok).collect();
        assert!(l1.contains(&Tok::Keyword) && l1.contains(&Tok::Number) && l1.contains(&Tok::String) && l1.contains(&Tok::Function));
        assert!(l1.contains(&Tok::Key) && l1.contains(&Tok::Comment));
        assert_eq!(lines[2][0].tok, Tok::Comment);
        assert_eq!(lines[3][0].tok, Tok::Meta);
        assert!(lines[4].iter().any(|s| s.tok == Tok::Keyword), "o código da cerca é Rust");
        assert_eq!(lines[5][0].tok, Tok::Meta);
        assert!(lines[6].is_empty(), "depois da cerca volta ao texto");
    }

    #[test]
    fn spans_are_always_valid_on_any_text() {
        let nasty = [
            "",
            "ção \"não fechada 中文 🙂 /* x",
            "'é' 'ab 'ç' r#\"ü\" \\ \"\\",
            "#[é] $é ${ 🙂 -ü --é @é.ü 0é 1.é",
            "`🙂 ``` ~~~ ``` [é](ü) **ç** __ü__ - [x] 1. é",
            "key: \"é\": 'ü' # ç",
            "[é] = \"\"\"ü",
            "\u{feff}x = ü",
            "r\"ü",
            "br##\"é\"## 'a",
        ];
        let langs = [
            Lang::Rust, Lang::Markdown, Lang::Toml, Lang::Json, Lang::Yaml, Lang::JsTs, Lang::Python, Lang::PowerShell, Lang::Shell,
        ];
        for lang in langs {
            let text = nasty.join("\n");
            for (line, spans) in text.split('\n').zip(highlight_lines(lang, &text)) {
                let mut end = 0;
                for s in &spans {
                    assert!(s.range.start >= end && s.range.start < s.range.end && s.range.end <= line.len(), "{lang:?} {line:?} {spans:?}");
                    assert!(line.is_char_boundary(s.range.start) && line.is_char_boundary(s.range.end), "{lang:?} {line:?} {spans:?}");
                    end = s.range.end;
                }
            }
        }
    }

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
