//! Modo Documentos do centro: o estado e a lógica pura (a pintura fica em `ui/docs.rs`).
//!
//! O centro alterna entre os agentes (a tela dividida) e os documentos: diff de uma tarefa, de um
//! commit ou de um arquivo, arquivo de texto e markdown, todos só para leitura. Os documentos são por
//! projeto e não são gravados. Abrir um documento já aberto só o foca; abrir qualquer um muda o centro
//! para Documentos, e fechar o último volta para Agentes.

use std::collections::HashMap;

use agentdeck_core::git::{DiffKind, EntryKind, GitDiff, GitEntry, GitError};
use agentdeck_core::workspace::READ_TEXT_MAX;
use agentdeck_core::TextFile;

use crate::deck::{Deck, UiMsg};

/// O que o centro mostra.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CenterView {
    #[default]
    Agents,
    Docs,
}

/// De onde vem o texto de um documento markdown.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MdSource {
    /// Um `.md` do projeto (caminho relativo).
    File(String),
    /// O pedido da tarefa (id): o texto vem da tarefa já carregada.
    TaskBrief(String),
    /// O relatório da tarefa (id).
    TaskReport(String),
}

/// O que um documento é. Dois documentos do mesmo projeto com o mesmo `DocKind` são o mesmo.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DocKind {
    /// O diff da tarefa: o da branch (`merged` falso) ou, depois da mesclagem, o que ela trouxe. Os dois são
    /// documentos diferentes: a tarefa que muda de estado não volta ao diff antigo.
    TaskDiff {
        id: String,
        merged: bool,
    },
    /// O diff de um commit (hash completo).
    CommitDiff(String),
    /// O diff de um arquivo alterado (caminho a partir da raiz do repositório), no lado preparado ou no não
    /// preparado; `orig` é o nome antigo de um renomeado.
    FileDiff {
        path: String,
        orig: Option<String>,
        staged: bool,
        untracked: bool,
    },
    /// Um arquivo de texto do projeto (caminho relativo).
    File(String),
    Markdown(MdSource),
}

impl DocKind {
    /// O arquivo do projeto que o documento lê, se for de arquivo (recarrega quando ele muda).
    pub fn file_path(&self) -> Option<&str> {
        match self {
            DocKind::File(p) | DocKind::Markdown(MdSource::File(p)) => Some(p),
            _ => None,
        }
    }

    /// Dá para ler de novo (botão de recarregar): tudo que vem do core, não o texto de uma tarefa.
    pub fn reloadable(&self) -> bool {
        !matches!(self, DocKind::Markdown(MdSource::TaskBrief(_) | MdSource::TaskReport(_)))
    }

    /// A leitura usa o projeto **ativo** na hora em que a thread roda (arquivo, commit, arquivo alterado): se o usuário
    /// trocou de projeto nesse meio-tempo, o resultado é de outro projeto. O diff da tarefa passa o projeto e não tem esse risco.
    pub fn reads_active_project(&self) -> bool {
        matches!(self, DocKind::CommitDiff(_) | DocKind::FileDiff { .. } | DocKind::File(_) | DocKind::Markdown(MdSource::File(_)))
    }
}

/// Um arquivo dentro de um diff (a lista da esquerda).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffFile {
    pub path: String,
    /// `A` novo, `D` apagado, `R` renomeado, `M` alterado.
    pub status: char,
    pub added: usize,
    pub removed: usize,
    /// Índice, em `GitDiff::lines`, da linha `diff --git` deste arquivo (para onde a lista rola).
    pub line: usize,
}

#[derive(Debug, Clone)]
pub struct DiffDoc {
    pub diff: GitDiff,
    pub files: Vec<DiffFile>,
    /// O arquivo selecionado na lista (o último em que se clicou).
    pub selected: usize,
    /// Linha para onde o diff rola no próximo quadro (clique na lista).
    pub jump: Option<usize>,
}

impl DiffDoc {
    pub fn new(diff: GitDiff) -> Self {
        let files = diff_files(&diff);
        Self { diff, files, selected: 0, jump: None }
    }

    /// Seleciona o arquivo `n` da lista e pede a rolagem até ele.
    pub fn select(&mut self, n: usize) {
        if let Some(f) = self.files.get(n) {
            self.selected = n;
            self.jump = Some(f.line);
        }
    }
}

/// As linhas de um arquivo de texto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextDoc {
    pub lines: Vec<String>,
    /// A linha mais comprida, em caracteres (a largura da rolagem horizontal).
    pub max_chars: usize,
}

/// Uma linha com mais caracteres que isto é cortada na tela (como o diff faz com 1000): um arquivo de 1 MB numa linha só
/// (JSON minificado) não deve travar a pintura.
const TEXT_LINE_MAX: usize = 2000;

impl TextDoc {
    pub fn new(text: &str) -> Self {
        let lines: Vec<String> = text
            .lines()
            .map(|l| {
                let l = l.replace('\t', "    ");
                if l.chars().count() > TEXT_LINE_MAX {
                    l.chars().take(TEXT_LINE_MAX).chain(['…']).collect()
                } else {
                    l
                }
            })
            .collect();
        let max_chars = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
        Self { lines, max_chars }
    }
}

#[derive(Debug, Clone)]
pub enum DocState {
    Loading,
    Failed(String),
    Diff(DiffDoc),
    Text(TextDoc),
    /// O texto markdown, desenhado já formatado.
    Markdown(String),
    /// Um arquivo que não vira texto na tela (grande demais ou binário): a explicação.
    Unreadable(String),
}

/// O que chegou de uma leitura feita fora da thread da interface.
#[derive(Debug, Clone)]
pub enum Loaded {
    Diff(Result<GitDiff, String>),
    Text(Result<TextFile, String>),
}

