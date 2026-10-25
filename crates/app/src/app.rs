//! Loop do winit (`ApplicationHandler`): janela, cola GL/egui e roteamento de eventos.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use agentdeck_core::{Core, CoreEvent};
use crossbeam_channel::Receiver;
use glow::HasContext;
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
use crate::window::GlWindow;
use crate::{theme, ui};

#[derive(Debug, Clone, Copy)]
pub enum UserEvent {
    /// Algo mudou em segundo plano (evento do core, saída de PTY, tarefa concluída).
    Wake,
    /// Pedido de repintura do egui após `delay`.
    Repaint(Duration),
    /// O usuário usou o ícone da bandeja.
    Tray(TrayCommand),
}

/// Resultado de `Core::start` (feito numa thread: pode levar alguns segundos).
pub type CoreStart = anyhow::Result<(Core, Receiver<CoreEvent>)>;

pub struct App {
    proxy: Arc<Mutex<EventLoopProxy<UserEvent>>>,
    core: Option<Core>,
    core_rx: Receiver<CoreStart>,
    core_frame: u64,
    gl_window: Option<GlWindow>,
    gl: Option<Arc<glow::Context>>,
    egui: Option<egui_glow::EguiGlow>,
    deck: Option<Deck>,
    modifiers: ModifiersState,
    repaint_at: Option<Instant>,
    frames: u64,
    smoke: Option<Smoke>,
    dropped: Vec<PathBuf>,
    size_persist_at: Option<Instant>,
    /// Estado da janela entregue ao egui a cada quadro (`ViewportInfo::maximized` etc.): o `EguiGlow` não o preenche.
    viewport_info: egui::ViewportInfo,
    viewport_first: bool,
    /// A janela mudou (tamanho, posição, foco, escala): relê o `viewport_info` no próximo quadro. Ler a cada quadro
    /// custaria várias chamadas ao sistema de janelas, e o terminal pede muitos quadros.
    viewport_stale: bool,
    /// O trabalho do `Deck` que roda sem quadro, enquanto a janela está escondida ou minimizada.
    background: Background,
    /// O ícone da bandeja; `None` = indisponível (o × passa a minimizar em vez de esconder).
    tray: Option<Tray>,
}

/// Agenda do `Deck::background`: com a janela escondida ou minimizada não há `RedrawRequested` (no Windows, nem
/// `WM_PAINT`), então entregas, saída dos processos e temporizadores rodam a partir dos eventos do laço. Sem laço
/// contínuo: roda quando algo acorda o app (`dirty`) ou no prazo que o próprio trabalho devolveu (`next`).
#[derive(Default)]
struct Background {
    /// Algo mudou desde a última vez (evento do core, saída de PTY, prazo do quadro anterior).
    dirty: bool,
    /// Próximo prazo devolvido por `Deck::background`.
    next: Option<Instant>,
    last: Option<Instant>,
}

/// Menor intervalo entre duas rodadas por `dirty`: um terminal que despeja saída acorda o app a cada pedaço.
const BACKGROUND_GAP: Duration = Duration::from_millis(50);

impl Background {
    fn wake(&mut self) {
        self.dirty = true;
    }

    /// Um quadro desenhado faz o mesmo trabalho: a agenda recomeça quando a janela esconder de novo.
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

    /// Quando o laço precisa acordar para rodar de novo (`None`: só um evento acorda).
    fn deadline(&self, now: Instant) -> Option<Instant> {
        let woken = self.dirty.then(|| self.last.map_or(now, |l| l + BACKGROUND_GAP));
        [woken, self.next].into_iter().flatten().min()
    }
}

/// A janela não recebe quadros: minimizada (0×0 no Windows), com área zero ou escondida. Desenhar assim é pior que inútil:
/// o `fit_tab` de um terminal de 0×0 estraga o CLI (ver `claude-code-sem-esc3j-no-windows.md`).
fn hidden_by(minimized: Option<bool>, visible: Option<bool>, size: (u32, u32)) -> bool {
    minimized == Some(true) || visible == Some(false) || size.0 == 0 || size.1 == 0
}

fn is_hidden(w: &Window) -> bool {
    hidden_by(w.is_minimized(), w.is_visible(), w.inner_size().into())
}

/// Lado (px) do ícone da bandeja.
const TRAY_ICON_SIZE: usize = 32;

/// Menor tamanho gravado/restaurado: abaixo disso é janela minimizada (0x0 no Windows) ou lixo.
const MIN_SAVED_SIZE: (f32, f32) = (640.0, 480.0);

