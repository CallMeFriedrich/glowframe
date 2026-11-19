//! Glow frame animation state (SPEC §5.5, §5.6): rotation phase with smooth
//! speed changes, breathing in pause/standby, beat envelope and OKLab palette
//! transitions. Produces the shader parameters per frame.

use std::time::{Duration, Instant};

use crate::color::Oklab;
use crate::render::GlowParams;
use crate::settings::model::{FALLBACK_PALETTE, FrameSettings, PalettePreset, Rgb};

/// Rotation speed at 100 %: 40 % equals one lap per 12 s (SPEC §7).
const LAPS_PER_SEC_AT_100: f64 = 1.0 / 12.0 / 0.4;
const PALETTE_TRANSITION: Duration = Duration::from_millis(1500);
/// Time constant for speed/breathing/beat blending (~1 s to settle).
const SMOOTHING_TAU: f32 = 0.33;
const BASE_LEVEL: f32 = 0.15;
const ARC_LEN: f32 = 0.6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameState {
    Playing,
    Paused,
    /// No Spotify session.
    Standby,
}

impl FrameState {
    fn speed_factor(self) -> f32 {
        match self {
            Self::Playing => 1.0,
            Self::Paused => 0.1,
            Self::Standby => 0.3,
        }
    }
}

/// Beat envelope: attack ~10 ms, decay ~180 ms (SPEC §5.5).
pub fn beat_envelope(since_beat: f32) -> f32 {
    const ATTACK: f32 = 0.010;
    const DECAY_TAU: f32 = 0.055; // ~3 tau = 170 ms
    if since_beat < 0.0 {
        0.0
    } else if since_beat < ATTACK {
        since_beat / ATTACK
    } else {
        (-(since_beat - ATTACK) / DECAY_TAU).exp()
    }
}

pub fn preset_colors(preset: PalettePreset, custom: [Rgb; 3]) -> [Rgb; 3] {
    match preset {
        PalettePreset::NeonPink => FALLBACK_PALETTE,
        PalettePreset::Ocean => [
            Rgb(0x00, 0xC8, 0xFF),
            Rgb(0x1E, 0x5B, 0xFF),
            Rgb(0x00, 0xF5, 0xC4),
        ],
        PalettePreset::Sunset => [
            Rgb(0xFF, 0x5E, 0x3A),
            Rgb(0xFF, 0x2E, 0x93),
            Rgb(0xFF, 0xB1, 0x3B),
        ],
        PalettePreset::Mono => [
            Rgb(0xFF, 0xFF, 0xFF),
            Rgb(0xC4, 0xCC, 0xDA),
            Rgb(0x8A, 0x98, 0xB0),
        ],
        PalettePreset::Custom => custom,
    }
}

/// Spreads 2-3 colors onto the three gradient stops.
pub fn to_stops(colors: &[Oklab]) -> [Oklab; 3] {
    match colors {
        [] => FALLBACK_PALETTE.map(Oklab::from_rgb),
        [a] => [*a, *a, *a],
        [a, b] => [*a, a.lerp(*b, 0.5), *b],
        [a, b, c, ..] => [*a, *b, *c],
    }
}

pub struct FrameAnimator {
    start: Instant,
    last: Option<Instant>,
    phase: f64,
    speed: f32,
    breath: f32,
    beat_weight: f32,
    from: [Oklab; 3],
    to: [Oklab; 3],
    transition_start: Option<Instant>,
}

/// Per-frame inputs from the app.
pub struct FrameInputs<'a> {
    pub state: FrameState,
    pub settings: &'a FrameSettings,
    /// Time since the last beat, if any (seconds).
    pub since_beat: Option<f32>,
    /// Smoothed energy 0..1.
    pub energy: f32,
}

impl FrameAnimator {
    pub fn new(now: Instant) -> Self {
        let stops = to_stops(&[]);
        Self {
            start: now,
            last: None,
            phase: 0.0,
            speed: 1.0,
            breath: 0.0,
            beat_weight: 1.0,
            from: stops,
            to: stops,
            transition_start: None,
        }
    }

    /// Starts an OKLab transition (~1.5 s) from the current colors.
    pub fn set_palette(&mut self, colors: &[Oklab], now: Instant) {
        let target = to_stops(colors);
        if target == self.to {
            return;
        }
        self.from = self.current_stops(now);
        self.to = target;
        self.transition_start = Some(now);
    }

    /// Sets the palette without transition (e.g. on mode start).
    pub fn jump_palette(&mut self, colors: &[Oklab]) {
        self.to = to_stops(colors);
        self.from = self.to;
        self.transition_start = None;
    }

    pub fn current_stops(&self, now: Instant) -> [Oklab; 3] {
        let Some(t0) = self.transition_start else {
            return self.to;
        };
        let t = (now.saturating_duration_since(t0).as_secs_f32()
            / PALETTE_TRANSITION.as_secs_f32())
        .clamp(0.0, 1.0);
        if t >= 1.0 {
            return self.to;
        }
        let e = t * t * (3.0 - 2.0 * t); // smoothstep
        [0, 1, 2].map(|i| self.from[i].lerp(self.to[i], e))
    }

