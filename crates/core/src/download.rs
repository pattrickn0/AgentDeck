//! Resilient download of large files (Whisper model and binaries).
//!
//! - resumes where it left off using `Range` (including across app restarts, via `.part` file);
//! - retries with exponential backoff when the connection drops or stalls;
//! - reports progress and checks SHA-256 before delivering the file.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, RecvTimeoutError};
use sha2::{Digest, Sha256};

use crate::events::{DownloadProgress, DownloadState};
use crate::t;

const CHUNK: usize = 64 * 1024;
const REPORT_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone)]
pub struct DownloadError {
    pub message: String,
    /// Eligible for retry (network drop, 5xx…). Disk, TLS and 4xx errors are not eligible.
    pub retryable: bool,
}

impl DownloadError {
    fn retry(message: impl Into<String>) -> Self {
        Self { message: message.into(), retryable: true }
    }

    fn fatal(message: impl Into<String>) -> Self {
        Self { message: message.into(), retryable: false }
    }
}

impl fmt::Display for DownloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DownloadError {}

#[derive(Debug, Clone)]
pub struct DownloadOpts<'a> {
    /// Expected SHA-256 (hex). Without it the file is accepted as received.
    pub sha256: Option<&'a str>,
    pub label: &'a str,
    /// Consecutive retries without progress before giving up.
    pub retries: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
    /// Time without receiving data that counts as a dropped connection.
    pub stall_timeout: Duration,
}

impl<'a> DownloadOpts<'a> {
    pub fn new(label: &'a str, sha256: Option<&'a str>) -> Self {
        Self {
            sha256,
            label,
            retries: 5,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
            stall_timeout: Duration::from_secs(30),
        }
    }
}

/// Throttles event frequency and calculates speed (smoothed average).
struct Reporter<'a> {
    label: String,
    sink: &'a mut dyn FnMut(DownloadProgress),
    received: u64,
    total: Option<u64>,
    speed: f64,
    last_emit: Instant,
    last_bytes: u64,
    progressed: bool,
}

impl<'a> Reporter<'a> {
    fn new(label: &str, sink: &'a mut dyn FnMut(DownloadProgress)) -> Self {
        Self {
            label: label.to_owned(),
            sink,
            received: 0,
            total: None,
            speed: 0.0,
            last_emit: Instant::now(),
            last_bytes: 0,
            progressed: false,
        }
    }

    fn emit(&mut self, state: DownloadState, attempt: u32, max_attempts: u32, retry_in_ms: u64, message: Option<String>) {
        (self.sink)(DownloadProgress {
            label: self.label.clone(),
            state,
            received: self.received,
            total: self.total,
            speed: self.speed,
            attempt,
            max_attempts,
            retry_in_ms,
            message,
        });
    }

    fn state(&mut self, state: DownloadState) {
        self.emit(state, 0, 0, 0, None);
    }

    fn begin(&mut self, start: u64, total: Option<u64>) {
        self.received = start;
        self.last_bytes = start;
        self.total = total;
        self.last_emit = Instant::now();
        self.state(DownloadState::Downloading);
    }

    fn bytes(&mut self, n: usize) {
        self.received += n as u64;
        self.progressed = true;
        let elapsed = self.last_emit.elapsed();
        if elapsed < REPORT_INTERVAL {
            return;
        }
        let inst = (self.received - self.last_bytes) as f64 / elapsed.as_secs_f64();
        self.speed = if self.speed > 0.0 { self.speed * 0.7 + inst * 0.3 } else { inst };
        self.last_emit = Instant::now();
        self.last_bytes = self.received;
        self.state(DownloadState::Downloading);
    }

    fn verifying(&mut self, done: u64, total: u64) {
        self.received = done;
        self.total = Some(total);
        self.state(DownloadState::Verifying);
    }

    /// `true` if the attempt that just failed managed to download something (resets failure count).
    fn take_progress(&mut self) -> bool {
        std::mem::take(&mut self.progressed)
    }
}

