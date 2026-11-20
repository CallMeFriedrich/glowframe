//! Synced lyrics from LRCLIB (SPEC §6.4).

pub mod lrc;
pub mod lrclib;
pub mod tracker;

pub use lrc::{LyricLine, state_at};
pub use tracker::LyricsTracker;

/// Parsed lyrics of one track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lyrics {
    /// Unique id (bitmap/text layout caches are keyed by it).
    pub id: u64,
    pub lines: Vec<LyricLine>,
}
