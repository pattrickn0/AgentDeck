//! Integração com o git do workspace (aba "Git" da interface).
//!
//! Tudo passa pelo executável `git` instalado, sempre com `Command::new(git).args([...])` (nunca
//! por um shell) e um ambiente que não trava: sem prompt de senha no terminal (`GIT_TERMINAL_PROMPT=0`,
//! ssh em `BatchMode`, Git Credential Manager sem janela: `GCM_INTERACTIVE=never`), sem pager, mensagens em inglês (`LC_ALL=C`, para reconhecer os erros) e
//! caminhos literais (`GIT_LITERAL_PATHSPECS=1`). Listas de caminhos e a mensagem de commit vão pela
//! entrada padrão (sem limite de linha de comando no Windows, sem interpretação de nada).
//!
//! A saída é lida pelas funções puras deste módulo (`parse_*`), testadas sem git; as operações
//! (`impl Core`) são bloqueantes: a interface as chama numa thread.

use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::process::{run_captured_raw, CapturedRaw, Exit};
use crate::Core;

/// Leituras rápidas (status, diff, branches).
const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// Operações locais que escrevem (stage, checkout…).
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// Commit (hooks e assinatura GPG podem demorar).
const COMMIT_TIMEOUT: Duration = Duration::from_secs(120);
/// Rede (fetch, pull, push).
const NET_TIMEOUT: Duration = Duration::from_secs(60);
/// Saída máxima guardada de um comando (o diff de um arquivo enorme é cortado).
const MAX_OUT: usize = 4 << 20;
/// Diff mostrado: no máximo isto de bytes e de linhas.
pub const DIFF_MAX_BYTES: usize = 1 << 20;
pub const DIFF_MAX_LINES: usize = 10_000;
/// Entradas de status mostradas.
pub const STATUS_MAX_ENTRIES: usize = 5000;
/// Tamanho máximo de um arquivo lido para resolução de conflitos.
pub const CONFLICT_MAX_BYTES: usize = 2 << 20;

// ---------- Tipos ----------

/// O que o git devolveu sobre o repositório.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitStatus {
    /// Raiz do repositório.
    pub root: PathBuf,
    /// O workspace relativo à raiz (`""` = o workspace é a raiz; `"workspace/"` = subpasta).
    pub prefix: String,
    /// Branch atual (`None` = HEAD destacado).
    pub branch: Option<String>,
    /// Commit atual (`None` = repositório sem commits).
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub state: RepoState,
    /// Caminhos relativos à raiz.
    pub entries: Vec<GitEntry>,
    /// Quantidade de stashes salvos no repositório.
    pub stash_count: u32,
    /// Havia mais de [`STATUS_MAX_ENTRIES`] alterações.
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RepoState {
    #[default]
    Normal,
    Merging,
    Rebasing,
    CherryPicking,
    Reverting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Ordinary,
    Renamed,
    Copied,
    /// Conflito de mesclagem.
    Unmerged,
    Untracked,
}

/// Um arquivo alterado. `index`/`worktree` são as letras do git (`M`, `A`, `D`, `R`, `C`, `U`,
/// `?`, `.` = sem alteração naquele lado).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitEntry {
    pub path: String,
    /// Nome anterior (renomeado/copiado).
    pub orig_path: Option<String>,
    pub index: char,
    pub worktree: char,
    pub kind: EntryKind,
}

impl GitEntry {
    /// Tem alteração preparada (staged).
    pub fn staged(&self) -> bool {
        !matches!(self.kind, EntryKind::Untracked | EntryKind::Unmerged) && self.index != '.'
    }

    /// Tem alteração não preparada na cópia de trabalho.
    pub fn unstaged(&self) -> bool {
        !matches!(self.kind, EntryKind::Untracked | EntryKind::Unmerged) && self.worktree != '.'
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitBranch {
    /// `main`, `origin/feat`.
    pub name: String,
    pub remote: bool,
    pub upstream: Option<String>,
    pub current: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    Add,
    Del,
    Context,
    /// `@@ -1,3 +1,4 @@`
    Hunk,
    /// Cabeçalhos (`diff --git`, `index`, `---`, `+++`, `\ No newline…`).
    Meta,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffKind,
    /// Número da linha no arquivo antigo / novo.
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitDiff {
    pub lines: Vec<DiffLine>,
    /// Cortado em [`DIFF_MAX_LINES`]/[`DIFF_MAX_BYTES`].
    pub truncated: bool,
    pub binary: bool,
}

/// Uma entrada da pilha de stash (`stash@{index}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitStash {
    pub index: usize,
    pub date: String,
    pub message: String,
}

/// Um commit do histórico (log).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCommit {
    pub hash: String,
    pub short_hash: String,
    pub author: String,
    pub date: String,
    pub subject: String,
}

/// Detalhe de um commit (metadados, arquivos alterados e diff).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitCommitDetail {
    pub hash: String,
    pub short_hash: String,
    pub author: String,
    pub email: String,
    pub date: String,
    pub message: String,
    /// `(letra, caminho)` dos arquivos alterados no commit.
    pub files: Vec<(char, String)>,
    pub diff: GitDiff,
}

/// Um bloco de conflito (`<<<<<<<` … `=======` … `>>>>>>>`) dentro de um arquivo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictBlock {
    /// Linha inicial do marcador `<<<<<<<` (1-indexed).
    pub start_line: u32,
    pub ours_label: String,
    pub ours: Vec<String>,
    pub theirs_label: String,
    pub theirs: Vec<String>,
}

/// Arquivo em conflito com os seus blocos (ou lados inteiros quando não há marcadores de texto).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitConflictFile {
    pub path: String,
    pub blocks: Vec<ConflictBlock>,
    pub binary: bool,
}

