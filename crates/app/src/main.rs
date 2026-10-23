//! AgentDeck — interface nativa (winit 0.30 + glutin/glow + egui).
//!
//! Porta de `renderer/` + `preload.js` da versão Electron. O backend fica em
//! `agentdeck_core::Core`; esta crate cuida da janela, dos terminais (PTY + emulação VT),
//! da árvore de arquivos, da "Conversa integrada" e do áudio.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agents;
mod app;
mod audio;
mod crash;
mod deck;
mod dictation;
mod docs;
mod docs_demo;
mod git_ui;
mod github_ui;
mod icons;
mod logfile;
mod smoke;
mod split;
#[cfg(test)]
mod tasks_flow_tests;
mod tasks_ui;
mod terminal;
mod theme;
mod ui;
mod util;
mod window;

use agentdeck_core::paths::AppPaths;
use agentdeck_core::Core;
use winit::event_loop::EventLoop;

fn main() -> anyhow::Result<()> {
    // O `ureq-proto` despeja os cabeçalhos da requisição em TRACE, inclusive o `Authorization` com o
    // token do GitHub, e o `ureq` registra em DEBUG cada pedido (cabeçalhos, IPs). Os dois ficam em WARN,
    // mesmo com RUST_LOG=trace: o `filter_module` fixa o nível (não é um teto), e em DEBUG eles enchiam o log
    // a cada consulta de uso.
    // O log vai também para `<dados>/agentdeck.log`: o app de janela não tem stderr.
    let data_dir = AppPaths::detect().data_dir;
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .filter_module("ureq_proto", log::LevelFilter::Warn)
        .filter_module("ureq", log::LevelFilter::Warn)
        .target(env_logger::Target::Pipe(Box::new(logfile::LogFile::open(&data_dir))))
        .init();
    crash::install(&data_dir);

    let event_loop = EventLoop::<app::UserEvent>::with_user_event().build()?;
    let proxy = std::sync::Arc::new(parking_lot::Mutex::new(event_loop.create_proxy()));
    // O backend pode levar alguns segundos para iniciar (descoberta de CLIs):
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
