//! Application state machine: Idle (tray only) <-> Active (fullscreen).

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use tray_icon::menu::MenuId;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, MouseButton, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorIcon, WindowId};

use crate::audio::AudioAnalyzer;
use crate::cli::CliArgs;
use crate::color::{Oklab, linear_to_srgb};
use crate::cover::{CoverResult, CoverWorker, Image, hd};
use crate::demo::Demo;
use crate::fullscreen::{self, MonitorSignature, MonitorWindow};
use crate::hotkey::Hotkey;
use crate::i18n::{self, I18n};
use crate::lyrics::lrc::{self, LineState};
use crate::lyrics::{LyricsTracker, state_at};
use crate::media::{Command, MediaService, MediaSnapshot, Playback, RepeatMode, Session};
use crate::net::{NetJob, NetResult, NetWorker};
use crate::render::ui::{ButtonView, CoverLyricsView, LyricsView, TrackView, UiModel};
use crate::render::{FrameResult, Gpu, Surface};
use crate::settings::model::ColorMode;
use crate::settings::paths::Paths;
use crate::settings::store;
use crate::settings::{MonitorMode, Settings};
use crate::settings_window::{MonitorInfo, PanelEvent, SettingsWindow};
use crate::tray::{MenuAction, Tray};
use crate::ui::anim::{Smooth, Toggle, ease_in_out_cubic};
use crate::ui::frame::{FrameAnimator, FrameInputs, FrameState, preset_colors};
use crate::ui::layout::{self, Button};

/// Second Esc must follow within this window (SPEC §4).
const DOUBLE_ESC: Duration = Duration::from_millis(600);
/// The Esc hint fades out after this time (SPEC §4).
const HINT_DURATION: f32 = 1.5;
const HINT_FADE: f32 = 0.4;
/// How often the monitor layout is checked while active.
const MONITOR_POLL: Duration = Duration::from_secs(2);
/// Player <-> cover view crossfade (SPEC §5.2).
const VIEW_TRANSITION: Duration = Duration::from_millis(400);
/// Cover crossfade on track change (SPEC §5.6).
const TRACK_TRANSITION: f32 = 0.5;
/// Centered <-> split player layout (SPEC §5.3).
const LAYOUT_TRANSITION: Duration = Duration::from_millis(550);
/// Fade/slide of the lyric line in the cover view (SPEC §5.4).
const LINE_TRANSITION: f32 = 0.35;
const DEMO_LYRICS: &str = include_str!("../assets/demo/lyrics.lrc");
/// Settings are written this long after the last change (slider drags).
const SAVE_DELAY: Duration = Duration::from_millis(600);
/// Delay before trimming the working set (driver cleanup finishes first).
const TRIM_DELAY: Duration = Duration::from_millis(1500);

/// Monitors for the settings list, left to right.
fn monitor_infos(event_loop: &ActiveEventLoop) -> Vec<MonitorInfo> {
    let primary = event_loop.primary_monitor();
    let mut list: Vec<_> = event_loop
        .available_monitors()
        .map(|m| {
            let size = m.size();
            (
                m.position().x,
                MonitorInfo {
                    id: m.name().unwrap_or_else(|| format!("{:?}", m.position())),
                    label: format!("{}×{}", size.width, size.height),
                    primary: primary.as_ref() == Some(&m),
                },
            )
        })
        .collect();
    list.sort_by_key(|(x, _)| *x);
    list.into_iter().map(|(_, m)| m).collect()
}
/// Mouse movements below this distance (physical px) are ignored.
const MOUSE_JITTER: f64 = 2.0;

fn fmt_time(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}.{}", s / 60, s % 60, d.subsec_millis() / 100)
}

fn hash_of(v: impl Hash) -> u64 {
    let mut h = DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Idle,
    Active,
}

/// Events posted into the event loop from other threads.
#[derive(Debug, Clone)]
pub enum UserEvent {
    /// A second instance was started.
    ShowSettings,
    /// `--exit-after` elapsed.
    Quit,
    TrayDoubleClick,
    Menu(MenuId),
    Hotkey(u32),
    Media(Box<MediaSnapshot>),
    Cover(Box<CoverResult>),
    Net(Box<NetResult>),
}

/// What the UI shows for one track.
#[derive(Debug, Clone)]
struct TrackDisplay {
    key: (String, String, String),
    title: String,
    artist: String,
    thumb_key: Option<u64>,
    album_key: Option<u64>,
}

struct ThumbImages {
    image: Option<Arc<Image>>,
    small: Option<Arc<Image>>,
}

/// Animated UI state of the fullscreen views.
struct UiState {
    view: Toggle,
    pause: Smooth,
    standby: Smooth,
    press: [Smooth; 5],
    last_activity: Instant,
    last_cursor: Option<PhysicalPosition<f64>>,
    /// Pointer position in DIPs on the window it is over.
    pointer: Option<(WindowId, f32, f32)>,
    hover: Option<Button>,
    pressed: Option<Button>,
    cursor_hidden: bool,
}

impl UiState {
    fn new(now: Instant) -> Self {
        Self {
            view: Toggle::new(false, VIEW_TRANSITION),
            pause: Smooth::new(0.0, 0.18),
            standby: Smooth::new(0.0, 0.25),
            press: [Smooth::new(0.0, 0.05); 5],
            last_activity: now,
            last_cursor: None,
            pointer: None,
            hover: None,
            pressed: None,
            cursor_hidden: false,
        }
    }
}

/// Animated lyrics presentation state.
struct LyricsUi {
    /// Centered (0) <-> split (1) player layout.
    split: Toggle,
    /// Last eased split value (hit testing between frames).
    split_value: f32,
    /// Lyric lines in the cover view on/off.
    cover: Toggle,
    /// Smoothly following line index.
    scroll: Smooth,
    scroll_id: u64,
    /// Cover view line change animation.
    line: Option<usize>,
    prev_line: Option<usize>,
    line_changed_at: Instant,
}

