//! Estado e ações da aba "Git" da barra direita (a pintura fica em `ui/git.rs`).
//!
//! Toda operação do git é bloqueante e roda numa thread (`Deck::spawn_bg`); a resposta volta como
//! `UiMsg::Git`. O status é relido quando a aba abre, depois de cada operação, quando o workspace ou
//! o `.git/` mudam (com um pequeno atraso para juntar eventos) e quando a janela volta a ter foco.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use commanddeck_core::git::{DiffKind, DiffLine, EntryKind, GitBranch, GitDiff, GitEntry, GitError, GitStatus};
use commanddeck_core::Core;

use crate::deck::{Deck, RightView, UiMsg};

/// Atraso para reler o status depois de um aviso de mudança (junta rajadas de eventos).
const REFRESH_DELAY: Duration = Duration::from_millis(400);
/// Repositório acima do workspace (o `.git` não é vigiado): relê assim, com a aba aberta.
const POLL_EVERY: Duration = Duration::from_secs(10);

/// O que a aba mostra.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitView {
    Loading,
    NoGit,
    NotRepo,
    /// O git recusa a pasta (`safe.directory`): o caminho a liberar.
    Unsafe(String),
    Error(String),
    Ready(GitStatus),
}

/// Diff aberto no lugar da lista.
#[derive(Debug, Clone)]
pub struct OpenDiff {
    pub entry: GitEntry,
    /// Lado preparado (índice × HEAD) ou não (cópia de trabalho × índice).
    pub staged: bool,
    /// `None` = carregando.
    pub diff: Option<Result<GitDiff, String>>,
}

/// Respostas das operações em segundo plano.
pub enum GitReply {
    Status(Result<GitStatus, GitError>),
    Diff {
        entry: GitEntry,
        staged: bool,
        result: Result<GitDiff, GitError>,
    },
    Branches(Result<Vec<GitBranch>, GitError>),
    LastMessage(Option<String>),
    /// Operação concluída (`label` = o que foi feito, para a mensagem).
    Done {
        label: &'static str,
        result: Result<String, GitError>,
    },
}

/// Seções da lista de alterações.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Conflicts,
    Staged,
    Changes,
    Untracked,
}

impl Section {
    pub fn title(self) -> &'static str {
        match self {
            Section::Conflicts => "CONFLITOS",
            Section::Staged => "ALTERAÇÕES PREPARADAS",
            Section::Changes => "ALTERAÇÕES",
            Section::Untracked => "NÃO RASTREADOS",
        }
    }
}

/// Uma linha da lista: o arquivo, de que lado (preparado ou não) e a letra a mostrar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub entry: GitEntry,
    pub staged: bool,
    pub letter: char,
}

/// Agrupa o status nas seções, na ordem em que aparecem (seções vazias ficam de fora). Um arquivo
/// com alteração preparada e outra não aparece nas duas.
pub fn sections(st: &GitStatus) -> Vec<(Section, Vec<Row>)> {
    let mut out: Vec<(Section, Vec<Row>)> = Vec::new();
    let mut push = |s: Section, row: Row| match out.iter_mut().find(|(k, _)| *k == s) {
        Some((_, rows)) => rows.push(row),
        None => out.push((s, vec![row])),
    };
    let order = [Section::Conflicts, Section::Staged, Section::Changes, Section::Untracked];
    for e in &st.entries {
        match e.kind {
            EntryKind::Unmerged => push(Section::Conflicts, Row { entry: e.clone(), staged: false, letter: 'U' }),
            EntryKind::Untracked => push(Section::Untracked, Row { entry: e.clone(), staged: false, letter: 'U' }),
            _ => {
                if e.staged() {
                    push(Section::Staged, Row { entry: e.clone(), staged: true, letter: e.index });
                }
                if e.unstaged() {
                    push(Section::Changes, Row { entry: e.clone(), staged: false, letter: e.worktree });
                }
            }
        }
    }
    out.sort_by_key(|(s, _)| order.iter().position(|o| o == s));
    out
}

/// Texto resumido da branch e alterações: `main ↑1 ↓2 •3` (só o que não for zero).
pub fn chip_text(st: &GitStatus) -> String {
    let mut t = st
        .branch
        .clone()
        .unwrap_or_else(|| st.head.as_deref().map(|h| h.chars().take(7).collect()).unwrap_or_else(|| "(sem commits)".into()));
    if st.ahead > 0 {
        t += &format!(" ↑{}", st.ahead);
    }
    if st.behind > 0 {
        t += &format!(" ↓{}", st.behind);
    }
    if !st.entries.is_empty() {
        t += &format!(" •{}", st.entries.len());
    }
    t
}

