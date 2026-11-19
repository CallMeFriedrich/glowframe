//! GSMTC worker thread. WinRT events are forwarded into a channel; the worker
//! coalesces them, re-reads the affected session data and publishes snapshots.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, SystemTime};

use windows::Foundation::TypedEventHandler;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as GsmtcSession,
    GlobalSystemMediaTransportControlsSessionManager as GsmtcManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as GsmtcStatus,
};
use windows::Media::MediaPlaybackAutoRepeatMode;
use windows::Storage::Streams::{
    Buffer, DataReader, IRandomAccessStreamReference, InputStreamOptions,
};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};

use super::clock::{duration_from_winrt, system_time_from_winrt};
use super::{
    Command, Controls, MediaSnapshot, Playback, RepeatMode, Session, Thumbnail, is_spotify,
};

/// Thumbnails larger than this are ignored (defensive limit).
const MAX_THUMBNAIL_BYTES: u64 = 16 * 1024 * 1024;
/// Retry interval when the session manager is unavailable.
const MANAGER_RETRY: Duration = Duration::from_secs(5);

enum Msg {
    Sessions,
    Media,
    Playback,
    Timeline,
    Command(Command),
    Shutdown,
}

/// Handle to the media worker. Dropping it stops the worker.
pub struct MediaService {
    tx: Sender<Msg>,
}

impl MediaService {
    pub fn start(on_update: impl Fn(MediaSnapshot) + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let worker_tx = tx.clone();
        let spawned = std::thread::Builder::new()
            .name("gsmtc".into())
            .spawn(move || run(rx, worker_tx, on_update));
        if let Err(e) = spawned {
            tracing::error!(error = %e, "could not spawn gsmtc thread");
        }
        Self { tx }
    }

    pub fn command(&self, command: Command) {
        let _ = self.tx.send(Msg::Command(command));
    }
}

impl Drop for MediaService {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Shutdown);
    }
}

/// The bound Spotify session and its event registrations.
struct Bound {
    session: GsmtcSession,
    app_id: String,
    tokens: [i64; 3],
}

impl Drop for Bound {
    fn drop(&mut self) {
        let [media, playback, timeline] = self.tokens;
        let _ = self.session.RemoveMediaPropertiesChanged(media);
        let _ = self.session.RemovePlaybackInfoChanged(playback);
        let _ = self.session.RemoveTimelinePropertiesChanged(timeline);
    }
}

struct Worker {
    manager: GsmtcManager,
    tx: Sender<Msg>,
    bound: Option<Bound>,
    snapshot: MediaSnapshot,
}

fn run(rx: Receiver<Msg>, tx: Sender<Msg>, on_update: impl Fn(MediaSnapshot)) {
    // SAFETY: initializes WinRT for this thread; an "already initialized"
    // result is fine.
    let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };

    let manager = loop {
        match GsmtcManager::RequestAsync().and_then(|op| op.join()) {
            Ok(m) => break m,
            Err(e) => {
                tracing::warn!(error = %e, "GSMTC manager unavailable, retrying");
                match rx.recv_timeout(MANAGER_RETRY) {
                    Ok(Msg::Shutdown) | Err(RecvTimeoutError::Disconnected) => return,
                    _ => {}
                }
            }
        }
    };

    let sessions_tx = tx.clone();
    let sessions_token = manager.SessionsChanged(&TypedEventHandler::new(move |_, _| {
        let _ = sessions_tx.send(Msg::Sessions);
        Ok(())
    }));
    if let Err(e) = &sessions_token {
        tracing::warn!(error = %e, "SessionsChanged subscription failed");
    }

    let mut w = Worker {
        manager,
        tx,
        bound: None,
        snapshot: MediaSnapshot::default(),
    };
    w.select_session();
    w.publish(&on_update);

    while let Ok(first) = rx.recv() {
        // Coalesce bursts of events into one refresh.
        let (mut sessions, mut media, mut playback, mut timeline) = (false, false, false, false);
        let mut pending = Some(first);
        while let Some(msg) = pending.take().or_else(|| rx.try_recv().ok()) {
            match msg {
                Msg::Sessions => sessions = true,
                Msg::Media => media = true,
                Msg::Playback => playback = true,
                Msg::Timeline => timeline = true,
                Msg::Command(c) => w.execute(c),
                Msg::Shutdown => {
                    w.bound = None;
                    if let Ok(t) = sessions_token {
                        let _ = w.manager.RemoveSessionsChanged(t);
                    }
                    return;
                }
            }
        }
        if sessions {
            w.select_session();
        } else {
            if media {
                w.refresh_media();
            }
            if playback {
                w.refresh_playback();
            }
            if timeline {
                w.refresh_timeline();
            }
        }
        w.publish(&on_update);
    }
}

