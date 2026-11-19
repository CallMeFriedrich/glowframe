//! Beat and energy analysis (SPEC §6.2): mono mix, FFT 1024 / hop 512,
//! spectral flux in the bass band (~40-150 Hz) with an adaptive threshold
//! (median of the last ~1 s x factor), minimum beat distance ~250 ms, and a
//! smoothed, self-normalizing RMS energy.

use std::collections::VecDeque;
use std::sync::Arc;

use realfft::{RealFftPlanner, RealToComplex};

pub const FFT_SIZE: usize = 1024;
pub const HOP: usize = 512;
const BAND_LOW_HZ: f32 = 40.0;
const BAND_HIGH_HZ: f32 = 150.0;
const MIN_BEAT_GAP_S: f32 = 0.25;
const MEDIAN_WINDOW_S: f32 = 1.0;
/// Below this RMS (~ -50 dBFS) the signal counts as silence: no beats.
const SILENCE_RMS: f32 = 0.003;
/// Absolute flux floor so that steady noise never triggers beats.
/// Keeps the log ratio finite in digital silence.
const BAND_EPS: f32 = 1e-6;

/// Result of one analysis hop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HopResult {
    pub beat: bool,
    /// Smoothed, normalized energy 0..1.
    pub energy: f32,
    /// Onset strength and threshold of this hop (diagnostics).
    pub flux: f32,
    pub threshold: f32,
}

pub struct BeatDetector {
    sample_rate: f32,
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    input: Vec<f32>,
    spectrum: Vec<realfft::num_complex::Complex<f32>>,
    scratch: Vec<realfft::num_complex::Complex<f32>>,
    /// Mono samples not yet consumed by a hop.
    pending: VecDeque<f32>,
    /// Last FFT_SIZE samples (analysis frame).
    frame: VecDeque<f32>,
    band: (usize, usize),
    /// Bass band energy of the last hops (reference for the flux).
    recent_band: VecDeque<f32>,
    /// Bass band energy over the last ~1 s.
    band_history: VecDeque<f32>,
    last_raw_energy: f32,
    flux_history: VecDeque<f32>,
    history_len: usize,
    prev_flux: f32,
    hops_since_beat: usize,
    min_gap_hops: usize,
    energy: f32,
    energy_peak: f32,
    /// 0..1 from the "Beat-Reaktion" setting; 0 disables beats.
    sensitivity: f32,
}

