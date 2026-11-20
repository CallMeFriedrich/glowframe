//! LRCLIB response handling (SPEC §6.4). Network access lives in `net.rs`.

use serde::Deserialize;

pub const API: &str = "https://lrclib.net/api";
/// Maximum duration difference for search results (seconds).
pub const MAX_SEARCH_DIFF: f64 = 3.0;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    #[serde(default)]
    pub duration: Option<f64>,
    #[serde(default)]
    pub instrumental: bool,
    #[serde(default)]
    pub synced_lyrics: Option<String>,
}

impl Record {
    /// Only synced lyrics count; instrumental or plain-only means none.
    pub fn synced(&self) -> Option<&str> {
        if self.instrumental {
            return None;
        }
        self.synced_lyrics
            .as_deref()
            .filter(|s| !s.trim().is_empty())
    }
}

/// Best search hit by duration difference (<= 3 s) that has synced lyrics.
pub fn best_match(records: &[Record], duration_s: Option<f64>) -> Option<&Record> {
    let with_lyrics = records.iter().filter(|r| r.synced().is_some());
    match duration_s {
        Some(d) => with_lyrics
            .filter_map(|r| r.duration.map(|rd| ((rd - d).abs(), r)))
            .filter(|(diff, _)| *diff <= MAX_SEARCH_DIFF)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, r)| r),
        None => with_lyrics.into_iter().next(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(duration: f64, synced: Option<&str>, instrumental: bool) -> Record {
        Record {
            duration: Some(duration),
            instrumental,
            synced_lyrics: synced.map(String::from),
        }
    }

    #[test]
    fn parses_api_record() {
        let json = r#"{"id":1,"trackName":"x","duration":233.0,"instrumental":false,
            "plainLyrics":"a","syncedLyrics":"[00:01.00]a"}"#;
        let r: Record = serde_json::from_str(json).unwrap();
        assert_eq!(r.synced(), Some("[00:01.00]a"));
        let plain: Record =
            serde_json::from_str(r#"{"plainLyrics":"a","syncedLyrics":null}"#).unwrap();
        assert_eq!(plain.synced(), None);
        assert_eq!(rec(1.0, Some("[00:01]x"), true).synced(), None);
    }

    #[test]
    fn search_picks_closest_duration() {
        let list = vec![
            rec(200.0, Some("[00:01]a"), false),
            rec(231.0, Some("[00:01]b"), false),
            rec(233.5, None, false),
            rec(240.0, Some("[00:01]c"), false),
        ];
        assert_eq!(
            best_match(&list, Some(233.0)).and_then(Record::synced),
            Some("[00:01]b")
        );
        assert!(best_match(&list, Some(220.0)).is_none());
        assert_eq!(
            best_match(&list, None).and_then(Record::synced),
            Some("[00:01]a")
        );
    }
}
