//! Application state machine: Idle (tray only) <-> Active (fullscreen).

use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::{Duration, Instant, SystemTime};

use tray_icon::menu::MenuId;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::WindowId;

use crate::cli::CliArgs;
use crate::color::Oklab;
use crate::cover::{CoverResult, CoverWorker};
use crate::demo::Demo;
use crate::fullscreen::{self, MonitorSignature, MonitorWindow};
use crate::hotkey::Hotkey;
use crate::i18n::{self, I18n};
use crate::media::{Command, MediaService, MediaSnapshot, Playback};
use crate::render::{FrameResult, Gpu, Overlay, Surface};
use crate::settings::model::ColorMode;
use crate::settings::paths::Paths;
use crate::settings::{MonitorMode, Settings};
use crate::tray::{MenuAction, Tray};
use crate::ui::frame::{FrameAnimator, FrameInputs, FrameState, preset_colors};

/// Second Esc must follow within this window (SPEC §4).
const DOUBLE_ESC: Duration = Duration::from_millis(600);
/// The Esc hint fades out after this time (SPEC §4).
const HINT_DURATION: f32 = 1.5;
const HINT_FADE: f32 = 0.4;
/// How often the monitor layout is checked while active.
const MONITOR_POLL: Duration = Duration::from_secs(2);

fn fmt_time(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}.{}", s / 60, s % 60, d.subsec_millis() / 100)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Idle,
    Active,
}

/// Events posted into the event loop from other threads.
#[derive(Debug, Clone)]
pub enum UserEvent {
    /// A second instance was started.
    ShowSettings,
    /// `--exit-after` elapsed.
    Quit,
    TrayDoubleClick,
    Menu(MenuId),
    Hotkey(u32),
    Media(Box<MediaSnapshot>),
    Cover(Box<CoverResult>),
}

pub struct App {
    args: CliArgs,
    proxy: EventLoopProxy<UserEvent>,
    media_service: Option<MediaService>,
    demo: Option<Demo>,
    media: MediaSnapshot,
    cover_worker: Option<CoverWorker>,
    /// Key of the thumbnail last submitted for color extraction.
    cover_key: Option<u64>,
    cover_palette: Option<Vec<Oklab>>,
    #[allow(dead_code)] // settings persistence from the settings window (phase 7)
    paths: Paths,
    settings: Settings,
    i18n: I18n,
    mode: Mode,
    tray: Option<Tray>,
    hotkey: Option<Hotkey>,
    /// Shown as a hint in the settings window (phase 7).
    hotkey_error: Option<String>,
    windows: Vec<MonitorWindow>,
    gpu: Option<Gpu>,
    /// GPU init failed for this activation: animation monitors stay black.
    gpu_failed: bool,
    animator: FrameAnimator,
    monitors: Option<MonitorSignature>,
    next_monitor_check: Instant,
    last_esc: Option<Instant>,
    hint_since: Option<Instant>,
    initialized: bool,
}

impl App {
    pub fn new(
        args: CliArgs,
        paths: Paths,
        settings: Settings,
        proxy: EventLoopProxy<UserEvent>,
    ) -> Self {
        let i18n = I18n::new(i18n::resolve_language(settings.language));
        Self {
            args,
            proxy,
            media_service: None,
            demo: None,
            media: MediaSnapshot::default(),
            cover_worker: None,
            cover_key: None,
            cover_palette: None,
            paths,
            settings,
            i18n,
            mode: Mode::Idle,
            tray: None,
            hotkey: None,
            hotkey_error: None,
            windows: Vec::new(),
            gpu: None,
            gpu_failed: false,
            animator: FrameAnimator::new(Instant::now()),
            monitors: None,
            next_monitor_check: Instant::now(),
            last_esc: None,
            hint_since: None,
            initialized: false,
        }
    }

    fn init(&mut self, event_loop: &ActiveEventLoop) {
        match Tray::new(&self.i18n) {
            Ok(t) => self.tray = Some(t),
            Err(e) => tracing::error!(error = %e, "tray icon unavailable"),
        }
        match Hotkey::new() {
            Ok(h) => self.hotkey = Some(h),
            Err(e) => tracing::error!(error = %e, "hotkey manager unavailable"),
        }
        self.register_hotkey();

        let proxy = self.proxy.clone();
        self.cover_worker = Some(CoverWorker::start(move |result| {
            let _ = proxy.send_event(UserEvent::Cover(Box::new(result)));
        }));

        if self.args.demo {
            tracing::info!("demo mode: fake Spotify data, synthetic 120 BPM beat");
            let demo = Demo::new();
            let snapshot = demo.snapshot();
            self.demo = Some(demo);
            self.on_media(snapshot);
        } else {
            let proxy = self.proxy.clone();
            self.media_service = Some(MediaService::start(move |snapshot| {
                let _ = proxy.send_event(UserEvent::Media(Box::new(snapshot)));
            }));
        }

        if self.args.start {
            self.activate(event_loop);
        }
    }

