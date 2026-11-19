//! Easing and small animation helpers.

use std::time::{Duration, Instant};

pub fn ease_in_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// Value moving linearly towards 0 or 1 over `duration`, read eased.
#[derive(Debug, Clone, Copy)]
pub struct Toggle {
    value: f32,
    duration: Duration,
    last: Option<Instant>,
}

impl Toggle {
    pub fn new(on: bool, duration: Duration) -> Self {
        Self {
            value: if on { 1.0 } else { 0.0 },
            duration,
            last: None,
        }
    }

    /// Advances towards `on` and returns the eased value.
    pub fn update(&mut self, now: Instant, on: bool) -> f32 {
        let dt = self
            .last
            .map_or(0.0, |l| now.saturating_duration_since(l).as_secs_f32());
        self.last = Some(now);
        let step = dt / self.duration.as_secs_f32().max(1e-3);
        let target = if on { 1.0 } else { 0.0 };
        self.value = if self.value < target {
            (self.value + step).min(target)
        } else {
            (self.value - step).max(target)
        };
        ease_in_out_cubic(self.value)
    }

    pub fn raw(&self) -> f32 {
        self.value
    }
}

/// Exponential approach (time constant `tau`), for state blends.
#[derive(Debug, Clone, Copy)]
pub struct Smooth {
    value: f32,
    tau: f32,
    last: Option<Instant>,
}

impl Smooth {
    pub fn new(value: f32, tau: f32) -> Self {
        Self {
            value,
            tau,
            last: None,
        }
    }

    pub fn update(&mut self, now: Instant, target: f32) -> f32 {
        let dt = self
            .last
            .map_or(0.0, |l| now.saturating_duration_since(l).as_secs_f32())
            .min(0.25);
        self.last = Some(now);
        let k = 1.0 - (-dt / self.tau.max(1e-3)).exp();
        self.value += (target - self.value) * k;
        if (target - self.value).abs() < 1e-3 {
            self.value = target;
        }
        self.value
    }

    pub fn set(&mut self, value: f32) {
        self.value = value;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing_endpoints() {
        let f = ease_in_out_cubic;
        assert_eq!(f(0.0), 0.0);
        assert!((f(1.0) - 1.0).abs() < 1e-6);
        assert!((f(0.5) - 0.5).abs() < 1e-6);
        assert!(f(0.25) < 0.25 && f(0.75) > 0.75);
    }

    #[test]
    fn toggle_takes_duration() {
        let t0 = Instant::now();
        let mut t = Toggle::new(false, Duration::from_millis(400));
        t.update(t0, true);
        assert!(t.update(t0 + Duration::from_millis(200), true) > 0.3);
        assert!(t.raw() < 1.0);
        assert_eq!(t.update(t0 + Duration::from_millis(450), true), 1.0);
        assert_eq!(t.update(t0 + Duration::from_millis(900), false), 0.0);
    }

    #[test]
    fn smooth_converges() {
        let t0 = Instant::now();
        let mut s = Smooth::new(0.0, 0.2);
        s.update(t0, 1.0);
        let mut v = 0.0;
        for i in 1..=20 {
            v = s.update(t0 + Duration::from_millis(100 * i), 1.0);
        }
        assert_eq!(v, 1.0);
    }
}