pub struct GitUi {
    pub view: GitView,
    /// Operação em andamento (ex.: "Enviando…"): os botões ficam desativados.
    pub busy: Option<&'static str>,
    status_running: bool,
    status_dirty: bool,
    refresh_at: Option<Instant>,
    last_poll: Instant,
    pub diff: Option<OpenDiff>,
    pub message: String,
    pub amend: bool,
    pub branch_menu: bool,
    pub branches: Vec<GitBranch>,
    pub branch_filter: String,
    /// Arquivo cujo descarte espera confirmação.
    pub confirm: Option<GitEntry>,
    /// Mensagem da última operação (texto, é erro?).
    pub notice: Option<(String, bool)>,
    pub collapsed: HashSet<Section>,
    /// SOMENTE DEPURAÇÃO: estado fictício que as releituras não substituem.
    pub debug_hold: bool,
}

impl Default for GitUi {
    fn default() -> Self {
        Self {
            view: GitView::Loading,
            busy: None,
            status_running: false,
            status_dirty: false,
            refresh_at: None,
            last_poll: Instant::now(),
            diff: None,
            message: String::new(),
            amend: false,
            branch_menu: false,
            branches: Vec::new(),
            branch_filter: String::new(),
            confirm: None,
            notice: None,
            collapsed: HashSet::new(),
            debug_hold: false,
        }
    }
}

impl GitUi {
    pub fn status(&self) -> Option<&GitStatus> {
        match &self.view {
            GitView::Ready(st) => Some(st),
            _ => None,
        }
    }
}

fn err_text(e: &GitError) -> String {
    e.to_string()
}

impl Deck {
    /// Relê o status (um por vez; um pedido no meio repete a leitura ao terminar).
    pub fn git_refresh(&mut self) {
        if self.git.debug_hold {
            return;
        }
        if self.git.status_running {
            self.git.status_dirty = true;
            return;
        }
        self.git.status_running = true;
        self.git.last_poll = Instant::now();
        self.spawn_bg("git-status", |core| Some(UiMsg::Git(GitReply::Status(core.git_status()))));
    }

    /// Agenda uma releitura (avisos de mudança chegam em rajadas).
    pub fn git_refresh_soon(&mut self) {
        self.git.refresh_at = Some(Instant::now() + REFRESH_DELAY);
    }

    /// Chamado a cada quadro: releituras agendadas e o polling (só quando o `.git` não é vigiado).
    /// Devolve quando precisa ser chamado de novo.
    pub fn git_tick(&mut self, now: Instant) -> Option<Instant> {
        if let Some(at) = self.git.refresh_at {
            if now >= at {
                self.git.refresh_at = None;
                self.git_refresh();
            } else {
                return Some(at);
            }
        }
        let outside = self.git.status().is_some_and(|st| !st.prefix.is_empty());
        if self.right_view == RightView::Git && self.window_focused && outside {
            let at = self.git.last_poll + POLL_EVERY;
            if now >= at {
                self.git_refresh();
            }
            return Some(at.max(now + Duration::from_millis(500)));
        }
        None
    }

    pub fn on_git(&mut self, reply: GitReply) {
        match reply {
            GitReply::Status(res) => {
                self.git.status_running = false;
                if self.git.debug_hold {
                    return;
                }
                self.git.view = match res {
                    Ok(st) => GitView::Ready(st),
                    Err(GitError::NotInstalled) => GitView::NoGit,
                    Err(GitError::NotARepo) => GitView::NotRepo,
                    Err(GitError::Unsafe { path }) => GitView::Unsafe(path),
                    Err(e) => GitView::Error(err_text(&e)),
                };
                // O diff aberto acompanha o status (ou fecha, se o arquivo saiu daquele lado).
                if let (Some(open), Some(st)) = (self.git.diff.clone(), self.git.status()) {
                    let still = st.entries.iter().find(|e| e.path == open.entry.path).cloned().filter(|e| {
                        if open.staged {
                            e.staged()
                        } else {
                            e.unstaged() || matches!(e.kind, EntryKind::Untracked | EntryKind::Unmerged)
                        }
                    });
                    match still {
                        Some(e) => self.git_load_diff(e, open.staged),
                        None => self.git.diff = None,
                    }
                }
                if std::mem::take(&mut self.git.status_dirty) {
                    self.git_refresh();
                }
            }
            GitReply::Diff { entry, staged, result } => {
                if let Some(open) = &mut self.git.diff {
                    if open.entry.path == entry.path && open.staged == staged {
                        open.entry = entry;
                        open.diff = Some(result.map_err(|e| err_text(&e)));
                    }
                }
            }
            GitReply::Branches(res) => match res {
                Ok(list) => self.git.branches = list,
                Err(e) => self.git.notice = Some((err_text(&e), true)),
            },
            GitReply::LastMessage(msg) => {
                if self.git.message.trim().is_empty() {
                    self.git.message = msg.unwrap_or_default();
                }
            }
            GitReply::Done { label, result } => {
                self.git.busy = None;
                match result {
                    Ok(text) => {
                        let text =
                            if text.trim().is_empty() { format!("{label}: concluído.") } else { format!("{label}: {}", text.trim()) };
                        self.git.notice = Some((text, false));
                    }
                    Err(e) => self.git.notice = Some((err_text(&e), true)),
                }
                self.git_refresh();
                self.refresh_tree();
            }
        }
    }

