//! Second opening of the app: instead of refusing ("another window is already using the data folder"), the new
//! instance asks the live one to show its window (`POST /activate` on the bridge) and exits silently.
//!
//! The live instance writes the bridge port and token to `data/.holt.bridge` after starting the bridge and
//! deletes the file on shutdown. The file alone does not prove that an instance exists (a crash leaves an old one):
//! the folder lock decides ([`crate::lock_data_dir`]), and whoever takes the lock deletes the stale file.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Port (line 1) and token (line 2) of the live instance's bridge.
const BRIDGE_FILE: &str = ".holt.bridge";
/// Migration (remove after 0.4, `memory/MAINTENANCE.md` §6): the same file as an AgentDeck left it, with the header
/// that bridge expects. A new Holt must find a still-running old instance instead of starting a second one.
const LEGACY_BRIDGE_FILE: &str = ".agentdeck.bridge";
const TOKEN_HEADER: &str = "x-holt-token";
const LEGACY_TOKEN_HEADER: &str = "x-agentdeck-token";
/// How long to wait for an instance that is still starting (lock taken, bridge not published yet).
const STARTUP_WAIT: Duration = Duration::from_secs(6);
const POLL: Duration = Duration::from_millis(200);
const POST_TIMEOUT: Duration = Duration::from_secs(3);

fn bridge_file(data_dir: &Path) -> PathBuf {
    data_dir.join(BRIDGE_FILE)
}

/// Publishes the bridge port and token for the second opening.
pub(crate) fn publish(data_dir: &Path, port: u16, token: &str) {
    if let Err(e) = std::fs::write(bridge_file(data_dir), format!("{port}\n{token}\n")) {
        log::warn!("could not write {}: {e}", bridge_file(data_dir).display());
    }
}

/// Deletes the bridge file (the instance is shutting down, or it took the lock and the one there belonged to a crash).
pub(crate) fn forget(data_dir: &Path) {
    let _ = std::fs::remove_file(bridge_file(data_dir));
}

/// The live instance's bridge (`None` while it has not published it yet) and the header its token goes in.
fn read(data_dir: &Path) -> Option<(u16, String, &'static str)> {
    let (text, header) = match std::fs::read_to_string(bridge_file(data_dir)) {
        Ok(text) => (text, TOKEN_HEADER),
        Err(_) => (std::fs::read_to_string(data_dir.join(LEGACY_BRIDGE_FILE)).ok()?, LEGACY_TOKEN_HEADER),
    };
    let mut lines = text.lines();
    let port = lines.next()?.trim().parse().ok()?;
    let token = lines.next()?.trim().to_owned();
    (!token.is_empty()).then_some((port, token, header))
}

/// Is `lock` (a file of `data_dir`) held by another handle?
pub(crate) fn lock_held(data_dir: &Path, lock: &str) -> bool {
    let Ok(file) = std::fs::OpenOptions::new().write(true).open(data_dir.join(lock)) else { return false };
    matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock))
}

/// Is there a live instance using `data_dir` (is the lock held by another handle, its own or an old AgentDeck's)?
fn running(data_dir: &Path) -> bool {
    lock_held(data_dir, crate::LOCK_FILE) || lock_held(data_dir, crate::LEGACY_LOCK_FILE)
}

/// Authenticated `POST /activate`. `true` = the instance answered 200.
fn post_activate(port: u16, token: &str, header: &str) -> bool {
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(POST_TIMEOUT)).build().into();
    match agent.post(format!("http://127.0.0.1:{port}/activate")).header(header, token).send_empty() {
        Ok(resp) => resp.status().as_u16() == 200,
        Err(e) => {
            log::warn!("the running instance did not answer /activate: {e}");
            false
        }
    }
}

/// Windows only lets a window come to the front for whoever received the user's focus: this process (which the user
/// just opened) hands that right to the live instance; otherwise the window just flashes on the taskbar.
#[cfg(windows)]
fn allow_foreground() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, ASFW_ANY};
    // SAFETY: call takes no pointers.
    unsafe {
        AllowSetForegroundWindow(ASFW_ANY);
    }
}

#[cfg(not(windows))]
fn allow_foreground() {}

/// If another instance of the app uses `data_dir`, asks it to show its window and returns `true` (this one may exit
/// silently). `false`: there is no live instance, or it did not answer (stuck): go on with the normal startup, which
/// shows the lock error.
pub fn activate_running(data_dir: &Path) -> bool {
    if !running(data_dir) {
        return false;
    }
    allow_foreground();
    let give_up = Instant::now() + STARTUP_WAIT;
    loop {
        if let Some((port, token, header)) = read(data_dir) {
            return post_activate(port, &token, header);
        }
        // No file: the instance is still starting. Wait until it publishes the bridge.
        if Instant::now() >= give_up || !running(data_dir) {
            return false;
        }
        std::thread::sleep(POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bridge_file_round_trips_and_ignores_garbage() {
        let tmp = tempfile::tempdir().expect("tmp");
        assert_eq!(read(tmp.path()), None);
        publish(tmp.path(), 4242, "abc123");
        assert_eq!(read(tmp.path()), Some((4242, "abc123".to_owned(), "x-holt-token")));
        std::fs::write(bridge_file(tmp.path()), "not a port\ntoken\n").expect("write");
        assert_eq!(read(tmp.path()), None);
        std::fs::write(bridge_file(tmp.path()), "80\n").expect("write");
        assert_eq!(read(tmp.path()), None, "without a token it does not count");
        forget(tmp.path());
        assert!(!bridge_file(tmp.path()).exists());
    }

    /// Migration: the bridge file of an old AgentDeck is found, and its token goes in the old header.
    #[test]
    fn the_bridge_file_of_an_old_agentdeck_is_found() {
        let tmp = tempfile::tempdir().expect("tmp");
        std::fs::write(tmp.path().join(LEGACY_BRIDGE_FILE), "4242\nabc123\n").expect("write");
        assert_eq!(read(tmp.path()), Some((4242, "abc123".to_owned(), "x-agentdeck-token")));
        publish(tmp.path(), 7, "new");
        assert_eq!(read(tmp.path()), Some((7, "new".to_owned(), "x-holt-token")), "the new file wins");
    }

    /// Migration: an old AgentDeck holding `.agentdeck.lock` counts as a running instance.
    #[test]
    fn a_running_old_agentdeck_is_seen_as_a_running_instance() {
        let tmp = tempfile::tempdir().expect("tmp");
        let old = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(tmp.path().join(crate::LEGACY_LOCK_FILE))
            .expect("open");
        old.try_lock().expect("lock");
        assert!(running(tmp.path()));
        drop(old);
        assert!(!running(tmp.path()));
    }

    #[test]
    fn without_a_live_instance_there_is_nothing_to_activate() {
        let tmp = tempfile::tempdir().expect("tmp");
        assert!(!running(tmp.path()), "without a lock file");
        // Old file from an instance that crashed: the lock is free, so there is nobody to ask.
        publish(tmp.path(), 1, "old");
        let _free = crate::lock_data_dir(tmp.path()).expect("lock");
        drop(_free);
        assert!(!activate_running(tmp.path()));
    }

    #[test]
    fn a_held_lock_is_seen_as_a_running_instance() {
        let tmp = tempfile::tempdir().expect("tmp");
        let held = crate::lock_data_dir(tmp.path()).expect("lock");
        assert!(running(tmp.path()));
        drop(held);
        assert!(!running(tmp.path()));
    }
}