/// Escolha para resolver um bloco ou arquivo em conflito.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictChoice {
    Ours,
    Theirs,
    Both,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitError {
    NotInstalled,
    NotARepo,
    /// O git recusa a pasta por ser de outro usuário (`safe.directory`).
    Unsafe {
        path: String,
    },
    /// Falta `user.name`/`user.email` para o commit.
    NoIdentity,
    Timeout,
    /// O remoto recusou (precisa de pull antes, por exemplo).
    Rejected(String),
    /// Credencial ausente ou recusada.
    Auth(String),
    Failed(String),
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitError::NotInstalled => write!(f, "O git não foi encontrado. Instale em https://git-scm.com e tente de novo."),
            GitError::NotARepo => write!(f, "A pasta de trabalho não é um repositório git."),
            GitError::Unsafe { path } => write!(
                f,
                "O git não confia nesta pasta (ela pertence a outro usuário). Para liberar, rode: git config --global --add safe.directory \"{path}\""
            ),
            GitError::NoIdentity => write!(
                f,
                "Defina seu nome e e-mail para o commit: git config --global user.name \"Seu Nome\" e git config --global user.email voce@exemplo.com"
            ),
            GitError::Timeout => write!(f, "O git demorou demais e foi interrompido."),
            GitError::Rejected(m) => write!(f, "O remoto recusou: {m}"),
            GitError::Auth(m) => write!(f, "Falha de autenticação: {m}"),
            GitError::Failed(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for GitError {}

// ---------- Interpretação da saída (funções puras) ----------

/// Lê `git status --porcelain=v2 -z --branch --show-stash`. `root`/`prefix`/`state` ficam por conta de quem chama.
pub fn parse_status_v2(out: &[u8]) -> GitStatus {
    let mut st = GitStatus::default();
    let mut records = out.split(|b| *b == 0).filter(|r| !r.is_empty());
    while let Some(rec) = records.next() {
        let line = String::from_utf8_lossy(rec);
        if let Some(h) = line.strip_prefix("# ") {
            let (key, val) = h.split_once(' ').unwrap_or((h, ""));
            match key {
                "branch.oid" => st.head = (val != "(initial)").then(|| val.to_owned()),
                "branch.head" => st.branch = (val != "(detached)").then(|| val.to_owned()),
                "branch.upstream" => st.upstream = Some(val.to_owned()),
                "branch.ab" => {
                    for part in val.split(' ') {
                        if let Some(n) = part.strip_prefix('+') {
                            st.ahead = n.parse().unwrap_or(0);
                        } else if let Some(n) = part.strip_prefix('-') {
                            st.behind = n.parse().unwrap_or(0);
                        }
                    }
                }
                "stash" => st.stash_count = val.parse().unwrap_or(0),
                _ => {}
            }
            continue;
        }
        let entry = match line.as_bytes().first() {
            Some(b'1') => {
                let f: Vec<&str> = line.splitn(9, ' ').collect();
                (f.len() == 9).then(|| xy_entry(f[1], f[8], None, EntryKind::Ordinary))
            }
            Some(b'2') => {
                let f: Vec<&str> = line.splitn(10, ' ').collect();
                // O nome antigo é o registro seguinte.
                let orig = records.next().map(|r| String::from_utf8_lossy(r).into_owned());
                let kind = if f.get(8).is_some_and(|s| s.starts_with('C')) { EntryKind::Copied } else { EntryKind::Renamed };
                (f.len() == 10).then(|| xy_entry(f[1], f[9], orig, kind))
            }
            Some(b'u') => {
                let f: Vec<&str> = line.splitn(11, ' ').collect();
                (f.len() == 11).then(|| xy_entry(f[1], f[10], None, EntryKind::Unmerged))
            }
            Some(b'?') => line.get(2..).map(|p| GitEntry {
                path: p.to_owned(),
                orig_path: None,
                index: '?',
                worktree: '?',
                kind: EntryKind::Untracked,
            }),
            _ => None,
        };
        if let Some(e) = entry {
            if st.entries.len() >= STATUS_MAX_ENTRIES {
                st.truncated = true;
            } else {
                st.entries.push(e);
            }
        }
    }
    st
}

fn xy_entry(xy: &str, path: &str, orig_path: Option<String>, kind: EntryKind) -> GitEntry {
    let mut c = xy.chars();
    let index = c.next().unwrap_or('.');
    let worktree = c.next().unwrap_or('.');
    GitEntry { path: path.to_owned(), orig_path, index, worktree, kind }
}

/// Formato pedido a `git for-each-ref` (campos separados por NUL, um ref por linha).
pub const BRANCH_FORMAT: &str = "%(refname)%00%(refname:short)%00%(upstream:short)%00%(HEAD)";

/// Lê a saída de `git for-each-ref --format=BRANCH_FORMAT refs/heads refs/remotes`.
pub fn parse_branches(out: &[u8]) -> Vec<GitBranch> {
    let text = String::from_utf8_lossy(out);
    let mut list = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split('\0').collect();
        if f.len() < 4 {
            continue;
        }
        let (full, short, upstream, head) = (f[0], f[1], f[2], f[3]);
        let remote = full.starts_with("refs/remotes/");
        // `origin/HEAD` aponta para outra branch: não é uma escolha útil.
        if remote && full.ends_with("/HEAD") {
            continue;
        }
        list.push(GitBranch {
            name: short.to_owned(),
            remote,
            upstream: (!upstream.is_empty()).then(|| upstream.to_owned()),
            current: head == "*",
        });
    }
    list
}

/// Lê um diff unificado (ou combinado, `diff --cc` / `@@@`). Corta em `max_lines` linhas (e cada
/// linha em 1000 caracteres).
pub fn parse_diff(text: &str, max_lines: usize) -> GitDiff {
    let mut diff = GitDiff::default();
    let (mut old, mut new) = (0u32, 0u32);
    let mut in_hunk = false;
    // Número de prefixos de coluna (1 para diff normal `@@`, >= 2 para diff combinado `@@@`).
    let mut cols = 1usize;
    for raw in text.lines() {
        if diff.lines.len() >= max_lines {
            diff.truncated = true;
            break;
        }
        let line: String = if raw.chars().count() > 1000 { raw.chars().take(1000).chain(['…']).collect() } else { raw.to_owned() };
        let (kind, o, n) = if raw.starts_with("diff --git ") || raw.starts_with("diff --cc ") || raw.starts_with("diff --combined ") {
            in_hunk = false;
            (DiffKind::Meta, None, None)
        } else if raw.starts_with("Binary files ") && raw.ends_with(" differ") {
            diff.binary = true;
            (DiffKind::Meta, None, None)
        } else if raw.starts_with("@@") {
            let ats = raw.bytes().take_while(|b| *b == b'@').count();
            let rest = &raw[ats..];
            in_hunk = true;
            cols = ats.saturating_sub(1).max(1);
            // `@@ -a[,b] +c[,d] @@` ou `@@@ -a[,b] -c[,d] +e[,f] @@@`
            let start = |p: &str, sign: char| -> Option<u32> {
                p.strip_prefix(sign).and_then(|s| s.split(',').next()).and_then(|s| s.parse().ok())
            };
            let mut old_found = None;
            let mut new_found = None;
            for part in rest.split_whitespace() {
                if part.starts_with('@') {
                    break;
                }
                if old_found.is_none() {
                    old_found = start(part, '-');
                }
                if let Some(v) = start(part, '+') {
                    new_found = Some(v);
                }
            }
            old = old_found.unwrap_or(0);
            new = new_found.unwrap_or(0);
            (DiffKind::Hunk, None, None)
        } else if !in_hunk || raw.starts_with('\\') {
            (DiffKind::Meta, None, None)
        } else if cols == 1 {
            if raw.starts_with('+') {
                new += 1;
                (DiffKind::Add, None, Some(new - 1))
            } else if raw.starts_with('-') {
                old += 1;
                (DiffKind::Del, Some(old - 1), None)
            } else {
                old += 1;
                new += 1;
                (DiffKind::Context, Some(old - 1), Some(new - 1))
            }
        } else {
            // Diff combinado (`diff --cc`): os primeiros `cols` caracteres indicam `+`, `-` ou ` ` em cada pai.
            let prefix: String = raw.chars().take(cols).collect();
            if prefix.contains('+') {
                new += 1;
                (DiffKind::Add, None, Some(new - 1))
            } else if prefix.contains('-') {
                let o = if prefix.starts_with('-') {
                    old += 1;
                    Some(old - 1)
                } else {
                    None
                };
                (DiffKind::Del, o, None)
            } else {
                old += 1;
                new += 1;
                (DiffKind::Context, Some(old - 1), Some(new - 1))
            }
        };
        diff.lines.push(DiffLine { kind, old: o, new: n, text: line });
    }
    diff
}

/// Constrói um patch unificado contendo apenas o trecho (`hunk_index`, 0-indexed) de `diff`,
/// pronto para `git apply`. Devolve `None` se o diff for combinado (`@@@`), truncado, binário ou
/// não tiver o cabeçalho `---`/`+++`.
pub fn extract_hunk_patch(diff: &GitDiff, hunk_index: usize) -> Option<String> {
    if diff.binary || diff.truncated {
        return None;
    }
    let mut header = Vec::new();
    let mut hunks: Vec<Vec<&DiffLine>> = Vec::new();
    for line in &diff.lines {
        match line.kind {
            DiffKind::Meta if hunks.is_empty() => header.push(line.text.as_str()),
            DiffKind::Hunk => {
                // Diff combinado (`@@@`) não pode ser aplicado com `git apply`.
                if line.text.starts_with("@@@") {
                    return None;
                }
                hunks.push(vec![line]);
            }
            _ => {
                if let Some(cur) = hunks.last_mut() {
                    cur.push(line);
                }
            }
        }
    }
    let hunk = hunks.get(hunk_index)?;
    if !header.iter().any(|l| l.starts_with("--- ")) || !header.iter().any(|l| l.starts_with("+++ ")) {
        return None;
    }
    // Linha truncada (> 1000 chars) não produziria um patch fiel.
    if hunk.iter().any(|l| l.text.ends_with('…') && l.text.chars().count() > 1000) {
        return None;
    }
    let mut out = String::new();
    for h in header {
        out.push_str(h);
        out.push('\n');
    }
    for l in hunk {
        out.push_str(&l.text);
        out.push('\n');
    }
    Some(out)
}

/// Formato pedido a `git stash list`.
pub const STASH_FORMAT: &str = "%gd%x00%cr%x00%gs";

/// Lê a saída de `git stash list --format=STASH_FORMAT`.
pub fn parse_stashes(out: &[u8]) -> Vec<GitStash> {
    let text = String::from_utf8_lossy(out);
    let mut list = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.splitn(3, '\0').collect();
        if f.len() < 3 {
            continue;
        }
        let idx = f[0].strip_prefix("stash@{").and_then(|s| s.strip_suffix('}')).and_then(|s| s.parse().ok()).unwrap_or(list.len());
        list.push(GitStash { index: idx, date: f[1].to_owned(), message: f[2].to_owned() });
    }
    list
}

/// Formato pedido a `git log -z`.
pub const LOG_FORMAT: &str = "%H%x00%h%x00%an%x00%cr%x00%s";

/// Lê a saída de `git log -z --format=LOG_FORMAT`.
pub fn parse_log(out: &[u8]) -> Vec<GitCommit> {
    let mut list = Vec::new();
    // Com `-z` e `%x00`, todos os campos ficam separados por NUL em sequência de 5 em 5.
    let fields: Vec<&[u8]> = out.split(|b| *b == 0).collect();
    for chunk in fields.chunks(5) {
        if chunk.len() < 5 {
            break;
        }
        let hash = String::from_utf8_lossy(chunk[0]).trim_start_matches('\n').to_owned();
        if hash.is_empty() {
            continue;
        }
        list.push(GitCommit {
            hash,
            short_hash: String::from_utf8_lossy(chunk[1]).into_owned(),
            author: String::from_utf8_lossy(chunk[2]).into_owned(),
            date: String::from_utf8_lossy(chunk[3]).into_owned(),
            subject: String::from_utf8_lossy(chunk[4]).into_owned(),
        });
    }
    list
}

/// Encontra os blocos de conflito (`<<<<<<<` … `=======` … `>>>>>>>`) num texto.
pub fn parse_conflict_blocks(text: &str) -> Vec<ConflictBlock> {
    let lines: Vec<&str> = text.lines().collect();
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some(ours_label) = lines[i].strip_prefix("<<<<<<<") else {
            i += 1;
            continue;
        };
        let start_line = i as u32 + 1;
        let ours_label = ours_label.trim().to_owned();
        i += 1;
        let mut ours = Vec::new();
        // Aceita também conflitos diff3 (`|||||||`), ignorando a seção base entre `|||||||` e `=======`.
        let mut in_base = false;
        while i < lines.len() && !lines[i].starts_with("=======") {
            if lines[i].starts_with("|||||||") {
                in_base = true;
            } else if !in_base {
                ours.push(lines[i].to_owned());
            }
            i += 1;
        }
        if i >= lines.len() {
            break;
        }
        i += 1; // pula `=======`
        let mut theirs = Vec::new();
        while i < lines.len() && !lines[i].starts_with(">>>>>>>") {
            theirs.push(lines[i].to_owned());
            i += 1;
        }
        if i >= lines.len() {
            break;
        }
        let theirs_label = lines[i].strip_prefix(">>>>>>>").unwrap_or("").trim().to_owned();
        blocks.push(ConflictBlock { start_line, ours_label, ours, theirs_label, theirs });
        i += 1;
    }
    blocks
}

