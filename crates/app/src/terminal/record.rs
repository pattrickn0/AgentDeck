//! Raw output recording for a tab, debug only (`HOLT_DEBUG_PTY=1`, off by default).
//!
//! Useful for bugs that only show up in the user's session (e.g. top half of the terminal blank): with
//! recording enabled, each tab process creates three files in `data/pty/` with the same prefix
//! `<tab>-<date>-<time>`:
//! - `.bin`: bytes delivered by the PTY, exactly as received (what the emulator received);
//! - `.events.txt`: one line per resize event (`<ms> <position in .bin> size <cols> <rows>`), to
//!   replay bytes at the same size and resize transitions (see `replay`, in tests);
//! - `.grid.txt`: the emulator grid (screen and tail of history) taken 1.5s after last output,
//!   rewritten each time. If text is present in it but the frame was blank, the bug is in rendering; if the
//!   grid is also blank, it comes from the emulator or what the CLI sent, and `.bin` resolves which.

use std::fmt::Write as _;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::TermMode;

use super::{Emulator, GridSize};

/// How long without output until taking the grid snapshot.
pub const SNAPSHOT_IDLE: Duration = Duration::from_millis(1500);
/// History lines included in snapshot (most recent).
const SNAPSHOT_HISTORY: usize = 400;

pub struct Recorder {
    bytes: File,
    events: File,
    grid: PathBuf,
    started: Instant,
    offset: u64,
    /// `output_seq` of last snapshot (to only snapshot fresh output).
    pub snapped: u64,
}

impl Recorder {
    /// Opens the three files for `name` (tab name) in `dir`, logging the initial size.
    pub fn start(dir: &Path, name: &str, size: GridSize) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let safe = safe_name(name);
        let stem = format!("{safe}-{}", stamp());
        let mut r = Self {
            bytes: File::create(dir.join(format!("{stem}.bin")))?,
            events: File::create(dir.join(format!("{stem}.events.txt")))?,
            grid: dir.join(format!("{stem}.grid.txt")),
            started: Instant::now(),
            offset: 0,
            snapped: 0,
        };
        r.resize(size);
        Ok(r)
    }

    pub fn bytes(&mut self, chunk: &[u8]) {
        if self.bytes.write_all(chunk).is_ok() {
            self.offset += chunk.len() as u64;
        }
    }

    pub fn resize(&mut self, size: GridSize) {
        let _ = writeln!(self.events, "{} {} size {} {}", self.started.elapsed().as_millis(), self.offset, size.cols, size.rows);
    }

    pub fn grid_path(&self) -> &Path {
        &self.grid
    }

    pub fn snapshot(&self, text: &str) {
        let _ = std::fs::write(&self.grid, text);
    }
}

/// Safe file name (letters, digits, and `-`).
fn safe_name(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect()
}

fn stamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

/// Saves a standalone grid snapshot (without active recording): `<tab>-<date>-<time>.grid.txt`.
pub fn write_snapshot(dir: &Path, name: &str, text: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}-{}.grid.txt", safe_name(name), stamp()));
    std::fs::write(&path, text)?;
    Ok(path)
}

/// The emulator grid as text: header with state, tail of history (`h<line>`) and screen (`s<line>`).
pub fn grid_text(emu: &Emulator) -> String {
    let term = &emu.term;
    let grid = term.grid();
    let (rows, cols, history) = (term.screen_lines(), term.columns(), grid.history_size());
    let cursor = grid.cursor.point;
    let mut out = format!(
        "# cols={cols} rows={rows} history={history} display_offset={} cursor=({},{}) alt_screen={}\n",
        grid.display_offset(),
        cursor.line.0,
        cursor.column.0,
        term.mode().contains(TermMode::ALT_SCREEN),
    );
    let row = |line: i32| -> String { (0..cols).map(|c| grid[Line(line)][Column(c)].c).collect::<String>().trim_end().to_owned() };
    for line in -(history.min(SNAPSHOT_HISTORY) as i32)..0 {
        let _ = writeln!(out, "h{line:>5}| {}", row(line));
    }
    for line in 0..rows as i32 {
        let _ = writeln!(out, "s{line:>5}| {}", row(line));
    }
    out
}

#[cfg(test)]
pub mod replay {
    //! Replays a recording (`.bin` + `.events.txt`) in a new emulator, at the same size and with the same resize transitions.

