//! Crash reporting: release builds have no console, so panics and fatal crashes vanished
//! without a trace. Here they become lines in `<data>/crash.log`.
//!
//! - Panics: message + backtrace.
//! - Windows: only what crashes the app logs code, address and backtrace: heap corruption
//!   (`0xC0000374`) and buffer overrun / fast-fail (`0xC0000409`), which are never handled, and any
//!   unhandled exception (last-chance filter). First-chance access violations handled by the graphics
//!   driver no longer get recorded. The first layer does not allocate memory (the heap might be
//!   corrupted); the second attempts to resolve symbols.
//! - The file rotates to `crash.log.1` (replacing the previous one) when exceeding `MAX_LOG`.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

/// File opened at startup (cannot open files with corrupted heap).
static LOG: OnceLock<Mutex<File>> = OnceLock::new();

/// Size threshold to rotate log on app launch.
const MAX_LOG: u64 = 256 * 1024;

/// Rotates oversized log: `crash.log` becomes `crash.log.1` (replacing previous) and next starts empty.
fn rotate(path: &Path, max: u64) {
    if std::fs::metadata(path).is_ok_and(|m| m.len() > max) {
        let old = path.with_extension("log.1");
        let _ = std::fs::remove_file(&old);
        let _ = std::fs::rename(path, &old);
    }
}

/// Installs hooks. `dir` is the app data directory.
pub fn install(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    rotate(&dir.join("crash.log"), MAX_LOG);
    let Ok(file) = OpenOptions::new().create(true).append(true).open(dir.join("crash.log")) else { return };
    let _ = LOG.set(Mutex::new(file));

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        append(&format!("PANIC in thread {:?}: {info}\n{}", thread.name().unwrap_or("?"), std::backtrace::Backtrace::force_capture()));
        default_hook(info);
    }));

    #[cfg(windows)]
    windows::install();
    debug_trigger();
}

/// DEBUG ONLY: `AGENTDECK_DEBUG_CRASH=panic|heap` crashes the app 3 s after opening, to
/// verify crash logging works.
fn debug_trigger() {
    let Some(kind) = agentdeck_core::env_var("DEBUG_CRASH") else { return };
    let _ = std::thread::Builder::new().name("crash-test".into()).spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(3));
        match kind.as_str() {
            "panic" => panic!("test crash (AGENTDECK_DEBUG_CRASH=panic)"),
            "heap" => {
                // SAFETY: deliberate double free to corrupt heap; debug only.
                unsafe {
                    let block = Box::into_raw(Box::new([0u8; 64]));
                    drop(Box::from_raw(block));
                    drop(Box::from_raw(block));
                }
            }
            _ => {}
        }
    });
}

