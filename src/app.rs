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
use crate::background::ImportError;
use crate::cli::CliArgs;
use crate::color::{Oklab, linear_to_srgb};
use crate::cover::{CoverResult, CoverWorker, Image, Palette, hd};
use crate::demo::Demo;
use crate::fullscreen::{self, MonitorSignature, MonitorWindow};
use crate::hotkey::Hotkey;
use crate::i18n::{self, I18n};
use crate::lyrics::lrc::{self, LineState};
use crate::lyrics::{LyricsTracker, state_at};
use crate::media::{Command, MediaService, MediaSnapshot, Playback, RepeatMode, Session};
use crate::net::{NetJob, NetResult, NetWorker};
use crate::render::capture::DesktopCapture;
use crate::render::ui::{
    BubbleKind, ButtonView, ChatBubble, ChatView, CoverLyricsView, LyricsView, TrackView, UiModel,
};
use crate::render::ui::{ChainStar, ChainView, StarsView};
use crate::render::{FrameResult, Gpu, Surface};
use crate::settings::model::ColorMode;
use crate::settings::model::{BackgroundMode, Language, LyricsStyle};
use crate::settings::paths::Paths;
use crate::settings::store;
use crate::settings::{MonitorMode, Settings};
use crate::settings_window::UpdateView;
use crate::settings_window::{MonitorInfo, PanelEvent, SettingsWindow};
use crate::tray::{MenuAction, Tray};
use crate::ui::anim::Spring;
use crate::ui::anim::spring_step;
use crate::ui::chain::StarChain;
use crate::ui::chat::{ChatThread, reveal_words};
use crate::ui::clock::{self, ClockFormat, LocalTime};
use crate::ui::frame::{FrameAnimator, FrameInputs, FrameState, preset_colors};
use crate::ui::idle::{IdleScheduler, IdleVariant, WakeGate, wake_distance};
use crate::ui::layout::{self, Button};
use crate::ui::vinyl::{self, Turntable};
use crate::update::Release;

/// Second Esc must follow within this window (SPEC §4).
const DOUBLE_ESC: Duration = Duration::from_millis(600);
/// The Esc hint fades out after this time (SPEC §4).
const HINT_DURATION: f32 = 1.5;
const HINT_FADE: f32 = 0.4;
/// How often the monitor layout is checked while active.
const MONITOR_POLL: Duration = Duration::from_secs(2);
/// Rendering runs as a chain (each frame requests the next). Modal loops
/// such as the tray menu can swallow a redraw request and stop the chain;
/// after this long without a frame it is restarted.
const RENDER_WATCHDOG: Duration = Duration::from_millis(150);
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

/// Lyrics style for a digit key 1-9 (main row or numpad, SPEC §4).
fn digit_style(key: &Key) -> Option<LyricsStyle> {
    let Key::Character(c) = key else { return None };
    let mut chars = c.chars();
    let d = chars.next()?.to_digit(10)?;
    if chars.next().is_some() {
        return None;
    }
    LyricsStyle::from_digit(d)
}

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
    /// Update check finished (`silent`: automatic check after start).
    UpdateChecked {
        silent: bool,
        result: Result<Option<Release>, String>,
    },
    /// Update downloaded and handed over (quit now) or failed.
    UpdateInstalled(Result<(), String>),
    /// Background image decoded for `key` (SPEC §5.1 v1.5).
    Background {
        key: BackgroundKey,
        image: Option<Arc<Image>>,
    },
    /// Custom background import finished (file name in the data folder).
    BackgroundImported(Result<String, ImportError>),
}

/// What the background shows; a change reloads the image.
pub type BackgroundKey = (BackgroundMode, Option<String>);

/// Startup update check runs this long after the program start.
const UPDATE_CHECK_DELAY: Duration = Duration::from_secs(10);

/// What the UI shows for one track.
#[derive(Debug, Clone)]
struct TrackDisplay {
    key: (String, String, String),
    title: String,
    artist: String,
    thumb_key: Option<u64>,
    album_key: Option<u64>,
    /// Coin flip of this song: record instead of cover (SPEC §5.4d).
    vinyl: bool,
    /// Last record blend 0..1 (kept while the track fades out).
    vinyl_mix: f32,
}

struct ThumbImages {
    image: Option<Arc<Image>>,
    small: Option<Arc<Image>>,
}

