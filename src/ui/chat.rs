//! Messages view (SPEC §5.4b, v1.3): pure logic for the chat-style lyrics.
//! Bubble sides, word-by-word timing, place detection for map bubbles and
//! the procedural mini map. Drawing lives in `render::ui`.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::{Duration, Instant};

use crate::lyrics::LyricLine;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// Gray, incoming.
    Left,
    /// Blue, outgoing.
    Right,
}

fn hash_of(v: impl Hash) -> u64 {
    let mut h = DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

/// Side of every line: alternating, with an occasional second bubble on the
/// same side (deterministic per lyrics, so it never flickers).
pub fn sides(lyrics_id: u64, lines: &[LyricLine]) -> Vec<Side> {
    let mut side = Side::Left;
    let mut first = true;
    lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            if l.text.is_empty() {
                return side;
            }
            if first {
                first = false;
            } else if hash_of((lyrics_id, i)) % 4 != 0 {
                side = match side {
                    Side::Left => Side::Right,
                    Side::Right => Side::Left,
                };
            }
            side
        })
        .collect()
}

/// Number of revealed words (fractional: the word in progress fades in).
/// LRCLIB has line timings only, so the time until the next line is spread
/// over the words by their length (SPEC §5.4b).
pub fn reveal_words(text: &str, elapsed: Duration, line_duration: Option<Duration>) -> f32 {
    let words: Vec<usize> = text.split_whitespace().map(|w| w.chars().count()).collect();
    if words.is_empty() {
        return 0.0;
    }
    // Weight per word: its letters plus a little for the gap after it.
    let weights: Vec<f32> = words.iter().map(|&c| c as f32 + 2.0).collect();
    let total: f32 = weights.iter().sum();
    // Sung text rarely fills the whole gap to the next line; cap the speed
    // at ~0.11 s per weight unit and leave a short rest at the end.
    let natural = total * 0.11;
    let window = line_duration
        .map_or(natural, |d| (d.as_secs_f32() * 0.85).min(natural))
        .max(0.25);
    let mut t = elapsed.as_secs_f32() / window * total;
    let mut revealed = 0.0;
    for w in weights {
        if t >= w {
            revealed += 1.0;
            t -= w;
        } else {
            revealed += (t / w).max(0.0);
            break;
        }
    }
    revealed.min(words.len() as f32)
}

/// UTF-16 ranges (start, len) of the words of `text`, for DirectWrite.
pub fn word_ranges_utf16(text: &str) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    let mut pos = 0u32;
    let mut start: Option<u32> = None;
    for ch in text.chars() {
        let len = ch.len_utf16() as u32;
        if ch.is_whitespace() {
            if let Some(s) = start.take() {
                out.push((s, pos - s));
            }
        } else if start.is_none() {
            start = Some(pos);
        }
        pos += len;
    }
    if let Some(s) = start {
        out.push((s, pos - s));
    }
    out
}

/// Map bubble for a line that mentions a place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapHint {
    /// Location line under the lyric, e.g. "Brooklyn Bridge, New York".
    pub label: &'static str,
    /// Seed of the procedural map.
    pub seed: u64,
}

