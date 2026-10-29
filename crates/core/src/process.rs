//! Child process execution with output capture, timeout, cancellation and registry
//! for bulk termination (window closing).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// Running headless processes (equivalent to main.js `Set running`).
#[derive(Default)]
pub(crate) struct ProcessRegistry {
    next_id: AtomicU64,
    children: Mutex<HashMap<u64, Arc<Mutex<Child>>>>,
}

impl ProcessRegistry {
    pub(crate) fn add(&self, child: Arc<Mutex<Child>>) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.children.lock().insert(id, child);
        id
    }

    pub(crate) fn remove(&self, id: u64) {
        self.children.lock().remove(&id);
    }

    /// Kills all registered processes (and their descendants).
    pub fn kill_all(&self) {
        let list: Vec<Arc<Mutex<Child>>> = self.children.lock().values().cloned().collect();
        for child in list {
            kill_tree(&mut child.lock());
        }
    }
}

/// How the process terminated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Exit {
    /// Exited on its own (`None` code = terminated by signal / killed externally).
    Code(Option<i32>),
    TimedOut,
    Cancelled,
}

#[derive(Debug)]
pub(crate) struct Captured {
    pub stdout: String,
    pub stderr: String,
    pub exit: Exit,
}

/// Configures platform flags: no console window on Windows; own process group
/// on Unix (to kill the tree without receiving terminal Ctrl+C).
pub(crate) fn platform_flags(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
}

/// Windows: Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` for headless processes. The
/// handle only closes when the app terminates (even if it crashes), and the system kills the whole job
/// (the agent and whatever it launched): nothing is left running orphaned without the UI.
#[cfg(windows)]
mod job {
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    /// Job handle (only used by system calls, which accept any thread).
    pub(super) struct Job(pub(super) HANDLE);
    // SAFETY: a job handle is an opaque kernel value, valid on any thread.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: the handle belongs to this `Job` and closes once (the app's job is static and never dropped here).
            unsafe { CloseHandle(self.0) };
        }
    }

    /// Creates a job that kills its processes when the last handle closes.
    pub(super) fn kill_on_close() -> Option<Job> {
        // SAFETY: Win32 calls with null pointers (unnamed, default security) and a
        // zeroed struct of the reported size; the handle is closed if configuration fails.
        unsafe {
            let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if h.is_null() {
                return None;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let size = std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
            let job = Job(h);
            if SetInformationJobObject(job.0, JobObjectExtendedLimitInformation, (&raw const info).cast(), size) == 0 {
                return None;
            }
            Some(job)
        }
    }

    pub(super) fn assign(job: &Job, child: &Child) -> bool {
        // SAFETY: process handle is valid while `child` exists; job handle while `job` exists.
        unsafe { AssignProcessToJobObject(job.0, child.as_raw_handle() as HANDLE) != 0 }
    }

    /// Puts the process in the app job (created on first use and never closed).
    pub(super) fn adopt(child: &Child) {
        static APP_JOB: OnceLock<Option<Job>> = OnceLock::new();
        match APP_JOB.get_or_init(kill_on_close) {
            Some(job) if assign(job, child) => {}
            _ => log::warn!("could not assign process {} to the app job", child.id()),
        }
    }
}

/// Makes the process die with the app, even if the app crashes (Windows Job Object; nothing to do elsewhere).
pub(crate) fn die_with_app(child: &Child) {
    #[cfg(windows)]
    job::adopt(child);
    #[cfg(not(windows))]
    let _ = child;
}

/// Kills the process and its descendants (best effort).
pub(crate) fn kill_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        let mut tk = Command::new("taskkill");
        tk.args(["/PID", &child.id().to_string(), "/T", "/F"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        platform_flags(&mut tk);
        let _ = tk.status();
    }
    #[cfg(unix)]
    {
        if let Ok(pid) = i32::try_from(child.id()) {
            // SAFETY: `kill` only sends a signal to the process group created by `process_group(0)`.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
    let _ = child.kill();
}

/// Reads `r` to the end keeping at most `max` bytes (the rest is read and discarded, so the
/// process does not hang on a full pipe).
fn spawn_reader<R: Read + Send + 'static>(mut r: R, buf: Arc<Mutex<Vec<u8>>>, max: usize) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        loop {
            match r.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let mut b = buf.lock();
                    let room = max.saturating_sub(b.len());
                    b.extend_from_slice(&chunk[..n.min(room)]);
                }
            }
        }
    })
}

/// Byte output (see [`run_captured_raw`]).
#[derive(Debug)]
pub(crate) struct CapturedRaw {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit: Exit,
}

/// Executes `cmd` capturing stdout/stderr. `stdin`: `Some` = writes and closes; `None` = null.
/// Terminates process tree on timeout or when `cancel` becomes `true`.
pub(crate) fn run_captured(
    cmd: Command,
    stdin: Option<Vec<u8>>,
    timeout: Duration,
    cancel: Option<&AtomicBool>,
    registry: Option<&ProcessRegistry>,
) -> std::io::Result<Captured> {
    let raw = run_captured_raw(cmd, stdin, timeout, cancel, registry, usize::MAX)?;
    let stdout = String::from_utf8_lossy(&raw.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&raw.stderr).into_owned();
    Ok(Captured { stdout, stderr, exit: raw.exit })
}