/// Animated UI state of the fullscreen views.
struct UiState {
    /// 0 = player view, 1 = cover view.
    view: Spring,
    pause: Spring,
    standby: Spring,
    press: [Spring; 5],
    /// Entry animation when the mode starts (0 -> 1).
    intro: Spring,
    /// Cover change on track change (restarts at 0).
    track: Spring,
    /// Idle view: cover view (0) <-> lyrics focus view (1).
    focus: Spring,
    idle_views: IdleScheduler,
    /// Clock line on/off (setting, faded).
    clock: Spring,
    /// Record view (SPEC §5.4d): cover <-> record blend and platter spin.
    vinyl: Spring,
    turntable: Turntable,
    turntable_at: Instant,
    /// Lyrics available for the focus view (held while loading).
    focus_lyrics: bool,
    last_activity: Instant,
    /// Mouse travel needed to leave the idle view (SPEC §5.2 v1.5).
    wake: WakeGate,
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
            view: Spring::ui(0.0),
            pause: Spring::smooth(0.0, 0.45),
            standby: Spring::smooth(0.0, 0.6),
            // Snappy, slightly bouncy press feedback.
            press: [Spring::new(0.0, 0.28, 0.55); 5],
            intro: Spring::new(0.0, 0.8, 0.9),
            track: Spring::new(1.0, 0.6, 0.88),
            // Slow, calm morph: this change happens without user input.
            focus: Spring::new(0.0, 0.95, 0.94),
            idle_views: IdleScheduler::from_clock(),
            clock: Spring::smooth(1.0, 0.4),
            vinyl: Spring::new(0.0, 0.9, 0.92),
            turntable: Turntable::default(),
            turntable_at: now,
            focus_lyrics: false,
            last_activity: now,
            wake: WakeGate::default(),
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
    split: Spring,
    /// Last split value (hit testing between frames).
    split_value: f32,
    /// Lyric lines in the cover view on/off.
    cover: Spring,
    /// Line index following the song like Apple Music's scrolling.
    scroll: Spring,
    scroll_id: u64,
    /// Cover view line change animation (restarts at 0).
    line: Option<usize>,
    prev_line: Option<usize>,
    line_change: Spring,
    /// Messages view (SPEC §5.4b): thread, scroll in bubble units, blend.
    thread: ChatThread,
    chat_scroll: Spring,
    chat_mix: Spring,
    /// Constellation view (SPEC §5.4c) blend.
    stars_mix: Spring,
    /// Constellation V2 (SPEC §5.4c2): word chain, blend, and the moment
    /// its lyrics went away (song change: the chain fades out).
    chain: StarChain,
    chain_mix: Spring,
    chain_gone: Option<Instant>,
}

impl LyricsUi {
    fn new() -> Self {
        Self {
            split: Spring::new(0.0, 0.65, 0.88),
            split_value: 0.0,
            cover: Spring::smooth(0.0, 0.5),
            scroll: Spring::new(-1.0, 0.55, 0.9),
            scroll_id: 0,
            line: None,
            prev_line: None,
            line_change: Spring::new(1.0, 0.45, 0.9),
            thread: ChatThread::new(),
            chat_scroll: Spring::new(0.0, 0.5, 0.86),
            chat_mix: Spring::smooth(0.0, 0.55),
            stars_mix: Spring::smooth(0.0, 0.55),
            chain: StarChain::new(),
            chain_mix: Spring::smooth(0.0, 0.55),
            chain_gone: None,
        }
    }
}

