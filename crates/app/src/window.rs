//! winit window + OpenGL context (glutin) — base adapted from egui_glow's `pure_glow` example.

use std::num::NonZeroU32;
use std::sync::Arc;

use glutin::config::{Config, GetGlConfig};
use glutin::context::{
    ContextApi, ContextAttributesBuilder, NotCurrentContext, NotCurrentGlContext, PossiblyCurrentContext, PossiblyCurrentGlContext,
};
use glutin::display::{GetGlDisplay, GlDisplay};
use glutin::surface::{GlSurface, Surface, SurfaceAttributesBuilder, SwapInterval, WindowSurface};
use raw_window_handle::HasWindowHandle;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowAttributes};

/// Fields drop in declaration order: the surface and the context before the window they draw on.
pub struct GlWindow {
    surface: Surface<WindowSurface>,
    context: PossiblyCurrentContext,
    pub window: Window,
}

impl GlWindow {
    /// Creates the window (invisible until the first frame) and the GL context.
    pub fn new(event_loop: &ActiveEventLoop, attrs: WindowAttributes) -> anyhow::Result<(Self, Arc<glow::Context>)> {
        let template = glutin::config::ConfigTemplateBuilder::new()
            .prefer_hardware_accelerated(None)
            .with_depth_size(0)
            .with_stencil_size(0)
            .with_transparency(false);
        let (window, config) = glutin_winit::DisplayBuilder::new()
            .with_preference(glutin_winit::ApiPreference::FallbackEgl)
            .with_window_attributes(Some(attrs.clone()))
            .build(event_loop, template, |mut configs| configs.next().expect("no OpenGL configuration available"))
            .map_err(|e| anyhow::anyhow!("failed to create the OpenGL configuration: {e}"))?;
        let display = config.display();
        let raw = window.as_ref().and_then(|w| w.window_handle().ok().map(|h| h.as_raw()));
        let ctx_attrs = ContextAttributesBuilder::new().build(raw);
        let fallback = ContextAttributesBuilder::new().with_context_api(ContextApi::Gles(None)).build(raw);
        // SAFETY: the window handle is valid while the window exists (kept together with the context).
        let not_current = unsafe { display.create_context(&config, &ctx_attrs).or_else(|_| display.create_context(&config, &fallback))? };
        let window = match window {
            Some(w) => w,
            None => glutin_winit::finalize_window(event_loop, attrs, &config)?,
        };
        let glw = Self::attach(window, &config, not_current)?;
        // SAFETY: the context is current on this thread; the pointers come from the display itself.
        let gl = unsafe {
            glow::Context::from_loader_function(|s| {
                let s = std::ffi::CString::new(s).expect("invalid GL function name");
                display.get_proc_address(&s)
            })
        };
        Ok((glw, Arc::new(gl)))
    }

    /// Gives `window` a surface and makes `context` current on it.
    fn attach(window: Window, config: &Config, context: NotCurrentContext) -> anyhow::Result<Self> {
        let (w, h): (u32, u32) = window.inner_size().into();
        let surface_attrs = SurfaceAttributesBuilder::<WindowSurface>::new().build(
            window.window_handle()?.as_raw(),
            NonZeroU32::new(w).unwrap_or(NonZeroU32::MIN),
            NonZeroU32::new(h).unwrap_or(NonZeroU32::MIN),
        );
        // SAFETY: the window handle is valid while the window exists — the surface lives together with it.
        let surface = unsafe { config.display().create_window_surface(config, &surface_attrs)? };
        let context = context.make_current(&surface)?;
        let _ = surface.set_swap_interval(&context, SwapInterval::Wait(NonZeroU32::MIN));
        Ok(Self { surface, context, window })
    }

    /// Destroys the window and keeps the GL context, with everything egui uploaded to it (Wayland cannot hide a
    /// window: closing to the tray destroys it, and [`ParkedGl::reopen`] makes another one).
    pub fn park(self) -> anyhow::Result<ParkedGl> {
        let Self { surface, context, window } = self;
        let context = context.make_not_current()?;
        drop(surface);
        drop(window);
        Ok(ParkedGl { context })
    }

    pub fn resize(&self, size: winit::dpi::PhysicalSize<u32>) {
        if let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) {
            self.surface.resize(&self.context, w, h);
        }
    }

    pub fn swap_buffers(&self) -> anyhow::Result<()> {
        self.surface.swap_buffers(&self.context)?;
        Ok(())
    }
}

/// The GL context of a destroyed window (see [`GlWindow::park`]). No GL call may run while parked.
pub struct ParkedGl {
    context: NotCurrentContext,
}

impl ParkedGl {
    /// A new window with `attrs`, drawing with the parked context. On failure the context is lost too.
    pub fn reopen(self, event_loop: &ActiveEventLoop, attrs: WindowAttributes) -> anyhow::Result<GlWindow> {
        let config = self.context.config();
        let window = glutin_winit::finalize_window(event_loop, attrs, &config)?;
        GlWindow::attach(window, &config, self.context)
    }
}
