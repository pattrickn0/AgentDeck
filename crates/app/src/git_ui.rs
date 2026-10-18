//! Estado e ações da aba "Git" da barra direita (a pintura fica em `ui/git.rs`).
//!
//! Toda operação do git é bloqueante e roda numa thread (`Deck::spawn_bg`); a resposta volta como
//! `UiMsg::Git`. O status é relido quando a aba abre, depois de cada operação, quando o workspace ou
//! o `.git/` mudam (com um pequeno atraso para juntar eventos) e quando a janela volta a ter foco.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use commanddeck_core::git::{
    extract_hunk_patch, ConflictBlock, ConflictChoice, DiffKind, DiffLine, EntryKind, GitBranch, GitCommit, GitCommitDetail,
    GitConflictFile, GitDiff, GitEntry, GitError, GitStash, GitStatus, RepoState,
};
use commanddeck_core::Core;

use crate::deck::{Deck, RightView, UiMsg};

/// Atraso para reler o status depois de um aviso de mudança (junta rajadas de eventos).
const REFRESH_DELAY: Duration = Duration::from_millis(400);
/// Repositório acima do workspace (o `.git` não é vigiado): relê assim, com a aba aberta.
const POLL_EVERY: Duration = Duration::from_secs(10);
/// Tamanho da página do histórico de commits (`git log`).
pub const LOG_PAGE_SIZE: usize = 50;

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

/// Visão interna da aba Git.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GitSubView {
    #[default]
    Changes,
    History,
    Stash,
    Conflicts,
}

/// Modo de escolha de branch no menu do topo.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BranchMenuMode {
    #[default]
    Switch,
    Merge,
    Rebase,
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

/// Diff de um stash aberto no lugar da lista de stashes.
#[derive(Debug, Clone)]
pub struct OpenStashDiff {
    pub stash: GitStash,
    pub diff: Option<Result<GitDiff, String>>,
}

/// Detalhe de um commit aberto no lugar da lista do histórico.
#[derive(Debug, Clone)]
pub struct OpenCommit {
    pub hash: String,
    pub short_hash: String,
    pub subject: String,
    pub detail: Option<Result<GitCommitDetail, String>>,
}

/// Arquivo em conflito aberto para resolução por blocos ou lado inteiro.
#[derive(Debug, Clone)]
pub struct OpenConflict {
    pub path: String,
    pub file: Option<Result<GitConflictFile, String>>,
}