/// Known places: lower-case spellings (DE/EN, single or multi word) and the
/// location line shown in the bubble.
const PLACES: &[(&[&str], &str)] = &[
    (&["new york", "nyc", "manhattan"], "Times Square, New York"),
    (&["brooklyn"], "Brooklyn Bridge, New York"),
    (&["harlem", "bronx", "queens"], "Bronx, New York"),
    (
        &["los angeles", "hollywood", "malibu"],
        "Hollywood, Los Angeles",
    ),
    (&["compton"], "Compton, Los Angeles"),
    (
        &["san francisco", "bay area"],
        "Golden Gate Bridge, San Francisco",
    ),
    (&["las vegas", "vegas"], "The Strip, Las Vegas"),
    (&["miami"], "Ocean Drive, Miami"),
    (&["chicago"], "Navy Pier, Chicago"),
    (&["atlanta"], "Midtown, Atlanta"),
    (&["houston", "texas", "dallas"], "Downtown, Houston"),
    (&["detroit"], "Downtown, Detroit"),
    (&["new orleans"], "French Quarter, New Orleans"),
    (&["nashville", "memphis"], "Broadway, Nashville"),
    (&["boston"], "Back Bay, Boston"),
    (&["seattle"], "Pike Place, Seattle"),
    (&["philadelphia", "philly"], "Old City, Philadelphia"),
    (&["california", "cali"], "Pacific Coast Highway, California"),
    (&["hawaii"], "Waikiki, Honolulu"),
    (&["toronto"], "CN Tower, Toronto"),
    (&["mexico"], "Zócalo, Mexico City"),
    (&["rio"], "Copacabana, Rio de Janeiro"),
    (&["jamaica"], "Kingston, Jamaica"),
    (&["bahamas"], "Nassau, Bahamas"),
    (&["london"], "Tower Bridge, London"),
    (&["paris"], "Champs-Élysées, Paris"),
    (&["berlin"], "Brandenburger Tor, Berlin"),
    (&["hamburg"], "Speicherstadt, Hamburg"),
    (&["münchen", "munich"], "Marienplatz, München"),
    (&["köln", "cologne"], "Domplatte, Köln"),
    (&["frankfurt"], "Römerberg, Frankfurt"),
    (&["wien", "vienna"], "Stephansplatz, Wien"),
    (&["zürich", "zurich"], "Bahnhofstrasse, Zürich"),
    (&["amsterdam"], "Grachtengordel, Amsterdam"),
    (&["barcelona"], "La Rambla, Barcelona"),
    (&["madrid"], "Gran Vía, Madrid"),
    (&["ibiza"], "Sant Antoni, Ibiza"),
    (&["lisbon", "lissabon"], "Alfama, Lisboa"),
    (&["rome", "rom"], "Fontana di Trevi, Roma"),
    (&["milan", "mailand", "milano"], "Galleria, Milano"),
    (&["venice", "venedig"], "Rialto, Venezia"),
    (&["prague", "prag"], "Karlův most, Praha"),
    (&["copenhagen", "kopenhagen"], "Nyhavn, København"),
    (&["stockholm"], "Gamla stan, Stockholm"),
    (&["dublin"], "Temple Bar, Dublin"),
    (&["moscow", "moskau"], "Red Square, Moscow"),
    (&["istanbul"], "Galata, Istanbul"),
    (&["dubai"], "Downtown, Dubai"),
    (&["cairo", "kairo"], "Zamalek, Cairo"),
    (&["tokyo", "tokio", "shibuya"], "Shibuya Crossing, Tokyo"),
    (&["osaka"], "Dōtonbori, Osaka"),
    (&["seoul"], "Gangnam, Seoul"),
    (&["hong kong", "hongkong"], "Victoria Harbour, Hong Kong"),
    (&["singapore", "singapur"], "Marina Bay, Singapore"),
    (&["bangkok"], "Sukhumvit, Bangkok"),
    (&["sydney"], "Harbour Bridge, Sydney"),
];

/// Words that make a line a "map" line without naming a place.
const PLACE_WORDS: &[&str] = &[
    "map",
    "maps",
    "city",
    "cities",
    "town",
    "street",
    "streets",
    "road",
    "roads",
    "highway",
    "avenue",
    "boulevard",
    "bridge",
    "downtown",
    "skyline",
    "subway",
    "karte",
    "karten",
    "stadt",
    "städte",
    "straße",
    "straßen",
    "strasse",
    "brücke",
    "autobahn",
    "innenstadt",
    "hafen",
];

/// Fallback locations for lines with a place word but no named place.
const FALLBACK_LABELS: &[&str] = &[
    "Shibuya Crossing, Tokyo",
    "Brooklyn Bridge, New York",
    "Golden Gate Bridge, San Francisco",
    "Marina Bay, Singapore",
    "Tower Bridge, London",
    "Champs-Élysées, Paris",
    "Brandenburger Tor, Berlin",
    "Ocean Drive, Miami",
];

fn tokens(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

fn contains_phrase(tokens: &[String], phrase: &str) -> bool {
    let words: Vec<&str> = phrase.split(' ').collect();
    tokens
        .windows(words.len())
        .any(|w| w.iter().zip(&words).all(|(a, b)| a == b))
}

/// Map bubble if the line names a place or talks about maps/streets/cities.
pub fn map_hint(text: &str) -> Option<MapHint> {
    let tokens = tokens(text);
    if tokens.is_empty() {
        return None;
    }
    let seed = hash_of(text);
    for (names, label) in PLACES {
        if names.iter().any(|n| contains_phrase(&tokens, n)) {
            return Some(MapHint { label, seed });
        }
    }
    if tokens.iter().any(|t| PLACE_WORDS.contains(&t.as_str())) {
        let label = FALLBACK_LABELS[(seed % FALLBACK_LABELS.len() as u64) as usize];
        return Some(MapHint { label, seed });
    }
    None
}

/// Procedural mini map in unit coordinates (0..1 in both axes).
#[derive(Debug, Clone, PartialEq)]
pub struct MiniMap {
    /// 0 = night blue, 1 = park green, 2 = dusk violet.
    pub theme: u8,
    /// Water area as a closed polygon, if any.
    pub water: Option<Vec<(f32, f32)>>,
    /// Roads as quadratic curves: start, control, end, relative width.
    pub roads: Vec<[(f32, f32); 3]>,
    pub road_widths: Vec<f32>,
    pub pin: (f32, f32),
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 40) as f32 / (1u64 << 24) as f32
    }

    fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.next()
    }
}

