//! Memória de longo prazo: os "aprendizados" do projeto.
//!
//! Cada aprendizado é um arquivo `.md` em `memory/aprendizados/`, versionado no git, com um frontmatter:
//!
//! ```text
//! ---
//! titulo: Acentos se perdem na entrada padrão do Windows
//! resumo: PowerShell 5.1 manda stdin em ASCII; texto com acento vai por --file.
//! touches: [crates/core/src/bridge.rs, crates/cli/**]
//! ---
//! O fato, e depois **Por quê:** e **Como aplicar:** em poucas linhas.
//! ```
//!
//! `touches` são globs relativos à raiz do repositório (`*` não cruza `/`, `**` cruza; um padrão sem `/`
//! vale para o nome do arquivo em qualquer pasta). Vazio ou ausente = o aprendizado vale para o projeto todo.
//!
//! Este módulo só tem funções puras (mais a leitura da pasta): o app entrega o pedaço certo a cada agente
//! ([`render`], usado no início da tarefa e por `ask-agent --learn`). Um arquivo mal formado é ignorado com um
//! aviso no log; nunca derruba a entrega dos outros.

use std::path::Path;

use regex::Regex;

/// Pasta dos aprendizados, relativa à raiz do repositório.
pub const DIR: &str = "memory/aprendizados";
/// Orçamento (em caracteres) dos textos completos entregues no início da tarefa; o que passar vira só a linha do índice.
pub const START_BUDGET: usize = 6_000;
/// Orçamento dos textos completos de `ask-agent --learn`.
pub const LEARN_BUDGET: usize = 12_000;
/// Orçamento do índice (uma linha por aprendizado); passando, termina em "e mais N".
const INDEX_BUDGET: usize = 2_500;
/// Arquivos lidos da pasta, e tamanho máximo de cada um: o que passar é ignorado.
const MAX_FILES: usize = 200;
const MAX_FILE_BYTES: u64 = 64 * 1024;
/// Caminhos extraídos de um texto (o resto é ignorado).
const MAX_PATHS: usize = 60;

/// Um aprendizado lido da pasta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Learning {
    /// Caminho relativo à raiz do repositório (`memory/aprendizados/acentos.md`), com `/`.
    pub file: String,
    pub titulo: String,
    pub resumo: String,
    /// Globs dos arquivos a que se aplica; vazio = o projeto todo.
    pub touches: Vec<String>,
    /// O texto depois do frontmatter.
    pub body: String,
}

impl Learning {
    /// Vale para o projeto todo (sem `touches`).
    pub fn is_global(&self) -> bool {
        self.touches.is_empty()
    }

    /// Algum dos `touches` casa com algum dos caminhos (já normalizados, relativos à raiz).
    pub fn touches_any(&self, paths: &[String]) -> bool {
        self.touches.iter().any(|g| paths.iter().any(|p| glob_match(g, p)))
    }

    /// A linha do índice: `- <arquivo> — <resumo>`.
    fn index_line(&self) -> String {
        format!("- {} — {}", self.file, self.resumo)
    }

    /// O texto completo: título, arquivo e corpo.
    fn full_text(&self) -> String {
        format!("### {} ({})\n{}", self.titulo, self.file, self.body)
    }
}

// ---------- Frontmatter ----------

/// Normaliza um caminho: `\` vira `/`, sem `./` no começo nem `/` sobrando nas pontas.
pub fn normalize_path(p: &str) -> String {
    let mut s = p.trim().replace('\\', "/");
    while let Some(rest) = s.strip_prefix("./") {
        s = rest.to_owned();
    }
    s.trim_matches('/').to_owned()
}

/// Um glob de `touches` aceitável: relativo, sem `..`.
fn valid_glob(g: &str) -> bool {
    !g.is_empty() && !g.starts_with('/') && !g.chars().nth(1).is_some_and(|c| c == ':') && !g.split('/').any(|s| s == "..")
}

fn unquote(s: &str) -> &str {
    let s = s.trim();
    for q in ['"', '\''] {
        if let Some(inner) = s.strip_prefix(q).and_then(|r| r.strip_suffix(q)) {
            return inner;
        }
    }
    s
}

