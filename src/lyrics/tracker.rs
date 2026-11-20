//! Lyrics state of the current track (SPEC §5.6): when to request, which
//! answers are still relevant, and which player layout to show while loading.

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{LyricLine, Lyrics};

/// Wait briefly after a track change so the timeline (duration) of the new
/// track has arrived before asking LRCLIB.
const REQUEST_DELAY: Duration = Duration::from_millis(400);
/// While loading, the previous layout stays at most this long (SPEC §5.6).
const LOADING_HOLD: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone)]
enum Status {
    /// No track yet or lyrics not applicable (ads, disabled).
    Missing,
    Pending {
        since: Instant,
        requested: bool,
    },
    Found(Arc<Lyrics>),
}

#[derive(Debug)]
pub struct LyricsTracker {
    next_id: u64,
    current_id: u64,
    status: Status,
    split: bool,
}

impl Default for LyricsTracker {
    fn default() -> Self {
        Self {
            next_id: 1,
            current_id: 0,
            status: Status::Missing,
            split: false,
        }
    }
}

impl LyricsTracker {
    /// New track. `eligible` is false for ads or when lyrics are disabled.
    pub fn track_changed(&mut self, now: Instant, eligible: bool) {
        self.current_id = self.next_id;
        self.next_id += 1;
        self.status = if eligible {
            Status::Pending {
                since: now,
                requested: false,
            }
        } else {
            Status::Missing
        };
    }

    /// Returns the request id once it is time to ask the network.
    pub fn due_request(&mut self, now: Instant) -> Option<u64> {
        match &mut self.status {
            Status::Pending { since, requested }
                if !*requested && now - *since >= REQUEST_DELAY =>
            {
                *requested = true;
                Some(self.current_id)
            }
            _ => None,
        }
    }

    /// Network answer; ignored if it belongs to an older track.
    pub fn on_result(&mut self, request_id: u64, lines: Option<Vec<LyricLine>>) {
        if request_id != self.current_id {
            return;
        }
        self.status = match lines {
            Some(lines) if !lines.is_empty() => Status::Found(Arc::new(Lyrics {
                id: request_id,
                lines,
            })),
            _ => Status::Missing,
        };
    }

    /// Sets lyrics directly (demo mode).
    pub fn set_found(&mut self, lines: Vec<LyricLine>) {
        self.on_result(self.current_id, Some(lines));
    }

    pub fn lyrics(&self) -> Option<&Arc<Lyrics>> {
        match &self.status {
            Status::Found(l) => Some(l),
            _ => None,
        }
    }

    /// Target of the player layout: split while lyrics are shown; while
    /// loading (max. 1.5 s) the previous layout stays.
    pub fn split_target(&mut self, now: Instant, show_in_player: bool) -> bool {
        self.split = match &self.status {
            Status::Found(_) => show_in_player,
            Status::Pending { since, .. } if now - *since < LOADING_HOLD => self.split,
            _ => false,
        };
        self.split
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines() -> Vec<LyricLine> {
        vec![LyricLine {
            time: Duration::ZERO,
            text: "x".into(),
        }]
    }

    #[test]
    fn requests_once_after_delay() {
        let t0 = Instant::now();
        let mut l = LyricsTracker::default();
        l.track_changed(t0, true);
        assert_eq!(l.due_request(t0 + Duration::from_millis(100)), None);
        let id = l.due_request(t0 + Duration::from_millis(450));
        assert!(id.is_some());
        assert_eq!(l.due_request(t0 + Duration::from_millis(900)), None);
    }

    #[test]
    fn ineligible_tracks_never_request() {
        let t0 = Instant::now();
        let mut l = LyricsTracker::default();
        l.track_changed(t0, false);
        assert_eq!(l.due_request(t0 + Duration::from_secs(5)), None);
        assert!(!l.split_target(t0, true));
    }

    #[test]
    fn stale_results_are_ignored() {
        let t0 = Instant::now();
        let mut l = LyricsTracker::default();
        l.track_changed(t0, true);
        let old = l.due_request(t0 + Duration::from_secs(1)).unwrap();
        l.track_changed(t0 + Duration::from_secs(2), true);
        l.on_result(old, Some(lines()));
        assert!(l.lyrics().is_none());
        let new = l.due_request(t0 + Duration::from_secs(3)).unwrap();
        l.on_result(new, Some(lines()));
        assert_eq!(l.lyrics().map(|x| x.id), Some(new));
    }

    #[test]
    fn layout_holds_while_loading_then_centers() {
        let t0 = Instant::now();
        let mut l = LyricsTracker::default();
        l.track_changed(t0, true);
        let id = l.due_request(t0 + Duration::from_secs(1)).unwrap();
        l.on_result(id, Some(lines()));
        assert!(l.split_target(t0, true));
        assert!(!l.split_target(t0, false));
        assert!(l.split_target(t0, true));

        // Next track: split layout stays while loading ...
        let t1 = t0 + Duration::from_secs(10);
        l.track_changed(t1, true);
        assert!(l.split_target(t1 + Duration::from_millis(500), true));
        // ... and goes back to centered after 1.5 s without lyrics.
        assert!(!l.split_target(t1 + Duration::from_millis(1600), true));
        // A miss switches immediately.
        l.track_changed(t1 + Duration::from_secs(5), true);
        let id = l.due_request(t1 + Duration::from_secs(6)).unwrap();
        l.on_result(id, None);
        assert!(!l.split_target(t1 + Duration::from_secs(6), true));
    }
}
