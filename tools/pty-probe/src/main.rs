//! Sonda do defeito t37 (terminal em branco): abre o `claude.exe` num PTY (portable-pty, ConPTY no Windows), alimenta um
//! `Term` do `alacritty_terminal` como o app faz e relata, por amostragem, qualquer "vão" (5+ linhas vazias seguidas com
//! texto depois) na grade. Grava `<saida>` (bytes crus) e `<saida>.events.txt` (trocas de tamanho), o mesmo formato da
//! gravação do app (`terminal/record.rs`). Ver `README.md`.

// A mesma mudança de tamanho do app (`PROBE_PULL=1` desliga a correção, para comparar).
#[path = "../../../crates/app/src/terminal/resize.rs"]
mod resize;

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone)]
struct L(Arc<Mutex<Vec<u8>>>);
impl EventListener for L {
    fn send_event(&self, e: Event) {
        if let Event::PtyWrite(s) = e {
            self.0.lock().unwrap().extend_from_slice(s.as_bytes());
        }
    }
}
struct Sz(usize, usize);
impl Dimensions for Sz {
    fn total_lines(&self) -> usize {
        self.1
    }
    fn screen_lines(&self) -> usize {
        self.1
    }
    fn columns(&self) -> usize {
        self.0
    }
}

fn row_text(t: &Term<L>, l: i32) -> String {
    let cols = t.columns();
    (0..cols).map(|c| t.grid()[Line(l)][Column(c)].c).collect::<String>().trim_end().to_string()
}
fn pattern(t: &Term<L>) -> String {
    (0..t.screen_lines() as i32).map(|l| if row_text(t, l).is_empty() { '.' } else { '#' }).collect()
}
// vão: >=5 linhas vazias seguidas com texto depois
fn has_gap(p: &str) -> bool {
    let t = p.trim_end_matches('.');
    t.contains(".....")
}

