//! Cover processing worker: decodes thumbnails and extracts the palette off
//! the render thread. HD covers and the cache follow in phase 4.

pub mod palette;
pub mod wic;

use std::sync::Arc;
use std::sync::mpsc::{self, Sender};

use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

use crate::color::Oklab;

/// Edge length used for color extraction (SPEC §6.3).
const PALETTE_SIZE: u32 = 64;

#[derive(Debug, Clone)]
pub struct CoverResult {
    /// Opaque key of the track the job belonged to.
    pub key: u64,
    /// 2-3 colors, or `None` if the cover could not be decoded.
    pub palette: Option<Vec<Oklab>>,
}

struct Job {
    key: u64,
    bytes: Arc<[u8]>,
}

pub struct CoverWorker {
    tx: Sender<Job>,
}

impl CoverWorker {
    pub fn start(on_result: impl Fn(CoverResult) + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        let spawned = std::thread::Builder::new()
            .name("cover".into())
            .spawn(move || {
                // SAFETY: initializes COM (MTA) for this worker thread.
                let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
                let wic = match wic::Wic::new() {
                    Ok(w) => w,
                    Err(e) => {
                        tracing::error!(error = %e, "WIC unavailable, no cover colors");
                        return;
                    }
                };
                while let Ok(mut job) = rx.recv() {
                    // Only the newest job matters after a quick skip burst.
                    while let Ok(newer) = rx.try_recv() {
                        job = newer;
                    }
                    let palette = match wic.decode(&job.bytes, Some(PALETTE_SIZE)) {
                        Ok(img) => palette::extract(&img.bgra),
                        Err(e) => {
                            tracing::warn!(error = %e, "cover decode failed");
                            None
                        }
                    };
                    tracing::debug!(?palette, "cover palette");
                    on_result(CoverResult {
                        key: job.key,
                        palette,
                    });
                }
            });
        if let Err(e) = spawned {
            tracing::error!(error = %e, "could not spawn cover thread");
        }
        Self { tx }
    }

    pub fn submit(&self, key: u64, bytes: Arc<[u8]>) {
        let _ = self.tx.send(Job { key, bytes });
    }
}