/// SHA-256 (lowercase hex) of a file, reporting progress.
pub fn sha256_file(path: &Path, on_progress: &mut dyn FnMut(u64, u64)) -> std::io::Result<String> {
    let mut f = File::open(path)?;
    let size = f.metadata()?.len();
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let (mut done, mut last) = (0u64, Instant::now());
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        done += n as u64;
        if last.elapsed() >= REPORT_INTERVAL {
            last = Instant::now();
            on_progress(done, size);
        }
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_os_string();
    s.push(".part");
    PathBuf::from(s)
}

#[derive(Debug)]
struct StallConnector(Duration);

#[derive(Debug)]
struct StallTransport {
    inner: Box<dyn ureq::unversioned::transport::Transport>,
    read_timeout: ureq::unversioned::transport::time::Duration,
}

impl ureq::unversioned::transport::Connector<Box<dyn ureq::unversioned::transport::Transport>> for StallConnector {
    type Out = StallTransport;

    fn connect(
        &self,
        _details: &ureq::unversioned::transport::ConnectionDetails,
        chained: Option<Box<dyn ureq::unversioned::transport::Transport>>,
    ) -> Result<Option<Self::Out>, ureq::Error> {
        Ok(chained.map(|inner| StallTransport { inner, read_timeout: self.0.into() }))
    }
}

impl ureq::unversioned::transport::Transport for StallTransport {
    fn buffers(&mut self) -> &mut dyn ureq::unversioned::transport::Buffers {
        self.inner.buffers()
    }

    fn transmit_output(&mut self, amount: usize, timeout: ureq::unversioned::transport::NextTimeout) -> Result<(), ureq::Error> {
        self.inner.transmit_output(amount, timeout)
    }

    fn await_input(&mut self, mut timeout: ureq::unversioned::transport::NextTimeout) -> Result<bool, ureq::Error> {
        timeout.after = timeout.after.min(self.read_timeout);
        self.inner.await_input(timeout)
    }

    fn is_open(&mut self) -> bool {
        self.inner.is_open()
    }

    fn is_tls(&self) -> bool {
        self.inner.is_tls()
    }
}

fn agent(url: &str, stall: Duration) -> ureq::Agent {
    use ureq::unversioned::resolver::DefaultResolver;
    use ureq::unversioned::transport::{Connector, DefaultConnector};

    let read_timeout = stall.saturating_add(Duration::from_millis(250));
    let config = ureq::config::Config::builder()
        .http_status_as_error(false)
        .user_agent("agentdeck")
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(Duration::from_secs(30).max(stall)))
        // Global body ceiling; each individual socket read has a timeout close to `stall`
        // (via `StallConnector`), preventing the `cd-download` thread from being orphaned for hours.
        .timeout_recv_body(Some(Duration::from_secs(4 * 3600)))
        // HTTPS is never downgraded to HTTP on redirect.
        .https_only(url.starts_with("https://"))
        .build();
    let connector = DefaultConnector::default().chain(StallConnector(read_timeout));
    ureq::Agent::with_parts(config, connector, DefaultResolver::default())
}

fn classify(e: ureq::Error) -> DownloadError {
    use ureq::Error::*;
    let msg = e.to_string();
    match e {
        BadUri(_) | Http(_) | TooManyRedirects | RedirectFailed | RequireHttpsOnly(_) | InvalidProxyUrl | Tls(_) | Pem(_) | Rustls(_) => {
            DownloadError::fatal(msg)
        }
        _ => DownloadError::retry(msg),
    }
}

/// Message read by network worker.
enum Msg {
    Data(Vec<u8>),
    Eof,
    Failed(String),
}