// uso: probe <rows> <cols> <out.bin> "<prompt1>" ["<prompt2>" ...]   env RESIZE="seg:rows:cols,..." (segundos desde o 1o prompt)
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let rows0: usize = args[1].parse().unwrap();
    let cols0: usize = args[2].parse().unwrap();
    let out = args[3].clone();
    let prompts: Vec<String> = args[4..].to_vec();
    let mut resizes: Vec<(f64, usize, usize)> = std::env::var("RESIZE")
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| {
            let v: Vec<&str> = s.split(':').collect();
            (v[0].parse().unwrap(), v[1].parse().unwrap(), v[2].parse().unwrap())
        })
        .collect();
    let boot: u64 = std::env::var("PROBE_BOOT").ok().and_then(|s| s.parse().ok()).unwrap_or(8);
    let (mut cols, mut rows) = (cols0, rows0);
    let pair = native_pty_system().openpty(PtySize { rows: rows as u16, cols: cols as u16, pixel_width: 0, pixel_height: 0 }).unwrap();
    let program = std::env::var("PROBE_PROGRAM").expect("PROBE_PROGRAM: caminho do claude.exe");
    let mut cmd = CommandBuilder::new(program);
    cmd.args(std::env::var("PROBE_ARGS").unwrap_or_default().split_whitespace());
    if let Ok(s) = std::env::var("PROBE_SETTINGS") {
        cmd.args(["--settings", s.as_str()]);
    }
    cmd.cwd(std::env::var("PROBE_CWD").unwrap_or_else(|_| ".".into()));
    cmd.env_clear();
    for (k, v) in std::env::vars() {
        let u = k.to_ascii_uppercase();
        if u.starts_with("CLAUDE_CODE_") || u == "CLAUDECODE" || u == "CLAUDE_PID" || u == "ELECTRON_RUN_AS_NODE" {
            continue;
        }
        cmd.env(k, v);
    }
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();
    let resp = L(Arc::new(Mutex::new(Vec::new())));
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut b = vec![0u8; 65536];
        loop {
            match reader.read(&mut b) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(b[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    let mut term = Term::new(Config { scrolling_history: 10000, ..Config::default() }, &Sz(cols, rows), resp.clone());
    let mut parser: Processor<StdSyncHandler> = Processor::new();
    let mut raw = std::fs::File::create(&out).unwrap();
    let mut ev = std::fs::File::create(format!("{out}.events.txt")).unwrap();
    let mut off: u64 = 0;
    writeln!(ev, "0 0 size {cols} {rows}").unwrap();
    let start = Instant::now();
    let mut next_prompt = 0usize;
    let mut first_prompt: Option<Instant> = None;
    let mut last_out = Instant::now();
    let mut last_sent = Instant::now();
    let mut next_snap = Instant::now() + Duration::from_millis(200);
    let mut last_pat = String::new();
    let mut report_at: Option<Instant> = None;
    loop {
        match rx.recv_timeout(Duration::from_millis(30)) {
            Ok(b) => {
                raw.write_all(&b).unwrap();
                off += b.len() as u64;
                parser.advance(&mut term, &b);
                last_out = Instant::now();
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => break,
        }
        if let Some(at) = parser.sync_timeout().sync_timeout() {
            if Instant::now() >= at {
                parser.stop_sync(&mut term);
            }
        }
        let r = std::mem::take(&mut *resp.0.lock().unwrap());
        if !r.is_empty() {
            writer.write_all(&r).unwrap();
            writer.flush().unwrap();
        }
        // próximo prompt: quando está quieto há 4 s
        let ready = if next_prompt == 0 {
            start.elapsed() > Duration::from_secs(boot) && last_out.elapsed() > Duration::from_secs(1)
        } else {
            last_out.elapsed() > Duration::from_secs(4) && last_sent.elapsed() > Duration::from_secs(6)
        };
        if next_prompt < prompts.len() && ready {
            writer.write_all(prompts[next_prompt].as_bytes()).unwrap();
            writer.flush().unwrap();
            std::thread::sleep(Duration::from_millis(500));
            writer.write_all(b"\r").unwrap();
            writer.flush().unwrap();
            next_prompt += 1;
            last_sent = Instant::now();
            if first_prompt.is_none() {
                first_prompt = Some(Instant::now());
            }
            println!("[{:.1}s] prompt {} enviado", start.elapsed().as_secs_f32(), next_prompt);
        }
        if let Some(fp) = first_prompt {
            while let Some(&(at, r, c)) = resizes.first() {
                if fp.elapsed().as_secs_f64() < at {
                    break;
                }
                resizes.remove(0);
                rows = r;
                cols = c;
                resize::resize(&mut term, Sz(cols, rows), std::env::var("PROBE_PULL").is_err());
                pair.master.resize(PtySize { rows: rows as u16, cols: cols as u16, pixel_width: 0, pixel_height: 0 }).unwrap();
                writeln!(ev, "0 {off} size {cols} {rows}").unwrap();
                println!("[{:.1}s] REDIMENSIONOU para {cols}x{rows}", start.elapsed().as_secs_f32());
                report_at = Some(Instant::now() + Duration::from_millis(900));
            }
        }
        if report_at.is_some_and(|t| Instant::now() >= t) {
            report_at = None;
            println!(
                "    depois: {}x{} {}  hist={}  SEQ {}",
                cols,
                rows,
                pattern(&term),
                term.grid().history_size(),
                sequence_report(&term)
            );
            if let Ok(w) = std::env::var("PROBE_WATCH") {
                let hist = term.grid().history_size() as i32;
                for l in -hist..term.screen_lines() as i32 {
                    let t = row_text(&term, l);
                    if w.split(',').any(|p| t.starts_with(p)) {
                        println!("        {l:>5}: {}", t.chars().take(24).collect::<String>());
                    }
                }
            }
        }
        if Instant::now() >= next_snap && first_prompt.is_some() {
            next_snap += Duration::from_millis(200);
            let p = pattern(&term);
            if has_gap(&p) && p != last_pat {
                println!("[{:.1}s] VAO  {p}  hist={}", start.elapsed().as_secs_f32(), term.grid().history_size());
            }
            last_pat = p;
        }
        if next_prompt == prompts.len() && last_out.elapsed() > Duration::from_secs(15) {
            break;
        }
        if start.elapsed() > Duration::from_secs(240) {
            break;
        }
    }
    println!(
        "padrao final {}  historico {}  corridas de linhas vazias (linha, tamanho): {:?}",
        pattern(&term),
        term.grid().history_size(),
        blank_runs(&term)
    );
    println!("SEQ {}", sequence_report(&term));
    for l in 0..rows as i32 {
        println!("{:2}|{}", l, row_text(&term, l));
    }
    let _ = child.kill();
}

#[allow(dead_code)]
fn blank_runs(t: &Term<L>) -> Vec<(i32, usize)> {
    let hist = t.grid().history_size() as i32;
    let mut runs = Vec::new();
    let mut start: Option<i32> = None;
    for l in -hist..t.screen_lines() as i32 {
        let blank = row_text(t, l).is_empty();
        match (blank, start) {
            (true, None) => start = Some(l),
            (false, Some(s)) => {
                runs.push((s, (l - s) as usize));
                start = None
            }
            _ => {}
        }
    }
    runs.into_iter().filter(|r| r.1 >= 6).collect()
}

/// Linhas numeradas (`L00001 ...`, o que o roteiro do shell imprime): confere, na ordem do histórico + tela, que a
/// numeração não pula (linha sumida) nem repete (cópia no histórico) e conta as linhas vazias entre duas numeradas.
fn sequence_report(t: &Term<L>) -> String {
    let hist = t.grid().history_size() as i32;
    let (mut prev, mut missing, mut repeated, mut blank_between, mut seen) = (None::<u32>, Vec::new(), Vec::new(), 0usize, 0usize);
    let mut blanks = 0usize;
    for l in -hist..t.screen_lines() as i32 {
        let text = row_text(t, l);
        let num = text.trim_start_matches(|c: char| !c.is_ascii_alphanumeric()).strip_prefix('L').and_then(|r| r.get(..5)).and_then(|d| d.parse::<u32>().ok());
        match num {
            Some(n) => {
                seen += 1;
                if let Some(p) = prev {
                    if n == p + 1 {
                        blank_between += blanks;
                    } else if n <= p {
                        repeated.push((p, n));
                    } else {
                        missing.push((p + 1, n - 1));
                    }
                }
                prev = Some(n);
                blanks = 0;
            }
            None if text.is_empty() => blanks += 1,
            None => {}
        }
    }
    format!("numeradas={seen} puladas={missing:?} repetidas/voltas={repeated:?} vazias_entre_consecutivas={blank_between}")
}