impl Worker {
    fn publish(&mut self, on_update: &impl Fn(MediaSnapshot)) {
        on_update(self.snapshot.clone());
    }

    /// Finds the Spotify session (preferring a playing one) and binds to it.
    fn select_session(&mut self) {
        let candidates: Vec<(GsmtcSession, String)> = match self.manager.GetSessions() {
            Ok(list) => list
                .into_iter()
                .filter_map(|s| {
                    let id = s.SourceAppUserModelId().ok()?.to_string();
                    is_spotify(&id).then_some((s, id))
                })
                .collect(),
            Err(e) => {
                tracing::warn!(error = %e, "GetSessions failed");
                Vec::new()
            }
        };

        let playing = |s: &GsmtcSession| {
            s.GetPlaybackInfo()
                .and_then(|p| p.PlaybackStatus())
                .is_ok_and(|st| st == GsmtcStatus::Playing)
        };
        let chosen = candidates
            .iter()
            .find(|(s, _)| playing(s))
            .or(candidates.first())
            .cloned();

        let Some((session, app_id)) = chosen else {
            if self.bound.take().is_some() {
                tracing::info!("Spotify session gone");
            }
            self.snapshot.session = None;
            return;
        };

        if self.bound.as_ref().is_some_and(|b| b.app_id == app_id) {
            // Same session; just refresh everything.
            self.refresh_all();
            return;
        }

        tracing::info!(app_id = %app_id, "binding Spotify session");
        self.bound = None;
        match self.subscribe(&session) {
            Ok(tokens) => {
                self.bound = Some(Bound {
                    session,
                    app_id: app_id.clone(),
                    tokens,
                });
            }
            Err(e) => tracing::warn!(error = %e, "session event subscription failed"),
        }
        self.snapshot.session = Some(Session {
            app_id,
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            playback: Playback::Other,
            controls: Controls::default(),
            shuffle: None,
            repeat: None,
            clock: Default::default(),
            thumbnail: None,
        });
        self.refresh_all();
    }

    fn subscribe(&self, s: &GsmtcSession) -> windows::core::Result<[i64; 3]> {
        let (a, b, c) = (self.tx.clone(), self.tx.clone(), self.tx.clone());
        let media = s.MediaPropertiesChanged(&TypedEventHandler::new(move |_, _| {
            let _ = a.send(Msg::Media);
            Ok(())
        }))?;
        let playback = s.PlaybackInfoChanged(&TypedEventHandler::new(move |_, _| {
            let _ = b.send(Msg::Playback);
            Ok(())
        }))?;
        let timeline = s.TimelinePropertiesChanged(&TypedEventHandler::new(move |_, _| {
            let _ = c.send(Msg::Timeline);
            Ok(())
        }))?;
        Ok([media, playback, timeline])
    }

    fn refresh_all(&mut self) {
        self.refresh_media();
        self.refresh_playback();
        self.refresh_timeline();
    }

    fn parts(&mut self) -> Option<(&GsmtcSession, &mut Session)> {
        Some((
            &self.bound.as_ref()?.session,
            self.snapshot.session.as_mut()?,
        ))
    }

