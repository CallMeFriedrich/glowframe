//! Contents of the settings window (SPEC §7). Edits `Settings` in place; the
//! app applies and saves changes immediately.

use egui::{Color32, RichText, Ui};

use super::hotkey_rec::{self, RecordError};
use crate::i18n::I18n;
use crate::settings::model::{ColorMode, Language, PalettePreset, Rgb};
use crate::settings::{MonitorMode, Settings};

pub const ACCENT: Color32 = Color32::from_rgb(0xFF, 0x2E, 0xA6);
const MUTED: Color32 = Color32::from_rgb(0x9A, 0x98, 0xA6);
const WARN: Color32 = Color32::from_rgb(0xFF, 0xB0, 0x3B);

#[derive(Debug, Clone)]
pub struct MonitorInfo {
    pub id: String,
    pub label: String,
    pub primary: bool,
}

pub struct PanelInfo<'a> {
    pub monitors: &'a [MonitorInfo],
    pub hotkey_error: Option<&'a str>,
    pub recording: bool,
    pub record_error: Option<RecordError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelEvent {
    StartRecording,
    Recorded(String),
    CancelRecording,
    RecordFailed(RecordError),
}

pub fn show(ui: &mut Ui, s: &mut Settings, i18n: &I18n, info: &PanelInfo) -> Vec<PanelEvent> {
    let mut events = Vec::new();
    let german = i18n.language() == Language::De;
    if info.recording {
        record_keys(ui, &mut events);
    }

    // Header: title left, language switch right.
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("Glowframe · {}", i18n.t("settings.title")))
                .size(22.0)
                .strong(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            for (lang, label) in [(Language::En, "EN"), (Language::De, "DE")] {
                let active = i18n.language() == lang;
                if ui
                    .selectable_label(active, RichText::new(label).strong())
                    .clicked()
                {
                    s.language = Some(lang);
                }
                if lang == Language::En {
                    ui.label(RichText::new("|").color(MUTED));
                }
            }
        });
    });
    ui.label(
        RichText::new(i18n.t("settings.autosave"))
            .color(MUTED)
            .size(13.0),
    );
    ui.add_space(8.0);

    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            section(ui, i18n.t("settings.general"), |ui| {
                grid(ui, "general", |ui| {
                    ui.label(i18n.t("settings.hotkey"));
                    ui.horizontal(|ui| {
                        if info.recording {
                            ui.label(
                                RichText::new(i18n.t("settings.hotkey_recording")).color(ACCENT),
                            );
                        } else {
                            ui.label(
                                RichText::new(hotkey_rec::display(&s.hotkey, german)).strong(),
                            );
                            if ui.button(i18n.t("settings.hotkey_change")).clicked() {
                                events.push(PanelEvent::StartRecording);
                            }
                        }
                    });
                    ui.end_row();
                });
                let note = match info.record_error {
                    Some(RecordError::NeedModifier) => {
                        Some(i18n.t("settings.hotkey_need_modifier"))
                    }
                    Some(RecordError::Unsupported) => Some(i18n.t("settings.hotkey_unsupported")),
                    None => info.hotkey_error.map(|_| i18n.t("settings.hotkey_taken")),
                };
                if let Some(note) = note {
                    ui.label(RichText::new(note).color(WARN));
                }
            });

            section(ui, i18n.t("settings.monitors"), |ui| {
                grid(ui, "monitors", |ui| {
                    for (i, m) in info.monitors.iter().enumerate() {
                        let mut label =
                            format!("{} {} · {}", i18n.t("settings.monitor"), i + 1, m.label);
                        if m.primary {
                            label.push_str(&format!(" ({})", i18n.t("settings.primary")));
                        }
                        ui.label(label);
                        let mut mode = s.monitor_mode(&m.id, m.primary);
                        let before = mode;
                        egui::ComboBox::from_id_salt(("monitor", &m.id))
                            .selected_text(mode_label(i18n, mode))
                            .show_ui(ui, |ui| {
                                for opt in [
                                    MonitorMode::Animation,
                                    MonitorMode::Black,
                                    MonitorMode::Free,
                                ] {
                                    ui.selectable_value(&mut mode, opt, mode_label(i18n, opt));
                                }
                            });
                        if mode != before {
                            s.monitors.insert(m.id.clone(), mode);
                        }
                        ui.end_row();
                    }
                });
            });

            section(ui, i18n.t("settings.view"), |ui| {
                grid(ui, "view", |ui| {
                    ui.label(i18n.t("settings.cover_delay"));
                    ui.add(egui::Slider::new(&mut s.cover_view_delay_s, 2..=60).suffix(" s"));
                    ui.end_row();
                });
                ui.checkbox(&mut s.show_clock, i18n.t("settings.show_clock"));
            });

            section(ui, i18n.t("settings.frame"), |ui| {
                let f = &mut s.frame;
                grid(ui, "frame", |ui| {
                    for (key, value) in [
                        ("settings.brightness", &mut f.brightness),
                        ("settings.width", &mut f.width),
                        ("settings.speed", &mut f.speed),
                        ("settings.beat", &mut f.beat),
                    ] {
                        ui.label(i18n.t(key));
                        ui.add(egui::Slider::new(value, 0..=100).suffix(" %"));
                        ui.end_row();
                    }
                    ui.label(i18n.t("settings.color_mode"));
                    ui.horizontal(|ui| {
                        ui.radio_value(
                            &mut f.color_mode,
                            ColorMode::Cover,
                            i18n.t("settings.color_cover"),
                        );
                        ui.radio_value(
                            &mut f.color_mode,
                            ColorMode::Palette,
                            i18n.t("settings.color_palette"),
                        );
                    });
                    ui.end_row();
                    if f.color_mode == ColorMode::Palette {
                        ui.label(i18n.t("settings.palette"));
                        egui::ComboBox::from_id_salt("palette")
                            .selected_text(preset_label(i18n, f.palette))
                            .show_ui(ui, |ui| {
                                for p in [
                                    PalettePreset::NeonPink,
                                    PalettePreset::Ocean,
                                    PalettePreset::Sunset,
                                    PalettePreset::Mono,
                                    PalettePreset::Custom,
                                ] {
                                    ui.selectable_value(&mut f.palette, p, preset_label(i18n, p));
                                }
                            });
                        ui.end_row();
                        if f.palette == PalettePreset::Custom {
                            ui.label("");
                            ui.horizontal(|ui| {
                                for c in &mut f.custom_colors {
                                    let mut rgb = [c.0, c.1, c.2];
                                    if egui::color_picker::color_edit_button_srgb(ui, &mut rgb)
                                        .changed()
                                    {
                                        *c = Rgb(rgb[0], rgb[1], rgb[2]);
                                    }
                                }
                            });
                            ui.end_row();
                        }
                    }
                });
            });

            section(ui, i18n.t("settings.lyrics"), |ui| {
                let l = &mut s.lyrics;
                ui.checkbox(&mut l.enabled, i18n.t("settings.lyrics_enabled"));
                ui.add_enabled_ui(l.enabled, |ui| {
                    ui.indent("lyrics_sub", |ui| {
                        ui.checkbox(&mut l.in_player, i18n.t("settings.lyrics_player"));
                        ui.checkbox(&mut l.in_cover, i18n.t("settings.lyrics_cover"));
                    });
                    egui::CollapsingHeader::new(i18n.t("settings.advanced"))
                        .id_salt("lyrics_advanced")
                        .show(ui, |ui| {
                            grid(ui, "lyrics_adv", |ui| {
                                ui.label(i18n.t("settings.offset"));
                                ui.add(
                                    egui::Slider::new(&mut l.offset_ms, -2000..=2000)
                                        .suffix(" ms")
                                        .step_by(50.0),
                                );
                                ui.end_row();
                            });
                        });
                });
            });

            section(ui, i18n.t("settings.cover"), |ui| {
                ui.checkbox(&mut s.hd_cover, i18n.t("settings.hd_cover"));
            });
        });
    events
}

