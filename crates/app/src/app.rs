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
        crate::debug_ui::apply(&mut deck);
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
