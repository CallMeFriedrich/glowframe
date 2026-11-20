//! Glow frame animation state (SPEC §5.5, §5.6): a comet travelling clockwise
//! around the screen (start bottom left), spring-driven speed, brightness and
//! breathing, beat envelope and OKLab palette transitions. Produces the shader
//! parameters per frame.

use std::time::{Duration, Instant};

use crate::color::Oklab;
use crate::render::GlowParams;
use crate::settings::model::{FALLBACK_PALETTE, FrameSettings, PalettePreset, Rgb};
use crate::ui::anim::Spring;

/// Rotation speed at 100 %: 40 % equals one lap per 12 s (SPEC §7).
const LAPS_PER_SEC_AT_100: f64 = 1.0 / 12.0 / 0.4;
const PALETTE_TRANSITION: Duration = Duration::from_millis(1500);
/// Faint glow on the rest of the border (SPEC §5.5 v1.1).
const BASE_LEVEL: f32 = 0.05;
/// Comet shape in fractions of the perimeter: bright head (~12 % incl. its
/// soft flanks), soft leading edge, tail fading over ~30 %.
const HEAD_LEN: f32 = 0.05;
const LEAD_LEN: f32 = 0.022;
const TAIL_LEN: f32 = 0.32;

/// Perimeter coordinate (clockwise from top center, as in glow.hlsl) of the
/// bottom-left corner, where the comet starts.
pub fn start_phase(width: f32, height: f32) -> f32 {
    let (w, h) = (width.max(1.0), height.max(1.0));
    // top-center -> top-right (w/2) -> right edge (h) -> bottom edge (w)
    ((0.5 * w + h + w) / (2.0 * (w + h))).fract()
}

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
    /// Head position on the perimeter; `None` until the size is known.
    phase: Option<f64>,
    /// Speed factor (playing 1, paused 0.1, standby 0.3); starts at rest so
    /// the comet launches from the bottom-left corner.
    speed: Spring,
    breath: Spring,
    beat_weight: Spring,
    /// Fade-in of the whole frame when the mode starts.
    intro: Spring,
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
            phase: None,
            // Critically damped: no oscillation in speed or brightness. The
            // pause slow-down takes ~1 s (SPEC §5.6).
            speed: Spring::smooth(0.0, 1.6),
            breath: Spring::smooth(0.0, 1.1),
            beat_weight: Spring::smooth(1.0, 0.5),
            intro: Spring::smooth(0.0, 1.2),
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

        let s = input.settings;
        let playing = input.state == FrameState::Playing;
        let speed = self.speed.update(now, input.state.speed_factor()).max(0.0);
        let breath = self
            .breath
            .update(now, if playing { 0.0 } else { 1.0 })
            .clamp(0.0, 1.0);
        let beat_weight = self
            .beat_weight
            .update(now, if playing { 1.0 } else { 0.0 })
            .clamp(0.0, 1.0);
        let intro = self.intro.update(now, 1.0).clamp(0.0, 1.0);

        let laps_per_sec = s.speed as f64 / 100.0 * LAPS_PER_SEC_AT_100;
        let phase = self
            .phase
            .unwrap_or_else(|| start_phase(width as f32, height as f32) as f64);
        let phase = (phase + dt as f64 * laps_per_sec * speed as f64).fract();
        self.phase = Some(phase);

        let time = now.saturating_duration_since(self.start).as_secs_f32();
        // Breathing: ~0.2 Hz between 20 and 40 % brightness.
        let breathing = 0.3 + 0.1 * (std::f32::consts::TAU * 0.2 * time).sin();
        let level = (1.0 + (breathing - 1.0) * breath) * intro;

        let beat_amount = s.beat as f32 / 100.0 * beat_weight;
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
            phase: phase as f32,
            c0: lin(stops[0]),
            c1: lin(stops[1]),
            c2: lin(stops[2]),
            brightness: s.brightness as f32 / 80.0 * level,
            width: h * (0.004 + 0.036 * s.width as f32 / 100.0),
            beat,
            energy,
            corner: h * 0.012,
            base_level: BASE_LEVEL,
            tail_len: TAIL_LEN,
            dither_seed: (time * 7.13).fract(),
            head_len: HEAD_LEN,
            lead_len: LEAD_LEN,
            _pad: [0.0; 2],
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

    /// Runs `secs` seconds in 10 ms steps and returns the last parameters.
    fn run(
        a: &mut FrameAnimator,
        t0: Instant,
        from_ms: u64,
        secs: u64,
        i: &FrameInputs,
    ) -> GlowParams {
        let mut p = GlowParams::default();
        for k in 0..=secs * 100 {
            p = a.update(t0 + Duration::from_millis(from_ms + k * 10), i, 1920, 1080);
        }
        p
    }

    #[test]
    fn comet_starts_bottom_left() {
        // Square: corners at 1/8, 3/8, 5/8, 7/8 from top center.
        assert!((start_phase(100.0, 100.0) - 0.625).abs() < 1e-6);
        let s = FrameSettings::default();
        let t0 = Instant::now();
        let mut a = FrameAnimator::new(t0);
        let p = a.update(t0, &inputs(FrameState::Playing, &s), 1920, 1080);
        assert!((p.phase - start_phase(1920.0, 1080.0)).abs() < 1e-6);
        // Launch from rest: the frame also fades in.
        assert!(p.brightness < 0.05);
    }

    #[test]
    fn rotation_speed_matches_spec_after_launch() {
        let s = FrameSettings::default(); // speed 40 % = 1 lap / 12 s
        let t0 = Instant::now();
        let mut a = FrameAnimator::new(t0);
        let play = inputs(FrameState::Playing, &s);
        let p1 = run(&mut a, t0, 0, 10, &play).phase;
        let p2 = run(&mut a, t0, 10_010, 6, &play).phase;
        // 6 s at full speed -> half a lap.
        let d = (p2 - p1).rem_euclid(1.0);
        assert!((d - 0.5).abs() < 0.01, "advance {d}");
    }

    #[test]
    fn pause_slows_down_and_dims() {
        let s = FrameSettings::default();
        let t0 = Instant::now();
        let mut a = FrameAnimator::new(t0);
        let p = run(&mut a, t0, 0, 6, &inputs(FrameState::Paused, &s));
        assert!((a.speed.value() - 0.1).abs() < 0.01);
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