    fn register_hotkey(&mut self) {
        let Some(hk) = self.hotkey.as_mut() else {
            return;
        };
        let spec = self.settings.hotkey.clone();
        match hk.register(&spec) {
            Ok(()) => self.hotkey_error = None,
            Err(e) => {
                tracing::warn!(hotkey = %spec, error = %e, "hotkey registration failed");
                self.hotkey_error = Some(e);
                if let Some(tray) = &self.tray {
                    tray.notify(
                        self.i18n.t("notify.hotkey_failed_title"),
                        &self.i18n.tf("notify.hotkey_failed", &[("hotkey", &spec)]),
                    );
                }
            }
        }
    }

    // ----- mode -----

    fn activate(&mut self, event_loop: &ActiveEventLoop) {
        if self.mode == Mode::Active {
            return;
        }
        tracing::info!("mode: active");
        self.mode = Mode::Active;
        self.last_esc = None;
        self.hint_since = None;
        self.gpu_failed = false;
        self.animator = FrameAnimator::new(Instant::now());
        self.animator.jump_palette(&self.target_palette());
        fullscreen::keep_awake(true);
        self.build_windows(event_loop);
        if let Some(t) = self.tray.as_mut() {
            t.set_active(true, &self.i18n);
        }
    }

    fn deactivate(&mut self) {
        if self.mode == Mode::Idle {
            return;
        }
        tracing::info!("mode: idle");
        self.mode = Mode::Idle;
        self.windows.clear();
        self.gpu = None;
        self.monitors = None;
        fullscreen::keep_awake(false);
        if let Some(t) = self.tray.as_mut() {
            t.set_active(false, &self.i18n);
        }
    }

    fn toggle(&mut self, event_loop: &ActiveEventLoop) {
        match self.mode {
            Mode::Idle => self.activate(event_loop),
            Mode::Active => self.deactivate(),
        }
    }

    fn build_windows(&mut self, event_loop: &ActiveEventLoop) {
        self.windows.clear();
        self.monitors = Some(fullscreen::monitor_signature(event_loop));
        self.windows = fullscreen::create_windows(event_loop, &self.settings);
        self.ensure_surfaces();
        self.next_monitor_check = Instant::now() + MONITOR_POLL;
    }

    /// Creates the GPU context and the swap chains of animation windows.
    /// On failure the affected monitors stay black (GDI).
    fn ensure_surfaces(&mut self) {
        let needs = self
            .windows
            .iter()
            .any(|w| w.mode == MonitorMode::Animation && w.surface.is_none());
        if !needs || self.gpu_failed {
            return;
        }
        if self.gpu.is_none() {
            match Gpu::new() {
                Ok(g) => self.gpu = Some(g),
                Err(e) => {
                    tracing::error!(error = %e, "GPU init failed, animation monitors stay black");
                    self.gpu_failed = true;
                    return;
                }
            }
        }
        let Some(gpu) = &self.gpu else { return };
        for w in self
            .windows
            .iter_mut()
            .filter(|w| w.mode == MonitorMode::Animation && w.surface.is_none())
        {
            let Some(hwnd) = fullscreen::hwnd(&w.window) else {
                continue;
            };
            let size = w.window.inner_size();
            let scale = w.window.scale_factor() as f32;
            match Surface::new(gpu, hwnd, size.width, size.height, scale) {
                Ok(s) => {
                    w.surface = Some(s);
                    w.window.request_redraw();
                }
                Err(e) => tracing::error!(error = %e, "swap chain creation failed"),
            }
        }
    }

    fn device_lost(&mut self) {
        tracing::warn!("recreating GPU resources");
        for w in &mut self.windows {
            w.surface = None;
        }
        self.gpu = None;
        self.ensure_surfaces();
    }

    fn check_monitors(&mut self, event_loop: &ActiveEventLoop) {
        let sig = fullscreen::monitor_signature(event_loop);
        if self.monitors.as_ref() != Some(&sig) {
            tracing::info!("monitor layout changed, rebuilding windows");
            self.build_windows(event_loop);
        }
        self.next_monitor_check = Instant::now() + MONITOR_POLL;
    }

