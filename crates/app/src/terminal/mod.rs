//! Terminais das abas: emulação VT (alacritty_terminal) + pseudo-terminal (portable-pty).
//!
//! Cada aba tem uma [`Session`]. A thread leitora do PTY alimenta o parser sob um mutex e
//! acorda o loop do winit; a UI só lê a grade para pintar (ver [`view`]).

pub mod colors;
pub mod input;
pub mod view;

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::{Config as TermConfig, Term};
use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};
use commanddeck_core::SpawnSpec;
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

type SharedWriter = Arc<Mutex<Option<Box<dyn Write + Send>>>>;

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
    fn write(&self, s: &str) {
        if let Some(w) = self.writer.lock().as_mut() {
            let _ = w.write_all(s.as_bytes());
            let _ = w.flush();
        }
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
        self.parser.advance(&mut self.term, bytes);
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
}

impl Shared {
    /// Escreve no processo atual (no-op se não houver processo).
    pub fn write_input(&self, bytes: &[u8]) {
        if let Some(w) = self.writer.lock().as_mut() {
            let _ = w.write_all(bytes);
            let _ = w.flush();
        }
    }
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
        });
        Self { shared, pty: None, generation: 0, size }
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
        *self.shared.writer.lock() = Some(writer);
        *self.shared.exit.lock() = None;

        // Leitora: alimenta o emulador e acorda a UI.
        let shared = self.shared.clone();
        let wake_r = wake.clone();
        std::thread::Builder::new().name(format!("pty-read-{}", spec.kind)).spawn(move || {
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
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
        self.shared.emu.lock().term.resize(size);
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
