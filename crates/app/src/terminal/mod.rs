//! Tab terminals: VT emulation (`alacritty_terminal`) + pseudo-terminal (`portable-pty`).
//!
//! Each tab has a [`Session`]. The PTY reader thread feeds the parser under a mutex and
//! wakes the winit event loop; the UI only reads the grid to paint (see [`view`]). Process writes
//! go through a queue with a dedicated thread (`pty-write`): neither the UI nor the reader block on writes.

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

/// History lines (like xterm's `scrollback: 10000`).
pub const SCROLLBACK: usize = 10_000;

/// Function that wakes the UI event loop (can be called from any thread).
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// Grid dimensions.
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

/// Write queue for current process. Writers (UI and emulator responses) only
/// enqueue; the tab's `pty-write` thread performs the write, which blocks while the program doesn't read
/// (e.g. large paste). A single queue per tab preserves the order of keystrokes, pastes, and Enters.
type SharedWriter = Arc<Mutex<Option<Sender<Vec<u8>>>>>;

/// Enqueues bytes for the process (no-op if there is no process). Never blocks.
fn enqueue(writer: &SharedWriter, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    if let Some(tx) = writer.lock().as_ref() {
        let _ = tx.send(bytes.to_vec());
    }
}

/// Receives emulator events (called with terminal locked — must not lock it again).
#[derive(Clone)]
pub struct Listener {
    writer: SharedWriter,
    bell: Arc<AtomicBool>,
    /// (cols, rows, cell width, cell height) in pixels.
    window_size: Arc<Mutex<WindowSize>>,
    /// Text requested via OSC 52 to be sent to clipboard.
    clipboard: Arc<Mutex<Option<String>>>,
}

impl Listener {
    /// Responses to the program (DA/DSR/colors): via queue, so the reader never blocks on write.
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

/// Emulator: grid + parser.
pub struct Emulator {
    pub term: Term<Listener>,
    parser: Processor<StdSyncHandler>,
    /// Copy of listener to recreate `Term` on `reset()`.
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

    /// Resizes grid (on Windows, as ConPTY expects: see [`resize`]).
    pub fn resize(&mut self, size: GridSize) {
        self.resize_as(size, cfg!(windows));
    }

    /// [`Emulator::resize`] with explicit choice (tests replay ConPTY recordings on any system).
    pub fn resize_as(&mut self, size: GridSize, keep_top: bool) {
        resize::resize(&mut self.term, size, keep_top);
    }

    /// Synchronized update (`ESC[?2026h`, used by Claude Code): parser holds output until
    /// `ESC[?2026l`, but the 150 ms timeout is managed by caller (in Alacritty, the PTY loop).
    /// Without this, a missed end (e.g. in ConPTY redraw on resize) freezes the screen.
    /// Releases held output if timeout expired; returns remaining deadline if still pending.
    pub fn expire_sync(&mut self, now: Instant) -> Option<Instant> {
        let at = self.parser.sync_timeout().sync_timeout()?;
        if now < at {
            return Some(at);
        }
        self.parser.stop_sync(&mut self.term);
        None
    }
}

/// Shared state between UI and PTY threads.
pub struct Shared {
    pub emu: Mutex<Emulator>,
    writer: SharedWriter,
    bell: Arc<AtomicBool>,
    window_size: Arc<Mutex<WindowSize>>,
    clipboard: Arc<Mutex<Option<String>>>,
    /// Incremented on each output chunk from the process.
    pub output_seq: AtomicU64,
    pub last_output: Mutex<Option<Instant>>,
    /// (generation, exit code) when the process exits.
    exit: Mutex<Option<(u64, i64)>>,
    /// Raw output recording (only with `AGENTDECK_DEBUG_PTY=1`, see [`record`]).
    recorder: Mutex<Option<record::Recorder>>,
}

impl Shared {
    /// Writes to current process (no-op if no process). Only enqueues: never blocks.
    pub fn write_input(&self, bytes: &[u8]) {
        enqueue(&self.writer, bytes);
    }