impl LyricsUi {
    fn new(now: Instant) -> Self {
        Self {
            split: Toggle::new(false, LAYOUT_TRANSITION),
            split_value: 0.0,
            cover: Toggle::new(false, VIEW_TRANSITION),
            scroll: Smooth::new(-1.0, 0.12),
            scroll_id: 0,
            line: None,
            prev_line: None,
            line_changed_at: now,
        }
    }
}

/// Per-frame UI values computed once per frame.
struct UiFrame {
    view: f32,
    pause: f32,
    standby: f32,
    press: [f32; 5],
    track_t: f32,
    split: f32,
    cover_lyrics: f32,
    scroll: f32,
    line_state: Option<LineState>,
    line_change: f32,
}

pub struct App {
    args: CliArgs,
    proxy: EventLoopProxy<UserEvent>,
    media_service: Option<MediaService>,
    demo: Option<Demo>,
    /// Beat analysis; only exists while the mode is active.
    audio: Option<AudioAnalyzer>,
    media: MediaSnapshot,
    cover_worker: Option<CoverWorker>,
    net: Option<NetWorker>,
    current: Option<TrackDisplay>,
    previous: Option<TrackDisplay>,
    track_changed_at: Instant,
    thumbs: HashMap<u64, ThumbImages>,
    hd_images: HashMap<u64, Arc<Image>>,
    hd_requested: HashSet<u64>,
    cover_palette: Option<Vec<Oklab>>,
    settings_window: Option<SettingsWindow>,
    settings_monitors: Vec<MonitorInfo>,
    /// Pending debounced save.
    save_due: Option<Instant>,
    /// Pending working set trim after leaving the fullscreen mode or closing
    /// the settings window (GPU/OpenGL driver memory, SPEC §3 idle target).
    trim_due: Option<Instant>,
    paths: Paths,
    settings: Settings,
    i18n: I18n,
    mode: Mode,
    tray: Option<Tray>,
    hotkey: Option<Hotkey>,
    /// Shown as a hint in the settings window (phase 7).
    hotkey_error: Option<String>,
    windows: Vec<MonitorWindow>,
    gpu: Option<Gpu>,
    /// GPU init failed for this activation: animation monitors stay black.
    gpu_failed: bool,
    animator: FrameAnimator,
    ui: UiState,
    lyrics: LyricsTracker,
    lyrics_ui: LyricsUi,
    monitors: Option<MonitorSignature>,
    next_monitor_check: Instant,
    last_esc: Option<Instant>,
    hint_since: Option<Instant>,
    initialized: bool,
}

impl App {
    pub fn new(
        args: CliArgs,
        paths: Paths,
        settings: Settings,
        proxy: EventLoopProxy<UserEvent>,
    ) -> Self {
        let i18n = I18n::new(i18n::resolve_language(settings.language));
        let now = Instant::now();
        Self {
            args,
            proxy,
            media_service: None,
            demo: None,
            audio: None,
            media: MediaSnapshot::default(),
            cover_worker: None,
            net: None,
            current: None,
            previous: None,
            track_changed_at: now,
            thumbs: HashMap::new(),
            hd_images: HashMap::new(),
            hd_requested: HashSet::new(),
            cover_palette: None,
            settings_window: None,
            settings_monitors: Vec::new(),
            save_due: None,
            trim_due: None,
            paths,
            settings,
            i18n,
            mode: Mode::Idle,
            tray: None,
            hotkey: None,
            hotkey_error: None,
            windows: Vec::new(),
            gpu: None,
            gpu_failed: false,
            animator: FrameAnimator::new(now),
            ui: UiState::new(now),
            lyrics: LyricsTracker::default(),
            lyrics_ui: LyricsUi::new(now),
            monitors: None,
            next_monitor_check: now,
            last_esc: None,
            hint_since: None,
            initialized: false,
        }
    }

    fn init(&mut self, event_loop: &ActiveEventLoop) {
        match Tray::new(&self.i18n) {
            Ok(t) => self.tray = Some(t),
            Err(e) => tracing::error!(error = %e, "tray icon unavailable"),
        }
        match Hotkey::new() {
            Ok(h) => self.hotkey = Some(h),
            Err(e) => tracing::error!(error = %e, "hotkey manager unavailable"),
        }
        self.register_hotkey();

        let proxy = self.proxy.clone();
        self.cover_worker = Some(CoverWorker::start(move |result| {
            let _ = proxy.send_event(UserEvent::Cover(Box::new(result)));
        }));
        let proxy = self.proxy.clone();
        self.net = Some(NetWorker::start(
            self.paths.cache_dir.clone(),
            move |result| {
                let _ = proxy.send_event(UserEvent::Net(Box::new(result)));
            },
        ));

        if self.args.demo {
            tracing::info!("demo mode: fake Spotify data, synthetic 120 BPM beat");
            let demo = Demo::new();
            let snapshot = demo.snapshot();
            self.demo = Some(demo);
            self.on_media(snapshot);
        } else {
            let proxy = self.proxy.clone();
            self.media_service = Some(MediaService::start(move |snapshot| {
                let _ = proxy.send_event(UserEvent::Media(Box::new(snapshot)));
            }));
        }

        if self.args.start {
            self.activate(event_loop);
        }
    }

    fn register_hotkey(&mut self) {
        let Some(hk) = self.hotkey.as_mut() else {
            return;
        };
        let spec = self.settings.hotkey.clone();
        match hk.register(&spec) {
            Ok(()) => self.hotkey_error = None,
            Err(e) => {
                tracing::warn!(hotkey = %spec, error = %e, "hotkey registration failed");
                self.hotkey_error = Some(e);
                if let Some(tray) = &self.tray {
                    tray.notify(
                        self.i18n.t("notify.hotkey_failed_title"),
                        &self.i18n.tf("notify.hotkey_failed", &[("hotkey", &spec)]),
                    );
                }
            }
        }
    }