/// Resolve um único bloco de conflito (`block_index`, 0-indexed) substituindo os marcadores pela
/// escolha (`Ours`, `Theirs` ou `Both`), preservando o estilo de quebra de linha (`\r\n` ou `\n`).
pub fn resolve_conflict_block_in_text(text: &str, block_index: usize, choice: ConflictChoice) -> Option<String> {
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let trailing_nl = text.ends_with('\n');
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut cur_block = 0usize;
    let mut resolved = false;
    let mut i = 0;
    while i < lines.len() {
        if lines[i].starts_with("<<<<<<<") {
            let mut j = i + 1;
            let mut ours = Vec::new();
            let mut in_base = false;
            while j < lines.len() && !lines[j].starts_with("=======") {
                if lines[j].starts_with("|||||||") {
                    in_base = true;
                } else if !in_base {
                    ours.push(lines[j]);
                }
                j += 1;
            }
            if j < lines.len() {
                let mut k = j + 1;
                let mut theirs = Vec::new();
                while k < lines.len() && !lines[k].starts_with(">>>>>>>") {
                    theirs.push(lines[k]);
                    k += 1;
                }
                if k < lines.len() {
                    if cur_block == block_index {
                        match choice {
                            ConflictChoice::Ours => out.extend(ours.into_iter().map(str::to_owned)),
                            ConflictChoice::Theirs => out.extend(theirs.into_iter().map(str::to_owned)),
                            ConflictChoice::Both => {
                                out.extend(ours.into_iter().map(str::to_owned));
                                out.extend(theirs.into_iter().map(str::to_owned));
                            }
                        }
                        resolved = true;
                    } else {
                        for line in &lines[i..=k] {
                            out.push((*line).to_owned());
                        }
                    }
                    cur_block += 1;
                    i = k + 1;
                    continue;
                }
            }
        }
        out.push(lines[i].to_owned());
        i += 1;
    }
    if !resolved {
        return None;
    }
    let mut res = out.join(eol);
    if trailing_nl && !res.is_empty() {
        res.push_str(eol);
    }
    Some(res)
}

/// Diff de um arquivo ainda não rastreado: todas as linhas como adicionadas.
pub fn untracked_diff(bytes: &[u8], max_lines: usize) -> GitDiff {
    if bytes.iter().take(8000).any(|b| *b == 0) {
        return GitDiff { lines: Vec::new(), truncated: false, binary: true };
    }
    let text = String::from_utf8_lossy(bytes);
    let mut diff = GitDiff::default();
    for (i, raw) in text.lines().enumerate() {
        if i >= max_lines {
            diff.truncated = true;
            break;
        }
        let text: String = if raw.chars().count() > 1000 { raw.chars().take(1000).chain(['…']).collect() } else { raw.to_owned() };
        diff.lines.push(DiffLine { kind: DiffKind::Add, old: None, new: Some(i as u32 + 1), text });
    }
    diff
}

/// Transforma o stderr de um comando que falhou num erro com sentido (as mensagens vêm em
/// inglês por causa do `LC_ALL=C`).
pub fn classify_error(stderr: &str) -> GitError {
    let s = stderr.trim();
    let low = s.to_lowercase();
    if low.contains("not a git repository") {
        return GitError::NotARepo;
    }
    if low.contains("dubious ownership") {
        let path = s.split("repository at '").nth(1).and_then(|r| r.split('\'').next()).unwrap_or("").to_owned();
        return GitError::Unsafe { path };
    }
    if low.contains("please tell me who you are") || low.contains("empty ident") || low.contains("unable to auto-detect email") {
        return GitError::NoIdentity;
    }
    let short = crate::util::last_meaningful_lines(s, 4);
    if low.contains("permission denied (publickey") || low.contains("sign_and_send_pubkey") || low.contains("no mutual signature algorithm")
    {
        return GitError::Auth(format!(
            "{short}\nSe a sua chave SSH tem senha, o CommandDeck não consegue perguntá-la: carregue a chave no ssh-agent e tente de novo.\n• Windows (PowerShell; na primeira vez como administrador): Set-Service ssh-agent -StartupType Manual; Start-Service ssh-agent; ssh-add\n• Linux/macOS: eval \"$(ssh-agent -s)\" && ssh-add"
        ));
    }
    if low.contains("authentication failed")
        || low.contains("could not read username")
        || low.contains("could not read password")
        || low.contains("terminal prompts disabled")
        || low.contains("user interactivity has been disabled")
        || low.contains("host key verification failed")
    {
        return GitError::Auth(short);
    }
    // `commit.gpgsign` com a chave travada ou sem agente/pinentry que consiga perguntar a senha.
    if low.contains("gpg failed to sign") || low.contains("no pinentry") || low.contains("inappropriate ioctl for device") {
        return GitError::Failed(format!(
            "O commit não foi feito: a assinatura GPG (commit.gpgsign) precisa da chave destravada, e o CommandDeck não consegue abrir o pedido de senha. Destrave a chave numa aba de terminal (echo teste | gpg --clearsign) e tente de novo em seguida; a senha fica no gpg-agent por alguns minutos. Se não quiser assinar neste repositório: git config commit.gpgsign false
{short}"
        ));
    }
    if low.contains("[rejected]")
        || low.contains("non-fast-forward")
        || low.contains("fetch first")
        || low.contains("not possible to fast-forward")
        || low.contains("diverging branches")
    {
        return GitError::Rejected(short);
    }
    GitError::Failed(if short.is_empty() { "o git falhou sem mensagem".into() } else { short })
}

/// Nome de branch aceitável (subconjunto seguro das regras de `git check-ref-format`).
pub fn valid_branch_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && !name.starts_with('-')
        && !name.starts_with('/')
        && !name.ends_with('/')
        && !name.ends_with('.')
        && !name.ends_with(".lock")
        && name != "@"
        && !name.contains("..")
        && !name.contains("@{")
        && !name.contains("//")
        && !name.chars().any(|c| c.is_whitespace() || c.is_control() || "~^:?*[\\".contains(c))
        && !name.split('/').any(|part| part.starts_with('.'))
}

/// Caminho relativo seguro para passar ao git (sem `..`, sem ser absoluto, sem NUL). Só aceita
/// componentes comuns (e `.`); no Windows, nenhum `:` (`C:..\x` é relativo à unidade e sai da raiz).
pub fn safe_rel(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\0')
        && !(cfg!(windows) && path.contains(':'))
        && !path.starts_with(['/', '\\'])
        && !path.split(['/', '\\']).any(|p| p == "..")
        && Path::new(path).components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

// ---------- Execução ----------

/// Estado do git guardado no core.
#[derive(Default)]
pub(crate) struct GitState {
    /// Executável encontrado (e quando foi procurado: refaz a busca depois de 30 s, para perceber
    /// uma instalação feita com o app aberto).
    exe: Mutex<Option<(Instant, Option<PathBuf>)>>,
    /// Uma operação que escreve no repositório por vez (o git usa `index.lock`).
    op: Mutex<()>,
}

/// Repositório encontrado a partir do workspace.
struct Repo {
    git: PathBuf,
    root: PathBuf,
    git_dir: PathBuf,
    prefix: String,
}

fn base_command(git: &Path, cwd: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(git);
    c.current_dir(cwd).args(["-c", "core.quotepath=false", "-c", "color.ui=false"]).args(args);
    for (k, v) in [
        ("LC_ALL", "C"),
        ("LANGUAGE", "C"),
        ("GIT_TERMINAL_PROMPT", "0"),
        // O Git Credential Manager abriria uma janela de login e o comando ficaria parado até o
        // timeout; sem credencial salva, falha logo (vira `GitError::Auth`).
        ("GCM_INTERACTIVE", "never"),
        ("SSH_ASKPASS_REQUIRE", "never"),
        ("GIT_OPTIONAL_LOCKS", "0"),
        ("GIT_MERGE_AUTOEDIT", "no"),
        ("GIT_PAGER", "cat"),
        ("GIT_EDITOR", "true"),
        ("GIT_SEQUENCE_EDITOR", "true"),
        ("GIT_LITERAL_PATHSPECS", "1"),
        // Sem TTY para o pinentry curses; se houver pinentry gráfico, ele usa DISPLAY/sessão do SO;
        // se for terminal, falha imediatamente em vez de esperar os 120 s de timeout.
        ("GPG_TTY", "not a tty"),
        ("PINENTRY_USER_DATA", "USE_CURSES=0"),
    ] {
        c.env(k, v);
    }
    for k in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_PREFIX", "GIT_ASKPASS", "SSH_ASKPASS"] {
        c.env_remove(k);
    }
    c
}

fn run(cmd: Command, stdin: Option<Vec<u8>>, timeout: Duration) -> Result<CapturedRaw, GitError> {
    let out = run_captured_raw(cmd, stdin, timeout, None, None, MAX_OUT).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            GitError::NotInstalled
        } else {
            GitError::Failed(format!("falha ao rodar o git: {e}"))
        }
    })?;
    match out.exit {
        Exit::Code(Some(0)) => Ok(out),
        Exit::TimedOut => Err(GitError::Timeout),
        _ => {
            // Alguns erros (ex.: "nothing to commit") saem pela saída padrão.
            let err = String::from_utf8_lossy(&out.stderr);
            let err = if err.trim().is_empty() { String::from_utf8_lossy(&out.stdout) } else { err };
            Err(classify_error(&err))
        }
    }
}

/// Caminhos separados por NUL (para `--pathspec-from-file=- --pathspec-file-nul`).
fn nul_paths(paths: &[String]) -> Result<Vec<u8>, GitError> {
    let mut buf = Vec::new();
    for p in paths {
        if !safe_rel(p) {
            return Err(GitError::Failed(format!("caminho inválido: {p}")));
        }
        buf.extend_from_slice(p.as_bytes());
        buf.push(0);
    }
    Ok(buf)
}

impl Repo {
    fn cmd(&self, args: &[&str]) -> Command {
        base_command(&self.git, &self.root, args)
    }

    fn read(&self, args: &[&str]) -> Result<CapturedRaw, GitError> {
        run(self.cmd(args), None, READ_TIMEOUT)
    }

