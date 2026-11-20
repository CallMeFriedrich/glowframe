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

    fn next_random(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        x
    }

    fn next_interval(&mut self) -> Duration {
        let x = self.next_random();
        let span = (self.max - self.min).as_millis() as u64;
        self.min + Duration::from_millis(x % (span + 1))
    }

    /// 50:50 choice (upper bits, the low bits of xorshift are weaker).
    fn coin_flip(&mut self) -> IdleVariant {
        if self.next_random() >> 63 == 1 {
            IdleVariant::Focus
        } else {
            IdleVariant::Cover
        }
    }

    /// Leaves the idle view: back to the cover view, schedule restarts.
    pub fn reset(&mut self) {
        self.variant = IdleVariant::Cover;
        self.next_switch = None;
    }

    /// Advances the schedule while the idle view is shown (`active`).
    /// Each time the idle view begins, cover view or lyrics focus view is
    /// picked 50:50; afterwards they alternate every interval.
    pub fn update(&mut self, now: Instant, active: bool) -> IdleVariant {
        if !active {
            self.reset();
            return self.variant;
        }
        match self.next_switch {
            None => {
                self.variant = self.coin_flip();
                self.next_switch = Some(now + self.next_interval());
            }
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
        let first = s.update(t0, true);
        let other = match first {
            IdleVariant::Cover => IdleVariant::Focus,
            IdleVariant::Focus => IdleVariant::Cover,
        };
        let at = s.next_switch().unwrap_or(t0);
        assert_eq!(s.update(at - Duration::from_millis(1), true), first);
        assert_eq!(s.update(at, true), other);
        let at2 = s.next_switch().unwrap_or(at);
        assert!(at2 > at);
        assert_eq!(s.update(at2, true), first);
    }

    #[test]
    fn each_idle_start_is_a_fair_coin_flip() {
        let t0 = Instant::now();
        let mut s = IdleScheduler::new(12345);
        let n = 4000;
        let mut focus = 0;
        for i in 0..n {
            let t = t0 + Duration::from_secs(i);
            if s.update(t, true) == IdleVariant::Focus {
                focus += 1;
            }
            // Mouse moved: idle view left, next rest starts fresh.
            s.update(t, false);
        }
        let share = focus as f64 / n as f64;
        assert!((0.45..=0.55).contains(&share), "focus share {share}");
    }

    #[test]
    fn inactive_falls_back_to_cover_and_restarts() {
        let t0 = Instant::now();
        let mut s = IdleScheduler::new(3);
        s.update(t0, true);
        assert!(s.next_switch().is_some());
        assert_eq!(s.update(t0, false), IdleVariant::Cover);
        assert_eq!(s.next_switch(), None);
    }
}