/// Lê um arquivo de aprendizado. `file` é só o nome que vai para [`Learning::file`]. Erro = por que foi
/// ignorado (frontmatter ausente ou aberto, título/resumo/corpo vazios, glob inválido).
pub fn parse(file: &str, text: &str) -> Result<Learning, String> {
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let mut lines = text.split_inclusive('\n');
    let first = lines.next().unwrap_or("");
    if first.trim_end() != "---" {
        return Err("falta o frontmatter (o arquivo precisa começar com ---)".into());
    }
    // Posição, no texto, em que a linha seguinte começa: o corpo é o que vem depois do `---` final.
    let mut offset = first.len();
    let (mut titulo, mut resumo, mut touches) = (String::new(), String::new(), Vec::<String>::new());
    let mut closed = false;
    // `touches:` pode vir em lista de uma linha (`[a, b]`) ou em itens `- a` nas linhas seguintes.
    let mut in_list = false;
    for line in lines {
        offset += line.len();
        let line = line.trim_end_matches('\n');
        if line.trim_end() == "---" {
            closed = true;
            break;
        }
        if in_list {
            if let Some(item) = line.trim_start().strip_prefix('-') {
                touches.push(unquote(item).to_owned());
                continue;
            }
            in_list = false;
        }
        let Some((key, value)) = line.split_once(':') else { continue };
        let value = value.trim();
        match key.trim() {
            "titulo" => titulo = unquote(value).to_owned(),
            "resumo" => resumo = unquote(value).to_owned(),
            "touches" if value.is_empty() => in_list = true,
            "touches" => {
                let inner = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')).unwrap_or(value);
                touches.extend(inner.split(',').map(|g| unquote(g).to_owned()));
            }
            _ => {}
        }
    }
    if !closed {
        return Err("o frontmatter não foi fechado (falta o --- final)".into());
    }
    let (titulo, resumo) = (one_line(&titulo), one_line(&resumo));
    if titulo.is_empty() || resumo.is_empty() {
        return Err("titulo e resumo são obrigatórios".into());
    }
    // Valida antes de normalizar: `normalize_path` tira a `/` inicial e esconderia um caminho absoluto.
    let touches: Vec<String> = touches.iter().map(|g| g.trim().replace('\\', "/")).filter(|g| !g.is_empty()).collect();
    if let Some(bad) = touches.iter().find(|g| !valid_glob(g)) {
        return Err(format!("touches inválido: \"{bad}\" (use caminhos relativos à raiz do repositório, sem ..)"));
    }
    let touches: Vec<String> = touches.iter().map(|g| normalize_path(g)).collect();
    let body = text.get(offset..).unwrap_or("").trim().to_owned();
    if body.is_empty() {
        return Err("o corpo está vazio: conte o fato, o porquê e como aplicar".into());
    }
    Ok(Learning { file: file.to_owned(), titulo, resumo, touches, body })
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Lê `memory/aprendizados/*.md` de `root`, em ordem de nome. Pasta ausente = nenhum aprendizado. Arquivo
/// inválido, grande demais ou ilegível é ignorado com um aviso no log (o `README.md` da pasta nunca conta).
pub fn load(root: &Path) -> Vec<Learning> {
    let dir = root.join(DIR);
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut files: Vec<_> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("md")))
        .filter(|e| !e.file_name().to_string_lossy().eq_ignore_ascii_case("README.md"))
        .collect();
    files.sort_by_key(|e| e.file_name());
    let mut out = Vec::new();
    for entry in files.into_iter().take(MAX_FILES) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = format!("{DIR}/{name}");
        if entry.metadata().map_or(true, |m| !m.is_file() || m.len() > MAX_FILE_BYTES) {
            log::warn!("aprendizado ignorado ({rel}): não é um arquivo ou passa de {} KB", MAX_FILE_BYTES / 1024);
            continue;
        }
        match std::fs::read(entry.path()) {
            Ok(raw) => match parse(&rel, &crate::util::decode_text(&raw)) {
                Ok(l) => out.push(l),
                Err(e) => log::warn!("aprendizado ignorado ({rel}): {e}"),
            },
            Err(e) => log::warn!("aprendizado ignorado ({rel}): {e}"),
        }
    }
    out
}

