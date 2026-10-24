//! Terminais das abas: emulação VT (alacritty_terminal) + pseudo-terminal (portable-pty).
//!
//! Cada aba tem uma [`Session`]. A thread leitora do PTY alimenta o parser sob um mutex e
//! acorda o loop do winit; a UI só lê a grade para pintar (ver [`view`]). A escrita no processo
//! vai por uma fila com thread própria (`pty-write`): nem a UI nem a leitora bloqueiam escrevendo.

pub mod colors;
pub mod input;
pub mod record;
pub mod resize;
pub mod view;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use agentdeck_core::SpawnSpec;
use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::{Config as TermConfig, Term};
use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};
use crossbeam_channel::Sender;
use parking_lot::Mutex;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize};

/// Linhas de histórico (como `scrollback: 10000` do xterm).
pub const SCROLLBACK: usize = 10_000;

/// Função que acorda o loop da UI (pode ser chamada de qualquer thread).
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// Dimensões da grade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridSize {
    pub cols: usize,
    pub rows: usize,
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// Fila de escrita do processo atual. Quem escreve (interface e respostas do emulador) só
/// enfileira; a thread `pty-write` da aba faz a escrita, que bloqueia enquanto o programa não lê
/// (ex.: colagem grande). Uma fila só por aba mantém a ordem das teclas, colagens e Enters.
type SharedWriter = Arc<Mutex<Option<Sender<Vec<u8>>>>>;

/// Enfileira bytes para o processo (no-op se não houver processo). Nunca bloqueia.
fn enqueue(writer: &SharedWriter, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    if let Some(tx) = writer.lock().as_ref() {
        let _ = tx.send(bytes.to_vec());
    }
}

/// Recebe eventos do emulador (chamado com o terminal travado — não pode travá-lo de novo).
#[derive(Clone)]
pub struct Listener {
    writer: SharedWriter,
    bell: Arc<AtomicBool>,
    /// (colunas, linhas, largura da célula, altura da célula) em pixels.
    window_size: Arc<Mutex<WindowSize>>,
    /// Texto pedido via OSC 52 para ir à área de transferência.
    clipboard: Arc<Mutex<Option<String>>>,
}

impl Listener {
    /// Respostas ao programa (DA/DSR/cores): pela fila, para a leitora nunca bloquear escrevendo.
    fn write(&self, s: &str) {
        enqueue(&self.writer, s.as_bytes());
    }
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(s) => self.write(&s),
            Event::Bell => self.bell.store(true, Ordering::Relaxed),
            Event::ColorRequest(index, fmt) => self.write(&fmt(colors::default_for_index(index))),
            Event::TextAreaSizeRequest(fmt) => {
                let ws = *self.window_size.lock();
                self.write(&fmt(ws));
            }
            Event::ClipboardStore(_, text) => *self.clipboard.lock() = Some(text),
            _ => {}
        }
    }
}

/// Emulador: grade + parser.
pub struct Emulator {
    pub term: Term<Listener>,
    parser: Processor<StdSyncHandler>,
    /// Cópia do listener para recriar o `Term` no `reset()`.
    listener: Listener,
}

impl Emulator {
    fn new(size: GridSize, listener: Listener) -> Self {
        let config = TermConfig { scrolling_history: SCROLLBACK, ..TermConfig::default() };
        Self { term: Term::new(config, &size, listener.clone()), parser: Processor::new(), listener }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.expire_sync(Instant::now());
        self.parser.advance(&mut self.term, bytes);
    }

    /// Muda o tamanho da grade (no Windows, como o ConPTY espera: ver [`resize`]).
    pub fn resize(&mut self, size: GridSize) {
        self.resize_as(size, cfg!(windows));
    }