#[derive(Debug, Clone)]
pub struct DocTab {
    pub uid: u64,
    /// Id do projeto a que o documento pertence.
    pub project: String,
    pub kind: DocKind,
    /// O nome curto, na aba.
    pub title: String,
    /// A linha de cima do documento (caminho, tarefa, commit) e a dica da aba.
    pub subtitle: String,
    pub state: DocState,
}

impl DocTab {
    /// Aplica o resultado de uma leitura. O estado vira o que o tipo do documento mostra; o arquivo
    /// selecionado e a rolagem de um diff relido não se perdem (o `selected` só é mantido se ainda existe).
    pub fn apply(&mut self, loaded: Loaded) {
        let previous = match &self.state {
            DocState::Diff(d) => Some(d.selected),
            _ => None,
        };
        self.state = match loaded {
            Loaded::Diff(Err(e)) | Loaded::Text(Err(e)) => DocState::Failed(e),
            Loaded::Diff(Ok(diff)) => {
                let mut doc = DiffDoc::new(diff);
                if let Some(n) = previous.filter(|n| *n < doc.files.len()) {
                    doc.selected = n;
                }
                DocState::Diff(doc)
            }
            Loaded::Text(Ok(TextFile::TooLarge(bytes))) => DocState::Unreadable(format!(
                "O arquivo tem {} e passa do limite de {}. Abra no programa padrão.",
                size_label(bytes),
                size_label(READ_TEXT_MAX)
            )),
            Loaded::Text(Ok(TextFile::Binary)) => {
                DocState::Unreadable("O arquivo é binário: o conteúdo não aparece aqui. Abra no programa padrão.".into())
            }
            Loaded::Text(Ok(TextFile::Text(text))) if matches!(self.kind, DocKind::Markdown(_)) => DocState::Markdown(text),
            Loaded::Text(Ok(TextFile::Text(text))) => DocState::Text(TextDoc::new(&text)),
        };
    }

    /// O texto já está na tela igual ao que chegou (a releitura do watcher não precisa repintar nada).
    pub fn same_text(&self, loaded: &Loaded) -> bool {
        match (&self.state, loaded) {
            (DocState::Markdown(old), Loaded::Text(Ok(TextFile::Text(new)))) => old == new,
            (DocState::Text(old), Loaded::Text(Ok(TextFile::Text(new)))) => *old == TextDoc::new(new),
            _ => false,
        }
    }
}

/// "12 KB", "1,5 MB".
pub fn size_label(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= KB * KB {
        format!("{:.1} MB", b / (KB * KB)).replace('.', ",")
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} bytes")
    }
}

/// Os documentos do app e o que o centro mostra.
#[derive(Default)]
pub struct Docs {
    pub view: CenterView,
    tabs: Vec<DocTab>,
    /// O documento em foco de cada projeto.
    current: HashMap<String, u64>,
    next_uid: u64,
    /// SOMENTE DEPURAÇÃO (`AGENTDECK_DEBUG_UI=docs-demo`): os documentos são fictícios; nada é lido nem relido.
    pub debug_hold: bool,
    /// SOMENTE DEPURAÇÃO (`docs-demo-badge`): o selo da chave, em vez do das abas de verdade.
    pub debug_badge: Option<Badge>,
    /// O cache do markdown formatado (links e rolagem), compartilhado pelos documentos.
    pub md_cache: egui_commonmark::CommonMarkCache,
}

/// O resultado de `Docs::open`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Opened {
    pub uid: u64,
    /// O documento é novo (quem abriu precisa carregá-lo).
    pub fresh: bool,
}

impl Docs {
    /// Abre o documento, ou só foca se o projeto já o tem aberto, e muda o centro para Documentos. Um documento cujo
    /// texto vem da tarefa (pedido, relatório) não é relido: ao abri-lo de novo, o estado e os títulos passam a ser os
    /// de agora (o relatório mudou depois de uma devolução).
    pub fn open(&mut self, project: &str, kind: DocKind, title: String, subtitle: String, state: DocState) -> Opened {
        self.view = CenterView::Docs;
        if let Some(t) = self.tabs.iter_mut().find(|t| t.project == project && t.kind == kind) {
            if !t.kind.reloadable() {
                (t.title, t.subtitle, t.state) = (title, subtitle, state);
            }
            let uid = t.uid;
            self.current.insert(project.to_owned(), uid);
            return Opened { uid, fresh: false };
        }
        self.next_uid += 1;
        let uid = self.next_uid;
        self.tabs.push(DocTab { uid, project: project.to_owned(), kind, title, subtitle, state });
        self.current.insert(project.to_owned(), uid);
        Opened { uid, fresh: true }
    }

    pub fn focus(&mut self, uid: u64) {
        if let Some(t) = self.tabs.iter().find(|t| t.uid == uid) {
            self.current.insert(t.project.clone(), uid);
        }
    }

    /// Fecha o documento. O foco passa ao vizinho (o da direita, ou o último); se o projeto fica sem
    /// documentos e o centro mostrava Documentos, volta para Agentes.
    pub fn close(&mut self, uid: u64) {
        let Some(pos) = self.tabs.iter().position(|t| t.uid == uid) else { return };
        let project = self.tabs.remove(pos).project;
        if self.current.get(&project) == Some(&uid) {
            let next = self.tabs.iter().skip(pos).chain(self.tabs.iter()).find(|t| t.project == project).map(|t| t.uid);
            match next {
                Some(n) => self.current.insert(project.clone(), n),
                None => self.current.remove(&project),
            };
        }
        self.sync(&project);
    }

