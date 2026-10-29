//! The winit loop (`ApplicationHandler`): window, GL/egui glue and event routing.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use glow::HasContext;
use holt_core::t;
use holt_core::{Core, CoreEvent};
use parking_lot::Mutex;
use winit::application::ApplicationHandler;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{Window, WindowAttributes, WindowId};

use crate::deck::Deck;
use crate::smoke::{self, Smoke};
use crate::terminal::Wake;
use crate::tray::{self, CloseAction, Tray, TrayCommand};
use crate::window::{GlWindow, ParkedGl};
use crate::{theme, ui};

#[derive(Debug, Clone, Copy)]
pub enum UserEvent {
    /// Something changed in the background (core event, PTY output, finished task).
    Wake,
    /// egui repaint request after `delay`.
    Repaint(Duration),
    /// The user used the tray icon.
    Tray(TrayCommand),
}

/// Result of `Core::start` (done on a thread: it can take a few seconds).
pub type CoreStart = anyhow::Result<(Core, Receiver<CoreEvent>)>;

pub struct App {
    proxy: Arc<Mutex<EventLoopProxy<UserEvent>>>,
    core: Option<Core>,
    core_rx: Receiver<CoreStart>,
    core_frame: u64,
    gl_window: Option<GlWindow>,
    /// Wayland, closed to the tray: the window was destroyed and its GL context waits here for the next one.
    parked: Option<ParkedGl>,
    gl: Option<Arc<glow::Context>>,
    egui: Option<egui_glow::EguiGlow>,
    deck: Option<Deck>,
    modifiers: ModifiersState,
    repaint_at: Option<Instant>,
    frames: u64,
    smoke: Option<Smoke>,
    dropped: Vec<PathBuf>,
    size_persist_at: Option<Instant>,
    /// Window state handed to egui every frame (`ViewportInfo::maximized` etc.): `EguiGlow` does not fill it.
    viewport_info: egui::ViewportInfo,
    viewport_first: bool,
    /// The window changed (size, position, focus, scale): rereads `viewport_info` on the next frame. Reading every
    /// frame would cost several calls to the windowing system, and the terminal asks for many frames.
    viewport_stale: bool,
    /// The `Deck` work that runs without a frame, while the window is hidden or minimized.
    background: Background,
    /// The tray icon; `None` = unavailable (the × then quits instead of hiding).
    tray: Option<Tray>,
}

/// `Deck::background` schedule: with the window hidden or minimized there is no `RedrawRequested` (on Windows, not even
/// `WM_PAINT`), so deliveries, process output and timers run from the loop events. No continuous loop: it runs when
/// something wakes the app (`dirty`) or at the deadline the work itself returned (`next`).
#[derive(Default)]
struct Background {
    /// Something changed since the last time (core event, PTY output, previous frame's deadline).
    dirty: bool,
    /// Next deadline returned by `Deck::background`.
    next: Option<Instant>,
    last: Option<Instant>,
}

/// Smallest interval between two runs due to `dirty`: a terminal that pours output wakes the app on every chunk.
const BACKGROUND_GAP: Duration = Duration::from_millis(50);

impl Background {
    fn wake(&mut self) {
        self.dirty = true;
    }

    /// A drawn frame does the same work: the schedule starts over when the window hides again.
    fn clear(&mut self) {
        *self = Self::default();
    }

    fn ran(&mut self, now: Instant, next: Option<Instant>) {
        self.dirty = false;
        self.last = Some(now);
        self.next = next;
    }

    fn due(&self, now: Instant) -> bool {
        self.deadline(now).is_some_and(|t| now >= t)
    }

    /// When the loop has to wake up to run again (`None`: only an event wakes it).
    fn deadline(&self, now: Instant) -> Option<Instant> {
        let woken = self.dirty.then(|| self.last.map_or(now, |l| l + BACKGROUND_GAP));
        [woken, self.next].into_iter().flatten().min()
    }
}

/// The window gets no frames: minimized (0×0 on Windows), with zero area or hidden. Drawing like that is worse than
/// useless: the `fit_tab` of a 0×0 terminal ruins the CLI (see `claude-code-without-esc3j-on-windows.md`).
fn hidden_by(minimized: Option<bool>, visible: Option<bool>, size: (u32, u32)) -> bool {
    minimized == Some(true) || visible == Some(false) || size.0 == 0 || size.1 == 0
}

