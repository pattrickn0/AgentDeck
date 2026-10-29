//! Integration with the workspace's git (the "Git" tab of the interface).
//!
//! Everything goes through the installed `git` executable, always with `Command::new(git).args([...])` (never
//! through a shell) and an environment that does not hang: no password prompt in the terminal (`GIT_TERMINAL_PROMPT=0`,
//! ssh in `BatchMode`, Git Credential Manager with no window: `GCM_INTERACTIVE=never`), no pager, messages in English (`LC_ALL=C`, to recognize the errors) and
//! literal paths (`GIT_LITERAL_PATHSPECS=1`). Path lists and the commit message go through
//! standard input (no command-line limit on Windows, no interpretation of anything).
//!
//! The output is read by the pure functions of this module (`parse_*`), tested without git; the operations
//! (`impl Core`) are blocking: the interface calls them on a thread.

use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::process::{run_captured_raw, CapturedRaw, Exit};
use crate::{t, Core};

/// Fast reads (status, diff, branches).
pub(crate) const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// Local operations that write (stage, checkout…).
pub(crate) const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// Commit (hooks and GPG signing can take a while).
pub(crate) const COMMIT_TIMEOUT: Duration = Duration::from_secs(120);
/// Network (fetch, pull, push).
const NET_TIMEOUT: Duration = Duration::from_secs(60);
/// Maximum output kept from a command (the diff of a huge file is cut).
pub(crate) const MAX_OUT: usize = 4 << 20;
/// Diff shown: at most this many bytes and lines.
pub const DIFF_MAX_BYTES: usize = 1 << 20;
pub const DIFF_MAX_LINES: usize = 10_000;
/// Status entries shown.
pub const STATUS_MAX_ENTRIES: usize = 5000;
/// Maximum size of a file read for conflict resolution.
pub const CONFLICT_MAX_BYTES: usize = 2 << 20;

// ---------- Types ----------

/// What git returned about the repository.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitStatus {
    /// Repository root.
    pub root: PathBuf,
    /// The workspace relative to the root (`""` = the workspace is the root; `"workspace/"` = subfolder).
    pub prefix: String,
    /// Current branch (`None` = detached HEAD).
    pub branch: Option<String>,
    /// Current commit (`None` = repository with no commits).
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub state: RepoState,
    /// Paths relative to the root.
    pub entries: Vec<GitEntry>,
    /// How many stashes are saved in the repository.
    pub stash_count: u32,
    /// There were more than [`STATUS_MAX_ENTRIES`] changes.
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
    /// Merge conflict.
    Unmerged,
    Untracked,
}

/// A changed file. `index`/`worktree` are git's letters (`M`, `A`, `D`, `R`, `C`, `U`,
/// `?`, `.` = no change on that side).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitEntry {
    pub path: String,
    /// Previous name (renamed/copied).
    pub orig_path: Option<String>,
    pub index: char,
    pub worktree: char,
    pub kind: EntryKind,
}

impl GitEntry {
    /// Has a staged change.
    pub fn staged(&self) -> bool {
        !matches!(self.kind, EntryKind::Untracked | EntryKind::Unmerged) && self.index != '.'
    }

    /// Has an unstaged change in the working copy.
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
    /// Headers (`diff --git`, `index`, `---`, `+++`, `\ No newline…`).
    Meta,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffKind,
    /// Line number in the old / new file.
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitDiff {
    pub lines: Vec<DiffLine>,
    /// Cut at [`DIFF_MAX_LINES`]/[`DIFF_MAX_BYTES`].
    pub truncated: bool,
    pub binary: bool,
}

/// An entry of the stash stack (`stash@{index}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitStash {
    pub index: usize,
    pub date: String,
    pub message: String,
}

/// A commit of the history (log).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCommit {
    pub hash: String,
    pub short_hash: String,
    pub author: String,
    pub date: String,
    pub subject: String,
}

/// Detail of a commit (metadata, changed files and diff).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitCommitDetail {
    pub hash: String,
    pub short_hash: String,
    pub author: String,
    pub email: String,
    pub date: String,
    pub message: String,
    /// `(letter, path)` of the files changed in the commit.
    pub files: Vec<(char, String)>,
    pub diff: GitDiff,
}

/// A conflict block (`<<<<<<<` … `=======` … `>>>>>>>`) inside a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictBlock {
    /// Starting line of the `<<<<<<<` marker (1-indexed).
    pub start_line: u32,
    pub ours_label: String,
    pub ours: Vec<String>,
    pub theirs_label: String,
    pub theirs: Vec<String>,
}

/// File in conflict with its blocks (or whole sides when there are no text markers).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitConflictFile {
    pub path: String,
    pub blocks: Vec<ConflictBlock>,
    pub binary: bool,
}