/// Per-frame UI values computed once per frame.
struct UiFrame {
    view: f32,
    pause: f32,
    standby: f32,
    press: [f32; 5],
    intro: f32,
    track_t: f32,
    split: f32,
    cover_lyrics: f32,
    focus: f32,
    clock: f32,
    /// Record label rotation (radians).
    spin: f32,
    scroll: f32,
    line_state: Option<LineState>,
    line_change: f32,
    /// Playback position incl. lyrics offset (word-by-word reveal).
    lyrics_pos: Option<Duration>,
    chat_scroll: f32,
    chat_mix: f32,
    stars_mix: f32,
    chain_mix: f32,
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
    cover_palette: Option<Palette>,
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
    /// Formatted clock line, rebuilt when the minute or language changes.
    clock: Option<(LocalTime, Language, String, String)>,
    /// Background image of the animation monitors and what it was loaded
    /// for (only while the mode runs).
    background_image: Option<Arc<Image>>,
    background_key: Option<BackgroundKey>,
    /// Live wallpaper capture (Wallpaper Engine & co.), declared before
    /// `gpu` users are dropped; only while the mode runs.
    desktop_capture: Option<DesktopCapture>,
    /// Localized problem with the last picked background image.
    image_note: Option<String>,
    /// Self-update (SPEC §6.5): state, found release, startup check time.
    update: UpdateView,
    release: Option<Release>,
    update_check_due: Option<Instant>,
    monitors: Option<MonitorSignature>,
    next_monitor_check: Instant,
    /// Time of the last rendered frame (render watchdog).
    last_frame: Instant,
    /// Number of watchdog restarts (diagnostics).
    watchdog_restarts: u64,
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
            lyrics_ui: LyricsUi::new(),
            clock: None,
            background_image: None,
            background_key: None,
            desktop_capture: None,
            image_note: None,
            update: UpdateView::Idle,
            release: None,
            update_check_due: Some(now + UPDATE_CHECK_DELAY),
            monitors: None,
            next_monitor_check: now,
            last_frame: now,
            watchdog_restarts: 0,
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
        // Keep the Run entry in sync with the setting and this exe's path.
        crate::autostart::apply(self.settings.autostart);
        crate::update::cleanup_old_exe();
        crate::render::capture::restore_icons_after_crash(&self.icons_marker());

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
        self.ui
            .clock
            .set(f32::from(u8::from(self.settings.show_clock)));
        if self.args.demo {
            self.ui.idle_views =
                IdleScheduler::from_clock().with_interval(crate::ui::idle::DEMO_INTERVAL);
        }
        self.lyrics_ui = LyricsUi::new();
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
        self.request_background();
        // Grace period for GPU setup before the render watchdog may act.
        self.last_frame = Instant::now() + Duration::from_secs(1);
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
        // Capture first: it uses the GPU device and restores the icons.
        self.desktop_capture = None;
        self.windows.clear();
        self.gpu = None;
        // No background image in memory while idle (SPEC §3 idle target).
        self.background_image = None;
        self.background_key = None;
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
            match Surface::new(gpu, hwnd, size.width, size.height, scale, w.transparent) {
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
        self.desktop_capture = None;
        for w in &mut self.windows {
            w.surface = None;
        }
        self.gpu = None;
        self.ensure_surfaces();
        // The live capture belongs to the old device: start it again.
        self.background_key = None;
        self.request_background();
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
                    &self.update,
                    self.image_note.as_deref(),
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
            PanelEvent::CheckUpdates => self.start_update_check(false),
            PanelEvent::InstallUpdate => self.start_update_install(),
            PanelEvent::PickBackground => self.pick_background(),
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
        // Window layout changes: monitors, transparency (other swap chain
        // and window style), taskbar (other rectangle).
        let transparent = |s: &Settings| s.background.mode == BackgroundMode::Transparent;
        let rebuild = new.monitors != old.monitors
            || transparent(&new) != transparent(old)
            || new.background.keep_taskbar != old.background.keep_taskbar;
        if rebuild && self.mode == Mode::Active {
            self.build_windows(event_loop);
            if let Some(sw) = &self.settings_window {
                sw.set_topmost(true);
            }
        }
        if new.background != old.background {
            self.request_background();
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
        if new.autostart != old.autostart {
            crate::autostart::apply(new.autostart);
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

    // ----- background (SPEC §5.1 v1.5) -----

    /// Exists while desktop icons are hidden for the live wallpaper.
    fn icons_marker(&self) -> std::path::PathBuf {
        self.paths.config_dir.join("icons_hidden")
    }

    fn background_key(&self) -> BackgroundKey {
        let b = &self.settings.background;
        let file = (b.mode == BackgroundMode::Image)
            .then(|| b.image_file.clone())
            .flatten();
        (b.mode, file)
    }

    /// Loads the background image for the current setting off the UI
    /// thread (only while the mode runs).
    fn request_background(&mut self) {
        if self.mode != Mode::Active {
            return;
        }
        let key = self.background_key();
        if self.background_key.as_ref() == Some(&key) {
            return;
        }
        self.background_key = Some(key.clone());
        self.desktop_capture = None;
        // Live wallpaper (Wallpaper Engine & co.) first, static image else.
        if key.0 == BackgroundMode::Wallpaper
            && let Some(gpu) = &self.gpu
        {
            match DesktopCapture::start(gpu, self.icons_marker()) {
                Ok(Some(c)) => {
                    self.desktop_capture = Some(c);
                    self.background_image = None;
                    return;
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "live wallpaper capture failed, using the image")
                }
            }
        }
        let path = match &key {
            (BackgroundMode::Wallpaper, _) => crate::background::wallpaper_path(),
            (BackgroundMode::Image, Some(file)) => Some(self.paths.config_dir.join(file)),
            _ => None,
        };
        let Some(path) = path else {
            self.background_image = None;
            return;
        };
        let proxy = self.proxy.clone();
        let spawned = std::thread::Builder::new()
            .name("background".into())
            .spawn(move || {
                let image = crate::background::load(&path);
                let _ = proxy.send_event(UserEvent::Background { key, image });
            });
        if let Err(e) = spawned {
            tracing::warn!(error = %e, "could not spawn background loader");
        }
    }

    /// File dialog for a custom background; the import (size check, copy)
    /// runs on a worker thread.
    fn pick_background(&mut self) {
        let Some(src) = crate::background::pick_file(
            self.i18n.t("settings.bg_pick_title"),
            self.i18n.t("settings.bg_filter"),
        ) else {
            return;
        };
        let dir = self.paths.config_dir.clone();
        let proxy = self.proxy.clone();
        let spawned = std::thread::Builder::new()
            .name("background-import".into())
            .spawn(move || {
                let result = crate::background::import(&src, &dir);
                let _ = proxy.send_event(UserEvent::BackgroundImported(result));
            });
        if let Err(e) = spawned {
            tracing::warn!(error = %e, "could not spawn background import");
        }
    }

    fn on_background_imported(
        &mut self,
        result: Result<String, ImportError>,
        event_loop: &ActiveEventLoop,
    ) {
        match result {
            Ok(file) => {
                self.image_note = None;
                let old = self.settings.clone();
                self.settings.background.image_file = Some(file);
                self.settings.background.mode = BackgroundMode::Image;
                // Same file name as before: force a reload of the new image.
                self.background_key = None;
                self.apply_settings(&old, event_loop);
            }
            Err(ImportError::TooSmall { width, height }) => {
                self.image_note = Some(self.i18n.tf(
                    "settings.bg_too_small",
                    &[
                        ("width", &width.to_string()),
                        ("height", &height.to_string()),
                    ],
                ));
            }
            Err(ImportError::Unreadable) => {
                self.image_note = Some(self.i18n.t("settings.bg_unreadable").to_string());
            }
        }
        if let Some(sw) = &self.settings_window {
            sw.request_redraw();
        }
    }

    // ----- updates (SPEC §6.5 v1.5) -----

    fn start_update_check(&mut self, silent: bool) {
        if matches!(self.update, UpdateView::Checking | UpdateView::Installing) {
            return;
        }
        if !silent {
            self.update = UpdateView::Checking;
        }
        let proxy = self.proxy.clone();
        let spawned = std::thread::Builder::new()
            .name("update-check".into())
            .spawn(move || {
                let result = crate::update::check();
                let _ = proxy.send_event(UserEvent::UpdateChecked { silent, result });
            });
        if let Err(e) = spawned {
            tracing::warn!(error = %e, "could not spawn update check");
        }
        if let Some(sw) = &self.settings_window {
            sw.request_redraw();
        }
    }

    fn on_update_checked(&mut self, silent: bool, result: Result<Option<Release>, String>) {
        match result {
            Ok(Some(release)) => {
                let version = release.version_text();
                tracing::info!(%version, "update available");
                if silent && let Some(tray) = &self.tray {
                    tray.notify_info(
                        self.i18n.t("notify.update_title"),
                        &self.i18n.tf("notify.update_body", &[("version", &version)]),
                    );
                }
                self.update = UpdateView::Available(version);
                self.release = Some(release);
            }
            Ok(None) => {
                tracing::info!("no update available");
                if !silent {
                    self.update = UpdateView::UpToDate;
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "update check failed");
                if !silent {
                    self.update = UpdateView::Failed(e);
                }
            }
        }
        if let Some(sw) = &self.settings_window {
            sw.request_redraw();
        }
    }

    fn start_update_install(&mut self) {
        let Some(release) = self.release.clone() else {
            return;
        };
        self.update = UpdateView::Installing;
        let portable = self.paths.portable;
        let proxy = self.proxy.clone();
        let spawned = std::thread::Builder::new()
            .name("update-install".into())
            .spawn(move || {
                let result = crate::update::install(&release, portable);
                let _ = proxy.send_event(UserEvent::UpdateInstalled(result));
            });
        if let Err(e) = spawned {
            self.update = UpdateView::Failed(e.to_string());
        }
        if let Some(sw) = &self.settings_window {
            sw.request_redraw();
        }
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
            let key_for_roll = key.clone();
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
                vinyl: self.args.demo_records
                    || vinyl::roll(hash_of((&key_for_roll, SystemTime::now()))),
                vinyl_mix: 0.0,
            });
            self.track_changed_at = Instant::now();
            self.ui.track.set(0.0);
            // No lyrics for ads (no title/artist) or when disabled (SPEC §5.6).
            let eligible = self.settings.lyrics.enabled
                && !self.args.demo_records
                && !s.title.trim().is_empty()
                && !s.artist.trim().is_empty();
            self.lyrics.track_changed(self.track_changed_at, eligible);
            if eligible && self.demo.is_some() {
                self.lyrics.set_found(lrc::parse(DEMO_LYRICS));
            }
            // The new layer fades in, so its record blend starts settled.
            let vinyl = self.vinyl_target();
            self.ui.vinyl.set(f32::from(u8::from(vinyl)));
            if let Some(c) = self.current.as_mut() {
                c.vinyl_mix = f32::from(u8::from(vinyl));
            }
            tracing::debug!(record = vinyl, "record roll");
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
                    tracing::debug!(comets = p.comets, "cover palette");
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

    /// Palette for the frame: cover colors (one comet per dominant color),
    /// fixed palette, or fallback (single comet).
    fn target_palette(&self) -> Palette {
        let f = &self.settings.frame;
        match (f.color_mode, &self.cover_palette) {
            (ColorMode::Cover, Some(p)) => p.clone(),
            _ => Palette::single(
                preset_colors(f.palette, f.custom_colors)
                    .map(Oklab::from_rgb)
                    .to_vec(),
            ),
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
        self.ui.view.value() < 0.5 && self.media.session.is_some()
    }

    fn on_cursor_moved(&mut self, id: WindowId, pos: PhysicalPosition<f64>) {
        let scale = self
            .windows
            .iter()
            .find(|w| w.id() == id)
            .map_or(1.0, |w| w.window.scale_factor());
        let dist = self
            .ui
            .last_cursor
            .map_or(f64::MAX, |l| (l.x - pos.x).hypot(l.y - pos.y));
        self.ui.last_cursor = Some(pos);
        if dist > MOUSE_JITTER {
            // After a long rest only a clear movement wakes (SPEC §5.2 v1.5).
            let now = Instant::now();
            let rest = now.saturating_duration_since(self.ui.last_activity);
            let delay = Duration::from_secs(self.settings.cover_view_delay_s as u64);
            let needed = wake_distance(rest, delay);
            // Capped per event: jumps between monitor windows (different
            // client coordinates) must not count as a large movement.
            let dip = ((dist / scale) as f32).min(30.0);
            if self.ui.wake.moved(now, dip, needed) {
                self.ui.last_activity = now;
                self.set_cursor_hidden(false);
            }
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
        let view = self.ui.view.update(now, f32::from(u8::from(cover_view)));
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
        if cover_view && view >= 0.98 {
            self.set_cursor_hidden(true);
        }
        if self.ui.hover.is_some() && !self.interactive() {
            self.update_hover();
        }
        let intro = self.ui.intro.update(now, 1.0);
        let track_t = self.ui.track.update(now, 1.0);

        // ----- lyrics -----
        self.request_lyrics(now);
        let ls = &self.settings.lyrics;
        let (show_player, show_cover) = (ls.enabled && ls.in_player, ls.enabled && ls.in_cover);
        let split_target = self.lyrics.split_target(now, show_player);
        let split = self
            .lyrics_ui
            .split
            .update(now, f32::from(u8::from(split_target)));
        self.lyrics_ui.split_value = split;
        let found = self.lyrics.lyrics().cloned();
        let cover_lyrics = self
            .lyrics_ui
            .cover
            .update(now, f32::from(u8::from(show_cover && found.is_some())));
        let lyrics_pos = self.media.session.as_ref().map(|s| {
            let pos = s.clock.position(SystemTime::now());
            let offset = Duration::from_millis(ls.offset_ms.unsigned_abs() as u64);
            // Positive offset shows the lyrics earlier.
            if ls.offset_ms >= 0 {
                pos + offset
            } else {
                pos.saturating_sub(offset)
            }
        });
        let line_state = match (&found, lyrics_pos) {
            (Some(l), Some(pos)) => Some((l.id, state_at(&l.lines, pos))),
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
            self.lyrics_ui.line_change.set(0.0);
        }
        let line_change = self.lyrics_ui.line_change.update(now, 1.0);

        // Idle alternation cover view <-> lyrics focus (SPEC §5.2, §5.4a).
        // The schedule runs whenever the idle view is shown; the focus view
        // only appears while lyrics are available.
        self.ui.focus_lyrics = ls.enabled && self.lyrics.available_held(now, self.ui.focus_lyrics);
        let variant = self.ui.idle_views.update(now, cover_view);
        // Messages style replaces the classic idle views (SPEC §5.4b).
        let chat_target = ls.style == LyricsStyle::Messages && self.ui.focus_lyrics;
        let chat_mix = self
            .lyrics_ui
            .chat_mix
            .update(now, f32::from(u8::from(chat_target)));
        if let (Some(l), Some((_, st))) = (&found, line_state) {
            let rebuilt = self.lyrics_ui.thread.update(now, l.id, &l.lines, st);
            let newest = self.lyrics_ui.thread.entries().last().map(|e| e.seq as f32);
            if let Some(seq) = newest
                && rebuilt
            {
                self.lyrics_ui.chat_scroll.set(seq);
            }
        }
        let chat_scroll = match self.lyrics_ui.thread.entries().last() {
            Some(e) => self.lyrics_ui.chat_scroll.update(now, e.seq as f32),
            None => self.lyrics_ui.chat_scroll.value(),
        };
        let stars_target = ls.style == LyricsStyle::Constellation && self.ui.focus_lyrics;
        let stars_mix = self
            .lyrics_ui
            .stars_mix
            .update(now, f32::from(u8::from(stars_target)));
        let chain_target = ls.style == LyricsStyle::ConstellationV2 && self.ui.focus_lyrics;
        let chain_mix = self
            .lyrics_ui
            .chain_mix
            .update(now, f32::from(u8::from(chain_target)));
        if chain_target || chain_mix > 0.0 {
            match (&found, lyrics_pos) {
                (Some(l), Some(pos)) => {
                    let aspect = self.chain_aspect();
                    self.lyrics_ui.chain.update(l.id, &l.lines, pos, aspect);
                    self.lyrics_ui.chain_gone = None;
                }
                _ => {
                    self.lyrics_ui.chain_gone.get_or_insert(now);
                }
            }
        }
        let focus_target = variant == IdleVariant::Focus
            && self.ui.focus_lyrics
            && !chat_target
            && !stars_target
            && !chain_target;
        let focus = self.ui.focus.update(now, f32::from(u8::from(focus_target)));
        let clock = self
            .ui
            .clock
            .update(now, f32::from(u8::from(self.settings.show_clock)));
        // Record view (SPEC §5.4d).
        let vinyl_target = self.vinyl_target();
        let vinyl_mix = self.ui.vinyl.update(now, f32::from(u8::from(vinyl_target)));
        if let Some(c) = self.current.as_mut() {
            c.vinyl_mix = vinyl_mix;
        }
        let dt = now
            .saturating_duration_since(self.ui.turntable_at)
            .as_secs_f32();
        self.ui.turntable_at = now;
        let spinning = self.frame_state() == FrameState::Playing;
        let spin = self.ui.turntable.update(dt, spinning);

        UiFrame {
            view,
            pause,
            standby,
            press,
            intro,
            track_t,
            split,
            cover_lyrics,
            focus,
            clock,
            spin,
            scroll,
            line_state: line_state.map(|(_, st)| st),
            line_change,
            lyrics_pos,
            chat_scroll,
            chat_mix,
            stars_mix,
            chain_mix,
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
            alpha: frame.split.clamp(0.0, 1.0),
        };
        let cover = CoverLyricsView {
            current: text(st.current),
            next,
            previous: text(self.lyrics_ui.prev_line),
            change: frame.line_change,
            gap: st.gap,
            alpha: frame.cover_lyrics.clamp(0.0, 1.0),
        };
        (Some(player), Some(cover))
    }

    /// Messages view model (SPEC §5.4b) from the thread state.
    fn chat_view(&self, frame: &UiFrame, now: Instant) -> Option<ChatView<'_>> {
        if frame.chat_mix <= 0.0 {
            return None;
        }
        let l = self.lyrics.lyrics()?;
        let thread = &self.lyrics_ui.thread;
        let current = frame.line_state.and_then(|st| st.current);
        let bubbles = thread
            .entries()
            .iter()
            .map(|e| {
                let appear = spring_step(
                    now.saturating_duration_since(e.at).as_secs_f32(),
                    0.42,
                    0.62,
                );
                let Some(i) = e.line else {
                    return ChatBubble {
                        seq: e.seq,
                        text: "",
                        side: e.side,
                        kind: BubbleKind::Typing,
                        appear,
                        reveal: None,
                    };
                };
                let line = &l.lines[i];
                // Only the line being sung reveals word by word.
                let reveal = match (current, frame.lyrics_pos) {
                    (Some(c), Some(pos)) if c == i => {
                        let next = l.lines.get(i + 1).map(|n| n.time.saturating_sub(line.time));
                        Some(reveal_words(
                            &line.text,
                            pos.saturating_sub(line.time),
                            next,
                        ))
                    }
                    _ => None,
                };
                let kind = match thread.map(i) {
                    Some(m) => BubbleKind::Map {
                        label: m.label,
                        seed: m.seed,
                    },
                    None => BubbleKind::Text,
                };
                ChatBubble {
                    seq: e.seq,
                    text: &line.text,
                    side: e.side,
                    kind,
                    appear,
                    reveal,
                }
            })
            .collect();
        let track = self.current.as_ref();
        let avatar = track.and_then(|t| {
            let thumb = t.thumb_key.and_then(|k| self.thumbs.get(&k));
            t.album_key
                .and_then(|k| self.hd_images.get(&k))
                .or(thumb.and_then(|i| i.image.as_ref()))
        });
        Some(ChatView {
            bubbles,
            scroll: frame.chat_scroll,
            title: track.map_or("", |t| t.title.as_str()),
            artist: track.map_or("", |t| t.artist.as_str()),
            avatar,
            maps_label: self.i18n.t("chat.maps"),
        })
    }

    /// Constellation view model (SPEC §5.4c).
    fn stars_view(&self, frame: &UiFrame) -> Option<StarsView<'_>> {
        if frame.stars_mix <= 0.0 {
            return None;
        }
        let l = self.lyrics.lyrics()?;
        let st = frame.line_state?;
        let text = |i: Option<usize>| {
            i.and_then(|i| l.lines.get(i))
                .map_or("", |x| x.text.as_str())
        };
        let seed = |i: Option<usize>| hash_of((l.id, i));
        let revealed = match (st.current, frame.lyrics_pos) {
            (Some(i), Some(pos)) => {
                let line = &l.lines[i];
                let next = l.lines.get(i + 1).map(|n| n.time.saturating_sub(line.time));
                reveal_words(&line.text, pos.saturating_sub(line.time), next)
            }
            _ => 0.0,
        };
        Some(StarsView {
            current: text(st.current),
            current_seed: seed(st.current),
            revealed,
            previous: text(self.lyrics_ui.prev_line),
            previous_seed: seed(self.lyrics_ui.prev_line),
            change: frame.line_change,
            gap: st.gap,
        })
    }

    /// Aspect ratio for placing chain stars: the first animation monitor.
    fn chain_aspect(&self) -> f32 {
        self.windows
            .iter()
            .find_map(|w| w.surface.as_ref())
            .map_or(16.0 / 9.0, |s| {
                let (w, h) = s.size_dip();
                w / h.max(1.0)
            })
    }

    /// Constellation V2 view model (SPEC §5.4c2).
    fn chain_view(&self, frame: &UiFrame, now: Instant) -> Option<ChainView<'_>> {
        if frame.chain_mix <= 0.0 {
            return None;
        }
        let c = &self.lyrics_ui.chain;
        if c.is_empty() {
            return None;
        }
        // Song change: the old chain fades out.
        let alpha = self.lyrics_ui.chain_gone.map_or(1.0, |t| {
            1.0 - now.saturating_duration_since(t).as_secs_f32() / 0.8
        });
        if alpha <= 0.0 {
            return None;
        }
        let newest = c.newest();
        let stars = c
            .placed()
            .filter_map(|p| {
                let w = c.word(p.k)?;
                Some(ChainStar {
                    text: &w.text,
                    pos: p.pos,
                    age: c.age(p.k),
                    fade: c.fade(p.k),
                    keyword: w.keyword,
                    current: newest == Some(p.k),
                    linked: p.linked,
                    k: p.k,
                })
            })
            .collect();
        Some(ChainView {
            stars,
            newest: newest.unwrap_or(0),
            gap: frame.line_state.is_some_and(|s| s.gap),
            alpha: alpha.clamp(0.0, 1.0),
        })
    }