fn is_hidden(w: &Window) -> bool {
    hidden_by(w.is_minimized(), w.is_visible(), w.inner_size().into())
}

/// Side (px) of the tray icon.
const TRAY_ICON_SIZE: usize = 32;

/// Smallest size saved/restored: below it the window is minimized (0x0 on Windows) or garbage.
const MIN_SAVED_SIZE: (f32, f32) = (640.0, 480.0);

/// Size to save in `Config`, or `None` to keep what is there. Maximized, the full-screen size
/// is not "the window size": saving it would make the normal window reopen (or restore) at the screen size.
fn size_to_persist(maximized: bool, size: (f32, f32)) -> Option<(f32, f32)> {
    (!maximized && size.0 >= MIN_SAVED_SIZE.0 && size.1 >= MIN_SAVED_SIZE.1).then_some(size)
}

impl App {
    pub fn new(core_rx: Receiver<CoreStart>, proxy: Arc<Mutex<EventLoopProxy<UserEvent>>>) -> Self {
        Self {
            proxy,
            core: None,
            core_rx,
            core_frame: 0,
            gl_window: None,
            parked: None,
            gl: None,
            egui: None,
            deck: None,
            modifiers: ModifiersState::empty(),
            repaint_at: None,
            frames: 0,
            smoke: Smoke::from_env(),
            dropped: Vec::new(),
            size_persist_at: None,
            viewport_info: egui::ViewportInfo::default(),
            viewport_first: true,
            viewport_stale: true,
            background: Background::default(),
            tray: None,
        }
    }

    /// Function that wakes the loop (used by the core and by the PTY threads).
    pub fn waker(proxy: &Arc<Mutex<EventLoopProxy<UserEvent>>>) -> Wake {
        let proxy = proxy.clone();
        // Coalesce: several wakes before the next frame become a single event.
        Arc::new(move || {
            let _ = proxy.lock().send_event(UserEvent::Wake);
        })
    }

    /// When the core finishes starting: creates the interface state.
    fn try_init_core(&mut self, event_loop: &ActiveEventLoop) {
        if self.core.is_some() || self.egui.is_none() {
            return;
        }
        let Ok(res) = self.core_rx.try_recv() else { return };
        let (core, events) = match res {
            Ok(v) => v,
            Err(e) => {
                log::error!("failed to start the backend: {e:#}");
                // No console on Windows: without the dialog, the app would just close with no explanation.
                rfd::MessageDialog::new()
                    .set_level(rfd::MessageLevel::Error)
                    .set_title("Holt")
                    .set_description(t!("Holt could not start: {error}", error = format!("{e:#}")))
                    .set_buttons(rfd::MessageButtons::Ok)
                    .show();
                event_loop.exit();
                return;
            }
        };
        core.set_waker(Self::waker(&self.proxy));
        // DEBUG ONLY: writes the bridge port and token for end-to-end tests with `ask-agent`.
        if let Some(file) = holt_core::env_var_os("DEBUG_BRIDGE") {
            let _ = std::fs::write(file, format!("{}\n{}\n", core.bridge_port(), core.bridge_token()));
        }
        // Saved window size (the window opens before the configuration is read).
        let cfg = core.config();
        if let Some(w) = self.window() {
            if size_to_persist(false, (cfg.window_width, cfg.window_height)).is_some() {
                let _ = w.request_inner_size(winit::dpi::LogicalSize::new(cfg.window_width as f64, cfg.window_height as f64));
            }
            // The normal size stays stored: when restored, the window goes back to it.
            if cfg.window_maximized {
                w.set_maximized(true);
            }
        }
        // DEBUG ONLY: screenshots and fake states do not write to the configuration (an "empty" or
        // "sidebar-collapsed" would stay saved and erase the real tabs).
        if self.smoke.is_some() || holt_core::env_var_os("DEBUG_UI").is_some() {
            core.disable_config_writes();
        }
        // Screenshots and fake states leave no icon in the user's tray.
        if self.smoke.is_none() && holt_core::env_var_os("DEBUG_UI").is_none() && holt_core::env_var_os("SMOKE").is_none() {
            let proxy = self.proxy.clone();
            let on_command: tray::OnCommand = Arc::new(move |cmd| {
                let _ = proxy.lock().send_event(UserEvent::Tray(cmd));
            });
            self.tray = Tray::start(crate::icons::logo_rgba(TRAY_ICON_SIZE, theme::pal().bg), TRAY_ICON_SIZE as u32, on_command);
        }
        let mut deck = Deck::new(core.clone(), events, Self::waker(&self.proxy));
        crate::debug_ui::apply(&mut deck);
        self.deck = Some(deck);
        self.core = Some(core);
        self.core_frame = self.frames + 1;
        self.request_redraw();
    }

