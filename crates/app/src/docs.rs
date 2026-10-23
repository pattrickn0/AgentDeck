//! Modo Documentos do centro: o estado e a lógica pura (a pintura fica em `ui/docs.rs`).
//!
//! O centro alterna entre os agentes (a tela dividida) e os documentos: diff de uma tarefa, de um
//! commit ou de um arquivo, arquivo de texto e markdown. Os arquivos de texto pequenos se leem e se **editam** (`Source`:
//! `Ctrl+S` grava, com o mesmo fim de linha e sem acrescentar BOM; o aviso quando um agente muda o arquivo com edição
//! aberta). Os documentos são por projeto e não são gravados entre aberturas. Abrir um documento já aberto só o foca;
//! abrir qualquer um muda o centro para Documentos, e fechar o último volta para Agentes.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use agentdeck_core::git::{DiffKind, EntryKind, GitDiff, GitEntry, GitError};
use agentdeck_core::workspace::READ_TEXT_MAX;
use agentdeck_core::{Eol, TextFile};

use crate::deck::{Deck, UiMsg};
use crate::highlight::{self, Lang, Span};

/// A fonte e a altura de uma linha do arquivo de texto (a mesma do diff).
pub const TEXT_FONT: f32 = 12.0;
pub const TEXT_ROW_H: f32 = 17.0;

/// O que o desenho de um arquivo guarda entre quadros: o realce por versão do texto e o galley já montado. O `TextEdit`
/// pede o galley a cada quadro; sem este cache o arquivo todo seria percorrido, colorido e medido de novo 60 vezes por
/// segundo.
#[derive(Debug, Clone, Default)]
pub struct Shaped {
    pub hl: highlight::Cache,
    key: Option<ShapeKey>,
    galley: Option<Arc<egui::Galley>>,
}

/// Para que o galley guardado vale: o texto, a linguagem, a largura de quebra, a escala da tela e a busca.
#[derive(Debug, Clone, PartialEq)]
struct ShapeKey {
    hash: u64,
    lang: Lang,
    wrap: u32,
    ppp: u32,
    marks: Option<(String, u64, usize)>,
}

