//! Storage locations. Portable mode: a `portable.txt` next to the exe keeps
//! settings, cache and logs next to the exe (SPEC §7).

use std::path::{Path, PathBuf};

pub const APP_DIR: &str = "Glowframe";
pub const PORTABLE_MARKER: &str = "portable.txt";

#[derive(Debug, Clone)]
pub struct Paths {
    pub portable: bool,
    /// Directory containing `settings.json`.
    pub config_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub log_dir: PathBuf,
}

impl Paths {
    pub fn detect() -> Self {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        let appdata = std::env::var_os("APPDATA").map(PathBuf::from);
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        Self::resolve(&exe_dir, appdata, local)
    }

    fn resolve(exe_dir: &Path, appdata: Option<PathBuf>, local: Option<PathBuf>) -> Self {
        let portable = exe_dir.join(PORTABLE_MARKER).is_file();
        // Without the env variables we fall back to the exe directory.
        match (portable, appdata, local) {
            (false, Some(appdata), Some(local)) => Self {
                portable: false,
                config_dir: appdata.join(APP_DIR),
                cache_dir: local.join(APP_DIR).join("cache"),
                log_dir: local.join(APP_DIR).join("logs"),
            },
            _ => Self {
                portable,
                config_dir: exe_dir.to_path_buf(),
                cache_dir: exe_dir.join("cache"),
                log_dir: exe_dir.join("logs"),
            },
        }
    }

    pub fn settings_file(&self) -> PathBuf {
        self.config_dir.join("settings.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_layout() {
        let dir = std::env::temp_dir().join("glowframe-paths-installed");
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::remove_file(dir.join(PORTABLE_MARKER));
        let p = Paths::resolve(&dir, Some("A".into()), Some("L".into()));
        assert!(!p.portable);
        assert_eq!(
            p.settings_file(),
            PathBuf::from("A/Glowframe/settings.json")
        );
        assert_eq!(p.log_dir, PathBuf::from("L/Glowframe/logs"));
        assert_eq!(p.cache_dir, PathBuf::from("L/Glowframe/cache"));
    }

    #[test]
    fn portable_layout() {
        let dir = std::env::temp_dir().join("glowframe-paths-portable");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(PORTABLE_MARKER), "").unwrap();
        let p = Paths::resolve(&dir, Some("A".into()), Some("L".into()));
        assert!(p.portable);
        assert_eq!(p.settings_file(), dir.join("settings.json"));
        assert_eq!(p.log_dir, dir.join("logs"));
    }
}
