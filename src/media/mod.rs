//! Spotify media session via GSMTC (SPEC §6.1).

pub mod clock;
mod gsmtc;

use std::sync::Arc;

pub use clock::PositionClock;
pub use gsmtc::MediaService;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Playback {
    Playing,
    Paused,
    /// Stopped, opening, changing, closed.
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepeatMode {
    Off,
    All,
    One,
}

impl RepeatMode {
    /// Cycle order of the repeat button: off -> all -> one -> off.
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::All,
            Self::All => Self::One,
            Self::One => Self::Off,
        }
    }
}

/// Which controls the session currently offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Controls {
    pub play: bool,
    pub pause: bool,
    pub next: bool,
    pub previous: bool,
    pub shuffle: bool,
    pub repeat: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    PlayPause,
    Next,
    Previous,
    ToggleShuffle,
    CycleRepeat,
}

/// Encoded thumbnail bytes (PNG/JPEG); decoding happens in `cover/`.
#[derive(Clone, PartialEq, Eq)]
pub struct Thumbnail(pub Arc<[u8]>);

impl std::fmt::Debug for Thumbnail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Thumbnail({} bytes)", self.0.len())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub app_id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub playback: Playback,
    pub controls: Controls,
    /// `None` if the session does not report it.
    pub shuffle: Option<bool>,
    pub repeat: Option<RepeatMode>,
    pub clock: PositionClock,
    pub thumbnail: Option<Thumbnail>,
}

impl Session {
    /// Title for display; ads may come without one (SPEC §5.6).
    pub fn display_title(&self) -> &str {
        if self.title.trim().is_empty() {
            "Spotify"
        } else {
            &self.title
        }
    }

    /// Identifies the track for change detection (lyrics, cover).
    pub fn track_key(&self) -> (&str, &str, &str) {
        (&self.title, &self.artist, &self.album)
    }
}

/// Current media state; `session` is `None` when Spotify is not running.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MediaSnapshot {
    pub session: Option<Session>,
}

/// SPEC §6.1: only sessions whose app id contains "spotify" (any case).
pub fn is_spotify(app_id: &str) -> bool {
    app_id.to_lowercase().contains("spotify")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spotify_filter() {
        assert!(is_spotify("Spotify.exe"));
        assert!(is_spotify("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify"));
        assert!(!is_spotify("MSEdge"));
        assert!(!is_spotify("chrome.exe"));
    }

    #[test]
    fn repeat_cycle() {
        assert_eq!(RepeatMode::Off.next(), RepeatMode::All);
        assert_eq!(RepeatMode::All.next(), RepeatMode::One);
        assert_eq!(RepeatMode::One.next(), RepeatMode::Off);
    }
}
