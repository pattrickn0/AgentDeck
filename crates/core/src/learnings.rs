//! Long-term memory: the project's "learnings".
//!
//! Each learning is a `.md` file in `memory/learnings/`, versioned in git, with a frontmatter:
//!
//! ```text
//! ---
//! title: Accents get lost on the Windows standard input
//! summary: PowerShell 5.1 sends stdin as ASCII; text with accents goes through --file.
//! touches: [crates/core/src/bridge.rs, crates/cli/**]
//! ---
//! The fact, then **Why:** and **How to apply:** in a few lines.
//! ```
//!
//! The old Portuguese keys `titulo:`/`resumo:` are still accepted; `title:`/`summary:` are preferred.
//!
//! `touches` are globs relative to the repository root (`*` does not cross `/`, `**` does; a pattern without
//! `/` applies to the file name in any folder). Empty or missing = the learning applies to the whole project.
//!
//! This module has only pure functions (plus reading the folder): the app hands the right piece to each agent
//! ([`render`], used at the start of a task and by `ask-agent --learn`). A malformed file is skipped with a
//! warning in the log; it never takes down the delivery of the others.

use std::path::Path;

use regex::Regex;

/// Folder of the learnings, relative to the repository root.
pub const DIR: &str = "memory/learnings";

/// The folder's name before the English rename. A project that still has it (and not [`DIR`]) is read from there.
pub const LEGACY_DIR: &str = "memory/aprendizados";

/// The learnings folder of `root`, relative to it: [`DIR`], or [`LEGACY_DIR`] when only that one exists.
pub fn dir_of(root: &Path) -> &'static str {
    if !root.join(DIR).is_dir() && root.join(LEGACY_DIR).is_dir() {
        LEGACY_DIR
    } else {
        DIR
    }
}
/// Budget (in characters) of the full texts delivered at the start of a task; what goes over becomes only the index line.
pub const START_BUDGET: usize = 6_000;
/// Budget of the full texts of `ask-agent --learn`.
pub const LEARN_BUDGET: usize = 12_000;
/// Budget of the index (one line per learning); going over it ends with "and N more".
const INDEX_BUDGET: usize = 2_500;
/// Files read from the folder, and the maximum size of each: what goes over is ignored.
const MAX_FILES: usize = 200;
const MAX_FILE_BYTES: u64 = 64 * 1024;
/// Paths extracted from one text (the rest is ignored).
const MAX_PATHS: usize = 60;

/// A learning read from the folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Learning {
    /// Path relative to the repository root (`memory/learnings/accents.md`), with `/`.
    pub file: String,
    pub titulo: String,
    pub resumo: String,
    /// Globs of the files it applies to; empty = the whole project.
    pub touches: Vec<String>,
    /// The text after the frontmatter.
    pub body: String,
}

impl Learning {
    /// Applies to the whole project (no `touches`).
    pub fn is_global(&self) -> bool {
        self.touches.is_empty()
    }

    /// Any of the `touches` matches any of the paths (already normalized, relative to the root).
    pub fn touches_any(&self, paths: &[String]) -> bool {
        self.touches.iter().any(|g| paths.iter().any(|p| glob_match(g, p)))
    }

    /// The index line: `- <file> — <summary>`.
    fn index_line(&self) -> String {
        format!("- {} — {}", self.file, self.resumo)
    }

    /// The full text: title, file and body.
    pub fn full_text(&self) -> String {
        format!("### {} ({})\n{}", self.titulo, self.file, self.body)
    }
}

// ---------- Frontmatter ----------

/// Normalizes a path: `\` becomes `/`, no `./` at the start nor a leftover `/` at the ends.
pub fn normalize_path(p: &str) -> String {
    let mut s = p.trim().replace('\\', "/");
    while let Some(rest) = s.strip_prefix("./") {
        s = rest.to_owned();
    }
    s.trim_matches('/').to_owned()
}

/// An acceptable `touches` glob: relative, without `..`.
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

