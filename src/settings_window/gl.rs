//! OpenGL (WGL) window for egui: winit window + glutin context + egui_glow.
//! Exists only while the settings window is open (SPEC §3).

use std::num::NonZeroU32;
use std::sync::Arc;

use glutin::config::{ConfigTemplateBuilder, GlConfig};
use glutin::context::{
    ContextAttributesBuilder, NotCurrentGlContext, PossiblyCurrentContext, PossiblyCurrentGlContext,
};
use glutin::display::{Display, DisplayApiPreference, GlDisplay};
use glutin::surface::{GlSurface, Surface, SurfaceAttributesBuilder, SwapInterval, WindowSurface};
use winit::event_loop::ActiveEventLoop;
use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use winit::window::{Window, WindowAttributes};

pub struct GlWindow {
    // Field order matters for drop: painter, then surface/context, then window.
    pub painter: egui_glow::Painter,
    surface: Surface<WindowSurface>,
    context: PossiblyCurrentContext,
    pub window: Window,
}

type BoxError = Box<dyn std::error::Error>;

impl GlWindow {
    pub fn new(event_loop: &ActiveEventLoop, attrs: WindowAttributes) -> Result<Self, BoxError> {
        let window = event_loop.create_window(attrs)?;
        let raw_window = window.window_handle()?.as_raw();
        // SAFETY: the display handle belongs to the live event loop/window.
        let display = unsafe {
            Display::new(
                window.display_handle()?.as_raw(),
                DisplayApiPreference::Wgl(Some(raw_window)),
            )?
        };
        let template = ConfigTemplateBuilder::new()
            .compatible_with_native_window(raw_window)
            .with_alpha_size(8)
            .build();
        // SAFETY: valid display.
        // Only hardware accelerated formats: the generic GDI format is
        // OpenGL 1.1, which egui_glow cannot use. Prefer no multisampling.
        let config = unsafe { display.find_configs(template)? }
            .filter(|c| c.hardware_accelerated())
            .min_by_key(|c| c.num_samples())
            .ok_or("no hardware accelerated OpenGL config")?;

        let size = window.inner_size();
        let (w, h) = (
            NonZeroU32::new(size.width.max(1)).unwrap_or(NonZeroU32::MIN),
            NonZeroU32::new(size.height.max(1)).unwrap_or(NonZeroU32::MIN),
        );
        let surface_attrs =
            SurfaceAttributesBuilder::<WindowSurface>::new().build(raw_window, w, h);
        let context_attrs = ContextAttributesBuilder::new().build(Some(raw_window));
        // SAFETY: config, window handle and display belong together.
        let (surface, context) = unsafe {
            let surface = display.create_window_surface(&config, &surface_attrs)?;
            let context = display
                .create_context(&config, &context_attrs)?
                .make_current(&surface)?;
            (surface, context)
        };
        if let Err(e) = surface.set_swap_interval(&context, SwapInterval::Wait(NonZeroU32::MIN)) {
            tracing::debug!(error = %e, "vsync not available");
        }
        // SAFETY: the context is current on this thread.
        let gl = unsafe {
            glow::Context::from_loader_function_cstr(|s| display.get_proc_address(s).cast())
        };
        let painter = egui_glow::Painter::new(Arc::new(gl), "", None, false)?;
        Ok(Self {
            painter,
            surface,
            context,
            window,
        })
    }

    pub fn resize(&self, width: u32, height: u32) {
        if let (Some(w), Some(h)) = (NonZeroU32::new(width), NonZeroU32::new(height)) {
            self.surface.resize(&self.context, w, h);
        }
    }

    /// Clears, paints the egui output and swaps buffers.
    pub fn paint(
        &mut self,
        clear: [f32; 4],
        pixels_per_point: f32,
        primitives: &[egui::ClippedPrimitive],
        textures: &mut egui::TexturesDelta,
    ) {
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        if let Err(e) = self.context.make_current(&self.surface) {
            tracing::warn!(error = %e, "make_current failed");
            return;
        }
        let gl = self.painter.gl().clone();
        // SAFETY: plain GL state calls with the context current.
        unsafe {
            use glow::HasContext;
            gl.clear_color(clear[0], clear[1], clear[2], clear[3]);
            gl.clear(glow::COLOR_BUFFER_BIT);
        }
        self.painter.paint_and_update_textures(
            [size.width, size.height],
            pixels_per_point,
            primitives,
            textures,
        );
        if let Err(e) = self.surface.swap_buffers(&self.context) {
            tracing::warn!(error = %e, "swap_buffers failed");
        }
    }
}

impl Drop for GlWindow {
    fn drop(&mut self) {
        let _ = self.context.make_current(&self.surface);
        self.painter.destroy();
    }
}