    // ----- mode -----

    fn activate(&mut self, event_loop: &ActiveEventLoop) {
        if self.mode == Mode::Active {
            return;
        }
        tracing::info!("mode: active");
        let now = Instant::now();
        self.mode = Mode::Active;
        self.last_esc = None;
        self.hint_since = None;
        self.gpu_failed = false;
        self.animator = FrameAnimator::new(now);
        self.animator.jump_palette(&self.target_palette());
        self.ui = UiState::new(now);
        self.lyrics_ui = LyricsUi::new(now);
        let standby = if self.media.session.is_none() {
            1.0
        } else {
            0.0
        };
        self.ui.standby.set(standby);
        self.track_changed_at = now - Duration::from_secs(1);
        if self.demo.is_none() {
            self.audio = Some(AudioAnalyzer::start(
                self.settings.frame.beat as f32 / 100.0,
            ));
        }
        fullscreen::keep_awake(true);
        self.build_windows(event_loop);
        if let Some(t) = self.tray.as_mut() {
            t.set_active(true, &self.i18n);
        }
        if let Some(sw) = &self.settings_window {
            sw.set_topmost(true);
        }
    }

    fn deactivate(&mut self) {
        if self.mode == Mode::Idle {
            return;
        }
        tracing::info!("mode: idle");
        self.mode = Mode::Idle;
        self.audio = None;
        self.windows.clear();
        self.gpu = None;
        self.monitors = None;
        fullscreen::keep_awake(false);
        self.trim_due = Some(Instant::now() + TRIM_DELAY);
        if let Some(t) = self.tray.as_mut() {
            t.set_active(false, &self.i18n);
        }
        if let Some(sw) = &self.settings_window {
            sw.set_topmost(false);
        }
    }

    fn toggle(&mut self, event_loop: &ActiveEventLoop) {
        match self.mode {
            Mode::Idle => self.activate(event_loop),
            Mode::Active => self.deactivate(),
        }
    }

    fn build_windows(&mut self, event_loop: &ActiveEventLoop) {
        self.windows.clear();
        self.monitors = Some(fullscreen::monitor_signature(event_loop));
        self.windows = fullscreen::create_windows(event_loop, &self.settings);
        self.ui.cursor_hidden = false;
        self.ensure_surfaces();
        self.next_monitor_check = Instant::now() + MONITOR_POLL;
    }

    /// Creates the GPU context and the swap chains of animation windows.
    /// On failure the affected monitors stay black (GDI).
    fn ensure_surfaces(&mut self) {
        let needs = self
            .windows
            .iter()
            .any(|w| w.mode == MonitorMode::Animation && w.surface.is_none());
        if !needs || self.gpu_failed {
            return;
        }
        if self.gpu.is_none() {
            match Gpu::new() {
                Ok(g) => self.gpu = Some(g),
                Err(e) => {
                    tracing::error!(error = %e, "GPU init failed, animation monitors stay black");
                    self.gpu_failed = true;
                    return;
                }
            }
        }
        let Some(gpu) = &self.gpu else { return };
        for w in self
            .windows
            .iter_mut()
            .filter(|w| w.mode == MonitorMode::Animation && w.surface.is_none())
        {
            let Some(hwnd) = fullscreen::hwnd(&w.window) else {
                continue;
            };
            let size = w.window.inner_size();
            let scale = w.window.scale_factor() as f32;
            match Surface::new(gpu, hwnd, size.width, size.height, scale) {
                Ok(s) => {
                    w.surface = Some(s);
                    w.window.request_redraw();
                }
                Err(e) => tracing::error!(error = %e, "swap chain creation failed"),
            }
        }
    }

    fn device_lost(&mut self) {
        tracing::warn!("recreating GPU resources");
        for w in &mut self.windows {
            w.surface = None;
        }
        self.gpu = None;
        self.ensure_surfaces();
    }

    fn check_monitors(&mut self, event_loop: &ActiveEventLoop) {
        let sig = fullscreen::monitor_signature(event_loop);
        if self.monitors.as_ref() != Some(&sig) {
            tracing::info!("monitor layout changed, rebuilding windows");
            self.build_windows(event_loop);
        }
        self.next_monitor_check = Instant::now() + MONITOR_POLL;
    }

    fn on_escape(&mut self) {
        let now = Instant::now();
        match self.last_esc {
            Some(t) if now.duration_since(t) <= DOUBLE_ESC => {
                self.last_esc = None;
                self.deactivate();
            }
            _ => {
                self.last_esc = Some(now);
                self.hint_since = Some(now);
            }
        }
    }

    // ----- settings window -----

    fn show_settings(&mut self, event_loop: &ActiveEventLoop) {
        self.settings_monitors = monitor_infos(event_loop);
        if let Some(sw) = &self.settings_window {
            sw.focus();
            sw.request_redraw();
            return;
        }
        match SettingsWindow::open(event_loop, &self.i18n, self.mode == Mode::Active) {
            Ok(sw) => {
                tracing::info!("settings window opened");
                self.settings_window = Some(sw);
            }
            Err(e) => {
                // E.g. remote desktop or a VM without OpenGL 2.0 driver.
                tracing::error!(error = %e, "settings window unavailable");
                if let Some(tray) = &self.tray {
                    let path = self.paths.settings_file().display().to_string();
                    tray.notify(
                        self.i18n.t("notify.settings_failed_title"),
                        &self.i18n.tf("notify.settings_failed", &[("path", &path)]),
                    );
                }
            }
        }
    }

