//! CommandDeck Terminal — interface nativa (winit 0.30 + glutin/glow + egui).
//!
//! Porta de `renderer/` + `preload.js` da versão Electron. O backend fica em
//! `commanddeck_core::Core`; esta crate cuida da janela, dos terminais (PTY + emulação VT),
//! da árvore de arquivos, da "Conversa integrada", do compositor e do áudio.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agents;
mod app;
mod audio;
mod crash;
mod deck;
mod icons;
mod smoke;
mod terminal;
mod theme;
mod ui;
mod util;
mod viewer;
mod window;

use commanddeck_core::paths::AppPaths;
use commanddeck_core::Core;
use winit::event_loop::EventLoop;

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    crash::install(&AppPaths::detect().data_dir);

    let event_loop = EventLoop::<app::UserEvent>::with_user_event().build()?;
    let proxy = std::sync::Arc::new(parking_lot::Mutex::new(event_loop.create_proxy()));
    // O backend pode levar alguns segundos para iniciar (descoberta de CLIs/Ollama):
    // a janela abre já, e a interface aparece quando ele fica pronto.
    let (tx, rx) = crossbeam_channel::bounded(1);
    let wake = app::App::waker(&proxy);
    std::thread::Builder::new().name("core-start".into()).spawn(move || {
        let _ = tx.send(Core::start(AppPaths::detect()));
        wake();
    })?;
    let mut app = app::App::new(rx, proxy);
    event_loop.run_app(&mut app)?;
    Ok(())
}