    /// Keys 1-9: lyrics style by number (SPEC §4 v1.5, saved).
    fn select_lyrics_style(&mut self, style: LyricsStyle) {
        let l = &mut self.settings.lyrics;
        if l.style == style {
            return;
        }
        l.style = style;
        tracing::info!(style = ?l.style, "lyrics style");
        self.save_due = Some(Instant::now() + SAVE_DELAY);
        if let Some(sw) = &self.settings_window {
            sw.request_redraw();
        }
    }

    /// Refreshes the cached clock line (once per minute or language change).
    fn update_clock(&mut self) {
        let t = LocalTime::now();
        let lang = self.i18n.language();
        if self
            .clock
            .as_ref()
            .is_some_and(|(ct, cl, _, _)| *ct == t && *cl == lang)
        {
            return;
        }
        let f = ClockFormat {
            weekdays: self.i18n.t("clock.weekdays"),
            months: self.i18n.t("clock.months"),
            date: self.i18n.t("clock.date"),
            hours: self.i18n.t("clock.hours"),
        };
        let (time, date) = clock::format(t, &f);
        self.clock = Some((t, lang, time, date));
    }

    /// Record instead of cover: this song's coin flip, and only once the
    /// lyrics are known to be missing (not while they load).
    fn vinyl_target(&self) -> bool {
        self.current.as_ref().is_some_and(|c| c.vinyl) && self.lyrics.missing()
    }