/// Choice to resolve a block or a whole file in conflict.
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
    /// Git refuses the folder because it belongs to another user (`safe.directory`).
    Unsafe {
        path: String,
    },
    /// Missing `user.name`/`user.email` for the commit.
    NoIdentity,
    Timeout,
    /// The remote refused (it needs a pull first, for example).
    Rejected(String),
    /// Credential missing or refused.
    Auth(String),
    Failed(String),
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitError::NotInstalled => write!(f, "{}", t!("Git was not found. Install it from https://git-scm.com and try again.")),
            GitError::NotARepo => write!(f, "{}", t!("The working folder is not a git repository.")),
            GitError::Unsafe { path } => write!(
                f,
                "{}",
                t!(
                    "Git does not trust this folder (it belongs to another user). To allow it, run: git config --global --add safe.directory \"{path}\"",
                    path
                )
            ),
            GitError::NoIdentity => write!(
                f,
                "{}",
                t!(
                    "Set your name and e-mail for the commit: git config --global user.name \"Your Name\" and git config --global user.email you@example.com"
                )
            ),
            GitError::Timeout => write!(f, "{}", t!("Git took too long and was interrupted.")),
            GitError::Rejected(m) => write!(f, "{}", t!("The remote refused: {message}", message = m)),
            GitError::Auth(m) => write!(f, "{}", t!("Authentication failed: {message}", message = m)),
            GitError::Failed(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for GitError {}

// ---------- Output parsing (pure functions) ----------

/// Reads `git status --porcelain=v2 -z --branch --show-stash`. `root`/`prefix`/`state` are up to the caller.
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
                // The previous name is the next record.
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

/// Format asked of `git for-each-ref` (NUL-separated fields, one ref per line).
pub const BRANCH_FORMAT: &str = "%(refname)%00%(refname:short)%00%(upstream:short)%00%(HEAD)";

/// Reads the output of `git for-each-ref --format=BRANCH_FORMAT refs/heads refs/remotes`.
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
        // `origin/HEAD` points to another branch: it is not a useful choice.
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

/// Reads a unified diff (or a combined one, `diff --cc` / `@@@`). Cuts at `max_lines` lines (and each
/// line at 1000 characters). The trailing `\r` of a CRLF line is kept in `text`.
pub fn parse_diff(text: &str, max_lines: usize) -> GitDiff {
    let mut diff = GitDiff::default();
    let (mut old, mut new) = (0u32, 0u32);
    let mut in_hunk = false;
    // Number of column prefixes (1 for a normal `@@` diff, >= 2 for a combined `@@@` diff).
    let mut cols = 1usize;
    // Split only on `\n`: the `\r` of CRLF files stays in the text (the patch of a hunk needs it to
    // apply; whoever draws it removes the `\r`).
    let mut raws: Vec<&str> = text.split('\n').collect();
    if raws.last() == Some(&"") {
        raws.pop();
    }
    for raw in raws {
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
            // `@@ -a[,b] +c[,d] @@` or `@@@ -a[,b] -c[,d] +e[,f] @@@`
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
            // Combined diff (`diff --cc`): the first `cols` characters mark `+`, `-` or ` ` in each parent.
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

/// Builds a unified patch containing only the hunk (`hunk_index`, 0-indexed) of `diff`, ready for
/// `git apply`. Returns `None` if the diff is combined (`@@@`), truncated, binary or has no
/// `---`/`+++` header.
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
                // A combined diff (`@@@`) cannot be applied with `git apply`.
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
    // A truncated line (> 1000 chars) would not produce a faithful patch.
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

/// Format asked of `git stash list`.
pub const STASH_FORMAT: &str = "%gd%x00%cr%x00%gs";

/// Reads the output of `git stash list --format=STASH_FORMAT`.
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

/// Format asked of `git log -z`.
pub const LOG_FORMAT: &str = "%H%x00%h%x00%an%x00%cr%x00%s";

/// Reads the output of `git log -z --format=LOG_FORMAT`.
pub fn parse_log(out: &[u8]) -> Vec<GitCommit> {
    let mut list = Vec::new();
    // With `-z` and `%x00`, all fields stay separated by NUL, five at a time.
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

/// Finds the conflict blocks (`<<<<<<<` … `=======` … `>>>>>>>`) in a text.
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
        // Also accepts diff3 conflicts (`|||||||`), ignoring the base section between `|||||||` and `=======`.
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
        i += 1; // skip `=======`
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

/// Resolves a single conflict block (`block_index`, 0-indexed) by replacing the markers with the
/// choice (`Ours`, `Theirs` or `Both`), preserving the line-break style (`\r\n` or `\n`).
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

/// Diff of a not yet tracked file: every line as added.
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

/// Turns the stderr of a command that failed into a meaningful error (the messages come in
/// English because of `LC_ALL=C`).
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
        return GitError::Auth(t!(
            "{output}\nIf your SSH key has a passphrase, AgentDeck cannot ask for it: load the key into ssh-agent and try again.\n• Windows (PowerShell; the first time as administrator): Set-Service ssh-agent -StartupType Manual; Start-Service ssh-agent; ssh-add\n• Linux/macOS: eval \"$(ssh-agent -s)\" && ssh-add",
            output = short
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
    // `commit.gpgsign` with a locked key or with no agent/pinentry that can ask for the passphrase.
    if low.contains("gpg failed to sign") || low.contains("no pinentry") || low.contains("inappropriate ioctl for device") {
        return GitError::Failed(t!(
            "The commit was not made: the GPG signature (commit.gpgsign) needs the key unlocked, and AgentDeck cannot open the passphrase prompt. Unlock the key in a terminal tab (echo test | gpg --clearsign) and try again right after; the passphrase stays in gpg-agent for a few minutes. If you do not want to sign in this repository: git config commit.gpgsign false\n{output}",
            output = short
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
    GitError::Failed(if short.is_empty() { t!("git failed with no message").to_owned() } else { short })
}

/// Acceptable branch name (a safe subset of the rules of `git check-ref-format`).
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

/// Safe relative path to pass to git (no `..`, not absolute, no NUL). Accepts only ordinary
/// components (and `.`); on Windows, no `:` (`C:..\x` is relative to the drive and leaves the root).
pub fn safe_rel(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\0')
        && !(cfg!(windows) && path.contains(':'))
        && !path.starts_with(['/', '\\'])
        && !path.split(['/', '\\']).any(|p| p == "..")
        && Path::new(path).components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

// ---------- Execution ----------

/// Git state kept in the core.
#[derive(Default)]
pub(crate) struct GitState {
    /// Executable found (and when it was looked up: the search is redone after 30 s, to notice
    /// an installation made with the app open).
    exe: Mutex<Option<(Instant, Option<PathBuf>)>>,
    /// One operation that writes to the repository at a time (git uses `index.lock`).
    pub(crate) op: Mutex<()>,
}

/// Repository found from the workspace.
pub(crate) struct Repo {
    pub(crate) git: PathBuf,
    pub(crate) root: PathBuf,
    pub(crate) git_dir: PathBuf,
    prefix: String,
}

pub(crate) fn base_command(git: &Path, cwd: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(git);
    c.current_dir(cwd).args(["-c", "core.quotepath=false", "-c", "color.ui=false"]).args(args);
    for (k, v) in [
        ("LC_ALL", "C"),
        ("LANGUAGE", "C"),
        ("GIT_TERMINAL_PROMPT", "0"),
        // Git Credential Manager would open a login window and the command would stay stuck until
        // the timeout; with no saved credential, it fails right away (becomes `GitError::Auth`).
        ("GCM_INTERACTIVE", "never"),
        ("SSH_ASKPASS_REQUIRE", "never"),
        ("GIT_OPTIONAL_LOCKS", "0"),
        ("GIT_MERGE_AUTOEDIT", "no"),
        ("GIT_PAGER", "cat"),
        ("GIT_EDITOR", "true"),
        ("GIT_SEQUENCE_EDITOR", "true"),
        ("GIT_LITERAL_PATHSPECS", "1"),
        // No TTY for the curses pinentry; with a graphical pinentry it uses DISPLAY/the OS session;
        // if it is a terminal, it fails immediately instead of waiting for the 120 s timeout.
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

pub(crate) fn run(cmd: Command, stdin: Option<Vec<u8>>, timeout: Duration) -> Result<CapturedRaw, GitError> {
    let out = run_captured_raw(cmd, stdin, timeout, None, None, MAX_OUT).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            GitError::NotInstalled
        } else {
            GitError::Failed(t!("Failed to run git: {error}", error = e))
        }
    })?;
    match out.exit {
        Exit::Code(Some(0)) => Ok(out),
        Exit::TimedOut => Err(GitError::Timeout),
        _ => {
            // Some errors (e.g. "nothing to commit") come out on standard output.
            let err = String::from_utf8_lossy(&out.stderr);
            let err = if err.trim().is_empty() { String::from_utf8_lossy(&out.stdout) } else { err };
            Err(classify_error(&err))
        }
    }
}

/// Paths separated by NUL (for `--pathspec-from-file=- --pathspec-file-nul`).
fn nul_paths(paths: &[String]) -> Result<Vec<u8>, GitError> {
    let mut buf = Vec::new();
    for p in paths {
        if !safe_rel(p) {
            return Err(GitError::Failed(t!("Invalid path: {path}", path = p)));
        }
        buf.extend_from_slice(p.as_bytes());
        buf.push(0);
    }
    Ok(buf)
}

impl Repo {
    pub(crate) fn cmd(&self, args: &[&str]) -> Command {
        base_command(&self.git, &self.root, args)
    }

    pub(crate) fn read(&self, args: &[&str]) -> Result<CapturedRaw, GitError> {
        run(self.cmd(args), None, READ_TIMEOUT)
    }

    pub(crate) fn write(&self, args: &[&str], stdin: Option<Vec<u8>>) -> Result<CapturedRaw, GitError> {
        run(self.cmd(args), stdin, WRITE_TIMEOUT)
    }

    pub(crate) fn state(&self) -> RepoState {
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

    /// Checks whether the current commit would use GPG signing (`commit.gpgsign = true`).
    fn is_gpgsign_enabled(&self) -> bool {
        self.read(&["config", "--bool", "--get", "commit.gpgsign"]).is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "true")
    }

    /// When `commit.gpgsign` is on, checks in an instant whether GPG can sign WITHOUT asking
    /// anything (`--pinentry-mode error`): with the key unlocked in `gpg-agent` it passes; with the
    /// key locked (or with no usable agent/pinentry) gpg fails right away, instead of letting the
    /// commit wait for the `COMMIT_TIMEOUT` for a passphrase prompt that nobody sees. Returns the
    /// explanation and does NOT touch the user's configuration. Only valid for OpenPGP signing.
    pub(crate) fn check_gpg_ready(&self) -> Result<(), GitError> {
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
        match run_captured_raw(cmd, Some(b"agentdeck".to_vec()), Duration::from_secs(5), None, None, 64 << 10) {
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

    /// Environment of the network operations: ssh with no prompt (otherwise it opens the terminal and hangs), unless
    /// the user has configured their own ssh.
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

/// Short text of what the command said (to show after pull/push).
fn summary(out: &CapturedRaw) -> String {
    let all = format!("{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    crate::util::last_meaningful_lines(&all, 3)
}

impl Core {
    /// Path of the git executable (or `None`).
    pub(crate) fn git_exe(&self) -> Option<PathBuf> {
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

    /// Finds the repository of the workspace.
    fn git_repo(&self) -> Result<Repo, GitError> {
        self.git_repo_at(&self.workspace())
    }

    /// Finds the repository that contains the folder `dir` (the one of the tasks, which are per project and not only
    /// of the active project).
    pub(crate) fn git_repo_at(&self, dir: &Path) -> Result<Repo, GitError> {
        let git = self.git_exe().ok_or(GitError::NotInstalled)?;
        let ws = dir;
        if ws.as_os_str().is_empty() {
            return Err(GitError::NotARepo);
        }
        let out =
            run(base_command(&git, ws, &["rev-parse", "--show-toplevel", "--absolute-git-dir", "--show-prefix"]), None, READ_TIMEOUT)?;
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let mut lines = text.lines();
        let root = lines.next().map(PathBuf::from).ok_or(GitError::NotARepo)?;
        let git_dir = lines.next().map(PathBuf::from).unwrap_or_else(|| root.join(".git"));
        let prefix = lines.next().unwrap_or("").to_owned();
        Ok(Repo { git, root, git_dir, prefix })
    }

    /// Status of the workspace's repository.
    pub fn git_status(&self) -> Result<GitStatus, GitError> {
        let repo = self.git_repo()?;
        let out = repo.read(&["status", "--porcelain=v2", "-z", "--branch", "--show-stash", "--untracked-files=normal"])?;
        let mut st = parse_status_v2(&out.stdout);
        st.state = repo.state();
        st.prefix = repo.prefix.clone();
        st.root = repo.root;
        Ok(st)
    }

    /// Diff of a file (`path` relative to the repository root). `staged`: what is staged
    /// (index × HEAD); otherwise, the working copy × index. Untracked file: the whole content.
    pub fn git_diff(&self, entry: &GitEntry, staged: bool) -> Result<GitDiff, GitError> {
        let repo = self.git_repo()?;
        if !safe_rel(&entry.path) {
            return Err(GitError::Failed(t!("Invalid path: {path}", path = entry.path)));
        }
        if entry.kind == EntryKind::Untracked {
            let full = repo.root.join(&entry.path);
            if full.is_dir() {
                return Ok(GitDiff::default());
            }
            let bytes = read_capped(&full, DIFF_MAX_BYTES)
                .map_err(|e| GitError::Failed(t!("Failed to read {path}: {error}", path = entry.path, error = e)))?;
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

    /// Stages the paths (relative to the root).
    pub fn git_stage(&self, paths: &[String]) -> Result<(), GitError> {
        let repo = self.git_repo()?;
        let _op = self.inner.git.op.lock();
        repo.write(&["add", "-A", "--pathspec-from-file=-", "--pathspec-file-nul"], Some(nul_paths(paths)?))?;
        Ok(())
    }

    /// Stages every change in the repository.
    pub fn git_stage_all(&self) -> Result<(), GitError> {
        let repo = self.git_repo()?;
        let _op = self.inner.git.op.lock();
        repo.write(&["add", "-A"], None)?;
        Ok(())
    }

    /// Unstages. A renamed file takes the old name with it (otherwise the old name would stay
    /// staged as deleted).
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
            // With no commit at all there is no HEAD to restore from: it only leaves the index.
            repo.write(&["rm", "--cached", "-r", "-q", "--pathspec-from-file=-", "--pathspec-file-nul"], Some(stdin))?;
        }
        Ok(())
    }

    /// Unstages everything.
    pub fn git_unstage_all(&self) -> Result<(), GitError> {
        let repo = self.git_repo()?;
        let _op = self.inner.git.op.lock();
        if repo.has_head() {
            repo.write(&["reset", "-q"], None)?;
        } else {
            // Empty index: nothing to unstage (without `--ignore-unmatch` git would fail with "did not match").
            repo.write(&["rm", "--cached", "-r", "-q", "--ignore-unmatch", "--", "."], None)?;
        }
        Ok(())
    }

    /// Discards the changes of a file:
    /// - untracked: DELETES the file or folder (uses `-ffdq` on folders to also remove sub-repositories with an inner `.git`);
    /// - in conflict (`Unmerged`): restores to the `HEAD` version (or removes it if it was only added by the other side, e.g. `AU`/`DU`/`UU`);
    /// - ordinary tracked file: restores the working copy to what is in the index.
    pub fn git_discard(&self, entry: &GitEntry) -> Result<(), GitError> {
        let repo = self.git_repo()?;
        if !safe_rel(&entry.path) {
            return Err(GitError::Failed(t!("Invalid path: {path}", path = entry.path)));
        }
        let _op = self.inner.git.op.lock();
        match entry.kind {
            EntryKind::Untracked => {
                let is_dir = entry.path.ends_with('/') || repo.root.join(&entry.path).is_dir();
                let flags = if is_dir { "-ffdq" } else { "-fq" };
                repo.write(&["clean", flags, "--", &entry.path], None)?;
            }
            EntryKind::Unmerged => {
                // In a conflicted file, `restore --worktree` fails because there are multiple stages in the index.
                // If the file exists in HEAD, `restore --source=HEAD --staged --worktree` restores our clean side;
                // if it does not exist in HEAD (e.g. added by them `UA` or by both `AA`), we remove it from the index and from disk.
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

    /// Applies the patch of a hunk to stage it, unstage it
    /// or discard it from the working copy.
    pub fn git_apply_hunk(&self, patch: &str, staged: bool, discard: bool) -> Result<(), GitError> {
        if patch.trim().is_empty() {
            return Err(GitError::Failed(t!("Empty hunk").to_owned()));
        }
        let repo = self.git_repo()?;
        let _op = self.inner.git.op.lock();
        let args: &[&str] = match (discard, staged) {
            // Discard an unstaged hunk from the working copy.
            (true, _) => &["apply", "--reverse", "--recount", "-"],
            // Unstage a hunk that was staged.
            (false, true) => &["apply", "--cached", "--reverse", "--recount", "-"],
            // Stage a hunk from the working copy.
            (false, false) => &["apply", "--cached", "--recount", "-"],
        };
        repo.write(args, Some(patch.as_bytes().to_vec()))?;
        Ok(())
    }

    /// Commits what is staged. Returns `"<hash> <subject>"`.
    pub fn git_commit(&self, message: &str, amend: bool) -> Result<String, GitError> {
        if message.trim().is_empty() {
            return Err(GitError::Failed(t!("Write the commit message").to_owned()));
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

    /// Message of the last commit (for "amend last commit" or a merge message).
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

    /// Pull only by fast-forward (without creating a merge by itself).
    pub fn git_pull(&self) -> Result<String, GitError> {
        let repo = self.git_repo()?;
        let _op = self.inner.git.op.lock();
        let out = run(repo.net(&["pull", "--ff-only", "--no-edit"]), None, NET_TIMEOUT)?;
        Ok(summary(&out))
    }

    /// Push. With no upstream, publishes the branch to the only remote (`push -u <remote> <branch>`).
    pub fn git_push(&self) -> Result<String, GitError> {
        let repo = self.git_repo()?;
        let _op = self.inner.git.op.lock();
        let st = parse_status_v2(&repo.read(&["status", "--porcelain=v2", "-z", "--branch", "--untracked-files=no"])?.stdout);
        let out = if st.upstream.is_some() {
            run(repo.net(&["push"]), None, NET_TIMEOUT)?
        } else {
            let branch = st.branch.ok_or_else(|| GitError::Failed(t!("Detached HEAD: switch to a branch before pushing").to_owned()))?;
            let remotes = repo.remotes()?;
            let remote = match remotes.as_slice() {
                [] => return Err(GitError::Failed(t!("No remote configured (git remote add origin <url>)").to_owned())),
                [one] => one.clone(),
                many if many.iter().any(|r| r == "origin") => "origin".into(),
                _ => {
                    return Err(GitError::Failed(
                        t!("Several remotes and no upstream: publish from the terminal (git push -u <remote> <branch>)").to_owned(),
                    ))
                }
            };
            run(repo.net(&["push", "-u", &remote, &branch]), None, NET_TIMEOUT)?
        };
        Ok(summary(&out))
    }

    /// Local and remote branches.
    pub fn git_branches(&self) -> Result<Vec<GitBranch>, GitError> {
        let repo = self.git_repo()?;
        let fmt = format!("--format={BRANCH_FORMAT}");
        let out = repo.read(&["for-each-ref", &fmt, "refs/heads", "refs/remotes"])?;
        Ok(parse_branches(&out.stdout))
    }

    /// Switches branch. A remote one (`origin/x`) becomes a local branch that tracks it; if the local one
    /// (`x`) already exists, it only switches to it.
    pub fn git_checkout(&self, branch: &GitBranch) -> Result<(), GitError> {
        if !valid_branch_name(&branch.name) {
            return Err(GitError::Failed(t!("Invalid branch name: {name}", name = branch.name)));
        }
        let repo = self.git_repo()?;
        let _op = self.inner.git.op.lock();
        if branch.remote {
            // Local name: what comes after the remote (the longest one that matches; remotes may have `/`).
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

    /// Creates a branch from HEAD (and switches to it, if `switch`).
    pub fn git_create_branch(&self, name: &str, switch: bool) -> Result<(), GitError> {
        if !valid_branch_name(name) {
            return Err(GitError::Failed(t!("Invalid branch name: {name}", name)));
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

    /// Lists the repository's stashes.
    pub fn git_stash_list(&self) -> Result<Vec<GitStash>, GitError> {
        let repo = self.git_repo()?;
        let fmt = format!("--format={STASH_FORMAT}");
        let out = repo.read(&["stash", "list", &fmt])?;
        Ok(parse_stashes(&out.stdout))
    }

    /// Saves the current changes to the stash (`include_untracked`: includes untracked files `-u`).
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
        // With no paths of our own: `stash -u` calls `git clean` inside, and with literal pathspecs
        // it would leave the untracked files in place after saving them.
        let mut cmd = repo.cmd(&args);
        cmd.env_remove("GIT_LITERAL_PATHSPECS");
        let out = run(cmd, None, WRITE_TIMEOUT)?;
        let text = summary(&out);
        if text.to_lowercase().contains("no local changes to save") {
            return Err(GitError::Failed(t!("No local change to save in the stash").to_owned()));
        }
        Ok(text)
    }

    /// Full diff of a stash (`stash@{index}`), including untracked files saved in it.
    pub fn git_stash_diff(&self, index: usize) -> Result<GitDiff, GitError> {
        let repo = self.git_repo()?;
        let spec = format!("stash@{{{index}}}");
        let out = repo.read(&["stash", "show", "-p", "--no-color", "--no-ext-diff", &spec])?;
        let mut raw = out.stdout;
        // When the stash was saved with `--include-untracked`, the untracked files stay in the
        // third parent (`stash@{n}^3`).
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

    /// Applies a stash without deleting it (`git stash apply stash@{index}`).
    pub fn git_stash_apply(&self, index: usize) -> Result<String, GitError> {
        let repo = self.git_repo()?;
        let _op = self.inner.git.op.lock();
        let spec = format!("stash@{{{index}}}");
        let out = repo.write(&["stash", "apply", "-q", &spec], None)?;
        Ok(summary(&out))
    }

    /// Applies a stash and removes it from the list (`git stash pop stash@{index}`).
    pub fn git_stash_pop(&self, index: usize) -> Result<String, GitError> {
        let repo = self.git_repo()?;
        let _op = self.inner.git.op.lock();
        let spec = format!("stash@{{{index}}}");
        let out = repo.write(&["stash", "pop", "-q", &spec], None)?;
        Ok(summary(&out))
    }

    /// Deletes a stash (`git stash drop stash@{index}`).
    pub fn git_stash_drop(&self, index: usize) -> Result<(), GitError> {
        let repo = self.git_repo()?;
        let _op = self.inner.git.op.lock();
        let spec = format!("stash@{{{index}}}");
        repo.write(&["stash", "drop", "-q", &spec], None)?;
        Ok(())
    }

    // ---------- History (log) ----------

    /// Lists commits of the current branch (paged by `skip` and `limit`). Returns `(commits, has_more)`.
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

    /// Detail of a commit: metadata, changed files and the whole diff.
    pub fn git_commit_detail(&self, hash: &str) -> Result<GitCommitDetail, GitError> {
        if hash.is_empty() || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(GitError::Failed(t!("Invalid commit hash").to_owned()));
        }
        let repo = self.git_repo()?;
        let meta_out = repo.read(&["show", "-s", "--format=%H%x00%h%x00%an%x00%ae%x00%cr%x00%B", hash])?;
        let meta_str = String::from_utf8_lossy(&meta_out.stdout);
        let fields: Vec<&str> = meta_str.splitn(6, '\0').collect();
        if fields.len() < 6 {
            return Err(GitError::Failed(t!("Could not read the commit data").to_owned()));
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

    // ---------- Merge and Rebase ----------

    /// Merges the branch `branch` into the current branch (`git merge --no-edit <branch>`).
    /// When it goes into conflict, returns `Ok` with a warning (the `Merging` state and the conflicts section
    /// appear in the status).
    pub fn git_merge(&self, branch: &str) -> Result<String, GitError> {
        if !valid_branch_name(branch) {
            return Err(GitError::Failed(t!("Invalid branch name: {name}", name = branch)));
        }
        let repo = self.git_repo()?;
        let _op = self.inner.git.op.lock();
        repo.check_gpg_ready()?;
        let out = run_captured_raw(repo.cmd(&["merge", "--no-edit", branch]), None, WRITE_TIMEOUT, None, None, MAX_OUT)
            .map_err(|e| GitError::Failed(t!("Failed to run git: {error}", error = e)))?;
        match out.exit {
            Exit::Code(Some(0)) => Ok(summary(&out)),
            Exit::TimedOut => Err(GitError::Timeout),
            _ if repo.state() == RepoState::Merging => {
                Ok(t!("Merge with conflicts: resolve the files in conflict and click Continue.").to_owned())
            }
            _ => Err(classify_error(&String::from_utf8_lossy(&out.stderr))),
        }
    }

    /// Rebase of the current branch onto `branch` (`git rebase <branch>`).
    /// When it stops on a conflict, returns `Ok` with a warning (the `Rebasing` state appears at the top).
    pub fn git_rebase(&self, branch: &str) -> Result<String, GitError> {
        if !valid_branch_name(branch) {
            return Err(GitError::Failed(t!("Invalid branch name: {name}", name = branch)));
        }
        let repo = self.git_repo()?;
        let _op = self.inner.git.op.lock();
        repo.check_gpg_ready()?;
        let out = run_captured_raw(repo.cmd(&["rebase", branch]), None, WRITE_TIMEOUT, None, None, MAX_OUT)
            .map_err(|e| GitError::Failed(t!("Failed to run git: {error}", error = e)))?;
        match out.exit {
            Exit::Code(Some(0)) => Ok(summary(&out)),
            Exit::TimedOut => Err(GitError::Timeout),
            _ if repo.state() == RepoState::Rebasing => {
                Ok(t!("Rebase paused on a conflict: resolve the conflicts and click Continue.").to_owned())
            }
            _ => Err(classify_error(&String::from_utf8_lossy(&out.stderr))),
        }
    }

    /// Continues the operation in progress (`merge --continue`, `rebase --continue`, `cherry-pick --continue`, `revert --continue`).
    pub fn git_continue_op(&self) -> Result<String, GitError> {
        let repo = self.git_repo()?;
        let _op = self.inner.git.op.lock();
        repo.check_gpg_ready()?;
        let sub = match repo.state() {
            RepoState::Merging => "merge",
            RepoState::Rebasing => "rebase",
            RepoState::CherryPicking => "cherry-pick",
            RepoState::Reverting => "revert",
            RepoState::Normal => return Err(GitError::Failed(t!("No operation in progress").to_owned())),
        };
        let out = run(repo.cmd(&[sub, "--continue"]), None, COMMIT_TIMEOUT)?;
        Ok(summary(&out))
    }

    /// Aborts the operation in progress (`merge --abort`, `rebase --abort`, `cherry-pick --abort`, `revert --abort`).
    pub fn git_abort_op(&self) -> Result<String, GitError> {
        let repo = self.git_repo()?;
        let _op = self.inner.git.op.lock();
        let sub = match repo.state() {
            RepoState::Merging => "merge",
            RepoState::Rebasing => "rebase",
            RepoState::CherryPicking => "cherry-pick",
            RepoState::Reverting => "revert",
            RepoState::Normal => return Err(GitError::Failed(t!("No operation in progress").to_owned())),
        };
        let out = repo.write(&[sub, "--abort"], None)?;
        Ok(summary(&out))
    }

    // ---------- Conflicts ----------

    /// Reads the conflict blocks (`<<<<<<<` … `>>>>>>>`) of a file in conflict.
    pub fn git_conflict_file(&self, path: &str) -> Result<GitConflictFile, GitError> {
        let repo = self.git_repo()?;
        if !safe_rel(path) {
            return Err(GitError::Failed(t!("Invalid path: {path}", path)));
        }
        let full = repo.root.join(path);
        if !full.is_file() {
            return Ok(GitConflictFile { path: path.to_owned(), blocks: Vec::new(), binary: false });
        }
        let (bytes, _cut) =
            read_capped(&full, CONFLICT_MAX_BYTES).map_err(|e| GitError::Failed(t!("Failed to read {path}: {error}", path, error = e)))?;
        if bytes.iter().take(8000).any(|b| *b == 0) {
            return Ok(GitConflictFile { path: path.to_owned(), blocks: Vec::new(), binary: true });
        }
        let text = String::from_utf8_lossy(&bytes);
        Ok(GitConflictFile { path: path.to_owned(), blocks: parse_conflict_blocks(&text), binary: false })
    }

    /// Resolves the whole file in conflict by choosing our side (`Ours` → `--ours`), theirs
    /// (`Theirs` → `--theirs`) or both (`Both`, resolving every block of the file), and already
    /// stages it (`git add`) as resolved.
    pub fn git_resolve_conflict_file(&self, path: &str, choice: ConflictChoice) -> Result<(), GitError> {
        let repo = self.git_repo()?;
        if !safe_rel(path) {
            return Err(GitError::Failed(t!("Invalid path: {path}", path)));
        }
        let _op = self.inner.git.op.lock();
        match choice {
            ConflictChoice::Ours | ConflictChoice::Theirs => {
                let side = if choice == ConflictChoice::Ours { "--ours" } else { "--theirs" };
                // If the chosen side exists in the index (stage 2 = ours, 3 = theirs), `checkout <side>`
                // restores it; if the file was deleted on that side (e.g. a modify/delete conflict),
                // `git rm` applies the removal.
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
                let text =
                    std::fs::read_to_string(&full).map_err(|e| GitError::Failed(t!("Failed to read {path}: {error}", path, error = e)))?;
                let mut cur = text;
                while let Some(next) = resolve_conflict_block_in_text(&cur, 0, ConflictChoice::Both) {
                    cur = next;
                }
                write_atomic(&full, cur.as_bytes())
                    .map_err(|e| GitError::Failed(t!("Failed to write {path}: {error}", path, error = e)))?;
                repo.write(&["add", "--", path], None)?;
            }
        }
        Ok(())
    }

    /// Resolves a single block (`block_index`, 0-indexed) of a file in conflict.
    pub fn git_resolve_conflict_block(&self, path: &str, block_index: usize, choice: ConflictChoice) -> Result<(), GitError> {
        let repo = self.git_repo()?;
        if !safe_rel(path) {
            return Err(GitError::Failed(t!("Invalid path: {path}", path)));
        }
        let _op = self.inner.git.op.lock();
        let full = repo.root.join(path);
        let text = std::fs::read_to_string(&full).map_err(|e| GitError::Failed(t!("Failed to read {path}: {error}", path, error = e)))?;
        let new_text = resolve_conflict_block_in_text(&text, block_index, choice)
            .ok_or_else(|| GitError::Failed(t!("Conflict block not found (the file changed)").to_owned()))?;
        write_atomic(&full, new_text.as_bytes()).map_err(|e| GitError::Failed(t!("Failed to write {path}: {error}", path, error = e)))?;
        Ok(())
    }

    /// `git init` in the workspace (never in the user's home folder).
    pub fn git_init(&self) -> Result<(), GitError> {
        let git = self.git_exe().ok_or(GitError::NotInstalled)?;
        let ws = self.workspace();
        if ws.as_os_str().is_empty() {
            return Err(GitError::Failed(t!("No project open").to_owned()));
        }
        if crate::util::same_path(&crate::util::absolutize(&ws), &crate::util::absolutize(&self.inner.home)) {
            return Err(GitError::Failed(t!("I do not create a repository in your home folder; choose a project folder").to_owned()));
        }
        run(base_command(&git, &ws, &["init", "-q"]), None, WRITE_TIMEOUT)?;
        Ok(())
    }
}

/// Writes `bytes` to `path` atomically through a temporary file in the same folder.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("tmp");
    let tmp = parent.join(format!(".{name}.agentdeck-tmp"));
    std::fs::write(&tmp, bytes)?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        // On Windows, `rename` over an existing file sometimes requires removing it or trying again.
        let _ = std::fs::write(path, bytes);
        let _ = std::fs::remove_file(&tmp);
        if !path.exists() {
            return Err(e);
        }
    }
    Ok(())
}

/// Reads at most `max` bytes of the file. Returns (bytes, cut).
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
        // A combined diff does not produce an individual hunk patch.
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
        let st = parse_stashes(b"stash@{0}\x002 hours ago\0On main: rascunho\nstash@{1}\x003 days ago\0WIP on feat: x\n");
        assert_eq!(
            st,
            [
                GitStash { index: 0, date: "2 hours ago".into(), message: "On main: rascunho".into() },
                GitStash { index: 1, date: "3 days ago".into(), message: "WIP on feat: x".into() },
            ]
        );
        let log_bytes = b"aaaa1111\0aaaa\0Ana\x002 hours ago\0feat: x\0\nbbbb2222\0bbbb\0Bia\0yesterday\0fix: y\0";
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
        // Relative to the drive: `C:..\x` would leave the repository root.
        if cfg!(windows) {
            for bad in ["C:..\\x", "C:x", "C:\\x", "a:b"] {
                assert!(!safe_rel(bad), "{bad:?}");
            }
        }
    }

    #[test]
    fn errors_are_explained_in_the_current_language() {
        assert!(GitError::NotInstalled.to_string().contains("git-scm.com"));
        assert!(GitError::Unsafe { path: "/x".into() }.to_string().contains("safe.directory \"/x\""));
        let pt = crate::i18n::with_lang(crate::i18n::Lang::PtBr, || GitError::NotARepo.to_string());
        assert_eq!(pt, "A pasta de trabalho não é um repositório git.");
    }
}
