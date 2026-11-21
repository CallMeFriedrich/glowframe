//! Settings window (SPEC §7): egui on OpenGL, exists only while open.

mod gl;
mod hotkey_rec;
mod panel;

use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Color32, FontData, FontDefinitions, FontFamily, ViewportId};
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId, WindowLevel};

pub use hotkey_rec::RecordError;
pub use panel::{MonitorInfo, PanelEvent, UpdateView};

use crate::i18n::I18n;
use crate::settings::Settings;

const BACKGROUND: [f32; 4] = [0.067, 0.063, 0.082, 1.0];

pub struct SettingsWindow {
    gl: gl::GlWindow,
    ctx: egui::Context,
    state: egui_winit::State,
    recording: bool,
    record_error: Option<RecordError>,
    repaint_at: Option<Instant>,
}

impl SettingsWindow {
    pub fn open(
        event_loop: &ActiveEventLoop,
        i18n: &I18n,
        topmost: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let attrs = Window::default_attributes()
            .with_title(title(i18n))
            .with_inner_size(LogicalSize::new(600.0, 780.0))
            .with_min_inner_size(LogicalSize::new(480.0, 420.0))
            .with_window_level(level(topmost));
        let gl = gl::GlWindow::new(event_loop, attrs)?;
        let ctx = egui::Context::default();
        ctx.set_fonts(system_fonts()?);
        style(&ctx);
        let state = egui_winit::State::new(
            ctx.clone(),
            ViewportId::ROOT,
            &gl.window,
            Some(gl.window.scale_factor() as f32),
            None,
            Some(gl.painter.max_texture_side()),
        );
        gl.window.focus_window();
        gl.window.request_redraw();
        Ok(Self {
            gl,
            ctx,
            state,
            recording: false,
            record_error: None,
            repaint_at: None,
        })
    }

    pub fn id(&self) -> WindowId {
        self.gl.window.id()
    }

    pub fn focus(&self) {
        self.gl.window.set_minimized(false);
        self.gl.window.focus_window();
    }

    pub fn set_topmost(&self, topmost: bool) {
        self.gl.window.set_window_level(level(topmost));
    }

    pub fn set_title(&self, i18n: &I18n) {
        self.gl.window.set_title(&title(i18n));
    }

    pub fn set_recording(&mut self, recording: bool, error: Option<RecordError>) {
        self.recording = recording;
        self.record_error = error;
        self.gl.window.request_redraw();
    }

    pub fn request_redraw(&self) {
        self.gl.window.request_redraw();
    }

    /// Time of the next repaint egui asked for (animations, tooltips).
    pub fn repaint_at(&self) -> Option<Instant> {
        self.repaint_at
    }

    pub fn on_event(&mut self, event: &WindowEvent) {
        if let WindowEvent::Resized(size) = event {
            self.gl.resize(size.width, size.height);
        }
        let response = self.state.on_window_event(&self.gl.window, event);
        if response.repaint {
            self.gl.window.request_redraw();
        }
    }

    /// Runs one egui frame; `settings` is edited in place.
    pub fn render(
        &mut self,
        settings: &mut Settings,
        i18n: &I18n,
        monitors: &[MonitorInfo],
        hotkey_error: Option<&str>,
        update: &UpdateView,
        image_note: Option<&str>,
    ) -> Vec<PanelEvent> {
        self.repaint_at = None;
        let raw = self.state.take_egui_input(&self.gl.window);
        let info = panel::PanelInfo {
            monitors,
            hotkey_error,
            recording: self.recording,
            record_error: self.record_error,
            update,
            image_note,
        };
        let mut events = Vec::new();
        let mut full = self.ctx.run_ui(raw, |ui| {
            egui::Frame::new()
                .fill(Color32::from_rgb(0x11, 0x10, 0x15))
                .inner_margin(22.0)
                .show(ui, |ui| {
                    events = panel::show(ui, settings, i18n, &info);
                });
        });
        self.state
            .handle_platform_output(&self.gl.window, std::mem::take(&mut full.platform_output));
        let primitives = self.ctx.tessellate(full.shapes, full.pixels_per_point);
        self.gl.paint(
            BACKGROUND,
            full.pixels_per_point,
            &primitives,
            &mut full.textures_delta,
        );
        if let Some(delay) = full
            .viewport_output
            .get(&ViewportId::ROOT)
            .map(|v| v.repaint_delay)
        {
            if delay.is_zero() {
                self.gl.window.request_redraw();
            } else if delay < Duration::from_secs(5) {
                self.repaint_at = Some(Instant::now() + delay);
            }
        }
        events
    }
}

fn title(i18n: &I18n) -> String {
    format!("Glowframe – {}", i18n.t("settings.title"))
}

fn level(topmost: bool) -> WindowLevel {
    // Above the topmost fullscreen windows while the mode runs (live preview).
    if topmost {
        WindowLevel::AlwaysOnTop
    } else {
        WindowLevel::Normal
    }
}

/// Segoe UI from the Windows font folder (no fonts embedded in the exe).
fn system_fonts() -> Result<FontDefinitions, std::io::Error> {
    let dir = std::env::var_os("WINDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "C:\\Windows".into())
        .join("Fonts");
    let regular = std::fs::read(dir.join("segoeui.ttf"))?;
    let mut fonts = FontDefinitions::empty();
    fonts
        .font_data
        .insert("segoe".into(), Arc::new(FontData::from_owned(regular)));
    let mut families = vec!["segoe".to_string()];
    // Symbols (arrows, check marks) used by egui widgets.
    if let Ok(symbols) = std::fs::read(dir.join("seguisym.ttf")) {
        fonts
            .font_data
            .insert("symbols".into(), Arc::new(FontData::from_owned(symbols)));
        families.push("symbols".into());
    }
    fonts
        .families
        .insert(FontFamily::Proportional, families.clone());
    fonts.families.insert(FontFamily::Monospace, families);
    Ok(fonts)
}

fn style(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = Color32::from_rgb(0x11, 0x10, 0x15);
    visuals.window_fill = Color32::from_rgb(0x1C, 0x1B, 0x22);
    visuals.extreme_bg_color = Color32::from_rgb(0x0B, 0x0A, 0x0E);
    visuals.selection.bg_fill = panel::ACCENT.gamma_multiply(0.75);
    visuals.selection.stroke.color = Color32::WHITE;
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(0x2A, 0x28, 0x33);
    visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(0x2A, 0x28, 0x33);
    visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x38, 0x35, 0x44);
    visuals.widgets.active.weak_bg_fill = panel::ACCENT.gamma_multiply(0.6);
    ctx.set_visuals(visuals);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(10.0, 10.0);
        s.spacing.button_padding = egui::vec2(12.0, 6.0);
        s.spacing.slider_width = 240.0;
        s.spacing.interact_size.y = 26.0;
        for (style, size) in [
            (egui::TextStyle::Body, 15.0),
            (egui::TextStyle::Button, 15.0),
            (egui::TextStyle::Heading, 22.0),
            (egui::TextStyle::Small, 12.0),
        ] {
            if let Some(f) = s.text_styles.get_mut(&style) {
                f.size = size;
            }
        }
    });
}
