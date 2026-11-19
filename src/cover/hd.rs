//! HD cover lookup via the public iTunes Search API (SPEC §6.3). A result is
//! only accepted if artist and album match after normalization.

use serde::Deserialize;

/// Normalizes a title for comparison: lowercase, no bracketed additions,
/// no " - Remastered ..." style suffixes, no "feat." parts, alphanumerics only.
pub fn normalize(s: &str) -> String {
    let lower = s.to_lowercase();
    // Drop bracketed parts.
    let mut out = String::with_capacity(lower.len());
    let mut depth = 0u32;
    for ch in lower.chars() {
        match ch {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    // Cut suffixes like " - Remastered 2011", " - Single", " - EP".
    if let Some(i) = out.find(" - ") {
        out.truncate(i);
    }
    for marker in [" feat. ", " feat ", " ft. ", " featuring "] {
        if let Some(i) = out.find(marker) {
            out.truncate(i);
        }
    }
    let out = out.replace('&', " and ");
    out.chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// First listed artist ("A, B", "A & B", "A x B" -> "a").
fn primary_artist(s: &str) -> String {
    let lower = s.to_lowercase();
    let cut = [",", " & ", " x ", " and ", ";", " feat"]
        .iter()
        .filter_map(|sep| lower.find(sep))
        .min()
        .unwrap_or(lower.len());
    normalize(&lower[..cut])
}

pub fn artist_matches(a: &str, b: &str) -> bool {
    let (na, nb) = (normalize(a), normalize(b));
    !na.is_empty() && (na == nb || primary_artist(a) == primary_artist(b))
}

pub fn album_matches(a: &str, b: &str) -> bool {
    let (na, nb) = (normalize(a), normalize(b));
    !na.is_empty() && na == nb
}

#[derive(Debug, Deserialize)]
pub struct SearchResponse {
    #[serde(default)]
    pub results: Vec<SearchResult>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    #[serde(default)]
    pub artist_name: String,
    #[serde(default)]
    pub collection_name: String,
    #[serde(default)]
    pub artwork_url100: Option<String>,
}

/// Picks the matching album and returns its artwork URL scaled to ~1200 px.
pub fn pick_artwork(resp: &SearchResponse, artist: &str, album: &str) -> Option<String> {
    resp.results
        .iter()
        .find(|r| {
            artist_matches(&r.artist_name, artist) && album_matches(&r.collection_name, album)
        })
        .and_then(|r| r.artwork_url100.as_deref())
        .map(|u| u.replace("100x100bb", "1200x1200bb"))
}

/// Cache file name for an album.
pub fn cache_name(artist: &str, album: &str) -> String {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut h = DefaultHasher::new();
    primary_artist(artist).hash(&mut h);
    normalize(album).hash(&mut h);
    format!("{:016x}", h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization() {
        assert_eq!(normalize("Graduation (Deluxe)"), "graduation");
        assert_eq!(normalize("Abbey Road - Remastered 2019"), "abbey road");
        assert_eq!(normalize("Easy [Maxi Edition]"), "easy");
        assert_eq!(normalize("  Beyoncé  "), "beyoncé");
        assert_eq!(normalize("Simon & Garfunkel"), "simon and garfunkel");
        assert_eq!(normalize("Song feat. Someone"), "song");
        assert_eq!(normalize("AC/DC"), "ac dc");
    }

    #[test]
    fn matching() {
        assert!(album_matches("Easy (Maxi Edition)", "Easy"));
        assert!(!album_matches("Easy", "Easy Rider"));
        assert!(!album_matches("(Deluxe)", ""));
        assert!(artist_matches("CRO", "Cro"));
        assert!(artist_matches("Kanye West, Dwele", "Kanye West"));
        assert!(!artist_matches("Kanye West", "Kanye"));
    }

    #[test]
    fn picks_matching_artwork() {
        let json = r#"{"resultCount":2,"results":[
            {"artistName":"Other","collectionName":"Easy","artworkUrl100":"https://x/a/100x100bb.jpg"},
            {"artistName":"CRO","collectionName":"Easy (Deluxe Version)","artworkUrl100":"https://x/b/100x100bb.jpg"}
        ]}"#;
        let resp: SearchResponse = serde_json::from_str(json).unwrap();
        assert_eq!(
            pick_artwork(&resp, "CRO", "Easy (Maxi Edition)").as_deref(),
            Some("https://x/b/1200x1200bb.jpg")
        );
        assert_eq!(pick_artwork(&resp, "Nobody", "Easy"), None);
    }

    #[test]
    fn cache_name_is_normalized() {
        assert_eq!(
            cache_name("CRO", "Easy (Maxi Edition)"),
            cache_name("Cro", "Easy")
        );
        assert_ne!(cache_name("CRO", "Easy"), cache_name("CRO", "Raop"));
    }
}
