//! Application state machine: Idle (tray only) <-> Active (fullscreen).
//! Phase 0 only contains the skeleton; windows, tray and hotkey follow in phase 1.

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::window::WindowId;

use crate::cli::CliArgs;

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
    /// `--exit-after` elapsed or quit was requested.
    Quit,
}

pub struct App {
    args: CliArgs,
    mode: Mode,
}

impl App {
    pub fn new(args: CliArgs) -> Self {
        Self {
            args,
            mode: Mode::Idle,
        }
    }

    fn set_mode(&mut self, mode: Mode) {
        if self.mode != mode {
            tracing::info!(from = ?self.mode, to = ?mode, "mode change");
            self.mode = mode;
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(ControlFlow::Wait);
        if self.args.start {
            self.set_mode(Mode::Active);
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::ShowSettings => tracing::info!("show settings requested (phase 7)"),
            UserEvent::Quit => {
                tracing::info!("quit");
                self.set_mode(Mode::Idle);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
}