    /// Tira do centro o que não existe: Documentos num projeto sem documento vira Agentes.
    pub fn sync(&mut self, project: &str) {
        if self.view == CenterView::Docs && self.count(project) == 0 {
            self.view = CenterView::Agents;
        }
    }

    /// Alterna a chave. Sem documentos no projeto, fica em Agentes.
    pub fn toggle(&mut self, project: &str) {
        self.view = match self.view {
            CenterView::Docs => CenterView::Agents,
            CenterView::Agents if self.count(project) > 0 => CenterView::Docs,
            CenterView::Agents => CenterView::Agents,
        };
    }

    /// Quantos documentos o projeto tem abertos.
    pub fn count(&self, project: &str) -> usize {
        self.tabs.iter().filter(|t| t.project == project).count()
    }

    /// Os documentos do projeto, na ordem de abertura.
    pub fn of_project<'a>(&'a self, project: &'a str) -> impl Iterator<Item = &'a DocTab> {
        self.tabs.iter().filter(move |t| t.project == project)
    }

    /// O documento em foco do projeto (o primeiro, se nenhum foi escolhido).
    pub fn current_uid(&self, project: &str) -> Option<u64> {
        self.current.get(project).copied().or_else(|| self.of_project(project).next().map(|t| t.uid))
    }

    pub fn get(&self, uid: u64) -> Option<&DocTab> {
        self.tabs.iter().find(|t| t.uid == uid)
    }

    pub fn get_mut(&mut self, uid: u64) -> Option<&mut DocTab> {
        self.tabs.iter_mut().find(|t| t.uid == uid)
    }

    /// Foco no documento seguinte (ou anterior) do projeto, dando a volta.
    pub fn cycle(&mut self, project: &str, back: bool) {
        let uids: Vec<u64> = self.of_project(project).map(|t| t.uid).collect();
        let n = uids.len();
        if n == 0 {
            return;
        }
        let cur = self.current_uid(project).and_then(|c| uids.iter().position(|u| *u == c)).unwrap_or(0);
        let next = (cur + if back { n - 1 } else { 1 }) % n;
        self.current.insert(project.to_owned(), uids[next]);
    }

    /// O que reler quando `project` está ativo: os arquivos (o watcher avisou, ou o projeto voltou à tela) e o que ficou
    /// "Carregando…" porque a resposta chegou com outro projeto ativo e foi descartada (`accepts_load`).
    pub fn to_reread(&self, project: &str) -> Vec<u64> {
        self.of_project(project)
            .filter(|t| t.kind.file_path().is_some() || (matches!(t.state, DocState::Loading) && t.kind.reads_active_project()))
            .map(|t| t.uid)
            .collect()
    }

    /// A resposta de leitura de `uid` vale? Não vale se o documento fechou, nem se ele lê o projeto ativo e o ativo
    /// agora não é o dele (a thread leu o workspace de outro projeto).
    pub fn accepts_load(&self, uid: u64, active_project: &str) -> bool {
        self.get(uid).is_some_and(|t| !t.kind.reads_active_project() || t.project == active_project)
    }

    /// Um projeto removido leva os documentos dele.
    pub fn drop_project(&mut self, project: &str) {
        self.tabs.retain(|t| t.project != project);
        self.current.remove(project);
        self.sync(project);
    }
}

/// O selo do lado "Agentes" da chave: o que aconteceu nas abas enquanto se lê um documento.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Badge {
    /// Abas que terminaram um turno sem ser vistas.
    pub done: usize,
    /// Abas trabalhando agora.
    pub busy: usize,
}

impl Badge {
    /// De `(done, busy)` de cada aba do projeto.
    pub fn of(tabs: impl IntoIterator<Item = (bool, bool)>) -> Self {
        tabs.into_iter()
            .fold(Self::default(), |b, (done, busy)| Self { done: b.done + usize::from(done), busy: b.busy + usize::from(busy) })
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.done == 0 && self.busy == 0
    }
}

/// Os arquivos de um diff, na ordem em que aparecem, com o que cada um mudou. O caminho vem de `+++ b/…`
/// (ou de `--- a/…` num arquivo apagado), porque o cabeçalho `diff --git a/x b/y` é ambíguo com espaços.
pub fn diff_files(diff: &GitDiff) -> Vec<DiffFile> {
    let mut files: Vec<DiffFile> = Vec::new();
    let mut in_header = false;
    for (i, line) in diff.lines.iter().enumerate() {
        if line.kind == DiffKind::Meta {
            let t = line.text.trim_end_matches('\r');
            if let Some(rest) =
                t.strip_prefix("diff --git ").or_else(|| t.strip_prefix("diff --cc ")).or_else(|| t.strip_prefix("diff --combined "))
            {
                in_header = true;
                files.push(DiffFile { path: header_path(rest, t.starts_with("diff --git ")), status: 'M', added: 0, removed: 0, line: i });
                continue;
            }
            let Some(file) = files.last_mut().filter(|_| in_header) else { continue };
            if t.starts_with("new file mode") {
                file.status = 'A';
            } else if t.starts_with("deleted file mode") {
                file.status = 'D';
            } else if let Some(to) = t.strip_prefix("rename to ") {
                file.status = 'R';
                file.path = unquote(to);
            } else if let Some(p) = t.strip_prefix("+++ ").filter(|p| *p != "/dev/null") {
                file.path = unquote(p.strip_prefix("b/").unwrap_or(p));
            } else if let Some(p) = t.strip_prefix("--- ").filter(|p| *p != "/dev/null") {
                if file.status == 'D' {
                    file.path = unquote(p.strip_prefix("a/").unwrap_or(p));
                }
            } else if t.starts_with("Binary files ") {
                // `Binary files a/x and b/y differ`: o cabeçalho já deu o caminho.
            }
            continue;
        }
        if line.kind == DiffKind::Hunk {
            in_header = false;
        }
        if let Some(file) = files.last_mut() {
            match line.kind {
                DiffKind::Add => file.added += 1,
                DiffKind::Del => file.removed += 1,
                _ => {}
            }
        }
    }
    files
}