/// Reads a learning file. `file` is only the name that goes to [`Learning::file`]. Error = why it was
/// ignored (missing or unclosed frontmatter, empty title/summary/body, invalid glob).
pub fn parse(file: &str, text: &str) -> Result<Learning, String> {
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let mut lines = text.split_inclusive('\n');
    let first = lines.next().unwrap_or("");
    if first.trim_end() != "---" {
        return Err("missing frontmatter (the file must start with ---)".into());
    }
    // Position, in the text, where the next line starts: the body is what comes after the final `---`.
    let mut offset = first.len();
    let (mut titulo, mut resumo, mut touches) = (String::new(), String::new(), Vec::<String>::new());
    let mut closed = false;
    // `touches:` may come as a one-line list (`[a, b]`) or as `- a` items on the following lines.
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
            // English keys are the preferred ones; the Portuguese ones from before keep working.
            "title" | "titulo" => titulo = unquote(value).to_owned(),
            "summary" | "resumo" => resumo = unquote(value).to_owned(),
            "touches" if value.is_empty() => in_list = true,
            "touches" => {
                let inner = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')).unwrap_or(value);
                touches.extend(inner.split(',').map(|g| unquote(g).to_owned()));
            }
            _ => {}
        }
    }
    if !closed {
        return Err("the frontmatter is not closed (the final --- is missing)".into());
    }
    let (titulo, resumo) = (one_line(&titulo), one_line(&resumo));
    if titulo.is_empty() || resumo.is_empty() {
        return Err("title and summary (or titulo and resumo) are required".into());
    }
    // Validate before normalizing: `normalize_path` strips the leading `/` and would hide an absolute path.
    let touches: Vec<String> = touches.iter().map(|g| g.trim().replace('\\', "/")).filter(|g| !g.is_empty()).collect();
    if let Some(bad) = touches.iter().find(|g| !valid_glob(g)) {
        return Err(format!("invalid touches: \"{bad}\" (use paths relative to the repository root, without ..)"));
    }
    let touches: Vec<String> = touches.iter().map(|g| normalize_path(g)).collect();
    let body = text.get(offset..).unwrap_or("").trim().to_owned();
    if body.is_empty() {
        return Err("the body is empty: tell the fact, the why and how to apply".into());
    }
    Ok(Learning { file: file.to_owned(), titulo, resumo, touches, body })
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Reads `memory/learnings/*.md` (or the legacy `memory/aprendizados/*.md`, see [`dir_of`]) from `root`, in name order. Missing folder = no learning. Invalid, too
/// large or unreadable file is skipped with a warning in the log (the folder `README.md` never counts).
pub fn load(root: &Path) -> Vec<Learning> {
    let base = dir_of(root);
    let dir = root.join(base);
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
        let rel = format!("{base}/{name}");
        if entry.metadata().map_or(true, |m| !m.is_file() || m.len() > MAX_FILE_BYTES) {
            log::warn!("learning ignored ({rel}): it is not a file or it goes over {} KB", MAX_FILE_BYTES / 1024);
            continue;
        }
        match std::fs::read(entry.path()) {
            Ok(raw) => match parse(&rel, &crate::util::decode_text(&raw)) {
                Ok(l) => out.push(l),
                Err(e) => log::warn!("learning ignored ({rel}): {e}"),
            },
            Err(e) => log::warn!("learning ignored ({rel}): {e}"),
        }
    }
    out
}

// ---------- Globs ----------

/// `*` (anything but `/`) and `?` (one character) inside a segment; no case difference.
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

/// Whether the glob `pattern` matches the path `path` (both relative to the root, with `/`). A pattern without
/// `/` (`*.rs`, `Cargo.toml`) applies to the file name in any folder. `crates/cli/**` matches everything
/// inside `crates/cli`.
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

/// The learnings that apply to `paths`: the global ones and the ones whose `touches` match. The specific ones
/// first (most relevant), then the global ones; each group in the folder order.
pub fn matching<'a>(all: &'a [Learning], paths: &[String]) -> Vec<&'a Learning> {
    let specific = all.iter().filter(|l| !l.is_global() && l.touches_any(paths));
    let global = all.iter().filter(|l| l.is_global());
    specific.chain(global).collect()
}

// ---------- Paths cited in a text ----------