    fn on_escape(&mut self) {
        let now = Instant::now();
        match self.last_esc {
            Some(t) if now.duration_since(t) <= DOUBLE_ESC => {
                self.last_esc = None;
                self.deactivate();
            }
            _ => {
                self.last_esc = Some(now);
                self.hint_since = Some(now);
            }
        }
    }

    fn show_settings(&mut self) {
        tracing::info!(hotkey_error = ?self.hotkey_error, "settings window requested (phase 7)");
    }

    fn quit(&mut self, event_loop: &ActiveEventLoop) {
        self.deactivate();
        self.media_service = None;
        self.cover_worker = None;
        tracing::info!("quit");
        event_loop.exit();
    }

    // ----- media and colors -----

    fn on_media(&mut self, snapshot: MediaSnapshot) {
        if snapshot == self.media {
            return;
        }
        let title = snapshot.session.as_ref().map(|s| s.display_title());
        if title != self.media.session.as_ref().map(|s| s.display_title())
            && let Some(tray) = &self.tray
        {
            tray.set_now_playing(title);
        }
        if let Some(s) = &snapshot.session
            && let Some(thumb) = &s.thumbnail
        {
            let mut h = DefaultHasher::new();
            s.track_key().hash(&mut h);
            thumb.0.hash(&mut h);
            let key = h.finish();
            if self.cover_key != Some(key) {
                self.cover_key = Some(key);
                if let Some(w) = &self.cover_worker {
                    w.submit(key, thumb.0.clone());
                }
            }
        }
        self.media = snapshot;
        self.log_position();
    }

    fn on_cover(&mut self, result: CoverResult) {
        if Some(result.key) != self.cover_key {
            return; // outdated
        }
        if let Some(p) = result.palette {
            self.cover_palette = Some(p);
            let target = self.target_palette();
            self.animator.set_palette(&target, Instant::now());
        }
    }

    /// Palette for the frame: cover colors, fixed palette, or fallback.
    fn target_palette(&self) -> Vec<Oklab> {
        let f = &self.settings.frame;
        match (f.color_mode, &self.cover_palette) {
            (ColorMode::Cover, Some(p)) => p.clone(),
            (ColorMode::Cover, None) => preset_colors(f.palette, f.custom_colors)
                .map(Oklab::from_rgb)
                .to_vec(),
            (ColorMode::Palette, _) => preset_colors(f.palette, f.custom_colors)
                .map(Oklab::from_rgb)
                .to_vec(),
        }
    }

    fn frame_state(&self) -> FrameState {
        match &self.media.session {
            None => FrameState::Standby,
            Some(s) if s.playback == Playback::Playing => FrameState::Playing,
            Some(_) => FrameState::Paused,
        }
    }

    fn send_command(&mut self, cmd: Command) {
        if let Some(demo) = self.demo.as_mut() {
            let snapshot = demo.command(cmd);
            self.on_media(snapshot);
        } else if let Some(m) = &self.media_service {
            m.command(cmd);
        }
    }

    /// Debug output of the interpolated position.
    fn log_position(&self) {
        match &self.media.session {
            Some(s) => tracing::debug!(
                title = %s.display_title(),
                artist = %s.artist,
                playback = ?s.playback,
                position = %fmt_time(s.clock.position(SystemTime::now())),
                duration = %s.clock.duration().map(fmt_time).unwrap_or_default(),
                "now playing"
            ),
            None => tracing::debug!("no Spotify session"),
        }
    }

    /// Development keys in debug builds: Space play/pause, Left/Right
    /// previous/next, S shuffle, R repeat. The final UI follows in phase 4.
    #[cfg(debug_assertions)]
    fn dev_key(&mut self, key: &Key) {
        let cmd = match key {
            Key::Named(NamedKey::Space) => Command::PlayPause,
            Key::Named(NamedKey::ArrowRight) => Command::Next,
            Key::Named(NamedKey::ArrowLeft) => Command::Previous,
            Key::Character(c) if c.eq_ignore_ascii_case("s") => Command::ToggleShuffle,
            Key::Character(c) if c.eq_ignore_ascii_case("r") => Command::CycleRepeat,
            _ => return,
        };
        self.send_command(cmd);
    }

    // ----- rendering -----