    fn write(&self, args: &[&str], stdin: Option<Vec<u8>>) -> Result<CapturedRaw, GitError> {
        run(self.cmd(args), stdin, WRITE_TIMEOUT)
    }

    fn state(&self) -> RepoState {
        let has = |p: &str| self.git_dir.join(p).exists();
        if has("rebase-merge") || has("rebase-apply") {
            RepoState::Rebasing
        } else if has("MERGE_HEAD") {
            RepoState::Merging
        } else if has("CHERRY_PICK_HEAD") {
            RepoState::CherryPicking
        } else if has("REVERT_HEAD") {
            RepoState::Reverting
        } else {
            RepoState::Normal
        }
    }

    fn has_head(&self) -> bool {
        run(self.cmd(&["rev-parse", "--verify", "-q", "HEAD"]), None, READ_TIMEOUT).is_ok()
    }

    /// Verifica se o commit atual usaria assinatura GPG (`commit.gpgsign = true`).
    fn is_gpgsign_enabled(&self) -> bool {
        self.read(&["config", "--bool", "--get", "commit.gpgsign"]).is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "true")
    }

    /// Quando `commit.gpgsign` está ligado, confere em instantes se o GPG consegue assinar SEM
    /// perguntar nada (`--pinentry-mode error`): com a chave destravada no `gpg-agent` passa; com a
    /// chave travada (ou sem agente/pinentry utilizável) o gpg falha na hora, em vez de deixar o
    /// commit esperar o `COMMIT_TIMEOUT` por um pedido de senha que ninguém vê. Devolve a explicação
    /// e NÃO mexe na configuração do usuário. Só vale para assinatura OpenPGP.
    fn check_gpg_ready(&self) -> Result<(), GitError> {
        if !self.is_gpgsign_enabled() {
            return Ok(());
        }
        let cfg = |key: &str| {
            self.read(&["config", "--get", key])
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
                .filter(|s| !s.is_empty())
        };
        if cfg("gpg.format").is_some_and(|f| f != "openpgp") {
            return Ok(());
        }
        let prog = cfg("gpg.openpgp.program").or_else(|| cfg("gpg.program")).unwrap_or_else(|| "gpg".into());
        let mut cmd = Command::new(&prog);
        cmd.current_dir(&self.root).args(["--batch", "--pinentry-mode", "error", "--detach-sign", "-o", "-"]);
        if let Some(k) = cfg("user.signingkey") {
            cmd.args(["-u", &k]);
        }
        cmd.env("LC_ALL", "C").env("GPG_TTY", "not a tty").env("PINENTRY_USER_DATA", "USE_CURSES=0");
        match run_captured_raw(cmd, Some(b"commanddeck".to_vec()), Duration::from_secs(5), None, None, 64 << 10) {
            Ok(out) if matches!(out.exit, Exit::Code(Some(0))) => Ok(()),
            Ok(out) if matches!(out.exit, Exit::TimedOut) => {
                Err(classify_error("error: gpg failed to sign the data (no response from gpg-agent)"))
            }
            Ok(out) => Err(classify_error(&format!(
                "error: gpg failed to sign the data
{}",
                String::from_utf8_lossy(&out.stderr)
            ))),
            Err(_) => Err(classify_error("error: gpg failed to sign the data (gpg executable not found)")),
        }
    }

    /// Ambiente das operações de rede: ssh sem prompt (senão ele abre o terminal e trava), a menos
    /// que o usuário tenha configurado o próprio ssh.
    fn net(&self, args: &[&str]) -> Command {
        let mut c = self.cmd(args);
        let user_ssh = std::env::var_os("GIT_SSH_COMMAND").is_some()
            || std::env::var_os("GIT_SSH").is_some()
            || self.read(&["config", "--get", "core.sshCommand"]).is_ok_and(|o| !o.stdout.is_empty());
        if !user_ssh {
            c.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
        }
        c
    }

    fn remotes(&self) -> Result<Vec<String>, GitError> {
        let out = self.read(&["remote"])?;
        Ok(String::from_utf8_lossy(&out.stdout).lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect())
    }
}