/// O caminho de um `diff --git a/x b/y` (a parte depois de `diff --git `): o `b/…`; com espaços, onde
/// a metade de `a/` e a de `b/` são iguais.
fn header_path(rest: &str, git: bool) -> String {
    if !git {
        return unquote(rest);
    }
    let rest = rest.trim();
    // Sem renomeação as duas metades são iguais: `a/<p> b/<p>`.
    if let Some(a) = rest.strip_prefix("a/").filter(|a| a.len() >= 3 && (a.len() - 3) % 2 == 0) {
        let n = (a.len() - 3) / 2;
        if a.is_char_boundary(n) && a.is_char_boundary(n + 3) && &a[n..n + 3] == " b/" && a[..n] == a[n + 3..] {
            return unquote(&a[..n]);
        }
    }
    match rest.rfind(" b/") {
        Some(i) => unquote(&rest[i + 3..]),
        None => unquote(rest),
    }
}

fn unquote(path: &str) -> String {
    let p = path.trim();
    p.strip_prefix('"').and_then(|p| p.strip_suffix('"')).unwrap_or(p).to_owned()
}

/// O nome do arquivo (último componente) de um caminho com `/` ou `\`.
pub fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().filter(|n| !n.is_empty()).unwrap_or(path)
}

// ---------------------------------------------------------------------------
// O Deck e os documentos: abrir, ler (fora da thread da interface) e a troca do centro
// ---------------------------------------------------------------------------

impl Deck {
    /// O centro volta a mostrar os agentes (clicar num agente, abrir uma aba).
    pub fn show_agents(&mut self) {
        self.docs.view = CenterView::Agents;
    }

    /// O centro trocou de lado: o terminal perde o teclado ao ler documentos, e ao voltar a aba que
    /// estava ativa conta como vista.
    pub(crate) fn center_changed(&mut self) {
        self.center_seen = self.docs.view;
        match self.docs.view {
            CenterView::Docs => {
                self.term_focus = false;
                self.ime_preedit.clear();
                self.new_tab_menu_open = false;
            }
            CenterView::Agents => {
                if let Some(tab) = self.active.and_then(|i| self.tabs.get_mut(i)) {
                    tab.done = false;
                }
                self.set_term_focus(true);
            }
        }
    }

    /// O selo do lado "Agentes" da chave: as abas do projeto ativo que terminaram sem ser vistas ou estão trabalhando.
    pub fn agents_badge(&self) -> Badge {
        if let Some(badge) = self.docs.debug_badge {
            return badge;
        }
        Badge::of(self.visible().into_iter().map(|i| (self.tabs[i].done, self.tabs[i].busy)))
    }

    /// Abre (ou foca) um documento do projeto ativo e começa a lê-lo se for novo.
    fn docs_open(&mut self, kind: DocKind, title: String, subtitle: String, state: DocState) {
        let project = self.active_project.clone();
        if project.is_empty() {
            return;
        }
        let opened = self.docs.open(&project, kind, title, subtitle, state);
        if opened.fresh {
            self.docs_load(opened.uid);
        }
    }

    /// O diff da tarefa: o da branch, ou o que a mesclagem trouxe (ver `tasks_ui::shows_merged_diff`).
    pub fn docs_open_task_diff(&mut self, id: &str) {
        let Some(task) = self.tasks.task(id) else { return };
        let merged = crate::tasks_ui::shows_merged_diff(task);
        let title = format!("{} · {}", task.id, if merged { "mesclagem" } else { "diff" });
        let subtitle = crate::tasks_ui::diff_title(task);
        self.docs_open(DocKind::TaskDiff { id: id.to_owned(), merged }, title, subtitle, DocState::Loading);
    }

    /// O pedido ou o relatório da tarefa, já formatados. O texto vem da tarefa carregada, sem ler nada.
    pub fn docs_open_task_text(&mut self, id: &str, report: bool) {
        let Some(task) = self.tasks.task(id) else { return };
        let (what, text) = if report {
            ("relatório", task.report.clone().unwrap_or_else(|| "A tarefa ainda não tem relatório.".to_owned()))
        } else {
            ("pedido", task.brief.clone())
        };
        let source = if report { MdSource::TaskReport(id.to_owned()) } else { MdSource::TaskBrief(id.to_owned()) };
        let (title, subtitle) = (format!("{} · {what}", task.id), format!("{} · {what}: {}", task.id, task.title));
        self.docs_open(DocKind::Markdown(source), title, subtitle, DocState::Markdown(text));
    }

    /// O diff de um commit do Histórico da aba Git.
    pub fn docs_open_commit(&mut self, hash: &str, short_hash: &str, subject: &str) {
        let (title, subtitle) = (short_hash.to_owned(), format!("{short_hash}  {subject}"));
        self.docs_open(DocKind::CommitDiff(hash.to_owned()), title, subtitle, DocState::Loading);
    }

    /// O diff de um arquivo alterado da aba Git (só leitura; o preparo por trecho continua na barra direita).
    pub fn docs_open_file_diff(&mut self, entry: &GitEntry, staged: bool) {
        let side = if staged {
            "preparado"
        } else if entry.kind == EntryKind::Untracked {
            "não rastreado"
        } else {
            "não preparado"
        };
        let title = format!("{} · diff", file_name(&entry.path));
        let subtitle = format!("{}  · {side}", entry.path);
        let kind = DocKind::FileDiff {
            path: entry.path.clone(),
            orig: entry.orig_path.clone(),
            staged,
            untracked: entry.kind == EntryKind::Untracked,
        };
        self.docs_open(kind, title, subtitle, DocState::Loading);
    }