pub fn mini_map(seed: u64) -> MiniMap {
    let mut r = Rng(seed | 1);
    let theme = (seed % 3) as u8;
    let water = (r.next() < 0.6).then(|| {
        // A river or coast band across one corner.
        let y0 = r.range(0.55, 0.85);
        let y1 = r.range(0.35, 0.75);
        vec![(0.0, y0), (1.0, y1), (1.0, 1.0), (0.0, 1.0)]
    });
    let mut roads = Vec::new();
    let mut road_widths = Vec::new();
    // Two main roads crossing near the pin, plus some side streets.
    let pin = (r.range(0.35, 0.65), r.range(0.3, 0.55));
    for k in 0..6 {
        let main = k < 2;
        let horizontal = k % 2 == 0;
        let (a, b) = if horizontal {
            let y = if main {
                pin.1 + r.range(-0.08, 0.08)
            } else {
                r.next()
            };
            (
                (-0.1, y + r.range(-0.15, 0.15)),
                (1.1, y + r.range(-0.15, 0.15)),
            )
        } else {
            let x = if main {
                pin.0 + r.range(-0.08, 0.08)
            } else {
                r.next()
            };
            (
                (x + r.range(-0.2, 0.2), -0.1),
                (x + r.range(-0.2, 0.2), 1.1),
            )
        };
        let c = (
            (a.0 + b.0) / 2.0 + r.range(-0.15, 0.15),
            (a.1 + b.1) / 2.0 + r.range(-0.15, 0.15),
        );
        roads.push([a, c, b]);
        road_widths.push(if main { 1.0 } else { r.range(0.35, 0.6) });
    }
    MiniMap {
        theme,
        water,
        roads,
        road_widths,
        pin,
    }
}

/// One bubble of the thread: a lyric line or the typing indicator (`None`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub line: Option<usize>,
    pub side: Side,
    pub seq: u64,
    /// When the bubble appeared (pop-in animation).
    pub at: Instant,
}

/// Bubbles kept in the thread (older ones are off screen anyway).
const MAX_ENTRIES: usize = 10;
/// Lines shown when the thread is rebuilt (start, seek, new lyrics).
const REBUILD_LINES: usize = 5;
/// Bigger forward jumps than this rebuild instead of popping every line.
const MAX_STEP: usize = 3;

/// Message thread following the song (SPEC §5.4b).
pub struct ChatThread {
    id: Option<u64>,
    sides: Vec<Side>,
    maps: Vec<Option<MapHint>>,
    entries: Vec<Entry>,
    next_seq: u64,
    last_line: Option<usize>,
}