    /// Roda uma operação em segundo plano (uma por vez; a interface desativa os botões).
    fn git_run(&mut self, label: &'static str, busy: &'static str, op: impl FnOnce(Core) -> Result<String, GitError> + Send + 'static) {
        if self.git.busy.is_some() {
            return;
        }
        self.git.busy = Some(busy);
        self.git.notice = None;
        self.spawn_bg("git-op", move |core| Some(UiMsg::Git(GitReply::Done { label, result: op(core) })));
    }

    pub fn git_open_diff(&mut self, entry: GitEntry, staged: bool) {
        self.git.diff = Some(OpenDiff { entry: entry.clone(), staged, diff: None });
        self.git_load_diff(entry, staged);
    }

    fn git_load_diff(&mut self, entry: GitEntry, staged: bool) {
        self.spawn_bg("git-diff", move |core| {
            let result = core.git_diff(&entry, staged);
            Some(UiMsg::Git(GitReply::Diff { entry, staged, result }))
        });
    }

    pub fn git_stage(&mut self, paths: Vec<String>) {
        self.git_run("Preparado", "Preparando…", move |core| core.git_stage(&paths).map(|_| String::new()));
    }

    pub fn git_unstage(&mut self, paths: Vec<String>) {
        self.git_run("Removido da preparação", "Removendo da preparação…", move |core| {
            core.git_unstage(&paths).map(|_| String::new())
        });
    }

    pub fn git_stage_all(&mut self) {
        self.git_run("Tudo preparado", "Preparando…", |core| core.git_stage_all().map(|_| String::new()));
    }

    pub fn git_unstage_all(&mut self) {
        self.git_run("Preparação desfeita", "Removendo da preparação…", |core| core.git_unstage_all().map(|_| String::new()));
    }

    /// Descarta de fato (depois da confirmação).
    pub fn git_discard_confirmed(&mut self) {
        let Some(entry) = self.git.confirm.take() else { return };
        self.git_run("Descartado", "Descartando…", move |core| core.git_discard(&entry).map(|_| String::new()));
    }

    pub fn git_commit(&mut self) {
        let msg = self.git.message.clone();
        let amend = self.git.amend;
        if msg.trim().is_empty() || self.git.busy.is_some() {
            return;
        }
        self.git.message.clear();
        self.git.amend = false;
        self.git_run("Commit", "Fazendo commit…", move |core| core.git_commit(&msg, amend));
    }

    /// Marca/desmarca "corrigir último commit" (marcando com a caixa vazia, traz a última mensagem).
    pub fn git_toggle_amend(&mut self) {
        self.git.amend = !self.git.amend;
        if self.git.amend && self.git.message.trim().is_empty() {
            self.spawn_bg("git-last-msg", |core| Some(UiMsg::Git(GitReply::LastMessage(core.git_last_message()))));
        }
    }

    pub fn git_fetch(&mut self) {
        self.git_run("Buscar", "Buscando…", |core| core.git_fetch());
    }

    pub fn git_pull(&mut self) {
        self.git_run("Puxar", "Puxando…", |core| core.git_pull());
    }

    pub fn git_push(&mut self) {
        self.git_run("Enviar", "Enviando…", |core| core.git_push());
    }

    pub fn git_init(&mut self) {
        self.git_run("Repositório criado", "Inicializando…", |core| core.git_init().map(|_| String::new()));
    }

    pub fn git_open_branch_menu(&mut self) {
        self.git.branch_menu = !self.git.branch_menu;
        self.git.branch_filter.clear();
        if self.git.branch_menu && !self.git.debug_hold {
            self.spawn_bg("git-branches", |core| Some(UiMsg::Git(GitReply::Branches(core.git_branches()))));
        }
    }

    pub fn git_checkout(&mut self, branch: GitBranch) {
        self.git.branch_menu = false;
        self.git.diff = None;
        self.git_run("Branch trocada", "Trocando de branch…", move |core| core.git_checkout(&branch).map(|_| String::new()));
    }

    pub fn git_create_branch(&mut self, name: String) {
        self.git.branch_menu = false;
        self.git_run("Branch criada", "Criando branch…", move |core| core.git_create_branch(&name, true).map(|_| String::new()));
    }