    /// Attaches `w` to write queue, with a dedicated thread that writes in FIFO order. The
    /// thread ends when the queue is replaced or dropped (`kill`, process exit) or write fails.
    fn attach_writer(&self, mut w: Box<dyn Write + Send>, name: String) -> std::io::Result<()> {
        let (tx, rx) = crossbeam_channel::unbounded::<Vec<u8>>();
        std::thread::Builder::new().name(name).spawn(move || {
            while let Ok(mut bytes) = rx.recv() {
                // Coalesce queued writes: fewer writes during chunked paste.
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

/// OSC 52 (program requests writing to clipboard) is only allowed for the active and
/// visible tab of the active project: a background tab cannot overwrite what the user copied.
pub fn clipboard_allowed(tab: usize, active: Option<usize>, tab_project: &str, active_project: &str) -> bool {
    active == Some(tab) && tab_project == active_project
}

struct PtyHandle {
    master: Box<dyn MasterPty + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
}

/// Tab terminal.
pub struct Session {
    pub shared: Arc<Shared>,
    pty: Option<PtyHandle>,
    generation: u64,
    size: GridSize,
    /// Directory and stem name for debug recording (see [`Session::record_to`]).
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

    /// Enables raw output recording for upcoming processes of this tab in `dir`, with `name` in filenames.
    pub fn record_to(&mut self, dir: PathBuf, name: String) {
        self.record_to = Some((dir, name));
    }

    /// Diagnostic recording for this tab is enabled (environment variable or tab menu item).
    pub fn recording(&self) -> bool {
        self.record_to.is_some()
    }

    /// Enables (`Some`) or disables (`None`) recording for this tab. Enabled while a process runs, starts now: `.bin` only
    /// contains subsequent output, and current grid goes to `.grid.txt` (earlier output cannot be replayed).
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

    /// Saves the grid (screen and tail of history) now, in `dir`; returns the file path. With active recording, rewrites
    /// its `.grid.txt`.
    pub fn save_snapshot(&self, dir: &Path, name: &str) -> std::io::Result<PathBuf> {
        let text = record::grid_text(&self.shared.emu.lock());
        if let Some(r) = self.shared.recorder.lock().as_ref() {
            r.snapshot(&text);
            return Ok(r.grid_path().to_owned());
        }
        record::write_snapshot(dir, name, &text)
    }

    /// Takes a grid snapshot when output has been idle for [`record::SNAPSHOT_IDLE`]; returns next inspection time.
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

    /// Clears screen and history (equivalent to `term.reset()`).
    pub fn reset(&mut self) {
        let mut emu = self.shared.emu.lock();
        let listener = emu.listener.clone();
        *emu = Emulator::new(self.size, listener);
    }

    /// Writes "output" bytes directly to emulator (internal UI messages).
    pub fn print(&self, text: &str) {
        self.shared.emu.lock().feed(text.as_bytes());
    }

    /// Sends bytes to process (user input).
    pub fn write(&self, bytes: &[u8]) {
        self.shared.write_input(bytes);
    }

    /// Opens PTY and spawns the program from [`SpawnSpec`] with its exact environment.
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
        // node-pty set TERM from `name: 'xterm-256color'`.
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
                    log::warn!("AGENTDECK_DEBUG_PTY: could not open recording at {}: {e}", dir.display());
                    None
                }
            });

        // Reader: feeds emulator and wakes UI.
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

        // Wait for process termination (in ConPTY reading does not receive EOF alone).
        let shared = self.shared.clone();
        std::thread::Builder::new().name(format!("pty-wait-{}", spec.kind)).spawn(move || {
            let code = child.wait().map(|s| s.exit_code() as i64).unwrap_or(-1);
            // Small delay for reader to flush final output before message.
            std::thread::sleep(std::time::Duration::from_millis(60));
            *shared.exit.lock() = Some((generation, code));
            wake();
        })?;

        self.pty = Some(PtyHandle { master: pair.master, killer });
        Ok(())
    }

    /// Returns exit code if the current process finished since last check.
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

    /// Kills the process (tab close / restart).
    pub fn kill(&mut self) {
        if let Some(mut p) = self.pty.take() {
            let _ = p.killer.kill();
            drop(p.master);
        }
        *self.shared.writer.lock() = None;
    }

    /// Resizes grid and PTY (minimum 2×2).
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

    /// Consumes BEL signal.
    pub fn take_bell(&self) -> bool {
        self.shared.bell.swap(false, Ordering::Relaxed)
    }

    /// Consumes text requested via OSC 52.
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
        assert_eq!(first_char(&s), ' ', "held during synchronized update");

        let at = s.shared.emu.lock().expire_sync(Instant::now()).expect("pending deadline");
        assert_eq!(first_char(&s), ' ');

        assert_eq!(s.shared.emu.lock().expire_sync(at + Duration::from_millis(1)), None);
        assert_eq!(first_char(&s), 'x', "expired deadline releases held output");
    }

    /// Process that does not read input until `open` is released (like a busy program).
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
        s.shared.attach_writer(Box::new(Stuck { open, opened: false, got: got.clone() }), "pty-write-test".into()).expect("thread");

        let started = Instant::now();
        s.write(b"a");
        let paste = vec![b'x'; 4 * 1024 * 1024];
        s.write(&paste);
        // DA query from program: response leaves emulator (reader) without waiting for write.
        s.print("\x1b[c");
        s.write(b"\r");
        assert!(started.elapsed() < Duration::from_secs(2), "nothing blocked with unread process");

        release.send(()).expect("release");
        let deadline = Instant::now() + Duration::from_secs(10);
        while got.lock().last() != Some(&b'\r') && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        let got = got.lock();
        assert_eq!(got[0], b'a');
        assert_eq!(&got[1..1 + paste.len()], &paste[..]);
        let tail = &got[1 + paste.len()..];
        assert!(tail.starts_with(b"\x1b[?") && tail.ends_with(b"c\r"), "DA response in order, before Enter: {tail:?}");
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
        assert!(!clipboard_allowed(0, Some(1), "p1", "p1"), "background tab");
        assert!(!clipboard_allowed(1, Some(1), "p2", "p1"), "tab of another project");
        assert!(!clipboard_allowed(1, None, "p1", "p1"));
    }

    #[test]
    fn osc52_from_the_program_is_captured_for_the_ui_to_decide() {
        let s = Session::new(GridSize { cols: 20, rows: 4 });
        s.print("\x1b]52;c;b2s=\x07");
        assert_eq!(s.take_clipboard().as_deref(), Some("ok"));
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
