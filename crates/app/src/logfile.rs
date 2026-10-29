//! The app's log on disk: a window app on Windows has no stderr, so what `log` writes
//! (info level by default) also goes to `<dados>/agentdeck.log`.
//!
//! The file moves to `agentdeck.log.1` (replacing the previous one) when it grows past `MAX_LOG`;
//! the check runs on every write, not only when the app opens.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Size from which the log is rotated.
const MAX_LOG: u64 = 2 * 1024 * 1024;

/// `env_logger` destination: stderr (when there is one) and the file.
pub struct LogFile {
    path: PathBuf,
    file: Option<File>,
    size: u64,
    max: u64,
}

impl LogFile {
    /// Opens (or creates) `<dir>/agentdeck.log`. When it cannot open, only stderr gets the log.
    pub fn open(dir: &Path) -> Self {
        Self::with_max(dir.join("agentdeck.log"), MAX_LOG)
    }

    fn with_max(path: PathBuf, max: u64) -> Self {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut log = Self { path, file: None, size: 0, max };
        log.reopen();
        log
    }

    /// Opens the file to append, rotating first if it is already past the size.
    fn reopen(&mut self) {
        self.file = None;
        if std::fs::metadata(&self.path).is_ok_and(|m| m.len() > self.max) {
            let old = self.path.with_extension("log.1");
            let _ = std::fs::remove_file(&old);
            let _ = std::fs::rename(&self.path, &old);
        }
        self.file = OpenOptions::new().create(true).append(true).open(&self.path).ok();
        self.size = self.file.as_ref().and_then(|f| f.metadata().ok()).map_or(0, |m| m.len());
    }
}

impl Write for LogFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // Without a console (window app) stderr fails or goes nowhere: the file is what matters.
        let _ = std::io::stderr().write_all(buf);
        if self.size > self.max {
            self.reopen();
        }
        if let Some(file) = &mut self.file {
            if file.write_all(buf).is_ok() {
                self.size += buf.len() as u64;
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(file) = &mut self.file {
            let _ = file.flush();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_appends_to_the_file_and_rotates_when_it_grows_past_the_limit() {
        let dir = std::env::temp_dir().join(format!("cd-logfile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut log = LogFile::with_max(dir.join("agentdeck.log"), 100);
        log.write_all(b"first\n").unwrap();
        log.write_all(&[b'x'; 120]).unwrap();
        // Past the limit: the next write starts a new file and the previous one becomes .log.1.
        log.write_all(b"after\n").unwrap();
        log.flush().unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("agentdeck.log")).unwrap(), "after\n");
        let old = std::fs::read_to_string(dir.join("agentdeck.log.1")).unwrap();
        assert!(old.starts_with("first\n") && old.len() == "first\n".len() + 120);
        // Reopening the app continues the file that already exists.
        drop(log);
        let mut again = LogFile::with_max(dir.join("agentdeck.log"), 100);
        again.write_all(b"more\n").unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("agentdeck.log")).unwrap(), "after\nmore\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
