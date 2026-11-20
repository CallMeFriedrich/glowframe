//! Alternation of the idle views (SPEC §5.2 v1.2): cover view with lyrics
//! line and lyrics focus view swap every 5 min ± 2 min.

use std::time::{Duration, Instant};

pub const MIN_INTERVAL: Duration = Duration::from_secs(180);
pub const MAX_INTERVAL: Duration = Duration::from_secs(420);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleVariant {
    Cover,
    Focus,
}

/// Short intervals for `--demo`, so the alternation can be watched.
pub const DEMO_INTERVAL: (Duration, Duration) = (Duration::from_secs(10), Duration::from_secs(15));

pub struct IdleScheduler {
    variant: IdleVariant,
    next_switch: Option<Instant>,
    rng: u64,
    min: Duration,
    max: Duration,
}

impl IdleScheduler {
    pub fn new(seed: u64) -> Self {
        Self {
            variant: IdleVariant::Cover,
            next_switch: None,
            // xorshift must not start at zero.
            rng: seed | 1,
            min: MIN_INTERVAL,
            max: MAX_INTERVAL,
        }
    }

    /// Seeded from the wall clock; randomness only needs to vary per run.
    pub fn from_clock() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        Self::new(seed)
    }

    pub fn with_interval(mut self, (min, max): (Duration, Duration)) -> Self {
        self.min = min;
        self.max = max.max(min);
        self
    }

    fn next_interval(&mut self) -> Duration {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        let span = (self.max - self.min).as_millis() as u64;
        self.min + Duration::from_millis(x % (span + 1))
    }

    /// Restarts with the cover view (entering the mode, lyrics lost).
    pub fn reset(&mut self) {
        self.variant = IdleVariant::Cover;
        self.next_switch = None;
    }

    /// Advances the schedule. Only runs while alternation is possible
    /// (`active`: lyrics available and the idle view is shown); otherwise
    /// the cover view is used and the timer restarts later.
    pub fn update(&mut self, now: Instant, active: bool) -> IdleVariant {
        if !active {
            self.reset();
            return self.variant;
        }
        match self.next_switch {
            None => self.next_switch = Some(now + self.next_interval()),
            Some(at) if now >= at => {
                self.variant = match self.variant {
                    IdleVariant::Cover => IdleVariant::Focus,
                    IdleVariant::Focus => IdleVariant::Cover,
                };
                self.next_switch = Some(now + self.next_interval());
            }
            Some(_) => {}
        }
        self.variant
    }

    #[cfg(test)]
    fn next_switch(&self) -> Option<Instant> {
        self.next_switch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intervals_stay_within_five_plus_minus_two_minutes() {
        let mut s = IdleScheduler::new(42);
        let mut seen_min = MAX_INTERVAL;
        let mut seen_max = MIN_INTERVAL;
        for _ in 0..2000 {
            let d = s.next_interval();
            assert!((MIN_INTERVAL..=MAX_INTERVAL).contains(&d));
            seen_min = seen_min.min(d);
            seen_max = seen_max.max(d);
        }
        // Actually random, not constant.
        assert!(seen_max - seen_min > Duration::from_secs(120));
    }

    #[test]
    fn alternates_on_schedule() {
        let t0 = Instant::now();
        let mut s = IdleScheduler::new(7);
        assert_eq!(s.update(t0, true), IdleVariant::Cover);
        let at = s.next_switch().unwrap_or(t0);
        assert_eq!(
            s.update(at - Duration::from_millis(1), true),
            IdleVariant::Cover
        );
        assert_eq!(s.update(at, true), IdleVariant::Focus);
        let at2 = s.next_switch().unwrap_or(at);
        assert!(at2 > at);
        assert_eq!(s.update(at2, true), IdleVariant::Cover);
    }

    #[test]
    fn inactive_falls_back_to_cover_and_restarts() {
        let t0 = Instant::now();
        let mut s = IdleScheduler::new(3);
        s.update(t0, true);
        let at = s.next_switch().unwrap_or(t0);
        assert_eq!(s.update(at, true), IdleVariant::Focus);
        assert_eq!(s.update(at, false), IdleVariant::Cover);
        assert_eq!(s.next_switch(), None);
    }
}
