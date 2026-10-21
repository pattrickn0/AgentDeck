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
        // Captura (COMMANDDECK_SMOKE) não baixa o whisper: cada pasta temporária começaria 1,6 GB.
        if self.frames == self.core_frame && std::env::var_os("COMMANDDECK_SMOKE").is_none() {
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

/// SOMENTE DEPURAÇÃO: projetos fictícios na barra lateral (`COMMANDDECK_DEBUG_UI=projects-demo`): um ativo com
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

/// SOMENTE DEPURAÇÃO: `COMMANDDECK_DEBUG_UI=tasks-demo,tasks-empty,tasks-detail,tasks-running,tasks-failed,tasks-diff,tasks-confirm,tasks-discard,tasks-return,tasks-error,tasks-busy,tasks-new,tasks-new-agent,tasks-new-model,tasks-new-provider,tasks-new-provider-models,settings,newtab,newtab-models:<agente>,drop,usage-demo,projects-demo,project:<id>,git,git-demo,git-diff,git-hunks,git-branches,git-branches-merge,git-merging,git-rebase,git-conflicts,git-conflict-file,git-history,git-commit,git-stash,git-stash-diff,git-norepo,git-nogit,split-drag,download,download-retry,download-error` abre menus/estados
/// para capturas de tela no modo smoke.
fn apply_debug_ui(deck: &mut Deck) {
    let Ok(v) = std::env::var("COMMANDDECK_DEBUG_UI") else { return };
    for item in v.split(',').map(str::trim) {
        match item {
            "settings" => deck.settings_open = true,
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
                    st.state = commanddeck_core::git::RepoState::Rebasing;
                    st.entries.retain(|e| e.kind != commanddeck_core::git::EntryKind::Unmerged);
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
                deck.github.view = crate::github_ui::GithubView::Error(commanddeck_core::github::GithubError::NoRemote);
            }
            "github-error" => {
                deck.github.debug_hold = true;
                deck.set_right_view(crate::deck::RightView::GitHub);
                deck.github.view = crate::github_ui::GithubView::Error(commanddeck_core::github::GithubError::NotFound {
                    repo: "pattr/commanddeck".into(),
                    authenticated: false,
                });
            }
            "tasks"
            | "tasks-demo"
            | "tasks-empty"
            | "tasks-detail"
            | "tasks-running"
            | "tasks-failed"
            | "tasks-diff"
            | "tasks-confirm"
            | "tasks-discard"
            | "tasks-return"
            | "tasks-error"
            | "tasks-busy"
            | "tasks-new"
            | "tasks-new-agent"
            | "tasks-new-model"
            | "tasks-new-provider"
            | "tasks-new-provider-models" => debug_tasks(deck, item),
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

/// SOMENTE DEPURAÇÃO: estados fictícios da aba Tarefas (`COMMANDDECK_DEBUG_UI=tasks-*`), sem git nem core.
fn debug_tasks(deck: &mut Deck, item: &str) {
    use crate::tasks_ui::{self as t, Confirm, FormMenu, OpenDiff, TasksView};
    deck.set_right_view(crate::deck::RightView::Tarefas);
    deck.tasks.debug_hold = true;
    deck.tasks.loaded = true;
    deck.tasks.tasks = if item == "tasks-empty" { Vec::new() } else { t::demo_tasks() };
    // A lista de capturas mostra todas as tarefas: as concluídas ficam abertas.
    deck.tasks.collapsed.clear();
    let detail = |id: &str| TasksView::Detail(id.to_owned());
    match item {
        "tasks-detail" | "tasks-confirm" | "tasks-return" | "tasks-error" | "tasks-busy" | "tasks-diff" => {
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
                "tasks-busy" => deck.tasks.busy = Some(t::Busy { id: "t4".into(), label: "Mesclando…" }),
                "tasks-diff" => {
                    deck.tasks.diff = Some(OpenDiff { id: "t4".into(), diff: Some(Ok(crate::git_ui::demo_diff())) });
                }
                _ => {}
            }
        }
        "tasks-discard" => {
            deck.tasks.view = detail("t6");
            deck.tasks.confirm = Some(Confirm::Discard("t6".into()));
        }
        "tasks-running" => deck.tasks.view = detail("t6"),
        "tasks-failed" => deck.tasks.view = detail("t3"),
        "tasks-new" | "tasks-new-agent" | "tasks-new-model" | "tasks-new-provider" | "tasks-new-provider-models" => {
            deck.tasks_open_new();
            let f = &mut deck.tasks.form;
            f.kind = if item.starts_with("tasks-new-provider") { "opencode".into() } else { "claude".into() };
            f.base = "dev".into();
            f.title = "Documentar as rotas /tasks da ponte".into();
            f.brief = "Explicar no memory/ARCHITECTURE.md cada rota, com o corpo de exemplo e os códigos de erro.\n\nConferir também o texto do ask-agent --task.".into();
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
