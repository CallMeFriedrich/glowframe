//! Cover processing worker: decodes thumbnails and HD covers and extracts the
//! palette off the render thread (SPEC §6.3).

pub mod hd;
pub mod palette;
pub mod wic;

use std::sync::Arc;
use std::sync::mpsc::{self, Sender};

use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

use crate::color::Oklab;
pub use wic::Image;

/// Edge length used for color extraction and the ambient blur (SPEC §6.3).
const SMALL_SIZE: u32 = 64;
/// HD covers larger than this are scaled down on decode.
const MAX_HD_SIZE: u32 = 1600;

#[derive(Debug, Clone)]
pub enum CoverResult {
    Thumb {
        key: u64,
        /// Full thumbnail for display.
        image: Option<Arc<Image>>,
        /// 64x64 version for the ambient glow.
        small: Option<Arc<Image>>,
        palette: Option<Vec<Oklab>>,
    },
    Hd {
        album_key: u64,
        image: Option<Arc<Image>>,
    },
}

enum Job {
    Thumb { key: u64, bytes: Arc<[u8]> },
    Hd { album_key: u64, bytes: Arc<[u8]> },
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
                        tracing::error!(error = %e, "WIC unavailable, no cover images");
                        return;
                    }
                };
                while let Ok(job) = rx.recv() {
                    on_result(process(&wic, job));
                }
            });
        if let Err(e) = spawned {
            tracing::error!(error = %e, "could not spawn cover thread");
        }
        Self { tx }
    }

    pub fn submit_thumb(&self, key: u64, bytes: Arc<[u8]>) {
        let _ = self.tx.send(Job::Thumb { key, bytes });
    }

    pub fn submit_hd(&self, album_key: u64, bytes: Arc<[u8]>) {
        let _ = self.tx.send(Job::Hd { album_key, bytes });
    }
}

fn process(wic: &wic::Wic, job: Job) -> CoverResult {
    let decode = |bytes: &[u8], size: Option<u32>| match wic.decode(bytes, size) {
        Ok(img) => Some(img),
        Err(e) => {
            tracing::warn!(error = %e, "cover decode failed");
            None
        }
    };
    match job {
        Job::Thumb { key, bytes } => {
            let image = decode(&bytes, None);
            let small = decode(&bytes, Some(SMALL_SIZE));
            let palette = small.as_ref().and_then(|s| palette::extract(&s.bgra));
            tracing::debug!(?image, ?palette, "thumbnail decoded");
            CoverResult::Thumb {
                key,
                image: image.map(Arc::new),
                small: small.map(Arc::new),
                palette,
            }
        }
        Job::Hd { album_key, bytes } => {
            let mut image = decode(&bytes, None);
            if image
                .as_ref()
                .is_some_and(|i| i.width.max(i.height) > MAX_HD_SIZE)
            {
                image = decode(&bytes, Some(MAX_HD_SIZE));
            }
            tracing::debug!(?image, "HD cover decoded");
            CoverResult::Hd {
                album_key,
                image: image.map(Arc::new),
            }
        }
    }
}