impl BeatDetector {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate.max(8000) as f32;
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let window = (0..FFT_SIZE)
            .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / FFT_SIZE as f32).cos())
            .collect();
        let bin_hz = sr / FFT_SIZE as f32;
        let lo = ((BAND_LOW_HZ / bin_hz).round() as usize).max(1);
        let hi = ((BAND_HIGH_HZ / bin_hz).round() as usize).max(lo);
        let hop_s = HOP as f32 / sr;
        Self {
            sample_rate: sr,
            input: fft.make_input_vec(),
            spectrum: fft.make_output_vec(),
            scratch: fft.make_scratch_vec(),
            fft,
            window,
            pending: VecDeque::with_capacity(HOP * 4),
            frame: VecDeque::from(vec![0.0; FFT_SIZE]),
            band: (lo, hi),
            recent_band: VecDeque::from(vec![0.0; 3]),
            band_history: VecDeque::new(),
            last_raw_energy: 0.0,
            flux_history: VecDeque::new(),
            history_len: ((MEDIAN_WINDOW_S / hop_s).round() as usize).max(8),
            prev_flux: 0.0,
            hops_since_beat: usize::MAX / 2,
            min_gap_hops: (MIN_BEAT_GAP_S / hop_s).ceil() as usize,
            energy: 0.0,
            energy_peak: 0.001,
            sensitivity: 0.6,
        }
    }

    pub fn set_sensitivity(&mut self, s: f32) {
        self.sensitivity = s.clamp(0.0, 1.0);
    }

    /// Spread factor of the threshold: more reaction = lower factor.
    fn threshold_factor(&self) -> f32 {
        6.0 - 3.0 * self.sensitivity
    }

    /// Minimum log rise of the bass energy for a beat: ~5x at 0 %, ~3x at
    /// the default 60 %, ~2.2x at 100 %.
    fn flux_floor(&self) -> f32 {
        1.6 - 0.8 * self.sensitivity
    }

    /// Feeds mono samples; calls `on_hop` for every completed hop.
    pub fn push(&mut self, mono: &[f32], mut on_hop: impl FnMut(HopResult)) {
        self.pending.extend(mono.iter().copied());
        while self.pending.len() >= HOP {
            for _ in 0..HOP {
                let s = self.pending.pop_front().unwrap_or(0.0);
                self.frame.pop_front();
                self.frame.push_back(s);
            }
            on_hop(self.hop());
        }
    }

    fn hop(&mut self) -> HopResult {
        // RMS of the newest hop.
        let n = self.frame.len();
        let rms = (self.frame.range(n - HOP..).map(|s| s * s).sum::<f32>() / HOP as f32).sqrt();

        for ((dst, s), w) in self.input.iter_mut().zip(&self.frame).zip(&self.window) {
            *dst = s * w;
        }
        let _ =
            self.fft
                .process_with_scratch(&mut self.input, &mut self.spectrum, &mut self.scratch);

        // Spectral flux of the bass band: positive log change of the band
        // energy against the mean of the previous hops. Summing the bins
        // before the log keeps noise fluctuations small.
        let raw_energy: f32 = (self.band.0..=self.band.1)
            .map(|i| self.spectrum[i].norm_sqr())
            .sum();
        // Average over two hops: the band has only ~3 bins, so single hops of
        // steady noise fluctuate strongly; kicks span several hops anyway.
        let band_energy = 0.5 * (raw_energy + self.last_raw_energy);
        self.last_raw_energy = raw_energy;
        // Reference: recent mean, but never below the 1 s median, so that a
        // random dip in steady noise does not make the next hop look like a hit.
        let short = self.recent_band.iter().sum::<f32>() / self.recent_band.len().max(1) as f32;
        let (long_median, _) = median_mad(&self.band_history);
        let reference = short.max(long_median);
        self.band_history.push_back(band_energy);
        if self.band_history.len() > self.history_len {
            self.band_history.pop_front();
        }
        let flux = ((band_energy + BAND_EPS) / (reference + BAND_EPS))
            .ln()
            .max(0.0);
        self.recent_band.pop_front();
        self.recent_band.push_back(band_energy);

        let (median, mad) = median_mad(&self.flux_history);
        self.flux_history.push_back(flux);
        if self.flux_history.len() > self.history_len {
            self.flux_history.pop_front();
        }
        self.hops_since_beat = self.hops_since_beat.saturating_add(1);

        // Adaptive threshold: running median plus a multiple of the spread
        // (median absolute deviation) over the last ~1 s.
        let threshold = (median + mad * self.threshold_factor()).max(self.flux_floor());
        let rising = flux > threshold && flux >= self.prev_flux;
        let beat = self.sensitivity > 0.0
            && rms > SILENCE_RMS
            && rising
            && self.flux_history.len() >= self.history_len / 2
            && self.hops_since_beat >= self.min_gap_hops;
        if beat {
            self.hops_since_beat = 0;
        }
        self.prev_flux = flux;

        // Energy: fast attack, slower release, normalized to a decaying peak.
        let hop_s = HOP as f32 / self.sample_rate;
        let tau = if rms > self.energy { 0.05 } else { 0.3 };
        self.energy += (rms - self.energy) * (1.0 - (-hop_s / tau).exp());
        self.energy_peak = (self.energy_peak * (-hop_s / 10.0).exp())
            .max(self.energy)
            .max(0.001);
        HopResult {
            beat,
            energy: (self.energy / self.energy_peak).clamp(0.0, 1.0),
            flux,
            threshold,
        }
    }
}

