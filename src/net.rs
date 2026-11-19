//! Network worker: one blocking HTTP request at a time on its own thread
//! (no Tokio, SPEC §3). Phase 4: HD covers; lyrics follow in phase 6.

use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use ureq::Agent;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

use crate::cache::DiskCache;
use crate::cover::hd::{self, SearchResponse};

const ITUNES_SEARCH: &str = "https://itunes.apple.com/search";
const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_JSON_BYTES: u64 = 2 * 1024 * 1024;
/// A failed HD lookup is retried after this time.
const NEGATIVE_TTL: Duration = Duration::from_secs(7 * 24 * 3600);
const COVER_CACHE_BYTES: u64 = 200 * 1024 * 1024;

pub enum NetJob {
    HdCover {
        album_key: u64,
        artist: String,
        album: String,
    },
}

#[derive(Debug, Clone)]
pub enum NetResult {
    /// Encoded HD image bytes, or `None` if no matching cover exists.
    HdCover {
        album_key: u64,
        bytes: Option<Arc<[u8]>>,
    },
}

pub struct NetWorker {
    tx: Sender<NetJob>,
}

impl NetWorker {
    pub fn start(
        cache_dir: std::path::PathBuf,
        on_result: impl Fn(NetResult) + Send + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::channel::<NetJob>();
        let spawned = std::thread::Builder::new()
            .name("net".into())
            .spawn(move || {
                let agent = agent();
                let covers = DiskCache::new(cache_dir.join("covers"), COVER_CACHE_BYTES);
                while let Ok(mut job) = rx.recv() {
                    // Skip outdated jobs after quick skipping.
                    while let Ok(newer) = rx.try_recv() {
                        job = newer;
                    }
                    match job {
                        NetJob::HdCover {
                            album_key,
                            artist,
                            album,
                        } => {
                            let bytes = hd_cover(&agent, &covers, &artist, &album);
                            on_result(NetResult::HdCover {
                                album_key,
                                bytes: bytes.map(Arc::from),
                            });
                        }
                    }
                }
            });
        if let Err(e) = spawned {
            tracing::error!(error = %e, "could not spawn net thread");
        }
        Self { tx }
    }

    pub fn submit(&self, job: NetJob) {
        let _ = self.tx.send(job);
    }
}

fn agent() -> Agent {
    Agent::config_builder()
        .tls_config(
            TlsConfig::builder()
                .provider(TlsProvider::NativeTls)
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .timeout_global(Some(Duration::from_secs(10)))
        .user_agent(format!(
            "Glowframe/{} (https://github.com/CallMeFriedrich/glowframe)",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .into()
}

/// Cache first, then iTunes search + artwork download. Negative results are
/// cached as an empty `.none` marker.
fn hd_cover(agent: &Agent, cache: &DiskCache, artist: &str, album: &str) -> Option<Vec<u8>> {
    if artist.trim().is_empty() || album.trim().is_empty() {
        return None;
    }
    let name = hd::cache_name(artist, album);
    let none_marker = format!("{name}.none");
    if let Some(bytes) = cache.get(&format!("{name}.img")) {
        tracing::debug!(%artist, %album, "HD cover from cache");
        return Some(bytes);
    }
    if cache
        .age(&none_marker)
        .is_some_and(|age| age < NEGATIVE_TTL)
    {
        return None;
    }

    match fetch_hd(agent, artist, album) {
        Ok(Some(bytes)) => {
            tracing::info!(%artist, %album, size = bytes.len(), "HD cover downloaded");
            cache.remove(&none_marker);
            cache.put(&format!("{name}.img"), &bytes);
            Some(bytes)
        }
        Ok(None) => {
            tracing::info!(%artist, %album, "no matching HD cover");
            cache.put(&none_marker, &[]);
            None
        }
        Err(e) => {
            // Network problems: do not cache, try again next time.
            tracing::warn!(error = %e, "HD cover lookup failed");
            None
        }
    }
}

/// iTunes store country from the Windows locale (e.g. `de-DE` -> `DE`).
fn store_country() -> String {
    let mut buf = [0u16; 85];
    // SAFETY: the buffer has LOCALE_NAME_MAX_LENGTH elements.
    let len = unsafe { windows::Win32::Globalization::GetUserDefaultLocaleName(&mut buf) };
    let name = String::from_utf16_lossy(&buf[..(len.max(1) as usize - 1)]);
    country_from_locale(&name).unwrap_or_else(|| "US".into())
}

fn country_from_locale(name: &str) -> Option<String> {
    let region = name.split('-').nth(1)?;
    (region.len() == 2 && region.chars().all(|c| c.is_ascii_alphabetic()))
        .then(|| region.to_ascii_uppercase())
}

fn fetch_hd(agent: &Agent, artist: &str, album: &str) -> Result<Option<Vec<u8>>, ureq::Error> {
    let term = format!("{} {}", artist, hd::normalize(album));
    let mut resp = agent
        .get(ITUNES_SEARCH)
        .query("term", &term)
        .query("entity", "album")
        .query("media", "music")
        .query("limit", "25")
        .query("country", store_country())
        .call()?;
    let text = resp
        .body_mut()
        .with_config()
        .limit(MAX_JSON_BYTES)
        .read_to_string()?;
    let parsed: SearchResponse = match serde_json::from_str(&text) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "iTunes response invalid");
            return Ok(None);
        }
    };
    let Some(url) = hd::pick_artwork(&parsed, artist, album) else {
        return Ok(None);
    };
    let mut img = agent.get(&url).call()?;
    let bytes = img
        .body_mut()
        .with_config()
        .limit(MAX_IMAGE_BYTES)
        .read_to_vec()?;
    Ok(Some(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn country_codes() {
        assert_eq!(country_from_locale("de-DE").as_deref(), Some("DE"));
        assert_eq!(country_from_locale("en-us").as_deref(), Some("US"));
        assert_eq!(country_from_locale("sr-Latn-RS"), None);
        assert_eq!(country_from_locale("de"), None);
    }
}