    use super::super::{GridSize, Session};

    /// Events `(position in .bin, size)` in order of `events`.
    pub fn parse_events(events: &str) -> Vec<(usize, GridSize)> {
        events
            .lines()
            .filter_map(|l| {
                let f: Vec<&str> = l.split_whitespace().collect();
                let [_ms, offset, "size", cols, rows] = f[..] else { return None };
                Some((offset.parse().ok()?, GridSize { cols: cols.parse().ok()?, rows: rows.parse().ok()? }))
            })
            .collect()
    }

    pub fn run(bytes: &[u8], events: &str) -> Session {
        let events = parse_events(events);
        let first = events.first().map_or(GridSize { cols: 80, rows: 24 }, |e| e.1);
        let mut session = Session::new(first);
        let mut pos = 0;
        for &(at, size) in events.iter().skip(1) {
            let at = at.min(bytes.len());
            session.shared.emu.lock().feed(&bytes[pos..at]);
            pos = at;
            session.resize(size, 8.0, 16.0);
        }
        session.shared.emu.lock().feed(&bytes[pos..]);
        session
    }

    /// Like [`run`], but with emulator only: `keep_top` selects resize behavior (see `resize.rs`), on any platform.
    pub fn run_with(bytes: &[u8], events: &str, keep_top: bool) -> Session {
        let events = parse_events(events);
        let first = events.first().map_or(GridSize { cols: 80, rows: 24 }, |e| e.1);
        let session = Session::new(first);
        let mut pos = 0;
        for &(at, size) in events.iter().skip(1) {
            let at = at.min(bytes.len());
            session.shared.emu.lock().feed(&bytes[pos..at]);
            pos = at;
            session.shared.emu.lock().resize_as(size, keep_top);
        }
        session.shared.emu.lock().feed(&bytes[pos..]);
        session
    }

    /// Numbers of lines `L00001 ...` (the PowerShell script) in history + screen order.
    pub fn numbered(session: &Session) -> Vec<u32> {
        use alacritty_terminal::grid::Dimensions;
        use alacritty_terminal::index::{Column, Line};
        let emu = session.shared.emu.lock();
        let grid = emu.term.grid();
        (-(grid.history_size() as i32)..emu.term.screen_lines() as i32)
            .filter_map(|l| {
                let text: String = (0..emu.term.columns()).take(6).map(|c| grid[Line(l)][Column(c)].c).collect();
                text.strip_prefix('L')?.parse().ok()
            })
            .collect()
    }

