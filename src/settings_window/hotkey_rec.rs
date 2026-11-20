//! Hotkey recorder: egui key events -> `global-hotkey` syntax (`Ctrl+Alt+L`).

use egui::{Key, Modifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordError {
    /// Ctrl or Alt is required so normal typing is never hijacked.
    NeedModifier,
    Unsupported,
}

/// Token of the main key as `global-hotkey` parses it.
fn key_token(key: Key) -> Option<String> {
    let name = key.name();
    let named = match name {
        "Space" | "Enter" | "Tab" | "Backspace" | "Insert" | "Delete" | "Home" | "End"
        | "PageUp" | "PageDown" => Some(name.to_string()),
        "Up" | "ArrowUp" => Some("ArrowUp".into()),
        "Down" | "ArrowDown" => Some("ArrowDown".into()),
        "Left" | "ArrowLeft" => Some("ArrowLeft".into()),
        "Right" | "ArrowRight" => Some("ArrowRight".into()),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphanumeric() => Some(c.to_ascii_uppercase().to_string()),
        (Some('F'), Some(_)) if name[1..].chars().all(|c| c.is_ascii_digit()) => {
            Some(name.to_string())
        }
        _ => None,
    }
}

pub fn combo_string(mods: Modifiers, key: Key) -> Result<String, RecordError> {
    let token = key_token(key).ok_or(RecordError::Unsupported)?;
    if !(mods.ctrl || mods.alt) {
        return Err(RecordError::NeedModifier);
    }
    let mut parts = Vec::new();
    if mods.ctrl {
        parts.push("Ctrl".to_string());
    }
    if mods.alt {
        parts.push("Alt".to_string());
    }
    if mods.shift {
        parts.push("Shift".to_string());
    }
    parts.push(token);
    Ok(parts.join("+"))
}

/// Human readable form; German keyboards say "Strg".
pub fn display(combo: &str, german: bool) -> String {
    if german {
        combo.replace("Ctrl", "Strg")
    } else {
        combo.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use global_hotkey::hotkey::HotKey;

    fn mods(ctrl: bool, alt: bool, shift: bool) -> Modifiers {
        Modifiers {
            ctrl,
            alt,
            shift,
            ..Default::default()
        }
    }

    #[test]
    fn recorded_combos_parse() {
        for (m, k, expect) in [
            (mods(true, true, false), Key::L, "Ctrl+Alt+L"),
            (mods(true, false, true), Key::F5, "Ctrl+Shift+F5"),
            (mods(false, true, false), Key::Num7, "Alt+7"),
            (mods(true, true, false), Key::Space, "Ctrl+Alt+Space"),
            (mods(true, false, false), Key::ArrowUp, "Ctrl+ArrowUp"),
        ] {
            let s = combo_string(m, k).unwrap();
            assert_eq!(s, expect);
            assert!(s.parse::<HotKey>().is_ok(), "{s} must parse");
        }
    }

    #[test]
    fn rejects_without_modifier() {
        assert_eq!(
            combo_string(mods(false, false, true), Key::A),
            Err(RecordError::NeedModifier)
        );
    }

    #[test]
    fn german_display() {
        assert_eq!(display("Ctrl+Alt+L", true), "Strg+Alt+L");
        assert_eq!(display("Ctrl+Alt+L", false), "Ctrl+Alt+L");
    }
}