/// Texto curto do que o comando disse (para mostrar depois de pull/push).
fn summary(out: &CapturedRaw) -> String {
    let all = format!("{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    crate::util::last_meaningful_lines(&all, 3)
}

impl Core {
    /// Caminho do executável do git (ou `None`).
    fn git_exe(&self) -> Option<PathBuf> {
        let state = &self.inner.git;
        let mut cache = state.exe.lock();
        if let Some((at, exe)) = cache.as_ref() {
            if at.elapsed() < Duration::from_secs(30) {
                return exe.clone();
            }
        }
        let mut exe = self.inner.agents.which(if cfg!(windows) { "git.exe" } else { "git" });
        if exe.is_none() && cfg!(windows) {
            exe = self.inner.agents.system_dir("ProgramFiles").map(|p| p.join("Git").join("cmd").join("git.exe")).filter(|p| p.is_file());
        }
        *cache = Some((Instant::now(), exe.clone()));
        exe
    }

    /// Acha o repositório do workspace.
    fn git_repo(&self) -> Result<Repo, GitError> {
        let git = self.git_exe().ok_or(GitError::NotInstalled)?;
        let ws = self.workspace();
        if ws.as_os_str().is_empty() {
            return Err(GitError::NotARepo);
        }
        let out =
            run(base_command(&git, &ws, &["rev-parse", "--show-toplevel", "--absolute-git-dir", "--show-prefix"]), None, READ_TIMEOUT)?;
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let mut lines = text.lines();
        let root = lines.next().map(PathBuf::from).ok_or(GitError::NotARepo)?;
        let git_dir = lines.next().map(PathBuf::from).unwrap_or_else(|| root.join(".git"));
        let prefix = lines.next().unwrap_or("").to_owned();
        Ok(Repo { git, root, git_dir, prefix })
    }

    /// Status do repositório do workspace.
    pub fn git_status(&self) -> Result<GitStatus, GitError> {
        let repo = self.git_repo()?;
        let out = repo.read(&["status", "--porcelain=v2", "-z", "--branch", "--show-stash", "--untracked-files=normal"])?;
        let mut st = parse_status_v2(&out.stdout);
        st.state = repo.state();
        st.prefix = repo.prefix.clone();
        st.root = repo.root;
        Ok(st)
    }

    /// Diff de um arquivo (`path` relativo à raiz do repositório). `staged`: o que está preparado
    /// (índice × HEAD); senão, a cópia de trabalho × índice. Arquivo não rastreado: o conteúdo todo.
    pub fn git_diff(&self, entry: &GitEntry, staged: bool) -> Result<GitDiff, GitError> {
        let repo = self.git_repo()?;
        if !safe_rel(&entry.path) {
            return Err(GitError::Failed(format!("caminho inválido: {}", entry.path)));
        }
        if entry.kind == EntryKind::Untracked {
            let full = repo.root.join(&entry.path);
            if full.is_dir() {
                return Ok(GitDiff::default());
            }
            let bytes = read_capped(&full, DIFF_MAX_BYTES).map_err(|e| GitError::Failed(format!("falha ao ler {}: {e}", entry.path)))?;
            let mut d = untracked_diff(&bytes.0, DIFF_MAX_LINES);
            d.truncated |= bytes.1;
            return Ok(d);
        }
        let mut args = vec!["diff", "--no-color", "--no-ext-diff", "-M"];
        if staged {
            args.push("--cached");
        }
        args.push("--");
        args.push(&entry.path);
        if let Some(orig) = entry.orig_path.as_deref().filter(|o| safe_rel(o)) {
            args.push(orig);
        }
        let out = run(repo.cmd(&args), None, READ_TIMEOUT)?;
        let cut = out.stdout.len() > DIFF_MAX_BYTES;
        let bytes = &out.stdout[..out.stdout.len().min(DIFF_MAX_BYTES)];
        let mut d = parse_diff(&String::from_utf8_lossy(bytes), DIFF_MAX_LINES);
        d.truncated |= cut;
        Ok(d)
    }

    /// Prepara (stage) os caminhos (relativos à raiz).
    pub fn git_stage(&self, paths: &[String]) -> Result<(), GitError> {
        let repo = self.git_repo()?;
        let _op = self.inner.git.op.lock();
        repo.write(&["add", "-A", "--pathspec-from-file=-", "--pathspec-file-nul"], Some(nul_paths(paths)?))?;
        Ok(())
    }

    /// Prepara todas as alterações do repositório.
    pub fn git_stage_all(&self) -> Result<(), GitError> {
        let repo = self.git_repo()?;
        let _op = self.inner.git.op.lock();
        repo.write(&["add", "-A"], None)?;
        Ok(())
    }

    /// Tira do stage. Um arquivo renomeado leva junto o nome antigo (senão este continuaria
    /// preparado como apagado).
    pub fn git_unstage(&self, paths: &[String]) -> Result<(), GitError> {
        let repo = self.git_repo()?;
        let _op = self.inner.git.op.lock();
        let stdin = nul_paths(paths)?;
        if repo.has_head() {
            let st = parse_status_v2(&repo.read(&["status", "--porcelain=v2", "-z", "--untracked-files=no"])?.stdout);
            let mut all = paths.to_vec();
            for e in st.entries.iter().filter(|e| paths.contains(&e.path)) {
                if let Some(orig) = e.orig_path.as_ref().filter(|o| !all.contains(o)) {
                    all.push(orig.clone());
                }
            }
            repo.write(&["restore", "--staged", "--pathspec-from-file=-", "--pathspec-file-nul"], Some(nul_paths(&all)?))?;
        } else {
            // Sem nenhum commit não há HEAD para restaurar: só sai do índice.
            repo.write(&["rm", "--cached", "-r", "-q", "--pathspec-from-file=-", "--pathspec-file-nul"], Some(stdin))?;
        }
        Ok(())
    }

    /// Tira tudo do stage.
    pub fn git_unstage_all(&self) -> Result<(), GitError> {
        let repo = self.git_repo()?;
        let _op = self.inner.git.op.lock();
        if repo.has_head() {
            repo.write(&["reset", "-q"], None)?;
        } else {
            // Índice vazio: nada a tirar (sem `--ignore-unmatch` o git falharia com "did not match").
            repo.write(&["rm", "--cached", "-r", "-q", "--ignore-unmatch", "--", "."], None)?;
        }
        Ok(())
    }

    /// Descarta as alterações de um arquivo:
    /// - não rastreado: APAGA o arquivo ou pasta (usa `-ffdq` em pastas para remover também sub-repositórios com `.git` interno);
    /// - em conflito (`Unmerged`): restaura à versão do `HEAD` (ou remove se foi adicionado apenas pelo outro lado, ex.: `AU`/`DU`/`UU`);
    /// - rastreado comum: restaura a cópia de trabalho ao que está no índice.
    pub fn git_discard(&self, entry: &GitEntry) -> Result<(), GitError> {
        let repo = self.git_repo()?;
        if !safe_rel(&entry.path) {
            return Err(GitError::Failed(format!("caminho inválido: {}", entry.path)));
        }
        let _op = self.inner.git.op.lock();
        match entry.kind {
            EntryKind::Untracked => {
                let is_dir = entry.path.ends_with('/') || repo.root.join(&entry.path).is_dir();
                let flags = if is_dir { "-ffdq" } else { "-fq" };
                repo.write(&["clean", flags, "--", &entry.path], None)?;
            }
            EntryKind::Unmerged => {
                // Num arquivo em conflito, `restore --worktree` falha porque há múltiplos estágios no índice.
                // Se o arquivo existe no HEAD, `restore --source=HEAD --staged --worktree` restaura o nosso lado limpo;
                // se não existe no HEAD (ex.: adicionado por eles `UA` ou ambos `AA`), removemos do índice e do disco.
                let in_head = run(repo.cmd(&["cat-file", "-e", &format!("HEAD:{}", entry.path)]), None, READ_TIMEOUT).is_ok();
                if in_head {
                    repo.write(&["restore", "--source=HEAD", "--staged", "--worktree", "--", &entry.path], None)?;
                } else {
                    repo.write(&["rm", "-f", "-q", "--", &entry.path], None)?;
                }
            }
            _ => {
                repo.write(&["restore", "--worktree", "--", &entry.path], None)?;
            }
        }
        Ok(())
    }

    /// Aplica o patch de um trecho (hunk) para preparar (`stage`), tirar da preparação (`unstage`)
    /// ou descartar da cópia de trabalho (`discard`).
    pub fn git_apply_hunk(&self, patch: &str, staged: bool, discard: bool) -> Result<(), GitError> {
        if patch.trim().is_empty() {
            return Err(GitError::Failed("trecho vazio".into()));
        }
        let repo = self.git_repo()?;
        let _op = self.inner.git.op.lock();
        let args: &[&str] = match (discard, staged) {
            // Descartar um trecho não preparado na cópia de trabalho.
            (true, _) => &["apply", "--reverse", "--recount", "-"],
            // Tirar do stage um trecho que estava preparado.
            (false, true) => &["apply", "--cached", "--reverse", "--recount", "-"],
            // Preparar um trecho da cópia de trabalho.
            (false, false) => &["apply", "--cached", "--recount", "-"],
        };
        repo.write(args, Some(patch.as_bytes().to_vec()))?;
        Ok(())
    }

    /// Faz o commit do que está preparado. Devolve `"<hash> <assunto>"`.
    pub fn git_commit(&self, message: &str, amend: bool) -> Result<String, GitError> {
        if message.trim().is_empty() {
            return Err(GitError::Failed("escreva a mensagem do commit".into()));
        }
        let repo = self.git_repo()?;
        let _op = self.inner.git.op.lock();
        repo.check_gpg_ready()?;
        let mut args = vec!["commit", "--cleanup=strip", "-F", "-"];
        if amend {
            args.push("--amend");
        }
        run(repo.cmd(&args), Some(message.as_bytes().to_vec()), COMMIT_TIMEOUT)?;
        let out = repo.read(&["log", "-1", "--format=%h %s"])?;
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    }

    /// Mensagem do último commit (para "corrigir último commit" ou mensagem de merge).
    pub fn git_last_message(&self) -> Option<String> {
        let repo = self.git_repo().ok()?;
        let out = repo.read(&["log", "-1", "--format=%B"]).ok()?;
        Some(String::from_utf8_lossy(&out.stdout).trim_end().to_owned())
    }

    pub fn git_fetch(&self) -> Result<String, GitError> {
        let repo = self.git_repo()?;
        let _op = self.inner.git.op.lock();
        let out = run(repo.net(&["fetch", "--all", "--prune"]), None, NET_TIMEOUT)?;
        Ok(summary(&out))
    }

    /// Pull só por avanço direto (sem criar merge sozinho).
    pub fn git_pull(&self) -> Result<String, GitError> {
        let repo = self.git_repo()?;
        let _op = self.inner.git.op.lock();
        let out = run(repo.net(&["pull", "--ff-only", "--no-edit"]), None, NET_TIMEOUT)?;
        Ok(summary(&out))
    }

    /// Push. Sem upstream, publica a branch no único remoto (`push -u <remoto> <branch>`).
    pub fn git_push(&self) -> Result<String, GitError> {
        let repo = self.git_repo()?;
        let _op = self.inner.git.op.lock();
        let st = parse_status_v2(&repo.read(&["status", "--porcelain=v2", "-z", "--branch", "--untracked-files=no"])?.stdout);
        let out = if st.upstream.is_some() {
            run(repo.net(&["push"]), None, NET_TIMEOUT)?
        } else {
            let branch = st.branch.ok_or_else(|| GitError::Failed("HEAD destacado: troque para uma branch antes de enviar".into()))?;
            let remotes = repo.remotes()?;
            let remote = match remotes.as_slice() {
                [] => return Err(GitError::Failed("nenhum remoto configurado (git remote add origin <url>)".into())),
                [one] => one.clone(),
                many if many.iter().any(|r| r == "origin") => "origin".into(),
                _ => {
                    return Err(GitError::Failed(
                        "vários remotos e nenhum upstream: publique pelo terminal (git push -u <remoto> <branch>)".into(),
                    ))
                }
            };
            run(repo.net(&["push", "-u", &remote, &branch]), None, NET_TIMEOUT)?
        };
        Ok(summary(&out))
    }

    /// Branches locais e remotas.
    pub fn git_branches(&self) -> Result<Vec<GitBranch>, GitError> {
        let repo = self.git_repo()?;
        let fmt = format!("--format={BRANCH_FORMAT}");
        let out = repo.read(&["for-each-ref", &fmt, "refs/heads", "refs/remotes"])?;
        Ok(parse_branches(&out.stdout))
    }

    /// Troca de branch. Uma remota (`origin/x`) vira uma branch local que a acompanha; se a local
    /// (`x`) já existe, só troca para ela.
    pub fn git_checkout(&self, branch: &GitBranch) -> Result<(), GitError> {
        if !valid_branch_name(&branch.name) {
            return Err(GitError::Failed(format!("nome de branch inválido: {}", branch.name)));
        }
        let repo = self.git_repo()?;
        let _op = self.inner.git.op.lock();
        if branch.remote {
            // Nome local: o que vem depois do remoto (o mais longo que casa; remotos podem ter `/`).
            let remotes = repo.remotes()?;
            let local = remotes
                .iter()
                .filter_map(|r| branch.name.strip_prefix(r.as_str())?.strip_prefix('/').map(|l| (r.len(), l)))
                .max_by_key(|(len, _)| *len)
                .map(|(_, l)| l)
                .or_else(|| branch.name.split_once('/').map(|(_, l)| l))
                .filter(|l| valid_branch_name(l));
            let exists = |l: &str| run(repo.cmd(&["rev-parse", "--verify", "-q", &format!("refs/heads/{l}")]), None, READ_TIMEOUT).is_ok();
            match local {
                Some(l) if exists(l) => repo.write(&["switch", l], None)?,
                _ => repo.write(&["switch", "--track", &branch.name], None)?,
            };
        } else {
            repo.write(&["switch", &branch.name], None)?;
        }
        Ok(())
    }

    /// Cria uma branch a partir do HEAD (e troca para ela, se `switch`).
    pub fn git_create_branch(&self, name: &str, switch: bool) -> Result<(), GitError> {
        if !valid_branch_name(name) {
            return Err(GitError::Failed(format!("nome de branch inválido: {name}")));
        }
        let repo = self.git_repo()?;
        let _op = self.inner.git.op.lock();
        if switch {
            repo.write(&["switch", "-c", name], None)?;
        } else {
            repo.write(&["branch", name], None)?;
        }
        Ok(())
    }

    // ---------- Stash ----------

    /// Lista os stashes do repositório.
    pub fn git_stash_list(&self) -> Result<Vec<GitStash>, GitError> {
        let repo = self.git_repo()?;
        let fmt = format!("--format={STASH_FORMAT}");
        let out = repo.read(&["stash", "list", &fmt])?;
        Ok(parse_stashes(&out.stdout))
    }

    /// Guarda as alterações atuais no stash (`include_untracked`: inclui arquivos não rastreados `-u`).
    pub fn git_stash_push(&self, message: &str, include_untracked: bool) -> Result<String, GitError> {
        let repo = self.git_repo()?;
        let _op = self.inner.git.op.lock();
        let mut args = vec!["stash", "push"];
        if include_untracked {
            args.push("--include-untracked");
        }
        let msg = message.trim();
        if !msg.is_empty() {
            args.push("-m");
            args.push(msg);
        }
        // Sem caminhos nossos: o `stash -u` chama o `git clean` por dentro e, com pathspecs literais,
        // ele deixaria os não rastreados no lugar depois de guardá-los.
        let mut cmd = repo.cmd(&args);
        cmd.env_remove("GIT_LITERAL_PATHSPECS");
        let out = run(cmd, None, WRITE_TIMEOUT)?;
        let text = summary(&out);
        if text.to_lowercase().contains("no local changes to save") {
            return Err(GitError::Failed("nenhuma alteração local para guardar no stash".into()));
        }
        Ok(text)
    }

    /// Diff completo de um stash (`stash@{index}`), incluindo arquivos não rastreados guardados nele.
    pub fn git_stash_diff(&self, index: usize) -> Result<GitDiff, GitError> {
        let repo = self.git_repo()?;
        let spec = format!("stash@{{{index}}}");
        let out = repo.read(&["stash", "show", "-p", "--no-color", "--no-ext-diff", &spec])?;
        let mut raw = out.stdout;
        // Quando o stash foi salvo com `--include-untracked`, os arquivos não rastreados ficam no
        // terceiro pai (`stash@{n}^3`).
        let untracked_spec = format!("{spec}^3");
        if let Ok(u) = repo.read(&["show", "--format=", "--patch", "--no-color", "--no-ext-diff", &untracked_spec]) {
            if !u.stdout.is_empty() {
                if !raw.is_empty() && !raw.ends_with(b"\n") {
                    raw.push(b'\n');
                }
                raw.extend_from_slice(&u.stdout);
            }
        }
        let cut = raw.len() > DIFF_MAX_BYTES;
        let bytes = &raw[..raw.len().min(DIFF_MAX_BYTES)];
        let mut d = parse_diff(&String::from_utf8_lossy(bytes), DIFF_MAX_LINES);
        d.truncated |= cut;
        Ok(d)
    }

    /// Aplica um stash sem apagá-lo (`git stash apply stash@{index}`).
    pub fn git_stash_apply(&self, index: usize) -> Result<String, GitError> {
        let repo = self.git_repo()?;
        let _op = self.inner.git.op.lock();
        let spec = format!("stash@{{{index}}}");
        let out = repo.write(&["stash", "apply", "-q", &spec], None)?;
        Ok(summary(&out))
    }

    /// Aplica um stash e o remove da lista (`git stash pop stash@{index}`).
    pub fn git_stash_pop(&self, index: usize) -> Result<String, GitError> {
        let repo = self.git_repo()?;
        let _op = self.inner.git.op.lock();
        let spec = format!("stash@{{{index}}}");
        let out = repo.write(&["stash", "pop", "-q", &spec], None)?;
        Ok(summary(&out))
    }

    /// Apaga um stash (`git stash drop stash@{index}`).
    pub fn git_stash_drop(&self, index: usize) -> Result<(), GitError> {
        let repo = self.git_repo()?;
        let _op = self.inner.git.op.lock();
        let spec = format!("stash@{{{index}}}");
        repo.write(&["stash", "drop", "-q", &spec], None)?;
        Ok(())
    }

    // ---------- Histórico (log) ----------

    /// Lista commits da branch atual (paginada por `skip` e `limit`). Devolve `(commits, há_mais)`.
    pub fn git_log(&self, skip: usize, limit: usize) -> Result<(Vec<GitCommit>, bool), GitError> {
        let repo = self.git_repo()?;
        if !repo.has_head() {
            return Ok((Vec::new(), false));
        }
        let limit = limit.clamp(1, 200);
        let fetch_n = format!("-n{}", limit + 1);
        let skip_arg = format!("--skip={skip}");
        let fmt = format!("--format={LOG_FORMAT}");
        let out = repo.read(&["log", "-z", &fetch_n, &skip_arg, &fmt])?;
        let mut list = parse_log(&out.stdout);
        let has_more = list.len() > limit;
        if has_more {
            list.truncate(limit);
        }
        Ok((list, has_more))
    }

    /// Detalhe de um commit: metadados, arquivos alterados e diff completo.
    pub fn git_commit_detail(&self, hash: &str) -> Result<GitCommitDetail, GitError> {
        if hash.is_empty() || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(GitError::Failed("hash de commit inválido".into()));
        }
        let repo = self.git_repo()?;
        let meta_out = repo.read(&["show", "-s", "--format=%H%x00%h%x00%an%x00%ae%x00%cr%x00%B", hash])?;
        let meta_str = String::from_utf8_lossy(&meta_out.stdout);
        let fields: Vec<&str> = meta_str.splitn(6, '\0').collect();
        if fields.len() < 6 {
            return Err(GitError::Failed("não foi possível ler os dados do commit".into()));
        }
        let files_out = repo.read(&["diff-tree", "--root", "-r", "--no-commit-id", "--name-status", "-z", hash])?;
        let mut files = Vec::new();
        let mut parts = files_out.stdout.split(|b| *b == 0).filter(|p| !p.is_empty());
        while let Some(st) = parts.next() {
            let st_s = String::from_utf8_lossy(st);
            let letter = st_s.chars().next().unwrap_or('M');
            if matches!(letter, 'R' | 'C') {
                let _old = parts.next();
            }
            if let Some(path) = parts.next() {
                files.push((letter, String::from_utf8_lossy(path).into_owned()));
            }
        }
        let diff_out = repo.read(&["show", "--format=", "--patch", "--no-color", "--no-ext-diff", "-M", hash])?;
        let cut = diff_out.stdout.len() > DIFF_MAX_BYTES;
        let bytes = &diff_out.stdout[..diff_out.stdout.len().min(DIFF_MAX_BYTES)];
        let mut diff = parse_diff(&String::from_utf8_lossy(bytes), DIFF_MAX_LINES);
        diff.truncated |= cut;
        Ok(GitCommitDetail {
            hash: fields[0].to_owned(),
            short_hash: fields[1].to_owned(),
            author: fields[2].to_owned(),
            email: fields[3].to_owned(),
            date: fields[4].to_owned(),
            message: fields[5].trim_end().to_owned(),
            files,
            diff,
        })
    }

    // ---------- Merge e Rebase ----------

    /// Mescla a branch `branch` na branch atual (`git merge --no-edit <branch>`).
    /// Quando entra em conflito, devolve `Ok` com aviso (o estado `Merging` e a seção de conflitos
    /// aparecem no status).
    pub fn git_merge(&self, branch: &str) -> Result<String, GitError> {
        if !valid_branch_name(branch) {
            return Err(GitError::Failed(format!("nome de branch inválido: {branch}")));
        }
        let repo = self.git_repo()?;
        let _op = self.inner.git.op.lock();
        repo.check_gpg_ready()?;
        let out = run_captured_raw(repo.cmd(&["merge", "--no-edit", branch]), None, WRITE_TIMEOUT, None, None, MAX_OUT)
            .map_err(|e| GitError::Failed(format!("falha ao rodar o git: {e}")))?;
        match out.exit {
            Exit::Code(Some(0)) => Ok(summary(&out)),
            Exit::TimedOut => Err(GitError::Timeout),
            _ if repo.state() == RepoState::Merging => {
                Ok("Mesclagem com conflitos: resolva os arquivos em conflito e clique em Continuar.".into())
            }
            _ => Err(classify_error(&String::from_utf8_lossy(&out.stderr))),
        }
    }

    /// Rebase da branch atual sobre `branch` (`git rebase <branch>`).
    /// Quando para em conflito, devolve `Ok` com aviso (o estado `Rebasing` aparece no topo).
    pub fn git_rebase(&self, branch: &str) -> Result<String, GitError> {
        if !valid_branch_name(branch) {
            return Err(GitError::Failed(format!("nome de branch inválido: {branch}")));
        }
        let repo = self.git_repo()?;
        let _op = self.inner.git.op.lock();
        repo.check_gpg_ready()?;
        let out = run_captured_raw(repo.cmd(&["rebase", branch]), None, WRITE_TIMEOUT, None, None, MAX_OUT)
            .map_err(|e| GitError::Failed(format!("falha ao rodar o git: {e}")))?;
        match out.exit {
            Exit::Code(Some(0)) => Ok(summary(&out)),
            Exit::TimedOut => Err(GitError::Timeout),
            _ if repo.state() == RepoState::Rebasing => {
                Ok("Rebase pausado em conflito: resolva os conflitos e clique em Continuar.".into())
            }
            _ => Err(classify_error(&String::from_utf8_lossy(&out.stderr))),
        }
    }

    /// Continua a operação em andamento (`merge --continue`, `rebase --continue`, `cherry-pick --continue`, `revert --continue`).
    pub fn git_continue_op(&self) -> Result<String, GitError> {
        let repo = self.git_repo()?;
        let _op = self.inner.git.op.lock();
        repo.check_gpg_ready()?;
        let sub = match repo.state() {
            RepoState::Merging => "merge",
            RepoState::Rebasing => "rebase",
            RepoState::CherryPicking => "cherry-pick",
            RepoState::Reverting => "revert",
            RepoState::Normal => return Err(GitError::Failed("nenhuma operação em andamento".into())),
        };
        let out = run(repo.cmd(&[sub, "--continue"]), None, COMMIT_TIMEOUT)?;
        Ok(summary(&out))
    }

    /// Aborta a operação em andamento (`merge --abort`, `rebase --abort`, `cherry-pick --abort`, `revert --abort`).
    pub fn git_abort_op(&self) -> Result<String, GitError> {
        let repo = self.git_repo()?;
        let _op = self.inner.git.op.lock();
        let sub = match repo.state() {
            RepoState::Merging => "merge",
            RepoState::Rebasing => "rebase",
            RepoState::CherryPicking => "cherry-pick",
            RepoState::Reverting => "revert",
            RepoState::Normal => return Err(GitError::Failed("nenhuma operação em andamento".into())),
        };
        let out = repo.write(&[sub, "--abort"], None)?;
        Ok(summary(&out))
    }

    // ---------- Conflitos ----------

    /// Lê os blocos de conflito (`<<<<<<<` … `>>>>>>>`) de um arquivo em conflito.
    pub fn git_conflict_file(&self, path: &str) -> Result<GitConflictFile, GitError> {
        let repo = self.git_repo()?;
        if !safe_rel(path) {
            return Err(GitError::Failed(format!("caminho inválido: {path}")));
        }
        let full = repo.root.join(path);
        if !full.is_file() {
            return Ok(GitConflictFile { path: path.to_owned(), blocks: Vec::new(), binary: false });
        }
        let (bytes, _cut) = read_capped(&full, CONFLICT_MAX_BYTES).map_err(|e| GitError::Failed(format!("falha ao ler {path}: {e}")))?;
        if bytes.iter().take(8000).any(|b| *b == 0) {
            return Ok(GitConflictFile { path: path.to_owned(), blocks: Vec::new(), binary: true });
        }
        let text = String::from_utf8_lossy(&bytes);
        Ok(GitConflictFile { path: path.to_owned(), blocks: parse_conflict_blocks(&text), binary: false })
    }

    /// Resolve o arquivo inteiro em conflito escolhendo o nosso lado (`Ours` → `--ours`), o deles
    /// (`Theirs` → `--theirs`) ou ambos (`Both`, resolvendo todos os blocos do arquivo), e já o
    /// prepara (`git add`) como resolvido.
    pub fn git_resolve_conflict_file(&self, path: &str, choice: ConflictChoice) -> Result<(), GitError> {
        let repo = self.git_repo()?;
        if !safe_rel(path) {
            return Err(GitError::Failed(format!("caminho inválido: {path}")));
        }
        let _op = self.inner.git.op.lock();
        match choice {
            ConflictChoice::Ours | ConflictChoice::Theirs => {
                let side = if choice == ConflictChoice::Ours { "--ours" } else { "--theirs" };
                // Se o lado escolhido existe no índice (estágio 2 = ours, 3 = theirs), `checkout <side>`
                // restaura-o; se o arquivo foi deletado naquele lado (ex.: conflito modify/delete),
                // `git rm` aplica a remoção.
                let stage = if choice == ConflictChoice::Ours { "2" } else { "3" };
                let has_stage = repo
                    .read(&["ls-files", "-u", "--", path])
                    .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).lines().any(|l| l.split_whitespace().nth(2) == Some(stage)));
                if has_stage {
                    repo.write(&["checkout", side, "--", path], None)?;
                    repo.write(&["add", "--", path], None)?;
                } else {
                    repo.write(&["rm", "-f", "-q", "--", path], None)?;
                }
            }
            ConflictChoice::Both => {
                let full = repo.root.join(path);
                let text = std::fs::read_to_string(&full).map_err(|e| GitError::Failed(format!("falha ao ler {path}: {e}")))?;
                let mut cur = text;
                while let Some(next) = resolve_conflict_block_in_text(&cur, 0, ConflictChoice::Both) {
                    cur = next;
                }
                write_atomic(&full, cur.as_bytes()).map_err(|e| GitError::Failed(format!("falha ao gravar {path}: {e}")))?;
                repo.write(&["add", "--", path], None)?;
            }
        }
        Ok(())
    }

    /// Resolve um único bloco (`block_index`, 0-indexed) de um arquivo em conflito.
    pub fn git_resolve_conflict_block(&self, path: &str, block_index: usize, choice: ConflictChoice) -> Result<(), GitError> {
        let repo = self.git_repo()?;
        if !safe_rel(path) {
            return Err(GitError::Failed(format!("caminho inválido: {path}")));
        }
        let _op = self.inner.git.op.lock();
        let full = repo.root.join(path);
        let text = std::fs::read_to_string(&full).map_err(|e| GitError::Failed(format!("falha ao ler {path}: {e}")))?;
        let new_text = resolve_conflict_block_in_text(&text, block_index, choice)
            .ok_or_else(|| GitError::Failed("bloco de conflito não encontrado (o arquivo mudou)".into()))?;
        write_atomic(&full, new_text.as_bytes()).map_err(|e| GitError::Failed(format!("falha ao gravar {path}: {e}")))?;
        Ok(())
    }

    /// `git init` no workspace (nunca na pasta pessoal do usuário).
    pub fn git_init(&self) -> Result<(), GitError> {
        let git = self.git_exe().ok_or(GitError::NotInstalled)?;
        let ws = self.workspace();
        if ws.as_os_str().is_empty() {
            return Err(GitError::Failed("nenhum projeto aberto".into()));
        }
        if crate::util::same_path(&crate::util::absolutize(&ws), &crate::util::absolutize(&self.inner.home)) {
            return Err(GitError::Failed("não crio um repositório na sua pasta pessoal; escolha uma pasta de projeto".into()));
        }
        run(base_command(&git, &ws, &["init", "-q"]), None, WRITE_TIMEOUT)?;
        Ok(())
    }
}