/// The files that `text` cites and that exist in `root` (files only, inside the repository), without repetition,
/// in the order they appear. Accepts `/` and `\`; ignores `..` and absolute paths.
pub fn paths_in_text(text: &str, root: &Path) -> Vec<String> {
    paths_in_text_where(text, |path| root.join(path).is_file())
}

/// Like [`paths_in_text`], with `exists` in place of the disk check (it receives the path already normalized,
/// relative to the root). It serves whoever checks the paths against another source, such as a commit tree.
pub fn paths_in_text_where(text: &str, exists: impl Fn(&str) -> bool) -> Vec<String> {
    use std::sync::OnceLock;
    static TOKEN: OnceLock<Regex> = OnceLock::new();
    let token = TOKEN.get_or_init(|| Regex::new(r"[A-Za-z0-9_.\-]+(?:[/\\][A-Za-z0-9_.\-]+)*").expect("fixed regex"));
    let mut out: Vec<String> = Vec::new();
    for m in token.find_iter(text) {
        // An absolute path (`/etc/x`, `C:\repo\x`: the piece after the `/` or `\`) or one with `..` does not
        // count: the repository is the root, and nothing leaves it.
        let after_separator = text[..m.start()].ends_with(['/', '\\']);
        if after_separator || m.as_str().split(['/', '\\']).any(|s| s == "..") {
            continue;
        }
        let path = normalize_path(m.as_str().trim_end_matches('.'));
        // A lone word without a dot (`tasks`) is not a file.
        if path.is_empty() || !path.contains(['/', '.']) || path.split('/').any(|s| s.is_empty() || s == "." || s == "..") {
            continue;
        }
        if out.contains(&path) || !exists(&path) {
            continue;
        }
        out.push(path);
        if out.len() >= MAX_PATHS {
            break;
        }
    }
    out
}

// ---------- Delivery ----------

/// Cuts `text` at `max` characters, with an ellipsis.
fn clip(text: &str, max: usize) -> String {
    let (head, cut) = crate::util::truncate_chars(text, max);
    if cut {
        format!("{}…", head.trim_end())
    } else {
        head.to_owned()
    }
}

/// The text to hand to an agent: the learnings that apply to `paths` in full, while they fit in `budget`
/// characters; what goes over the budget and the remaining learnings become one index line (file + summary).
/// Empty when the folder has no learning at all.
pub fn render(all: &[Learning], paths: &[String], budget: usize) -> String {
    render_with(all, paths, budget, INDEX_BUDGET)
}

/// Header of the index in [`render_with`].
const INDEX_HEADER: &str = "Other learnings (open the file, or run ask-agent --learn <file> before touching a related file):\n";

/// Like [`render`], with the cap of the index lines in `index_budget` characters; `0` drops the index.
pub fn render_with(all: &[Learning], paths: &[String], budget: usize, index_budget: usize) -> String {
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
        out.push_str("Apply to this work (whole project or the files in question):\n\n");
        out.push_str(&full.join("\n\n"));
    }
    if !index.is_empty() && index_budget > 0 {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(INDEX_HEADER);
        let mut lines = String::new();
        for (n, l) in index.iter().enumerate() {
            let line = clip(&l.index_line(), 220);
            if lines.chars().count() + line.chars().count() + 1 > index_budget {
                lines.push_str(&format!("- … and {} more (see the {DIR} folder)\n", index.len() - n));
                break;
            }
            lines.push_str(&line);
            lines.push('\n');
        }
        out.push_str(lines.trim_end());
    }
    out
}

/// How to create a learning, for the project that has neither the folder nor the `README.md` with the format.
const FORMAT_HELP: &str = "To create one, write memory/learnings/<short-name>.md (one per trap, decision or convention that cost time and the code does not show):