/// Appends an entry with local timestamp.
fn append(text: &str) {
    let Some(log) = LOG.get() else { return };
    let Ok(mut file) = log.lock() else { return };
    let _ = writeln!(file, "[{}] v{} {text}\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), env!("CARGO_PKG_VERSION"));
    let _ = file.flush();
}

#[cfg(windows)]
mod windows {
    use std::fmt::Write as _;
    use std::io::Write as _;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::OnceLock;

    use windows_sys::Win32::System::Diagnostics::Debug::{
        AddVectoredExceptionHandler, RtlCaptureStackBackTrace, SetUnhandledExceptionFilter, EXCEPTION_POINTERS,
        LPTOP_LEVEL_EXCEPTION_FILTER,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

    const HEAP_CORRUPTION: u32 = 0xC000_0374;
    const FAST_FAIL: u32 = 0xC000_0409;
    const CONTINUE_SEARCH: i32 = 0;

    static LOGGED: AtomicBool = AtomicBool::new(false);
    /// Last-chance filter present before ours (called after logging).
    static PREVIOUS: OnceLock<LPTOP_LEVEL_EXCEPTION_FILTER> = OnceLock::new();

    pub fn install() {
        // SAFETY: hooks only read exception record passed by system and return
        // CONTINUE_SEARCH (or whatever previous filter decides).
        unsafe {
            AddVectoredExceptionHandler(1, Some(handler));
            let _ = PREVIOUS.set(SetUnhandledExceptionFilter(Some(unhandled)));
        }
    }

    /// First chance: only codes that are never handled (process is going to crash). An access
    /// violation here may be handled by whoever caused it (OpenGL driver does this), so it
    /// is only logged if it reaches the unhandled filter.
    unsafe extern "system" fn handler(info: *mut EXCEPTION_POINTERS) -> i32 {
        // SAFETY: system guarantees valid pointers during call.
        let code = unsafe { (*(*info).ExceptionRecord).ExceptionCode as u32 };
        if matches!(code, HEAP_CORRUPTION | FAST_FAIL) {
            // SAFETY: passes same pointer, valid during this call.
            unsafe { log_fatal(info) };
        }
        CONTINUE_SEARCH
    }

    /// Last chance: nobody handled exception and process will crash.
    unsafe extern "system" fn unhandled(info: *const EXCEPTION_POINTERS) -> i32 {
        // SAFETY: system guarantees valid pointers during call.
        unsafe { log_fatal(info) };
        match PREVIOUS.get().copied().flatten() {
            // SAFETY: filter installed before ours, called with same argument.
            Some(prev) => unsafe { prev(info) },
            None => CONTINUE_SEARCH,
        }
    }

    /// Fixed stack buffer: formats without allocating.
    struct Stack {
        data: [u8; 2048],
        len: usize,
    }

    impl std::fmt::Write for Stack {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            let n = s.len().min(self.data.len() - self.len);
            self.data[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
            self.len += n;
            Ok(())
        }
    }

    /// Logs the exception that will crash the app (once only).
    ///
    /// # Safety
    /// `info` must be the valid pointer passed by system to the hook.
    unsafe fn log_fatal(info: *const EXCEPTION_POINTERS) {
        if LOGGED.swap(true, Ordering::SeqCst) {
            return;
        }
        // SAFETY: system guarantees valid pointers during call.
        let (code, address) = unsafe {
            let record = (*info).ExceptionRecord;
            ((*record).ExceptionCode as u32, (*record).ExceptionAddress as usize)
        };

        // Layer 1: non-allocating. Addresses relative to module, resolvable with matching build's .pdb.
        let mut frames = [std::ptr::null_mut::<std::ffi::c_void>(); 48];
        // SAFETY: `frames` holds 48 entries; hash is optional.
        let count = unsafe { RtlCaptureStackBackTrace(0, frames.len() as u32, frames.as_mut_ptr(), std::ptr::null_mut()) } as usize;
        // SAFETY: null = executable module.
        let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
        let mut out = Stack { data: [0; 2048], len: 0 };
        let _ = write!(out, "FATAL EXCEPTION 0x{code:08X} at 0x{address:X} (exe base 0x{base:X}); stack (offset in exe):");
        for frame in &frames[..count.min(frames.len())] {
            let ip = *frame as usize;
            // Only what falls within exe image becomes offset; remainder (ntdll, drivers) stays absolute.
            if ip >= base && ip - base < 0x2000_0000 {
                let _ = write!(out, " +0x{:X}", ip - base);
            } else {
                let _ = write!(out, " 0x{ip:X}");
            }
        }
        let _ = out.write_char('\n');
        if let Some(log) = super::LOG.get() {
            if let Ok(mut file) = log.try_lock() {
                let _ = file.write_all(&out.data[..out.len]);
                let _ = file.flush();
            }
        }

        // Layer 2: resolved stack (allocates; may fail if heap is severely damaged).
        let thread = std::thread::current();
        super::append(&format!("thread {:?}\n{}", thread.name().unwrap_or("?"), std::backtrace::Backtrace::force_capture()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_big_log_is_rotated_and_a_small_one_is_kept() {
        let dir = std::env::temp_dir().join(format!("cd-crash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("crash.log");
        std::fs::write(&log, "small").unwrap();
        rotate(&log, 100);
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "small");
        std::fs::write(&log, "x".repeat(101)).unwrap();
        std::fs::write(dir.join("crash.log.1"), "old").unwrap();
        rotate(&log, 100);
        assert!(!log.exists());
        assert_eq!(std::fs::read_to_string(dir.join("crash.log.1")).unwrap().len(), 101);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