    /// Screen lines (excluding history), trimmed of trailing whitespace.
    pub fn screen(session: &Session) -> Vec<String> {
        use alacritty_terminal::grid::Dimensions;
        use alacritty_terminal::index::{Column, Line};
        let emu = session.shared.emu.lock();
        let cols = emu.term.columns();
        (0..emu.term.screen_lines() as i32)
            .map(|l| (0..cols).map(|c| emu.term.grid()[Line(l)][Column(c)].c).collect::<String>().trim_end().to_owned())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::super::Session;
    use super::replay;
    use super::*;
    use std::sync::atomic::Ordering;

    /// Real recording (portable-pty + `claude.exe`, classic mode, `--model haiku`): Claude Code responds with a 40-line
    /// table on a 120×58 screen that shrinks to 50×12 during the response (like a window that leaves Highlight)
    /// and grows back to 120×58.
    const CLAUDE_BYTES: &[u8] = include_bytes!("testdata/claude-classico-encolhe-e-alarga.bin");
    const CLAUDE_EVENTS: &str = include_str!("testdata/claude-classico-encolhe-e-alarga.events.txt");

    /// Longest sequence of empty lines that still has text afterwards (the "gap" of bug t37).
    fn widest_gap(screen: &[String]) -> usize {
        let last = screen.iter().rposition(|l| !l.is_empty()).unwrap_or(0);
        let (mut run, mut widest) = (0, 0);
        for line in &screen[..=last] {
            run = if line.is_empty() { run + 1 } else { 0 };
            widest = widest.max(run);
        }
        widest
    }

    #[test]
    fn events_are_parsed_and_bad_lines_ignored() {
        let ev = replay::parse_events("0 0 size 120 58\ngarbage\n5 100 size 50 12\n7 x size 1 2\n");
        assert_eq!(ev, vec![(0, GridSize { cols: 120, rows: 58 }), (100, GridSize { cols: 50, rows: 12 })]);
    }

    #[test]
    fn a_real_claude_session_that_shrinks_and_grows_leaves_no_gap_in_the_grid() {
        let session = replay::run(CLAUDE_BYTES, CLAUDE_EVENTS);
        let screen = replay::screen(&session);
        assert_eq!((screen.len(), session.size()), (58, GridSize { cols: 120, rows: 58 }));
        assert!(widest_gap(&screen) < 5, "gap in the middle of the screen:\n{}", screen.join("\n"));
        assert!(screen.iter().any(|l| l.contains("Baleia") || l.contains("40")), "the end of the table is on screen");
        assert!(screen.iter().any(|l| l.starts_with('❯')), "the input prompt returned to the end");
    }

    #[test]
    fn replaying_in_every_chunk_size_gives_the_same_grid() {
        // The reader delivers ConPTY output in arbitrary chunk sizes: the grid must not depend on them.
        let whole = replay::screen(&replay::run(CLAUDE_BYTES, CLAUDE_EVENTS));
        for chunk in [1usize, 7, 61, 4096] {
            let events = replay::parse_events(CLAUDE_EVENTS);
            let mut session = Session::new(events[0].1);
            let mut pos = 0;
            let mut next = events.iter().skip(1).peekable();
            while pos < CLAUDE_BYTES.len() {
                let until = next.peek().map_or(CLAUDE_BYTES.len(), |e| e.0).min(CLAUDE_BYTES.len());
                if pos >= until {
                    session.resize(next.next().expect("event").1, 8.0, 16.0);
                    continue;
                }
                let end = (pos + chunk).min(until);
                session.shared.emu.lock().feed(&CLAUDE_BYTES[pos..end]);
                pos = end;
            }
            for e in next {
                session.resize(e.1, 8.0, 16.0);
            }
            assert_eq!(replay::screen(&session), whole, "{chunk}-byte chunks");
        }
    }

    /// Real ConPTY recordings (Windows, `powershell -NoLogo`, `1..N | % { "L{0:D5} ..." }`) with the window resizing:
    /// height 58→30→58→40→58 (200 lines) and width 120→60→120 (400 lines wrapped in half).
    const PS_HEIGHT: (&[u8], &str) =
        (include_bytes!("testdata/conhost-powershell-altura.bin"), include_str!("testdata/conhost-powershell-altura.events.txt"));
    const PS_WIDTH: (&[u8], &str) =
        (include_bytes!("testdata/conhost-powershell-largura.bin"), include_str!("testdata/conhost-powershell-largura.events.txt"));

    #[test]
    fn growing_the_window_does_not_lose_the_lines_the_grid_pulled_from_the_history() {
        // Bug t37: alacritty_terminal pulls history to the top when gaining lines and ConPTY repaints over it.
        let (bytes, events) = PS_HEIGHT;
        let lost = replay::numbered(&replay::run_with(bytes, events, false));
        assert!(lost.len() < 200 && lost.windows(2).any(|w| w[1] != w[0] + 1), "without the fix lines disappear: {lost:?}");

        let session = replay::run_with(bytes, events, true);
        assert_eq!(replay::numbered(&session), (1..=200).collect::<Vec<u32>>(), "no lines disappear from history");
        let screen = replay::screen(&session);
        assert!(screen[0].starts_with("L00172"), "top of the window matches ConPTY: {:?}", &screen[..3]);
        assert!(screen[29].starts_with("PS C:"), "prompt stays where ConPTY painted it");
        assert!(screen[30..].iter().all(|l| l.is_empty()), "new lines remain blank at the bottom, as in ConPTY");
    }

    #[test]
    fn widening_a_wrapped_session_does_not_lose_the_lines_the_reflow_pulled_from_the_history() {
        let (bytes, events) = PS_WIDTH;
        let lost = replay::numbered(&replay::run_with(bytes, events, false));
        assert!(lost.windows(2).any(|w| w[1] != w[0] + 1), "without the fix a line disappears: {lost:?}");

        let numbers = replay::numbered(&replay::run_with(bytes, events, true));
        assert_eq!(numbers, (1..=400).collect::<Vec<u32>>(), "no lines disappear or repeat");
    }

    #[test]
    fn the_snapshot_has_the_state_header_the_history_tail_and_the_screen() {
        let session = Session::new(GridSize { cols: 20, rows: 3 });
        session.print(&"line\r\n".repeat(10));
        let text = grid_text(&session.shared.emu.lock());
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some("# cols=20 rows=3 history=8 display_offset=0 cursor=(2,0) alt_screen=false"));
        assert_eq!(text.lines().filter(|l| l.starts_with('h')).count(), 8);
        assert_eq!(text.lines().filter(|l| l.starts_with('s')).count(), 3);
        assert!(text.contains("s    0| line"));
    }