    fn hint_alpha(&self, now: Instant) -> f32 {
        let Some(t0) = self.hint_since else {
            return 0.0;
        };
        let e = now.saturating_duration_since(t0).as_secs_f32();
        let fade_in = (e / 0.12).min(1.0);
        let fade_out = ((HINT_DURATION - e) / HINT_FADE).clamp(0.0, 1.0);
        fade_in * fade_out
    }

    fn render(&mut self, id: WindowId) {
        let now = Instant::now();
        let state = self.frame_state();
        let (since_beat, energy) = match &self.demo {
            Some(d) => (d.since_beat(now), d.energy(now)),
            None => (None, 0.5), // audio analysis follows in phase 5
        };
        let hint_alpha = self.hint_alpha(now);
        if hint_alpha == 0.0
            && self
                .hint_since
                .is_some_and(|t| now - t > Duration::from_secs(2))
        {
            self.hint_since = None;
        }

        let Some(gpu) = &self.gpu else { return };
        let Some(w) = self.windows.iter().find(|w| w.id() == id) else {
            return;
        };
        let Some(surface) = &w.surface else { return };
        let (width, height) = surface.size();
        let inputs = FrameInputs {
            state,
            settings: &self.settings.frame,
            since_beat,
            energy,
        };
        let params = self.animator.update(now, &inputs, width, height);
        let overlay = Overlay {
            hint: (hint_alpha > 0.0).then(|| (self.i18n.t("fullscreen.esc_again"), hint_alpha)),
        };
        let result = surface.render(gpu, &params, &overlay);
        w.window.request_redraw();
        if result == FrameResult::DeviceLost {
            self.device_lost();
        }
    }

    fn resize(&mut self, id: WindowId) {
        let Some(gpu) = &self.gpu else { return };
        if let Some(w) = self.windows.iter_mut().find(|w| w.id() == id)
            && let Some(s) = w.surface.as_mut()
        {
            let size = w.window.inner_size();
            let scale = w.window.scale_factor() as f32;
            if let Err(e) = s.resize(gpu, size.width, size.height, scale) {
                tracing::warn!(error = %e, "resize failed, recreating surface");
                w.surface = None;
                self.ensure_surfaces();
            }
        }
    }

    fn window(&self, id: WindowId) -> Option<&MonitorWindow> {
        self.windows.iter().find(|w| w.id() == id)
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        if cause == StartCause::Init && !self.initialized {
            self.initialized = true;
            self.init(event_loop);
        }
    }

    fn resumed(&mut self, _: &ActiveEventLoop) {}

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::ShowSettings => self.show_settings(),
            UserEvent::Quit => self.quit(event_loop),
            UserEvent::TrayDoubleClick => self.activate(event_loop),
            UserEvent::Hotkey(id) => {
                if self.hotkey.as_ref().is_some_and(|h| h.matches(id)) {
                    self.toggle(event_loop);
                }
            }
            UserEvent::Menu(id) => match self.tray.as_ref().and_then(|t| t.action(&id)) {
                Some(MenuAction::Toggle) => self.toggle(event_loop),
                Some(MenuAction::Settings) => self.show_settings(),
                Some(MenuAction::Quit) => self.quit(event_loop),
                None => {}
            },
            UserEvent::Media(snapshot) => self.on_media(*snapshot),
            UserEvent::Cover(result) => self.on_cover(*result),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::RedrawRequested => {
                let has_surface = self.window(id).is_some_and(|w| w.surface.is_some());
                if has_surface {
                    self.render(id);
                } else if let Some(w) = self.window(id) {
                    fullscreen::paint_black(&w.window);
                }
            }
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed
                    && !event.repeat
                    && event.logical_key == Key::Named(NamedKey::Escape) =>
            {
                self.on_escape();
            }
            #[cfg(debug_assertions)]
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed && !event.repeat =>
            {
                self.dev_key(&event.logical_key);
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                ..
            } => {
                // Win back keyboard focus for Esc after clicks.
                if let Some(w) = self.window(id) {
                    fullscreen::focus(&w.window);
                }
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                if self.mode == Mode::Active {
                    self.resize(id);
                    self.check_monitors(event_loop);
                }
            }
            WindowEvent::Moved(_) => {
                if self.mode == Mode::Active {
                    self.check_monitors(event_loop);
                }
            }
            WindowEvent::CloseRequested => {
                // Alt+F4 on a fullscreen window ends the mode, not the program.
                self.deactivate();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.mode == Mode::Active {
            if Instant::now() >= self.next_monitor_check {
                self.check_monitors(event_loop);
                self.log_position();
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_monitor_check));
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
}