/// Like [`run_captured`], with output in bytes and at most `max_out` bytes from each stream (e.g. a
/// massive file diff does not consume all memory).
pub(crate) fn run_captured_raw(
    mut cmd: Command,
    stdin: Option<Vec<u8>>,
    timeout: Duration,
    cancel: Option<&AtomicBool>,
    registry: Option<&ProcessRegistry>,
    max_out: usize,
) -> std::io::Result<CapturedRaw> {
    cmd.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    platform_flags(&mut cmd);
    let mut child = cmd.spawn()?;
    // Registered (headless) processes die with the app, even if it crashes.
    #[cfg(windows)]
    if registry.is_some() {
        job::adopt(&child);
    }

    let out_buf = Arc::new(Mutex::new(Vec::new()));
    let err_buf = Arc::new(Mutex::new(Vec::new()));
    let mut readers = Vec::new();
    if let Some(out) = child.stdout.take() {
        readers.push(spawn_reader(out, out_buf.clone(), max_out));
    }
    if let Some(err) = child.stderr.take() {
        readers.push(spawn_reader(err, err_buf.clone(), max_out.max(64 * 1024)));
    }
    if let (Some(data), Some(mut pipe)) = (stdin, child.stdin.take()) {
        // Dedicated thread: a large prompt must not block output reading.
        std::thread::spawn(move || {
            let _ = pipe.write_all(&data);
        });
    }

    let child = Arc::new(Mutex::new(child));
    let reg_id = registry.map(|r| r.add(child.clone()));
    let started = Instant::now();
    let exit = loop {
        match child.lock().try_wait() {
            Ok(Some(status)) => break Exit::Code(status.code()),
            Ok(None) => {}
            Err(_) => break Exit::Code(None),
        }
        if started.elapsed() >= timeout {
            kill_tree(&mut child.lock());
            let _ = child.lock().wait();
            break Exit::TimedOut;
        }
        if cancel.is_some_and(|c| c.load(Ordering::SeqCst)) {
            kill_tree(&mut child.lock());
            let _ = child.lock().wait();
            break Exit::Cancelled;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    if let (Some(r), Some(id)) = (registry, reg_id) {
        r.remove(id);
    }

    // Grandchildren may hold pipes open; we do not wait for them indefinitely.
    let deadline = Instant::now() + Duration::from_secs(2);
    for h in readers {
        while !h.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if h.is_finished() {
            let _ = h.join();
        }
    }
    let stdout = std::mem::take(&mut *out_buf.lock());
    let stderr = std::mem::take(&mut *err_buf.lock());
    Ok(CapturedRaw { stdout, stderr, exit })
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    #[test]
    fn closing_the_job_kills_its_processes() {
        let job = job::kill_on_close().expect("job");
        let mut cmd = Command::new("cmd");
        // ~60 s ping: finishing in under 10 s only happens if the job killed it.
        cmd.args(["/d", "/c", "ping -n 60 127.0.0.1 >nul"]).stdout(Stdio::null()).stderr(Stdio::null());
        platform_flags(&mut cmd);
        let mut child = cmd.spawn().expect("spawn");
        assert!(job::assign(&job, &child));
        assert!(child.try_wait().expect("try_wait").is_none(), "still running");

        drop(job); // like the app crashing: the last job handle closes
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(s) = child.try_wait().expect("try_wait") {
                break Some(s);
            }
            if Instant::now() > deadline {
                break None;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let _ = child.kill();
        // (the system terminates processes of a closed job with code 0)
        assert!(status.is_some(), "process should die with the job");
    }

    #[test]
    fn headless_runs_go_into_the_app_job() {
        let reg = ProcessRegistry::default();
        let mut cmd = Command::new("cmd");
        cmd.args(["/d", "/c", "echo ok"]);
        let c = run_captured(cmd, None, Duration::from_secs(10), None, Some(&reg)).expect("run");
        assert_eq!(c.stdout.trim(), "ok");
        assert_eq!(c.exit, Exit::Code(Some(0)));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn captures_and_feeds_stdin() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "cat; echo error >&2; exit 3"]);
        let c = run_captured(cmd, Some(b"ola".to_vec()), Duration::from_secs(10), None, None).expect("run");
        assert_eq!(c.stdout, "ola");
        assert_eq!(c.stderr.trim(), "error");
        assert_eq!(c.exit, Exit::Code(Some(3)));
    }

    #[test]
    fn raw_output_is_capped_without_blocking_the_process() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "head -c 300000 /dev/zero; echo fim >&2"]);
        let c = run_captured_raw(cmd, None, Duration::from_secs(10), None, None, 1000).expect("run");
        assert_eq!(c.stdout.len(), 1000);
        assert_eq!(c.exit, Exit::Code(Some(0)), "process terminated: the rest was read and discarded");
        assert_eq!(String::from_utf8_lossy(&c.stderr).trim(), "fim");
    }

    #[test]
    fn timeout_kills() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 5"]);
        let t = Instant::now();
        let c = run_captured(cmd, None, Duration::from_millis(200), None, None).expect("run");
        assert_eq!(c.exit, Exit::TimedOut);
        assert!(t.elapsed() < Duration::from_secs(4));
    }

    #[test]
    fn registry_kill_all() {
        let reg = Arc::new(ProcessRegistry::default());
        let reg2 = reg.clone();
        let h = std::thread::spawn(move || {
            let mut cmd = Command::new("sh");
            cmd.args(["-c", "sleep 5"]);
            run_captured(cmd, None, Duration::from_secs(30), None, Some(&reg2)).expect("run")
        });
        std::thread::sleep(Duration::from_millis(200));
        reg.kill_all();
        let c = h.join().expect("join");
        assert!(matches!(c.exit, Exit::Code(None) | Exit::Code(Some(_))));
        assert_ne!(c.exit, Exit::Code(Some(0)));
    }
}
