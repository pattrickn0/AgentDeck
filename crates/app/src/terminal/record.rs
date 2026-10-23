//! Gravação da saída crua de uma aba, só para depuração (`AGENTDECK_DEBUG_PTY=1`, desligada por padrão).
//!
//! Serve para o defeito que só aparece na sessão do usuário (ex.: a metade de cima do terminal em branco): com a
//! gravação ligada, cada processo de aba deixa em `dados/pty/` três arquivos com o mesmo prefixo
//! `<aba>-<data>-<hora>`:
//! - `.bin`: os bytes que o PTY entregou, exatamente como chegaram (é o que o emulador recebeu);
//! - `.events.txt`: uma linha por mudança de tamanho (`<ms> <posição no .bin> size <colunas> <linhas>`), para
//!   reproduzir os bytes no mesmo tamanho e nas mesmas trocas de tamanho (ver `replay`, nos testes);
//! - `.grid.txt`: a grade do emulador (tela e o fim do histórico) tirada 1,5 s depois da última saída,
//!   reescrita a cada vez. Se o texto está nela mas o quadro mostrava em branco, o defeito é do desenho; se a
//!   grade também está em branco, é do emulador ou do que o CLI mandou, e o `.bin` decide qual.

use std::fmt::Write as _;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::TermMode;

use super::{Emulator, GridSize};

/// Quanto tempo sem saída até tirar a foto da grade.
pub const SNAPSHOT_IDLE: Duration = Duration::from_millis(1500);
/// Linhas de histórico que entram na foto (as mais recentes).
const SNAPSHOT_HISTORY: usize = 400;

pub struct Recorder {
    bytes: File,
    events: File,
    grid: PathBuf,
    started: Instant,
    offset: u64,
    /// `output_seq` da última foto (para só fotografar saída nova).
    pub snapped: u64,
}

impl Recorder {
    /// Abre os três arquivos de `name` (nome da aba) em `dir`, já anotando o tamanho inicial.
    pub fn start(dir: &Path, name: &str, size: GridSize) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
        let stem = format!("{safe}-{}", chrono::Local::now().format("%Y%m%d-%H%M%S"));
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

    pub fn snapshot(&self, text: &str) {
        let _ = std::fs::write(&self.grid, text);
    }
}

