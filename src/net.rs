//! Network worker: one blocking HTTP request at a time on its own thread
//! (no Tokio, SPEC §3). Serves lyrics (LRCLIB) and HD covers (iTunes).

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, SystemTime};

use ureq::Agent;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

use crate::cache::DiskCache;
use crate::cover::hd::{self, SearchResponse};
use crate::lyrics::lrclib::{self, Record};
use crate::lyrics::{LyricLine, lrc};

const ITUNES_SEARCH: &str = "https://itunes.apple.com/search";
const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_JSON_BYTES: u64 = 2 * 1024 * 1024;
/// Negative results (no HD cover, no lyrics) are retried after this time.
const NEGATIVE_TTL: Duration = Duration::from_secs(7 * 24 * 3600);
const COVER_CACHE_BYTES: u64 = 200 * 1024 * 1024;
const LYRICS_CACHE_BYTES: u64 = 20 * 1024 * 1024;
/// Upper bound for honoring `Retry-After` on HTTP 429.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(30);

pub enum NetJob {
    Lyrics {
        request_id: u64,
        title: String,
        artist: String,
        album: String,
        duration: Option<Duration>,
    },
    HdCover {
        album_key: u64,
        artist: String,
        album: String,
    },
}

#[derive(Debug, Clone)]
pub enum NetResult {
    /// Parsed synced lyrics, or `None` if there are none.
    Lyrics {
        request_id: u64,
        lines: Option<Vec<LyricLine>>,
    },
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
                let lyrics = DiskCache::new(cache_dir.join("lyrics"), LYRICS_CACHE_BYTES);
                while let Some((lyrics_job, cover_job)) = next_jobs(&rx) {
                    // Lyrics first: they matter more right after a track change.
                    if let Some(NetJob::Lyrics {
                        request_id,
                        title,
                        artist,
                        album,
                        duration,
                    }) = lyrics_job
                    {
                        let lines =
                            fetch_lyrics(&agent, &lyrics, &title, &artist, &album, duration);
                        on_result(NetResult::Lyrics { request_id, lines });
                    }
                    if let Some(NetJob::HdCover {
                        album_key,
                        artist,
                        album,
                    }) = cover_job
                    {
                        let bytes = hd_cover(&agent, &covers, &artist, &album);
                        on_result(NetResult::HdCover {
                            album_key,
                            bytes: bytes.map(Arc::from),
                        });
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

/// Blocks for the next job, then drains the queue keeping only the newest job
/// of each kind (outdated after quick skipping). `None` when disconnected.
fn next_jobs(rx: &Receiver<NetJob>) -> Option<(Option<NetJob>, Option<NetJob>)> {
    let first = rx.recv().ok()?;
    let (mut lyrics, mut cover) = (None, None);
    for job in std::iter::once(first).chain(rx.try_iter()) {
        match job {
            j @ NetJob::Lyrics { .. } => lyrics = Some(j),
            j @ NetJob::HdCover { .. } => cover = Some(j),
        }
    }
    Some((lyrics, cover))
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
        // Status codes are checked explicitly (404 / 429 handling).
        .http_status_as_error(false)
        .user_agent(format!(
            "Glowframe/{} (https://github.com/CallMeFriedrich/glowframe)",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .into()
}

#[derive(Debug)]
enum FetchError {
    Http(ureq::Error),
    Status(u16),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(e) => write!(f, "{e}"),
            Self::Status(s) => write!(f, "HTTP {s}"),
        }
    }
}

impl From<ureq::Error> for FetchError {
    fn from(e: ureq::Error) -> Self {
        Self::Http(e)
    }
}

/// GET returning `Ok(None)` on 404. Honors 429 `Retry-After` (+ jitter) once.
fn get_text(
    agent: &Agent,
    url: &str,
    query: &[(&str, String)],
) -> Result<Option<String>, FetchError> {
    for attempt in 0..2 {
        let mut req = agent.get(url);
        for (k, v) in query {
            req = req.query(*k, v);
        }
        let mut resp = req.call()?;
        match resp.status().as_u16() {
            200 => {
                let text = resp
                    .body_mut()
                    .with_config()
                    .limit(MAX_JSON_BYTES)
                    .read_to_string()?;
                return Ok(Some(text));
            }
            404 => return Ok(None),
            429 if attempt == 0 => {
                let wait = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .map_or(Duration::from_secs(5), Duration::from_secs)
                    .min(MAX_RETRY_AFTER);
                let jitter = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map_or(0, |d| d.subsec_millis() as u64);
                tracing::warn!(?wait, "rate limited, waiting");
                std::thread::sleep(wait + Duration::from_millis(jitter));
            }
            s => return Err(FetchError::Status(s)),
        }
    }
    Err(FetchError::Status(429))
}

// ----- lyrics -----

fn lyrics_cache_name(title: &str, artist: &str, album: &str, duration: Option<Duration>) -> String {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut h = DefaultHasher::new();
    (title, artist, album, duration.map(|d| d.as_secs())).hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Cache first (`.lrc` = found, `.none` = not found within 7 days), then
/// LRCLIB `/get`, falling back to `/search`.
fn fetch_lyrics(
    agent: &Agent,
    cache: &DiskCache,
    title: &str,
    artist: &str,
    album: &str,
    duration: Option<Duration>,
) -> Option<Vec<LyricLine>> {
    let name = lyrics_cache_name(title, artist, album, duration);
    if let Some(bytes) = cache.get(&format!("{name}.lrc")) {
        tracing::debug!(%title, "lyrics from cache");
        let lines = lrc::parse(&String::from_utf8_lossy(&bytes));
        return (!lines.is_empty()).then_some(lines);
    }
    let none_marker = format!("{name}.none");
    if cache
        .age(&none_marker)
        .is_some_and(|age| age < NEGATIVE_TTL)
    {
        tracing::debug!(%title, "lyrics known missing (cached)");
        return None;
    }

    match lookup_lyrics(agent, title, artist, album, duration) {
        Ok(Some(text)) => {
            let lines = lrc::parse(&text);
            if lines.is_empty() {
                cache.put(&none_marker, &[]);
                return None;
            }
            tracing::info!(%title, %artist, lines = lines.len(), "lyrics found");
            cache.remove(&none_marker);
            cache.put(&format!("{name}.lrc"), text.as_bytes());
            Some(lines)
        }
        Ok(None) => {
            tracing::info!(%title, %artist, "no synced lyrics");
            cache.put(&none_marker, &[]);
            None
        }
        Err(e) => {
            // Network problems: not cached, retried next time.
            tracing::warn!(error = %e, "lyrics lookup failed");
            None
        }
    }
}

fn lookup_lyrics(
    agent: &Agent,
    title: &str,
    artist: &str,
    album: &str,
    duration: Option<Duration>,
) -> Result<Option<String>, FetchError> {
    let secs = duration.map(|d| d.as_secs_f64());
    if let Some(d) = secs {
        let query = [
            ("track_name", title.to_string()),
            ("artist_name", artist.to_string()),
            ("album_name", album.to_string()),
            ("duration", format!("{}", d.round() as u64)),
        ];
        if let Some(text) = get_text(agent, &format!("{}/get", lrclib::API), &query)? {
            match serde_json::from_str::<Record>(&text) {
                Ok(r) if r.instrumental => return Ok(None),
                Ok(r) if r.synced().is_some() => return Ok(r.synced().map(String::from)),
                Ok(_) => {} // plain only: try search
                Err(e) => tracing::warn!(error = %e, "LRCLIB get response invalid"),
            }
        }
    }
    let query = [
        ("track_name", title.to_string()),
        ("artist_name", artist.to_string()),
    ];
    let Some(text) = get_text(agent, &format!("{}/search", lrclib::API), &query)? else {
        return Ok(None);
    };
    let records: Vec<Record> = match serde_json::from_str(&text) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "LRCLIB search response invalid");
            return Ok(None);
        }
    };
    Ok(lrclib::best_match(&records, secs)
        .and_then(Record::synced)
        .map(String::from))
}

// ----- HD covers -----

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

fn fetch_hd(agent: &Agent, artist: &str, album: &str) -> Result<Option<Vec<u8>>, FetchError> {
    let query = [
        ("term", format!("{} {}", artist, hd::normalize(album))),
        ("entity", "album".into()),
        ("media", "music".into()),
        ("limit", "25".into()),
        ("country", store_country()),
    ];
    let Some(text) = get_text(agent, ITUNES_SEARCH, &query)? else {
        return Ok(None);
    };
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
    if img.status().as_u16() != 200 {
        return Err(FetchError::Status(img.status().as_u16()));
    }
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

    #[test]
    fn job_queue_keeps_newest_per_kind() {
        let (tx, rx) = mpsc::channel();
        let lyr = |id| NetJob::Lyrics {
            request_id: id,
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            duration: None,
        };
        let cov = |k| NetJob::HdCover {
            album_key: k,
            artist: String::new(),
            album: String::new(),
        };
        for j in [lyr(1), cov(10), lyr(2), cov(11), lyr(3)] {
            tx.send(j).unwrap();
        }
        let (l, c) = next_jobs(&rx).unwrap();
        assert!(matches!(l, Some(NetJob::Lyrics { request_id: 3, .. })));
        assert!(matches!(c, Some(NetJob::HdCover { album_key: 11, .. })));
        drop(tx);
        assert!(next_jobs(&rx).is_none());
    }

    /// Manual check against the live API:
    /// `cargo test lrclib_live -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn lrclib_live() {
        let a = agent();
        let r = lookup_lyrics(
            &a,
            "Hips Don't Lie",
            "Shakira",
            "Oral Fixation, Vol. 2",
            Some(Duration::from_secs(218)),
        );
        match r {
            Ok(Some(t)) => println!("found {} lines", lrc::parse(&t).len()),
            other => println!("result: {other:?}"),
        }
    }
}