/// Downloads the remaining bytes into `.part`, resuming from its current size.
fn fetch_to_part(url: &str, part: &Path, stall: Duration, rep: &mut Reporter) -> Result<(), DownloadError> {
    let offset = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0);
    let mut req = agent(url, stall).get(url).header("Accept-Encoding", "identity");
    if offset > 0 {
        req = req.header("Range", format!("bytes={offset}-"));
    }
    let resp = req.call().map_err(classify)?;
    let status = resp.status().as_u16();
    let header = |name: &str| resp.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_owned);

    if status == 416 {
        // Requested past end: .part is already complete (total == offset) or is corrupted.
        let total = header("content-range").and_then(|v| v.rsplit('/').next().and_then(|n| n.trim().parse::<u64>().ok()));
        if offset > 0 && total == Some(offset) {
            return Ok(());
        }
        let _ = std::fs::remove_file(part);
        return Err(DownloadError::retry(t!("invalid partial file; restarting from scratch")));
    }
    if status != 200 && status != 206 {
        return Err(DownloadError {
            message: t!("HTTP {status} downloading {url}", status, url),
            retryable: status == 408 || status == 429 || status >= 500,
        });
    }

    let (start, total) = if status == 206 {
        let cr = header("content-range").unwrap_or_default();
        // "bytes START-END/TOTAL"
        let parsed = cr.strip_prefix("bytes ").and_then(|r| {
            let (range, total) = r.split_once('/')?;
            let (s, _) = range.split_once('-')?;
            Some((s.trim().parse::<u64>().ok()?, total.trim().parse::<u64>().ok()))
        });
        match parsed {
            Some((s, total)) if s == offset => (offset, total),
            _ => {
                let _ = std::fs::remove_file(part);
                return Err(DownloadError::retry(t!("inconsistent partial response; restarting from scratch")));
            }
        }
    } else {
        // 200: the server ignored Range (or this is the first request). Use entire body.
        (0, header("content-length").and_then(|v| v.trim().parse::<u64>().ok()))
    };

    let mut file = if start == 0 { File::create(part) } else { OpenOptions::new().append(true).open(part) }
        .map_err(|e| DownloadError::fatal(t!("could not write {path}: {error}", path = part.display(), error = e)))?;

    rep.begin(start, total);

    // Network read runs on a separate thread: the main thread only awaits chunks with a deadline,
    // thus detecting stalled connections (ureq lacks an inactivity deadline per read).
    let (tx, rx) = bounded::<Msg>(8);
    let mut reader = resp.into_body().into_reader();
    std::thread::Builder::new()
        .name("cd-download".into())
        .spawn(move || loop {
            let mut buf = vec![0u8; CHUNK];
            match reader.read(&mut buf) {
                Ok(0) => {
                    let _ = tx.send(Msg::Eof);
                    return;
                }
                Ok(n) => {
                    buf.truncate(n);
                    if tx.send(Msg::Data(buf)).is_err() {
                        return; // main thread gave up (stalled connection)
                    }
                }
                Err(e) => {
                    let _ = tx.send(Msg::Failed(e.to_string()));
                    return;
                }
            }
        })
        .map_err(|e| DownloadError::fatal(t!("could not start download: {error}", error = e)))?;

    loop {
        match rx.recv_timeout(stall) {
            Ok(Msg::Data(chunk)) => {
                file.write_all(&chunk)
                    .map_err(|e| DownloadError::fatal(t!("failed to write {path}: {error}", path = part.display(), error = e)))?;
                rep.bytes(chunk.len());
            }
            Ok(Msg::Eof) => break,
            Ok(Msg::Failed(e)) => return Err(DownloadError::retry(t!("connection interrupted: {error}", error = e))),
            Err(RecvTimeoutError::Timeout) => {
                return Err(DownloadError::retry(t!("connection stalled (no data for {seconds}s)", seconds = stall.as_secs().max(1))));
            }
            Err(RecvTimeoutError::Disconnected) => return Err(DownloadError::retry(t!("connection interrupted"))),
        }
    }
    file.flush().map_err(|e| DownloadError::fatal(t!("failed to write {path}: {error}", path = part.display(), error = e)))?;
    drop(file);

    if let Some(t) = total {
        if std::fs::metadata(part).map(|m| m.len()).unwrap_or(0) != t {
            return Err(DownloadError::retry(t!("incomplete download")));
        }
    }
    Ok(())
}