/// A grade do emulador em texto: cabeçalho com o estado, o fim do histórico (`h<linha>`) e a tela (`s<linha>`).
pub fn grid_text(emu: &Emulator) -> String {
    let term = &emu.term;
    let grid = term.grid();
    let (rows, cols, history) = (term.screen_lines(), term.columns(), grid.history_size());
    let cursor = grid.cursor.point;
    let mut out = format!(
        "# colunas={cols} linhas={rows} historico={history} display_offset={} cursor=({},{}) tela_alternativa={}\n",
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
    //! Reproduz uma gravação (`.bin` + `.events.txt`) num emulador novo, no mesmo tamanho e com as mesmas trocas de tamanho.

    use super::super::{GridSize, Session};

    /// Eventos `(posição no .bin, tamanho)` na ordem de `events`.
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

    /// Linhas da tela (sem o histórico), sem os espaços do fim.
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

    /// Gravação real (portable-pty + o `claude.exe`, modo clássico, `--model haiku`): o Claude Code responde uma tabela de
    /// 40 linhas numa tela de 120×58 que encolhe para 50×12 durante a resposta (como uma janela que deixa de ser a
    /// grande no Destaque) e volta a 120×58.
    const CLAUDE_BYTES: &[u8] = include_bytes!("testdata/claude-classico-encolhe-e-alarga.bin");
    const CLAUDE_EVENTS: &str = include_str!("testdata/claude-classico-encolhe-e-alarga.events.txt");

    /// Maior sequência de linhas vazias que ainda tem texto depois (o "vão" do defeito t37).
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
        let ev = replay::parse_events("0 0 size 120 58\nlixo\n5 100 size 50 12\n7 x size 1 2\n");
        assert_eq!(ev, vec![(0, GridSize { cols: 120, rows: 58 }), (100, GridSize { cols: 50, rows: 12 })]);
    }

    #[test]
    fn a_real_claude_session_that_shrinks_and_grows_leaves_no_gap_in_the_grid() {
        let session = replay::run(CLAUDE_BYTES, CLAUDE_EVENTS);
        let screen = replay::screen(&session);
        assert_eq!((screen.len(), session.size()), (58, GridSize { cols: 120, rows: 58 }));
        assert!(widest_gap(&screen) < 5, "vão no meio da tela:\n{}", screen.join("\n"));
        assert!(screen.iter().any(|l| l.contains("Baleia") || l.contains("40")), "o fim da tabela está na tela");
        assert!(screen.iter().any(|l| l.starts_with('❯')), "a caixa de entrada voltou ao fim");
    }

    #[test]
    fn replaying_in_every_chunk_size_gives_the_same_grid() {
        // A leitora entrega o que o ConPTY mandou em pedaços de tamanho qualquer: a grade não pode depender deles.
        let whole = replay::screen(&replay::run(CLAUDE_BYTES, CLAUDE_EVENTS));
        for chunk in [1usize, 7, 61, 4096] {
            let events = replay::parse_events(CLAUDE_EVENTS);
            let mut session = Session::new(events[0].1);
            let mut pos = 0;
            let mut next = events.iter().skip(1).peekable();
            while pos < CLAUDE_BYTES.len() {
                let until = next.peek().map_or(CLAUDE_BYTES.len(), |e| e.0).min(CLAUDE_BYTES.len());
                if pos >= until {
                    session.resize(next.next().expect("evento").1, 8.0, 16.0);
                    continue;
                }
                let end = (pos + chunk).min(until);
                session.shared.emu.lock().feed(&CLAUDE_BYTES[pos..end]);
                pos = end;
            }
            for e in next {
                session.resize(e.1, 8.0, 16.0);
            }
            assert_eq!(replay::screen(&session), whole, "pedaços de {chunk} bytes");
        }
    }

    #[test]
    fn the_snapshot_has_the_state_header_the_history_tail_and_the_screen() {
        let session = Session::new(GridSize { cols: 20, rows: 3 });
        session.print(&"linha\r\n".repeat(10));
        let text = grid_text(&session.shared.emu.lock());
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some("# colunas=20 linhas=3 historico=8 display_offset=0 cursor=(2,0) tela_alternativa=false"));
        assert_eq!(text.lines().filter(|l| l.starts_with('h')).count(), 8);
        assert_eq!(text.lines().filter(|l| l.starts_with('s')).count(), 3);
        assert!(text.contains("s    0| linha"));
    }

    #[test]
    fn recording_writes_bytes_resizes_and_a_snapshot_when_output_goes_idle() {
        let dir = tempfile::tempdir().expect("pasta");
        let mut s = Session::new(GridSize { cols: 20, rows: 4 });
        s.record_to(dir.path().join("pty"), "claude@t1".into());
        // Sem processo real: liga a gravação como o `spawn` faz e alimenta como a leitora.
        *s.shared.recorder.lock() = Recorder::start(&dir.path().join("pty"), "claude@t1", s.size()).ok();
        s.resize(GridSize { cols: 30, rows: 5 }, 8.0, 16.0);
        s.shared.recorder.lock().as_mut().expect("gravando").bytes(b"oi\r\n");
        s.print("oi\r\n");
        s.shared.output_seq.fetch_add(1, Ordering::Relaxed);
        *s.shared.last_output.lock() = Some(Instant::now());

        let at = s.record_tick(Instant::now()).expect("ainda não está parada");
        assert_eq!(s.record_tick(at + Duration::from_millis(1)), None, "tirou a foto");
        assert_eq!(s.record_tick(at + Duration::from_secs(9)), None, "sem saída nova não repete");

        let files = |ext: &str| -> Vec<String> {
            let mut v: Vec<_> = std::fs::read_dir(dir.path().join("pty"))
                .expect("pasta")
                .filter_map(|e| e.ok()?.file_name().into_string().ok())
                .filter(|n| n.ends_with(ext))
                .collect();
            v.sort();
            v
        };
        let read = |ext: &str| std::fs::read(dir.path().join("pty").join(&files(ext)[0])).expect("arquivo");
        assert!(files(".bin")[0].starts_with("claude_t1-"), "nome seguro: {:?}", files(".bin"));
        assert_eq!(read(".bin"), b"oi\r\n");
        let events = String::from_utf8(read(".events.txt")).expect("utf8");
        assert_eq!(replay::parse_events(&events), vec![(0, GridSize { cols: 20, rows: 4 }), (0, GridSize { cols: 30, rows: 5 })]);
        assert!(String::from_utf8(read(".grid.txt")).expect("utf8").starts_with("# colunas=30 linhas=5"));
    }
}
