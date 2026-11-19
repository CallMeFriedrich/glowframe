//! Application state machine: Idle (tray only) <-> Active (fullscreen).

use std::time::{Duration, Instant};

use tray_icon::menu::MenuId;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::{Key, NamedKey};
use winit::window::WindowId;

use crate::cli::CliArgs;
use crate::fullscreen::{self, MonitorSignature, MonitorWindow};
use crate::hotkey::Hotkey;
use crate::i18n::{self, I18n};
use crate::settings::Settings;
use crate::settings::paths::Paths;
use crate::tray::{MenuAction, Tray};

/// Second Esc must follow within this window (SPEC §4).
const DOUBLE_ESC: Duration = Duration::from_millis(600);
/// How often the monitor layout is checked while active.
const MONITOR_POLL: Duration = Duration::from_secs(2);

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
}

pub struct App {
    args: CliArgs,
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
    monitors: Option<MonitorSignature>,
    next_monitor_check: Instant,
    last_esc: Option<Instant>,
    initialized: bool,
}

impl App {
    pub fn new(args: CliArgs, paths: Paths, settings: Settings) -> Self {
        let i18n = I18n::new(i18n::resolve_language(settings.language));
        Self {
            args,
            paths,
            settings,
            i18n,
            mode: Mode::Idle,
            tray: None,
            hotkey: None,
            hotkey_error: None,
            windows: Vec::new(),
            monitors: None,
            next_monitor_check: Instant::now(),
            last_esc: None,
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

    fn activate(&mut self, event_loop: &ActiveEventLoop) {
        if self.mode == Mode::Active {
            return;
        }
        tracing::info!("mode: active");
        self.mode = Mode::Active;
        self.last_esc = None;
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
        self.next_monitor_check = Instant::now() + MONITOR_POLL;
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
                // The on-screen hint is drawn by the D2D layer (phase 3).
                tracing::debug!("first Esc");
                self.last_esc = Some(now);
            }
        }
    }

    fn show_settings(&mut self) {
        tracing::info!(hotkey_error = ?self.hotkey_error, "settings window requested (phase 7)");
    }

    fn quit(&mut self, event_loop: &ActiveEventLoop) {
        self.deactivate();
        tracing::info!("quit");
        event_loop.exit();
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
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::RedrawRequested => {
                if let Some(w) = self.window(id) {
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
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                ..
            } => {
                // Win back keyboard focus for Esc after clicks.
                if let Some(w) = self.window(id) {
                    fullscreen::focus(&w.window);
                }
            }
            WindowEvent::Moved(_)
            | WindowEvent::Resized(_)
            | WindowEvent::ScaleFactorChanged { .. } => {
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
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_monitor_check));
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
}
