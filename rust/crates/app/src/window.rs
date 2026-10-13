//! Janela winit + contexto OpenGL (glutin) — base adaptada do exemplo `pure_glow` do egui_glow.

use std::num::NonZeroU32;
use std::sync::Arc;

use glutin::context::{ContextApi, ContextAttributesBuilder, NotCurrentGlContext, PossiblyCurrentContext};
use glutin::display::{GetGlDisplay, GlDisplay};
use glutin::surface::{GlSurface, Surface, SurfaceAttributesBuilder, SwapInterval, WindowSurface};
use raw_window_handle::HasWindowHandle;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowAttributes};

pub struct GlWindow {
    pub window: Window,
    context: PossiblyCurrentContext,
    surface: Surface<WindowSurface>,
}

impl GlWindow {
    /// Cria a janela (invisível até o primeiro quadro) e o contexto GL.
    pub fn new(event_loop: &ActiveEventLoop, attrs: WindowAttributes) -> anyhow::Result<(Self, Arc<glow::Context>)> {
        let template = glutin::config::ConfigTemplateBuilder::new().prefer_hardware_accelerated(None).with_depth_size(0).with_stencil_size(0).with_transparency(false);
        let (window, config) = glutin_winit::DisplayBuilder::new()
            .with_preference(glutin_winit::ApiPreference::FallbackEgl)
            .with_window_attributes(Some(attrs.clone()))
            .build(event_loop, template, |mut configs| configs.next().expect("nenhuma configuração OpenGL disponível"))
            .map_err(|e| anyhow::anyhow!("falha ao criar a configuração OpenGL: {e}"))?;
        let display = config.display();
        let raw = window.as_ref().and_then(|w| w.window_handle().ok().map(|h| h.as_raw()));
        let ctx_attrs = ContextAttributesBuilder::new().build(raw);
        let fallback = ContextAttributesBuilder::new().with_context_api(ContextApi::Gles(None)).build(raw);
        // SAFETY: o handle da janela é válido enquanto a janela existir (guardada junto do contexto).
        let not_current = unsafe { display.create_context(&config, &ctx_attrs).or_else(|_| display.create_context(&config, &fallback))? };
        let window = match window {
            Some(w) => w,
            None => glutin_winit::finalize_window(event_loop, attrs, &config)?,
        };
        let (w, h): (u32, u32) = window.inner_size().into();
        let surface_attrs = SurfaceAttributesBuilder::<WindowSurface>::new().build(
            window.window_handle()?.as_raw(),
            NonZeroU32::new(w).unwrap_or(NonZeroU32::MIN),
            NonZeroU32::new(h).unwrap_or(NonZeroU32::MIN),
        );
        // SAFETY: idem — a superfície vive junto da janela.
        let surface = unsafe { display.create_window_surface(&config, &surface_attrs)? };
        let context = not_current.make_current(&surface)?;
        let _ = surface.set_swap_interval(&context, SwapInterval::Wait(NonZeroU32::MIN));
        // SAFETY: o contexto está corrente nesta thread; os ponteiros vêm do próprio display.
        let gl = unsafe {
            glow::Context::from_loader_function(|s| {
                let s = std::ffi::CString::new(s).expect("nome de função GL inválido");
                display.get_proc_address(&s)
            })
        };
        Ok((Self { window, context, surface }, Arc::new(gl)))
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