/// Ação destrutiva aguardando confirmação no modal.
#[derive(Debug, Clone)]
pub enum ConfirmAction {
    /// Descartar arquivo inteiro.
    DiscardFile(GitEntry),
    /// Descartar um único trecho (hunk) da cópia de trabalho.
    DiscardHunk { path: String, patch: String },
    /// Apagar um stash (`stash@{index}`).
    DropStash(GitStash),
    /// Abortar merge/rebase/cherry-pick/revert em andamento.
    AbortOp(RepoState),
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
    Stashes(Result<Vec<GitStash>, GitError>),
    StashDiff {
        index: usize,
        result: Result<GitDiff, GitError>,
    },
    Log {
        append: bool,
        result: Result<(Vec<GitCommit>, bool), GitError>,
    },
    CommitDetail {
        hash: String,
        result: Result<GitCommitDetail, GitError>,
    },
    ConflictFile {
        path: String,
        result: Result<GitConflictFile, GitError>,
    },
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

/// Quantidade de arquivos em conflito (`Unmerged`) no status.
pub fn conflict_count(st: &GitStatus) -> usize {
    st.entries.iter().filter(|e| e.kind == EntryKind::Unmerged).count()
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
    pub subview: GitSubView,
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
    pub branch_menu_mode: BranchMenuMode,
    pub branches: Vec<GitBranch>,
    pub branch_filter: String,
    /// Ação destrutiva esperando confirmação.
    pub confirm: Option<ConfirmAction>,
    /// Mensagem da última operação (texto, é erro?).
    pub notice: Option<(String, bool)>,
    pub collapsed: HashSet<Section>,
    /// Estado da visão Stash.
    pub stashes: Vec<GitStash>,
    pub stash_loading: bool,
    pub stash_message: String,
    pub stash_include_untracked: bool,
    pub stash_diff: Option<OpenStashDiff>,
    /// Estado da visão Histórico (log).
    pub log: Vec<GitCommit>,
    pub log_has_more: bool,
    pub log_loading: bool,
    /// HEAD em que `log` foi lido: se o HEAD mudar, a lista é relida.
    pub log_head: Option<String>,
    pub commit_detail: Option<OpenCommit>,
    /// Estado da visão Conflitos.
    pub conflict_open: Option<OpenConflict>,
    /// SOMENTE DEPURAÇÃO: estado fictício que as releituras não substituem.
    pub debug_hold: bool,
    /// Muda a cada troca de projeto: respostas de operações pedidas no projeto anterior chegam
    /// com a geração velha e são descartadas (não podem aparecer nem agir no projeto novo).
    pub generation: u64,
}

impl Default for GitUi {
    fn default() -> Self {
        Self {
            view: GitView::Loading,
            subview: GitSubView::Changes,
            busy: None,
            status_running: false,
            status_dirty: false,
            refresh_at: None,
            last_poll: Instant::now(),
            diff: None,
            message: String::new(),
            amend: false,
            branch_menu: false,
            branch_menu_mode: BranchMenuMode::Switch,
            branches: Vec::new(),
            branch_filter: String::new(),
            confirm: None,
            notice: None,
            collapsed: HashSet::new(),
            stashes: Vec::new(),
            stash_loading: false,
            stash_message: String::new(),
            stash_include_untracked: true,
            stash_diff: None,
            log: Vec::new(),
            log_has_more: false,
            log_loading: false,
            log_head: None,
            commit_detail: None,
            conflict_open: None,
            generation: 0,
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
    /// O projeto da tela mudou: esquece tudo do anterior (confirmação de descarte, branches,
    /// mensagem, operação em andamento) e ignora as respostas que ainda vierem dele.
    pub fn git_reset_for_project(&mut self) {
        let generation = self.git.generation + 1;
        let collapsed = std::mem::take(&mut self.git.collapsed);
        self.git = GitUi { generation, collapsed, ..GitUi::default() };
    }

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
        let generation = self.git.generation;
        self.spawn_bg("git-status", move |core| Some(UiMsg::Git(generation, GitReply::Status(core.git_status()))));
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

    pub fn on_git(&mut self, generation: u64, reply: GitReply) {
        if generation != self.git.generation {
            return; // pedida no projeto anterior
        }
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
                // Se estava na aba Conflitos e todos os conflitos acabaram, volta para Alterações.
                if let Some(st) = self.git.status() {
                    if self.git.subview == GitSubView::Conflicts && conflict_count(st) == 0 && st.state == RepoState::Normal {
                        self.git.subview = GitSubView::Changes;
                        self.git.conflict_open = None;
                    }
                }
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
                // Se o editor de conflitos está aberto num arquivo, recarrega (ou fecha se já foi resolvido).
                if let (Some(open), Some(st)) = (self.git.conflict_open.clone(), self.git.status()) {
                    let still = st.entries.iter().any(|e| e.path == open.path && e.kind == EntryKind::Unmerged);
                    if still {
                        self.git_load_conflict_file(open.path);
                    } else {
                        self.git.conflict_open = None;
                    }
                }
                // Histórico e stash só são relidos quando mudaram (HEAD novo; outra quantidade de stashes).
                if let Some(st) = self.git.status() {
                    let (head, stashes) = (st.head.clone(), st.stash_count as usize);
                    if !self.git.log.is_empty() && self.git.log_head != head {
                        self.git.log.clear();
                        self.git.log_has_more = false;
                        self.git.commit_detail = None;
                    }
                    match self.git.subview {
                        GitSubView::History if self.git.log.is_empty() => self.git_load_log(false),
                        GitSubView::Stash if stashes != self.git.stashes.len() => self.git_load_stashes(),
                        _ => {}
                    }
                    if stashes == 0 {
                        self.git.stashes.clear();
                        self.git.stash_diff = None;
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
            GitReply::Stashes(res) => {
                self.git.stash_loading = false;
                match res {
                    Ok(list) => self.git.stashes = list,
                    Err(e) => self.git.notice = Some((err_text(&e), true)),
                }
            }
            GitReply::StashDiff { index, result } => {
                if let Some(open) = &mut self.git.stash_diff {
                    if open.stash.index == index {
                        open.diff = Some(result.map_err(|e| err_text(&e)));
                    }
                }
            }
            GitReply::Log { append, result } => {
                self.git.log_loading = false;
                match result {
                    Ok((commits, has_more)) => {
                        if !append {
                            self.git.log_head = self.git.status().and_then(|st| st.head.clone());
                        }
                        if append {
                            self.git.log.extend(commits);
                        } else {
                            self.git.log = commits;
                        }
                        self.git.log_has_more = has_more;
                    }
                    Err(e) => self.git.notice = Some((err_text(&e), true)),
                }
            }
            GitReply::CommitDetail { hash, result } => {
                if let Some(open) = &mut self.git.commit_detail {
                    if open.hash == hash {
                        open.detail = Some(result.map_err(|e| err_text(&e)));
                    }
                }
            }
            GitReply::ConflictFile { path, result } => {
                if let Some(open) = &mut self.git.conflict_open {
                    if open.path == path {
                        open.file = Some(result.map_err(|e| err_text(&e)));
                    }
                }
            }
            GitReply::Done { label, result } => {
                self.git.busy = None;
                match result {
                    Ok(text) => {
                        // A mensagem do commit só sai da caixa quando ele deu certo.
                        if label == "Commit" {
                            self.git.message.clear();
                            self.git.amend = false;
                        }
                        if label == "Stash salvo" {
                            self.git.stash_message.clear();
                        }
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

    /// Troca a subvisão da aba Git e carrega os dados correspondentes.
    pub fn git_set_subview(&mut self, subview: GitSubView) {
        self.git.subview = subview;
        if self.git.debug_hold {
            return;
        }
        match subview {
            GitSubView::History => {
                if self.git.log.is_empty() {
                    self.git_load_log(false);
                }
            }
            GitSubView::Stash => {
                self.git_load_stashes();
            }
            _ => {}
        }
    }

    /// Roda uma operação em segundo plano (uma por vez; a interface desativa os botões).
    fn git_run(&mut self, label: &'static str, busy: &'static str, op: impl FnOnce(Core) -> Result<String, GitError> + Send + 'static) {
        if self.git.busy.is_some() {
            return;
        }
        self.git.busy = Some(busy);
        self.git.notice = None;
        let generation = self.git.generation;
        self.spawn_bg("git-op", move |core| Some(UiMsg::Git(generation, GitReply::Done { label, result: op(core) })));
    }

    pub fn git_open_diff(&mut self, entry: GitEntry, staged: bool) {
        self.git.diff = Some(OpenDiff { entry: entry.clone(), staged, diff: None });
        self.git_load_diff(entry, staged);
    }

    fn git_load_diff(&mut self, entry: GitEntry, staged: bool) {
        let generation = self.git.generation;
        self.spawn_bg("git-diff", move |core| {
            let result = core.git_diff(&entry, staged);
            Some(UiMsg::Git(generation, GitReply::Diff { entry, staged, result }))
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

    pub fn git_unstage_all(&mut self) {
        self.git_run("Preparação desfeita", "Removendo da preparação…", |core| core.git_unstage_all().map(|_| String::new()));
    }

    /// Prepara ou tira da preparação um único trecho (`hunk_index`) do diff aberto.
    pub fn git_stage_or_unstage_hunk(&mut self, hunk_index: usize) {
        let Some(open) = &self.git.diff else { return };
        let Some(Ok(diff)) = &open.diff else { return };
        let Some(patch) = extract_hunk_patch(diff, hunk_index) else { return };
        let staged = open.staged;
        let (label, busy) =
            if staged { ("Trecho removido da preparação", "Removendo trecho…") } else { ("Trecho preparado", "Preparando trecho…") };
        self.git_run(label, busy, move |core| core.git_apply_hunk(&patch, staged, false).map(|_| String::new()));
    }

    /// Pede confirmação para descartar um trecho (`hunk_index`) do diff aberto.
    pub fn git_ask_discard_hunk(&mut self, hunk_index: usize) {
        let Some(open) = &self.git.diff else { return };
        if open.staged {
            return;
        }
        let Some(Ok(diff)) = &open.diff else { return };
        let Some(patch) = extract_hunk_patch(diff, hunk_index) else { return };
        self.git.confirm = Some(ConfirmAction::DiscardHunk { path: open.entry.path.clone(), patch });
    }

    /// Executa a ação destrutiva depois da confirmação.
    pub fn git_confirm_accepted(&mut self) {
        let Some(action) = self.git.confirm.take() else { return };
        match action {
            ConfirmAction::DiscardFile(entry) => {
                self.git_run("Descartado", "Descartando…", move |core| core.git_discard(&entry).map(|_| String::new()));
            }
            ConfirmAction::DiscardHunk { patch, .. } => {
                self.git_run("Trecho descartado", "Descartando trecho…", move |core| {
                    core.git_apply_hunk(&patch, false, true).map(|_| String::new())
                });
            }
            ConfirmAction::DropStash(stash) => {
                if self.git.stash_diff.as_ref().is_some_and(|d| d.stash.index == stash.index) {
                    self.git.stash_diff = None;
                }
                let index = stash.index;
                self.git_run("Stash apagado", "Apagando stash…", move |core| core.git_stash_drop(index).map(|_| String::new()));
            }
            ConfirmAction::AbortOp(_) => {
                self.git_run("Operação abortada", "Abortando…", |core| core.git_abort_op());
            }
        }
    }

    pub fn git_commit(&mut self) {
        let msg = self.git.message.clone();
        let amend = self.git.amend;
        if msg.trim().is_empty() || self.git.busy.is_some() {
            return;
        }
        self.git_run("Commit", "Fazendo commit…", move |core| core.git_commit(&msg, amend));
    }

    /// Marca/desmarca "corrigir último commit" (marcando com a caixa vazia, traz a última mensagem).
    pub fn git_toggle_amend(&mut self) {
        self.git.amend = !self.git.amend;
        if self.git.amend && self.git.message.trim().is_empty() {
            let generation = self.git.generation;
            self.spawn_bg("git-last-msg", move |core| Some(UiMsg::Git(generation, GitReply::LastMessage(core.git_last_message()))));
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

    pub fn git_open_branch_menu(&mut self, mode: BranchMenuMode) {
        if self.git.branch_menu && self.git.branch_menu_mode == mode {
            self.git.branch_menu = false;
            return;
        }
        self.git.branch_menu = true;
        self.git.branch_menu_mode = mode;
        self.git.branch_filter.clear();
        if !self.git.debug_hold {
            let generation = self.git.generation;
            self.spawn_bg("git-branches", move |core| Some(UiMsg::Git(generation, GitReply::Branches(core.git_branches()))));
        }
    }

    /// Troca o que o menu de branches faz (trocar, mesclar ou rebase) sem fechá-lo.
    pub fn git_open_branch_menu_mode(&mut self, mode: BranchMenuMode) {
        self.git.branch_menu_mode = mode;
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

    pub fn git_merge_branch(&mut self, branch: String) {
        self.git.branch_menu = false;
        self.git_run("Mesclar", "Mesclando…", move |core| core.git_merge(&branch));
    }

    pub fn git_rebase_onto(&mut self, branch: String) {
        self.git.branch_menu = false;
        self.git_run("Rebase", "Fazendo rebase…", move |core| core.git_rebase(&branch));
    }

    pub fn git_continue_op(&mut self) {
        self.git_run("Continuar", "Continuando…", |core| core.git_continue_op());
    }

    // ---------- Stash ----------

    pub fn git_load_stashes(&mut self) {
        if self.git.debug_hold {
            return;
        }
        self.git.stash_loading = true;
        let generation = self.git.generation;
        self.spawn_bg("git-stashes", move |core| Some(UiMsg::Git(generation, GitReply::Stashes(core.git_stash_list()))));
    }

    pub fn git_stash_push(&mut self) {
        let msg = self.git.stash_message.clone();
        let untracked = self.git.stash_include_untracked;
        self.git_run("Stash salvo", "Guardando no stash…", move |core| core.git_stash_push(&msg, untracked));
    }

    pub fn git_open_stash_diff(&mut self, stash: GitStash) {
        let index = stash.index;
        self.git.stash_diff = Some(OpenStashDiff { stash, diff: None });
        if self.git.debug_hold {
            return;
        }
        let generation = self.git.generation;
        self.spawn_bg("git-stash-diff", move |core| {
            let result = core.git_stash_diff(index);
            Some(UiMsg::Git(generation, GitReply::StashDiff { index, result }))
        });
    }

    pub fn git_stash_apply(&mut self, index: usize) {
        self.git_run("Stash aplicado", "Aplicando stash…", move |core| core.git_stash_apply(index));
    }

    pub fn git_stash_pop(&mut self, index: usize) {
        self.git.stash_diff = None;
        self.git_run("Stash aplicado e removido", "Aplicando stash…", move |core| core.git_stash_pop(index));
    }

    // ---------- Histórico ----------

    pub fn git_load_log(&mut self, append: bool) {
        if self.git.debug_hold || self.git.log_loading {
            return;
        }
        self.git.log_loading = true;
        let skip = if append { self.git.log.len() } else { 0 };
        let generation = self.git.generation;
        self.spawn_bg("git-log", move |core| {
            let result = core.git_log(skip, LOG_PAGE_SIZE);
            Some(UiMsg::Git(generation, GitReply::Log { append, result }))
        });
    }

    pub fn git_open_commit(&mut self, commit: GitCommit) {
        let hash = commit.hash.clone();
        self.git.commit_detail =
            Some(OpenCommit { hash: hash.clone(), short_hash: commit.short_hash, subject: commit.subject, detail: None });
        if self.git.debug_hold {
            return;
        }
        let generation = self.git.generation;
        self.spawn_bg("git-commit-detail", move |core| {
            let result = core.git_commit_detail(&hash);
            Some(UiMsg::Git(generation, GitReply::CommitDetail { hash, result }))
        });
    }

    // ---------- Conflitos ----------

    pub fn git_open_conflict(&mut self, path: String) {
        self.git.subview = GitSubView::Conflicts;
        self.git.conflict_open = Some(OpenConflict { path: path.clone(), file: None });
        if !self.git.debug_hold {
            self.git_load_conflict_file(path);
        }
    }

    fn git_load_conflict_file(&mut self, path: String) {
        let generation = self.git.generation;
        self.spawn_bg("git-conflict-file", move |core| {
            let result = core.git_conflict_file(&path);
            Some(UiMsg::Git(generation, GitReply::ConflictFile { path, result }))
        });
    }

    pub fn git_resolve_conflict_file(&mut self, path: String, choice: ConflictChoice) {
        let label = match choice {
            ConflictChoice::Ours => "Conflito resolvido (nosso)",
            ConflictChoice::Theirs => "Conflito resolvido (deles)",
            ConflictChoice::Both => "Conflito resolvido (ambos)",
        };
        self.git_run(label, "Resolvendo conflito…", move |core| core.git_resolve_conflict_file(&path, choice).map(|_| String::new()));
    }

    pub fn git_resolve_conflict_block(&mut self, path: String, block_index: usize, choice: ConflictChoice) {
        let label = match choice {
            ConflictChoice::Ours => "Bloco resolvido (nosso)",
            ConflictChoice::Theirs => "Bloco resolvido (deles)",
            ConflictChoice::Both => "Bloco resolvido (ambos)",
        };
        self.git_run(label, "Resolvendo bloco…", move |core| {
            core.git_resolve_conflict_block(&path, block_index, choice).map(|_| String::new())
        });
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
        stash_count: 2,
        truncated: false,
    }
}

/// SOMENTE DEPURAÇÃO: diff fictício.
pub fn demo_diff() -> GitDiff {
    let l = |kind, old, new, text: &str| DiffLine { kind, old, new, text: text.into() };
    GitDiff {
        lines: vec![
            l(DiffKind::Meta, None, None, "diff --git a/memory/GERAL.md b/memory/GERAL.md"),
            l(DiffKind::Meta, None, None, "--- a/memory/GERAL.md"),
            l(DiffKind::Meta, None, None, "+++ b/memory/GERAL.md"),
            l(DiffKind::Hunk, None, None, "@@ -20,4 +20,6 @@ ## 1. Ambientação"),
            l(DiffKind::Context, Some(20), Some(20), " Terminal desktop nativo em Rust (winit + egui)."),
            l(DiffKind::Del, Some(21), None, "-A Conversa integrada fica embaixo dos terminais."),
            l(DiffKind::Add, None, Some(21), "+A Conversa integrada fica na barra direita,"),
            l(DiffKind::Add, None, Some(22), "+com a aba Git (branch, alterações, diff e commit)."),
            l(DiffKind::Context, Some(22), Some(23), " "),
            l(DiffKind::Context, Some(23), Some(24), " ### Pastas"),
        ],
        truncated: false,
        binary: false,
    }
}

/// SOMENTE DEPURAÇÃO: diff com dois trechos (`COMMANDDECK_DEBUG_UI=git-hunks`).
pub fn demo_hunks_diff() -> GitDiff {
    let mut d = demo_diff();
    let l = |kind, old, new, text: &str| DiffLine { kind, old, new, text: text.into() };
    d.lines.extend([
        l(DiffKind::Hunk, None, None, "@@ -80,3 +82,3 @@ ### Limites"),
        l(DiffKind::Context, Some(80), Some(82), " | Regra | Valor |"),
        l(DiffKind::Del, Some(81), None, "-| Profundidade máxima | 2 |"),
        l(DiffKind::Add, None, Some(83), "+| Profundidade máxima de delegação | 2 |"),
    ]);
    d
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

/// SOMENTE DEPURAÇÃO: histórico fictício (`COMMANDDECK_DEBUG_UI=git-log`).
pub fn demo_log() -> Vec<GitCommit> {
    let c = |h: &str, author: &str, date: &str, subject: &str| GitCommit {
        hash: format!("{h}000000000000000000000000000000000"),
        short_hash: h.into(),
        author: author.into(),
        date: date.into(),
        subject: subject.into(),
    };
    vec![
        c("61621aa", "Ana", "há 12 min", "fix(terminal): prazo da atualização sincronizada"),
        c("70e7441", "Claude", "há 2 horas", "fix: auditoria de regressão (ponte, Git, config e interface)"),
        c("8c3de88", "Ana", "há 4 horas", "chore: skills de interface do projeto"),
        c("52e96ad", "Antigravity", "ontem", "feat(ui): agentes em árvore com equipe reorganizável"),
        c("e60286b", "Claude", "ontem", "feat(ui): abas por projeto na Deck"),
        c("b1f7f48", "Claude", "há 2 dias", "feat(core): projetos como unidade principal"),
    ]
}

/// SOMENTE DEPURAÇÃO: detalhe de um commit (`COMMANDDECK_DEBUG_UI=git-commit`).
pub fn demo_commit_detail() -> GitCommitDetail {
    GitCommitDetail {
        hash: "61621aa000000000000000000000000000000000".into(),
        short_hash: "61621aa".into(),
        author: "Ana".into(),
        email: "ana@commanddeck.dev".into(),
        date: "há 12 min".into(),
        message: "fix(terminal): prazo da atualização sincronizada\n\nSolta a saída retida após 150 ms quando o fim do bloco se perde."
            .into(),
        files: vec![('M', "crates/app/src/terminal/mod.rs".into()), ('M', "memory/GERAL.md".into())],
        diff: demo_diff(),
    }
}

/// SOMENTE DEPURAÇÃO: stashes fictícios (`COMMANDDECK_DEBUG_UI=git-stash`).
pub fn demo_stashes() -> Vec<GitStash> {
    vec![
        GitStash { index: 0, date: "há 25 min".into(), message: "On feat/git: rascunho do editor de conflitos".into() },
        GitStash { index: 1, date: "ontem".into(), message: "WIP on dev: ajuste na barra lateral".into() },
    ]
}

/// SOMENTE DEPURAÇÃO: status com conflito e merge em andamento (`COMMANDDECK_DEBUG_UI=git-conflicts`).
pub fn demo_conflicts_status() -> GitStatus {
    let mut st = demo_status();
    st.state = RepoState::Merging;
    st.entries.insert(
        0,
        GitEntry { path: "crates/core/src/git.rs".into(), orig_path: None, index: 'U', worktree: 'U', kind: EntryKind::Unmerged },
    );
    st.entries
        .insert(1, GitEntry { path: "memory/GERAL.md".into(), orig_path: None, index: 'U', worktree: 'U', kind: EntryKind::Unmerged });
    st
}

/// SOMENTE DEPURAÇÃO: arquivo em conflito com blocos (`COMMANDDECK_DEBUG_UI=git-conflicts`).
pub fn demo_conflict_file() -> GitConflictFile {
    GitConflictFile {
        path: "crates/core/src/git.rs".into(),
        binary: false,
        blocks: vec![
            ConflictBlock {
                start_line: 42,
                ours_label: "HEAD (feat/git)".into(),
                ours: vec!["const READ_TIMEOUT: Duration = Duration::from_secs(5);".into()],
                theirs_label: "dev".into(),
                theirs: vec!["const READ_TIMEOUT: Duration = Duration::from_secs(10);".into()],
            },
            ConflictBlock {
                start_line: 118,
                ours_label: "HEAD (feat/git)".into(),
                ours: vec!["pub stash_count: u32,".into(), "pub truncated: bool,".into()],
                theirs_label: "dev".into(),
                theirs: vec!["pub truncated: bool,".into()],
            },
        ],
    }
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