---
title: one line
summary: one line (it is what shows in the index)
touches: [globs/of/files/**, another.rs]   # optional; without it, it applies to the whole project
---
The fact, then **Why:** and **How to apply:** in a few lines.

Versioned in git, it goes through the same review as the code.";

/// Budget of the learnings delivered in a return or a rejection ([`render_specific`]).
pub const FEEDBACK_BUDGET: usize = 3_000;

/// Only the specific learnings that match `paths`, in full, while they fit in `budget` characters; no index and
/// no global ones (the agent already got them at the start of the task). Empty if `paths` is empty or nothing matches.
pub fn render_specific(all: &[Learning], paths: &[String], budget: usize) -> String {
    let mut used = 0usize;
    let mut full: Vec<String> = Vec::new();
    for l in all.iter().filter(|l| !l.is_global() && l.touches_any(paths)) {
        let text = l.full_text();
        let cost = text.chars().count();
        if used + cost > budget {
            break;
        }
        used += cost;
        full.push(text);
    }
    full.join("\n\n")
}

/// Answer of `ask-agent --learn [<path>…]`: what the project learned about those files; without paths, the
/// global ones in full and the index of the specific ones.
pub fn learn_report(all: &[Learning], paths: &[String]) -> String {
    learn_report_with(all, paths, LEARN_BUDGET, INDEX_BUDGET)
}

/// [`learn_report`] with the caps of the full learnings (`budget`) and of the index lines (`index_budget`).
fn learn_report_with(all: &[Learning], paths: &[String], budget: usize, index_budget: usize) -> String {
    if all.is_empty() {
        return format!(
            "The project has no learnings yet ({DIR}/ is empty or does not exist).

{FORMAT_HELP}"
        );
    }
    if paths.is_empty() {
        return format!(
            "Project learnings:

{}",
            render_with(all, &[], budget, index_budget)
        );
    }
    let specific = all.iter().filter(|l| !l.is_global() && l.touches_any(paths)).count();
    let body = render_with(all, paths, budget, index_budget);
    let head = if specific == 0 {
        format!("No specific learning for: {}. Only the global ones apply, if any:\n\n", paths.join(", "))
    } else {
        format!("Learnings for: {}\n\n", paths.join(", "))
    };
    format!("{head}{body}")
}

/// `path` relative to one of the `roots` (an absolute path inside the worktree or the repository becomes the
/// repository path); any other is only normalized.
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
    /// Answer of `ask-agent --learn`: what the project learned about `paths`. It reads the learnings folder of the
    /// worktree of `instance`'s task (where the agent works, with the learnings it created itself) or, without a
    /// task, of the project repository. It adds the current situation of the project ([`crate::situation`]): with
    /// `paths`, at the top, the warning about the ones another running task also touches; without `paths`, at the
    /// end, the whole section. The agent of a task does not see itself in it.
    pub fn learn(&self, project: &str, instance: &str, paths: &[String]) -> Result<String, String> {
        let repo = self.project_path(project).and_then(|p| self.git_repo_at(&p).ok());
        let project_root = repo.as_ref().map(|r| r.root.clone());
        let work_root = self.task_cwd(project, instance).or_else(|| project_root.clone());
        let Some(work_root) = work_root else {
            return Err("could not find the project repository: the learnings live in its memory/learnings/".into());
        };
        let roots: Vec<std::path::PathBuf> = [Some(work_root.clone()), project_root].into_iter().flatten().collect();
        let paths: Vec<String> = paths.iter().map(|p| relativize(p, &roots)).filter(|p| !p.is_empty()).collect();
        let all = load(&work_root);
        let Some(repo) = repo else { return Ok(learn_report(&all, &paths)) };
        let others = self.busy_tasks(project, &repo, instance);
        if !paths.is_empty() {
            let report = learn_report(&all, &paths);
            let warnings = crate::situation::clash_warnings(&others, &paths);
            return Ok(if warnings.is_empty() { report } else { format!("WARNING:\n{warnings}\n\n{report}") });
        }
        let base = self.task_of_instance(project, instance).map(|t| t.base).or_else(|| current_branch(&repo));
        let base = base.unwrap_or_else(|| "HEAD".into());
        let commits = crate::situation::base_commits(&repo, &base);
        let render = |commits: &[String]| crate::situation::render(&others, commits, &base, &[], crate::situation::BUDGET);
        Ok(learn_with_situation(&all, &render(&commits), &render(&[])))
    }
}

/// `--learn` without files: the learnings and the current situation under a single cap (`LEARN_BUDGET`). Does
/// everything fit? If not, it cuts the commits of the situation first (`lean_situation` is it without them), then
/// the learnings index and finally the global ones in full.
fn learn_with_situation(all: &[Learning], full_situation: &str, lean_situation: &str) -> String {
    let join = |report: String, situation: &str| format!("{report}\n\nCurrent situation:\n{situation}");
    let size = |text: &str| text.chars().count();
    for situation in [full_situation, lean_situation] {
        let text = join(learn_report(all, &[]), situation);
        if size(&text) <= LEARN_BUDGET {
            return text;
        }
    }
    // The index takes what is left after the global ones and the lean situation (its header also counts).
    let without_index = join(learn_report_with(all, &[], LEARN_BUDGET, 0), lean_situation);
    // The final "… and N more" line goes a little over the index cap: leave a margin for it.
    let spare = LEARN_BUDGET.saturating_sub(size(&without_index) + size(INDEX_HEADER) + 100);
    if spare > 0 {
        let text = join(learn_report_with(all, &[], LEARN_BUDGET, spare), lean_situation);
        if size(&text) <= LEARN_BUDGET {
            return text;
        }
    }
    if size(&without_index) <= LEARN_BUDGET {
        return without_index;
    }
    let overhead = size(&join("Project learnings:\n\n".to_owned(), lean_situation));
    join(learn_report_with(all, &[], LEARN_BUDGET.saturating_sub(overhead), 0), lean_situation)
}

/// The branch the main repository is on (empty in a detached HEAD).
fn current_branch(repo: &crate::git::Repo) -> Option<String> {
    let out = repo.read(&["branch", "--show-current"]).ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_owned()).filter(|b| !b.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "---\ntitulo: Acentos\nresumo: Use --file.\ntouches: [crates/cli/**, \"crates/core/src/bridge.rs\"]\n---\nThe fact.\n\n**Why:** x.\n";

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
        let l = parse("memory/learnings/a.md", GOOD).unwrap();
        assert_eq!((l.titulo.as_str(), l.resumo.as_str()), ("Acentos", "Use --file."));
        assert_eq!(l.touches, ["crates/cli/**", "crates/core/src/bridge.rs"]);
        assert_eq!(l.body, "The fact.\n\n**Why:** x.");
        assert!(!l.is_global());
    }

    #[test]
    fn the_header_takes_english_or_portuguese_keys() {
        for text in ["---\ntitle: A\nsummary: B\n---\nbody", "---\ntitulo: A\nresumo: B\n---\nbody", "---\ntitle: A\nresumo: B\n---\nbody"]
        {
            let l = parse("a.md", text).unwrap_or_else(|e| panic!("{text:?}: {e}"));
            assert_eq!((l.titulo.as_str(), l.resumo.as_str()), ("A", "B"), "{text:?}");
        }
        assert!(parse("a.md", "---\ntitle: A\n---\nbody").unwrap_err().contains("summary"));
    }

    #[test]
    fn touches_can_be_missing_empty_or_a_dash_list() {
        let global = "---\ntitulo: A\nresumo: B\n---\nbody";
        assert!(parse("a.md", global).unwrap().is_global());
        assert!(parse("a.md", "---\ntitulo: A\nresumo: B\ntouches: []\n---\nbody").unwrap().is_global());
        let list = "---\ntitulo: A\nresumo: B\ntouches:\n  - src/a.rs\n  - \"x/**\"\n---\nbody";
        assert_eq!(parse("a.md", list).unwrap().touches, ["src/a.rs", "x/**"]);
        // CRLF and BOM (Windows writes them that way).
        let crlf = "\u{feff}---\r\ntitulo: A\r\nresumo: B\r\ntouches: [a.rs]\r\n---\r\nbody\r\n";
        let l = parse("a.md", crlf).unwrap();
        assert_eq!((l.touches, l.body.as_str()), (strings(&["a.rs"]), "body"));
    }

    #[test]
    fn a_broken_frontmatter_is_rejected_with_a_reason() {
        for (text, why) in [
            ("no frontmatter", "frontmatter"),
            ("---\ntitulo: A\nresumo: B\nbody without closing", "closed"),
            ("---\nresumo: B\n---\nbody", "required"),
            ("---\ntitulo: A\n---\nbody", "required"),
            ("---\ntitulo: A\nresumo: B\n---\n   \n", "body"),
            ("---\ntitulo: A\nresumo: B\ntouches: [../outside.rs]\n---\nbody", "touches"),
            ("---\ntitulo: A\nresumo: B\ntouches: [/abs/a.rs]\n---\nbody", "touches"),
            ("---\ntitulo: A\nresumo: B\ntouches: [C:/a.rs]\n---\nbody", "touches"),
        ] {
            let err = parse("a.md", text).unwrap_err();
            assert!(err.contains(why), "{text:?} → {err}");
        }
    }

    #[test]
    fn globs_follow_star_and_double_star() {
        // `*` does not cross a folder; `**` does (and matches zero folders).
        assert!(glob_match("crates/core/src/*.rs", "crates/core/src/tasks.rs"));
        assert!(!glob_match("crates/core/src/*.rs", "crates/core/src/usage/mod.rs"));
        assert!(glob_match("crates/cli/**", "crates/cli/src/bin/ask-agent.rs"));
        assert!(glob_match("crates/**/ask-agent.rs", "crates/cli/src/bin/ask-agent.rs"));
        assert!(glob_match("crates/**/tasks.rs", "crates/tasks.rs"));
        assert!(!glob_match("crates/cli/**", "crates/core/src/x.rs"));
        assert!(glob_match("crates/core/src/ta?ks.rs", "crates/core/src/tasks.rs"));
        assert!(glob_match("memory/*.md", "memory/OVERVIEW.md"));
        assert!(!glob_match("memory/*.md", "memory/learnings/a.md"));
        // Without a slash: the file name in any folder. Backslashes and `./` are normalized.
        assert!(glob_match("*.rs", "crates/core/src/tasks.rs"));
        assert!(glob_match("Cargo.toml", "crates/cli/Cargo.toml"));
        assert!(!glob_match("*.rs", "crates/core/README.md"));
        assert!(glob_match("crates/cli/**", ".\\crates\\cli\\src\\a.rs"));
        // No case difference (Windows does not tell them apart).
        assert!(glob_match("Crates/CLI/**", "crates/cli/a.rs"));
        // An empty pattern or path never matches.
        assert!(!glob_match("", "a.rs") && !glob_match("a.rs", ""));
    }

    #[test]
    fn matching_puts_the_specific_before_the_global() {
        let all =
            vec![learning("a-global", &[], "g"), learning("b-cli", &["crates/cli/**"], "c"), learning("c-ui", &["crates/app/**"], "u")];
        let got: Vec<&str> = matching(&all, &strings(&["crates/cli/src/x.rs"])).iter().map(|l| l.file.as_str()).collect();
        assert_eq!(got, [format!("{DIR}/b-cli.md"), format!("{DIR}/a-global.md")]);
        // Without cited files, only the global ones.
        assert_eq!(matching(&all, &[]).len(), 1);
    }

    #[test]
    fn paths_are_taken_from_the_text_only_when_they_exist() {
        let dir = tempfile::tempdir().unwrap();
        for f in ["crates/core/src/tasks.rs", "memory/HOLT.md", "Cargo.toml"] {
            let p = dir.path().join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "x").unwrap();
        }
        let text = "Read `memory/HOLT.md` and crates/core/src/tasks.rs (see `start_message`); \
                    crates\\core\\src\\tasks.rs again, Cargo.toml, ../outside.rs, crates/core/src/does_not_exist.rs and crates/core (folder).";
        assert_eq!(paths_in_text(text, dir.path()), ["memory/HOLT.md", "crates/core/src/tasks.rs", "Cargo.toml"]);
        assert!(paths_in_text("nothing here, only words. and a period.", dir.path()).is_empty());
        // A folder is not a file; `..` and absolute paths do not escape the repository.
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
        // Only the first one fits in full; the second becomes an index line, together with the ones that do not match.
        assert!(out.contains("### T a-cli") && !out.contains("### T b-cli") && !out.contains("### T c-ui"), "{out}");
        assert!(out.contains(&format!("- {DIR}/b-cli.md — R b-cli")), "{out}");
        assert!(out.contains(&format!("- {DIR}/c-ui.md — R c-ui")), "{out}");
        // With room, everything that matches comes in full and the rest stays in the index.
        let out = render(&all, &strings(&["crates/cli/a.rs"]), 10_000);
        assert!(out.contains("### T a-cli") && out.contains("### T b-cli") && !out.contains("### T c-ui"), "{out}");
        assert!(out.contains("c-ui.md — R c-ui") && !out.contains("b-cli.md — R"), "{out}");
        // Empty folder: nothing to deliver.
        assert_eq!(render(&[], &[], 100), "");
    }

    #[test]
    fn only_the_specific_learnings_that_match_are_rendered() {
        let big = "x".repeat(300);
        let all = vec![
            learning("a-global", &[], "g"),
            learning("b-cli", &["crates/cli/**"], "c"),
            learning("c-ui", &["crates/app/**"], "u"),
            learning("d-cli", &["crates/cli/**"], &big),
        ];
        let out = render_specific(&all, &strings(&["crates/cli/a.rs"]), 10_000);
        // No global ones, none that does not match and no index line.
        assert!(out.contains("### T b-cli") && out.contains("### T d-cli"), "{out}");
        assert!(!out.contains("T a-global") && !out.contains("T c-ui") && !out.contains("- memory/"), "{out}");
        // What goes over the budget stays out (and does not become an index line).
        let out = render_specific(&all, &strings(&["crates/cli/a.rs"]), 100);
        assert!(out.contains("### T b-cli") && !out.contains("T d-cli"), "{out}");
        // Without paths or without anything matching: empty.
        assert_eq!(render_specific(&all, &[], 10_000), "");
        assert_eq!(render_specific(&all, &strings(&["docs/x.md"]), 10_000), "");
    }

    #[test]
    fn a_long_index_ends_with_a_count() {
        let all: Vec<Learning> = (0..80).map(|n| learning(&format!("n{n:03}"), &["x/**"], &"body ".repeat(40))).collect();
        let out = render(&all, &[], 0);
        assert!(out.contains("- … and ") && out.chars().count() < INDEX_BUDGET + 400, "{}", out.chars().count());
    }

    #[test]
    fn the_learn_report_says_when_nothing_matches() {
        let all = vec![learning("a", &["crates/cli/**"], "c"), learning("g", &[], "global")];
        let hit = learn_report(&all, &strings(&["crates/cli/x.rs"]));
        assert!(hit.starts_with("Learnings for: crates/cli/x.rs") && hit.contains("### T a") && hit.contains("### T g"), "{hit}");
        let miss = learn_report(&all, &strings(&["docs/x.md"]));
        assert!(miss.contains("No specific learning") && !miss.contains("### T a") && miss.contains("### T g"), "{miss}");
        let empty = learn_report(&[], &strings(&["a.rs"]));
        assert!(empty.contains("has no learnings yet"));
        // With no learnings (new project), the report teaches the format: there is no `memory/learnings/README.md` there.
        assert!(empty.contains("memory/learnings/<short-name>.md") && empty.contains("title:") && empty.contains("summary:"), "{empty}");
    }

    #[test]
    fn the_learn_report_without_paths_gives_the_globals_and_the_index() {
        let all = vec![learning("a", &["crates/cli/**"], "c"), learning("g", &[], "global")];
        let out = learn_report(&all, &[]);
        assert!(out.starts_with("Project learnings:") && !out.contains("No specific learning"), "{out}");
        assert!(out.contains("### T g") && out.contains("global"), "{out}");
        assert!(!out.contains("### T a") && out.contains(&format!("- {DIR}/a.md — R a")), "{out}");
        assert!(learn_report(&[], &[]).contains("has no learnings yet"));
    }

    #[test]
    fn the_learn_with_situation_cuts_the_commits_then_the_index_then_the_globals() {
        let size = |t: &str| t.chars().count();
        let specific: Vec<Learning> = (0..30).map(|n| learning(&format!("e{n}"), &["crates/**"], "body")).collect();
        let (full, lean) = ("full situation\ncommits".to_owned(), "lean situation".to_owned());
        // Everything fits: nothing is cut.
        let mut all = vec![learning("g", &[], "global")];
        all.extend(specific.clone());
        let text = learn_with_situation(&all, &full, &lean);
        assert!(text.contains("full situation") && text.contains("### T g") && !text.contains("- … and "), "{text}");
        // The global ones take almost everything: the situation loses the commits, the index shrinks, the global ones stay.
        let big = "x".repeat(LEARN_BUDGET - 700);
        let mut all = vec![learning("g", &[], &big)];
        all.extend(specific.clone());
        let text = learn_with_situation(&all, &"c".repeat(1_500), &lean);
        assert!(size(&text) <= LEARN_BUDGET && text.contains(&big) && text.contains("lean situation"), "{}", size(&text));
        assert!(
            text.contains("- … and "),
            "the index shrank: {} {}",
            size(&text),
            text.split("Other learnings").nth(1).unwrap_or("no index")
        );
        // Not even the global ones fit with the situation: the index goes, then the global ones; the lean situation stays.
        let huge = "x".repeat(LEARN_BUDGET - 60);
        let all = vec![learning("g", &[], &huge), learning("e", &["crates/**"], "body")];
        let text = learn_with_situation(&all, &full, &lean);
        assert!(size(&text) <= LEARN_BUDGET && text.ends_with("lean situation") && !text.contains("Other learnings"), "{}", size(&text));
    }

    #[test]
    fn the_folder_is_read_and_a_bad_file_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        // Missing folder: no learning, no error.
        assert!(load(dir.path()).is_empty());
        let learn = dir.path().join(DIR);
        std::fs::create_dir_all(&learn).unwrap();
        std::fs::write(learn.join("b.md"), GOOD).unwrap();
        std::fs::write(learn.join("a.md"), "---\ntitulo: A\nresumo: B\n---\nbody").unwrap();
        std::fs::write(learn.join("broken.md"), "no frontmatter").unwrap();
        std::fs::write(learn.join("README.md"), "---\ntitulo: R\nresumo: R\n---\ndoes not count").unwrap();
        std::fs::write(learn.join("note.txt"), GOOD).unwrap();
        let got = load(dir.path());
        assert_eq!(got.iter().map(|l| l.file.as_str()).collect::<Vec<_>>(), [format!("{DIR}/a.md"), format!("{DIR}/b.md")]);
    }

    #[test]
    fn every_learning_in_the_repository_is_valid() {
        // The real learnings of the project: a broken frontmatter would be silently ignored in the app.
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let files = std::fs::read_dir(root.join(DIR))
            .expect("memory/learnings exists")
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|x| x == "md") && e.file_name() != "README.md")
            .count();
        let loaded = load(&root);
        assert!(files >= 4, "the project must have seeded learnings");
        assert_eq!(loaded.len(), files, "some learning in the folder did not pass validation");
        // Every literal `touches` (without a wildcard) points to a file that exists.
        for l in &loaded {
            for g in l.touches.iter().filter(|g| !g.contains(['*', '?'])) {
                assert!(root.join(g).exists(), "{}: touches points to {g}, which does not exist", l.file);
            }
        }
    }

    #[test]
    fn absolute_paths_become_repository_paths() {
        let roots = [std::path::PathBuf::from("C:/repo/.holt/worktrees/t13"), std::path::PathBuf::from("C:/repo")];
        assert_eq!(relativize(r"C:\repo\.holt\worktrees\t13\crates\cli\a.rs", &roots), "crates/cli/a.rs");
        assert_eq!(relativize("c:/REPO/crates/core/x.rs", &roots), "crates/core/x.rs");
        assert_eq!(relativize(r"./crates\app\y.rs", &roots), "crates/app/y.rs");
        // Outside the roots, and a partial folder-name prefix: only normalized.
        assert_eq!(relativize("C:/other/z.rs", &roots), "C:/other/z.rs");
        assert_eq!(relativize("C:/repository/z.rs", &roots), "C:/repository/z.rs");
    }
}