/// Median and median absolute deviation.
fn median_mad(values: &VecDeque<f32>) -> (f32, f32) {
    if values.is_empty() {
        return (0.0, 0.0);
    }
    let mut v: Vec<f32> = values.iter().copied().collect();
    v.sort_by(f32::total_cmp);
    let median = v[v.len() / 2];
    for x in &mut v {
        *x = (*x - median).abs();
    }
    v.sort_by(f32::total_cmp);
    (median, v[v.len() / 2])
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    /// Deterministic pseudo noise in -1..1.
    fn noise(state: &mut u32) -> f32 {
        *state ^= *state << 13;
        *state ^= *state >> 17;
        *state ^= *state << 5;
        (*state as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    /// Kick drum (decaying 60 Hz sine) at `bpm` plus background noise.
    fn kick_track(seconds: f32, bpm: f32, noise_level: f32) -> Vec<f32> {
        let mut seed = 12345u32;
        let period = 60.0 / bpm;
        (0..(seconds * SR as f32) as usize)
            .map(|i| {
                let t = i as f32 / SR as f32;
                let since = t % period;
                let kick =
                    (std::f32::consts::TAU * 60.0 * since).sin() * (-since / 0.08).exp() * 0.8;
                kick + noise(&mut seed) * noise_level
            })
            .collect()
    }

    fn beat_times(samples: &[f32], sensitivity: f32) -> Vec<f32> {
        let mut d = BeatDetector::new(SR);
        d.set_sensitivity(sensitivity);
        let mut hops = 0usize;
        let mut times = Vec::new();
        for chunk in samples.chunks(480) {
            d.push(chunk, |r| {
                hops += 1;
                if r.beat {
                    times.push(hops as f32 * HOP as f32 / SR as f32);
                }
            });
        }
        times
    }

    #[test]
    fn detects_120_bpm() {
        let times = beat_times(&kick_track(10.0, 120.0, 0.05), 0.6);
        // ~20 beats minus the warm-up of ~0.5 s.
        assert!((17..=21).contains(&times.len()), "{} beats", times.len());
        for pair in times.windows(2) {
            let gap = pair[1] - pair[0];
            assert!((gap - 0.5).abs() < 0.05, "gap {gap}");
        }
    }

    #[test]
    fn detects_kicks_over_bassline() {
        // Steady 90 Hz bass line plus noise underneath the kicks.
        let samples: Vec<f32> = kick_track(10.0, 120.0, 0.08)
            .iter()
            .enumerate()
            .map(|(i, s)| s + 0.25 * (std::f32::consts::TAU * 90.0 * i as f32 / SR as f32).sin())
            .collect();
        let times = beat_times(&samples, 0.6);
        assert!((15..=21).contains(&times.len()), "{} beats", times.len());
        let on_grid = times
            .iter()
            .filter(|t| {
                let phase = *t % 0.5;
                phase < 0.06 || phase > 0.44
            })
            .count();
        assert!(on_grid * 10 >= times.len() * 9, "{times:?}");
    }

    #[test]
    fn respects_minimum_gap() {
        // 300 BPM kicks (0.2 s) must not produce beats closer than 250 ms.
        let times = beat_times(&kick_track(6.0, 300.0, 0.02), 1.0);
        assert!(!times.is_empty());
        assert!(times.windows(2).all(|p| p[1] - p[0] >= 0.249));
    }

    #[test]
    fn no_beats_on_noise_or_silence() {
        let mut seed = 99u32;
        let hiss: Vec<f32> = (0..SR as usize * 5)
            .map(|_| noise(&mut seed) * 0.2)
            .collect();
        let n = beat_times(&hiss, 0.6).len();
        assert!(n <= 2, "{n} beats on noise");
        assert!(beat_times(&vec![0.0; SR as usize * 5], 1.0).is_empty());
        let quiet: Vec<f32> = kick_track(5.0, 120.0, 0.0)
            .iter()
            .map(|s| s * 0.002)
            .collect();
        assert!(beat_times(&quiet, 1.0).is_empty());
    }

    #[test]
    fn zero_sensitivity_disables_beats() {
        assert!(beat_times(&kick_track(5.0, 120.0, 0.05), 0.0).is_empty());
    }

    #[test]
    fn energy_is_normalized() {
        let mut d = BeatDetector::new(SR);
        let mut last = 0.0;
        d.push(&kick_track(3.0, 120.0, 0.1), |r| last = r.energy);
        assert!(last > 0.0 && last <= 1.0);
    }
}
