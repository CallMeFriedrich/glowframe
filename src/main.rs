#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod cli;
mod color;
mod cover;
mod demo;
mod fullscreen;
mod hotkey;
mod i18n;
mod logging;
mod media;
mod render;
mod settings;
mod single_instance;
mod tray;
mod ui;

use global_hotkey::{GlobalHotKeyEvent, HotKeyState};
use tray_icon::menu::MenuEvent;
use tray_icon::{MouseButton, TrayIconEvent};
use winit::event_loop::EventLoop;

use crate::app::{App, UserEvent};
use crate::cli::CliArgs;
use crate::settings::paths::Paths;
use crate::single_instance::Instance;

fn main() {
    let (args, warnings) = CliArgs::parse(std::env::args().skip(1));
    let paths = Paths::detect();
    let _log_guard = logging::init(&paths.log_dir, args.log_level.as_deref());

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        portable = paths.portable,
        ?args,
        "Glowframe starting"
    );
    tracing::debug!(
        settings = %paths.settings_file().display(),
        cache = %paths.cache_dir.display(),
        logs = %paths.log_dir.display(),
        "paths"
    );
    for w in &warnings {
        tracing::warn!("{w}");
    }

    let guard = match single_instance::acquire() {
        Some(Instance::Secondary) => {
            tracing::info!("already running, asking primary instance to show settings");
            single_instance::signal_primary();
            return;
        }
        Some(Instance::Primary(guard)) => Some(guard),
        None => None,
    };

    let (settings, outcome) = settings::store::load(&paths.settings_file());
    tracing::info!(?outcome, "settings loaded");

    if let Err(e) = run(args, paths, settings, guard.as_ref()) {
        tracing::error!(error = %e, "event loop failed");
    }
    tracing::info!("Glowframe stopped");
}

fn run(
    args: CliArgs,
    paths: Paths,
    settings: settings::Settings,
    guard: Option<&single_instance::PrimaryGuard>,
) -> Result<(), winit::error::EventLoopError> {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;

    if let Some(guard) = guard {
        let proxy = event_loop.create_proxy();
        guard.listen(move || {
            let _ = proxy.send_event(UserEvent::ShowSettings);
        });
    }

    let proxy = event_loop.create_proxy();
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        if let TrayIconEvent::DoubleClick {
            button: MouseButton::Left,
            ..
        } = e
        {
            let _ = proxy.send_event(UserEvent::TrayDoubleClick);
        }
    }));
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        let _ = proxy.send_event(UserEvent::Menu(e.id));
    }));
    let proxy = event_loop.create_proxy();
    GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
        if e.state == HotKeyState::Pressed {
            let _ = proxy.send_event(UserEvent::Hotkey(e.id));
        }
    }));

    if let Some(after) = args.exit_after {
        let proxy = event_loop.create_proxy();
        let spawned = std::thread::Builder::new()
            .name("exit-after".into())
            .spawn(move || {
                std::thread::sleep(after);
                let _ = proxy.send_event(UserEvent::Quit);
            });
        if let Err(e) = spawned {
            tracing::error!(error = %e, "could not spawn exit-after thread");
        }
    }

    let mut app = App::new(args, paths, settings, event_loop.create_proxy());
    event_loop.run_app(&mut app)
}
