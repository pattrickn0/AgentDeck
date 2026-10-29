---
title: On Wayland closing to the tray destroys the window, and the exit must drop Wayland objects early
summary: set_visible(false) is a no-op on Wayland: the × parks the GL context and the tray reopens a new window; App::exiting drops egui and the window before winit closes the connection.
touches: [crates/app/src/app.rs, crates/app/src/window.rs]
---
Wayland has no hidden window: winit's `set_visible(false)` does nothing, so the × left the window on screen. On
Wayland (`is_wayland`), `App::close_window` destroys the window with `GlWindow::park`, which keeps the GL context
(not current) in `App::parked`; `show_window` rebuilds it with `ParkedGl::reopen` on the same context, so egui's
textures and the `Deck` stay as they were. While parked `gl_window` is `None`, `hidden()` is true and the frame-less
`Deck::background` does the work, exactly as with a hidden window on X11/Windows.

A second trap, same cause: `EventLoop::run_app` closes winit's Wayland connection when it returns, before `App` is
dropped. egui-winit's clipboard (smithay-clipboard, with its own thread) and the window hold proxies of that
connection; dropped afterwards they crashed every exit with `SIGSEGV` in `wl_proxy_destroy`.

**Why:** found on the first real Wayland run (openSUSE Tumbleweed, GNOME, 2026-10-29); CI never opens a window.
**How to apply:** no GL call may run while parked (`App::exiting` skips `EguiGlow::destroy` then). Anything that owns
Wayland objects is dropped in `App::exiting`, not left for `Drop`. `GlWindow` fields drop surface, context, then
window. Wayland also ignores the window icon and position: the icon comes from `holt.desktop` matched by the app
id, which a release build installs (`crates/app/build.rs`). It backdates the files it writes: written "now", they
look newer than the run and `rerun-if-changed` would recompile the app on every build.