impl Default for ChatThread {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatThread {
    pub fn new() -> Self {
        Self {
            id: None,
            sides: Vec::new(),
            maps: Vec::new(),
            entries: Vec::new(),
            next_seq: 0,
            last_line: None,
        }
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn map(&self, line: usize) -> Option<&MapHint> {
        self.maps.get(line).and_then(Option::as_ref)
    }

    fn push(&mut self, line: Option<usize>, side: Side, at: Instant) {
        // The typing indicator turns into the message (same place, no scroll).
        if line.is_some()
            && let Some(last) = self.entries.last_mut()
            && last.line.is_none()
        {
            *last = Entry {
                line,
                side,
                at,
                ..*last
            };
            return;
        }
        self.entries.push(Entry {
            line,
            side,
            seq: self.next_seq,
            at,
        });
        self.next_seq += 1;
        if self.entries.len() > MAX_ENTRIES {
            self.entries.remove(0);
        }
    }

    fn next_side(&self, lines: &[LyricLine], after: Option<usize>) -> Side {
        let start = after.map_or(0, |i| i + 1);
        (start..lines.len())
            .find(|&i| !lines[i].text.is_empty())
            .and_then(|i| self.sides.get(i).copied())
            .unwrap_or(Side::Left)
    }

    /// Follows the playback state. Returns `true` when the thread was
    /// rebuilt (the scroll position should jump instead of animating).
    pub fn update(
        &mut self,
        now: Instant,
        id: u64,
        lines: &[LyricLine],
        st: crate::lyrics::lrc::LineState,
    ) -> bool {
        let mut rebuilt = false;
        if self.id != Some(id) {
            self.id = Some(id);
            self.sides = sides(id, lines);
            self.maps = lines.iter().map(|l| map_hint(&l.text)).collect();
            self.entries.clear();
            self.last_line = None;
            rebuilt = true;
        }
        if st.current != self.last_line {
            let forward = match (self.last_line, st.current) {
                (Some(p), Some(i)) => i > p && i - p <= MAX_STEP && !rebuilt,
                (None, Some(i)) => !rebuilt && i <= MAX_STEP && !self.entries.is_empty(),
                _ => false,
            };
            if forward {
                let from = self.last_line.map_or(0, |p| p + 1);
                for i in from..=st.current.unwrap_or(0) {
                    if !lines[i].text.is_empty() {
                        self.push(Some(i), self.sides[i], now);
                    }
                }
            } else {
                // Start, seek or new lyrics: show the recent lines settled.
                self.entries.clear();
                let settled = now.checked_sub(Duration::from_secs(5)).unwrap_or(now);
                if let Some(cur) = st.current {
                    let recent: Vec<usize> = (0..=cur.min(lines.len().saturating_sub(1)))
                        .rev()
                        .filter(|&i| !lines[i].text.is_empty())
                        .take(REBUILD_LINES)
                        .collect();
                    for &i in recent.iter().rev() {
                        self.push(Some(i), self.sides[i], settled);
                    }
                }
                rebuilt = true;
            }
            self.last_line = st.current;
        }
        // Instrumental gap: typing indicator on the side of the next line.
        let typing = self.entries.last().is_some_and(|e| e.line.is_none());
        if st.gap && !typing {
            let side = self.next_side(lines, st.current);
            self.push(None, side, now);
        } else if !st.gap && typing && st.current.is_some_and(|i| lines[i].text.is_empty()) {
            // Short pause marker without a long gap: no indicator.
            self.entries.pop();
        }
        rebuilt
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::lrc::LineState;

    fn st(current: Option<usize>, gap: bool) -> LineState {
        LineState { current, gap }
    }

    #[test]
    fn thread_pops_lines_in_order_and_morphs_typing() {
        let lines = vec![
            line(0, ""),
            line(10, "first line"),
            line(12, "second line"),
            line(14, "third"),
        ];
        let t0 = Instant::now();
        let mut t = ChatThread::new();
        // Intro gap: typing indicator only.
        assert!(t.update(t0, 1, &lines, st(Some(0), true)));
        assert_eq!(t.entries().len(), 1);
        assert_eq!(t.entries()[0].line, None);
        let seq = t.entries()[0].seq;
        // First line replaces the indicator in place.
        let t1 = t0 + Duration::from_secs(10);
        assert!(!t.update(t1, 1, &lines, st(Some(1), false)));
        assert_eq!(t.entries().len(), 1);
        assert_eq!(t.entries()[0].line, Some(1));
        assert_eq!(t.entries()[0].seq, seq);
        assert_eq!(t.entries()[0].at, t1);
        // Next lines pop in one by one.
        assert!(!t.update(t1, 1, &lines, st(Some(2), false)));
        assert!(!t.update(t1, 1, &lines, st(Some(3), false)));
        let got: Vec<_> = t.entries().iter().map(|e| e.line).collect();
        assert_eq!(got, vec![Some(1), Some(2), Some(3)]);
        let seqs: Vec<_> = t.entries().iter().map(|e| e.seq).collect();
        assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1));
    }

    #[test]
    fn seeking_rebuilds_settled() {
        let lines: Vec<_> = (0..40).map(|i| line(i * 3, "la la la")).collect();
        let t0 = Instant::now();
        let mut t = ChatThread::new();
        t.update(t0, 1, &lines, st(Some(2), false));
        t.update(t0, 1, &lines, st(Some(3), false));
        assert!(t.update(t0, 1, &lines, st(Some(30), false)));
        assert_eq!(t.entries().len(), REBUILD_LINES);
        assert_eq!(t.entries().last().and_then(|e| e.line), Some(30));
        assert!(t.entries().iter().all(|e| e.at < t0));
        // Backwards as well.
        assert!(t.update(t0, 1, &lines, st(Some(5), false)));
        assert_eq!(t.entries().last().and_then(|e| e.line), Some(5));
        // New lyrics rebuild too.
        assert!(t.update(t0, 2, &lines, st(Some(5), false)));
    }

