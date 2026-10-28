//! Adjustments to run under WSLg (WSL 2 with a graphical interface).
//!
//! WSLg shows the Linux windows through a Weston (Wayland) with an XWayland next to it. Weston 9's
//! Wayland client breaks with clients that draw their own decoration and resize: the frame comes out
//! wrong, the cursor disappears and Weston itself has already crashed (`rust-windowing/winit#4306`,
//! `microsoft/wslg#1008` and `#633`), while XWayland is the path the reports call stable. That is why,
//! on WSLg, the app opens through X11.
//!
//! Detection requires WSL **and** the `/mnt/wslg` folder (which only exists with the graphical
//! interface): on a WSL without WSLg, or with `DISPLAY` pointing outside, nothing changes.

/// The WSLg hints, separated from the system so they can be tested.
///
/// `distro` is `WSL_DISTRO_NAME`; `osrelease` is the content of `/proc/sys/kernel/osrelease` (on WSL it
/// mentions "microsoft"); `has_wslg_dir` is whether `/mnt/wslg` exists.
#[cfg(any(target_os = "linux", test))]
fn looks_like_wslg(distro: Option<&str>, osrelease: &str, has_wslg_dir: bool) -> bool {
    let wsl = distro.is_some_and(|d| !d.trim().is_empty()) || osrelease.to_ascii_lowercase().contains("microsoft");
    wsl && has_wslg_dir
}

/// `true` when the app is running inside WSLg.
#[cfg(target_os = "linux")]
pub fn on_wslg() -> bool {
    let distro = std::env::var("WSL_DISTRO_NAME").ok();
    let osrelease = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
    looks_like_wslg(distro.as_deref(), &osrelease, std::path::Path::new("/mnt/wslg").is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wslg_needs_the_wsl_marks_and_the_graphical_folder() {
        assert!(looks_like_wslg(Some("Ubuntu-24.04"), "5.15.90.1-microsoft-standard-WSL2", true));
        // Without WSL_DISTRO_NAME the kernel version still says it is WSL (WSL 1 and 2).
        assert!(looks_like_wslg(None, "4.4.0-19041-Microsoft", true));
        // WSL without a graphical interface, or with X11 pointing outside: the backend is chosen as always.
        assert!(!looks_like_wslg(Some("Ubuntu-24.04"), "5.15.90.1-microsoft-standard-WSL2", false));
        // Real Linux has none of the hints, even if the folder exists for some reason.
        assert!(!looks_like_wslg(None, "6.8.0-45-generic", true));
        assert!(!looks_like_wslg(Some(""), "6.8.0-45-generic", true));
    }
}
