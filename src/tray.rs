//! Tray icon and context menu (SPEC §4).

use tray_icon::menu::{Menu, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Shell::{
    NIF_INFO, NIIF_WARNING, NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW,
};

use crate::i18n::I18n;

pub const APP_NAME: &str = "Glowframe";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    Toggle,
    Settings,
    Quit,
}

pub struct Tray {
    icon: TrayIcon,
    toggle: MenuItem,
    settings: MenuItem,
    quit: MenuItem,
    active: bool,
}

impl Tray {
    pub fn new(i18n: &I18n) -> Result<Self, String> {
        let toggle = MenuItem::new(i18n.t("tray.start"), true, None);
        let settings = MenuItem::new(i18n.t("tray.settings"), true, None);
        let quit = MenuItem::new(i18n.t("tray.quit"), true, None);
        let menu = Menu::new();
        menu.append_items(&[&toggle, &settings, &PredefinedMenuItem::separator(), &quit])
            .map_err(|e| e.to_string())?;

        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_tooltip(APP_NAME)
            .with_icon(tray_icon_image()?)
            .build()
            .map_err(|e| e.to_string())?;

        Ok(Self {
            icon,
            toggle,
            settings,
            quit,
            active: false,
        })
    }

    pub fn action(&self, id: &MenuId) -> Option<MenuAction> {
        if id == self.toggle.id() {
            Some(MenuAction::Toggle)
        } else if id == self.settings.id() {
            Some(MenuAction::Settings)
        } else if id == self.quit.id() {
            Some(MenuAction::Quit)
        } else {
            None
        }
    }

    pub fn set_active(&mut self, active: bool, i18n: &I18n) {
        self.active = active;
        self.refresh_texts(i18n);
    }

    pub fn refresh_texts(&self, i18n: &I18n) {
        self.toggle.set_text(i18n.t(if self.active {
            "tray.stop"
        } else {
            "tray.start"
        }));
        self.settings.set_text(i18n.t("tray.settings"));
        self.quit.set_text(i18n.t("tray.quit"));
    }

    /// Tooltip: "Glowframe" plus the current title if Spotify is running.
    #[allow(dead_code)] // used from phase 2 on
    pub fn set_now_playing(&self, title: Option<&str>) {
        let text = match title {
            Some(t) if !t.is_empty() => format!("{APP_NAME} – {t}"),
            _ => APP_NAME.to_string(),
        };
        // The Windows tooltip is limited to 127 characters.
        let text: String = text.chars().take(127).collect();
        if let Err(e) = self.icon.set_tooltip(Some(text)) {
            tracing::warn!(error = %e, "set tooltip failed");
        }
    }

    /// Shows a balloon notification on the tray icon.
    ///
    /// `tray-icon` has no notification API, so we modify its shell icon
    /// directly. It registers icons with `uID` from a counter starting at 1;
    /// we create exactly one icon, hence `uID = 1`.
    pub fn notify(&self, title: &str, text: &str) {
        let mut data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: HWND(self.icon.window_handle()),
            uID: 1,
            uFlags: NIF_INFO,
            dwInfoFlags: NIIF_WARNING,
            ..Default::default()
        };
        copy_wide(&mut data.szInfoTitle, title);
        copy_wide(&mut data.szInfo, text);
        // SAFETY: `data` is fully initialized and lives for the call.
        let ok = unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
        if !ok.as_bool() {
            tracing::warn!("tray notification failed");
        }
    }
}

fn copy_wide(dst: &mut [u16], s: &str) {
    let max = dst.len().saturating_sub(1);
    for (d, c) in dst.iter_mut().zip(s.encode_utf16().take(max)) {
        *d = c;
    }
}

/// App icon from the exe resources (ID 1, see build.rs); Windows picks the
/// matching size. Falls back to a generated ring if the resource is missing.
fn tray_icon_image() -> Result<Icon, String> {
    match Icon::from_resource(1, None) {
        Ok(icon) => Ok(icon),
        Err(e) => {
            tracing::warn!(error = %e, "icon resource unavailable, using generated icon");
            generated_icon(32).map_err(|e| e.to_string())
        }
    }
}

/// Generated glowing ring in the fallback palette.
fn generated_icon(size: u32) -> Result<Icon, tray_icon::BadIcon> {
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    let c = (size as f32 - 1.0) / 2.0;
    let r = size as f32 * 0.36;
    for y in 0..size {
        for x in 0..size {
            let (dx, dy) = (x as f32 - c, y as f32 - c);
            let d = (dx * dx + dy * dy).sqrt();
            let ring = (1.0 - ((d - r).abs() / (size as f32 * 0.11))).clamp(0.0, 1.0);
            // Pink at the top-left, violet at the bottom-right.
            let t = ((dx + dy) / (2.0 * c) * 0.5 + 0.5).clamp(0.0, 1.0);
            let lerp = |a: f32, b: f32| (a + (b - a) * t) as u8;
            rgba.extend_from_slice(&[
                lerp(255.0, 91.0),
                lerp(46.0, 60.0),
                lerp(166.0, 255.0),
                (ring.powf(0.8) * 255.0) as u8,
            ]);
        }
    }
    Icon::from_rgba(rgba, size, size)
}
