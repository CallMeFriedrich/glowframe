//! UI texts from `locales/*.toml` (embedded). Missing keys fall back to English.

use std::collections::HashMap;

use crate::settings::model::Language;

const DE: &str = include_str!("../locales/de.toml");
const EN: &str = include_str!("../locales/en.toml");

pub struct I18n {
    lang: Language,
    de: HashMap<String, String>,
    en: HashMap<String, String>,
}

impl I18n {
    pub fn new(lang: Language) -> Self {
        Self {
            lang,
            de: flatten(DE),
            en: flatten(EN),
        }
    }

    #[allow(dead_code)] // live language switch from the settings window (phase 7)
    pub fn set_language(&mut self, lang: Language) {
        self.lang = lang;
    }

    /// Looks up `section.key`. Returns the key itself if it is missing everywhere.
    pub fn t<'a>(&'a self, key: &'a str) -> &'a str {
        let primary = match self.lang {
            Language::De => &self.de,
            Language::En => &self.en,
        };
        primary
            .get(key)
            .or_else(|| self.en.get(key))
            .map_or(key, String::as_str)
    }

    /// Like [`t`](Self::t), replacing `{name}` placeholders.
    pub fn tf(&self, key: &str, args: &[(&str, &str)]) -> String {
        let mut s = self.t(key).to_string();
        for (name, value) in args {
            s = s.replace(&format!("{{{name}}}"), value);
        }
        s
    }
}

/// Resolves `None` (follow system) to a concrete language: DE if the Windows
/// UI language is German, otherwise EN.
pub fn resolve_language(setting: Option<Language>) -> Language {
    setting.unwrap_or_else(system_language)
}

fn system_language() -> Language {
    // SAFETY: no arguments, returns a LANGID.
    let langid = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
    const LANG_GERMAN: u16 = 0x07;
    if langid & 0x3FF == LANG_GERMAN {
        Language::De
    } else {
        Language::En
    }
}

fn flatten(src: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    match src.parse::<toml::Table>() {
        Ok(table) => walk("", &table, &mut out),
        Err(e) => tracing::error!(error = %e, "locale file invalid"),
    }
    out
}

fn walk(prefix: &str, table: &toml::Table, out: &mut HashMap<String, String>) {
    for (k, v) in table {
        let key = if prefix.is_empty() {
            k.clone()
        } else {
            format!("{prefix}.{k}")
        };
        match v {
            toml::Value::String(s) => {
                out.insert(key, s.clone());
            }
            toml::Value::Table(t) => walk(&key, t, out),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locales_have_same_keys() {
        let mut de: Vec<_> = flatten(DE).into_keys().collect();
        let mut en: Vec<_> = flatten(EN).into_keys().collect();
        de.sort();
        en.sort();
        assert!(!en.is_empty());
        assert_eq!(de, en);
    }

    #[test]
    fn lookup_and_fallback() {
        let mut i = I18n::new(Language::De);
        assert_eq!(i.t("tray.quit"), "Beenden");
        i.de.remove("tray.quit");
        assert_eq!(i.t("tray.quit"), "Quit");
        assert_eq!(i.t("does.not.exist"), "does.not.exist");
        i.set_language(Language::En);
        assert_eq!(
            i.tf("notify.hotkey_failed", &[("hotkey", "Ctrl+Alt+L")]),
            "Ctrl+Alt+L is already in use. Please choose another combination in the settings."
        );
    }
}