    /// Advances the animation to `now` and returns the shader parameters for
    /// a surface of `width x height` pixels.
    pub fn update(
        &mut self,
        now: Instant,
        input: &FrameInputs,
        width: u32,
        height: u32,
    ) -> GlowParams {
        let dt = self
            .last
            .map_or(0.0, |l| now.saturating_duration_since(l).as_secs_f32())
            .min(0.1);
        self.last = Some(now);
        let k = 1.0 - (-dt / SMOOTHING_TAU).exp();

        let s = input.settings;
        let playing = input.state == FrameState::Playing;
        self.speed += (input.state.speed_factor() - self.speed) * k;
        self.breath += (if playing { 0.0 } else { 1.0 } - self.breath) * k;
        self.beat_weight += (if playing { 1.0 } else { 0.0 } - self.beat_weight) * k;

        let laps_per_sec = s.speed as f64 / 100.0 * LAPS_PER_SEC_AT_100;
        self.phase = (self.phase + dt as f64 * laps_per_sec * self.speed as f64).fract();

        let time = now.saturating_duration_since(self.start).as_secs_f32();
        // Breathing: ~0.2 Hz between 20 and 40 % brightness.
        let breathing = 0.3 + 0.1 * (std::f32::consts::TAU * 0.2 * time).sin();
        let level = 1.0 + (breathing - 1.0) * self.breath;

        let beat_amount = s.beat as f32 / 100.0 * self.beat_weight;
        let beat = input.since_beat.map_or(0.0, beat_envelope) * beat_amount;
        let energy = 0.5 + (input.energy.clamp(0.0, 1.0) - 0.5) * beat_amount;

        let h = height as f32;
        let stops = self.current_stops(now);
        let lin = |c: Oklab| {
            let l = c.clip_to_gamut().to_linear();
            [l.r.max(0.0), l.g.max(0.0), l.b.max(0.0), 1.0]
        };

        GlowParams {
            size: [width as f32, h],
            time,
            phase: self.phase as f32,
            c0: lin(stops[0]),
            c1: lin(stops[1]),
            c2: lin(stops[2]),
            brightness: s.brightness as f32 / 80.0 * level,
            width: h * (0.004 + 0.036 * s.width as f32 / 100.0),
            beat,
            energy,
            corner: h * 0.012,
            base_level: BASE_LEVEL,
            arc_len: ARC_LEN,
            dither_seed: (time * 7.13).fract(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(state: FrameState, s: &FrameSettings) -> FrameInputs<'_> {
        FrameInputs {
            state,
            settings: s,
            since_beat: None,
            energy: 0.5,
        }
    }

    #[test]
    fn envelope_shape() {
        assert_eq!(beat_envelope(-1.0), 0.0);
        assert!((beat_envelope(0.005) - 0.5).abs() < 1e-6);
        assert!((beat_envelope(0.010) - 1.0).abs() < 1e-6);
        assert!(beat_envelope(0.19) < 0.1);
    }

    #[test]
    fn rotation_speed_matches_spec() {
        let s = FrameSettings::default(); // speed 40 % = 1 lap / 12 s
        let t0 = Instant::now();
        let mut a = FrameAnimator::new(t0);
        a.update(t0, &inputs(FrameState::Playing, &s), 100, 100);
        for i in 1..=60 {
            a.update(
                t0 + Duration::from_millis(100 * i),
                &inputs(FrameState::Playing, &s),
                100,
                100,
            );
        }
        // 6 s -> half a lap.
        assert!((a.phase - 0.5).abs() < 1e-3, "phase {}", a.phase);
    }

    #[test]
    fn pause_slows_down_and_dims() {
        let s = FrameSettings::default();
        let t0 = Instant::now();
        let mut a = FrameAnimator::new(t0);
        let mut p = GlowParams::default();
        for i in 0..=30 {
            p = a.update(
                t0 + Duration::from_millis(100 * i),
                &inputs(FrameState::Paused, &s),
                100,
                100,
            );
        }
        assert!((a.speed - 0.1).abs() < 0.01);
        assert!(p.brightness < 0.45 && p.brightness > 0.15);
    }

    #[test]
    fn beat_off_when_paused_and_scaled_by_setting() {
        let mut s = FrameSettings::default();
        let t0 = Instant::now();
        let mut a = FrameAnimator::new(t0);
        let mut beat_in = inputs(FrameState::Playing, &s);
        beat_in.since_beat = Some(0.01);
        assert!((a.update(t0, &beat_in, 10, 10).beat - 0.6).abs() < 1e-3);
        s.beat = 0;
        let mut zero = inputs(FrameState::Playing, &s);
        zero.since_beat = Some(0.01);
        assert_eq!(a.update(t0, &zero, 10, 10).beat, 0.0);
    }

    #[test]
    fn palette_transition_interpolates() {
        let t0 = Instant::now();
        let mut a = FrameAnimator::new(t0);
        let red = Oklab::from_rgb(Rgb(255, 0, 0));
        let blue = Oklab::from_rgb(Rgb(0, 0, 255));
        a.jump_palette(&[red, red]);
        a.set_palette(&[blue, blue], t0);
        let mid = a.current_stops(t0 + Duration::from_millis(750))[0];
        assert!(mid.dist(red) > 0.05 && mid.dist(blue) > 0.05);
        assert_eq!(a.current_stops(t0 + Duration::from_secs(2))[0], blue);
    }
}
