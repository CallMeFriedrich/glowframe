//! Audio capture and beat analysis (SPEC §6.2). Runs only while the mode is
//! active, on its own thread; results reach the render thread lock-free via
//! atomics.

pub mod beat;
mod capture;
mod process;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

use beat::BeatDetector;
use capture::{Capture, Source};

/// How often the Spotify process is re-checked while capturing.
const PROCESS_CHECK: Duration = Duration::from_secs(2);
const RETRY_DELAY: Duration = Duration::from_secs(1);

struct Shared {
    epoch: Instant,
    /// Microseconds since `epoch` of the last beat + 1 (0 = none yet).
    last_beat_us: AtomicU64,
    energy: AtomicU32,
    sensitivity: AtomicU32,
    stop: AtomicBool,
}

/// Handle to the audio thread. Dropping it stops the thread.
pub struct AudioAnalyzer {
    shared: Arc<Shared>,
}

impl AudioAnalyzer {
    pub fn start(sensitivity: f32) -> Self {
        let shared = Arc::new(Shared {
            epoch: Instant::now(),
            last_beat_us: AtomicU64::new(0),
            energy: AtomicU32::new(0.5f32.to_bits()),
            sensitivity: AtomicU32::new(sensitivity.to_bits()),
            stop: AtomicBool::new(false),
        });
        let s = shared.clone();
        let spawned = std::thread::Builder::new()
            .name("audio".into())
            .spawn(move || run(&s));
        if let Err(e) = spawned {
            tracing::error!(error = %e, "could not spawn audio thread, no beat");
        }
        Self { shared }
    }

    #[allow(dead_code)] // live update from the settings window (phase 7)
    pub fn set_sensitivity(&self, s: f32) {
        self.shared
            .sensitivity
            .store(s.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    /// Seconds since the last detected beat.
    pub fn since_beat(&self, now: Instant) -> Option<f32> {
        let us = self.shared.last_beat_us.load(Ordering::Relaxed);
        (us > 0).then(|| {
            let beat = self.shared.epoch + Duration::from_micros(us - 1);
            now.saturating_duration_since(beat).as_secs_f32()
        })
    }

    pub fn energy(&self) -> f32 {
        f32::from_bits(self.shared.energy.load(Ordering::Relaxed))
    }
}

impl Drop for AudioAnalyzer {
    fn drop(&mut self) {
        // The thread notices within ~100 ms and exits; we do not block the
        // UI thread by joining.
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

fn run(shared: &Shared) {
    // SAFETY: COM (MTA) for WASAPI on this thread.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let mut process_supported = true;

    while !shared.stop.load(Ordering::Relaxed) {
        let pid = process::spotify_root_pid();
        let capture = match pid {
            Some(pid) if process_supported => match Capture::process(pid) {
                Ok(c) => Ok(c),
                Err(e) => {
                    tracing::warn!(error = %e, "process loopback unavailable, using system loopback");
                    process_supported = false;
                    Capture::system()
                }
            },
            // Without process loopback the system mix is the fallback.
            _ if !process_supported => Capture::system(),
            // Spotify not running: nothing to analyze yet.
            _ => {
                sleep_unless_stopped(shared, RETRY_DELAY);
                continue;
            }
        };
        match capture {
            Ok(c) => {
                tracing::info!(source = ?c.source, rate = c.format.sample_rate, ?pid, "audio capture started");
                if let Err(e) = analyze(shared, &c, pid) {
                    tracing::warn!(error = %e, "audio capture interrupted");
                }
            }
            Err(e) => tracing::warn!(error = %e, "audio capture failed"),
        }
        sleep_unless_stopped(shared, RETRY_DELAY);
    }
    tracing::info!("audio thread stopped");
}

/// Capture loop. Returns `Ok` when stopped or when the Spotify process went
/// away (so the caller re-resolves it).
fn analyze(shared: &Shared, capture: &Capture, pid: Option<u32>) -> windows::core::Result<()> {
    let mut detector = BeatDetector::new(capture.format.sample_rate);
    let mut mono = Vec::with_capacity(4096);
    let mut next_check = Instant::now() + PROCESS_CHECK;
    let mut next_check_pid = next_check;
    let mut beats = 0u32;
    while !shared.stop.load(Ordering::Relaxed) {
        capture.wait();
        mono.clear();
        capture.read(&mut mono)?;
        detector.set_sensitivity(f32::from_bits(shared.sensitivity.load(Ordering::Relaxed)));
        detector.push(&mono, |hop| {
            if hop.beat {
                let us = shared.epoch.elapsed().as_micros() as u64 + 1;
                shared.last_beat_us.store(us, Ordering::Relaxed);
                beats += 1;
            }
            shared.energy.store(hop.energy.to_bits(), Ordering::Relaxed);
        });
        if Instant::now() >= next_check {
            next_check = Instant::now() + PROCESS_CHECK;
            tracing::debug!(
                beats,
                energy = f32::from_bits(shared.energy.load(Ordering::Relaxed)),
                "beat stats (last 2 s)"
            );
            beats = 0;
        }
        if capture.source == Source::Process && next_check_pid <= Instant::now() {
            next_check_pid = Instant::now() + PROCESS_CHECK;
            if process::spotify_root_pid() != pid {
                tracing::info!("Spotify process changed, restarting capture");
                return Ok(());
            }
        }
    }
    Ok(())
}

fn sleep_unless_stopped(shared: &Shared, d: Duration) {
    let end = Instant::now() + d;
    while Instant::now() < end && !shared.stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    /// Manual diagnostic of the full analyzer thread:
    /// `cargo test analyzer_diag -- --ignored --nocapture` with Spotify playing.
    #[test]
    #[ignore]
    fn analyzer_diag() {
        let a = super::AudioAnalyzer::start(0.6);
        for _ in 0..8 {
            std::thread::sleep(std::time::Duration::from_millis(500));
            println!(
                "energy {:.3} since_beat {:?}",
                a.energy(),
                a.since_beat(std::time::Instant::now())
            );
        }
    }
}
