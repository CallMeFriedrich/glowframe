//! Playback position interpolation (SPEC §6.1).
//!
//! Spotify only reports the timeline sporadically. While playing, the position
//! is `anchor + (now - anchor_time)`; every timeline event re-anchors.

use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PositionClock {
    anchor_pos: Duration,
    anchor_time: SystemTime,
    playing: bool,
    /// Track length, if known (used for clamping).
    duration: Option<Duration>,
}

impl Default for PositionClock {
    fn default() -> Self {
        Self {
            anchor_pos: Duration::ZERO,
            anchor_time: SystemTime::UNIX_EPOCH,
            playing: false,
            duration: None,
        }
    }
}

impl PositionClock {
    /// Timeline event: position as of `last_updated`.
    pub fn sync(
        &mut self,
        position: Duration,
        last_updated: SystemTime,
        duration: Option<Duration>,
    ) {
        self.anchor_pos = position;
        self.anchor_time = last_updated;
        self.duration = duration.filter(|d| !d.is_zero());
    }

    /// Playback state change. Freezes the position on pause and restarts the
    /// interpolation from the frozen position on resume.
    pub fn set_playing(&mut self, playing: bool, now: SystemTime) {
        if playing == self.playing {
            return;
        }
        self.anchor_pos = self.position(now);
        self.anchor_time = now;
        self.playing = playing;
    }

    pub fn position(&self, now: SystemTime) -> Duration {
        let pos = if self.playing {
            // A timestamp in the future (clock skew) counts as "just now".
            self.anchor_pos + now.duration_since(self.anchor_time).unwrap_or_default()
        } else {
            self.anchor_pos
        };
        match self.duration {
            Some(d) => pos.min(d),
            None => pos,
        }
    }

    pub fn duration(&self) -> Option<Duration> {
        self.duration
    }
}

/// Converts a WinRT `DateTime` (100 ns ticks since 1601-01-01 UTC).
pub fn system_time_from_winrt(ticks: i64) -> SystemTime {
    const UNIX_EPOCH_TICKS: i64 = 116_444_736_000_000_000;
    let since_unix = ticks - UNIX_EPOCH_TICKS;
    if since_unix >= 0 {
        SystemTime::UNIX_EPOCH + Duration::from_nanos(since_unix as u64 * 100)
    } else {
        SystemTime::UNIX_EPOCH
    }
}

/// Converts a WinRT `TimeSpan` (100 ns ticks); negative values become zero.
pub fn duration_from_winrt(ticks: i64) -> Duration {
    Duration::from_nanos(ticks.max(0) as u64 * 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000 + secs)
    }

    #[test]
    fn interpolates_while_playing() {
        let mut c = PositionClock::default();
        c.set_playing(true, t(0));
        c.sync(
            Duration::from_secs(10),
            t(0),
            Some(Duration::from_secs(200)),
        );
        assert_eq!(c.position(t(5)), Duration::from_secs(15));
    }

    #[test]
    fn freezes_on_pause_and_resumes() {
        let mut c = PositionClock::default();
        c.set_playing(true, t(0));
        c.sync(Duration::from_secs(10), t(0), None);
        c.set_playing(false, t(4));
        assert_eq!(c.position(t(100)), Duration::from_secs(14));
        c.set_playing(true, t(100));
        assert_eq!(c.position(t(103)), Duration::from_secs(17));
    }

    #[test]
    fn resync_overrides_drift() {
        let mut c = PositionClock::default();
        c.set_playing(true, t(0));
        c.sync(Duration::from_secs(0), t(0), None);
        c.sync(Duration::from_secs(30), t(20), None);
        assert_eq!(c.position(t(21)), Duration::from_secs(31));
    }

    #[test]
    fn clamps_to_duration_and_future_timestamps() {
        let mut c = PositionClock::default();
        c.set_playing(true, t(0));
        c.sync(
            Duration::from_secs(195),
            t(0),
            Some(Duration::from_secs(200)),
        );
        assert_eq!(c.position(t(60)), Duration::from_secs(200));
        c.sync(
            Duration::from_secs(50),
            t(10),
            Some(Duration::from_secs(200)),
        );
        assert_eq!(c.position(t(5)), Duration::from_secs(50));
    }

    #[test]
    fn winrt_conversions() {
        assert_eq!(
            system_time_from_winrt(116_444_736_000_000_000),
            SystemTime::UNIX_EPOCH
        );
        assert_eq!(
            system_time_from_winrt(116_444_736_000_000_000 + 10_000_000),
            SystemTime::UNIX_EPOCH + Duration::from_secs(1)
        );
        assert_eq!(duration_from_winrt(25_000_000), Duration::from_millis(2500));
        assert_eq!(duration_from_winrt(-5), Duration::ZERO);
    }
}
