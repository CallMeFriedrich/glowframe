//! Spring physics for all UI and frame transitions (SPEC §5.2 v1.1).
//!
//! Parameterized like iOS/SwiftUI springs: `response` is the period of the
//! undamped oscillation in seconds (how quick it feels), `damping` the damping
//! ratio (1.0 = critically damped, no overshoot; < 1 = slight overshoot).

use std::time::Instant;

/// Integration step; small and fixed so the motion is identical at any
/// frame rate.
const STEP: f32 = 1.0 / 480.0;

#[derive(Debug, Clone, Copy)]
pub struct Spring {
    value: f32,
    velocity: f32,
    stiffness: f32,
    damping: f32,
    last: Option<Instant>,
}

impl Spring {
    pub fn new(value: f32, response: f32, damping_ratio: f32) -> Self {
        let omega = std::f32::consts::TAU / response.max(0.01);
        Self {
            value,
            velocity: 0.0,
            stiffness: omega * omega,
            damping: 2.0 * damping_ratio * omega,
            last: None,
        }
    }

    /// Smooth UI motion with a hint of overshoot (scales, positions).
    pub fn ui(value: f32) -> Self {
        Self::new(value, 0.5, 0.86)
    }

    /// No overshoot (opacities, speeds, brightness).
    pub fn smooth(value: f32, response: f32) -> Self {
        Self::new(value, response, 1.0)
    }

    /// Advances towards `target` and returns the current value.
    pub fn update(&mut self, now: Instant, target: f32) -> f32 {
        let dt = self
            .last
            .map_or(0.0, |l| now.saturating_duration_since(l).as_secs_f32())
            .min(0.1);
        self.last = Some(now);
        let mut left = dt;
        while left > 0.0 {
            let h = left.min(STEP);
            let accel = self.stiffness * (target - self.value) - self.damping * self.velocity;
            self.velocity += accel * h;
            self.value += self.velocity * h;
            left -= h;
        }
        if (target - self.value).abs() < 1e-4 && self.velocity.abs() < 1e-3 {
            self.value = target;
            self.velocity = 0.0;
        }
        self.value
    }

    /// Jumps to `value` without motion.
    pub fn set(&mut self, value: f32) {
        self.value = value;
        self.velocity = 0.0;
    }

    pub fn value(&self) -> f32 {
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn run(mut s: Spring, target: f32, ms: u64) -> (f32, f32) {
        let t0 = Instant::now();
        s.update(t0, target);
        let mut max: f32 = 0.0;
        for i in 1..=ms / 8 {
            let v = s.update(t0 + Duration::from_millis(i * 8), target);
            max = max.max(v);
        }
        (s.value(), max)
    }

    #[test]
    fn critically_damped_never_overshoots() {
        let (end, max) = run(Spring::smooth(0.0, 0.5), 1.0, 3000);
        assert_eq!(end, 1.0);
        assert!(max <= 1.0 + 1e-6);
    }

    #[test]
    fn ui_spring_overshoots_slightly_and_settles() {
        let (end, max) = run(Spring::ui(0.0), 1.0, 3000);
        assert_eq!(end, 1.0);
        assert!(max > 1.0 && max < 1.03, "overshoot {max}");
    }

    #[test]
    fn response_sets_the_pace() {
        let t0 = Instant::now();
        let (mut fast, mut slow) = (Spring::smooth(0.0, 0.3), Spring::smooth(0.0, 1.0));
        fast.update(t0, 1.0);
        slow.update(t0, 1.0);
        let t = t0 + Duration::from_millis(250);
        assert!(fast.update(t, 1.0) > slow.update(t, 1.0));
    }

    #[test]
    fn independent_of_frame_rate() {
        let t0 = Instant::now();
        let (mut a, mut b) = (Spring::ui(0.0), Spring::ui(0.0));
        a.update(t0, 1.0);
        b.update(t0, 1.0);
        for i in 1..=30 {
            a.update(t0 + Duration::from_micros(i * 16_667), 1.0); // 60 Hz
        }
        for i in 1..=72 {
            b.update(t0 + Duration::from_micros(i * 6_944), 1.0); // 144 Hz
        }
        assert!((a.value() - b.value()).abs() < 0.01);
    }
}