    fn track_view<'a>(&'a self, t: &'a TrackDisplay, alpha: f32, scale: f32) -> TrackView<'a> {
        let thumb = t.thumb_key.and_then(|k| self.thumbs.get(&k));
        let hd = t.album_key.and_then(|k| self.hd_images.get(&k));
        TrackView {
            title: &t.title,
            artist: &t.artist,
            image: hd.or(thumb.and_then(|i| i.image.as_ref())),
            small: thumb.and_then(|i| i.small.as_ref()),
            alpha,
            scale,
            vinyl: t.vinyl_mix,
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
        self.last_frame = now;
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
        self.update_clock();
        // Newest live wallpaper frame (shared by all monitors).
        if let (Some(c), Some(gpu)) = (self.desktop_capture.as_mut(), self.gpu.as_ref()) {
            c.poll(gpu);
        }
        // Part of the captured desktop behind this window (physical pixels).
        let live_src = self.desktop_capture.as_ref().and_then(|c| {
            let w = self.windows.iter().find(|w| w.id() == id)?;
            let (ox, oy) = c.origin();
            let (pos, size) = w.rect;
            Some(layout::Rect::new(
                (pos.x - ox) as f32,
                (pos.y - oy) as f32,
                size.width as f32,
                size.height as f32,
            ))
        });

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

        // Track change: the old cover fades while growing a little, the new
        // one springs in from slightly smaller (overshoot comes from the spring).
        let t = frame.track_t;
        let fade = t.clamp(0.0, 1.0);
        let mut tracks = Vec::with_capacity(2);
        if fade < 1.0
            && let Some(p) = &self.previous
        {
            tracks.push(self.track_view(p, 1.0 - fade, 1.0 + 0.04 * fade));
        }
        if let Some(c) = &self.current {
            tracks.push(self.track_view(c, fade, 0.93 + 0.07 * t));
        }
        let (lyrics, cover_lyrics) = self.lyrics_views(&frame);
        let model = UiModel {
            time: now
                .saturating_duration_since(self.track_changed_at)
                .as_secs_f32(),
            player: layout::player_mixed(size_dip.0, size_dip.1, frame.split.clamp(0.0, 1.0)),
            intro: frame.intro,
            lyrics,
            cover_lyrics,
            cover_lyrics_mix: frame.cover_lyrics,
            view: frame.view,
            focus: frame.focus,
            spin: frame.spin,
            chat: self.chat_view(&frame, now),
            chat_mix: frame.chat_mix,
            stars: self.stars_view(&frame),
            stars_mix: frame.stars_mix,
            chain: self.chain_view(&frame, now),
            chain_mix: frame.chain_mix,
            background: self.background_image.as_ref(),
            live_background: self
                .desktop_capture
                .as_ref()
                .and_then(|c| c.bitmap())
                .zip(live_src),
            background_dim: match self.settings.background.mode {
                BackgroundMode::Black => 0.0,
                _ => self.settings.background.dim as f32 / 100.0,
            },
            clock: frame.clock,
            clock_time: self.clock.as_ref().map_or("", |c| c.2.as_str()),
            clock_date: self.clock.as_ref().map_or("", |c| c.3.as_str()),
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

    /// Size/position/DPI event of a fullscreen window: restore the exact
    /// monitor rectangle, then repaint (black) or resize the swap chain.
    fn on_geometry_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId) {
        if self.mode != Mode::Active {
            return;
        }
        if let Some(w) = self.window(id) {
            w.enforce_rect();
            w.window.request_redraw();
        }
        self.resize(id);
        self.check_monitors(event_loop);
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
            UserEvent::UpdateChecked { silent, result } => self.on_update_checked(silent, result),
            UserEvent::UpdateInstalled(Ok(())) => {
                tracing::info!("update handed over to the installer, quitting");
                self.quit(event_loop);
            }
            UserEvent::UpdateInstalled(Err(e)) => {
                tracing::warn!(error = %e, "update failed");
                self.update = UpdateView::Failed(e);
                if let Some(sw) = &self.settings_window {
                    sw.request_redraw();
                }
            }
            UserEvent::Background { key, image } => {
                if self.mode == Mode::Active && self.background_key.as_ref() == Some(&key) {
                    self.background_image = image;
                }
            }
            UserEvent::BackgroundImported(result) => {
                self.on_background_imported(result, event_loop);
            }
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
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed
                    && !event.repeat
                    && digit_style(&event.logical_key).is_some() =>
            {
                if let Some(style) = digit_style(&event.logical_key) {
                    self.select_lyrics_style(style);
                }
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
            WindowEvent::ScaleFactorChanged {
                mut inner_size_writer,
                ..
            } => {
                // Keep the physical monitor size instead of winit's DPI resize.
                if let Some(w) = self.window(id) {
                    let _ = inner_size_writer.request_inner_size(w.rect.1);
                }
                self.on_geometry_event(event_loop, id);
            }
            WindowEvent::Resized(_) | WindowEvent::Moved(_) => {
                self.on_geometry_event(event_loop, id);
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
        if self.update_check_due.is_some_and(|t| now >= t) {
            self.update_check_due = None;
            self.start_update_check(true);
        }
        if self.trim_due.is_some_and(|t| now >= t) {
            self.trim_due = None;
            if self.mode == Mode::Idle && self.settings_window.is_none() {
                fullscreen::trim_working_set();
            }
        }
        // Render watchdog: restart a stalled redraw chain.
        let rendering =
            self.mode == Mode::Active && self.windows.iter().any(|w| w.surface.is_some());
        if rendering && now.saturating_duration_since(self.last_frame) >= RENDER_WATCHDOG {
            self.watchdog_restarts += 1;
            let n = self.watchdog_restarts;
            if n <= 3 || n.is_multiple_of(100) {
                tracing::warn!(
                    restarts = n,
                    "render watchdog: redraw chain stalled, restarted"
                );
            }
            self.last_frame = now;
            for w in self.windows.iter().filter(|w| w.surface.is_some()) {
                w.window.request_redraw();
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
            rendering.then_some(self.last_frame + RENDER_WATCHDOG),
            self.save_due,
            self.trim_due,
            self.update_check_due,
            repaint.filter(|t| *t > now),
        ];
        match deadlines.into_iter().flatten().min() {
            Some(t) => event_loop.set_control_flow(ControlFlow::WaitUntil(t)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }
}