    /// [`Emulator::resize`] com a escolha explícita (os testes reproduzem gravações do ConPTY em qualquer sistema).
    pub fn resize_as(&mut self, size: GridSize, keep_top: bool) {
        resize::resize(&mut self.term, size, keep_top);
    }
    /// Atualização sincronizada (`ESC[?2026h`, usada pelo Claude Code): o parser retém a saída até o
    /// `ESC[?2026l`, mas o prazo de 150 ms fica a cargo de quem o usa (no Alacritty, o laço do PTY).
    /// Sem isto, um fim perdido (ex.: no redesenho do ConPTY ao redimensionar) congela a tela.
    /// Solta o que estava retido se o prazo venceu; devolve o prazo se ainda falta.
    pub fn expire_sync(&mut self, now: Instant) -> Option<Instant> {
        let at = self.parser.sync_timeout().sync_timeout()?;
        if now < at {
            return Some(at);
        }
        self.parser.stop_sync(&mut self.term);
        None
    }
}

/// Estado compartilhado entre a UI e as threads do PTY.
pub struct Shared {
    pub emu: Mutex<Emulator>,
    writer: SharedWriter,
    bell: Arc<AtomicBool>,
    window_size: Arc<Mutex<WindowSize>>,
    clipboard: Arc<Mutex<Option<String>>>,
    /// Incrementado a cada bloco de saída do processo.
    pub output_seq: AtomicU64,
    pub last_output: Mutex<Option<Instant>>,
    /// (geração, código de saída) quando o processo termina.
    exit: Mutex<Option<(u64, i64)>>,
    /// Gravação da saída crua (só com `AGENTDECK_DEBUG_PTY=1`, ver [`record`]).
    recorder: Mutex<Option<record::Recorder>>,
}

impl Shared {
    /// Escreve no processo atual (no-op se não houver processo). Só enfileira: nunca bloqueia.
    pub fn write_input(&self, bytes: &[u8]) {
        enqueue(&self.writer, bytes);
    }

    /// Liga `w` à fila de escrita, com uma thread própria que escreve na ordem de chegada. A
    /// thread termina quando a fila é trocada ou solta (`kill`, fim do processo) ou a escrita falha.
    fn attach_writer(&self, mut w: Box<dyn Write + Send>, name: String) -> std::io::Result<()> {
        let (tx, rx) = crossbeam_channel::unbounded::<Vec<u8>>();
        std::thread::Builder::new().name(name).spawn(move || {
            while let Ok(mut bytes) = rx.recv() {
                // Junta o que já está na fila: menos escritas numa colagem em pedaços.
                while let Ok(more) = rx.try_recv() {
                    bytes.extend_from_slice(&more);
                }
                if w.write_all(&bytes).and_then(|()| w.flush()).is_err() {
                    break;
                }
            }
        })?;
        *self.writer.lock() = Some(tx);
        Ok(())
    }
}

/// OSC 52 (o programa pede para gravar na área de transferência) só vale para a aba ativa e
/// visível, do projeto ativo: uma aba em segundo plano não troca o que o usuário copiou.
pub fn clipboard_allowed(tab: usize, active: Option<usize>, tab_project: &str, active_project: &str) -> bool {
    active == Some(tab) && tab_project == active_project
}

struct PtyHandle {
    master: Box<dyn MasterPty + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
}

/// Terminal de uma aba.
pub struct Session {
    pub shared: Arc<Shared>,
    pty: Option<PtyHandle>,
    generation: u64,
    size: GridSize,
    /// Pasta e nome da gravação de depuração (ver [`Session::record_to`]).
    record_to: Option<(PathBuf, String)>,
}

impl Session {
    pub fn new(size: GridSize) -> Self {
        let writer: SharedWriter = Arc::new(Mutex::new(None));
        let bell = Arc::new(AtomicBool::new(false));
        let window_size =
            Arc::new(Mutex::new(WindowSize { num_lines: size.rows as u16, num_cols: size.cols as u16, cell_width: 8, cell_height: 17 }));
        let clipboard = Arc::new(Mutex::new(None));
        let listener =
            Listener { writer: writer.clone(), bell: bell.clone(), window_size: window_size.clone(), clipboard: clipboard.clone() };
        let shared = Arc::new(Shared {
            emu: Mutex::new(Emulator::new(size, listener)),
            writer,
            bell,
            window_size,
            clipboard,
            output_seq: AtomicU64::new(0),
            last_output: Mutex::new(None),
            exit: Mutex::new(None),
            recorder: Mutex::new(None),
        });
        Self { shared, pty: None, generation: 0, size, record_to: None }
    }