    /// Um arquivo do projeto (caminho relativo a ele): `.md` formatado, o resto como texto.
    pub fn docs_open_file(&mut self, rel: &str) {
        let ext = std::path::Path::new(rel).extension().and_then(|e| e.to_str()).unwrap_or("");
        let is_md = ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown");
        let kind = if is_md { DocKind::Markdown(MdSource::File(rel.to_owned())) } else { DocKind::File(rel.to_owned()) };
        self.docs_open(kind, file_name(rel).to_owned(), rel.to_owned(), DocState::Loading);
    }

    /// Fecha um documento (se era o último do projeto, o centro volta para os agentes).
    pub fn docs_close(&mut self, uid: u64) {
        self.docs.close(uid);
    }

    /// Lê o documento de novo, mostrando "Carregando…" (botão de recarregar).
    pub fn docs_reload(&mut self, uid: u64) {
        if let Some(tab) = self.docs.get_mut(uid).filter(|t| t.kind.reloadable()) {
            tab.state = DocState::Loading;
            self.docs_load(uid);
        }
    }

    /// Os arquivos abertos do projeto ativo mudaram no disco (o watcher avisou): relê, sem piscar.
    pub fn docs_reload_files(&mut self) {
        if self.docs.debug_hold {
            return;
        }
        for uid in self.docs.to_reread(&self.active_project) {
            self.docs_load(uid);
        }
    }

    /// Lê o documento numa thread; o resultado volta como `UiMsg::Doc`.
    fn docs_load(&mut self, uid: u64) {
        let Some(tab) = self.docs.get(uid).filter(|_| !self.docs.debug_hold) else { return };
        let (project, kind) = (tab.project.clone(), tab.kind.clone());
        let text_err = |e: GitError| e.to_string();
        match kind {
            DocKind::TaskDiff { id, .. } => self.spawn_bg("doc-task-diff", move |core| {
                Some(UiMsg::Doc(uid, Loaded::Diff(core.task_diff(&project, &id).map_err(|e| e.message().to_owned()))))
            }),
            DocKind::CommitDiff(hash) => self.spawn_bg("doc-commit-diff", move |core| {
                Some(UiMsg::Doc(uid, Loaded::Diff(core.git_commit_detail(&hash).map(|d| d.diff).map_err(text_err))))
            }),
            DocKind::FileDiff { path, orig, staged, untracked } => self.spawn_bg("doc-file-diff", move |core| {
                let kind = if untracked { EntryKind::Untracked } else { EntryKind::Ordinary };
                let entry = GitEntry { path, orig_path: orig, index: '.', worktree: '.', kind };
                Some(UiMsg::Doc(uid, Loaded::Diff(core.git_diff(&entry, staged).map_err(text_err))))
            }),
            DocKind::File(rel) | DocKind::Markdown(MdSource::File(rel)) => {
                self.spawn_bg("doc-file", move |core| Some(UiMsg::Doc(uid, Loaded::Text(core.read_text(&rel)))))
            }
            DocKind::Markdown(MdSource::TaskBrief(_) | MdSource::TaskReport(_)) => {}
        }
    }

