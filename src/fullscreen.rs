//! One borderless topmost window per covered monitor (SPEC §5.1).

use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event_loop::ActiveEventLoop;
use winit::monitor::MonitorHandle;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{Fullscreen, Window, WindowId, WindowLevel};

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    BLACK_BRUSH, FillRect, GetDC, GetStockObject, HBRUSH, ReleaseDC, ValidateRect,
};
use windows::Win32::System::Power::{
    ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetClientRect, GetWindowLongPtrW, HWND_TOPMOST, SWP_FRAMECHANGED, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, WS_EX_APPWINDOW,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
};

use crate::settings::{MonitorMode, Settings};

pub struct MonitorWindow {
    pub window: Window,
    pub mode: MonitorMode,
}

impl MonitorWindow {
    pub fn id(&self) -> WindowId {
        self.window.id()
    }
}

/// Identity of the current monitor layout; a change triggers a rebuild.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorSignature(Vec<(String, PhysicalPosition<i32>, PhysicalSize<u32>, bool)>);

pub fn monitor_signature(event_loop: &ActiveEventLoop) -> MonitorSignature {
    let primary = event_loop.primary_monitor();
    let mut v: Vec<_> = event_loop
        .available_monitors()
        .map(|m| {
            let is_primary = primary.as_ref() == Some(&m);
            (monitor_name(&m), m.position(), m.size(), is_primary)
        })
        .collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    MonitorSignature(v)
}

fn monitor_name(m: &MonitorHandle) -> String {
    m.name().unwrap_or_else(|| format!("{:?}", m.position()))
}

/// Creates the windows for all monitors that are not `Free`.
pub fn create_windows(event_loop: &ActiveEventLoop, settings: &Settings) -> Vec<MonitorWindow> {
    let primary = event_loop.primary_monitor();
    let mut out = Vec::new();
    for monitor in event_loop.available_monitors() {
        let name = monitor_name(&monitor);
        let is_primary = primary.as_ref() == Some(&monitor);
        let mode = settings.monitor_mode(&name, is_primary);
        if mode == MonitorMode::Free {
            continue;
        }
        let attrs = Window::default_attributes()
            .with_title("Glowframe")
            .with_decorations(false)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_position(monitor.position())
            .with_inner_size(monitor.size())
            .with_fullscreen(Some(Fullscreen::Borderless(Some(monitor.clone()))));
        match event_loop.create_window(attrs) {
            Ok(window) => {
                if let Some(hwnd) = hwnd(&window) {
                    apply_tool_window_style(hwnd);
                }
                tracing::info!(monitor = %name, ?mode, primary = is_primary, size = ?monitor.size(), "window created");
                window.request_redraw();
                out.push(MonitorWindow { window, mode });
            }
            Err(e) => tracing::error!(error = %e, monitor = %name, "window creation failed"),
        }
    }
    // Keyboard focus (Esc) goes to the first animation window.
    if let Some(w) = out
        .iter()
        .find(|w| w.mode == MonitorMode::Animation)
        .or(out.first())
    {
        focus(&w.window);
    }
    out
}

pub fn hwnd(window: &Window) -> Option<HWND> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(HWND(h.hwnd.get() as *mut _)),
        _ => None,
    }
}

pub fn focus(window: &Window) {
    window.focus_window();
    if let Some(h) = hwnd(window) {
        // SAFETY: valid window handle owned by this thread.
        let _ = unsafe { SetForegroundWindow(h) };
    }
}

/// No taskbar entry (WS_EX_TOOLWINDOW) and topmost.
fn apply_tool_window_style(hwnd: HWND) {
    // SAFETY: valid window handle owned by this thread.
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let ex =
            (ex & !(WS_EX_APPWINDOW.0 as isize)) | (WS_EX_TOOLWINDOW.0 | WS_EX_TOPMOST.0) as isize;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex);
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
}

/// Fills the client area black via GDI. Black monitors are painted once and
/// only again when Windows asks for it (no render loop).
pub fn paint_black(window: &Window) {
    let Some(hwnd) = hwnd(window) else { return };
    // SAFETY: valid window handle; the DC is released before returning.
    unsafe {
        let mut rect = RECT::default();
        if GetClientRect(hwnd, &mut rect).is_err() {
            return;
        }
        let hdc = GetDC(Some(hwnd));
        if hdc.is_invalid() {
            return;
        }
        FillRect(hdc, &rect, HBRUSH(GetStockObject(BLACK_BRUSH).0));
        ReleaseDC(Some(hwnd), hdc);
        let _ = ValidateRect(Some(hwnd), None);
    }
}

/// Prevents screensaver and display sleep while the mode is active.
pub fn keep_awake(on: bool) {
    let flags = if on {
        ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED
    } else {
        ES_CONTINUOUS
    };
    // SAFETY: plain flag call on the event loop thread.
    let prev = unsafe { SetThreadExecutionState(flags) };
    if prev.0 == 0 {
        tracing::warn!(on, "SetThreadExecutionState failed");
    }
}
