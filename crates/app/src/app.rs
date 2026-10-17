//! Loop do winit (`ApplicationHandler`): janela, cola GL/egui e roteamento de eventos.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use commanddeck_core::{Core, CoreEvent};
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
use crate::window::GlWindow;
use crate::{theme, ui};

#[derive(Debug, Clone, Copy)]
pub enum UserEvent {
    /// Algo mudou em segundo plano (evento do core, saída de PTY, tarefa concluída).
    Wake,
    /// Pedido de repintura do egui após `delay`.
    Repaint(Duration),
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
                    .set_title("CommandDeck")
                    .set_description(format!("{e:#}"))
                    .set_buttons(rfd::MessageButtons::Ok)
                    .show();
                event_loop.exit();
                return;
            }
        };
        core.set_waker(Self::waker(&self.proxy));
        // SOMENTE DEPURAÇÃO: grava porta e token da ponte para testes ponta a ponta com `ask-agent`.
        if let Some(file) = std::env::var_os("COMMANDDECK_DEBUG_BRIDGE") {
            let _ = std::fs::write(file, format!("{}\n{}\n", core.bridge_port(), core.bridge_token()));
        }
        // Tamanho salvo da janela (a janela abre antes de a configuração ser lida).
        let cfg = core.config();
        if let Some(w) = self.window() {
            if cfg.window_width >= 640.0 && cfg.window_height >= 480.0 {
                let _ = w.request_inner_size(winit::dpi::LogicalSize::new(cfg.window_width as f64, cfg.window_height as f64));
            }
        }
        // SOMENTE DEPURAÇÃO: capturas e estados fictícios não gravam na configuração (um "empty" ou
        // "sidebar-collapsed" ficaria salvo e apagaria as abas de verdade).
        if self.smoke.is_some() || std::env::var_os("COMMANDDECK_DEBUG_UI").is_some() {
            core.disable_config_writes();
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

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let (Some(glw), Some(gl), Some(egui)) = (self.gl_window.as_ref(), self.gl.as_ref(), self.egui.as_mut()) else { return };
        let mut deck = self.deck.as_mut();
        egui.run(&glw.window, |ui| match deck.as_deref_mut() {
            Some(deck) => {
                deck.before_frame(ui.ctx());
                ui::draw(deck, ui);
            }
            // Enquanto o backend inicia: só o fundo.
            None => {
                ui.painter().rect_filled(ui.max_rect(), 0.0, theme::BG);
            }
        });
        // SAFETY: contexto GL corrente nesta thread.
        unsafe {
            let c = theme::BG;
            gl.clear_color(c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
        }
        egui.paint(&glw.window);

        // O "×" da barra de título: o mesmo caminho do fechar do sistema.
        if self.deck.as_ref().is_some_and(|d| d.quit_requested) {
            self.shutdown();
            event_loop.exit();
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
        if self.frames == self.core_frame {
            // Modelos/binários do whisper em segundo plano, depois do primeiro quadro.
            if let Some(core) = &self.core {
                core.ensure_whisper_async();
            }
        }
    }

    fn shutdown(&mut self) {
        if let Some(deck) = self.deck.as_mut() {
            deck.shutdown();
        }
        let Some(core) = self.core.as_ref() else { return };
        if let Some(w) = self.window() {
            let size = w.inner_size().to_logical::<f32>(w.scale_factor());
            core.update_config(|c| {
                c.window_width = size.width;
                c.window_height = size.height;
            });
        }
        core.shutdown();
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gl_window.is_some() {
            return;
        }
        let (w, h) = (1280.0f64, 860.0f64);
        let attrs = WindowAttributes::default()
            .with_title("CommandDeck")
            // A barra de título é desenhada pela interface (`ui/titlebar.rs`).
            .with_decorations(false)
            .with_inner_size(winit::dpi::LogicalSize::new(w, h))
            // A barra direita (≥ 260px) e o terminal (≥ 360px) precisam caber ao lado da esquerda.
            .with_min_inner_size(winit::dpi::LogicalSize::new(820.0, 480.0))
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
                self.request_redraw();
            }
            UserEvent::Repaint(delay) => {
                if delay.is_zero() {
                    self.request_redraw();
                } else if let Some(t) = Instant::now().checked_add(delay) {
                    self.repaint_at = Some(self.repaint_at.map_or(t, |r| r.min(t)));
                }
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
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
        let (Some(glw), Some(egui), Some(deck)) = (self.gl_window.as_ref(), self.egui.as_mut(), self.deck.as_mut()) else { return };
        let mut forward = true;
        match &event {
            WindowEvent::CloseRequested => {
                self.shutdown();
                event_loop.exit();
                return;
            }
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
        // Arquivos soltos numa mesma operação são anexados juntos.
        if !self.dropped.is_empty() {
            let files = std::mem::take(&mut self.dropped);
            if let Some(deck) = self.deck.as_mut() {
                deck.attach_paths(files);
            }
        }
        let now = Instant::now();
        if let Some(t) = self.size_persist_at {
            if now >= t {
                self.size_persist_at = None;
                if let (Some(w), Some(core)) = (self.window(), self.core.as_ref()) {
                    let size = w.inner_size().to_logical::<f32>(w.scale_factor());
                    core.update_config(|c| {
                        c.window_width = size.width;
                        c.window_height = size.height;
                    });
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
                self.request_redraw();
            }
        }
        let deadlines = [self.repaint_at, self.size_persist_at, self.smoke.as_ref().filter(|s| !s.armed).map(|s| s.at)];
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

/// SOMENTE DEPURAÇÃO: mapa de conversas fictício (`COMMANDDECK_DEBUG_UI=map-demo`).
fn debug_map() -> crate::agent_map::Snapshot {
    use crate::agent_map::{AnswerInFlight, Link, LinkState, NodeState, Recent, Snapshot};
    let now = commanddeck_core::util::now_ms();
    let link =
        |id, from: &str, to: &str, ago: u64, state| Link { id: Some(id), from: from.into(), to: to.into(), since_ms: now - ago, state };
    Snapshot {
        nodes: vec![
            ("claude".into(), NodeState::Busy),
            ("claude@dev".into(), NodeState::Busy),
            ("antigravity".into(), NodeState::Idle),
            ("codex".into(), NodeState::Idle),
            ("ollama:qwen3.5:9b".into(), NodeState::Idle),
            ("shell".into(), NodeState::Ended),
        ],
        links: vec![
            link(12, "claude", "claude@dev", 42_000, LinkState::Typed),
            link(13, "claude@dev", "antigravity", 5_000, LinkState::Queued(1)),
            link(14, "claude", "codex", 70_000, LinkState::Detached),
        ],
        answers: vec![AnswerInFlight { request: 9, from: "ollama:qwen3.5:9b".into(), to: "claude".into() }],
        recent: vec![Recent { from: "antigravity".into(), to: "claude@dev".into(), ok: true, at_ms: now - 20_000 }],
    }
}

/// SOMENTE DEPURAÇÃO: projetos fictícios na barra lateral (`COMMANDDECK_DEBUG_UI=projects-demo`): um ativo com
/// três agentes (um ocupado), outro com um, outro vazio.
fn debug_projects() -> Vec<crate::deck::ProjectView> {
    use crate::deck::{AgentBadge, ProjectView};
    let agent = |id: &str, parent: Option<&str>, kind: &str, label: &str, color, busy| AgentBadge {
        id: id.into(),
        parent: parent.map(Into::into),
        kind: kind.into(),
        label: label.into(),
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
            "commanddeck-dev",
            true,
            vec![
                agent("claude", None, "claude", "Claude", theme::CLAUDE, true),
                agent("claude@dev", Some("claude"), "claude", "Claude · Dev", theme::CLAUDE, false),
                agent("ollama:qwen3.5:9b", Some("claude@dev"), "ollama:qwen3.5:9b", "Ollama (qwen3.5:9b)", theme::OLLAMA, false),
                agent("antigravity@sidebar", Some("claude"), "antigravity", "Antigravity · Sidebar", theme::ANTIGRAVITY, false),
                agent("claude@review", None, "claude", "Claude · Review", theme::CLAUDE, false),
            ],
        ),
        project("p2", "jwic", false, vec![agent("claude", None, "claude", "Claude", theme::CLAUDE, false)]),
        project("p3", "site-novo", false, vec![]),
    ]
}

/// SOMENTE DEPURAÇÃO: uso fictício na barra de título (`COMMANDDECK_DEBUG_UI=usage-demo`): Claude no meio
/// da sessão, Antigravity com a semana esgotada (anel vermelho), Ollama local e Codex sem dados.
fn debug_usage() -> std::collections::HashMap<String, commanddeck_core::AgentUsage> {
    use commanddeck_core::{AgentUsage, UsageWindow};
    let now = commanddeck_core::util::now_ms();
    let w = |used, hours: u64| Some(UsageWindow { used, resets_at_ms: Some(now + hours * 3_600_000) });
    std::collections::HashMap::from([
        ("claude".into(), AgentUsage { session: w(0.34, 2), weekly: w(0.12, 100), updated_ms: now - 60_000, local: false }),
        ("antigravity".into(), AgentUsage { session: w(0.61, 3), weekly: w(1.0, 30), updated_ms: now - 120_000, local: false }),
        ("ollama".into(), AgentUsage { local: true, updated_ms: now, ..Default::default() }),
    ])
}

/// SOMENTE DEPURAÇÃO: `COMMANDDECK_DEBUG_UI=settings,newtab,drop,map,map-demo,usage-demo,projects-demo,project:<id>,git,git-demo,git-diff,git-branches,git-norepo,git-nogit,split-drag,download,download-retry,download-error` abre menus/estados
/// para capturas de tela no modo smoke.
fn apply_debug_ui(deck: &mut Deck) {
    let Ok(v) = std::env::var("COMMANDDECK_DEBUG_UI") else { return };
    for item in v.split(',').map(str::trim) {
        match item {
            "settings" => deck.settings_open = true,
            "shortcuts" => deck.toggle_shortcuts = true,
            "newtab" => deck.new_tab_menu_open = true,
            "drop" => deck.drop_hover = true,
            "map" => deck.set_right_view(crate::deck::RightView::Mapa),
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
            "git-norepo" | "git-nogit" => {
                deck.set_right_view(crate::deck::RightView::Git);
                deck.git.debug_hold = true;
                deck.git.view = if item == "git-norepo" { crate::git_ui::GitView::NotRepo } else { crate::git_ui::GitView::NoGit };
            }
            "github" => deck.set_right_view(crate::deck::RightView::GitHub),
            "github-demo" | "github-issues" | "github-actions" => {
                deck.github.debug_hold = true;
                deck.set_right_view(crate::deck::RightView::GitHub);
                deck.github.view = crate::github_ui::GithubView::Ready(crate::github_ui::demo_overview());
                let detail = crate::github_ui::demo_pr_detail();
                let log = crate::github_ui::demo_job_log();
                deck.github.run_jobs.insert(9001, detail.jobs.clone());
                deck.github.pr_details.insert(42, detail);
                deck.github.job_logs.insert(log.job_id, log.clone());
                match item {
                    "github-issues" => {
                        deck.github.tab = crate::github_ui::GithubTab::Issues;
                        deck.github.selected_issue = Some(38);
                    }
                    "github-actions" => {
                        deck.github.tab = crate::github_ui::GithubTab::Actions;
                        deck.github.selected_run = Some(9001);
                        deck.github.open_log = Some((log.job_id, log.job_name, None));
                    }
                    _ => {
                        deck.github.tab = crate::github_ui::GithubTab::Prs;
                        deck.github.selected_pr = Some(42);
                        deck.github.open_log = Some((log.job_id, log.job_name, Some(42)));
                    }
                }
            }
            "github-norepo" => {
                deck.github.debug_hold = true;
                deck.set_right_view(crate::deck::RightView::GitHub);
                deck.github.view = crate::github_ui::GithubView::NotGithub {
                    reason: "Esta pasta não tem um remoto `origin` apontando para github.com.".into(),
                };
            }
            "map-demo" => {
                deck.set_right_view(crate::deck::RightView::Mapa);
                deck.debug_map = Some(debug_map());
            }
            "usage-demo" => deck.debug_usage = Some(debug_usage()),
            "projects-demo" => deck.debug_projects = Some(debug_projects()),
            "split-drag" => {
                deck.pane_drag = Some(0);
                deck.debug_pane_drop = Some(1);
            }
            "sidebar-collapsed" => deck.sidebar_collapsed = true,
            "attach" => deck.attachments = vec!["dados/anexos/1a2b3c-captura de tela.png".into(), "dados/anexos/9f8e-relatorio.pdf".into()],
            "text" => deck.input = "Revisa o @README.md e sugere melhorias na seção de instalação".into(),
            "needs-text" => deck.needs_text = true,
            "empty" => deck.debug_close_all = true,
            "download" | "download-retry" | "download-error" => {
                deck.download = Some((debug_download(item), None));
                deck.debug_hold_download = true;
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

/// Progresso fictício para capturas de tela da barra de download (`COMMANDDECK_DEBUG_UI`).
fn debug_download(kind: &str) -> commanddeck_core::DownloadProgress {
    use commanddeck_core::{DownloadProgress, DownloadState};
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