impl Shaped {
    /// O galley de `text` (monoespaçado, realçado, quebrando em `wrap` px), refeito só se algo da chave mudou.
    /// `marks` são as ocorrências da busca, em bytes de `text`, e `current` a atual entre elas.
    pub fn galley(&mut self, fonts: &egui::Context, text: &str, lang: Lang, wrap: f32, find: &Find) -> Arc<egui::Galley> {
        let hash = highlight::text_hash(text);
        let ppp = fonts.pixels_per_point();
        let marks = find.stamp().map(|(q, v)| (q.clone(), *v, find.current));
        let key = ShapeKey { hash, lang, wrap: wrap.to_bits(), ppp: ppp.to_bits(), marks };
        if let (Some(k), Some(g)) = (&self.key, &self.galley) {
            if *k == key {
                return g.clone();
            }
        }
        let pal = crate::theme::pal();
        let font = crate::theme::mono(TEXT_FONT);
        let matches: &[Range<usize>] = if key.marks.is_some() { &find.ranges } else { &[] };
        let pieces = highlight::pieces(text, self.hl.lines(lang, text, hash), matches, key.marks.as_ref().map(|_| find.current));
        let mut job = egui::text::LayoutJob::default();
        job.wrap.max_width = wrap;
        job.keep_trailing_whitespace = true;
        let format = |color, background| egui::TextFormat {
            font_id: font.clone(),
            color,
            background,
            line_height: Some(TEXT_ROW_H),
            ..Default::default()
        };
        for p in &pieces {
            let background = match p.mark {
                highlight::Mark::None => egui::Color32::TRANSPARENT,
                highlight::Mark::Found => crate::theme::with_alpha(pal.warning, 0.30),
                highlight::Mark::Current => crate::theme::with_alpha(pal.warning, 0.65),
            };
            job.append(&text[p.range.clone()], 0.0, format(pal.syn(p.tok, pal.text), background));
        }
        if pieces.is_empty() {
            job.append("", 0.0, format(pal.text, egui::Color32::TRANSPARENT));
        }
        let galley = fonts.fonts_mut(|f| f.layout_job(job));
        (self.key, self.galley) = (Some(key), Some(galley.clone()));
        galley
    }
}

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

    /// O arquivo de um diff de um arquivo só (o realce do código do diff).
    pub fn diff_path(&self) -> Option<&str> {
        match self {
            DocKind::FileDiff { path, .. } => Some(path),
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

/// As linhas de um arquivo de texto que não abre no editor (grande demais, linha comprida demais).
#[derive(Debug, Clone)]
pub struct TextDoc {
    pub lines: Vec<String>,
    /// A linha mais comprida, em caracteres (a largura da rolagem horizontal).
    pub max_chars: usize,
    /// O realce de cada linha (vazio sem linguagem); calculado uma vez, quando o arquivo carrega.
    pub spans: Vec<Vec<Span>>,
}

impl PartialEq for TextDoc {
    fn eq(&self, other: &Self) -> bool {
        self.lines == other.lines
    }
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
        Self { lines, max_chars, spans: Vec::new() }
    }

    /// Como [`TextDoc::new`], com o realce de `lang` (uma vez só, aqui: a pintura só lê).
    pub fn highlighted(text: &str, lang: Lang) -> Self {
        let mut doc = Self::new(text);
        if lang != Lang::Plain {
            doc.spans = highlight::highlight_lines(lang, &doc.lines.join("\n"));
        }
        doc
    }
}

/// Acima disto o arquivo abre só em leitura, sem editor: o desenho do editor refaz as letras de todo o texto e, com
/// vários MB, travaria a interface.
pub const EDIT_MAX: usize = 256 * 1024;
/// Uma linha mais comprida que isto (JSON minificado) também deixa o editor de fora.
const EDIT_LINE_MAX: usize = 4000;

/// Um arquivo de texto aberto como código: o texto que está na tela (que se edita, se o usuário pedir), o que está no
/// disco, e o que é preciso para gravar de volta igual. Também serve à leitura: o mesmo desenho dá seleção, cópia e busca.
#[derive(Debug, Clone)]
pub struct Source {
    /// O texto na tela: com `\n`, sem BOM.
    pub text: String,
    /// O que está no disco, no mesmo formato; `text != disk` é "edição não salva".
    pub disk: String,
    pub eol: Eol,
    pub bom: bool,
    pub lang: Lang,
    /// O usuário está editando (senão, só lendo).
    pub editing: bool,
    /// Markdown: o código em vez do texto formatado.
    pub raw: bool,
    /// Por que não dá para editar: gravar de volta mudaria o que o usuário não tocou.
    pub locked: Option<String>,
    /// O disco mudou com edição não salva: o texto de lá, esperando a escolha entre manter a minha ou recarregar.
    pub external: Option<String>,
    /// O arquivo sumiu (ou ficou ilegível) enquanto havia edição não salva: ela continua aqui.
    pub gone: bool,
    /// Uma gravação em andamento, e o erro da última que falhou.
    pub saving: bool,
    pub save_error: Option<String>,
    /// Muda a cada alteração do texto (a chave do que se guarda: realce, busca).
    pub version: u64,
}

/// O texto como o editor o guarda (sem BOM, com `\n`), o BOM e o fim de linha; e por que não dá para editar, se não dá.
fn read_source(raw: &str) -> (String, bool, Eol, Option<String>) {
    let (body, bom) = match raw.strip_prefix('\u{feff}') {
        Some(rest) => (rest, true),
        None => (raw, false),
    };
    let (text, eol, mut locked) = match Eol::normalize(body) {
        Some((text, eol)) => (text, eol, None),
        None => (
            body.replace("\r\n", "\n"),
            Eol::Lf,
            Some(
                "Este arquivo mistura fins de linha (CRLF e LF): gravar de volta mudaria linhas que você não tocou. Só leitura.".to_owned(),
            ),
        ),
    };
    if locked.is_none() && body.contains('\u{fffd}') {
        locked = Some("Este arquivo não é UTF-8 (há bytes que não viram texto): gravar de volta o estragaria. Só leitura.".to_owned());
    }
    (text, bom, eol, locked)
}

impl Source {
    /// O texto cabe no editor: não passa de `EDIT_MAX` e nenhuma linha passa de `EDIT_LINE_MAX`.
    pub fn fits(raw: &str) -> bool {
        raw.len() <= EDIT_MAX && !raw.split('\n').any(|l| l.len() > EDIT_LINE_MAX)
    }

    /// O arquivo como código, se cabe no editor (`EDIT_MAX`, sem linha comprida demais); senão `None`.
    pub fn open(raw: &str, lang: Lang) -> Option<Source> {
        if raw.len() > EDIT_MAX || raw.split('\n').any(|l| l.len() > EDIT_LINE_MAX) {
            return None;
        }
        let (text, bom, eol, locked) = read_source(raw);
        Some(Source {
            disk: text.clone(),
            text,
            eol,
            bom,
            lang,
            editing: false,
            raw: false,
            locked,
            external: None,
            gone: false,
            saving: false,
            save_error: None,
            version: 0,
        })
    }

    pub fn dirty(&self) -> bool {
        self.text != self.disk
    }

    /// `raw` (o que o disco tem agora) é o que já está em `disk`?
    pub fn is_disk(&self, raw: &str) -> bool {
        self.external.is_none() && !self.gone && read_source(raw).0 == self.disk
    }

    /// O arquivo foi lido de novo (o watcher avisou). Sem edição não salva, a tela acompanha o disco; com ela, nada do
    /// que o usuário digitou se perde: se o disco mudou de fato, o texto de lá espera em `external`.
    pub fn reload(&mut self, raw: &str) {
        let (text, bom, eol, locked) = read_source(raw);
        self.gone = false;
        if !self.dirty() {
            if text != self.text {
                self.version += 1;
            }
            (self.text, self.disk, self.bom, self.eol, self.locked) = (text.clone(), text, bom, eol, locked);
            self.external = None;
            return;
        }
        if text == self.disk {
            self.external = None;
        } else if text == self.text {
            // O agente escreveu o mesmo que o usuário: não há o que escolher.
            self.disk = text;
            self.external = None;
        } else {
            self.external = Some(text);
        }
    }

    /// "Manter a minha": o disco que mudou passa a ser a base; a edição continua e a próxima gravação a põe por cima.
    pub fn keep_mine(&mut self) {
        if let Some(ext) = self.external.take() {
            self.disk = ext;
        }
    }

    /// "Recarregar a do disco": a edição é descartada.
    pub fn take_disk(&mut self) {
        if let Some(ext) = self.external.take() {
            self.text = ext.clone();
            self.disk = ext;
            self.version += 1;
        }
    }

    /// Começa a editar (se pode). Markdown vai para o código enquanto se edita.
    pub fn set_editing(&mut self, on: bool) {
        if on && self.can_edit() {
            self.editing = true;
            self.raw = true;
        } else if !on {
            self.editing = false;
        }
    }

    /// Joga fora a edição: a tela volta ao que está no disco (ou ao texto novo que o disco trouxe).
    pub fn discard(&mut self) {
        self.text = self.external.take().unwrap_or_else(|| self.disk.clone());
        self.disk = self.text.clone();
        self.version += 1;
        self.save_error = None;
    }

    /// A gravação de `saved` deu certo.
    pub fn mark_saved(&mut self, saved: &str) {
        self.disk = saved.to_owned();
        self.external = None;
        self.gone = false;
        self.saving = false;
        self.save_error = None;
    }

    /// O texto foi editado (o editor devolveu mudança).
    pub fn touched(&mut self) {
        self.version += 1;
    }

    /// Dá para editar? (Não, se `locked`.)
    pub fn can_edit(&self) -> bool {
        self.locked.is_none()
    }
}

/// A busca do documento aberto (`Ctrl+F`).
#[derive(Debug, Clone, Default)]
pub struct Find {
    pub open: bool,
    pub query: String,
    /// A posição, nas ocorrências, da atual.
    pub current: usize,
    /// O campo pede o foco no próximo quadro.
    pub focus: bool,
    /// Rolar até a ocorrência atual no próximo quadro.
    pub jump: bool,
    /// Ocorrências em bytes do texto (arquivo no editor) ou linhas (diff, arquivo grande); só uma das duas se usa.
    pub ranges: Vec<Range<usize>>,
    pub lines: Vec<usize>,
    /// Para que consulta e versão do texto as ocorrências valem.
    key: Option<(String, u64)>,
}

impl Find {
    pub fn show(&mut self) {
        self.open = true;
        self.focus = true;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.focus = false;
    }

    pub fn count(&self) -> usize {
        self.ranges.len() + self.lines.len()
    }

    /// Refaz as ocorrências se a consulta ou a versão do texto mudaram (`search` devolve bytes e linhas). Uma consulta
    /// nova leva à primeira ocorrência; editar o texto não rola a tela.
    pub fn refresh(&mut self, version: u64, search: impl FnOnce(&str) -> (Vec<Range<usize>>, Vec<usize>)) {
        let same_query = self.key.as_ref().is_some_and(|(q, _)| *q == self.query);
        if self.key.as_ref().is_some_and(|(q, v)| *q == self.query && *v == version) {
            return;
        }
        (self.ranges, self.lines) = search(&self.query);
        self.key = Some((self.query.clone(), version));
        if !same_query {
            self.current = 0;
            self.jump = true;
        } else if self.current >= self.count() {
            self.current = 0;
        }
    }

    /// A próxima (ou a anterior) ocorrência, dando a volta.
    pub fn step(&mut self, back: bool) {
        let n = self.count();
        if n > 0 {
            self.current = (self.current + if back { n - 1 } else { 1 }) % n;
            self.jump = true;
        }
    }

    /// De que consulta e versão do texto as ocorrências são (a chave do que depende delas, como o galley).
    pub fn stamp(&self) -> Option<&(String, u64)> {
        self.key.as_ref().filter(|_| self.open && !self.query.is_empty())
    }

    /// A ocorrência atual, em bytes (arquivo no editor).
    pub fn current_range(&self) -> Option<&Range<usize>> {
        self.ranges.get(self.current)
    }

    /// A linha da ocorrência atual (diff, arquivo grande).
    pub fn current_line(&self) -> Option<usize> {
        self.lines.get(self.current).copied()
    }
}

#[derive(Debug, Clone)]
pub enum DocState {
    Loading,
    Failed(String),
    Diff(DiffDoc),
    Text(TextDoc),
    /// Um arquivo de texto pequeno, como código: lê, seleciona, busca e edita.
    Source(Source),
    /// O texto markdown, desenhado já formatado (pedido e relatório da tarefa).
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
    pub find: Find,
    pub shaped: Shaped,
}

impl DocTab {
    pub fn new(uid: u64, project: String, kind: DocKind, title: String, subtitle: String, state: DocState) -> Self {
        Self { uid, project, kind, title, subtitle, state, find: Find::default(), shaped: Shaped::default() }
    }

    /// Há edição não salva.
    pub fn dirty(&self) -> bool {
        matches!(&self.state, DocState::Source(s) if s.dirty())
    }

    /// A linguagem do realce, pelo nome do arquivo.
    pub fn lang(&self) -> Lang {
        self.kind.file_path().map_or(Lang::Plain, Lang::from_path)
    }

    /// Aplica o resultado de uma leitura. O estado vira o que o tipo do documento mostra; o arquivo
    /// selecionado e a rolagem de um diff relido não se perdem (o `selected` só é mantido se ainda existe). Um arquivo
    /// aberto como código com edição não salva nunca é trocado por uma leitura: ela só vira aviso (`Source::reload`).
    pub fn apply(&mut self, loaded: Loaded) {
        if let DocState::Source(src) = &mut self.state {
            match &loaded {
                // Um arquivo que cresceu além do editor só segue o disco se não há edição a proteger.
                Loaded::Text(Ok(TextFile::Text(raw))) if src.dirty() || Source::fits(raw) => return src.reload(raw),
                Loaded::Text(_) if src.dirty() => {
                    src.gone = true;
                    return;
                }
                _ => {}
            }
        }
        let previous = match &self.state {
            DocState::Diff(d) => Some(d.selected),
            _ => None,
        };
        let lang = self.lang();
        let is_markdown = matches!(self.kind, DocKind::Markdown(_));
        let from_file = self.kind.file_path().is_some();
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
            Loaded::Text(Ok(TextFile::Text(text))) => match Source::open(&text, lang).filter(|_| from_file) {
                Some(source) => DocState::Source(source),
                None if is_markdown => DocState::Markdown(text),
                None => DocState::Text(TextDoc::highlighted(&text, lang)),
            },
        };
    }

    /// O texto já está na tela igual ao que chegou (a releitura do watcher não precisa repintar nada).
    pub fn same_text(&self, loaded: &Loaded) -> bool {
        match (&self.state, loaded) {
            (DocState::Source(old), Loaded::Text(Ok(TextFile::Text(new)))) => old.is_disk(new),
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
    /// A pergunta aberta sobre edição não salva (fechar a aba ou o app).
    pub confirm: Option<Unsaved>,
    /// Gravações em andamento e o que fazer quando terminarem.
    pub pending: HashMap<u64, After>,
}

/// O que a pergunta sobre edição não salva decide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unsaved {
    /// Fechar a aba do documento (uid).
    Tab(u64),
    /// Fechar o app.
    Quit,
    /// Descartar a edição e ler o arquivo do disco (o botão Recarregar).
    Revert(u64),
}

/// O que fazer depois que uma gravação der certo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum After {
    Nothing,
    /// Fechar a aba.
    Close,
    /// Sair do app (quando não sobrar nada por gravar).
    Quit,
}