    fn close_settings(&mut self) {
        if let Some(sw) = self.settings_window.take() {
            drop(sw);
            self.trim_due = Some(Instant::now() + TRIM_DELAY);
            tracing::info!("settings window closed");
            // A hotkey recording in progress is abandoned.
            self.register_hotkey();
            self.save_settings_now();
        }
    }

    fn settings_window_event(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => self.close_settings(),
            WindowEvent::RedrawRequested => {
                let Some(mut sw) = self.settings_window.take() else {
                    return;
                };
                let before = self.settings.clone();
                let events = sw.render(
                    &mut self.settings,
                    &self.i18n,
                    &self.settings_monitors,
                    self.hotkey_error.as_deref(),
                );
                for e in events {
                    self.on_panel_event(&mut sw, e);
                }
                self.settings_window = Some(sw);
                if self.settings != before {
                    self.apply_settings(&before, event_loop);
                }
            }
            other => {
                if let Some(sw) = self.settings_window.as_mut() {
                    sw.on_event(&other);
                }
            }
        }
    }

    fn on_panel_event(&mut self, sw: &mut SettingsWindow, event: PanelEvent) {
        match event {
            PanelEvent::StartRecording => {
                // Free the current combination so it can be pressed again.
                if let Some(h) = self.hotkey.as_mut() {
                    h.suspend();
                }
                sw.set_recording(true, None);
            }
            PanelEvent::RecordFailed(e) => sw.set_recording(true, Some(e)),
            PanelEvent::CancelRecording => {
                self.register_hotkey();
                sw.set_recording(false, None);
            }
            PanelEvent::Recorded(spec) => {
                sw.set_recording(false, None);
                let Some(h) = self.hotkey.as_mut() else {
                    return;
                };
                match h.register(&spec) {
                    Ok(()) => {
                        self.settings.hotkey = spec;
                        self.hotkey_error = None;
                    }
                    Err(e) => {
                        tracing::warn!(hotkey = %spec, error = %e, "recorded hotkey unavailable");
                        let _ = h.register(&self.settings.hotkey);
                        self.hotkey_error = Some(e);
                    }
                }
            }
        }
    }

    /// Applies changed settings immediately (SPEC §7: live preview).
    fn apply_settings(&mut self, old: &Settings, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let new = self.settings.clone();
        if new.language != old.language {
            self.i18n.set_language(i18n::resolve_language(new.language));
            if let Some(t) = &self.tray {
                t.refresh_texts(&self.i18n);
            }
            if let Some(sw) = &self.settings_window {
                sw.set_title(&self.i18n);
                sw.request_redraw();
            }
        }
        if new.monitors != old.monitors && self.mode == Mode::Active {
            self.build_windows(event_loop);
            if let Some(sw) = &self.settings_window {
                sw.set_topmost(true);
            }
        }
        let f = (&new.frame, &old.frame);
        if (f.0.color_mode, f.0.palette, f.0.custom_colors)
            != (f.1.color_mode, f.1.palette, f.1.custom_colors)
        {
            self.animator.set_palette(&self.target_palette(), now);
        }
        if f.0.beat != f.1.beat
            && let Some(a) = &self.audio
        {
            a.set_sensitivity(f.0.beat as f32 / 100.0);
        }
        if new.lyrics.enabled != old.lyrics.enabled {
            self.restart_lyrics(now);
        }
        if new.hd_cover
            && !old.hd_cover
            && let Some(s) = self.media.session.clone()
        {
            let key = self.current.as_ref().and_then(|t| t.album_key);
            self.request_hd(&s, key);
        }
        self.save_due = Some(now + SAVE_DELAY);
    }

    /// Re-evaluates the lyrics of the current track (setting toggled).
    fn restart_lyrics(&mut self, now: Instant) {
        let Some(s) = self.media.session.clone() else {
            return;
        };
        let eligible = self.settings.lyrics.enabled
            && !s.title.trim().is_empty()
            && !s.artist.trim().is_empty();
        self.lyrics
            .track_changed(now - Duration::from_secs(1), eligible);
        if eligible && self.demo.is_some() {
            self.lyrics.set_found(lrc::parse(DEMO_LYRICS));
        }
        self.request_lyrics(now);
    }

    fn save_settings_now(&mut self) {
        if self.save_due.take().is_some() {
            store::save(&self.paths.settings_file(), &self.settings);
            tracing::debug!("settings saved");
        }
    }

    fn quit(&mut self, event_loop: &ActiveEventLoop) {
        self.close_settings();
        self.save_settings_now();
        self.deactivate();
        self.media_service = None;
        self.cover_worker = None;
        self.net = None;
        tracing::info!("quit");
        event_loop.exit();
    }

    // ----- media, covers and colors -----

    fn on_media(&mut self, snapshot: MediaSnapshot) {
        if snapshot == self.media {
            return;
        }
        let title = snapshot.session.as_ref().map(|s| s.display_title());
        if title != self.media.session.as_ref().map(|s| s.display_title())
            && let Some(tray) = &self.tray
        {
            tray.set_now_playing(title);
        }
        if let Some(s) = &snapshot.session {
            self.update_track(s);
        }
        self.media = snapshot;
        self.request_lyrics(Instant::now());
        self.log_position();
    }

    fn update_track(&mut self, s: &Session) {
        let key = (s.title.clone(), s.artist.clone(), s.album.clone());
        if self.current.as_ref().map(|t| &t.key) != Some(&key) {
            let album_key = (!s.album.trim().is_empty() && !s.artist.trim().is_empty())
                .then(|| hash_of(hd::cache_name(&s.artist, &s.album)));
            tracing::info!(title = %s.display_title(), artist = %s.artist, "track change");
            self.previous = self.current.take();
            self.current = Some(TrackDisplay {
                key,
                title: s.display_title().to_string(),
                artist: s.artist.clone(),
                thumb_key: None,
                album_key,
            });
            self.track_changed_at = Instant::now();
            // No lyrics for ads (no title/artist) or when disabled (SPEC §5.6).
            let eligible = self.settings.lyrics.enabled
                && !s.title.trim().is_empty()
                && !s.artist.trim().is_empty();
            self.lyrics.track_changed(self.track_changed_at, eligible);
            if eligible && self.demo.is_some() {
                self.lyrics.set_found(lrc::parse(DEMO_LYRICS));
            }
            self.request_hd(s, album_key);
            self.prune_images();
        }

        if let Some(thumb) = &s.thumbnail {
            let key = hash_of((s.track_key(), &thumb.0));
            let current = self.current.as_mut();
            if let Some(t) = current
                && t.thumb_key != Some(key)
            {
                t.thumb_key = Some(key);
                if let Some(w) = &self.cover_worker {
                    w.submit_thumb(key, thumb.0.clone());
                }
            }
        }
    }

    fn request_hd(&mut self, s: &Session, album_key: Option<u64>) {
        let Some(album_key) = album_key else { return };
        if !self.settings.hd_cover || self.demo.is_some() {
            return;
        }
        if self.hd_requested.insert(album_key)
            && let Some(net) = &self.net
        {
            net.submit(NetJob::HdCover {
                album_key,
                artist: s.artist.clone(),
                album: s.album.clone(),
            });
        }
    }

    /// Keeps only the images of the current and previous track.
    fn prune_images(&mut self) {
        let tracks = [self.current.as_ref(), self.previous.as_ref()];
        let thumbs: HashSet<u64> = tracks
            .iter()
            .flatten()
            .filter_map(|t| t.thumb_key)
            .collect();
        let albums: HashSet<u64> = tracks
            .iter()
            .flatten()
            .filter_map(|t| t.album_key)
            .collect();
        self.thumbs.retain(|k, _| thumbs.contains(k));
        self.hd_images.retain(|k, _| albums.contains(k));
    }

    fn on_cover(&mut self, result: CoverResult) {
        match result {
            CoverResult::Thumb {
                key,
                image,
                small,
                palette,
            } => {
                let is_current = self.current.as_ref().and_then(|t| t.thumb_key) == Some(key);
                let is_previous = self.previous.as_ref().and_then(|t| t.thumb_key) == Some(key);
                if !is_current && !is_previous {
                    return; // outdated
                }
                self.thumbs.insert(key, ThumbImages { image, small });
                if is_current && let Some(p) = palette {
                    self.cover_palette = Some(p);
                    let target = self.target_palette();
                    self.animator.set_palette(&target, Instant::now());
                }
            }
            CoverResult::Hd { album_key, image } => {
                if let Some(img) = image {
                    tracing::info!(?img, "HD cover ready");
                    self.hd_images.insert(album_key, img);
                }
            }
        }
    }

    /// Asks LRCLIB once the new track's timeline (duration) is known.
    fn request_lyrics(&mut self, now: Instant) {
        let Some(request_id) = self.lyrics.due_request(now) else {
            return;
        };
        let (Some(s), Some(net)) = (&self.media.session, &self.net) else {
            return;
        };
        net.submit(NetJob::Lyrics {
            request_id,
            title: s.title.clone(),
            artist: s.artist.clone(),
            album: s.album.clone(),
            duration: s.clock.duration(),
        });
    }

    fn on_net(&mut self, result: NetResult) {
        match result {
            NetResult::Lyrics { request_id, lines } => {
                self.lyrics.on_result(request_id, lines);
            }
            NetResult::HdCover { album_key, bytes } => {
                if let (Some(bytes), Some(w)) = (bytes, &self.cover_worker) {
                    w.submit_hd(album_key, bytes);
                }
            }
        }
    }

    /// Palette for the frame: cover colors, fixed palette, or fallback.
    fn target_palette(&self) -> Vec<Oklab> {
        let f = &self.settings.frame;
        match (f.color_mode, &self.cover_palette) {
            (ColorMode::Cover, Some(p)) => p.clone(),
            _ => preset_colors(f.palette, f.custom_colors)
                .map(Oklab::from_rgb)
                .to_vec(),
        }
    }

    fn frame_state(&self) -> FrameState {
        match &self.media.session {
            None => FrameState::Standby,
            Some(s) if s.playback == Playback::Playing => FrameState::Playing,
            Some(_) => FrameState::Paused,
        }
    }

    fn send_command(&mut self, cmd: Command) {
        if let Some(demo) = self.demo.as_mut() {
            let snapshot = demo.command(cmd);
            self.on_media(snapshot);
        } else if let Some(m) = &self.media_service {
            m.command(cmd);
        }
    }

    /// Debug output of the interpolated position.
    fn log_position(&self) {
        match &self.media.session {
            Some(s) => tracing::debug!(
                title = %s.display_title(),
                artist = %s.artist,
                playback = ?s.playback,
                position = %fmt_time(s.clock.position(SystemTime::now())),
                duration = %s.clock.duration().map(fmt_time).unwrap_or_default(),
                "now playing"
            ),
            None => tracing::debug!("no Spotify session"),
        }
    }

    /// Development keys in debug builds: Space play/pause, Left/Right
    /// previous/next, S shuffle, R repeat.
    #[cfg(debug_assertions)]
    fn dev_key(&mut self, key: &Key) {
        let cmd = match key {
            Key::Named(NamedKey::Space) => Command::PlayPause,
            Key::Named(NamedKey::ArrowRight) => Command::Next,
            Key::Named(NamedKey::ArrowLeft) => Command::Previous,
            Key::Character(c) if c.eq_ignore_ascii_case("s") => Command::ToggleShuffle,
            Key::Character(c) if c.eq_ignore_ascii_case("r") => Command::CycleRepeat,
            _ => return,
        };
        self.send_command(cmd);
    }

    // ----- pointer and buttons -----

    fn button_views(&self, hover: Option<Button>, press: [f32; 5]) -> [ButtonView; 5] {
        let s = self.media.session.as_ref();
        let c = s.map(|s| s.controls).unwrap_or_default();
        let playing = s.is_some_and(|s| s.playback == Playback::Playing);
        Button::ALL.map(|b| {
            let i = Button::ALL.iter().position(|x| *x == b).unwrap_or(0);
            let (enabled, active) = match b {
                Button::Shuffle => (
                    c.shuffle && s.is_some_and(|s| s.shuffle.is_some()),
                    s.and_then(|s| s.shuffle) == Some(true),
                ),
                Button::Previous => (c.previous, false),
                Button::PlayPause => (if playing { c.pause } else { c.play }, false),
                Button::Next => (c.next, false),
                Button::Repeat => (
                    c.repeat && s.is_some_and(|s| s.repeat.is_some()),
                    s.and_then(|s| s.repeat)
                        .is_some_and(|r| r != RepeatMode::Off),
                ),
            };
            ButtonView {
                button: b,
                enabled: enabled && s.is_some(),
                active,
                repeat_one: s.and_then(|s| s.repeat) == Some(RepeatMode::One),
                playing,
                hover: hover == Some(b),
                press: press[i],
            }
        })
    }

    fn button_enabled(&self, b: Button) -> bool {
        self.button_views(None, [0.0; 5])
            .iter()
            .any(|v| v.button == b && v.enabled)
    }

    /// Buttons are only interactive while the player view is visible.
    fn interactive(&self) -> bool {
        self.ui.view.raw() < 0.5 && self.media.session.is_some()
    }

    fn on_cursor_moved(&mut self, id: WindowId, pos: PhysicalPosition<f64>) {
        let moved = self
            .ui
            .last_cursor
            .is_none_or(|l| (l.x - pos.x).hypot(l.y - pos.y) > MOUSE_JITTER);
        self.ui.last_cursor = Some(pos);
        if moved {
            self.ui.last_activity = Instant::now();
            self.set_cursor_hidden(false);
        }
        let Some(w) = self.windows.iter().find(|w| w.id() == id) else {
            return;
        };
        let scale = w.window.scale_factor();
        let (x, y) = ((pos.x / scale) as f32, (pos.y / scale) as f32);
        self.ui.pointer = Some((id, x, y));
        self.update_hover();
    }

    fn update_hover(&mut self) {
        let hover = match self.ui.pointer {
            Some((id, x, y)) if self.interactive() => self
                .windows
                .iter()
                .find(|w| w.id() == id)
                .and_then(|w| w.surface.as_ref())
                .and_then(|s| {
                    let (w, h) = s.size_dip();
                    layout::player_mixed(w, h, self.lyrics_ui.split_value).hit(x, y)
                }),
            _ => None,
        };
        if hover != self.ui.hover {
            self.ui.hover = hover;
            let clickable = hover.is_some_and(|b| self.button_enabled(b));
            let icon = if clickable {
                CursorIcon::Pointer
            } else {
                CursorIcon::Default
            };
            for w in &self.windows {
                w.window.set_cursor(icon);
            }
        }
    }

    fn on_mouse_button(&mut self, id: WindowId, state: ElementState, button: MouseButton) {
        if state == ElementState::Pressed {
            self.ui.last_activity = Instant::now();
            self.set_cursor_hidden(false);
            if let Some(w) = self.windows.iter().find(|w| w.id() == id) {
                fullscreen::focus(&w.window);
            }
        }
        if button != MouseButton::Left {
            return;
        }
        match state {
            ElementState::Pressed => {
                self.ui.pressed = self.ui.hover.filter(|b| self.button_enabled(*b));
            }
            ElementState::Released => {
                if let Some(b) = self.ui.pressed.take()
                    && self.ui.hover == Some(b)
                {
                    self.send_command(match b {
                        Button::Shuffle => Command::ToggleShuffle,
                        Button::Previous => Command::Previous,
                        Button::PlayPause => Command::PlayPause,
                        Button::Next => Command::Next,
                        Button::Repeat => Command::CycleRepeat,
                    });
                }
            }
        }
    }

    fn set_cursor_hidden(&mut self, hidden: bool) {
        if self.ui.cursor_hidden != hidden {
            self.ui.cursor_hidden = hidden;
            for w in &self.windows {
                w.window.set_cursor_visible(!hidden);
            }
        }
    }

    // ----- rendering -----

    fn hint_alpha(&self, now: Instant) -> f32 {
        let Some(t0) = self.hint_since else {
            return 0.0;
        };
        let e = now.saturating_duration_since(t0).as_secs_f32();
        let fade_in = (e / 0.12).min(1.0);
        let fade_out = ((HINT_DURATION - e) / HINT_FADE).clamp(0.0, 1.0);
        fade_in * fade_out
    }

    /// Advances all UI animations to `now`.
    fn ui_tick(&mut self, now: Instant) -> UiFrame {
        let has_session = self.media.session.is_some();
        let delay = Duration::from_secs(self.settings.cover_view_delay_s as u64);
        let idle = now.saturating_duration_since(self.ui.last_activity) >= delay;
        let cover_view = idle && has_session;
        let view = self.ui.view.update(now, cover_view);
        let paused = has_session && self.frame_state() == FrameState::Paused;
        let pause = self.ui.pause.update(now, if paused { 1.0 } else { 0.0 });
        let standby = self
            .ui
            .standby
            .update(now, if has_session { 0.0 } else { 1.0 });
        let mut press = [0.0; 5];
        for (i, b) in Button::ALL.iter().enumerate() {
            let target = if self.ui.pressed == Some(*b) {
                1.0
            } else {
                0.0
            };
            press[i] = self.ui.press[i].update(now, target);
        }
        // Cursor hidden in the cover view (SPEC §5.1).
        if cover_view && self.ui.view.raw() >= 1.0 {
            self.set_cursor_hidden(true);
        }
        if self.ui.hover.is_some() && !self.interactive() {
            self.update_hover();
        }
        let track_t = ease_in_out_cubic(
            now.saturating_duration_since(self.track_changed_at)
                .as_secs_f32()
                / TRACK_TRANSITION,
        );

        // ----- lyrics -----
        self.request_lyrics(now);
        let ls = &self.settings.lyrics;
        let (show_player, show_cover) = (ls.enabled && ls.in_player, ls.enabled && ls.in_cover);
        let split_target = self.lyrics.split_target(now, show_player);
        let split = self.lyrics_ui.split.update(now, split_target);
        self.lyrics_ui.split_value = split;
        let found = self.lyrics.lyrics().cloned();
        let cover_lyrics = self
            .lyrics_ui
            .cover
            .update(now, show_cover && found.is_some());
        let line_state = match (&found, &self.media.session) {
            (Some(l), Some(s)) => {
                let pos = s.clock.position(SystemTime::now());
                let offset = Duration::from_millis(ls.offset_ms.unsigned_abs() as u64);
                // Positive offset shows the lyrics earlier.
                let pos = if ls.offset_ms >= 0 {
                    pos + offset
                } else {
                    pos.saturating_sub(offset)
                };
                Some((l.id, state_at(&l.lines, pos)))
            }
            _ => None,
        };
        let scroll_target = line_state
            .and_then(|(_, st)| st.current)
            .map_or(-1.0, |i| i as f32);
        if let Some((id, _)) = line_state
            && id != self.lyrics_ui.scroll_id
        {
            // New lyrics: jump instead of scrolling through the whole text.
            self.lyrics_ui.scroll_id = id;
            self.lyrics_ui.scroll.set(scroll_target);
        }
        let scroll = self.lyrics_ui.scroll.update(now, scroll_target);
        let current_line = line_state.and_then(|(_, st)| st.current);
        if current_line != self.lyrics_ui.line {
            self.lyrics_ui.prev_line = self.lyrics_ui.line;
            self.lyrics_ui.line = current_line;
            self.lyrics_ui.line_changed_at = now;
        }
        let line_change = ease_in_out_cubic(
            now.saturating_duration_since(self.lyrics_ui.line_changed_at)
                .as_secs_f32()
                / LINE_TRANSITION,
        );

        UiFrame {
            view,
            pause,
            standby,
            press,
            track_t,
            split,
            cover_lyrics,
            scroll,
            line_state: line_state.map(|(_, st)| st),
            line_change,
        }
    }

    /// Lyrics views for the split player and the cover view.
    fn lyrics_views(
        &self,
        frame: &UiFrame,
    ) -> (Option<LyricsView<'_>>, Option<CoverLyricsView<'_>>) {
        let (Some(l), Some(st)) = (self.lyrics.lyrics(), frame.line_state) else {
            return (None, None);
        };
        let text = |i: Option<usize>| {
            i.and_then(|i| l.lines.get(i))
                .map_or("", |x| x.text.as_str())
        };
        // Next line with text after the current one.
        let start = st.current.map_or(0, |i| i + 1);
        let next = l.lines[start.min(l.lines.len())..]
            .iter()
            .find(|x| !x.text.is_empty())
            .map_or("", |x| x.text.as_str());
        let player = LyricsView {
            id: l.id,
            lines: &l.lines,
            scroll: frame.scroll,
            gap: st.gap,
            alpha: frame.split,
        };
        let cover = CoverLyricsView {
            current: text(st.current),
            next,
            previous: text(self.lyrics_ui.prev_line),
            change: frame.line_change,
            gap: st.gap,
            alpha: frame.cover_lyrics,
        };
        (Some(player), Some(cover))
    }

    fn track_view<'a>(&'a self, t: &'a TrackDisplay, alpha: f32) -> TrackView<'a> {
        let thumb = t.thumb_key.and_then(|k| self.thumbs.get(&k));
        let hd = t.album_key.and_then(|k| self.hd_images.get(&k));
        TrackView {
            title: &t.title,
            artist: &t.artist,
            image: hd.or(thumb.and_then(|i| i.image.as_ref())),
            small: thumb.and_then(|i| i.small.as_ref()),
            alpha,
        }
    }

    fn accent(&self, now: Instant) -> [f32; 3] {
        let c = self.animator.current_stops(now)[1].to_linear();
        [
            linear_to_srgb(c.r),
            linear_to_srgb(c.g),
            linear_to_srgb(c.b),
        ]
    }

    fn render(&mut self, id: WindowId) {
        let now = Instant::now();
        let state = self.frame_state();
        let (since_beat, energy) = match (&self.demo, &self.audio) {
            (Some(d), _) => (d.since_beat(now), d.energy(now)),
            (None, Some(a)) => (a.since_beat(now), a.energy()),
            (None, None) => (None, 0.5),
        };
        let hint_alpha = self.hint_alpha(now);
        if hint_alpha == 0.0
            && self
                .hint_since
                .is_some_and(|t| now - t > Duration::from_secs(2))
        {
            self.hint_since = None;
        }
        let frame = self.ui_tick(now);

        let Some((size, size_dip)) = self
            .windows
            .iter()
            .find(|w| w.id() == id)
            .and_then(|w| w.surface.as_ref())
            .map(|s| (s.size(), s.size_dip()))
        else {
            return;
        };
        let inputs = FrameInputs {
            state,
            settings: &self.settings.frame,
            since_beat,
            energy,
        };
        let params = self.animator.update(now, &inputs, size.0, size.1);

        let mut tracks = Vec::with_capacity(2);
        if frame.track_t < 1.0
            && let Some(p) = &self.previous
        {
            tracks.push(self.track_view(p, 1.0 - frame.track_t));
        }
        if let Some(c) = &self.current {
            tracks.push(self.track_view(c, frame.track_t));
        }
        let (lyrics, cover_lyrics) = self.lyrics_views(&frame);
        let model = UiModel {
            time: now
                .saturating_duration_since(self.track_changed_at)
                .as_secs_f32(),
            player: layout::player_mixed(size_dip.0, size_dip.1, frame.split),
            lyrics,
            cover_lyrics,
            cover_lyrics_mix: frame.cover_lyrics,
            view: frame.view,
            standby: frame.standby,
            pause: frame.pause,
            tracks,
            buttons: self.button_views(self.ui.hover, frame.press),
            accent: self.accent(now),
            standby_title: self.i18n.t("fullscreen.waiting"),
            standby_hint: self.i18n.t("fullscreen.esc_esc"),
            esc_hint: (hint_alpha > 0.0).then(|| (self.i18n.t("fullscreen.esc_again"), hint_alpha)),
        };

        let (Some(gpu), Some(w)) = (&self.gpu, self.windows.iter().find(|w| w.id() == id)) else {
            return;
        };
        let Some(surface) = &w.surface else { return };
        let result = surface.render(gpu, &params, &model);
        w.window.request_redraw();
        if result == FrameResult::DeviceLost {
            self.device_lost();
        }
    }

    fn resize(&mut self, id: WindowId) {
        let Some(gpu) = &self.gpu else { return };
        if let Some(w) = self.windows.iter_mut().find(|w| w.id() == id)
            && let Some(s) = w.surface.as_mut()
        {
            let size = w.window.inner_size();
            let scale = w.window.scale_factor() as f32;
            if let Err(e) = s.resize(gpu, size.width, size.height, scale) {
                tracing::warn!(error = %e, "resize failed, recreating surface");
                w.surface = None;
                self.ensure_surfaces();
            }
        }
    }

    fn window(&self, id: WindowId) -> Option<&MonitorWindow> {
        self.windows.iter().find(|w| w.id() == id)
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        if cause == StartCause::Init && !self.initialized {
            self.initialized = true;
            self.init(event_loop);
        }
    }

    fn resumed(&mut self, _: &ActiveEventLoop) {}

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::ShowSettings => self.show_settings(event_loop),
            UserEvent::Quit => self.quit(event_loop),
            UserEvent::TrayDoubleClick => self.activate(event_loop),
            UserEvent::Hotkey(id) => {
                if self.hotkey.as_ref().is_some_and(|h| h.matches(id)) {
                    self.toggle(event_loop);
                }
            }
            UserEvent::Menu(id) => match self.tray.as_ref().and_then(|t| t.action(&id)) {
                Some(MenuAction::Toggle) => self.toggle(event_loop),
                Some(MenuAction::Settings) => self.show_settings(event_loop),
                Some(MenuAction::Quit) => self.quit(event_loop),
                None => {}
            },
            UserEvent::Media(snapshot) => self.on_media(*snapshot),
            UserEvent::Cover(result) => self.on_cover(*result),
            UserEvent::Net(result) => self.on_net(*result),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self
            .settings_window
            .as_ref()
            .is_some_and(|sw| sw.id() == id)
        {
            self.settings_window_event(event_loop, event);
            return;
        }
        match event {
            WindowEvent::RedrawRequested => {
                let has_surface = self.window(id).is_some_and(|w| w.surface.is_some());
                if has_surface {
                    self.render(id);
                } else if let Some(w) = self.window(id) {
                    fullscreen::paint_black(&w.window);
                }
            }
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed
                    && !event.repeat
                    && event.logical_key == Key::Named(NamedKey::Escape) =>
            {
                self.on_escape();
            }
            #[cfg(debug_assertions)]
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed && !event.repeat =>
            {
                self.dev_key(&event.logical_key);
            }
            WindowEvent::CursorMoved { position, .. } => self.on_cursor_moved(id, position),
            WindowEvent::CursorLeft { .. } => {
                if self.ui.pointer.is_some_and(|p| p.0 == id) {
                    self.ui.pointer = None;
                    self.update_hover();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.on_mouse_button(id, state, button);
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                if self.mode == Mode::Active {
                    self.resize(id);
                    self.check_monitors(event_loop);
                }
            }
            WindowEvent::Moved(_) => {
                if self.mode == Mode::Active {
                    self.check_monitors(event_loop);
                }
            }
            WindowEvent::CloseRequested => {
                // Alt+F4 on a fullscreen window ends the mode, not the program.
                self.deactivate();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if self.mode == Mode::Active && now >= self.next_monitor_check {
            self.check_monitors(event_loop);
            self.log_position();
        }
        if self.save_due.is_some_and(|t| now >= t) {
            self.save_settings_now();
        }
        if self.trim_due.is_some_and(|t| now >= t) {
            self.trim_due = None;
            if self.mode == Mode::Idle && self.settings_window.is_none() {
                fullscreen::trim_working_set();
            }
        }
        let repaint = self.settings_window.as_ref().and_then(|sw| sw.repaint_at());
        if let Some(sw) = &self.settings_window
            && repaint.is_some_and(|t| now >= t)
        {
            sw.request_redraw();
        }
        let deadlines = [
            (self.mode == Mode::Active).then_some(self.next_monitor_check),
            self.save_due,
            self.trim_due,
            repaint.filter(|t| *t > now),
        ];
        match deadlines.into_iter().flatten().min() {
            Some(t) => event_loop.set_control_flow(ControlFlow::WaitUntil(t)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }
}