/// Tamanho a gravar na `Config`, ou `None` para manter o que está lá. Maximizada, o tamanho da tela inteira
/// não é "o tamanho da janela": gravá-lo faria a janela normal reabrir (ou restaurar) do tamanho da tela.
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

    /// Função que acorda o loop (usada pelo core e pelas threads do PTY).
    pub fn waker(proxy: &Arc<Mutex<EventLoopProxy<UserEvent>>>) -> Wake {
        let proxy = proxy.clone();
        // Coalesce: vários acordes antes do próximo quadro viram um só evento.
        Arc::new(move || {
            let _ = proxy.lock().send_event(UserEvent::Wake);
        })
    }

    /// Quando o core termina de iniciar: cria o estado da interface.
    fn try_init_core(&mut self, event_loop: &ActiveEventLoop) {
        if self.core.is_some() || self.egui.is_none() {
            return;
        }
        let Ok(res) = self.core_rx.try_recv() else { return };
        let (core, events) = match res {
            Ok(v) => v,
            Err(e) => {
                log::error!("falha ao iniciar o backend: {e:#}");
                // Sem console no Windows: sem a caixa, o app só fecharia sem explicação.
                rfd::MessageDialog::new()
                    .set_level(rfd::MessageLevel::Error)
                    .set_title("AgentDeck")
                    .set_description(format!("{e:#}"))
                    .set_buttons(rfd::MessageButtons::Ok)
                    .show();
                event_loop.exit();
                return;
            }
        };
        core.set_waker(Self::waker(&self.proxy));
        // SOMENTE DEPURAÇÃO: grava porta e token da ponte para testes ponta a ponta com `ask-agent`.
        if let Some(file) = agentdeck_core::env_var_os("DEBUG_BRIDGE") {
            let _ = std::fs::write(file, format!("{}\n{}\n", core.bridge_port(), core.bridge_token()));
        }
        // Tamanho salvo da janela (a janela abre antes de a configuração ser lida).
        let cfg = core.config();
        if let Some(w) = self.window() {
            if size_to_persist(false, (cfg.window_width, cfg.window_height)).is_some() {
                let _ = w.request_inner_size(winit::dpi::LogicalSize::new(cfg.window_width as f64, cfg.window_height as f64));
            }
            // O tamanho normal fica guardado: ao restaurar, a janela volta a ele.
            if cfg.window_maximized {
                w.set_maximized(true);
            }
        }
        // SOMENTE DEPURAÇÃO: capturas e estados fictícios não gravam na configuração (um "empty" ou
        // "sidebar-collapsed" ficaria salvo e apagaria as abas de verdade).
        if self.smoke.is_some() || agentdeck_core::env_var_os("DEBUG_UI").is_some() {
            core.disable_config_writes();
        }
        // Capturas e estados fictícios não deixam ícone na bandeja do usuário.
        if self.smoke.is_none() && agentdeck_core::env_var_os("DEBUG_UI").is_none() && agentdeck_core::env_var_os("SMOKE").is_none() {
            let proxy = self.proxy.clone();
            let on_command: tray::OnCommand = Arc::new(move |cmd| {
                let _ = proxy.lock().send_event(UserEvent::Tray(cmd));
            });
            self.tray = Tray::start(crate::icons::logo_rgba(TRAY_ICON_SIZE, theme::pal().bg), TRAY_ICON_SIZE as u32, on_command);
        }
        let mut deck = Deck::new(core.clone(), events, Self::waker(&self.proxy));
        apply_debug_ui(&mut deck);
        self.deck = Some(deck);
        self.core = Some(core);
        self.core_frame = self.frames + 1;
        self.request_redraw();
    }

    fn window(&self) -> Option<&Window> {
        self.gl_window.as_ref().map(|g| &g.window)
    }

    fn request_redraw(&self) {
        if let Some(w) = self.window() {
            w.request_redraw();
        }
    }

    /// Algo mudou: um quadro (que faz o trabalho do `Deck` e desenha) ou, sem quadros, só o trabalho.
    fn request_work(&mut self) {
        if self.hidden() {
            self.background.wake();
        } else {
            self.request_redraw();
        }
    }

    /// Sem quadros (ver `is_hidden`). Antes do primeiro quadro a janela ainda nasce invisível e precisa ser desenhada.
    fn hidden(&self) -> bool {
        self.frames > 0 && self.window().is_some_and(is_hidden)
    }

    /// O `Deck` trabalha sem quadro e a agenda guarda quando voltar. Com a janela escondida, é também aqui que se vê o
    /// que só o trabalho sem quadro decide: encerrar (gravação concluída) e a segunda abertura pedindo a janela.
    fn run_background(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let next = self.deck.as_mut().and_then(Deck::background);
        self.background.ran(now, next);
        if self.quit_if_requested(event_loop) {
            return;
        }
        if self.deck.as_mut().is_some_and(|d| std::mem::take(&mut d.activate_requested)) {
            self.show_window();
        }
    }

    /// O app foi mandado encerrar (o "Encerrar" da bandeja, ou o "Sair" depois da pergunta): encerra tudo.
    fn quit_if_requested(&mut self, event_loop: &ActiveEventLoop) -> bool {
        if !self.deck.as_ref().is_some_and(|d| d.quit_requested) {
            return false;
        }
        self.shutdown();
        event_loop.exit();
        true
    }

    /// Traz a janela de volta: mostra, restaura se minimizada e foca. Quem a esconde não é avisado pelo winit, e quem a
    /// mostra de novo precisa de um quadro (o `request_redraw`).
    fn show_window(&mut self) {
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

    /// O × da janela (e o da barra de título): esconde na bandeja, sem perguntar; sem bandeja, minimiza. Os CLIs, a
    /// ponte e as entregas seguem rodando pelo `Deck::background`.
    fn close_window(&mut self) {
        let Some(w) = self.gl_window.as_ref().map(|g| &g.window) else { return };
        if let Some(core) = self.core.as_ref() {
            persist_window(core, w);
        }
        match tray::close_action(self.tray.is_some()) {
            CloseAction::Hide => {
                w.set_visible(false);
                // O winit não avisa quando a janela é escondida por código: o trabalho sem quadro começa daqui.
                self.background.wake();
            }
            CloseAction::Minimize => w.set_minimized(true),
        }
    }

    /// O usuário usou o ícone da bandeja.
    fn on_tray(&mut self, event_loop: &ActiveEventLoop, cmd: TrayCommand) {
        match cmd {
            TrayCommand::Open => self.show_window(),
            TrayCommand::Quit => {
                let Some(deck) = self.deck.as_mut() else {
                    // O backend ainda nem iniciou: não há CLI a perder.
                    event_loop.exit();
                    return;
                };
                deck.request_quit();
                // Para perguntar, a janela precisa aparecer primeiro.
                if deck.docs.confirm.is_some() {
                    self.show_window();
                }
                self.quit_if_requested(event_loop);
            }
        }
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        // Um pedido de quadro que chega com a janela escondida (a minimizada informa 0×0) vira trabalho sem quadro.
        if self.hidden() {
            self.run_background(event_loop);
            return;
        }
        self.background.clear();
        let (Some(glw), Some(gl), Some(egui)) = (self.gl_window.as_ref(), self.gl.as_ref(), self.egui.as_mut()) else { return };
        let mut deck = self.deck.as_mut();
        // Sem isto `viewport().maximized` é sempre `None` (o `EguiGlow` não chama `update_viewport_info`).
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
            // Enquanto o backend inicia: só o fundo.
            None => {
                ui.painter().rect_filled(ui.max_rect(), 0.0, theme::pal().bg);
            }
        });
        // SAFETY: contexto GL corrente nesta thread.
        unsafe {
            let c = theme::pal().bg;
            gl.clear_color(c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
        }
        egui.paint(&glw.window);

        // A segunda abertura do app pediu a janela (já visível: só foca).
        if self.deck.as_mut().is_some_and(|d| std::mem::take(&mut d.activate_requested)) {
            Self::show(&glw.window);
        }
        // Encerrar (depois da pergunta, quando havia o que perguntar).
        if self.deck.as_ref().is_some_and(|d| d.quit_requested) {
            self.shutdown();
            event_loop.exit();
            return;
        }
        // O "×" da barra de título: o mesmo caminho do fechar do sistema.
        if self.deck.as_mut().is_some_and(|d| std::mem::take(&mut d.close_requested)) {
            self.close_window();
            return;
        }

        if self.smoke.as_ref().is_some_and(|s| s.armed) {
            let size = glw.window.inner_size();
            let pixels = smoke::read_framebuffer(gl, size.width, size.height);
            let smoke = self.smoke.take().expect("smoke armado");
            let path = smoke.dir.join("screen.png");
            match smoke::save_png(&path, size.width, size.height, &pixels) {
                Ok(()) => log::info!("captura salva em {}", path.display()),
                Err(e) => log::error!("falha ao salvar {}: {e}", path.display()),
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
        // Captura (AGENTDECK_SMOKE) não baixa o whisper nem pergunta: cada pasta temporária começaria 1,6 GB.
        if self.frames == self.core_frame && agentdeck_core::env_var_os("SMOKE").is_none() {
            // Depois do primeiro quadro: quem já usa o ditado baixa o que faltar em segundo plano; quem nunca foi
            // perguntado vê a pergunta (o Whisper é opcional e só baixa com o "sim").
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

/// Grava o estado da janela: maximizada ou não, e o tamanho normal (só fora do maximizado).
fn persist_window(core: &Core, w: &Window) {
    // Minimizada, o winit no Windows perde o "maximizada": gravar agora faria a janela reabrir normal.
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
        let (w, h) = (1280.0f64, 860.0f64);
        let attrs = WindowAttributes::default()
            .with_title("AgentDeck")
            // A barra de título é desenhada pela interface (`ui/titlebar.rs`).
            .with_decorations(false)
            .with_inner_size(winit::dpi::LogicalSize::new(w, h))
            // A barra direita (≥ 260px) e o terminal (≥ 360px) precisam caber ao lado da esquerda.
            .with_min_inner_size(winit::dpi::LogicalSize::new(820.0, 480.0))
            .with_window_icon(window_icon())
            .with_visible(false);
        // Sem decoração, o Windows tira também a sombra da janela; ela volta assim.
        #[cfg(windows)]
        let attrs = winit::platform::windows::WindowAttributesExtWindows::with_undecorated_shadow(attrs, true);
        let (glw, gl) = match GlWindow::new(event_loop, attrs) {
            Ok(v) => v,
            Err(e) => {
                log::error!("não foi possível criar a janela OpenGL: {e}");
                event_loop.exit();
                return;
            }
        };
        let egui = egui_glow::EguiGlow::new(event_loop, gl.clone(), None, None, true);
        let fonts = theme::install_fonts(&egui.egui_ctx);
        log::info!("fontes: ui={:?} mono={:?}", fonts.ui, fonts.mono);
        theme::install_style(&egui.egui_ctx);
        let proxy = self.proxy.clone();
        egui.egui_ctx.set_request_repaint_callback(move |info| {
            let _ = proxy.lock().send_event(UserEvent::Repaint(info.delay));
        });
        self.egui = Some(egui);
        self.gl = Some(gl);
        self.gl_window = Some(glw);
        // No Windows, uma janela invisível não recebe WM_PAINT: `request_redraw` nunca vira
        // `RedrawRequested` e o `set_visible(true)` do primeiro quadro não roda. Desenha já.
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
        // Maximizar, restaurar e minimizar chegam como `Resized`.
        if matches!(
            event,
            WindowEvent::Resized(_)
                | WindowEvent::Moved(_)
                | WindowEvent::Focused(_)
                | WindowEvent::ScaleFactorChanged { .. }
                | WindowEvent::Occluded(_)
        ) {
            self.viewport_stale = true;
            // A janela acabou de sumir (minimizar chega como `Resized` 0×0): assume o que o quadro fazia.
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
        // O × (e Alt+F4, e fechar pela barra de tarefas) não encerra: esconde na bandeja. Encerrar é o menu dela.
        if matches!(event, WindowEvent::CloseRequested) {
            self.close_window();
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
            // Com o terminal focado o IME é do terminal (não do egui).
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
        // Arquivos soltos numa mesma operação são anexados juntos no painel/formulário sob o ponteiro.
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
        if let Some(e) = self.egui.as_mut() {
            e.destroy();
        }
    }
}

/// SOMENTE DEPURAÇÃO: projetos fictícios na barra lateral (`AGENTDECK_DEBUG_UI=projects-demo`): um ativo com
/// três agentes (um ocupado), outro com um, outro vazio.
fn debug_projects() -> Vec<crate::deck::ProjectView> {
    use crate::deck::{AgentBadge, ProjectView};
    let agent = |id: &str, parent: Option<&str>, kind: &str, label: &str, color, busy| AgentBadge {
        id: id.into(),
        parent: parent.map(Into::into),
        kind: kind.into(),
        label: label.into(),
        model: match id {
            "claude@dev" => Some("sonnet".into()),
            "claude@review" => Some("opus".into()),
            "antigravity@sidebar" => Some("claude-sonnet-5-5-medium".into()),
            _ => None,
        },
        color,
        busy,
        done: false,
        alive: true,
        tab: None,
    };
    let project = |id: &str, name: &str, active, agents| ProjectView {
        id: id.into(),
        name: name.into(),
        path: format!(r"C:\Users\voce\{name}").into(),
        active,
        expanded: true,
        agents,
    };
    vec![
        project(
            "p1",
            "agentdeck-dev",
            true,
            vec![
                agent("claude", None, "claude", "Claude", theme::CLAUDE, true),
                agent("claude@dev", Some("claude"), "claude", "Claude · Dev", theme::CLAUDE, false),
                agent("codex", Some("claude@dev"), "codex", "Codex", theme::SHELL, false),
                agent("antigravity@sidebar", Some("claude"), "antigravity", "Antigravity · Sidebar", theme::ANTIGRAVITY, false),
                agent("claude@review", None, "claude", "Claude · Review", theme::CLAUDE, false),
            ],
        ),
        project("p2", "jwic", false, vec![agent("claude", None, "claude", "Claude", theme::CLAUDE, false)]),
        project("p3", "site-novo", false, vec![]),
    ]
}

/// SOMENTE DEPURAÇÃO: uso fictício na barra de título (`AGENTDECK_DEBUG_UI=usage-demo` e `usage-panel`): Claude
/// no meio da sessão, Antigravity com a semana em aviso (e os três grupos de modelos), Codex com a sessão esgotada.
fn debug_usage() -> std::collections::HashMap<String, agentdeck_core::AgentUsage> {
    use agentdeck_core::{AgentUsage, UsageGroup, UsageWindow};
    let now = agentdeck_core::util::now_ms();
    let w = |used, hours: u64| Some(UsageWindow { used, resets_at_ms: Some(now + hours * 3_600_000) });
    std::collections::HashMap::from([
        ("claude".into(), AgentUsage { session: w(0.34, 2), weekly: w(0.12, 100), updated_ms: now - 60_000, ..Default::default() }),
        (
            "antigravity".into(),
            AgentUsage {
                session: w(0.45, 3),
                weekly: w(0.86, 30),
                updated_ms: now - 120_000,
                local: false,
                groups: vec![
                    UsageGroup { name: "Gemini Models".into(), session: w(0.45, 3), weekly: w(0.2, 60) },
                    UsageGroup { name: "Gemini 4 Argon".into(), session: None, weekly: w(0.86, 30) },
                    UsageGroup { name: "Claude and GPT models".into(), session: w(0.0, 5), weekly: w(0.1, 150) },
                ],
            },
        ),
        (
            "codex".into(),
            AgentUsage {
                session: Some(UsageWindow { used: 1.0, resets_at_ms: Some(now + 40 * 60_000) }),
                weekly: w(0.4, 80),
                updated_ms: now - 5 * 3_600_000,
                ..Default::default()
            },
        ),
    ])
}

/// SOMENTE DEPURAÇÃO: `AGENTDECK_DEBUG_UI=conversa,tasks-demo,tasks-empty,tasks-detail,tasks-messages,tasks-messages-open,tasks-running,tasks-failed,tasks-confirm,tasks-discard,tasks-return,tasks-error,tasks-refused-main,tasks-refused-main-files,tasks-refused-commit,tasks-refused-conflict,tasks-stash-kept,tasks-busy,tasks-new,tasks-new-attach,tasks-new-effort,tasks-new-opencode,tasks-new-agent,tasks-new-model,tasks-new-provider,tasks-new-provider-models,settings,newtab,newtab-models:<agente>,drop,drop-pane:<n>,drop-task,usage-demo,usage-panel,usage-panel:<agente>,projects-demo,project:<id>,git,git-demo,git-diff,git-hunks,git-branches,git-branches-merge,git-merging,git-rebase,git-conflicts,git-conflict-file,git-history,git-commit,git-stash,git-stash-diff,git-norepo,git-nogit,split-drag,docs-demo,docs-demo-file,docs-demo-md,docs-demo-report,docs-demo-unreadable,docs-demo-many,docs-demo-badge,docs-demo-edit,docs-demo-find,docs-demo-diff-find,docs-demo-external,docs-demo-confirm,docs-demo-large,docs-demo-md-code,download,download-retry,download-error,mic-recording,mic-transcribing,mic-opening,dictation-ask` abre menus/estados
/// para capturas de tela no modo smoke.
fn apply_debug_ui(deck: &mut Deck) {
    let Some(v) = agentdeck_core::env_var("DEBUG_UI") else { return };
    for item in v.split(',').map(str::trim) {
        match item {
            "conversa" => deck.set_right_view(crate::deck::RightView::Conversa),
            "settings" => deck.settings_open = true,
            "dictation-ask" => deck.dictation_prompt = true,
            "shortcuts" => deck.toggle_shortcuts = true,
            "newtab" => deck.new_tab_menu_open = true,
            // Segundo passo do menu de novo agente (escolher o modelo): `newtab-models:claude`, `newtab-models:opencode`…
            // e, no OpenCode, `newtab-models:opencode@opencode` abre direto os modelos do provedor.
            k if k.starts_with("newtab-models:") => {
                let rest = &k["newtab-models:".len()..];
                let (kind, provider) = match rest.split_once('@') {
                    Some((kind, provider)) => (kind, Some(provider.to_owned())),
                    None => (rest, None),
                };
                deck.new_tab_menu_open = true;
                deck.new_tab_models_for = Some(kind.to_owned());
                deck.new_tab_provider = provider;
            }
            "drop" => deck.drop_hover = true,
            k if k.starts_with("drop-pane:") => {
                deck.drop_hover = true;
                deck.debug_drop_slot = k["drop-pane:".len()..].parse().ok();
            }
            "drop-task" => {
                debug_tasks(deck, "tasks-new-attach");
                deck.drop_hover = true;
                deck.debug_drop_task_form = true;
            }
            "git" => deck.set_right_view(crate::deck::RightView::Git),
            "git-demo" | "git-diff" | "git-branches" => {
                deck.set_right_view(crate::deck::RightView::Git);
                deck.git.debug_hold = true;
                deck.git.view = crate::git_ui::GitView::Ready(crate::git_ui::demo_status());
                deck.git.message = "Aba Git na barra direita".into();
                if item == "git-diff" {
                    let entry = crate::git_ui::demo_status().entries[2].clone();
                    deck.git.diff = Some(crate::git_ui::OpenDiff { entry, staged: false, diff: Some(Ok(crate::git_ui::demo_diff())) });
                }
                if item == "git-branches" {
                    deck.git.branch_menu = true;
                    deck.git.branches = crate::git_ui::demo_branches();
                }
            }
            "git-merging" | "git-rebase" | "git-conflicts" | "git-conflict-file" | "git-history" | "git-commit" | "git-stash"
            | "git-stash-diff" | "git-branches-merge" | "git-hunks" => {
                use crate::git_ui::{self as g, GitSubView};
                deck.set_right_view(crate::deck::RightView::Git);
                deck.git.debug_hold = true;
                let mut st = g::demo_conflicts_status();
                if item == "git-rebase" {
                    st.state = agentdeck_core::git::RepoState::Rebasing;
                    st.entries.retain(|e| e.kind != agentdeck_core::git::EntryKind::Unmerged);
                }
                if matches!(item, "git-history" | "git-commit" | "git-stash" | "git-stash-diff" | "git-hunks" | "git-branches-merge") {
                    st = g::demo_status();
                }
                deck.git.view = g::GitView::Ready(st);
                deck.git.message.clear();
                match item {
                    "git-merging" => deck.git.subview = GitSubView::Changes,
                    "git-rebase" => deck.git.subview = GitSubView::Changes,
                    "git-conflicts" | "git-conflict-file" => {
                        deck.git.subview = GitSubView::Conflicts;
                        if item == "git-conflict-file" {
                            deck.git.conflict_open =
                                Some(g::OpenConflict { path: "crates/core/src/git.rs".into(), file: Some(Ok(g::demo_conflict_file())) });
                        }
                    }
                    "git-history" | "git-commit" => {
                        deck.git.subview = GitSubView::History;
                        deck.git.log = g::demo_log();
                        deck.git.log_has_more = true;
                        if item == "git-commit" {
                            let c = g::demo_log().remove(0);
                            deck.git.commit_detail = Some(g::OpenCommit {
                                hash: c.hash,
                                short_hash: c.short_hash,
                                subject: c.subject,
                                detail: Some(Ok(g::demo_commit_detail())),
                            });
                        }
                    }
                    "git-stash" | "git-stash-diff" => {
                        deck.git.subview = GitSubView::Stash;
                        deck.git.stashes = g::demo_stashes();
                        if item == "git-stash-diff" {
                            let stash = g::demo_stashes().remove(0);
                            deck.git.stash_diff = Some(g::OpenStashDiff { stash, diff: Some(Ok(g::demo_diff())) });
                        }
                    }
                    "git-hunks" => {
                        let entry = g::demo_status().entries[2].clone();
                        deck.git.diff = Some(g::OpenDiff { entry, staged: false, diff: Some(Ok(g::demo_hunks_diff())) });
                    }
                    "git-branches-merge" => {
                        deck.git.branch_menu = true;
                        deck.git.branch_menu_mode = g::BranchMenuMode::Merge;
                        deck.git.branches = g::demo_branches();
                    }
                    _ => {}
                }
            }
            "git-norepo" | "git-nogit" => {
                deck.set_right_view(crate::deck::RightView::Git);
                deck.git.debug_hold = true;
                deck.git.view = if item == "git-norepo" { crate::git_ui::GitView::NotRepo } else { crate::git_ui::GitView::NoGit };
            }
            "github" => deck.set_right_view(crate::deck::RightView::GitHub),
            "github-demo" | "github-issues" | "github-actions" | "github-menu" => {
                deck.github.debug_hold = true;
                deck.set_right_view(crate::deck::RightView::GitHub);
                let ov = crate::github_ui::demo_overview();
                deck.github.repo = Some(ov.repo.clone());
                deck.github.view = crate::github_ui::GithubView::Ready(ov);
                let detail = crate::github_ui::demo_pr_detail();
                let log = crate::github_ui::demo_job_log();
                deck.github.run_jobs.insert(9001, detail.jobs.clone());
                deck.github.pr_details.insert(42, detail);
                deck.github.job_logs.insert(501, Ok(log));
                match item {
                    "github-issues" => {
                        deck.github.tab = crate::github_ui::GithubTab::Issues;
                        deck.github.selected_issue = Some(38);
                    }
                    "github-actions" => {
                        deck.github.tab = crate::github_ui::GithubTab::Actions;
                        deck.github.selected_run = Some(9001);
                        deck.github.open_log = Some((501, "clippy (windows-latest)".into(), None));
                    }
                    _ => {
                        deck.github.tab = crate::github_ui::GithubTab::Prs;
                        deck.github.selected_pr = Some(42);
                        deck.github.open_log = Some((501, "clippy (windows-latest)".into(), Some(42)));
                        if item == "github-menu" {
                            let anchor = egui::Rect::from_min_size(egui::pos2(900.0, 330.0), egui::vec2(140.0, 22.0));
                            deck.open_github_agent_menu(anchor, "PR #42".into(), "review", "Revise o PR #42".into());
                        }
                    }
                }
            }
            "github-norepo" => {
                deck.github.debug_hold = true;
                deck.set_right_view(crate::deck::RightView::GitHub);
                deck.github.view = crate::github_ui::GithubView::Error(agentdeck_core::github::GithubError::NoRemote);
            }
            "github-error" => {
                deck.github.debug_hold = true;
                deck.set_right_view(crate::deck::RightView::GitHub);
                deck.github.view = crate::github_ui::GithubView::Error(agentdeck_core::github::GithubError::NotFound {
                    repo: "pattr/agentdeck".into(),
                    authenticated: false,
                });
            }
            "tasks"
            | "tasks-demo"
            | "tasks-empty"
            | "tasks-detail"
            | "tasks-messages"
            | "tasks-messages-open"
            | "tasks-running"
            | "tasks-failed"
            | "tasks-confirm"
            | "tasks-discard"
            | "tasks-validating"
            | "tasks-reject"
            | "tasks-discard-validating"
            | "tasks-return"
            | "tasks-error"
            | "tasks-refused-main"
            | "tasks-refused-main-files"
            | "tasks-refused-commit"
            | "tasks-refused-conflict"
            | "tasks-stash-kept"
            | "tasks-busy"
            | "tasks-new"
            | "tasks-new-attach"
            | "tasks-new-effort"
            | "tasks-new-opencode"
            | "tasks-new-agent"
            | "tasks-new-model"
            | "tasks-new-provider"
            | "tasks-new-provider-models" => debug_tasks(deck, item),
            "docs-demo"
            | "docs-demo-file"
            | "docs-demo-md"
            | "docs-demo-report"
            | "docs-demo-unreadable"
            | "docs-demo-many"
            | "docs-demo-badge"
            | "docs-demo-edit"
            | "docs-demo-find"
            | "docs-demo-diff-find"
            | "docs-demo-external"
            | "docs-demo-confirm"
            | "docs-demo-large"
            | "docs-demo-md-code" => crate::docs_demo::apply(deck, item),
            "usage-demo" => deck.debug_usage = Some(debug_usage()),
            _ if item == "usage-panel" || item.starts_with("usage-panel:") => {
                deck.debug_usage = Some(debug_usage());
                deck.debug_usage_panel = Some(item.strip_prefix("usage-panel:").unwrap_or_default().to_owned());
            }
            "projects-demo" => deck.debug_projects = Some(debug_projects()),
            "split-drag" => {
                deck.pane_drag = Some(0);
                deck.debug_pane_drop = Some(1);
            }
            "sidebar-collapsed" => deck.sidebar_collapsed = true,
            "attach" => debug_tasks(deck, "tasks-new-attach"),
            "empty" => deck.debug_close_all = true,
            "download" | "download-retry" | "download-error" => {
                deck.download = Some((debug_download(item), None));
                deck.debug_hold_download = true;
            }
            "mic-recording" => {
                deck.mic_phase = crate::dictation::MicPhase::Recording(crate::dictation::MicTarget::Tab(1));
            }
            "mic-transcribing" => {
                deck.mic_phase = crate::dictation::MicPhase::Transcribing(crate::dictation::MicTarget::Tab(1));
            }
            "mic-opening" => {
                deck.mic_phase = crate::dictation::MicPhase::Opening(crate::dictation::MicTarget::Tab(1));
            }
            other => {
                if let Some(id) = other.strip_prefix("project:") {
                    deck.debug_switch_project = Some(id.to_owned());
                } else if let Some(dir) = other.strip_prefix("expand:") {
                    deck.tree.expanded.insert(dir.to_owned());
                } else if let Some(f) = other.strip_prefix("select:") {
                    deck.tree.active_file = Some(f.to_owned());
                }
            }
        }
    }
}

/// SOMENTE DEPURAÇÃO: estados fictícios da aba Tarefas (`AGENTDECK_DEBUG_UI=tasks-*`), sem git nem core.
fn debug_tasks(deck: &mut Deck, item: &str) {
    use crate::tasks_ui::{self as t, Confirm, FormMenu, TasksView};
    deck.set_right_view(crate::deck::RightView::Tarefas);
    deck.tasks.debug_hold = true;
    deck.tasks.loaded = true;
    deck.tasks.tasks = if item == "tasks-empty" { Vec::new() } else { t::demo_tasks() };
    // A lista de capturas mostra todas as tarefas: os grupos recolhidos ficam abertos.
    deck.tasks.collapsed.clear();
    let detail = |id: &str| TasksView::Detail(id.to_owned());
    match item {
        "tasks-validating" | "tasks-reject" | "tasks-discard-validating" => {
            deck.tasks.view = detail("t8");
            match item {
                "tasks-reject" => {
                    deck.tasks.return_open = true;
                    deck.tasks.feedback = "Ao abrir uma tarefa mesclada, o cabeçalho continua mostrando \"em revisão\".".into();
                }
                "tasks-discard-validating" => deck.tasks.confirm = Some(Confirm::Discard("t8".into())),
                _ => {}
            }
        }
        "tasks-messages" | "tasks-messages-open" => {
            use crate::tasks_ui::Section;
            deck.tasks.view = detail("t4");
            // Só a linha do tempo à vista: as outras seções ficam recolhidas.
            deck.tasks.closed_sections.extend([Section::Brief, Section::Report, Section::Changes, Section::History]);
            if item == "tasks-messages-open" {
                if let Some(m) = deck.tasks.task("t4").and_then(|t| t.messages.get(1).map(|m| m.at_ms)) {
                    deck.tasks.open_messages.insert(("t4".into(), m));
                }
            }
        }
        "tasks-detail"
        | "tasks-confirm"
        | "tasks-return"
        | "tasks-error"
        | "tasks-refused-main"
        | "tasks-refused-main-files"
        | "tasks-refused-commit"
        | "tasks-refused-conflict"
        | "tasks-stash-kept"
        | "tasks-busy" => {
            deck.tasks.view = detail("t4");
            match item {
                "tasks-confirm" => deck.tasks.confirm = Some(Confirm::Merge("t4".into())),
                "tasks-return" => {
                    deck.tasks.return_open = true;
                    deck.tasks.feedback = "Falta tratar o caso de nenhuma tarefa no projeto e um teste para o resumo do diff.".into();
                }
                "tasks-error" => {
                    deck.tasks.notice = Some(
                        "o repositório principal tem alterações sem commit (crates/app/src/deck.rs, crates/app/src/app.rs): faça o commit ou guarde-as e tente de novo"
                            .into(),
                    )
                }
                "tasks-refused-main" | "tasks-refused-main-files" => {
                    let files = ["crates/app/src/deck.rs", "crates/app/src/app.rs", "memory/pendencias.md"].map(String::from).to_vec();
                    deck.tasks.notice = Some("o repositório principal tem alterações sem commit".into());
                    deck.tasks.notice_block = Some(agentdeck_core::MergeBlock::MainDirty(files));
                    deck.tasks.notice_files = item == "tasks-refused-main-files";
                }
                "tasks-refused-commit" => {
                    let files = ["crates/core/src/tasks.rs", "memory/testes.md"].map(String::from).to_vec();
                    deck.tasks.notice = Some("a worktree da tarefa tem trabalho sem commit".into());
                    deck.tasks.notice_block = Some(agentdeck_core::MergeBlock::TaskDirty(files));
                }
                "tasks-refused-conflict" => {
                    deck.tasks.notice = Some("conflito ao mesclar".into());
                    deck.tasks.notice_block = Some(agentdeck_core::MergeBlock::Conflict(vec!["crates/app/src/app.rs".into()]));
                }
                "tasks-stash-kept" => {
                    deck.tasks.notice_info = true;
                    deck.tasks.notice = Some("Suas alterações sem commit continuam guardadas no stash do git (\"agentdeck: guardado antes de mesclar a tarefa t4\"): ao devolvê-las elas se chocariam com o que a tarefa trouxe em crates/app/src/app.rs. Nada se perdeu. Para recuperá-las: abra um terminal na pasta do projeto, rode git stash list para achar a entrada e git stash pop para devolvê-la (resolvendo o que chocar).".into());
                }
                "tasks-busy" => deck.tasks.busy = Some(t::Busy { id: "t4".into(), label: "Mesclando…" }),
                _ => {}
            }
        }
        "tasks-discard" => {
            deck.tasks.view = detail("t6");
            deck.tasks.confirm = Some(Confirm::Discard("t6".into()));
        }
        "tasks-running" => deck.tasks.view = detail("t6"),
        "tasks-failed" => deck.tasks.view = detail("t3"),
        "tasks-new"
        | "tasks-new-attach"
        | "tasks-new-effort"
        | "tasks-new-opencode"
        | "tasks-new-agent"
        | "tasks-new-model"
        | "tasks-new-provider"
        | "tasks-new-provider-models" => {
            deck.tasks_open_new();
            let f = &mut deck.tasks.form;
            f.kind =
                if item.starts_with("tasks-new-provider") || item == "tasks-new-opencode" { "opencode".into() } else { "claude".into() };
            if item == "tasks-new-effort" {
                f.model = Some("sonnet".into());
                f.effort = Some(agentdeck_core::models::Effort::High);
            }
            f.base = "dev".into();
            f.title = "Documentar as rotas /tasks da ponte".into();
            f.brief = "Explicar no memory/ARCHITECTURE.md cada rota, com o corpo de exemplo e os códigos de erro.\n\nConferir também o texto do ask-agent --task.".into();
            if item == "tasks-new-attach" {
                f.attachments = vec![
                    r"C:\Users\voce\agentdeck-dev\dados\anexos\1a2b3c-captura de tela.png".into(),
                    r"C:\Users\voce\agentdeck-dev\dados\anexos\9f8e-relatorio.pdf".into(),
                ];
            }
            f.focus_title = false;
            f.menu = match item {
                "tasks-new-agent" => Some(FormMenu::Agent),
                "tasks-new-model" | "tasks-new-provider" | "tasks-new-provider-models" => Some(FormMenu::Model),
                _ => None,
            };
            if item == "tasks-new-provider-models" {
                f.provider = Some("opencode".into());
            }
            if item.starts_with("tasks-new-provider") {
                deck.request_models("opencode");
            }
        }
        _ => {}
    }
}

/// Progresso fictício para capturas de tela da barra de download (`AGENTDECK_DEBUG_UI`).
fn debug_download(kind: &str) -> agentdeck_core::DownloadProgress {
    use agentdeck_core::{DownloadProgress, DownloadState};
    let mut p = DownloadProgress {
        label: "Modelo large-v3-turbo".into(),
        state: DownloadState::Downloading,
        received: 680_000_000,
        total: Some(1_624_555_275),
        speed: 12_900_000.0,
        attempt: 0,
        max_attempts: 0,
        retry_in_ms: 0,
        message: None,
    };
    match kind {
        "download-retry" => (p.state, p.attempt, p.max_attempts, p.retry_in_ms, p.speed) = (DownloadState::Retrying, 2, 5, 4200, 0.0),
        "download-error" => {
            (p.state, p.message) = (DownloadState::Error, Some("SHA-256 não confere (esperado 1fc70f77…, obtido 9a3b02c1…)".into()))
        }
        _ => {}
    }
    p
}

/// O ícone da janela (barra de tarefas e Alt+Tab): a marca do app, a partir do mesmo desenho do `.ico`.
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
        assert!(!hidden_by(None, None, (1280, 860)), "sem resposta do sistema de janelas (Linux/X11): desenha");
        assert!(hidden_by(Some(true), Some(true), (1280, 860)));
        assert!(hidden_by(None, None, (0, 0)), "o Windows informa 0×0 para a minimizada");
        assert!(hidden_by(Some(false), None, (1280, 0)));
        assert!(hidden_by(Some(false), Some(false), (1280, 860)));
    }

    #[test]
    fn the_background_runs_when_woken_or_at_its_deadline_and_never_in_a_loop() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut bg = Background::default();
        assert!(!bg.due(t0) && bg.deadline(t0).is_none(), "nada pedido, nada agendado: o laço dorme");

        // Acordado pela primeira vez: roda já; depois só uma rodada a cada BACKGROUND_GAP, por mais acordes que cheguem.
        bg.wake();
        assert!(bg.due(t0 + ms(1)));
        bg.ran(t0 + ms(1), None);
        assert!(!bg.due(t0 + ms(2)) && bg.deadline(t0).is_none(), "rodou e nada ficou agendado");
        bg.wake();
        bg.wake();
        assert!(!bg.due(t0 + ms(10)), "acordes em rajada esperam o intervalo");
        assert_eq!(bg.deadline(t0), Some(t0 + ms(1) + BACKGROUND_GAP));
        assert!(bg.due(t0 + ms(1) + BACKGROUND_GAP));

        // O prazo devolvido pelo trabalho acorda o laço sem ninguém pedir.
        bg.ran(t0 + ms(100), Some(t0 + ms(400)));
        assert!(!bg.due(t0 + ms(399)) && bg.due(t0 + ms(400)));
        assert_eq!(bg.deadline(t0), Some(t0 + ms(400)));

        // Um quadro desenhado faz esse trabalho e zera a agenda.
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