    /// Liga a gravação da saída crua dos próximos processos desta aba em `dir`, com `name` no nome dos arquivos.
    pub fn record_to(&mut self, dir: PathBuf, name: String) {
        self.record_to = Some((dir, name));
    }

    /// A gravação de diagnóstico desta aba está ligada (variável de ambiente ou item do menu da aba).
    pub fn recording(&self) -> bool {
        self.record_to.is_some()
    }

    /// Liga (`Some`) ou desliga (`None`) a gravação desta aba. Ligada com o processo rodando, começa agora: o `.bin` só tem
    /// o que vier depois, e a grade de agora vai para o `.grid.txt` (não dá para reproduzir o que veio antes).
    pub fn set_recording(&mut self, to: Option<(PathBuf, String)>) {
        self.record_to = to;
        let now = self
            .record_to
            .as_ref()
            .filter(|_| self.pty.is_some())
            .and_then(|(dir, name)| record::Recorder::start(dir, name, self.size).ok());
        let text = now.as_ref().map(|_| record::grid_text(&self.shared.emu.lock()));
        if let (Some(r), Some(text)) = (&now, text) {
            r.snapshot(&text);
        }
        *self.shared.recorder.lock() = now;
    }

    /// Salva a grade (tela e fim do histórico) agora, em `dir`; devolve o arquivo. Com a gravação ligada reescreve o
    /// `.grid.txt` dela.
    pub fn save_snapshot(&self, dir: &Path, name: &str) -> std::io::Result<PathBuf> {
        let text = record::grid_text(&self.shared.emu.lock());
        if let Some(r) = self.shared.recorder.lock().as_ref() {
            r.snapshot(&text);
            return Ok(r.grid_path().to_owned());
        }
        record::write_snapshot(dir, name, &text)
    }

    /// Tira a foto da grade quando a saída parou há [`record::SNAPSHOT_IDLE`]; devolve quando voltar a olhar.
    pub fn record_tick(&self, now: Instant) -> Option<Instant> {
        let seq = self.shared.output_seq.load(Ordering::Relaxed);
        if self.shared.recorder.lock().as_ref().is_none_or(|r| r.snapped == seq) {
            return None;
        }
        let due = (*self.shared.last_output.lock())? + record::SNAPSHOT_IDLE;
        if now < due {
            return Some(due);
        }
        let text = record::grid_text(&self.shared.emu.lock());
        if let Some(r) = self.shared.recorder.lock().as_mut() {
            r.snapshot(&text);
            r.snapped = seq;
        }
        None
    }

    pub fn size(&self) -> GridSize {
        self.size
    }

    /// Limpa a tela e o histórico (equivalente a `term.reset()`).
    pub fn reset(&mut self) {
        let mut emu = self.shared.emu.lock();
        let listener = emu.listener.clone();
        *emu = Emulator::new(self.size, listener);
    }

    /// Escreve bytes "de saída" diretamente no emulador (mensagens da própria UI).
    pub fn print(&self, text: &str) {
        self.shared.emu.lock().feed(text.as_bytes());
    }

    /// Envia bytes ao processo (entrada do usuário).
    pub fn write(&self, bytes: &[u8]) {
        self.shared.write_input(bytes);
    }

    /// Abre o PTY e inicia o programa da [`SpawnSpec`] com exatamente o ambiente dela.
    pub fn spawn(&mut self, spec: &SpawnSpec, wake: Wake) -> anyhow::Result<()> {
        self.kill();
        self.generation += 1;
        let generation = self.generation;
        let pty_system = portable_pty::native_pty_system();
        let pair =
            pty_system.openpty(PtySize { rows: self.size.rows as u16, cols: self.size.cols as u16, pixel_width: 0, pixel_height: 0 })?;

        let mut cmd = CommandBuilder::new(&spec.program);
        cmd.args(&spec.args);
        cmd.cwd(&spec.cwd);
        cmd.env_clear();
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }
        // O node-pty definia TERM a partir de `name: 'xterm-256color'`.
        #[cfg(not(windows))]
        if !spec.env.contains_key("TERM") {
            cmd.env("TERM", "xterm-256color");
        }