    /// Abre o arquivo (caminho relativo à raiz do repositório) no programa do sistema.
    pub fn git_open_file(&mut self, rel: &str) {
        let Some(root) = self.git.status().map(|st| st.root.clone()) else { return };
        let full = root.join(rel);
        if let Err(e) = self.core.open_path(&full.to_string_lossy()) {
            self.git.notice = Some((e.to_string(), true));
        }
    }
}

/// SOMENTE DEPURAÇÃO (`COMMANDDECK_DEBUG_UI=git-demo`): status fictício para capturas de tela.
pub fn demo_status() -> GitStatus {
    let e =
        |path: &str, index: char, worktree: char, kind: EntryKind| GitEntry { path: path.into(), orig_path: None, index, worktree, kind };
    GitStatus {
        root: std::path::PathBuf::from("/projetos/commanddeck"),
        prefix: String::new(),
        branch: Some("feat/barra-direita".into()),
        head: Some("b1457ef".into()),
        upstream: Some("origin/feat/barra-direita".into()),
        ahead: 2,
        behind: 1,
        state: Default::default(),
        entries: vec![
            e("crates/app/src/ui/git.rs", 'A', '.', EntryKind::Ordinary),
            e("crates/core/src/git.rs", 'M', 'M', EntryKind::Ordinary),
            e("memory/GERAL.md", '.', 'M', EntryKind::Ordinary),
            e("README.md", '.', 'D', EntryKind::Ordinary),
            e("docs/rascunho de ideias.md", '?', '?', EntryKind::Untracked),
        ],
        truncated: false,
    }
}

/// SOMENTE DEPURAÇÃO: diff fictício.
pub fn demo_diff() -> GitDiff {
    let l = |kind, old, new, text: &str| DiffLine { kind, old, new, text: text.into() };
    GitDiff {
        lines: vec![
            l(DiffKind::Meta, None, None, "diff --git a/memory/GERAL.md b/memory/GERAL.md"),
            l(DiffKind::Hunk, None, None, "@@ -20,4 +20,6 @@ ## 1. Ambientação"),
            l(DiffKind::Context, Some(20), Some(20), " Terminal desktop nativo em Rust (winit + egui)."),
            l(DiffKind::Del, Some(21), None, "-A Conversa integrada fica embaixo dos terminais."),
            l(DiffKind::Add, None, Some(21), "+A Conversa integrada fica na barra direita, com o Mapa"),
            l(DiffKind::Add, None, Some(22), "+e a aba Git (branch, alterações, diff e commit)."),
            l(DiffKind::Context, Some(22), Some(23), " "),
            l(DiffKind::Context, Some(23), Some(24), " ### Pastas"),
        ],
        truncated: false,
        binary: false,
    }
}

/// SOMENTE DEPURAÇÃO: branches fictícias.
pub fn demo_branches() -> Vec<GitBranch> {
    let b = |name: &str, remote: bool, current: bool| GitBranch { name: name.into(), remote, upstream: None, current };
    vec![
        b("feat/barra-direita", false, true),
        b("main", false, false),
        b("dev", false, false),
        b("origin/main", true, false),
        b("origin/dev", true, false),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_go_to_their_sections_in_order() {
        let st = demo_status();
        let s = sections(&st);
        type Summary<'a> = Vec<(Section, Vec<(&'a str, bool, char)>)>;
        let summary: Summary =
            s.iter().map(|(k, rows)| (*k, rows.iter().map(|r| (r.entry.path.as_str(), r.staged, r.letter)).collect())).collect();
        assert_eq!(
            summary,
            [
                (Section::Staged, vec![("crates/app/src/ui/git.rs", true, 'A'), ("crates/core/src/git.rs", true, 'M')]),
                (
                    Section::Changes,
                    vec![("crates/core/src/git.rs", false, 'M'), ("memory/GERAL.md", false, 'M'), ("README.md", false, 'D')]
                ),
                (Section::Untracked, vec![("docs/rascunho de ideias.md", false, 'U')]),
            ]
        );
        let conflict = GitStatus {
            entries: vec![GitEntry { path: "x".into(), orig_path: None, index: 'U', worktree: 'U', kind: EntryKind::Unmerged }],
            ..Default::default()
        };
        assert_eq!(sections(&conflict)[0].0, Section::Conflicts);
    }

    #[test]
    fn the_chip_shows_only_what_matters() {
        assert_eq!(chip_text(&demo_status()), "feat/barra-direita ↑2 ↓1 •5");
        let clean = GitStatus { branch: Some("main".into()), head: Some("abc1234567".into()), ..Default::default() };
        assert_eq!(chip_text(&clean), "main");
        let detached = GitStatus { head: Some("abc1234567".into()), ..Default::default() };
        assert_eq!(chip_text(&detached), "abc1234");
        assert_eq!(chip_text(&GitStatus::default()), "(sem commits)");
    }
}