    #[test]
    fn thread_is_bounded_and_marks_maps() {
        let lines: Vec<_> = (0..60)
            .map(|i| line(i, if i == 7 { "back in the city" } else { "words" }))
            .collect();
        let t0 = Instant::now();
        let mut t = ChatThread::new();
        for i in 0..60 {
            t.update(t0, 9, &lines, st(Some(i), false));
        }
        assert!(t.entries().len() <= MAX_ENTRIES);
        assert!(t.map(7).is_some());
        assert!(t.map(8).is_none());
    }

    fn line(t: u64, text: &str) -> LyricLine {
        LyricLine {
            time: Duration::from_secs(t),
            text: text.into(),
        }
    }

    #[test]
    fn sides_alternate_mostly_and_are_stable() {
        let lines: Vec<_> = (0..200).map(|i| line(i, "la la")).collect();
        let a = sides(42, &lines);
        assert_eq!(a, sides(42, &lines));
        assert_eq!(a[0], Side::Left);
        let changes = a.windows(2).filter(|w| w[0] != w[1]).count();
        // ~3 of 4 lines switch sides.
        assert!((120..=175).contains(&changes), "{changes}");
    }

    #[test]
    fn empty_lines_keep_the_side() {
        let lines = vec![line(0, "a"), line(1, ""), line(2, "b")];
        let s = sides(1, &lines);
        assert_eq!(s[1], s[0]);
    }

    #[test]
    fn words_reveal_over_the_line() {
        let text = "one two three four";
        let d = Some(Duration::from_secs(10));
        assert_eq!(reveal_words(text, Duration::ZERO, d), 0.0);
        let mid = reveal_words(text, Duration::from_millis(900), d);
        assert!(mid > 0.5 && mid < 3.5, "{mid}");
        assert_eq!(reveal_words(text, Duration::from_secs(9), d), 4.0);
        // A short line finishes before the next one starts.
        let short = Some(Duration::from_millis(800));
        assert_eq!(reveal_words(text, Duration::from_millis(700), short), 4.0);
        assert_eq!(reveal_words("", Duration::from_secs(1), d), 0.0);
    }

    #[test]
    fn utf16_word_ranges() {
        assert_eq!(
            word_ranges_utf16(" ab  cde f"),
            vec![(1, 2), (5, 3), (9, 1)]
        );
        // Emoji take two UTF-16 units.
        assert_eq!(word_ranges_utf16("😀a b"), vec![(0, 3), (4, 1)]);
    }

    #[test]
    fn detects_places_and_place_words() {
        let h = map_hint("Moved to the city").map(|m| m.label);
        assert!(h.is_some_and(|l| FALLBACK_LABELS.contains(&l)));
        assert_eq!(
            map_hint("Took a cab to Brooklyn, yeah").map(|m| m.label),
            Some("Brooklyn Bridge, New York")
        );
        assert_eq!(
            map_hint("Wir fahren nach München").map(|m| m.label),
            Some("Marienplatz, München")
        );
        assert_eq!(
            map_hint("Back in New York tonight").map(|m| m.label),
            Some("Times Square, New York")
        );
        assert!(map_hint("Durch die Straßen dieser Stadt").is_some());
        // Substrings do not count ("parish", "rome" in "chromed").
        assert_eq!(map_hint("A parish of chromed hearts"), None);
        assert_eq!(map_hint("I love you"), None);
        // Stable seed per text.
        assert_eq!(map_hint("the city"), map_hint("the city"));
    }

    #[test]
    fn mini_map_stays_in_bounds() {
        for seed in 0..200 {
            let m = mini_map(seed);
            assert!(m.theme < 3);
            assert!((0.0..=1.0).contains(&m.pin.0) && (0.0..=1.0).contains(&m.pin.1));
            assert_eq!(m.roads.len(), m.road_widths.len());
            if let Some(w) = &m.water {
                assert!(
                    w.iter()
                        .all(|p| (0.0..=1.0).contains(&p.0) && (0.0..=1.0).contains(&p.1))
                );
            }
        }
        assert_eq!(mini_map(7), mini_map(7));
    }
}
