//! Global hotkey (SPEC §4). Toggles the fullscreen mode.

use global_hotkey::GlobalHotKeyManager;
use global_hotkey::hotkey::HotKey;

pub struct Hotkey {
    manager: GlobalHotKeyManager,
    current: Option<HotKey>,
}

impl Hotkey {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            manager: GlobalHotKeyManager::new().map_err(|e| e.to_string())?,
            current: None,
        })
    }

    /// Replaces the registered hotkey. On error no hotkey is active.
    pub fn register(&mut self, spec: &str) -> Result<(), String> {
        if let Some(old) = self.current.take() {
            let _ = self.manager.unregister(old);
        }
        let hotkey: HotKey = spec.parse().map_err(|e| format!("{e}"))?;
        self.manager.register(hotkey).map_err(|e| e.to_string())?;
        tracing::info!(hotkey = spec, "hotkey registered");
        self.current = Some(hotkey);
        Ok(())
    }

    pub fn matches(&self, id: u32) -> bool {
        self.current.is_some_and(|h| h.id() == id)
    }
}

#[cfg(test)]
mod tests {
    use global_hotkey::hotkey::{Code, HotKey, Modifiers};

    #[test]
    fn default_hotkey_parses() {
        let h: HotKey = "Ctrl+Alt+L".parse().unwrap();
        assert_eq!(
            h,
            HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyL)
        );
    }
}
