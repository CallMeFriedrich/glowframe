//! Record view (SPEC §5.4d): turntable spin with run-up and run-out.

use std::f32::consts::TAU;

/// 33 1/3 rpm.
pub const REVS_PER_SEC: f32 = 100.0 / 3.0 / 60.0;
/// Time constant of the run-up on play (reaches speed in well under 1 s).
const RUN_UP: f32 = 0.18;
/// Time constant of the run-out on pause (coasts for a few seconds).
const RUN_OUT: f32 = 0.85;
/// Below this speed (rev/s) the platter counts as stopped.
const STOPPED: f32 = 0.002;

#[derive(Debug, Clone, Copy, Default)]
pub struct Turntable {
    /// Label rotation in radians, 0..TAU.
    angle: f32,
    /// Current speed in revolutions per second.
    speed: f32,
}

impl Turntable {
    /// Advances by `dt` seconds; the speed approaches 33 1/3 rpm while
    /// playing and 0 otherwise (exponential, like a belt-driven platter).
    pub fn update(&mut self, dt: f32, playing: bool) -> f32 {
        let dt = dt.clamp(0.0, 0.25);
        let (target, tau) = if playing {
            (REVS_PER_SEC, RUN_UP)
        } else {
            (0.0, RUN_OUT)
        };
        let k = 1.0 - (-dt / tau).exp();
        let old = self.speed;
        self.speed += (target - self.speed) * k;
        if !playing && self.speed < STOPPED {
            self.speed = 0.0;
        }
        // Trapezoid: average speed over the step.
        self.angle = (self.angle + (old + self.speed) * 0.5 * dt * TAU).rem_euclid(TAU);
        self.angle
    }

    #[cfg(test)]
    pub fn angle(&self) -> f32 {
        self.angle
    }

    #[cfg(test)]
    pub fn speed(&self) -> f32 {
        self.speed
    }
}

/// Per-song coin flip: record instead of cover (SPEC §5.4d).
pub fn roll(random: u64) -> bool {
    random >> 63 == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(t: &mut Turntable, secs: f32, playing: bool) {
        let steps = (secs * 120.0) as usize;
        for _ in 0..steps {
            t.update(1.0 / 120.0, playing);
        }
    }

    #[test]
    fn reaches_speed_quickly() {
        let mut t = Turntable::default();
        run(&mut t, 0.8, true);
        assert!((t.speed() - REVS_PER_SEC).abs() < 0.02 * REVS_PER_SEC);
    }

    #[test]
    fn coasts_slowly_then_stops() {
        let mut t = Turntable::default();
        run(&mut t, 2.0, true);
        run(&mut t, 1.0, false);
        // Still turning noticeably after one second.
        assert!(t.speed() > 0.2 * REVS_PER_SEC);
        run(&mut t, 8.0, false);
        assert_eq!(t.speed(), 0.0);
        let a = t.angle();
        run(&mut t, 1.0, false);
        assert_eq!(t.angle(), a);
    }

    #[test]
    fn one_revolution_takes_1_8_s() {
        let mut t = Turntable::default();
        run(&mut t, 3.0, true);
        let a0 = t.angle();
        run(&mut t, 0.45, true);
        let quarter = (t.angle() - a0).rem_euclid(TAU);
        assert!((quarter - TAU / 4.0).abs() < 0.02, "{quarter}");
    }

    #[test]
    fn angle_stays_wrapped() {
        let mut t = Turntable::default();
        run(&mut t, 30.0, true);
        assert!((0.0..TAU).contains(&t.angle()));
    }
}
