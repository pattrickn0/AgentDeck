//! What holds a folder that will not go away (a task worktree when validating or discarding): the files that were left
//! and, on Windows, the processes that have them open (Restart Manager, the same thing the Windows installer uses to
//! say "close these programs"). It only serves the log and the message: nothing here closes a process.

use std::path::{Path, PathBuf};

use crate::t;

/// How many leftover files go into the message (the log takes the same).
const MAX_FILES: usize = 10;
/// How many entries the search walks over, at most (a worktree with `target/` can have tens of thousands).
const MAX_WALK: usize = 20_000;

/// What was left of a folder that did not go away.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Leftovers {
    /// Up to [`MAX_FILES`] files that still exist.
    pub files: Vec<PathBuf>,
    /// How many files were left in total (as far as the search went).
    pub total: usize,
    /// The processes that have one of those files open ("holt.exe (pid 123)"); empty outside Windows.
    pub holders: Vec<String>,
}

impl Leftovers {
    /// A sentence for the task warning: which files were left and who holds them, or the hint that only empty
    /// folders were left (a process with its current directory inside it, such as the agent's terminal).
    pub fn describe(&self, dir: &Path) -> String {
        if self.total == 0 {
            return t!("only empty folders were left: some process has its current directory inside it (a terminal, the \
                       agent, the editor)")
            .to_owned();
        }
        let shown: Vec<String> = self.files.iter().map(|f| f.strip_prefix(dir).unwrap_or(f).to_string_lossy().replace('\\', "/")).collect();
        let files = shown.join(", ");
        let more = self.total - shown.len();
        let mut text = if more > 0 {
            t!("{count} file(s) were left: {files}, and {more} more", count = self.total, files, more)
        } else {
            t!("{count} file(s) were left: {files}", count = self.total, files)
        };
        if !self.holders.is_empty() {
            text.push_str(&t!("; open by: {holders}", holders = self.holders.join(", ")));
        }
        text
    }
}

/// The files that still exist in `dir` and who has them open.
pub(crate) fn leftovers(dir: &Path) -> Leftovers {
    let mut files = Vec::new();
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    let mut walked = 0;
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            walked += 1;
            if walked > MAX_WALK {
                break;
            }
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(e.path()),
                Ok(_) => {
                    total += 1;
                    if files.len() < MAX_FILES {
                        files.push(e.path());
                    }
                }
                Err(_) => {}
            }
        }
    }
    let holders = holders(&files);
    Leftovers { files, total, holders }
}

/// The processes that have any of `files` open, through the Restart Manager.
#[cfg(windows)]
fn holders(files: &[PathBuf]) -> Vec<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows_sys::Win32::System::RestartManager::{
        RmEndSession, RmGetList, RmRegisterResources, RmStartSession, CCH_RM_SESSION_KEY, RM_PROCESS_INFO,
    };

    if files.is_empty() {
        return Vec::new();
    }
    let wide: Vec<Vec<u16>> = files.iter().map(|f| f.as_os_str().encode_wide().chain(std::iter::once(0)).collect()).collect();
    let names: Vec<*const u16> = wide.iter().map(|w| w.as_ptr()).collect();
    let mut session = 0u32;
    let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
    // SAFETY: `key` has the size the API asks for (CCH_RM_SESSION_KEY + 1); `session` is only used if it succeeds.
    if unsafe { RmStartSession(&mut session, 0, key.as_mut_ptr()) } != ERROR_SUCCESS {
        return Vec::new();
    }
    let mut out = Vec::new();
    // SAFETY: `names` points to NUL-terminated texts that live in `wide` until the end of the function.
    let registered = unsafe { RmRegisterResources(session, names.len() as u32, names.as_ptr(), 0, std::ptr::null(), 0, std::ptr::null()) };
    if registered == ERROR_SUCCESS {
        let mut infos: Vec<RM_PROCESS_INFO> = Vec::new();
        let (mut needed, mut count, mut reasons) = (0u32, 0u32, 0u32);
        // First asks how many they are; then reads (it can grow in between: try again a few times).
        for _ in 0..3 {
            infos.resize_with(needed as usize, Default::default);
            count = infos.len() as u32;
            // SAFETY: `infos` has `count` slots; the API writes at most that many and says in `needed` how many it needs.
            let r = unsafe { RmGetList(session, &mut needed, &mut count, infos.as_mut_ptr(), &mut reasons) };
            if r == ERROR_SUCCESS {
                infos.truncate(count as usize);
                break;
            }
            if r != ERROR_MORE_DATA {
                infos.clear();
                break;
            }
        }
        for info in infos.iter().take(count as usize) {
            let len = info.strAppName.iter().position(|&c| c == 0).unwrap_or(info.strAppName.len());
            let name = String::from_utf16_lossy(&info.strAppName[..len]);
            out.push(format!("{name} (pid {})", info.Process.dwProcessId));
        }
    }
    // SAFETY: closes the session opened above, once.
    unsafe { RmEndSession(session) };
    out.sort();
    out.dedup();
    out
}

#[cfg(not(windows))]
fn holders(_files: &[PathBuf]) -> Vec<String> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_files_left_behind_are_listed_and_described() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        std::fs::create_dir_all(dir.join("target/debug")).expect("mkdir");
        std::fs::create_dir_all(dir.join("vazia")).expect("mkdir");
        for i in 0..12 {
            std::fs::write(dir.join("target/debug").join(format!("f{i}.rlib")), b"x").expect("write");
        }
        let left = leftovers(dir);
        assert_eq!((left.total, left.files.len()), (12, MAX_FILES));
        let text = left.describe(dir);
        assert!(text.starts_with("12 file(s) were left: target/debug/f"), "{text}");
        assert!(text.contains(", and 2 more"), "{text}");
        // Only empty folders: the hint about the current directory.
        let empty = tempfile::tempdir().expect("tmp");
        std::fs::create_dir_all(empty.path().join("a/b")).expect("mkdir");
        assert!(leftovers(empty.path()).describe(empty.path()).starts_with("only empty folders were left"));
    }

    #[cfg(windows)]
    #[test]
    fn a_file_held_open_names_this_process() {
        let tmp = tempfile::tempdir().expect("tmp");
        let file = tmp.path().join("held.txt");
        std::fs::write(&file, b"x").expect("write");
        let _open = std::fs::File::open(&file).expect("open");
        let held = holders(&[file]);
        let me = format!("(pid {})", std::process::id());
        assert!(held.iter().any(|h| h.ends_with(&me)), "{held:?}");
    }
}