    /// The window to create: at startup (the saved size comes later, with the configuration) and when Wayland
    /// reopens it from the tray (with the saved size).
    fn window_attributes(&self) -> WindowAttributes {
        let saved = self.core.as_ref().map(|c| c.config()).and_then(|c| size_to_persist(false, (c.window_width, c.window_height)));
        let (w, h) = saved.map_or((1280.0, 860.0), |(w, h)| (w as f64, h as f64));
        let attrs = WindowAttributes::default()
            .with_title("Holt")
            // The title bar is drawn by the interface (`ui/titlebar.rs`).
            .with_decorations(false)
            .with_inner_size(winit::dpi::LogicalSize::new(w, h))
            // The right bar (≥ 260px) and the terminal (≥ 360px) must fit next to the left side.
            .with_min_inner_size(winit::dpi::LogicalSize::new(820.0, 480.0))
            .with_window_icon(window_icon());
        // Without decoration, Windows also removes the window shadow; it comes back like this.
        #[cfg(windows)]
        let attrs = winit::platform::windows::WindowAttributesExtWindows::with_undecorated_shadow(attrs, true);
        // Wayland ignores the window icon: the desktop takes the icon and the name from the `.desktop` entry whose
        // name is the app id (`scripts/install-desktop.sh`). X11 matches it by `WM_CLASS`.
        #[cfg(target_os = "linux")]
        let attrs = {
            let attrs = winit::platform::wayland::WindowAttributesExtWayland::with_name(attrs, APP_ID, "");
            winit::platform::x11::WindowAttributesExtX11::with_name(attrs, APP_ID, APP_ID)
        };
        attrs
    }

    fn window(&self) -> Option<&Window> {
        self.gl_window.as_ref().map(|g| &g.window)
    }

    fn request_redraw(&self) {
        if let Some(w) = self.window() {
            w.request_redraw();
        }
    }

    /// Something changed: a frame (which does the `Deck` work and draws) or, with no frames, only the work.
    fn request_work(&mut self) {
        if self.hidden() {
            self.background.wake();
        } else {
            self.request_redraw();
        }
    }

    /// No frames (see `is_hidden`). Before the first frame the window is still born invisible and must be drawn.
    fn hidden(&self) -> bool {
        self.frames > 0 && self.window().is_none_or(is_hidden)
    }

    /// `Deck` works without a frame and the schedule keeps when to come back. With the window hidden, this is also where
    /// we see what only the frame-less work decides: exit (recording finished) and the second launch asking for the window.
    fn run_background(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let next = self.deck.as_mut().and_then(Deck::background);
        self.background.ran(now, next);
        if self.quit_if_requested(event_loop) {
            return;
        }
        if self.deck.as_mut().is_some_and(|d| std::mem::take(&mut d.activate_requested)) {
            self.show_window(event_loop);
        }
    }

    /// The app was told to quit (the tray's "Quit", or "Exit" after the question): shuts everything down.
    fn quit_if_requested(&mut self, event_loop: &ActiveEventLoop) -> bool {
        if !self.deck.as_ref().is_some_and(|d| d.quit_requested) {
            return false;
        }
        log::info!("quitting");
        self.shutdown();
        event_loop.exit();
        true
    }

    /// The user asked to quit (the tray's "Quit", or the × when there is no tray): asks first when there is an unsaved
    /// edit or an agent in the middle of a turn, showing the window for the question; otherwise quits now.
    fn request_quit(&mut self, event_loop: &ActiveEventLoop) {
        let Some(deck) = self.deck.as_mut() else {
            // The backend has not even started: there is no CLI to lose.
            event_loop.exit();
            return;
        };
        deck.request_quit();
        // To ask, the window must appear first.
        if deck.docs.confirm.is_some() {
            log::info!("quit asked: waiting for the answer to the question");
            self.show_window(event_loop);
        }
        self.quit_if_requested(event_loop);
    }