/// Captures the next key press while recording a hotkey.
fn record_keys(ui: &mut Ui, events: &mut Vec<PanelEvent>) {
    ui.input_mut(|input| {
        let mut done = None;
        input.events.retain(|e| match e {
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                if done.is_none() {
                    done = Some(if *key == egui::Key::Escape {
                        PanelEvent::CancelRecording
                    } else {
                        match hotkey_rec::combo_string(*modifiers, *key) {
                            Ok(s) => PanelEvent::Recorded(s),
                            Err(err) => PanelEvent::RecordFailed(err),
                        }
                    });
                }
                false
            }
            egui::Event::Key { .. } | egui::Event::Text(_) => false,
            _ => true,
        });
        if let Some(e) = done {
            events.push(e);
        }
    });
}

fn section(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui)) {
    ui.add_space(6.0);
    ui.label(RichText::new(title).size(16.0).strong().color(ACCENT));
    egui::Frame::new()
        .fill(Color32::from_rgb(0x1C, 0x1B, 0x22))
        .corner_radius(10.0)
        .inner_margin(14.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

fn grid(ui: &mut Ui, id: &str, add: impl FnOnce(&mut Ui)) {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([24.0, 12.0])
        .min_col_width(170.0)
        .show(ui, add);
}

fn mode_label(i18n: &I18n, m: MonitorMode) -> &str {
    i18n.t(match m {
        MonitorMode::Animation => "settings.mode_animation",
        MonitorMode::Black => "settings.mode_black",
        MonitorMode::Free => "settings.mode_free",
    })
}

fn preset_label(i18n: &I18n, p: PalettePreset) -> &str {
    i18n.t(match p {
        PalettePreset::NeonPink => "settings.preset_neon_pink",
        PalettePreset::Ocean => "settings.preset_ocean",
        PalettePreset::Sunset => "settings.preset_sunset",
        PalettePreset::Mono => "settings.preset_mono",
        PalettePreset::Custom => "settings.preset_custom",
    })
}