    #[test]
    fn the_menu_toggle_starts_recording_now_and_a_snapshot_works_with_or_without_it() {
        let dir = tempfile::tempdir().expect("dir");
        let pty = dir.path().join("pty");
        let mut s = Session::new(GridSize { cols: 20, rows: 4 });
        s.print("abc\r\n");
        assert!(!s.recording());

        // Without recording: standalone snapshot, `<tab>-<date>-<time>.grid.txt`.
        let alone = s.save_snapshot(&pty, "claude@t1").expect("snapshot");
        assert!(alone.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("claude_t1-") && n.ends_with(".grid.txt")));
        assert!(std::fs::read_to_string(&alone).expect("read").contains("s    0| abc"));

        // Enabling without a process only buffers the request (recording opens on spawn); disabling clears.
        s.set_recording(Some((pty.clone(), "claude@t1".into())));
        assert!(s.recording());
        assert!(s.shared.recorder.lock().is_none(), "no process yet");
        s.set_recording(None);
        assert!(!s.recording());
        assert!(!s.shared.recorder.lock().is_some());

        // Recording: snapshot overwrites recording's .grid.txt.
        *s.shared.recorder.lock() = Recorder::start(&pty, "claude@t1", s.size()).ok();
        let grid = s.save_snapshot(&pty, "claude@t1").expect("snapshot");
        assert!(grid.to_string_lossy().ends_with(".grid.txt"));
        assert!(std::fs::read_to_string(&grid).expect("read").starts_with("# cols=20 rows=4"));
    }

    #[test]
    fn recording_writes_bytes_resizes_and_a_snapshot_when_output_goes_idle() {
        let dir = tempfile::tempdir().expect("dir");
        let mut s = Session::new(GridSize { cols: 20, rows: 4 });
        s.record_to(dir.path().join("pty"), "claude@t1".into());
        // Without a real process: enables recording like spawn does and feeds like the reader.
        *s.shared.recorder.lock() = Recorder::start(&dir.path().join("pty"), "claude@t1", s.size()).ok();
        s.resize(GridSize { cols: 30, rows: 5 }, 8.0, 16.0);
        s.shared.recorder.lock().as_mut().expect("recording").bytes(b"hello\r\n");
        s.print("hello\r\n");
        s.shared.output_seq.fetch_add(1, Ordering::Relaxed);
        *s.shared.last_output.lock() = Some(Instant::now());

        let at = s.record_tick(Instant::now()).expect("not idle yet");
        assert_eq!(s.record_tick(at + Duration::from_millis(1)), None, "took snapshot");
        assert_eq!(s.record_tick(at + Duration::from_secs(9)), None, "does not repeat without fresh output");

        let files = |ext: &str| -> Vec<String> {
            let mut v: Vec<_> = std::fs::read_dir(dir.path().join("pty"))
                .expect("dir")
                .filter_map(|e| e.ok()?.file_name().into_string().ok())
                .filter(|n| n.ends_with(ext))
                .collect();
            v.sort();
            v
        };
        let read = |ext: &str| std::fs::read(dir.path().join("pty").join(&files(ext)[0])).expect("file");
        assert!(files(".bin")[0].starts_with("claude_t1-"), "safe name: {:?}", files(".bin"));
        assert_eq!(read(".bin"), b"hello\r\n");
        let events = String::from_utf8(read(".events.txt")).expect("utf8");
        assert_eq!(replay::parse_events(&events), vec![(0, GridSize { cols: 20, rows: 4 }), (0, GridSize { cols: 30, rows: 5 })]);
        assert!(String::from_utf8(read(".grid.txt")).expect("utf8").starts_with("# cols=30 rows=5"));
    }
}
