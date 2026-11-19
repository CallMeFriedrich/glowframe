//! `--demo`: fake Spotify data with a self-made cover and a synthetic
//! 120 BPM beat, for testing without Spotify (CLAUDE.md).

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use crate::media::{
    Command, Controls, MediaSnapshot, Playback, PositionClock, RepeatMode, Session, Thumbnail,
};

const COVER: &[u8] = include_bytes!("../assets/demo/cover.png");
const BEAT_INTERVAL: f32 = 0.5; // 120 BPM
const TRACK_LENGTH: Duration = Duration::from_secs(210);

pub struct Demo {
    start: Instant,
    session: Session,
}

impl Demo {
    pub fn new() -> Self {
        let now = SystemTime::now();
        let mut clock = PositionClock::default();
        clock.sync(Duration::ZERO, now, Some(TRACK_LENGTH));
        clock.set_playing(true, now);
        Self {
            start: Instant::now(),
            session: Session {
                app_id: "Glowframe.Demo".into(),
                title: "Neon Horizon".into(),
                artist: "Glowframe Demo".into(),
                album: "Test Pattern".into(),
                playback: Playback::Playing,
                controls: Controls {
                    play: true,
                    pause: true,
                    next: true,
                    previous: true,
                    shuffle: true,
                    repeat: true,
                },
                shuffle: Some(false),
                repeat: Some(RepeatMode::Off),
                clock,
                thumbnail: Some(Thumbnail(Arc::from(COVER))),
            },
        }
    }

    pub fn snapshot(&self) -> MediaSnapshot {
        MediaSnapshot {
            session: Some(self.session.clone()),
        }
    }

    /// Applies a control command locally. Returns the new snapshot.
    pub fn command(&mut self, cmd: Command) -> MediaSnapshot {
        let s = &mut self.session;
        let now = SystemTime::now();
        match cmd {
            Command::PlayPause => {
                s.playback = if s.playback == Playback::Playing {
                    Playback::Paused
                } else {
                    Playback::Playing
                };
                s.clock.set_playing(s.playback == Playback::Playing, now);
            }
            Command::Next | Command::Previous => {
                s.clock.sync(Duration::ZERO, now, Some(TRACK_LENGTH));
            }
            Command::ToggleShuffle => s.shuffle = Some(!s.shuffle.unwrap_or(false)),
            Command::CycleRepeat => {
                s.repeat = Some(s.repeat.unwrap_or(RepeatMode::Off).next());
            }
        }
        self.snapshot()
    }

    /// Seconds since the last synthetic beat (only while playing).
    pub fn since_beat(&self, now: Instant) -> Option<f32> {
        (self.session.playback == Playback::Playing)
            .then(|| now.saturating_duration_since(self.start).as_secs_f32() % BEAT_INTERVAL)
    }

    /// Synthetic slowly varying energy.
    pub fn energy(&self, now: Instant) -> f32 {
        let t = now.saturating_duration_since(self.start).as_secs_f32();
        0.5 + 0.35 * (t * 0.7).sin()
    }
}