    /// A leitura de um documento chegou. Um documento fechado nesse meio-tempo, ou de um projeto que já não é o ativo
    /// (a leitura foi feita na pasta do outro), ignora a resposta; um arquivo que não mudou não é repintado.
    pub fn on_doc_loaded(&mut self, uid: u64, loaded: Loaded) {
        if self.docs.debug_hold || !self.docs.accepts_load(uid, &self.active_project) {
            return;
        }
        if let Some(tab) = self.docs.get_mut(uid).filter(|t| !t.same_text(&loaded)) {
            tab.apply(loaded);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentdeck_core::git::parse_diff;

    const P: &str = "proj";

    fn open(docs: &mut Docs, project: &str, kind: DocKind) -> Opened {
        docs.open(project, kind, "t".into(), String::new(), DocState::Loading)
    }

    #[test]
    fn opening_creates_a_document_and_switches_the_center_to_documents() {
        let mut docs = Docs::default();
        assert_eq!(docs.view, CenterView::Agents);
        let o = open(&mut docs, P, DocKind::File("a.rs".into()));
        assert!(o.fresh);
        assert_eq!(docs.view, CenterView::Docs);
        assert_eq!(docs.count(P), 1);
        assert_eq!(docs.current_uid(P), Some(o.uid));
    }

    #[test]
    fn opening_an_open_document_only_focuses_it() {
        let mut docs = Docs::default();
        let a = open(&mut docs, P, DocKind::File("a.rs".into()));
        let b = open(&mut docs, P, DocKind::TaskDiff { id: "t1".into(), merged: false });
        assert_eq!(docs.current_uid(P), Some(b.uid));
        docs.view = CenterView::Agents;
        let again = open(&mut docs, P, DocKind::File("a.rs".into()));
        assert_eq!(again, Opened { uid: a.uid, fresh: false });
        assert_eq!(docs.count(P), 2);
        assert_eq!(docs.current_uid(P), Some(a.uid));
        assert_eq!(docs.view, CenterView::Docs, "focar também leva o centro para os documentos");
    }

    #[test]
    fn the_same_kind_in_two_projects_is_two_documents() {
        let mut docs = Docs::default();
        let a = open(&mut docs, "p1", DocKind::File("a.rs".into()));
        let b = open(&mut docs, "p2", DocKind::File("a.rs".into()));
        assert_ne!(a.uid, b.uid);
        assert!(b.fresh);
        // A mesma chave com outro lado é outro documento.
        let file_diff = |staged| DocKind::FileDiff { path: "a.rs".into(), orig: None, staged, untracked: false };
        let s = open(&mut docs, "p1", file_diff(true));
        let u = open(&mut docs, "p1", file_diff(false));
        assert_ne!(s.uid, u.uid);
        // O diff da tarefa antes e depois de mesclar também.
        let before = open(&mut docs, "p1", DocKind::TaskDiff { id: "t1".into(), merged: false });
        let after = open(&mut docs, "p1", DocKind::TaskDiff { id: "t1".into(), merged: true });
        assert_ne!(before.uid, after.uid);
    }

    #[test]
    fn documents_belong_to_a_project() {
        let mut docs = Docs::default();
        open(&mut docs, "p1", DocKind::File("a.rs".into()));
        open(&mut docs, "p1", DocKind::File("b.rs".into()));
        open(&mut docs, "p2", DocKind::File("c.rs".into()));
        assert_eq!(docs.count("p1"), 2);
        assert_eq!(docs.count("p2"), 1);
        assert_eq!(docs.count("p3"), 0);
        let names: Vec<&str> = docs.of_project("p1").map(|t| t.kind.file_path().unwrap_or("")).collect();
        assert_eq!(names, ["a.rs", "b.rs"]);
        // Cada projeto lembra o seu foco.
        let c = docs.current_uid("p2").expect("foco");
        assert_eq!(docs.get(c).map(|t| t.project.as_str()), Some("p2"));
        // Num projeto sem documentos o centro não fica em Documentos.
        docs.sync("p3");
        assert_eq!(docs.view, CenterView::Agents);
    }

    #[test]
    fn closing_moves_focus_to_the_neighbour_and_the_last_one_goes_back_to_agents() {
        let mut docs = Docs::default();
        let a = open(&mut docs, P, DocKind::File("a.rs".into())).uid;
        let b = open(&mut docs, P, DocKind::File("b.rs".into())).uid;
        let c = open(&mut docs, P, DocKind::File("c.rs".into())).uid;
        docs.focus(b);
        docs.close(b);
        assert_eq!(docs.current_uid(P), Some(c), "o da direita assume");
        docs.close(c);
        assert_eq!(docs.current_uid(P), Some(a), "sem vizinho à direita, o da esquerda");
        // Fechar o que não está em foco não mexe no foco.
        let d = open(&mut docs, P, DocKind::File("d.rs".into())).uid;
        docs.focus(d);
        docs.close(a);
        assert_eq!(docs.current_uid(P), Some(d));
        assert_eq!(docs.view, CenterView::Docs);
        docs.close(d);
        assert_eq!(docs.count(P), 0);
        assert_eq!(docs.view, CenterView::Agents);
        // Fechar um que não existe não faz nada.
        docs.close(999);
    }

    #[test]
    fn the_switch_needs_documents_to_show_them() {
        let mut docs = Docs::default();
        docs.toggle(P);
        assert_eq!(docs.view, CenterView::Agents);
        let uid = open(&mut docs, P, DocKind::File("a.rs".into())).uid;
        docs.toggle(P);
        assert_eq!(docs.view, CenterView::Agents);
        docs.toggle(P);
        assert_eq!(docs.view, CenterView::Docs);
        docs.toggle("outro");
        assert_eq!(docs.view, CenterView::Agents);
        docs.toggle("outro");
        assert_eq!(docs.view, CenterView::Agents, "o outro projeto não tem documentos");
        assert_eq!(docs.current_uid(P), Some(uid));
    }

    #[test]
    fn cycling_goes_around_the_project_documents() {
        let mut docs = Docs::default();
        let a = open(&mut docs, P, DocKind::File("a.rs".into())).uid;
        let b = open(&mut docs, P, DocKind::File("b.rs".into())).uid;
        let x = open(&mut docs, "p2", DocKind::File("x.rs".into())).uid;
        let c = open(&mut docs, P, DocKind::File("c.rs".into())).uid;
        docs.cycle(P, false);
        assert_eq!(docs.current_uid(P), Some(a));
        docs.cycle(P, false);
        assert_eq!(docs.current_uid(P), Some(b));
        docs.cycle(P, true);
        docs.cycle(P, true);
        assert_eq!(docs.current_uid(P), Some(c));
        assert_eq!(docs.current_uid("p2"), Some(x));
        docs.cycle("vazio", false);
    }

    #[test]
    fn dropping_a_project_takes_its_documents() {
        let mut docs = Docs::default();
        open(&mut docs, "p1", DocKind::File("a.rs".into()));
        open(&mut docs, "p2", DocKind::File("b.rs".into()));
        docs.drop_project("p1");
        assert_eq!((docs.count("p1"), docs.count("p2")), (0, 1));
        assert_eq!(docs.current_uid("p1"), None);
    }

    #[test]
    fn file_documents_are_the_ones_reread_when_the_workspace_changes() {
        let mut docs = Docs::default();
        let a = open(&mut docs, P, DocKind::File("a.rs".into())).uid;
        let m = open(&mut docs, P, DocKind::Markdown(MdSource::File("LEIA.md".into()))).uid;
        open(&mut docs, P, DocKind::Markdown(MdSource::TaskBrief("t1".into())));
        open(&mut docs, P, DocKind::TaskDiff { id: "t1".into(), merged: false });
        open(&mut docs, "p2", DocKind::File("z.rs".into()));
        assert_eq!(docs.to_reread(P), [a, m]);
        assert!(DocKind::File("a".into()).reloadable() && DocKind::CommitDiff("abc".into()).reloadable());
        assert!(!DocKind::Markdown(MdSource::TaskReport("t1".into())).reloadable());
    }

    #[test]
    fn reopening_the_task_request_or_report_shows_the_current_text() {
        let mut docs = Docs::default();
        let kind = DocKind::Markdown(MdSource::TaskReport("t1".into()));
        let text = |s: &str| DocState::Markdown(s.to_owned());
        let first = docs.open(P, kind.clone(), "t1 · relatório".into(), "antigo".into(), text("relatório velho"));
        assert!(first.fresh);
        let again = docs.open(P, kind, "t1 · relatório".into(), "novo".into(), text("relatório novo"));
        assert_eq!(again, Opened { uid: first.uid, fresh: false });
        let tab = docs.get(first.uid).expect("aba");
        assert!(matches!(&tab.state, DocState::Markdown(t) if t == "relatório novo"));
        assert_eq!(tab.subtitle, "novo");
        // O que vem do core não é trocado ao reabrir (só o "Recarregar" ou o watcher relê).
        let file = DocKind::File("a.rs".into());
        let f = docs.open(P, file.clone(), "a.rs".into(), String::new(), DocState::Loading);
        docs.get_mut(f.uid).expect("aba").state = DocState::Failed("x".into());
        docs.open(P, file, "a.rs".into(), String::new(), DocState::Loading);
        assert!(matches!(docs.get(f.uid).map(|t| &t.state), Some(DocState::Failed(_))));
    }

    #[test]
    fn a_read_that_arrives_after_switching_project_is_ignored() {
        let mut docs = Docs::default();
        let file = open(&mut docs, "p1", DocKind::File("a.rs".into())).uid;
        let md = open(&mut docs, "p1", DocKind::Markdown(MdSource::File("a.md".into()))).uid;
        let commit = open(&mut docs, "p1", DocKind::CommitDiff("abc".into())).uid;
        let changed = open(&mut docs, "p1", DocKind::FileDiff { path: "a.rs".into(), orig: None, staged: false, untracked: false }).uid;
        let task = open(&mut docs, "p1", DocKind::TaskDiff { id: "t1".into(), merged: false }).uid;
        for uid in [file, md, commit, changed] {
            assert!(docs.accepts_load(uid, "p1"));
            assert!(!docs.accepts_load(uid, "p2"), "lido na pasta do outro projeto");
        }
        // O diff da tarefa passa o projeto na chamada: vale de qualquer lugar.
        assert!(docs.accepts_load(task, "p2"));
        // Documento fechado: ignora.
        docs.close(file);
        assert!(!docs.accepts_load(file, "p1"));
    }

    #[test]
    fn what_was_discarded_while_loading_is_read_again_when_the_project_returns() {
        let mut docs = Docs::default();
        let file = open(&mut docs, "p1", DocKind::File("a.rs".into())).uid;
        let commit = open(&mut docs, "p1", DocKind::CommitDiff("abc".into())).uid;
        let done = open(&mut docs, "p1", DocKind::CommitDiff("def".into())).uid;
        docs.get_mut(done).expect("aba").state = DocState::Failed("x".into());
        open(&mut docs, "p1", DocKind::TaskDiff { id: "t1".into(), merged: false });
        open(&mut docs, "p2", DocKind::CommitDiff("ghi".into()));
        assert_eq!(docs.to_reread("p1"), [file, commit]);
    }

    #[test]
    fn badge_counts_finished_and_working_tabs() {
        assert!(Badge::of([]).is_empty());
        assert!(Badge::of([(false, false), (false, false)]).is_empty());
        assert_eq!(Badge::of([(true, false), (false, true), (true, false), (false, false)]), Badge { done: 2, busy: 1 });
    }

    const DIFF: &str = "diff --git a/src/a.rs b/src/a.rs
index 111..222 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,3 +1,4 @@
 fn a() {}
-let x = 1;
+let x = 2;
+let y = 3;
 fn b() {}
diff --git a/new file.txt b/new file.txt
new file mode 100644
index 0000000..333
--- /dev/null
+++ b/new file.txt
@@ -0,0 +1,2 @@
+um
+dois
diff --git a/old.txt b/old.txt
deleted file mode 100644
index 444..0000000
--- a/old.txt
+++ /dev/null
@@ -1 +0,0 @@
-adeus
diff --git a/antes.rs b/depois.rs
similarity index 90%
rename from antes.rs
rename to depois.rs
index 555..666 100644
--- a/antes.rs
+++ b/depois.rs
@@ -1 +1 @@
-a
+b
diff --git a/img.png b/img.png
index 777..888 100644
Binary files a/img.png and b/img.png differ
";

    #[test]
    fn the_file_list_comes_from_the_diff_headers() {
        let diff = parse_diff(DIFF, 1000);
        let files = diff_files(&diff);
        let got: Vec<(&str, char, usize, usize)> = files.iter().map(|f| (f.path.as_str(), f.status, f.added, f.removed)).collect();
        assert_eq!(
            got,
            [
                ("src/a.rs", 'M', 2, 1),
                ("new file.txt", 'A', 2, 0),
                ("old.txt", 'D', 0, 1),
                ("depois.rs", 'R', 1, 1),
                ("img.png", 'M', 0, 0),
            ]
        );
        // Cada arquivo aponta para a sua linha `diff --git`.
        for f in &files {
            assert!(diff.lines[f.line].text.starts_with("diff --git "));
        }
        assert_eq!(files[0].line, 0);
        assert!(diff.lines[files[1].line].text.contains("new file.txt"));
    }

    #[test]
    fn a_diff_without_files_has_an_empty_list() {
        assert!(diff_files(&GitDiff::default()).is_empty());
        assert!(diff_files(&parse_diff("", 10)).is_empty());
    }

    #[test]
    fn a_truncated_diff_lists_only_what_it_has() {
        let diff = parse_diff(DIFF, 12);
        assert!(diff.truncated);
        assert_eq!(diff_files(&diff).len(), 2);
    }

    #[test]
    fn a_combined_diff_names_the_file_from_its_header() {
        let text = "diff --cc conflito.txt\nindex 1,2..3\n--- a/conflito.txt\n+++ b/conflito.txt\n@@@ -1,1 -1,1 +1,5 @@@\n++<<<<<<< ours\n";
        let files = diff_files(&parse_diff(text, 100));
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "conflito.txt");
        assert_eq!(files[0].added, 1);
    }

    #[test]
    fn selecting_a_file_asks_the_diff_to_scroll_to_it() {
        let mut doc = DiffDoc::new(parse_diff(DIFF, 1000));
        assert_eq!((doc.selected, doc.jump), (0, None));
        doc.select(2);
        assert_eq!((doc.selected, doc.jump), (2, Some(doc.files[2].line)));
        doc.select(99);
        assert_eq!(doc.selected, 2, "índice fora da lista não muda nada");
    }

    fn tab(kind: DocKind) -> DocTab {
        DocTab { uid: 1, project: P.into(), kind, title: "t".into(), subtitle: String::new(), state: DocState::Loading }
    }

    #[test]
    fn a_loaded_file_becomes_text_or_markdown_by_kind() {
        let mut t = tab(DocKind::File("a.rs".into()));
        t.apply(Loaded::Text(Ok(TextFile::Text("um\n\tdois\n".into()))));
        let DocState::Text(doc) = &t.state else { panic!("texto") };
        assert_eq!(doc.lines, ["um", "    dois"]);
        assert_eq!(doc.max_chars, 8);

        let mut m = tab(DocKind::Markdown(MdSource::File("a.md".into())));
        m.apply(Loaded::Text(Ok(TextFile::Text("# Oi".into()))));
        assert!(matches!(&m.state, DocState::Markdown(t) if t == "# Oi"));
    }

    #[test]
    fn a_huge_line_is_cut_for_the_screen() {
        let doc = TextDoc::new(&format!("curta\n{}\n", "x".repeat(TEXT_LINE_MAX + 500)));
        assert_eq!(doc.lines[0], "curta");
        assert_eq!(doc.lines[1].chars().count(), TEXT_LINE_MAX + 1);
        assert!(doc.lines[1].ends_with('…'));
        assert_eq!(doc.max_chars, TEXT_LINE_MAX + 1);
    }

    #[test]
    fn big_binary_and_failed_files_explain_themselves() {
        let mut t = tab(DocKind::File("a.bin".into()));
        t.apply(Loaded::Text(Ok(TextFile::TooLarge(3 * 1024 * 1024))));
        assert!(matches!(&t.state, DocState::Unreadable(m) if m.contains("3,0 MB") && m.contains("1,0 MB")));
        t.apply(Loaded::Text(Ok(TextFile::Binary)));
        assert!(matches!(&t.state, DocState::Unreadable(m) if m.contains("binário")));
        t.apply(Loaded::Text(Err("não consegui abrir a.bin".into())));
        assert!(matches!(&t.state, DocState::Failed(m) if m.contains("a.bin")));
    }

    #[test]
    fn a_reloaded_diff_keeps_the_selected_file_when_it_still_exists() {
        let mut t = tab(DocKind::TaskDiff { id: "t1".into(), merged: false });
        t.apply(Loaded::Diff(Ok(parse_diff(DIFF, 1000))));
        let DocState::Diff(d) = &mut t.state else { panic!("diff") };
        d.select(3);
        t.apply(Loaded::Diff(Ok(parse_diff(DIFF, 1000))));
        assert!(matches!(&t.state, DocState::Diff(d) if d.selected == 3));
        t.apply(Loaded::Diff(Ok(parse_diff(&DIFF[..DIFF.find("diff --git a/new").unwrap_or(0)], 1000))));
        assert!(matches!(&t.state, DocState::Diff(d) if d.selected == 0 && d.files.len() == 1));
        t.apply(Loaded::Diff(Err("a branch sumiu".into())));
        assert!(matches!(&t.state, DocState::Failed(_)));
    }

    #[test]
    fn an_unchanged_file_does_not_need_repainting() {
        let mut t = tab(DocKind::File("a.rs".into()));
        t.apply(Loaded::Text(Ok(TextFile::Text("um\ndois".into()))));
        assert!(t.same_text(&Loaded::Text(Ok(TextFile::Text("um\ndois".into())))));
        assert!(!t.same_text(&Loaded::Text(Ok(TextFile::Text("um\ntres".into())))));
        assert!(!t.same_text(&Loaded::Text(Ok(TextFile::Binary))));
        assert!(!t.same_text(&Loaded::Diff(Ok(GitDiff::default()))));
    }

    #[test]
    fn sizes_are_written_in_portuguese() {
        assert_eq!(size_label(512), "512 bytes");
        assert_eq!(size_label(2048), "2 KB");
        assert_eq!(size_label(1_572_864), "1,5 MB");
    }

    #[test]
    fn the_file_name_is_the_last_component() {
        assert_eq!(file_name("crates/app/src/docs.rs"), "docs.rs");
        assert_eq!(file_name("a\\b\\c.md"), "c.md");
        assert_eq!(file_name("solto.txt"), "solto.txt");
    }
}