// ---------- Globs ----------

/// `*` (qualquer coisa menos `/`) e `?` (um caractere) dentro de um segmento; sem diferença de caixa.
fn segment_match(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ti));
            pi += 1;
        } else if pi < p.len() && (p[pi] == '?' || p[pi].eq_ignore_ascii_case(&t[ti])) {
            pi += 1;
            ti += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

fn segments_match(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|skip| segments_match(rest, &path[skip..])),
        Some((first, rest)) => path.split_first().is_some_and(|(head, tail)| segment_match(first, head) && segments_match(rest, tail)),
    }
}

/// Se o glob `pattern` casa com o caminho `path` (ambos relativos à raiz, com `/`). Um padrão sem `/`
/// (`*.rs`, `Cargo.toml`) vale para o nome do arquivo em qualquer pasta. `crates/cli/**` casa com tudo
/// dentro de `crates/cli`.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let (pattern, path) = (normalize_path(pattern), normalize_path(path));
    if pattern.is_empty() || path.is_empty() {
        return false;
    }
    let segs: Vec<&str> = path.split('/').collect();
    if !pattern.contains('/') {
        return pattern == "**" || segs.last().is_some_and(|name| segment_match(&pattern, name));
    }
    segments_match(&pattern.split('/').collect::<Vec<_>>(), &segs)
}

/// Os aprendizados que valem para `paths`: os globais e os cujo `touches` casa. Primeiro os específicos
/// (mais relevantes), depois os globais; cada grupo na ordem da pasta.
pub fn matching<'a>(all: &'a [Learning], paths: &[String]) -> Vec<&'a Learning> {
    let specific = all.iter().filter(|l| !l.is_global() && l.touches_any(paths));
    let global = all.iter().filter(|l| l.is_global());
    specific.chain(global).collect()
}

// ---------- Caminhos citados num texto ----------

/// Os arquivos que `text` cita e que existem em `root` (só arquivos, dentro do repositório), sem repetição, na
/// ordem em que aparecem. Aceita `/` e `\`; ignora `..` e caminhos absolutos.
pub fn paths_in_text(text: &str, root: &Path) -> Vec<String> {
    use std::sync::OnceLock;
    static TOKEN: OnceLock<Regex> = OnceLock::new();
    let token = TOKEN.get_or_init(|| Regex::new(r"[A-Za-z0-9_.\-]+(?:[/\\][A-Za-z0-9_.\-]+)*").expect("regex fixa"));
    let mut out: Vec<String> = Vec::new();
    for m in token.find_iter(text) {
        // Caminho absoluto (`/etc/x`, `C:\repo\x`: o pedaço depois da `/` ou `\`) ou com `..` não vale: o
        // repositório é a raiz, e nada sai dele.
        let after_separator = text[..m.start()].ends_with(['/', '\\']);
        if after_separator || m.as_str().split(['/', '\\']).any(|s| s == "..") {
            continue;
        }
        let path = normalize_path(m.as_str().trim_end_matches('.'));
        // Uma palavra solta sem ponto (`tasks`) não é um arquivo.
        if path.is_empty() || !path.contains(['/', '.']) || path.split('/').any(|s| s.is_empty() || s == "." || s == "..") {
            continue;
        }
        if out.contains(&path) || !root.join(&path).is_file() {
            continue;
        }
        out.push(path);
        if out.len() >= MAX_PATHS {
            break;
        }
    }
    out
}

// ---------- Entrega ----------

/// Corta `text` em `max` caracteres, com reticências.
fn clip(text: &str, max: usize) -> String {
    let (head, cut) = crate::util::truncate_chars(text, max);
    if cut {
        format!("{}…", head.trim_end())
    } else {
        head.to_owned()
    }
}