fn retry_io<T>(delays_ms: &[u64], mut op: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    let mut last_err = match op() {
        Ok(v) => return Ok(v),
        Err(e) => e,
    };
    for &ms in delays_ms {
        std::thread::sleep(Duration::from_millis(ms));
        match op() {
            Ok(v) => return Ok(v),
            Err(e) => last_err = e,
        }
    }
    Err(last_err)
}

/// Moves `from` to `to`, retrying a few times (Windows antivirus may hold newly closed `.part`
/// for a few milliseconds).
pub(crate) fn rename_with_retry(from: &Path, to: &Path) -> std::io::Result<()> {
    retry_io(&[25, 50, 100, 200, 400], || std::fs::rename(from, to))
}

/// A full attempt: downloads remaining data, verifies hash, and publishes the file.
fn attempt(
    url: &str,
    dest: &Path,
    part: &Path,
    opts: &DownloadOpts,
    mismatches: &mut u32,
    rep: &mut Reporter,
) -> Result<(), DownloadError> {
    fetch_to_part(url, part, opts.stall_timeout, rep)?;
    if let Some(expected) = opts.sha256 {
        let size = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0);
        rep.verifying(0, size);
        let actual = sha256_file(part, &mut |done, total| rep.verifying(done, total))
            .map_err(|e| DownloadError::fatal(t!("failed to read {path} to verify hash: {error}", path = part.display(), error = e)))?;
        let expected = expected.to_lowercase();
        if actual != expected {
            let _ = std::fs::remove_file(part);
            *mismatches += 1;
            return Err(if *mismatches > 1 {
                DownloadError::fatal(t!("SHA-256 does not match (expected {expected}, got {actual}); file discarded", expected, actual,))
            } else {
                DownloadError::retry(t!(
                    "SHA-256 does not match (expected {expected}, got {actual}); retrying from scratch",
                    expected,
                    actual,
                ))
            });
        }
    }
    rename_with_retry(part, dest).map_err(|e| DownloadError::fatal(t!("could not write {path}: {error}", path = dest.display(), error = e)))
}

