#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod cli;
mod logging;
mod settings;
mod single_instance;

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

    if let Err(e) = run(args, guard.as_ref()) {
        tracing::error!(error = %e, "event loop failed");
    }
    tracing::info!("Glowframe stopped");
}

fn run(
    args: CliArgs,
    guard: Option<&single_instance::PrimaryGuard>,
) -> Result<(), winit::error::EventLoopError> {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;

    if let Some(guard) = guard {
        let proxy = event_loop.create_proxy();
        guard.listen(move || {
            let _ = proxy.send_event(UserEvent::ShowSettings);
        });
    }

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

    let mut app = App::new(args);
    event_loop.run_app(&mut app)
}