/// A resposta do usuário à pergunta sobre edição não salva.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Save,
    Discard,
    Cancel,
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
        self.tabs.push(DocTab::new(uid, project.to_owned(), kind, title, subtitle, state));
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

    /// Os documentos com edição não salva, de todos os projetos.
    pub fn unsaved(&self) -> Vec<&DocTab> {
        self.tabs.iter().filter(|t| t.dirty()).collect()
    }

    /// Alguma gravação ainda não voltou.
    pub fn saving(&self) -> bool {
        self.tabs.iter().any(|t| matches!(&t.state, DocState::Source(s) if s.saving))
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

    /// Fecha um documento (se era o último do projeto, o centro volta para os agentes). Sem perguntar: quem fecha pela
    /// interface usa [`Deck::docs_request_close`].
    pub fn docs_close(&mut self, uid: u64) {
        self.docs.close(uid);
        if self.docs.confirm == Some(Unsaved::Tab(uid)) || self.docs.confirm == Some(Unsaved::Revert(uid)) {
            self.docs.confirm = None;
        }
        self.docs.pending.remove(&uid);
    }

    /// O pedido de fechar um documento (o ×): com edição não salva, pergunta antes.
    pub fn docs_request_close(&mut self, uid: u64) {
        if self.docs.get(uid).is_some_and(DocTab::dirty) {
            self.docs.focus(uid);
            self.docs.confirm = Some(Unsaved::Tab(uid));
        } else {
            self.docs_close(uid);
        }
    }

    /// O pedido de fechar o app (o × da janela): com edição não salva, pergunta antes.
    pub fn request_quit(&mut self) {
        let first = self.docs.unsaved().first().map(|t| (t.uid, t.project.clone()));
        let Some((uid, project)) = first else {
            self.quit_requested = true;
            return;
        };
        // Mostra o primeiro documento com edição, se é do projeto à vista.
        if project == self.active_project {
            self.docs.focus(uid);
            self.docs.view = CenterView::Docs;
        }
        self.docs.confirm = Some(Unsaved::Quit);
    }

    /// Lê o documento de novo (botão de recarregar). Um arquivo aberto como código não passa por "Carregando…": se tem
    /// edição não salva, pergunta antes de jogá-la fora; senão relê e a tela acompanha o disco.
    pub fn docs_reload(&mut self, uid: u64) {
        let Some(tab) = self.docs.get_mut(uid).filter(|t| t.kind.reloadable()) else { return };
        if tab.dirty() {
            self.docs.confirm = Some(Unsaved::Revert(uid));
        } else if matches!(tab.state, DocState::Source(_)) {
            self.docs_load(uid);
        } else {
            tab.state = DocState::Loading;
            self.docs_load(uid);
        }
    }

    /// `Ctrl+F`: abre a busca do documento. O markdown formatado não tem onde marcar o achado: a busca mostra o código.
    pub fn docs_find(&mut self, uid: u64) {
        let Some(tab) = self.docs.get_mut(uid) else { return };
        if let DocState::Source(src) = &mut tab.state {
            src.raw = true;
        }
        if matches!(tab.state, DocState::Source(_) | DocState::Text(_) | DocState::Diff(_)) {
            tab.find.show();
        }
    }

    /// Grava a edição de `uid` (`Ctrl+S`). `after` é o que fazer quando a gravação der certo. Não grava com o disco
    /// mudado e a escolha (manter a minha / recarregar) por fazer: o aviso do documento pede isso antes.
    pub fn docs_save(&mut self, uid: u64, after: After) {
        let Some(tab) = self.docs.get_mut(uid) else { return };
        let (Some(rel), project) = (tab.kind.file_path().map(str::to_owned), tab.project.clone()) else { return };
        let DocState::Source(src) = &mut tab.state else { return };
        if src.saving || src.locked.is_some() || src.external.is_some() {
            return;
        }
        if !src.dirty() {
            return self.docs_finish(uid, after);
        }
        src.saving = true;
        src.save_error = None;
        let (text, eol, bom) = (src.text.clone(), src.eol, src.bom);
        self.docs.pending.insert(uid, after);
        self.spawn_bg("doc-save", move |core| {
            let result = core.write_text(&project, &rel, &text, eol, bom);
            Some(UiMsg::DocSaved(uid, text, result))
        });
    }

    /// Grava todas as edições (a pergunta de fechar o app) e sai quando a última terminar. Se alguma não pode ser gravada
    /// (o disco mudou e falta escolher), o app não sai: o documento é mostrado.
    pub fn docs_save_all_and_quit(&mut self) {
        let blocked = self.docs.unsaved().iter().find(|t| matches!(&t.state, DocState::Source(s) if s.external.is_some())).map(|t| t.uid);
        if let Some(uid) = blocked {
            self.docs.focus(uid);
            self.docs.view = CenterView::Docs;
            return;
        }
        let dirty: Vec<u64> = self.docs.unsaved().iter().map(|t| t.uid).collect();
        for uid in dirty {
            self.docs_save(uid, After::Quit);
        }
    }

    /// A gravação de `uid` voltou. `saved` é o texto que foi gravado (o da tela pode ter mudado desde então).
    pub fn on_doc_saved(&mut self, uid: u64, saved: String, result: Result<(), String>) {
        let after = self.docs.pending.remove(&uid).unwrap_or(After::Nothing);
        let Some(tab) = self.docs.get_mut(uid) else { return };
        let DocState::Source(src) = &mut tab.state else { return };
        match result {
            Ok(()) => src.mark_saved(&saved),
            Err(e) => {
                src.saving = false;
                src.save_error = Some(e);
                self.docs.focus(uid);
                self.docs.view = CenterView::Docs;
                return;
            }
        }
        self.docs_finish(uid, after);
    }

    /// O que vem depois de uma gravação (ou de um "nada a gravar").
    fn docs_finish(&mut self, uid: u64, after: After) {
        match after {
            After::Nothing => {}
            After::Close => {
                // Digitou mais enquanto gravava: a aba fica, com o que falta gravar.
                if self.docs.get(uid).is_some_and(|t| !t.dirty()) {
                    self.docs_close(uid);
                }
            }
            After::Quit => {
                if self.docs.unsaved().is_empty() && !self.docs.saving() {
                    self.quit_requested = true;
                }
            }
        }
    }

    /// A resposta da pergunta sobre edição não salva.
    pub fn docs_answer(&mut self, answer: Answer) {
        let Some(asked) = self.docs.confirm else { return };
        if answer == Answer::Cancel {
            self.docs.confirm = None;
            return;
        }
        match (asked, answer) {
            (Unsaved::Tab(uid), Answer::Save) => {
                self.docs.confirm = None;
                self.docs_save(uid, After::Close);
            }
            (Unsaved::Tab(uid), Answer::Discard) => self.docs_close(uid),
            (Unsaved::Quit, Answer::Save) => {
                self.docs.confirm = None;
                self.docs_save_all_and_quit();
            }
            (Unsaved::Quit, Answer::Discard) => {
                self.docs.confirm = None;
                self.quit_requested = true;
            }
            (Unsaved::Revert(uid), Answer::Discard | Answer::Save) => {
                self.docs.confirm = None;
                if let Some(DocState::Source(src)) = self.docs.get_mut(uid).map(|t| &mut t.state) {
                    src.discard();
                }
                self.docs_load(uid);
            }
            (_, Answer::Cancel) => {}
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
        DocTab::new(1, P.into(), kind, "t".into(), String::new(), DocState::Loading)
    }

    #[test]
    fn a_loaded_file_becomes_text_or_markdown_by_kind() {
        let mut t = tab(DocKind::File("a.rs".into()));
        t.apply(Loaded::Text(Ok(TextFile::Text("um\n\tdois\n".into()))));
        // Um arquivo que cabe no editor abre como código (lê, busca e edita)...
        let DocState::Source(src) = &t.state else { panic!("código") };
        assert_eq!((src.text.as_str(), src.lang, src.editing), ("um\n\tdois\n", Lang::Rust, false));
        // ...e o grande demais, ou com linha comprida demais, só se lê.
        let big = "x\n".repeat(EDIT_MAX / 2 + 1);
        t.apply(Loaded::Text(Ok(TextFile::Text(big))));
        assert!(matches!(&t.state, DocState::Text(d) if d.lines.len() > 1000));
        t.apply(Loaded::Text(Ok(TextFile::Text(format!("curta\n{}\n", "y".repeat(EDIT_LINE_MAX + 1))))));
        let DocState::Text(doc) = &t.state else { panic!("texto") };
        assert_eq!(doc.lines[0], "curta");
        t.apply(Loaded::Text(Ok(TextFile::Text("um\n\tdois\n".into()))));
        assert!(matches!(&t.state, DocState::Source(_)));

        let mut m = tab(DocKind::Markdown(MdSource::File("a.md".into())));
        m.apply(Loaded::Text(Ok(TextFile::Text("# Oi".into()))));
        let DocState::Source(md) = &m.state else { panic!("markdown como código") };
        assert_eq!((md.text.as_str(), md.lang), ("# Oi", Lang::Markdown));
        // O pedido e o relatório de uma tarefa não vêm de arquivo: seguem como markdown formatado.
        let mut brief = tab(DocKind::Markdown(MdSource::TaskBrief("t1".into())));
        brief.apply(Loaded::Text(Ok(TextFile::Text("# Oi".into()))));
        assert!(matches!(&brief.state, DocState::Markdown(t) if t == "# Oi"));
    }

    fn source(raw: &str) -> Source {
        Source::open(raw, Lang::Plain).expect("cabe no editor")
    }

    #[test]
    fn the_line_ending_and_the_bom_come_back_as_they_were() {
        let s = source("\u{feff}um\r\ndois\r\n");
        assert_eq!((s.text.as_str(), s.eol, s.bom, s.locked.is_none()), ("um\ndois\n", Eol::Crlf, true, true));
        let s = source("um\ndois");
        assert_eq!((s.eol, s.bom), (Eol::Lf, false));
        // Fim de linha misturado, ou texto que não é UTF-8: só leitura, com a razão.
        assert!(source("a\r\nb\n").locked.as_deref().is_some_and(|m| m.contains("mistura")));
        assert!(source("a\u{fffd}b").locked.as_deref().is_some_and(|m| m.contains("UTF-8")));
        assert!(!source("a\u{fffd}b").can_edit());
    }

    #[test]
    fn a_big_file_or_a_long_line_does_not_open_in_the_editor() {
        assert!(Source::open(&"a\n".repeat(EDIT_MAX / 2), Lang::Plain).is_some());
        assert!(Source::open(&format!("{}a", "a\n".repeat(EDIT_MAX / 2)), Lang::Plain).is_none());
        assert!(Source::open(&format!("ok\n{}", "a".repeat(EDIT_LINE_MAX + 1)), Lang::Plain).is_none());
    }

    #[test]
    fn without_unsaved_edits_the_screen_follows_the_disk() {
        let mut s = source("um\n");
        let v = s.version;
        s.reload("um\n");
        assert_eq!(s.version, v, "o mesmo texto não conta como mudança");
        s.reload("dois\n");
        assert_eq!((s.text.as_str(), s.disk.as_str(), s.external.is_none()), ("dois\n", "dois\n", true));
        assert!(s.version > v);
        assert!(!s.dirty());
    }

    #[test]
    fn an_agent_changing_the_file_during_an_edit_asks_instead_of_replacing() {
        let mut s = source("um\n");
        s.set_editing(true);
        s.text = "um\nminha linha\n".into();
        s.touched();
        assert!(s.dirty());
        // O disco ainda é o mesmo: nada a avisar (é o que a leitura do watcher traz depois de gravar).
        s.reload("um\n");
        assert!(s.external.is_none());
        assert!(s.is_disk("um\n"));
        // O agente mudou: o texto dele espera; o meu continua intacto.
        s.reload("um\nlinha do agente\n");
        assert_eq!(s.external.as_deref(), Some("um\nlinha do agente\n"));
        assert_eq!(s.text, "um\nminha linha\n");
        assert!(!s.is_disk("um\nlinha do agente\n"), "a releitura repete o aviso em vez de ser ignorada");
        // Manter a minha: o disco novo vira a base e a edição continua para gravar por cima.
        s.keep_mine();
        assert_eq!((s.disk.as_str(), s.text.as_str(), s.external.is_none()), ("um\nlinha do agente\n", "um\nminha linha\n", true));
        assert!(s.dirty());
        // Recarregar a do disco: a edição se descarta.
        s.reload("outra\n");
        s.take_disk();
        assert_eq!((s.text.as_str(), s.disk.as_str(), s.dirty()), ("outra\n", "outra\n", false));
    }

    #[test]
    fn an_agent_writing_what_the_user_typed_needs_no_choice() {
        let mut s = source("um\n");
        s.text = "dois\n".into();
        s.reload("dois\n");
        assert!(s.external.is_none() && !s.dirty());
    }

    #[test]
    fn a_save_that_finished_while_typing_leaves_the_rest_unsaved() {
        let mut s = source("um\n");
        s.text = "dois\n".into();
        s.saving = true;
        // O usuário digitou mais enquanto a gravação de "dois" andava.
        s.text = "dois e meio\n".into();
        s.mark_saved("dois\n");
        assert!(!s.saving && s.dirty(), "o que veio depois ainda falta gravar");
        s.mark_saved("dois e meio\n");
        assert!(!s.dirty());
    }

    #[test]
    fn discarding_goes_back_to_the_disk() {
        let mut s = source("um\n");
        s.text = "x\n".into();
        s.save_error = Some("falhou".into());
        s.discard();
        assert_eq!((s.text.as_str(), s.dirty(), s.save_error.is_none()), ("um\n", false, true));
    }

    #[test]
    fn a_dirty_file_is_never_replaced_by_a_read() {
        let mut t = tab(DocKind::File("a.rs".into()));
        t.apply(Loaded::Text(Ok(TextFile::Text("um\n".into()))));
        let DocState::Source(src) = &mut t.state else { panic!("código") };
        src.set_editing(true);
        src.text = "meu\n".into();
        // O arquivo foi apagado, ficou binário ou grande demais: a edição fica e o aviso aparece.
        for gone in [Loaded::Text(Err("sumiu".into())), Loaded::Text(Ok(TextFile::Binary)), Loaded::Text(Ok(TextFile::TooLarge(9)))] {
            t.apply(gone);
            let DocState::Source(src) = &t.state else { panic!("a edição continua") };
            assert!(src.gone && src.text == "meu\n" && src.editing);
        }
        // Voltou: o aviso some e o disco novo vira o aviso de "mudou".
        t.apply(Loaded::Text(Ok(TextFile::Text("novo\n".into()))));
        let DocState::Source(src) = &t.state else { panic!("código") };
        assert!(!src.gone && src.external.as_deref() == Some("novo\n") && src.text == "meu\n");
        // A mesma leitura de novo não repinta quando a tela já a mostra (sem edição) e repete o aviso com ela.
        assert!(!t.same_text(&Loaded::Text(Ok(TextFile::Text("novo\n".into())))));
    }

    #[test]
    fn a_tab_with_unsaved_edits_says_so() {
        let mut t = tab(DocKind::File("a.rs".into()));
        assert!(!t.dirty());
        t.apply(Loaded::Text(Ok(TextFile::Text("um\n".into()))));
        assert!(!t.dirty());
        if let DocState::Source(s) = &mut t.state {
            s.text.push('x');
        }
        assert!(t.dirty());
        let mut docs = Docs::default();
        docs.tabs.push(t);
        assert_eq!(docs.unsaved().len(), 1);
    }

    #[test]
    fn the_search_walks_through_the_matches_and_wraps_around() {
        let mut f = Find::default();
        f.show();
        f.query = "ab".into();
        f.refresh(1, |q| (highlight::find_all("AB ab xab", q), Vec::new()));
        assert_eq!((f.count(), f.current, f.jump), (3, 0, true));
        f.jump = false;
        f.step(false);
        f.step(false);
        assert_eq!(f.current_range(), Some(&(7..9)));
        f.step(false);
        assert_eq!(f.current, 0, "dá a volta");
        f.step(true);
        assert_eq!(f.current, 2);
        // A mesma consulta e versão não refaz; editar o texto refaz sem rolar a tela.
        f.jump = false;
        f.refresh(1, |_| panic!("não devia buscar de novo"));
        f.refresh(2, |q| (highlight::find_all("ab", q), Vec::new()));
        assert_eq!((f.count(), f.current, f.jump), (1, 0, false));
        // Uma consulta nova volta à primeira ocorrência e rola até ela.
        f.query = "b".into();
        f.refresh(2, |q| (highlight::find_all("ab b", q), Vec::new()));
        assert_eq!((f.count(), f.current, f.jump), (2, 0, true));
        assert_eq!(f.stamp().map(|(q, v)| (q.as_str(), *v)), Some(("b", 2)));
        f.close();
        assert!(f.stamp().is_none(), "fechada, a busca não marca nada");
    }

    #[test]
    fn the_search_by_lines_serves_the_diff_and_the_big_file() {
        let mut f = Find::default();
        f.show();
        f.query = "oi".into();
        f.refresh(0, |q| (Vec::new(), highlight::find_lines(["oi", "x", "Oi!"], q)));
        assert_eq!((f.count(), f.current_line()), (2, Some(0)));
        f.step(false);
        assert_eq!(f.current_line(), Some(2));
    }

    #[test]
    fn the_shaped_text_is_rebuilt_only_when_the_text_the_width_or_the_search_change() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let mut out = ctx.run_ui(egui::RawInput::default(), |_| {});
        out.textures_delta.clear();
        let mut shaped = Shaped::default();
        let mut find = Find::default();
        let a = shaped.galley(&ctx, "fn a() {}\nlet x = 1;", Lang::Rust, 600.0, &find);
        let again = shaped.galley(&ctx, "fn a() {}\nlet x = 1;", Lang::Rust, 600.0, &find);
        assert!(Arc::ptr_eq(&a, &again), "o mesmo texto reaproveita o galley");
        assert_eq!(shaped.hl.runs, 1);
        let wider = shaped.galley(&ctx, "fn a() {}\nlet x = 1;", Lang::Rust, 800.0, &find);
        assert!(!Arc::ptr_eq(&a, &wider));
        assert_eq!(shaped.hl.runs, 1, "a largura não refaz o realce");
        let edited = shaped.galley(&ctx, "fn b() {}\nlet x = 1;", Lang::Rust, 800.0, &find);
        assert!(!Arc::ptr_eq(&wider, &edited));
        assert_eq!(shaped.hl.runs, 2);
        // A busca aberta marca as ocorrências (outro galley); fechada, volta ao de antes.
        find.show();
        find.query = "x".into();
        find.refresh(1, |q| (highlight::find_all("fn b() {}\nlet x = 1;", q), Vec::new()));
        let marked = shaped.galley(&ctx, "fn b() {}\nlet x = 1;", Lang::Rust, 800.0, &find);
        assert!(!Arc::ptr_eq(&edited, &marked));
        assert_eq!(shaped.hl.runs, 2, "a busca não refaz o realce");
        // Todo o texto está no galley, sem perder nem repetir nada.
        assert_eq!(marked.job.text, "fn b() {}\nlet x = 1;");
    }

    #[test]
    fn the_editor_shows_up_to_a_quarter_megabyte_and_lines_up_to_four_thousand_bytes() {
        assert_eq!((EDIT_MAX, EDIT_LINE_MAX), (256 * 1024, 4000));
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
