//! Settings model (SPEC §7). Unknown fields are ignored, missing fields get
//! defaults (`#[serde(default)]`), out-of-range values are clamped.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub const CURRENT_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    De,
    En,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MonitorMode {
    Animation,
    Black,
    Free,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorMode {
    Cover,
    Palette,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PalettePreset {
    NeonPink,
    Ocean,
    Sunset,
    Mono,
    Custom,
}

/// sRGB color, serialized as `#RRGGBB`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub fn parse_hex(s: &str) -> Option<Self> {
        let h = s.trim().strip_prefix('#')?;
        if h.len() != 6 || !h.is_ascii() {
            return None;
        }
        let c = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
        Some(Self(c(0)?, c(2)?, c(4)?))
    }
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02X}{:02X}{:02X}", self.0, self.1, self.2)
    }
}

impl Serialize for Rgb {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Rgb {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Rgb::parse_hex(&s).ok_or_else(|| serde::de::Error::custom(format!("invalid color {s}")))
    }
}

/// SPEC §6.3 fallback palette.
pub const FALLBACK_PALETTE: [Rgb; 3] = [
    Rgb(0xFF, 0x2E, 0xA6),
    Rgb(0xB0, 0x26, 0xFF),
    Rgb(0x5B, 0x3C, 0xFF),
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FrameSettings {
    /// Percent 0..=100.
    pub brightness: u8,
    pub width: u8,
    pub speed: u8,
    pub beat: u8,
    pub color_mode: ColorMode,
    pub palette: PalettePreset,
    pub custom_colors: [Rgb; 3],
}

impl Default for FrameSettings {
    fn default() -> Self {
        Self {
            brightness: 80,
            width: 50,
            speed: 40,
            beat: 60,
            color_mode: ColorMode::Cover,
            palette: PalettePreset::NeonPink,
            custom_colors: FALLBACK_PALETTE,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LyricsSettings {
    pub enabled: bool,
    pub in_player: bool,
    pub in_cover: bool,
    pub offset_ms: i32,
}

impl Default for LyricsSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            in_player: true,
            in_cover: true,
            offset_ms: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    /// `None` = follow the system language.
    pub language: Option<Language>,
    /// Hotkey in `global-hotkey` syntax, e.g. `Ctrl+Alt+L`.
    pub hotkey: String,
    /// Monitor device name (e.g. `\\.\DISPLAY1`) -> mode. Monitors not listed
    /// use the default: primary = Animation, others = Black.
    pub monitors: BTreeMap<String, MonitorMode>,
    pub cover_view_delay_s: u32,
    /// Clock line with date and time at the top (v1.2).
    pub show_clock: bool,
    pub frame: FrameSettings,
    pub lyrics: LyricsSettings,
    pub hd_cover: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            language: None,
            hotkey: "Ctrl+Alt+L".into(),
            monitors: BTreeMap::new(),
            cover_view_delay_s: 5,
            show_clock: true,
            frame: FrameSettings::default(),
            lyrics: LyricsSettings::default(),
            hd_cover: true,
        }
    }
}

impl Settings {
    pub fn monitor_mode(&self, name: &str, is_primary: bool) -> MonitorMode {
        self.monitors.get(name).copied().unwrap_or(if is_primary {
            MonitorMode::Animation
        } else {
            MonitorMode::Black
        })
    }

    /// Clamps values into their valid ranges and applies migrations.
    pub fn sanitize(&mut self) {
        // Migrations from older versions go here (none yet).
        self.version = CURRENT_VERSION;
        self.cover_view_delay_s = self.cover_view_delay_s.clamp(2, 60);
        let f = &mut self.frame;
        for v in [&mut f.brightness, &mut f.width, &mut f.speed, &mut f.beat] {
            *v = (*v).min(100);
        }
        self.lyrics.offset_ms = self.lyrics.offset_ms.clamp(-2000, 2000);
        if self.hotkey.trim().is_empty() {
            self.hotkey = Settings::default().hotkey;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_hex_roundtrip() {
        assert_eq!(Rgb::parse_hex("#ff2ea6"), Some(Rgb(0xFF, 0x2E, 0xA6)));
        assert_eq!(Rgb(1, 2, 255).to_string(), "#0102FF");
        assert_eq!(Rgb::parse_hex("ff2ea6"), None);
        assert_eq!(Rgb::parse_hex("#ff2ea"), None);
    }

    #[test]
    fn monitor_defaults() {
        let mut s = Settings::default();
        assert_eq!(s.monitor_mode("A", true), MonitorMode::Animation);
        assert_eq!(s.monitor_mode("B", false), MonitorMode::Black);
        s.monitors.insert("B".into(), MonitorMode::Free);
        assert_eq!(s.monitor_mode("B", false), MonitorMode::Free);
    }

    #[test]
    fn sanitize_clamps() {
        let mut s = Settings {
            cover_view_delay_s: 0,
            hotkey: " ".into(),
            ..Default::default()
        };
        s.frame.brightness = 250;
        s.lyrics.offset_ms = -9000;
        s.version = 0;
        s.sanitize();
        assert_eq!(s.cover_view_delay_s, 2);
        assert_eq!(s.frame.brightness, 100);
        assert_eq!(s.lyrics.offset_ms, -2000);
        assert_eq!(s.hotkey, "Ctrl+Alt+L");
        assert_eq!(s.version, CURRENT_VERSION);
    }

    #[test]
    fn clock_defaults_on_for_old_files() {
        // Settings files from v1.1 have no `show_clock` field.
        let s: Settings =
            serde_json::from_str(r#"{"version":1,"cover_view_delay_s":7}"#).unwrap_or_default();
        assert!(s.show_clock);
        assert_eq!(s.cover_view_delay_s, 7);
    }
}