    /// Brings the window back: shows it, restores it if minimized and focuses it. Whoever hides it gets no notice from
    /// winit, and whoever shows it again needs a frame (the `request_redraw`).
    fn show_window(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(parked) = self.parked.take() {
            let attrs = self.window_attributes().with_visible(true);
            match parked.reopen(event_loop, attrs) {
                Ok(glw) => {
                    if self.core.as_ref().is_some_and(|c| c.config().window_maximized) {
                        glw.window.set_maximized(true);
                    }
                    self.gl_window = Some(glw);
                    self.viewport_first = true;
                }
                Err(e) => {
                    // Without a window and with the context gone there is no way back: quit like the tray's "Quit".
                    log::error!("could not reopen the window: {e}");
                    self.request_quit(event_loop);
                    return;
                }
            }
        }
        let Some(w) = self.window() else { return };
        Self::show(w);
        self.viewport_stale = true;
    }

    fn show(w: &Window) {
        w.set_visible(true);
        if w.is_minimized() == Some(true) {
            w.set_minimized(false);
        }
        w.focus_window();
        w.request_redraw();
    }

    /// The window's × (and the title bar's): hides it in the tray, without asking; the CLIs, the bridge and the
    /// deliveries keep running through `Deck::background`. With no tray, quits (see [`CloseAction::Quit`]).
    fn close_window(&mut self, event_loop: &ActiveEventLoop) {
        let Some(w) = self.gl_window.as_ref().map(|g| &g.window) else { return };
        if let Some(core) = self.core.as_ref() {
            persist_window(core, w);
        }
        match tray::close_action(self.tray.is_some()) {
            CloseAction::Hide => {
                // Wayland has no hidden window (`set_visible` does nothing): the window is destroyed and comes back
                // from the tray as a new one.
                if is_wayland(event_loop) {
                    match self.gl_window.take().map(GlWindow::park) {
                        Some(Ok(parked)) => self.parked = Some(parked),
                        Some(Err(e)) => {
                            log::error!("could not close the window to the tray: {e}");
                            self.request_quit(event_loop);
                            return;
                        }
                        None => {}
                    }
                } else {
                    w.set_visible(false);
                }
                // winit gives no notice when the window is hidden from code: the frame-less work starts here.
                self.background.wake();
            }
            CloseAction::Quit => self.request_quit(event_loop),
        }
    }

