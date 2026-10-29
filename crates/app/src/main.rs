//! AgentDeck — native interface (winit 0.30 + glutin/glow + egui).
//!
//! Port of `renderer/` + `preload.js` from the Electron version. The backend lives in
//! `agentdeck_core::Core`; this crate takes care of the window, the terminals (PTY + VT emulation),
//! the file tree, the "Integrated conversation" and the audio.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agents;
mod app;
mod audio;
mod crash;
mod debug_ui;
mod deck;
mod dictation;
mod docs;
mod docs_demo;
mod git_ui;
mod github_ui;
mod highlight;
mod icons;
mod logfile;
mod smoke;
mod split;
#[cfg(test)]
mod tasks_flow_tests;
mod tasks_ui;
mod terminal;
mod theme;
mod tray;
mod ui;
mod util;
mod window;
mod wslg;

use agentdeck_core::paths::AppPaths;
use agentdeck_core::Core;
use winit::event_loop::EventLoop;

fn main() -> anyhow::Result<()> {
    // `ureq-proto` dumps the request headers at TRACE, including the `Authorization` with the
    // GitHub token, and `ureq` logs every request at DEBUG (headers, IPs). Both stay at WARN,
    // even with RUST_LOG=trace: `filter_module` pins the level (it is not a ceiling), and at
    // DEBUG they filled the log on every usage query. `zbus` (the Linux tray, over D-Bus) logs every menu
    // event at INFO: 139 of 156 lines of a session's log.
    // The log also goes to `<data>/agentdeck.log`: a window app has no stderr.
    let data_dir = AppPaths::detect().data_dir;
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .filter_module("ureq_proto", log::LevelFilter::Warn)
        .filter_module("ureq", log::LevelFilter::Warn)
        .filter_module("zbus", log::LevelFilter::Warn)
        .target(env_logger::Target::Pipe(Box::new(logfile::LogFile::open(&data_dir))))
        .init();
    crash::install(&data_dir);

    // Second launch: the live instance's window comes back (it may be hidden in the tray) and this one exits
    // silently. If it does not answer (hung), the normal startup continues, which shows the lock error.
    if agentdeck_core::activation::activate_running(&data_dir) {
        log::info!("another AgentDeck instance is already open: I asked it to show its window");
        return Ok(());
    }

    let mut event_loop_builder = EventLoop::<app::UserEvent>::with_user_event();
    // On WSLg the Wayland backend (Weston 9 + rdprail) breaks the window and even takes Weston down: the
    // XWayland next to it is the stable path. Outside WSLg, winit keeps choosing the backend, and an explicit
    // `WINIT_UNIX_BACKEND` wins (useful to test the Wayland from there).
    #[cfg(target_os = "linux")]
    if wslg::on_wslg() && std::env::var_os("WINIT_UNIX_BACKEND").is_none_or(|v| v.is_empty()) {
        log::info!("WSLg detected: opening through X11 (the WSLg Wayland breaks the window)");
        winit::platform::x11::EventLoopBuilderExtX11::with_x11(&mut event_loop_builder);
    }
    let event_loop = event_loop_builder.build()?;
    let proxy = std::sync::Arc::new(parking_lot::Mutex::new(event_loop.create_proxy()));
    // The backend can take a few seconds to start (CLI discovery):
    // the window opens right away, and the interface shows up when it is ready.
    let (tx, rx) = crossbeam_channel::bounded(1);
    let wake = app::App::waker(&proxy);
    std::thread::Builder::new().name("core-start".into()).spawn(move || {
        let _ = tx.send(Core::start(AppPaths::detect()));
        wake();
    })?;
    let mut app = app::App::new(rx, proxy);
    event_loop.run_app(&mut app)?;
    log::info!("event loop ended: exiting");
    Ok(())
}
