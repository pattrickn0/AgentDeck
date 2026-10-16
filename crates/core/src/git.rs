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

/// Lê `git status --porcelain=v2 -z --branch`. `root`/`prefix`/`state` ficam por conta de quem chama.
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

/// Lê um diff unificado. Corta em `max_lines` linhas (e cada linha em 1000 caracteres).
pub fn parse_diff(text: &str, max_lines: usize) -> GitDiff {
    let mut diff = GitDiff::default();
    let (mut old, mut new) = (0u32, 0u32);
    let mut in_hunk = false;
    for raw in text.lines() {
        if diff.lines.len() >= max_lines {
            diff.truncated = true;
            break;
        }
        let line: String = if raw.chars().count() > 1000 { raw.chars().take(1000).chain(['…']).collect() } else { raw.to_owned() };
        let (kind, o, n) = if raw.starts_with("diff --git ") || raw.starts_with("diff --cc ") {
            in_hunk = false;
            (DiffKind::Meta, None, None)
        } else if raw.starts_with("Binary files ") && raw.ends_with(" differ") {
            diff.binary = true;
            (DiffKind::Meta, None, None)
        } else if let Some(rest) = raw.strip_prefix("@@") {
            in_hunk = true;
            // `@@ -a[,b] +c[,d] @@`
            let mut parts = rest.split_whitespace();
            let start = |p: Option<&str>, sign: char| -> u32 {
                p.and_then(|s| s.strip_prefix(sign)).and_then(|s| s.split(',').next()).and_then(|s| s.parse().ok()).unwrap_or(0)
            };
            old = start(parts.next(), '-');
            new = start(parts.next(), '+');
            (DiffKind::Hunk, None, None)
        } else if !in_hunk || raw.starts_with('\\') {
            (DiffKind::Meta, None, None)
        } else if raw.starts_with('+') {
            new += 1;
            (DiffKind::Add, None, Some(new - 1))
        } else if raw.starts_with('-') {
            old += 1;
            (DiffKind::Del, Some(old - 1), None)
        } else {
            old += 1;
            new += 1;
            (DiffKind::Context, Some(old - 1), Some(new - 1))
        };
        diff.lines.push(DiffLine { kind, old: o, new: n, text: line });
    }
    diff
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
    if low.contains("authentication failed")
        || low.contains("could not read username")
        || low.contains("could not read password")
        || low.contains("permission denied (publickey")
        || low.contains("terminal prompts disabled")
        || low.contains("user interactivity has been disabled")
        || low.contains("host key verification failed")
    {
        return GitError::Auth(short);
    }
    // `commit.gpgsign` sem agente/pinentry que peça a senha: o gpg não consegue assinar.
    if low.contains("gpg failed to sign") {
        return GitError::Failed(format!(
            "o git não conseguiu assinar o commit (commit.gpgsign). Destrave a chave no seu agente GPG ou desative a assinatura neste repositório: git config commit.gpgsign false\n{short}"
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
        ("GIT_OPTIONAL_LOCKS", "0"),
        ("GIT_MERGE_AUTOEDIT", "no"),
        ("GIT_PAGER", "cat"),
        ("GIT_EDITOR", "true"),
        ("GIT_LITERAL_PATHSPECS", "1"),
    ] {
        c.env(k, v);
    }
    for k in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_PREFIX"] {
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
        _ => Err(classify_error(&String::from_utf8_lossy(&out.stderr))),
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
        let out = repo.read(&["status", "--porcelain=v2", "-z", "--branch", "--untracked-files=normal"])?;
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

    /// Descarta as alterações não preparadas de um arquivo (rastreado: volta ao que está no
    /// índice; não rastreado: APAGA o arquivo). A interface pede confirmação antes.
    pub fn git_discard(&self, entry: &GitEntry) -> Result<(), GitError> {
        let repo = self.git_repo()?;
        if !safe_rel(&entry.path) {
            return Err(GitError::Failed(format!("caminho inválido: {}", entry.path)));
        }
        let _op = self.inner.git.op.lock();
        if entry.kind == EntryKind::Untracked {
            let flags = if entry.path.ends_with('/') { "-fdq" } else { "-fq" };
            repo.write(&["clean", flags, "--", &entry.path], None)?;
        } else {
            repo.write(&["restore", "--worktree", "--", &entry.path], None)?;
        }
        Ok(())
    }

    /// Faz o commit do que está preparado. Devolve `"<hash> <assunto>"`.
    pub fn git_commit(&self, message: &str, amend: bool) -> Result<String, GitError> {
        if message.trim().is_empty() {
            return Err(GitError::Failed("escreva a mensagem do commit".into()));
        }
        let repo = self.git_repo()?;
        let _op = self.inner.git.op.lock();
        let mut args = vec!["commit", "--cleanup=strip", "-F", "-"];
        if amend {
            args.push("--amend");
        }
        run(repo.cmd(&args), Some(message.as_bytes().to_vec()), COMMIT_TIMEOUT)?;
        let out = repo.read(&["log", "-1", "--format=%h %s"])?;
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    }

    /// Mensagem do último commit (para "corrigir último commit").
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
        assert_eq!((st.ahead, st.behind), (2, 3));
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