    /// The user used the tray icon.
    fn on_tray(&mut self, event_loop: &ActiveEventLoop, cmd: TrayCommand) {
        match cmd {
            TrayCommand::Open => self.show_window(event_loop),
            TrayCommand::Quit => self.request_quit(event_loop),
        }
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        // A frame request that arrives with the window hidden (the minimized one reports 0×0) becomes frame-less work.
        if self.hidden() {
            self.run_background(event_loop);
            return;
        }
        self.background.clear();
        let (Some(glw), Some(gl), Some(egui)) = (self.gl_window.as_ref(), self.gl.as_ref(), self.egui.as_mut()) else { return };
        let mut deck = self.deck.as_mut();
        // Without this `viewport().maximized` is always `None` (`EguiGlow` does not call `update_viewport_info`).
        if self.viewport_stale {
            egui_winit::update_viewport_info(&mut self.viewport_info, &egui.egui_ctx, &glw.window, self.viewport_first);
            self.viewport_first = false;
            self.viewport_stale = false;
        }
        egui.egui_winit.egui_input_mut().viewports.insert(egui::ViewportId::ROOT, self.viewport_info.clone());
        egui.run(&glw.window, |ui| match deck.as_deref_mut() {
            Some(deck) => {
                deck.before_frame(ui.ctx());
                ui::draw(deck, ui);
            }
            // While the backend starts: only the background.
            None => {
                ui.painter().rect_filled(ui.max_rect(), 0.0, theme::pal().bg);
            }
        });
        // SAFETY: GL context current on this thread.
        unsafe {
            let c = theme::pal().bg;
            gl.clear_color(c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
        }
        egui.paint(&glw.window);

        // The app's second launch asked for the window (already visible: just focuses it).
        if self.deck.as_mut().is_some_and(|d| std::mem::take(&mut d.activate_requested)) {
            Self::show(&glw.window);
        }
        // Quit (after the question, when there was something to ask).
        if self.deck.as_ref().is_some_and(|d| d.quit_requested) {
            self.shutdown();
            event_loop.exit();
            return;
        }
        // The title bar's "×": the same path as the system close.
        if self.deck.as_mut().is_some_and(|d| std::mem::take(&mut d.close_requested)) {
            self.close_window(event_loop);
            return;
        }

        if self.smoke.as_ref().is_some_and(|s| s.armed) {
            let size = glw.window.inner_size();
            let pixels = smoke::read_framebuffer(gl, size.width, size.height);
            let smoke = self.smoke.take().expect("smoke armed");
            let path = smoke.dir.join("screen.png");
            match smoke::save_png(&path, size.width, size.height, &pixels) {
                Ok(()) => log::info!("screenshot saved to {}", path.display()),
                Err(e) => log::error!("failed to save {}: {e}", path.display()),
            }
            self.shutdown();
            event_loop.exit();
            return;
        }

        if let Err(e) = glw.swap_buffers() {
            log::error!("swap_buffers: {e}");
        }
        self.frames += 1;
        if self.frames == 1 {
            glw.window.set_visible(true);
        }
        // A screenshot (HOLT_SMOKE) does not download whisper nor ask: every temporary folder would start 1.6 GB.
        if self.frames == self.core_frame && holt_core::env_var_os("SMOKE").is_none() {
            // After the first frame: whoever already uses dictation downloads what is missing in the background; whoever
            // was never asked sees the question (Whisper is optional and only downloads with a "yes").
            if let Some(deck) = self.deck.as_mut() {
                deck.dictation_startup();
            }
        }
    }

    fn shutdown(&mut self) {
        if let Some(deck) = self.deck.as_mut() {
            deck.shutdown();
        }
        let Some(core) = self.core.as_ref() else { return };
        if let Some(w) = self.window() {
            persist_window(core, w);
        }
        core.shutdown();
    }
}

/// Saves the window state: maximized or not, and the normal size (only when not maximized).
fn persist_window(core: &Core, w: &Window) {
    // Minimized, winit on Windows loses the "maximized": saving now would make the window reopen normal.
    if w.is_minimized() == Some(true) {
        return;
    }
    let maximized = w.is_maximized();
    let size = w.inner_size().to_logical::<f32>(w.scale_factor());
    let size = size_to_persist(maximized, (size.width, size.height));
    core.update_config(|c| {
        c.window_maximized = maximized;
        if let Some((width, height)) = size {
            c.window_width = width;
            c.window_height = height;
        }
    });
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gl_window.is_some() {
            return;
        }
        let attrs = self.window_attributes().with_visible(false);
        let (glw, gl) = match GlWindow::new(event_loop, attrs) {
            Ok(v) => v,
            Err(e) => {
                log::error!("could not create the OpenGL window: {e}");
                event_loop.exit();
                return;
            }
        };
        let egui = egui_glow::EguiGlow::new(event_loop, gl.clone(), None, None, true);
        let fonts = theme::install_fonts(&egui.egui_ctx);
        log::info!("fonts: ui={:?} mono={:?}", fonts.ui, fonts.mono);
        theme::install_style(&egui.egui_ctx);
        let proxy = self.proxy.clone();
        egui.egui_ctx.set_request_repaint_callback(move |info| {
            let _ = proxy.lock().send_event(UserEvent::Repaint(info.delay));
        });
        self.egui = Some(egui);
        self.gl = Some(gl);
        self.gl_window = Some(glw);
        // On Windows an invisible window gets no WM_PAINT: `request_redraw` never becomes
        // `RedrawRequested` and the first frame's `set_visible(true)` never runs. Draw right away.
        self.redraw(event_loop);
        self.try_init_core(event_loop);
    }

    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: StartCause) {
        if let StartCause::ResumeTimeReached { .. } = cause {
            self.request_redraw();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Wake => {
                self.try_init_core(event_loop);
                self.request_work();
            }
            UserEvent::Tray(cmd) => self.on_tray(event_loop, cmd),
            UserEvent::Repaint(delay) => {
                if delay.is_zero() {
                    self.request_work();
                } else if let Some(t) = Instant::now().checked_add(delay) {
                    self.repaint_at = Some(self.repaint_at.map_or(t, |r| r.min(t)));
                }
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        // Maximize, restore and minimize arrive as `Resized`.
        if matches!(
            event,
            WindowEvent::Resized(_)
                | WindowEvent::Moved(_)
                | WindowEvent::Focused(_)
                | WindowEvent::ScaleFactorChanged { .. }
                | WindowEvent::Occluded(_)
        ) {
            self.viewport_stale = true;
            // The window just vanished (minimize arrives as `Resized` 0×0): take over what the frame used to do.
            if self.hidden() {
                self.background.wake();
            }
        }
        if self.deck.is_none() {
            match event {
                WindowEvent::CloseRequested => event_loop.exit(),
                WindowEvent::RedrawRequested => self.redraw(event_loop),
                WindowEvent::Resized(size) => {
                    if let Some(g) = self.gl_window.as_ref() {
                        g.resize(size);
                        g.window.request_redraw();
                    }
                }
                _ => {}
            }
            return;
        }
        // The × (and Alt+F4, and closing from the taskbar) does not quit: it hides in the tray. Quitting is its menu.
        // Without a tray, it quits.
        if matches!(event, WindowEvent::CloseRequested) {
            self.close_window(event_loop);
            return;
        }
        let (Some(glw), Some(egui), Some(deck)) = (self.gl_window.as_ref(), self.egui.as_mut(), self.deck.as_mut()) else { return };
        let mut forward = true;
        match &event {
            WindowEvent::RedrawRequested => {
                self.redraw(event_loop);
                return;
            }
            WindowEvent::Resized(size) => {
                glw.resize(*size);
                self.size_persist_at = Some(Instant::now() + Duration::from_millis(500));
                glw.window.request_redraw();
            }
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::KeyboardInput { event: key, .. } => {
                let egui_focus = egui.egui_ctx.memory(|m| m.focused().is_some());
                if deck.handle_key(key, self.modifiers, egui_focus) {
                    forward = false;
                }
                glw.window.request_redraw();
            }
            // With the terminal focused the IME belongs to the terminal (not to egui).
            WindowEvent::Ime(ime) if deck.term_focus => {
                deck.handle_ime(ime);
                forward = false;
                glw.window.request_redraw();
            }
            WindowEvent::HoveredFile(_) => {
                deck.drop_hover = true;
                glw.window.request_redraw();
            }
            WindowEvent::HoveredFileCancelled => {
                deck.drop_hover = false;
                glw.window.request_redraw();
            }
            WindowEvent::DroppedFile(path) => {
                deck.drop_hover = false;
                self.dropped.push(path.clone());
                glw.window.request_redraw();
            }
            WindowEvent::Focused(f) => {
                deck.window_focused = *f;
                if *f {
                    deck.refresh_tree();
                    deck.git_refresh_soon();
                }
                glw.window.request_redraw();
            }
            _ => {}
        }
        if forward {
            let resp = egui.on_window_event(&glw.window, &event);
            if resp.repaint {
                glw.window.request_redraw();
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Files dropped in the same operation are attached together in the panel/form under the pointer.
        if !self.dropped.is_empty() {
            let files = std::mem::take(&mut self.dropped);
            let pointer = self.egui.as_ref().and_then(|e| e.egui_ctx.input(|i| i.pointer.latest_pos()));
            if let Some(deck) = self.deck.as_mut() {
                deck.attach_dropped_files(files, pointer);
            }
        }
        let now = Instant::now();
        if let Some(t) = self.size_persist_at {
            if now >= t {
                self.size_persist_at = None;
                if let (Some(w), Some(core)) = (self.window(), self.core.as_ref()) {
                    persist_window(core, w);
                }
            }
        }
        if let Some(s) = self.smoke.as_mut() {
            if !s.armed && now >= s.at {
                s.armed = true;
                self.request_redraw();
            }
        }
        if let Some(t) = self.repaint_at {
            if now >= t {
                self.repaint_at = None;
                self.request_work();
            }
        }
        let hidden = self.hidden();
        if hidden && self.background.due(now) {
            self.run_background(event_loop);
        }
        let background = self.background.deadline(now).filter(|_| hidden);
        let deadlines = [self.repaint_at, self.size_persist_at, self.smoke.as_ref().filter(|s| !s.armed).map(|s| s.at), background];
        match deadlines.into_iter().flatten().min() {
            Some(t) => event_loop.set_control_flow(ControlFlow::WaitUntil(t)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // Freeing egui's GL objects needs a current context, which a closed (parked) window does not have.
        if let (Some(e), Some(_)) = (self.egui.as_mut(), self.gl_window.as_ref()) {
            e.destroy();
        }
        // On Wayland the clipboard (inside egui-winit) and the window hold proxies of winit's connection, which closes
        // when `run_app` returns, before `App` is dropped: dropped later, they crash the exit (SIGSEGV in
        // `wl_proxy_destroy`). They go now, while the connection is alive.
        self.egui = None;
        self.gl_window = None;
        self.parked = None;
    }
}

/// The window runs on Wayland (where it cannot be hidden, positioned nor given an icon).
fn is_wayland(event_loop: &ActiveEventLoop) -> bool {
    #[cfg(target_os = "linux")]
    return winit::platform::wayland::ActiveEventLoopExtWayland::is_wayland(event_loop);
    #[cfg(not(target_os = "linux"))]
    {
        let _ = event_loop;
        false
    }
}

/// Wayland app id and X11 `WM_CLASS`: the `.desktop` entry is `holt.desktop`.
#[cfg(target_os = "linux")]
const APP_ID: &str = "holt";

/// The window icon (taskbar and Alt+Tab): the app's mark, from the same drawing as the `.ico`.
fn window_icon() -> Option<winit::window::Icon> {
    const SIZE: usize = 64;
    let rgba = crate::icons::logo_rgba(SIZE, theme::pal().bg);
    winit::window::Icon::from_rgba(rgba, SIZE as u32, SIZE as u32).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maximized_window_keeps_the_normal_size() {
        assert_eq!(size_to_persist(false, (1280.0, 860.0)), Some((1280.0, 860.0)));
        assert_eq!(size_to_persist(true, (1920.0, 1040.0)), None);
    }

    #[test]
    fn a_window_without_area_or_visibility_gets_no_frames() {
        assert!(!hidden_by(Some(false), Some(true), (1280, 860)));
        assert!(!hidden_by(None, None, (1280, 860)), "no answer from the windowing system (Linux/X11): draw");
        assert!(hidden_by(Some(true), Some(true), (1280, 860)));
        assert!(hidden_by(None, None, (0, 0)), "Windows reports 0×0 for the minimized window");
        assert!(hidden_by(Some(false), None, (1280, 0)));
        assert!(hidden_by(Some(false), Some(false), (1280, 860)));
    }

    #[test]
    fn the_background_runs_when_woken_or_at_its_deadline_and_never_in_a_loop() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut bg = Background::default();
        assert!(!bg.due(t0) && bg.deadline(t0).is_none(), "nothing requested, nothing scheduled: the loop sleeps");

        // Woken for the first time: it runs right away; after that, only one run per BACKGROUND_GAP, however many wakes arrive.
        bg.wake();
        assert!(bg.due(t0 + ms(1)));
        bg.ran(t0 + ms(1), None);
        assert!(!bg.due(t0 + ms(2)) && bg.deadline(t0).is_none(), "it ran and nothing stayed scheduled");
        bg.wake();
        bg.wake();
        assert!(!bg.due(t0 + ms(10)), "wakes in a burst wait for the interval");
        assert_eq!(bg.deadline(t0), Some(t0 + ms(1) + BACKGROUND_GAP));
        assert!(bg.due(t0 + ms(1) + BACKGROUND_GAP));

        // The deadline returned by the work wakes the loop with nobody asking.
        bg.ran(t0 + ms(100), Some(t0 + ms(400)));
        assert!(!bg.due(t0 + ms(399)) && bg.due(t0 + ms(400)));
        assert_eq!(bg.deadline(t0), Some(t0 + ms(400)));

        // A drawn frame does that work and resets the schedule.
        bg.clear();
        assert!(!bg.due(t0 + ms(500)) && bg.deadline(t0).is_none());
    }

    #[test]
    fn minimized_or_tiny_size_is_not_saved() {
        assert_eq!(size_to_persist(false, (0.0, 0.0)), None);
        assert_eq!(size_to_persist(false, (639.0, 900.0)), None);
        assert_eq!(size_to_persist(false, (900.0, 479.0)), None);
        assert_eq!(size_to_persist(false, MIN_SAVED_SIZE), Some(MIN_SAVED_SIZE));
    }
}