/// O texto a entregar a um agente: os aprendizados que valem para `paths` por inteiro, enquanto couberem em
/// `budget` caracteres; o que passar do orçamento e os demais aprendizados viram uma linha de índice
/// (arquivo + resumo). Vazio quando a pasta não tem aprendizado algum.
pub fn render(all: &[Learning], paths: &[String], budget: usize) -> String {
    if all.is_empty() {
        return String::new();
    }
    let relevant = matching(all, paths);
    let mut used = 0usize;
    let mut full: Vec<String> = Vec::new();
    let mut index: Vec<&Learning> = Vec::new();
    for l in &relevant {
        let text = l.full_text();
        let cost = text.chars().count();
        if used + cost <= budget {
            used += cost;
            full.push(text);
        } else {
            index.push(l);
        }
    }
    index.extend(all.iter().filter(|l| !relevant.iter().any(|r| r.file == l.file)));

    let mut out = String::new();
    if !full.is_empty() {
        out.push_str("Valem para este trabalho (projeto todo ou os arquivos em questão):\n\n");
        out.push_str(&full.join("\n\n"));
    }
    if !index.is_empty() {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str("Outros aprendizados (abra o arquivo, ou rode ask-agent --learn <arquivo> antes de mexer num arquivo relacionado):\n");
        let mut lines = String::new();
        for (n, l) in index.iter().enumerate() {
            let line = clip(&l.index_line(), 220);
            if lines.chars().count() + line.chars().count() + 1 > INDEX_BUDGET {
                lines.push_str(&format!("- … e mais {} (veja a pasta {DIR})\n", index.len() - n));
                break;
            }
            lines.push_str(&line);
            lines.push('\n');
        }
        out.push_str(lines.trim_end());
    }
    out
}

