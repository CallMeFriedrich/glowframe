//! Rotating file log (last 3 files) plus stderr in debug builds.

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{Builder, Rotation};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;

const DEFAULT_FILTER: &str = "info";

/// Initializes logging. The returned guard must live until the program ends,
/// otherwise buffered log lines are lost.
pub fn init(log_dir: &Path, level: Option<&str>) -> Option<WorkerGuard> {
    let filter = || {
        EnvFilter::try_new(level.unwrap_or(DEFAULT_FILTER))
            .unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER))
    };

    let file = std::fs::create_dir_all(log_dir).ok().and_then(|()| {
        Builder::new()
            .rotation(Rotation::DAILY)
            .filename_prefix("glowframe")
            .filename_suffix("log")
            .max_log_files(3)
            .build(log_dir)
            .ok()
    });

    let (file_layer, guard) = match file {
        Some(appender) => {
            let (writer, guard) = tracing_appender::non_blocking(appender);
            let layer = fmt::layer()
                .with_writer(writer)
                .with_ansi(false)
                .with_thread_names(true);
            (Some(layer), Some(guard))
        }
        None => (None, None),
    };

    let stderr_layer = cfg!(debug_assertions).then(|| fmt::layer().with_writer(std::io::stderr));

    let _ = tracing_subscriber::registry()
        .with(filter())
        .with(file_layer)
        .with(stderr_layer)
        .try_init();

    if guard.is_none() {
        tracing::warn!(dir = %log_dir.display(), "file logging unavailable");
    }
    guard
}