    fn refresh_media(&mut self) {
        let Some((gs, s)) = self.parts() else { return };
        let props = match gs.TryGetMediaPropertiesAsync().and_then(|op| op.join()) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, "media properties unavailable");
                return;
            }
        };
        let text = |r: windows::core::Result<windows::core::HSTRING>| {
            r.map(|h| h.to_string()).unwrap_or_default()
        };
        let title = text(props.Title());
        let artist = text(props.Artist());
        let album = text(props.AlbumTitle());
        let track_changed = (title.as_str(), artist.as_str(), album.as_str()) != s.track_key();
        s.title = title;
        s.artist = artist;
        s.album = album;

        let thumb = props
            .Thumbnail()
            .ok()
            .and_then(|r| match read_thumbnail(&r) {
                Ok(bytes) => Some(Thumbnail(Arc::from(bytes))),
                Err(e) => {
                    tracing::debug!(error = %e, "thumbnail unavailable");
                    None
                }
            });
        // Spotify sometimes delivers the thumbnail in a later event; keep the
        // old one only if the track did not change.
        if thumb.is_some() || track_changed {
            s.thumbnail = thumb;
        }
        tracing::info!(
            title = %s.title, artist = %s.artist, album = %s.album,
            thumbnail = ?s.thumbnail, track_changed, "media properties"
        );
    }

    fn refresh_playback(&mut self) {
        let Some((gs, s)) = self.parts() else { return };
        let info = match gs.GetPlaybackInfo() {
            Ok(i) => i,
            Err(e) => {
                tracing::warn!(error = %e, "playback info unavailable");
                return;
            }
        };
        s.playback = match info.PlaybackStatus() {
            Ok(GsmtcStatus::Playing) => Playback::Playing,
            Ok(GsmtcStatus::Paused) => Playback::Paused,
            _ => Playback::Other,
        };
        if let Ok(c) = info.Controls() {
            s.controls = Controls {
                play: c.IsPlayEnabled().unwrap_or(false),
                pause: c.IsPauseEnabled().unwrap_or(false),
                next: c.IsNextEnabled().unwrap_or(false),
                previous: c.IsPreviousEnabled().unwrap_or(false),
                shuffle: c.IsShuffleEnabled().unwrap_or(false),
                repeat: c.IsRepeatEnabled().unwrap_or(false),
            };
        }
        s.shuffle = info.IsShuffleActive().and_then(|r| r.Value()).ok();
        s.repeat = info
            .AutoRepeatMode()
            .and_then(|r| r.Value())
            .ok()
            .map(|m| match m {
                MediaPlaybackAutoRepeatMode::Track => RepeatMode::One,
                MediaPlaybackAutoRepeatMode::List => RepeatMode::All,
                _ => RepeatMode::Off,
            });
        s.clock
            .set_playing(s.playback == Playback::Playing, SystemTime::now());
        tracing::info!(
            playback = ?s.playback, shuffle = ?s.shuffle, repeat = ?s.repeat,
            controls = ?s.controls, "playback info"
        );
    }

    fn refresh_timeline(&mut self) {
        let Some((gs, s)) = self.parts() else { return };
        let tl = match gs.GetTimelineProperties() {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(error = %e, "timeline unavailable");
                return;
            }
        };
        let (Ok(pos), Ok(start), Ok(end), Ok(updated)) = (
            tl.Position(),
            tl.StartTime(),
            tl.EndTime(),
            tl.LastUpdatedTime(),
        ) else {
            return;
        };
        let duration = duration_from_winrt(end.Duration - start.Duration);
        s.clock.sync(
            duration_from_winrt(pos.Duration - start.Duration),
            system_time_from_winrt(updated.UniversalTime),
            Some(duration),
        );
        tracing::debug!(
            position_ms = s.clock.position(SystemTime::now()).as_millis() as u64,
            duration_ms = duration.as_millis() as u64,
            "timeline"
        );
    }

    fn execute(&mut self, command: Command) {
        let Some(bound) = &self.bound else {
            tracing::debug!(?command, "no session for command");
            return;
        };
        let s = &bound.session;
        let current = self.snapshot.session.as_ref();
        let result = match command {
            Command::PlayPause => s.TryTogglePlayPauseAsync(),
            Command::Next => s.TrySkipNextAsync(),
            Command::Previous => s.TrySkipPreviousAsync(),
            Command::ToggleShuffle => {
                let on = current.and_then(|c| c.shuffle).unwrap_or(false);
                s.TryChangeShuffleActiveAsync(!on)
            }
            Command::CycleRepeat => {
                let next = current
                    .and_then(|c| c.repeat)
                    .unwrap_or(RepeatMode::Off)
                    .next();
                s.TryChangeAutoRepeatModeAsync(match next {
                    RepeatMode::Off => MediaPlaybackAutoRepeatMode::None,
                    RepeatMode::All => MediaPlaybackAutoRepeatMode::List,
                    RepeatMode::One => MediaPlaybackAutoRepeatMode::Track,
                })
            }
        }
        .and_then(|op| op.join());
        match result {
            Ok(accepted) => tracing::info!(?command, accepted, "command"),
            Err(e) => tracing::warn!(?command, error = %e, "command failed"),
        }
    }
}

fn read_thumbnail(r: &IRandomAccessStreamReference) -> windows::core::Result<Vec<u8>> {
    let stream = r.OpenReadAsync()?.join()?;
    let size = stream.Size()?;
    if size == 0 || size > MAX_THUMBNAIL_BYTES {
        return Err(windows::core::Error::empty());
    }
    let size = size as u32;
    let buffer = Buffer::Create(size)?;
    let filled = stream
        .ReadAsync(&buffer, size, InputStreamOptions::None)?
        .join()?;
    let len = filled.Length()? as usize;
    let mut bytes = vec![0u8; len];
    DataReader::FromBuffer(&filled)?.ReadBytes(&mut bytes)?;
    Ok(bytes)
}