/// Resposta de `ask-agent --learn [<caminho>…]`: o que o projeto aprendeu sobre esses arquivos; sem caminhos, os
/// globais por inteiro e o índice dos específicos.
pub fn learn_report(all: &[Learning], paths: &[String]) -> String {
    if all.is_empty() {
        return format!("O projeto ainda não tem aprendizados ({DIR}/ está vazia ou não existe).");
    }
    if paths.is_empty() {
        return format!("Aprendizados do projeto:

{}", render(all, &[], LEARN_BUDGET));
    }
    let specific = all.iter().filter(|l| !l.is_global() && l.touches_any(paths)).count();
    let body = render(all, paths, LEARN_BUDGET);
    let head = if specific == 0 {
        format!("Nenhum aprendizado específico para: {}. Valem só os globais, se houver:\n\n", paths.join(", "))
    } else {
        format!("Aprendizados para: {}\n\n", paths.join(", "))
    };
    format!("{head}{body}")
}

/// `path` relativo a uma das `roots` (um caminho absoluto dentro da worktree ou do repositório vira o caminho
/// do repositório); qualquer outro é só normalizado.
pub fn relativize(path: &str, roots: &[std::path::PathBuf]) -> String {
    let norm = normalize_path(path);
    for root in roots {
        let root = normalize_path(&crate::util::strip_verbatim(root).to_string_lossy());
        let inside =
            norm.get(..root.len()).is_some_and(|head| head.eq_ignore_ascii_case(&root)) && norm.as_bytes().get(root.len()) == Some(&b'/');
        if !root.is_empty() && inside {
            return norm[root.len() + 1..].to_owned();
        }
    }
    norm
}

impl crate::Core {
    /// Resposta de `ask-agent --learn`: o que o projeto aprendeu sobre `paths`. Lê a pasta de aprendizados da
    /// worktree da tarefa de `instance` (onde o agente trabalha, com os aprendizados que ele mesmo criou) ou, sem
    /// tarefa, do repositório do projeto.
    pub fn learn(&self, project: &str, instance: &str, paths: &[String]) -> Result<String, String> {
        let project_root = self.project_path(project).and_then(|p| self.git_repo_at(&p).ok()).map(|r| r.root);
        let work_root = self.task_cwd(project, instance).or_else(|| project_root.clone());
        let Some(work_root) = work_root else {
            return Err("não achei o repositório do projeto: os aprendizados moram em memory/aprendizados/ dele".into());
        };
        let roots: Vec<std::path::PathBuf> = [Some(work_root.clone()), project_root].into_iter().flatten().collect();
        let paths: Vec<String> = paths.iter().map(|p| relativize(p, &roots)).filter(|p| !p.is_empty()).collect();
        Ok(learn_report(&load(&work_root), &paths))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "---\ntitulo: Acentos\nresumo: Use --file.\ntouches: [crates/cli/**, \"crates/core/src/bridge.rs\"]\n---\nO fato.\n\n**Por quê:** x.\n";

    fn learning(file: &str, touches: &[&str], body: &str) -> Learning {
        Learning {
            file: format!("{DIR}/{file}.md"),
            titulo: format!("T {file}"),
            resumo: format!("R {file}"),
            touches: touches.iter().map(|s| (*s).to_owned()).collect(),
            body: body.to_owned(),
        }
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn a_well_formed_file_is_read() {
        let l = parse("memory/aprendizados/a.md", GOOD).unwrap();
        assert_eq!((l.titulo.as_str(), l.resumo.as_str()), ("Acentos", "Use --file."));
        assert_eq!(l.touches, ["crates/cli/**", "crates/core/src/bridge.rs"]);
        assert_eq!(l.body, "O fato.\n\n**Por quê:** x.");
        assert!(!l.is_global());
    }

    #[test]
    fn touches_can_be_missing_empty_or_a_dash_list() {
        let global = "---\ntitulo: A\nresumo: B\n---\ncorpo";
        assert!(parse("a.md", global).unwrap().is_global());
        assert!(parse("a.md", "---\ntitulo: A\nresumo: B\ntouches: []\n---\ncorpo").unwrap().is_global());
        let list = "---\ntitulo: A\nresumo: B\ntouches:\n  - src/a.rs\n  - \"x/**\"\n---\ncorpo";
        assert_eq!(parse("a.md", list).unwrap().touches, ["src/a.rs", "x/**"]);
        // CRLF e BOM (o Windows grava assim).
        let crlf = "\u{feff}---\r\ntitulo: A\r\nresumo: B\r\ntouches: [a.rs]\r\n---\r\ncorpo\r\n";
        let l = parse("a.md", crlf).unwrap();
        assert_eq!((l.touches, l.body.as_str()), (strings(&["a.rs"]), "corpo"));
    }

    #[test]
    fn a_broken_frontmatter_is_rejected_with_a_reason() {
        for (text, why) in [
            ("sem frontmatter", "frontmatter"),
            ("---\ntitulo: A\nresumo: B\ncorpo sem fechar", "fechado"),
            ("---\nresumo: B\n---\ncorpo", "obrigatórios"),
            ("---\ntitulo: A\n---\ncorpo", "obrigatórios"),
            ("---\ntitulo: A\nresumo: B\n---\n   \n", "corpo"),
            ("---\ntitulo: A\nresumo: B\ntouches: [../fora.rs]\n---\ncorpo", "touches"),
            ("---\ntitulo: A\nresumo: B\ntouches: [/abs/a.rs]\n---\ncorpo", "touches"),
            ("---\ntitulo: A\nresumo: B\ntouches: [C:/a.rs]\n---\ncorpo", "touches"),
        ] {
            let err = parse("a.md", text).unwrap_err();
            assert!(err.contains(why), "{text:?} → {err}");
        }
    }

    #[test]
    fn globs_follow_star_and_double_star() {
        // `*` não cruza pasta; `**` cruza (e casa zero pastas).
        assert!(glob_match("crates/core/src/*.rs", "crates/core/src/tasks.rs"));
        assert!(!glob_match("crates/core/src/*.rs", "crates/core/src/usage/mod.rs"));
        assert!(glob_match("crates/cli/**", "crates/cli/src/bin/ask-agent.rs"));
        assert!(glob_match("crates/**/ask-agent.rs", "crates/cli/src/bin/ask-agent.rs"));
        assert!(glob_match("crates/**/tasks.rs", "crates/tasks.rs"));
        assert!(!glob_match("crates/cli/**", "crates/core/src/x.rs"));
        assert!(glob_match("crates/core/src/ta?ks.rs", "crates/core/src/tasks.rs"));
        assert!(glob_match("memory/*.md", "memory/GERAL.md"));
        assert!(!glob_match("memory/*.md", "memory/aprendizados/a.md"));
        // Sem barra: o nome do arquivo em qualquer pasta. Barras invertidas e `./` são normalizados.
        assert!(glob_match("*.rs", "crates/core/src/tasks.rs"));
        assert!(glob_match("Cargo.toml", "crates/cli/Cargo.toml"));
        assert!(!glob_match("*.rs", "crates/core/README.md"));
        assert!(glob_match("crates/cli/**", ".\\crates\\cli\\src\\a.rs"));
        // Sem diferença de caixa (o Windows não distingue).
        assert!(glob_match("Crates/CLI/**", "crates/cli/a.rs"));
        // Padrão ou caminho vazio nunca casa.
        assert!(!glob_match("", "a.rs") && !glob_match("a.rs", ""));
    }

    #[test]
    fn matching_puts_the_specific_before_the_global() {
        let all =
            vec![learning("a-global", &[], "g"), learning("b-cli", &["crates/cli/**"], "c"), learning("c-ui", &["crates/app/**"], "u")];
        let got: Vec<&str> = matching(&all, &strings(&["crates/cli/src/x.rs"])).iter().map(|l| l.file.as_str()).collect();
        assert_eq!(got, [format!("{DIR}/b-cli.md"), format!("{DIR}/a-global.md")]);
        // Sem arquivos citados, só os globais.
        assert_eq!(matching(&all, &[]).len(), 1);
    }

    #[test]
    fn paths_are_taken_from_the_text_only_when_they_exist() {
        let dir = tempfile::tempdir().unwrap();
        for f in ["crates/core/src/tasks.rs", "memory/AGENTDECK.md", "Cargo.toml"] {
            let p = dir.path().join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "x").unwrap();
        }
        let text = "Leia `memory/AGENTDECK.md` e crates/core/src/tasks.rs (ver `start_message`); \
                    crates\\core\\src\\tasks.rs de novo, Cargo.toml, ../fora.rs, crates/core/src/nao_existe.rs e crates/core (pasta).";
        assert_eq!(paths_in_text(text, dir.path()), ["memory/AGENTDECK.md", "crates/core/src/tasks.rs", "Cargo.toml"]);
        assert!(paths_in_text("nada aqui, só palavras. e ponto.", dir.path()).is_empty());
        // Uma pasta não é arquivo; `..` e caminhos absolutos não escapam do repositório.
        assert!(paths_in_text("crates/core ../Cargo.toml /etc/passwd", dir.path()).is_empty());
    }

    #[test]
    fn the_budget_turns_the_overflow_into_index_lines() {
        let big = "x".repeat(300);
        let all = vec![
            learning("a-cli", &["crates/cli/**"], &big),
            learning("b-cli", &["crates/cli/**"], &big),
            learning("c-ui", &["crates/app/**"], "u"),
        ];
        let out = render(&all, &strings(&["crates/cli/a.rs"]), 400);
        // Só o primeiro cabe por inteiro; o segundo vira linha de índice, junto com o que não casa.
        assert!(out.contains("### T a-cli") && !out.contains("### T b-cli") && !out.contains("### T c-ui"), "{out}");
        assert!(out.contains(&format!("- {DIR}/b-cli.md — R b-cli")), "{out}");
        assert!(out.contains(&format!("- {DIR}/c-ui.md — R c-ui")), "{out}");
        // Com folga, tudo o que casa vem por inteiro e o resto fica no índice.
        let out = render(&all, &strings(&["crates/cli/a.rs"]), 10_000);
        assert!(out.contains("### T a-cli") && out.contains("### T b-cli") && !out.contains("### T c-ui"), "{out}");
        assert!(out.contains("c-ui.md — R c-ui") && !out.contains("b-cli.md — R"), "{out}");
        // Pasta vazia: nada a entregar.
        assert_eq!(render(&[], &[], 100), "");
    }

    #[test]
    fn a_long_index_ends_with_a_count() {
        let all: Vec<Learning> = (0..80).map(|n| learning(&format!("n{n:03}"), &["x/**"], &"corpo ".repeat(40))).collect();
        let out = render(&all, &[], 0);
        assert!(out.contains("- … e mais ") && out.chars().count() < INDEX_BUDGET + 400, "{}", out.chars().count());
    }

    #[test]
    fn the_learn_report_says_when_nothing_matches() {
        let all = vec![learning("a", &["crates/cli/**"], "c"), learning("g", &[], "global")];
        let hit = learn_report(&all, &strings(&["crates/cli/x.rs"]));
        assert!(hit.starts_with("Aprendizados para: crates/cli/x.rs") && hit.contains("### T a") && hit.contains("### T g"), "{hit}");
        let miss = learn_report(&all, &strings(&["docs/x.md"]));
        assert!(miss.contains("Nenhum aprendizado específico") && !miss.contains("### T a") && miss.contains("### T g"), "{miss}");
        assert!(learn_report(&[], &strings(&["a.rs"])).contains("ainda não tem aprendizados"));
    }

    #[test]
    fn the_learn_report_without_paths_gives_the_globals_and_the_index() {
        let all = vec![learning("a", &["crates/cli/**"], "c"), learning("g", &[], "global")];
        let out = learn_report(&all, &[]);
        assert!(out.starts_with("Aprendizados do projeto:") && !out.contains("Nenhum aprendizado específico"), "{out}");
        assert!(out.contains("### T g") && out.contains("global"), "{out}");
        assert!(!out.contains("### T a") && out.contains(&format!("- {DIR}/a.md — R a")), "{out}");
        assert!(learn_report(&[], &[]).contains("ainda não tem aprendizados"));
    }

    #[test]
    fn the_folder_is_read_and_a_bad_file_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        // Pasta ausente: nenhum aprendizado, nenhum erro.
        assert!(load(dir.path()).is_empty());
        let learn = dir.path().join(DIR);
        std::fs::create_dir_all(&learn).unwrap();
        std::fs::write(learn.join("b.md"), GOOD).unwrap();
        std::fs::write(learn.join("a.md"), "---\ntitulo: A\nresumo: B\n---\ncorpo").unwrap();
        std::fs::write(learn.join("quebrado.md"), "sem frontmatter").unwrap();
        std::fs::write(learn.join("README.md"), "---\ntitulo: R\nresumo: R\n---\nnão conta").unwrap();
        std::fs::write(learn.join("nota.txt"), GOOD).unwrap();
        let got = load(dir.path());
        assert_eq!(got.iter().map(|l| l.file.as_str()).collect::<Vec<_>>(), [format!("{DIR}/a.md"), format!("{DIR}/b.md")]);
    }

    #[test]
    fn every_learning_in_the_repository_is_valid() {
        // Os aprendizados reais do projeto: um frontmatter quebrado seria ignorado em silêncio no app.
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let files = std::fs::read_dir(root.join(DIR))
            .expect("memory/aprendizados existe")
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|x| x == "md") && e.file_name() != "README.md")
            .count();
        let loaded = load(&root);
        assert!(files >= 4, "o projeto deve ter aprendizados semeados");
        assert_eq!(loaded.len(), files, "algum aprendizado da pasta não passou na validação");
        // Cada `touches` literal (sem curinga) aponta para um arquivo que existe.
        for l in &loaded {
            for g in l.touches.iter().filter(|g| !g.contains(['*', '?'])) {
                assert!(root.join(g).exists(), "{}: touches aponta para {g}, que não existe", l.file);
            }
        }
    }

    #[test]
    fn absolute_paths_become_repository_paths() {
        let roots = [std::path::PathBuf::from("C:/repo/.agentdeck/worktrees/t13"), std::path::PathBuf::from("C:/repo")];
        assert_eq!(relativize(r"C:\repo\.agentdeck\worktrees\t13\crates\cli\a.rs", &roots), "crates/cli/a.rs");
        assert_eq!(relativize("c:/REPO/crates/core/x.rs", &roots), "crates/core/x.rs");
        assert_eq!(relativize(r"./crates\app\y.rs", &roots), "crates/app/y.rs");
        // Fora das raízes, e prefixo parcial de nome de pasta: só normaliza.
        assert_eq!(relativize("C:/outro/z.rs", &roots), "C:/outro/z.rs");
        assert_eq!(relativize("C:/repository/z.rs", &roots), "C:/repository/z.rs");
    }
}