/// Grava `bytes` em `path` atomicamente via arquivo temporário na mesma pasta.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("tmp");
    let tmp = parent.join(format!(".{name}.commanddeck-tmp"));
    std::fs::write(&tmp, bytes)?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        // No Windows, `rename` por cima de arquivo existente às vezes exige remover ou tentar de novo.
        let _ = std::fs::write(path, bytes);
        let _ = std::fs::remove_file(&tmp);
        if !path.exists() {
            return Err(e);
        }
    }
    Ok(())
}

/// Lê no máximo `max` bytes do arquivo. Devolve (bytes, cortado).
fn read_capped(path: &Path, max: usize) -> std::io::Result<(Vec<u8>, bool)> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut buf = Vec::new();
    f.by_ref().take(max as u64 + 1).read_to_end(&mut buf)?;
    let cut = buf.len() > max;
    buf.truncate(max);
    Ok((buf, cut))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn z(parts: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        for p in parts {
            out.extend_from_slice(p.as_bytes());
            out.push(0);
        }
        out
    }

    #[test]
    fn status_of_a_fresh_repository() {
        let st = parse_status_v2(&z(&["# branch.oid (initial)", "# branch.head main", "? novo arquivo.txt"]));
        assert_eq!((st.head, st.branch.as_deref(), st.upstream, st.ahead, st.behind), (None, Some("main"), None, 0, 0));
        assert_eq!(
            st.entries,
            [GitEntry { path: "novo arquivo.txt".into(), orig_path: None, index: '?', worktree: '?', kind: EntryKind::Untracked }]
        );
    }

    #[test]
    fn status_with_upstream_renames_conflicts_and_spaces() {
        let out = z(&[
            "# branch.oid 1234567890abcdef",
            "# branch.head feat/x",
            "# branch.upstream origin/feat/x",
            "# branch.ab +2 -3",
            "# stash 4",
            "1 .M N... 100644 100644 100644 aaaa bbbb a b.txt",
            "1 A. N... 000000 100644 100644 0000 cccc ção/ü.txt",
            "2 R. N... 100644 100644 100644 dddd eeee R100 novo nome.rs",
            "velho nome.rs",
            "u UU N... 100644 100644 100644 100644 h1 h2 h3 conflito.md",
            "? dir/",
        ]);
        let st = parse_status_v2(&out);
        assert_eq!(
            (st.head.as_deref(), st.branch.as_deref(), st.upstream.as_deref()),
            (Some("1234567890abcdef"), Some("feat/x"), Some("origin/feat/x"))
        );
        assert_eq!((st.ahead, st.behind, st.stash_count), (2, 3, 4));
        let e = &st.entries;
        assert_eq!(e.len(), 5);
        assert_eq!((e[0].path.as_str(), e[0].index, e[0].worktree), ("a b.txt", '.', 'M'));
        assert!(!e[0].staged() && e[0].unstaged());
        assert_eq!((e[1].path.as_str(), e[1].index), ("ção/ü.txt", 'A'));
        assert!(e[1].staged() && !e[1].unstaged());
        assert_eq!((e[2].path.as_str(), e[2].orig_path.as_deref(), e[2].kind), ("novo nome.rs", Some("velho nome.rs"), EntryKind::Renamed));
        assert_eq!((e[3].path.as_str(), e[3].kind), ("conflito.md", EntryKind::Unmerged));
        assert!(!e[3].staged() && !e[3].unstaged());
        assert_eq!((e[4].path.as_str(), e[4].kind), ("dir/", EntryKind::Untracked));
    }

    #[test]
    fn detached_head_and_invalid_utf8() {
        let mut out = z(&["# branch.oid abc", "# branch.head (detached)"]);
        out.extend_from_slice(b"? arq\xff.txt\0");
        let st = parse_status_v2(&out);
        assert_eq!(st.branch, None);
        assert_eq!(st.entries[0].path, "arq\u{fffd}.txt");
    }

    #[test]
    fn too_many_entries_are_truncated() {
        let lines: Vec<String> = (0..STATUS_MAX_ENTRIES + 3).map(|i| format!("? f{i}")).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let st = parse_status_v2(&z(&refs));
        assert_eq!(st.entries.len(), STATUS_MAX_ENTRIES);
        assert!(st.truncated);
    }

    #[test]
    fn branches_skip_the_remote_head() {
        let out = "refs/heads/main\0main\0origin/main\0*\nrefs/heads/feat\0feat\0\0 \nrefs/remotes/origin/HEAD\0origin\0\0 \nrefs/remotes/origin/main\0origin/main\0\0 \n";
        let b = parse_branches(out.as_bytes());
        assert_eq!(
            b.iter().map(|b| (b.name.as_str(), b.remote, b.current)).collect::<Vec<_>>(),
            [("main", false, true), ("feat", false, false), ("origin/main", true, false)]
        );
        assert_eq!(b[0].upstream.as_deref(), Some("origin/main"));
        assert_eq!(b[1].upstream, None);
    }

    #[test]
    fn diff_lines_are_numbered() {
        let text = "diff --git a/a.rs b/a.rs\nindex 1..2 100644\n--- a/a.rs\n+++ b/a.rs\n@@ -10,3 +10,4 @@ fn x()\n ctx\n-old\n+new\n+more\n ctx2\n\\ No newline at end of file\n";
        let d = parse_diff(text, 100);
        let k: Vec<(DiffKind, Option<u32>, Option<u32>)> = d.lines.iter().map(|l| (l.kind, l.old, l.new)).collect();
        assert_eq!(
            k,
            [
                (DiffKind::Meta, None, None),
                (DiffKind::Meta, None, None),
                (DiffKind::Meta, None, None),
                (DiffKind::Meta, None, None),
                (DiffKind::Hunk, None, None),
                (DiffKind::Context, Some(10), Some(10)),
                (DiffKind::Del, Some(11), None),
                (DiffKind::Add, None, Some(11)),
                (DiffKind::Add, None, Some(12)),
                (DiffKind::Context, Some(12), Some(13)),
                (DiffKind::Meta, None, None),
            ]
        );
        assert!(!d.binary && !d.truncated);
        let d = parse_diff("diff --git a/x b/x\nBinary files a/x and b/x differ\n", 100);
        assert!(d.binary);
        let d = parse_diff(text, 3);
        assert!(d.truncated && d.lines.len() == 3);
    }

    #[test]
    fn combined_diff_cc_is_parsed_correctly() {
        let text = "diff --cc a.txt\nindex 111,222..000\n--- a/a.txt\n+++ b/a.txt\n@@@ -1,3 -1,3 +1,5 @@@\n  um\n++<<<<<<< HEAD\n +main\n++=======\n+ feat\n++>>>>>>> feat/x\n";
        let d = parse_diff(text, 100);
        let k: Vec<(DiffKind, Option<u32>, Option<u32>)> = d.lines.iter().map(|l| (l.kind, l.old, l.new)).collect();
        assert_eq!(
            k,
            [
                (DiffKind::Meta, None, None),
                (DiffKind::Meta, None, None),
                (DiffKind::Meta, None, None),
                (DiffKind::Meta, None, None),
                (DiffKind::Hunk, None, None),
                (DiffKind::Context, Some(1), Some(1)),
                (DiffKind::Add, None, Some(2)),
                (DiffKind::Add, None, Some(3)),
                (DiffKind::Add, None, Some(4)),
                (DiffKind::Add, None, Some(5)),
                (DiffKind::Add, None, Some(6)),
            ]
        );
        // Diff combinado não gera patch de hunk individual.
        assert!(extract_hunk_patch(&d, 0).is_none());
    }

    #[test]
    fn extract_single_hunk_patch() {
        let text = "diff --git a/a.txt b/a.txt\nindex 1..2 100644\n--- a/a.txt\n+++ b/a.txt\n@@ -1,2 +1,2 @@\n-um\n+UM\n dois\n@@ -10,2 +10,2 @@\n nove\n-dez\n+DEZ\n";
        let d = parse_diff(text, 100);
        let p0 = extract_hunk_patch(&d, 0).expect("hunk 0");
        assert!(p0.contains("@@ -1,2 +1,2 @@") && !p0.contains("@@ -10,2 +10,2 @@"));
        let p1 = extract_hunk_patch(&d, 1).expect("hunk 1");
        assert!(p1.contains("@@ -10,2 +10,2 @@") && !p1.contains("@@ -1,2 +1,2 @@"));
        assert!(extract_hunk_patch(&d, 2).is_none());
    }

    #[test]
    fn conflict_blocks_are_parsed_and_resolved() {
        let content = "inicio\n<<<<<<< HEAD\nnosso 1\nnosso 2\n=======\ndeles 1\n>>>>>>> feat/x\nmeio\n<<<<<<< HEAD\nA\n||||||| base\nO\n=======\nB\n>>>>>>> feat/x\nfim\n";
        let blocks = parse_conflict_blocks(content);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].start_line, 2);
        assert_eq!(blocks[0].ours_label, "HEAD");
        assert_eq!(blocks[0].ours, ["nosso 1", "nosso 2"]);
        assert_eq!(blocks[0].theirs_label, "feat/x");
        assert_eq!(blocks[0].theirs, ["deles 1"]);
        assert_eq!(blocks[1].ours, ["A"]);
        assert_eq!(blocks[1].theirs, ["B"]);

        let after0 = resolve_conflict_block_in_text(content, 0, ConflictChoice::Ours).expect("resolve 0");
        assert_eq!(after0, "inicio\nnosso 1\nnosso 2\nmeio\n<<<<<<< HEAD\nA\n||||||| base\nO\n=======\nB\n>>>>>>> feat/x\nfim\n");
        let after1 = resolve_conflict_block_in_text(&after0, 0, ConflictChoice::Both).expect("resolve 1");
        assert_eq!(after1, "inicio\nnosso 1\nnosso 2\nmeio\nA\nB\nfim\n");
    }

    #[test]
    fn stashes_and_log_are_parsed() {
        let st = parse_stashes(b"stash@{0}\02 hours ago\0On main: rascunho\nstash@{1}\03 days ago\0WIP on feat: x\n");
        assert_eq!(
            st,
            [
                GitStash { index: 0, date: "2 hours ago".into(), message: "On main: rascunho".into() },
                GitStash { index: 1, date: "3 days ago".into(), message: "WIP on feat: x".into() },
            ]
        );
        let log_bytes = b"aaaa1111\0aaaa\0Ana\02 hours ago\0feat: x\0\nbbbb2222\0bbbb\0Bia\0yesterday\0fix: y\0";
        let commits = parse_log(log_bytes);
        assert_eq!(
            commits,
            [
                GitCommit {
                    hash: "aaaa1111".into(),
                    short_hash: "aaaa".into(),
                    author: "Ana".into(),
                    date: "2 hours ago".into(),
                    subject: "feat: x".into()
                },
                GitCommit {
                    hash: "bbbb2222".into(),
                    short_hash: "bbbb".into(),
                    author: "Bia".into(),
                    date: "yesterday".into(),
                    subject: "fix: y".into()
                },
            ]
        );
    }

    #[test]
    fn untracked_files_show_every_line_or_binary() {
        let d = untracked_diff(b"a\nb\n", 100);
        assert_eq!(
            d.lines.iter().map(|l| (l.kind, l.new, l.text.as_str())).collect::<Vec<_>>(),
            [(DiffKind::Add, Some(1), "a"), (DiffKind::Add, Some(2), "b")]
        );
        assert!(untracked_diff(b"PK\0\x03", 100).binary);
    }

    #[test]
    fn errors_are_recognized() {
        assert_eq!(classify_error("fatal: not a git repository (or any of the parent directories): .git"), GitError::NotARepo);
        assert_eq!(
            classify_error("fatal: detected dubious ownership in repository at '/srv/x'\nTo add an exception…"),
            GitError::Unsafe { path: "/srv/x".into() }
        );
        assert_eq!(classify_error("*** Please tell me who you are.\n\nRun\n"), GitError::NoIdentity);
        assert!(matches!(
            classify_error("fatal: could not read Username for 'https://github.com': terminal prompts disabled"),
            GitError::Auth(_)
        ));
        let GitError::Auth(ssh_msg) =
            classify_error("git@github.com: Permission denied (publickey).\nfatal: Could not read from remote repository.")
        else {
            panic!("ssh auth")
        };
        assert!(ssh_msg.contains("ssh-agent") && ssh_msg.contains("ssh-add") && ssh_msg.contains("Start-Service ssh-agent"), "{ssh_msg}");
        assert!(matches!(classify_error(" ! [rejected]        main -> main (fetch first)\nerror: failed to push"), GitError::Rejected(_)));
        assert!(matches!(classify_error("fatal: Not possible to fast-forward, aborting."), GitError::Rejected(_)));
        assert!(matches!(
            classify_error("fatal: Cannot prompt because user interactivity has been disabled.\nfatal: could not read Username"),
            GitError::Auth(_)
        ));
        let GitError::Failed(m) = classify_error("error: gpg failed to sign the data\nfatal: failed to write commit object") else {
            panic!("gpg")
        };
        assert!(m.contains("commit.gpgsign"), "{m}");
        assert_eq!(classify_error("error: pathspec 'x' did not match"), GitError::Failed("error: pathspec 'x' did not match".into()));
    }

    #[test]
    fn branch_names_and_paths_are_checked() {
        for ok in ["main", "feat/x", "fix-1.2", "user/ana/teste"] {
            assert!(valid_branch_name(ok), "{ok}");
        }
        for bad in ["", "-f", "a b", "a..b", "a~1", "a^", "x:y", "a.lock", "a/", "/a", ".hidden", "a/.b", "@", "a@{1}", "a\\b", "a*"] {
            assert!(!valid_branch_name(bad), "{bad:?}");
        }
        assert!(safe_rel("src/main.rs") && safe_rel("a b/ção.txt") && safe_rel("./a") && safe_rel("dir/"));
        for bad in ["", "/etc/passwd", "/x", "../x", "a/../b", "a/../../x", "a\\..\\b", "\\\\server\\x", "\\x", "a\0b"] {
            assert!(!safe_rel(bad), "{bad:?}");
        }
        // Relativo à unidade: `C:..\x` sairia da raiz do repositório.
        if cfg!(windows) {
            for bad in ["C:..\\x", "C:x", "C:\\x", "a:b"] {
                assert!(!safe_rel(bad), "{bad:?}");
            }
        }
    }

    #[test]
    fn errors_read_in_portuguese() {
        assert!(GitError::NotInstalled.to_string().contains("git-scm.com"));
        assert!(GitError::Unsafe { path: "/x".into() }.to_string().contains("safe.directory \"/x\""));
    }
}