/// Downloads `url` to `dest`. Only delivers file after verifying SHA-256 (when provided).
pub fn download_file(
    url: &str,
    dest: &Path,
    opts: &DownloadOpts,
    on_progress: &mut dyn FnMut(DownloadProgress),
) -> Result<(), DownloadError> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| DownloadError::fatal(e.to_string()))?;
    }
    let part = part_path(dest);
    let mut rep = Reporter::new(opts.label, on_progress);
    let (mut failures, mut mismatches) = (0u32, 0u32);
    loop {
        match attempt(url, dest, &part, opts, &mut mismatches, &mut rep) {
            Ok(()) => {
                let size = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
                rep.received = size;
                rep.total = Some(size);
                rep.state(DownloadState::Done);
                return Ok(());
            }
            Err(e) => {
                if rep.take_progress() {
                    failures = 0;
                }
                failures += 1;
                if !e.retryable || failures > opts.retries {
                    rep.emit(DownloadState::Error, 0, 0, 0, Some(e.message.clone()));
                    return Err(e);
                }
                let exp = opts.base_delay.saturating_mul(1u32 << (failures - 1).min(16));
                let jitter = 0.75
                    + (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0) % 500)
                        as f64
                        / 1000.0;
                let delay = exp.min(opts.max_delay).mul_f64(jitter);
                rep.emit(DownloadState::Retrying, failures, opts.retries, delay.as_millis() as u64, Some(e.message));
                std::thread::sleep(delay);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::{Arc, Mutex};
    use tiny_http::{Header, Response, Server, StatusCode};

    #[derive(Default, Clone)]
    struct Behavior {
        /// Bytes sent before dropping connection (for first `drop_times` body responses).
        drop_after: Option<usize>,
        drop_times: usize,
        ignore_range: bool,
        /// Status codes returned in first responses (no body).
        statuses: Vec<u16>,
        /// After `stall_after` body bytes, response hangs for `stall_for` (for first `stall_times` responses).
        stall_after: usize,
        stall_for: Option<Duration>,
        stall_times: usize,
    }

    /// Reader that delivers `limit` bytes and then fails, causing server to reset connection.
    struct Breaking {
        inner: Cursor<Vec<u8>>,
        limit: usize,
        sent: usize,
    }

    impl Read for Breaking {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.sent >= self.limit {
                return Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "test disconnect"));
            }
            let max = buf.len().min(self.limit - self.sent);
            let n = self.inner.read(&mut buf[..max])?;
            self.sent += n;
            Ok(n)
        }
    }

    /// Reader that hangs for a duration after `at` bytes (open connection, but no data).
    struct Stalling {
        inner: Cursor<Vec<u8>>,
        at: usize,
        sent: usize,
        pause: Duration,
        paused: bool,
    }

    impl Read for Stalling {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if !self.paused && self.sent >= self.at {
                self.paused = true;
                std::thread::sleep(self.pause);
            }
            let max = if self.paused { buf.len() } else { buf.len().min(self.at - self.sent) };
            let n = self.inner.read(&mut buf[..max])?;
            self.sent += n;
            Ok(n)
        }
    }

    /// (url, Range header) of each received request.
    type RequestLog = Arc<Mutex<Vec<(String, Option<String>)>>>;

    struct Fixture {
        base: String,
        log: RequestLog,
        server: Arc<Server>,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.server.unblock();
        }
    }

    fn body() -> Vec<u8> {
        (0..600 * 1024u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect()
    }

    fn sha(data: &[u8]) -> String {
        hex(&Sha256::digest(data))
    }

    fn header(k: &str, v: &str) -> Header {
        Header::from_bytes(k.as_bytes(), v.as_bytes()).expect("header")
    }

    fn serve(data: Vec<u8>, b: Behavior) -> Fixture {
        let server = Arc::new(Server::http("127.0.0.1:0").expect("bind"));
        let base = format!("http://{}", server.server_addr().to_ip().expect("ip"));
        let log = Arc::new(Mutex::new(Vec::new()));
        let (srv, lg) = (server.clone(), log.clone());
        std::thread::spawn(move || {
            let (mut served, mut drops, mut stalls) = (0usize, 0usize, 0usize);
            for req in srv.incoming_requests() {
                let range = req.headers().iter().find(|h| h.field.equiv("Range")).map(|h| h.value.to_string());
                lg.lock().expect("log").push((req.url().to_owned(), range.clone()));
                if req.url() == "/redirect" {
                    let _ = req.respond(Response::empty(302).with_header(header("Location", "/file")));
                    continue;
                }
                if let Some(code) = b.statuses.get(served).copied() {
                    served += 1;
                    let _ = req.respond(Response::empty(StatusCode(code)));
                    continue;
                }
                let start = range
                    .as_deref()
                    .and_then(|r| r.strip_prefix("bytes="))
                    .and_then(|r| r.strip_suffix('-'))
                    .and_then(|n| n.parse::<usize>().ok())
                    .filter(|_| !b.ignore_range)
                    .unwrap_or(0);
                if start >= data.len() {
                    let _ = req.respond(Response::empty(416).with_header(header("Content-Range", &format!("bytes */{}", data.len()))));
                    continue;
                }
                let chunk = data[start..].to_vec();
                let len = chunk.len();
                let (code, mut headers) = if start > 0 {
                    (206, vec![header("Content-Range", &format!("bytes {start}-{}/{}", data.len() - 1, data.len()))])
                } else {
                    (200, Vec::new())
                };
                headers.push(header("Content-Type", "application/octet-stream"));
                // Like GitHub and Hugging Face: always with Content-Length (never chunked).
                let never_chunked = usize::MAX;
                if let (Some(n), true) = (b.drop_after, drops < b.drop_times) {
                    drops += 1;
                    let reader = Breaking { inner: Cursor::new(chunk), limit: n, sent: 0 };
                    let _ = req
                        .respond(Response::new(StatusCode(code), headers, reader, Some(len), None).with_chunked_threshold(never_chunked));
                } else if let (Some(pause), true) = (b.stall_for, stalls < b.stall_times) {
                    stalls += 1;
                    let reader = Stalling { inner: Cursor::new(chunk), at: b.stall_after, sent: 0, pause, paused: false };
                    let _ = req
                        .respond(Response::new(StatusCode(code), headers, reader, Some(len), None).with_chunked_threshold(never_chunked));
                } else {
                    let _ = req.respond(
                        Response::new(StatusCode(code), headers, Cursor::new(chunk), Some(len), None).with_chunked_threshold(never_chunked),
                    );
                }
            }
        });
        Fixture { base, log, server }
    }

    fn fast<'a>(label: &'static str, sha256: Option<&'a str>) -> DownloadOpts<'a> {
        let mut o = DownloadOpts::new(label, sha256);
        o.base_delay = Duration::from_millis(5);
        o.max_delay = Duration::from_millis(40);
        o.stall_timeout = Duration::from_secs(3);
        o
    }

    fn run(url: &str, dest: &Path, opts: &DownloadOpts) -> (Result<(), DownloadError>, Vec<DownloadProgress>) {
        let mut events = Vec::new();
        let r = download_file(url, dest, opts, &mut |p| events.push(p));
        (r, events)
    }

    #[test]
    fn downloads_verifies_hash_and_reports_progress() {
        let data = body();
        let fx = serve(data.clone(), Behavior::default());
        let dir = tempfile::tempdir().expect("tmp");
        let dest = dir.path().join("out.bin");
        let digest = sha(&data);
        let (r, ev) = run(&format!("{}/file", fx.base), &dest, &fast("test", Some(&digest)));
        r.expect("download");
        assert_eq!(std::fs::read(&dest).expect("read"), data);
        assert!(!part_path(&dest).exists());
        let states: Vec<_> = ev.iter().map(|e| e.state).collect();
        assert!(states.contains(&DownloadState::Downloading) && states.contains(&DownloadState::Verifying));
        assert_eq!(ev.last().expect("event").state, DownloadState::Done);
        assert_eq!(ev.iter().find(|e| e.state == DownloadState::Downloading).expect("dl").total, Some(data.len() as u64));
    }

    #[test]
    fn follows_redirects() {
        let data = body();
        let fx = serve(data.clone(), Behavior::default());
        let dir = tempfile::tempdir().expect("tmp");
        let dest = dir.path().join("out.bin");
        run(&format!("{}/redirect", fx.base), &dest, &fast("test", Some(&sha(&data)))).0.expect("download");
        assert_eq!(std::fs::read(&dest).expect("read"), data);
    }

    #[test]
    fn resumes_with_range_when_connection_drops() {
        let data = body();
        let fx = serve(data.clone(), Behavior { drop_after: Some(200 * 1024), drop_times: 1, ..Default::default() });
        let dir = tempfile::tempdir().expect("tmp");
        let dest = dir.path().join("out.bin");
        let (r, ev) = run(&format!("{}/file", fx.base), &dest, &fast("test", Some(&sha(&data))));
        r.expect("download");
        assert_eq!(std::fs::read(&dest).expect("read"), data);
        let ranged: Vec<_> = fx.log.lock().expect("log").iter().filter_map(|(_, r)| r.clone()).collect();
        assert_eq!(ranged.len(), 1, "{ranged:?}");
        let from: u64 = ranged[0].trim_start_matches("bytes=").trim_end_matches('-').parse().expect("offset");
        assert!(from > 0 && from < data.len() as u64);
        assert!(ev.iter().any(|e| e.state == DownloadState::Retrying));
    }

    #[test]
    fn restarts_from_scratch_if_server_ignores_range() {
        let data = body();
        let fx = serve(data.clone(), Behavior { drop_after: Some(150 * 1024), drop_times: 1, ignore_range: true, ..Default::default() });
        let dir = tempfile::tempdir().expect("tmp");
        let dest = dir.path().join("out.bin");
        run(&format!("{}/file", fx.base), &dest, &fast("test", Some(&sha(&data)))).0.expect("download");
        assert_eq!(std::fs::read(&dest).expect("read"), data);
    }

    #[test]
    fn resumes_a_part_from_previous_run() {
        let data = body();
        let fx = serve(data.clone(), Behavior::default());
        let dir = tempfile::tempdir().expect("tmp");
        let dest = dir.path().join("out.bin");
        std::fs::write(part_path(&dest), &data[..100 * 1024]).expect("part");
        run(&format!("{}/file", fx.base), &dest, &fast("test", Some(&sha(&data)))).0.expect("download");
        assert_eq!(std::fs::read(&dest).expect("read"), data);
        assert_eq!(fx.log.lock().expect("log")[0].1.as_deref(), Some(format!("bytes={}-", 100 * 1024).as_str()));
    }

    #[test]
    fn accepts_a_part_that_is_already_complete() {
        let data = body();
        let fx = serve(data.clone(), Behavior::default());
        let dir = tempfile::tempdir().expect("tmp");
        let dest = dir.path().join("out.bin");
        std::fs::write(part_path(&dest), &data).expect("part");
        run(&format!("{}/file", fx.base), &dest, &fast("test", Some(&sha(&data)))).0.expect("download");
        assert_eq!(std::fs::read(&dest).expect("read"), data);
    }

    #[test]
    fn retries_on_503() {
        let data = body();
        let fx = serve(data.clone(), Behavior { statuses: vec![503, 503], ..Default::default() });
        let dir = tempfile::tempdir().expect("tmp");
        let dest = dir.path().join("out.bin");
        let (r, ev) = run(&format!("{}/file", fx.base), &dest, &fast("test", Some(&sha(&data))));
        r.expect("download");
        assert_eq!(ev.iter().filter(|e| e.state == DownloadState::Retrying).count(), 2);
        assert_eq!(fx.log.lock().expect("log").len(), 3);
    }

    #[test]
    fn does_not_retry_on_404() {
        let fx = serve(body(), Behavior { statuses: vec![404], ..Default::default() });
        let dir = tempfile::tempdir().expect("tmp");
        let dest = dir.path().join("out.bin");
        let (r, ev) = run(&format!("{}/file", fx.base), &dest, &fast("test", None));
        assert!(r.expect_err("404").message.contains("HTTP 404"));
        assert_eq!(fx.log.lock().expect("log").len(), 1);
        assert!(!dest.exists());
        assert_eq!(ev.last().expect("event").state, DownloadState::Error);
    }

    #[test]
    fn gives_up_when_retries_without_progress_are_exhausted() {
        let fx = serve(body(), Behavior { statuses: vec![500, 500, 500, 500], ..Default::default() });
        let dir = tempfile::tempdir().expect("tmp");
        let dest = dir.path().join("out.bin");
        let mut opts = fast("teste", None);
        opts.retries = 2;
        let err = run(&format!("{}/file", fx.base), &dest, &opts).0.expect_err("500");
        assert!(err.message.contains("HTTP 500"));
        assert_eq!(fx.log.lock().expect("log").len(), 3);
    }

    #[test]
    fn bad_hash_discards_retries_once_from_scratch_and_fails() {
        let fx = serve(body(), Behavior::default());
        let dir = tempfile::tempdir().expect("tmp");
        let dest = dir.path().join("out.bin");
        let err = run(&format!("{}/file", fx.base), &dest, &fast("test", Some(&"a".repeat(64)))).0.expect_err("hash");
        assert!(err.message.contains("SHA-256 does not match") && err.message.contains("discarded"), "{err}");
        assert!(!dest.exists() && !part_path(&dest).exists());
        assert_eq!(fx.log.lock().expect("log").len(), 2);
    }

    #[test]
    fn detects_stalled_connection_and_retries() {
        let data = body();
        let fx = serve(
            data.clone(),
            Behavior { stall_after: 100 * 1024, stall_for: Some(Duration::from_millis(1500)), stall_times: 1, ..Default::default() },
        );
        let dir = tempfile::tempdir().expect("tmp");
        let dest = dir.path().join("out.bin");
        let digest = sha(&data);
        let mut opts = fast("teste", Some(&digest));
        opts.stall_timeout = Duration::from_millis(300);
        let (r, ev) = run(&format!("{}/file", fx.base), &dest, &opts);
        r.expect("download");
        assert!(
            ev.iter().any(|e| e.state == DownloadState::Retrying
                && e.message.as_deref().is_some_and(|m| m.contains("stalled") || m.contains("timeout"))),
            "{ev:?}"
        );
        assert_eq!(std::fs::read(&dest).expect("read"), data);
    }

    /// Opt-in (`cargo test -p agentdeck-core -- --ignored`): uses actual GitHub and Hugging Face.
    #[test]
    #[ignore = "uses network"]
    fn real_network_github_resumes_and_huggingface_honors_range() {
        const URL: &str = "https://github.com/ggml-org/whisper.cpp/releases/download/b5454/whisper-bin-x64.zip";
        const SHA: &str = "6ba69e3482d7826214f90a6a9c84ca07782aec1e1d0c6a7c30c994fd5d816ccb";
        let dir = tempfile::tempdir().expect("tmp");
        let dest = dir.path().join("cpu.zip");

        // Seeds .part with first 3 MB (Range request through GitHub redirect).
        let seed = ureq::get(URL).header("Range", "bytes=0-2999999").call().expect("seed");
        assert_eq!(seed.status().as_u16(), 206);
        let mut bytes = Vec::new();
        seed.into_body().into_reader().read_to_end(&mut bytes).expect("read seed");
        assert_eq!(bytes.len(), 3_000_000);
        std::fs::write(part_path(&dest), &bytes).expect("part");

        let mut opts = DownloadOpts::new("cpu", Some(SHA));
        opts.stall_timeout = Duration::from_secs(30);
        let (r, ev) = run(URL, &dest, &opts);
        r.expect("real download");
        assert_eq!(
            ev.iter().find(|e| e.state == DownloadState::Downloading).expect("dl").received,
            3_000_000,
            "should resume from byte 3,000,000"
        );
        assert_eq!(std::fs::metadata(&dest).expect("meta").len(), 8_928_640);

        // Hugging Face: Range must survive redirect to CDN.
        let hf = ureq::get("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin")
            .header("Range", "bytes=1000000-1000999")
            .call()
            .expect("hf");
        assert_eq!(hf.status().as_u16(), 206);
        assert!(hf.headers().get("content-range").and_then(|v| v.to_str().ok()).is_some_and(|v| v.ends_with("/1624555275")));
    }

    #[test]
    fn sha256_of_known_vector() {
        let dir = tempfile::tempdir().expect("tmp");
        let f = dir.path().join("abc");
        std::fs::write(&f, b"abc").expect("write");
        assert_eq!(sha256_file(&f, &mut |_, _| {}).expect("sha"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn socket_read_expires_near_stall_timeout() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let srv = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut req = [0u8; 512];
            let _ = stream.read(&mut req);
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n").expect("write");
            // Hold connection open without sending body: reader should fail due to socket timeout.
            std::thread::sleep(Duration::from_secs(3));
        });
        let started = Instant::now();
        let resp = agent(&format!("http://{addr}/x"), Duration::from_millis(200)).get(&format!("http://{addr}/x")).call().expect("headers");
        let mut buf = [0u8; 64];
        let res = resp.into_body().into_reader().read(&mut buf);
        assert!(res.is_err(), "read should fail due to timeout");
        assert!(started.elapsed() < Duration::from_millis(2500), "took {:?}", started.elapsed());
        let _ = srv.join();
    }

    #[test]
    fn retry_io_survives_transient_failures_in_rename() {
        let mut tries = 0;
        let r = retry_io(&[5, 5, 5], || {
            tries += 1;
            if tries < 3 {
                Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "antivirus"))
            } else {
                Ok(42)
            }
        });
        assert_eq!(r.expect("ok on 3rd attempt"), 42);
        assert_eq!(tries, 3);

        let dir = tempfile::tempdir().expect("tmp");
        let part = dir.path().join("a.bin.part");
        let dest = dir.path().join("a.bin");
        std::fs::write(&part, b"dados").expect("write");
        rename_with_retry(&part, &dest).expect("rename");
        assert_eq!(std::fs::read(&dest).expect("read"), b"dados");
    }
}