        let mut child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);
        let killer = child.clone_killer();
        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        self.shared.attach_writer(writer, format!("pty-write-{}", spec.kind))?;
        *self.shared.exit.lock() = None;
        *self.shared.recorder.lock() =
            self.record_to.as_ref().and_then(|(dir, name)| match record::Recorder::start(dir, name, self.size) {
                Ok(r) => Some(r),
                Err(e) => {
                    log::warn!("AGENTDECK_DEBUG_PTY: não consegui abrir a gravação em {}: {e}", dir.display());
                    None
                }
            });

        // Leitora: alimenta o emulador e acorda a UI.
        let shared = self.shared.clone();
        let wake_r = wake.clone();
        std::thread::Builder::new().name(format!("pty-read-{}", spec.kind)).spawn(move || {
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if let Some(r) = shared.recorder.lock().as_mut() {
                            r.bytes(&buf[..n]);
                        }
                        shared.emu.lock().feed(&buf[..n]);
                        *shared.last_output.lock() = Some(Instant::now());
                        shared.output_seq.fetch_add(1, Ordering::Relaxed);
                        wake_r();
                    }
                }
            }
        })?;

        // Espera o término do processo (no ConPTY a leitura não recebe EOF sozinha).
        let shared = self.shared.clone();
        std::thread::Builder::new().name(format!("pty-wait-{}", spec.kind)).spawn(move || {
            let code = child.wait().map(|s| s.exit_code() as i64).unwrap_or(-1);
            // Pequena folga para a leitora despejar a saída final antes da mensagem.
            std::thread::sleep(std::time::Duration::from_millis(60));
            *shared.exit.lock() = Some((generation, code));
            wake();
        })?;

        self.pty = Some(PtyHandle { master: pair.master, killer });
        Ok(())
    }

    /// Retorna o código de saída se o processo atual terminou desde a última consulta.
    pub fn take_exit(&mut self) -> Option<i64> {
        let mut exit = self.shared.exit.lock();
        match *exit {
            Some((generation, code)) if generation == self.generation && self.pty.is_some() => {
                *exit = None;
                drop(exit);
                self.pty = None;
                *self.shared.writer.lock() = None;
                Some(code)
            }
            _ => None,
        }
    }

    /// Mata o processo (fechar aba / reiniciar).
    pub fn kill(&mut self) {
        if let Some(mut p) = self.pty.take() {
            let _ = p.killer.kill();
            drop(p.master);
        }
        *self.shared.writer.lock() = None;
    }

    /// Redimensiona grade e PTY (mínimo 2×2).
    pub fn resize(&mut self, size: GridSize, cell_w: f32, cell_h: f32) {
        let size = GridSize { cols: size.cols.max(2), rows: size.rows.max(2) };
        *self.shared.window_size.lock() = WindowSize {
            num_lines: size.rows as u16,
            num_cols: size.cols as u16,
            cell_width: cell_w.round() as u16,
            cell_height: cell_h.round() as u16,
        };
        if size == self.size {
            return;
        }
        self.size = size;
        if let Some(r) = self.shared.recorder.lock().as_mut() {
            r.resize(size);
        }
        self.shared.emu.lock().resize(size);
        if let Some(p) = &self.pty {
            let _ = p.master.resize(PtySize { rows: size.rows as u16, cols: size.cols as u16, pixel_width: 0, pixel_height: 0 });
        }
    }

    /// Consome o sinal de BEL.
    pub fn take_bell(&self) -> bool {
        self.shared.bell.swap(false, Ordering::Relaxed)
    }

    /// Consome texto pedido via OSC 52.
    pub fn take_clipboard(&self) -> Option<String> {
        self.shared.clipboard.lock().take()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::index::{Column, Line};
    use std::time::Duration;

    fn first_char(s: &Session) -> char {
        s.shared.emu.lock().term.grid()[Line(0)][Column(0)].c
    }

    #[test]
    fn a_synchronized_update_without_its_end_is_released_after_the_deadline() {
        let s = Session::new(GridSize { cols: 20, rows: 4 });
        s.print("\x1b[?2026hx");
        assert_eq!(first_char(&s), ' ', "retido durante a atualização sincronizada");

        let at = s.shared.emu.lock().expire_sync(Instant::now()).expect("prazo pendente");
        assert_eq!(first_char(&s), ' ');

        assert_eq!(s.shared.emu.lock().expire_sync(at + Duration::from_millis(1)), None);
        assert_eq!(first_char(&s), 'x', "o prazo vencido solta a saída retida");
    }

    /// Processo que não lê a entrada até `open` ser liberado (como um programa ocupado).
    struct Stuck {
        open: crossbeam_channel::Receiver<()>,
        opened: bool,
        got: Arc<Mutex<Vec<u8>>>,
    }

    impl Write for Stuck {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if !self.opened {
                let _ = self.open.recv();
                self.opened = true;
            }
            self.got.lock().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn writes_never_block_and_keep_their_order() {
        let s = Session::new(GridSize { cols: 20, rows: 4 });
        let (release, open) = crossbeam_channel::bounded(1);
        let got = Arc::new(Mutex::new(Vec::new()));
        s.shared.attach_writer(Box::new(Stuck { open, opened: false, got: got.clone() }), "pty-write-teste".into()).expect("thread");

        let started = Instant::now();
        s.write(b"a");
        let paste = vec![b'x'; 4 * 1024 * 1024];
        s.write(&paste);
        // Consulta DA do programa: a resposta sai do emulador (leitora) sem esperar a escrita.
        s.print("\x1b[c");
        s.write(b"\r");
        assert!(started.elapsed() < Duration::from_secs(2), "nada bloqueou com o processo sem ler");

        release.send(()).expect("libera");
        let deadline = Instant::now() + Duration::from_secs(10);
        while got.lock().last() != Some(&b'\r') && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        let got = got.lock();
        assert_eq!(got[0], b'a');
        assert_eq!(&got[1..1 + paste.len()], &paste[..]);
        let tail = &got[1 + paste.len()..];
        assert!(tail.starts_with(b"\x1b[?") && tail.ends_with(b"c\r"), "resposta DA na ordem, antes do Enter: {tail:?}");
    }

    #[test]
    fn without_a_process_writes_are_dropped() {
        let mut s = Session::new(GridSize { cols: 20, rows: 4 });
        s.write(b"nada");
        s.print("\x1b[6n");
        s.kill();
        s.write(b"nada");
    }

    #[test]
    fn osc52_only_for_the_active_tab_of_the_active_project() {
        assert!(clipboard_allowed(1, Some(1), "p1", "p1"));
        assert!(!clipboard_allowed(0, Some(1), "p1", "p1"), "aba em segundo plano");
        assert!(!clipboard_allowed(1, Some(1), "p2", "p1"), "aba de outro projeto");
        assert!(!clipboard_allowed(1, None, "p1", "p1"));
    }

    #[test]
    fn osc52_from_the_program_is_captured_for_the_ui_to_decide() {
        let s = Session::new(GridSize { cols: 20, rows: 4 });
        s.print("\x1b]52;c;b2k=\x07");
        assert_eq!(s.take_clipboard().as_deref(), Some("oi"));
        assert_eq!(s.take_clipboard(), None);
    }

    #[test]
    fn a_synchronized_update_with_its_end_shows_at_once() {
        let s = Session::new(GridSize { cols: 20, rows: 4 });
        s.print("\x1b[?2026hy\x1b[?2026l");
        assert_eq!(first_char(&s), 'y');
        assert_eq!(s.shared.emu.lock().expire_sync(Instant::now()), None);
    }
}
