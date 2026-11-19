//! Loading and saving `settings.json` (SPEC §7).

use std::path::{Path, PathBuf};

use super::model::Settings;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadOutcome {
    /// No file existed; defaults were written.
    Created,
    Loaded,
    /// The file was unreadable/defective; it was backed up as `.bak`.
    Recovered,
}

/// Loads the settings. Never fails: problems are logged and defaults used.
pub fn load(path: &Path) -> (Settings, LoadOutcome) {
    let (mut settings, outcome, original) = match std::fs::read_to_string(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            (Settings::default(), LoadOutcome::Created, None)
        }
        Err(e) => {
            tracing::warn!(error = %e, "settings unreadable");
            backup(path);
            (Settings::default(), LoadOutcome::Recovered, None)
        }
        Ok(text) => match serde_json::from_str::<Settings>(&text) {
            Ok(s) => (s, LoadOutcome::Loaded, Some(text)),
            Err(e) => {
                tracing::warn!(error = %e, "settings defective, backing up");
                backup(path);
                (Settings::default(), LoadOutcome::Recovered, None)
            }
        },
    };
    settings.sanitize();

    // Rewrite when the file is new, recovered, or gained/lost fields.
    let normalized = serde_json::to_string_pretty(&settings).ok();
    if normalized.is_some() && normalized != original {
        save(path, &settings);
    }
    (settings, outcome)
}

/// Saves atomically (temp file + rename). Errors are logged.
pub fn save(path: &Path, settings: &Settings) {
    let result = (|| -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(settings).map_err(std::io::Error::other)?;
        let tmp = with_suffix(path, "tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)
    })();
    if let Err(e) = result {
        tracing::error!(error = %e, path = %path.display(), "saving settings failed");
    }
}

fn backup(path: &Path) {
    let bak = with_suffix(path, "bak");
    if let Err(e) = std::fs::rename(path, &bak) {
        tracing::warn!(error = %e, "could not back up settings");
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".");
    s.push(suffix);
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::model::MonitorMode;

    fn temp_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("glowframe-store-tests")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("settings.json")
    }

    #[test]
    fn creates_defaults() {
        let p = temp_file("create");
        let (s, o) = load(&p);
        assert_eq!(o, LoadOutcome::Created);
        assert_eq!(s, Settings::default());
        assert!(p.is_file());
    }

    #[test]
    fn fills_missing_ignores_unknown() {
        let p = temp_file("partial");
        std::fs::write(
            &p,
            r#"{"hotkey":"Ctrl+Shift+G","bogus":1,"frame":{"speed":10}}"#,
        )
        .unwrap();
        let (s, o) = load(&p);
        assert_eq!(o, LoadOutcome::Loaded);
        assert_eq!(s.hotkey, "Ctrl+Shift+G");
        assert_eq!(s.frame.speed, 10);
        assert_eq!(s.frame.brightness, 80);
        let rewritten = std::fs::read_to_string(&p).unwrap();
        assert!(!rewritten.contains("bogus"));
        assert!(rewritten.contains("brightness"));
    }

    #[test]
    fn defective_file_is_backed_up() {
        let p = temp_file("broken");
        std::fs::write(&p, "{ not json").unwrap();
        let (s, o) = load(&p);
        assert_eq!(o, LoadOutcome::Recovered);
        assert_eq!(s, Settings::default());
        assert_eq!(
            std::fs::read_to_string(with_suffix(&p, "bak")).unwrap(),
            "{ not json"
        );
    }

    #[test]
    fn roundtrip() {
        let p = temp_file("roundtrip");
        let mut s = Settings::default();
        s.monitors.insert(r"\\.\DISPLAY2".into(), MonitorMode::Free);
        s.lyrics.offset_ms = -300;
        save(&p, &s);
        let (loaded, o) = load(&p);
        assert_eq!(o, LoadOutcome::Loaded);
        assert_eq!(loaded, s);
    }
}
