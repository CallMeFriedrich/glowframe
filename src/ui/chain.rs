//! Constellation V2 (SPEC §5.4c2): one continuous chain of words across
//! lines, up to 30 visible, oldest fading slowly; keywords drawn larger.
//! Pure logic; drawing lives in `render::ui`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;

use crate::lyrics::LyricLine;
use crate::ui::chat::word_starts;
use crate::ui::stars::{AREA, Rng, dist};

/// Words fully visible at once.
pub const MAX_WORDS: usize = 30;
/// Song seconds an old word takes to fade out once pushed out.
pub const FADE_SECS: f32 = 6.0;
/// A forward jump by more words than this rebuilds the chain.
const REBUILD_JUMP: usize = 40;
/// Share of the distinct candidate words that become keywords.
const KEYWORD_SHARE: f32 = 0.15;
const CANDIDATES: usize = 80;

/// Filler words that never count as keywords (DE/EN, lowercase).
const STOPWORDS: &[&str] = &[
    // English
    "the", "and", "you", "your", "you're", "youre", "yours", "for", "but", "not", "are", "was",
    "were", "with", "that", "this", "these", "those", "have", "has", "had", "what", "when",
    "where", "who", "why", "how", "all", "can", "could", "would", "should", "will", "just", "like",
    "from", "into", "onto", "out", "our", "ours", "they", "them", "their", "there", "then", "than",
    "she", "her", "his", "him", "its", "it's", "i'm", "im", "i've", "i'll", "i'd", "don't", "dont",
    "can't", "cant", "won't", "wont", "ain't", "aint", "gonna", "wanna", "gotta", "yeah", "yea",
    "ooh", "oooh", "ohh", "whoa", "woah", "hey", "uh", "huh", "nah", "mmm", "hmm", "let", "let's",
    "lets", "get", "got", "too", "now", "yes", "about", "over", "some", "any", "here", "been",
    "being", "did", "does", "doing", "done", "one", "every", "which", "while", "because", "cause",
    "'cause", "cuz", "off", "own", "very", "only", "also", "more", "most", "much", "many", "again",
    "still", "even", "ever", "never", "always", "way", "they're", "we're", "it", "me", "my", "we",
    "us", "on", "in", "to", "of", "a", "an", "is", "be", "so", "if", "or", "do", "no", "oh", "la",
    "na", "da", // German
    "und", "der", "die", "das", "den", "dem", "des", "ein", "eine", "einen", "einem", "einer",
    "eines", "ich", "du", "er", "sie", "es", "wir", "ihr", "mich", "dich", "sich", "mir", "dir",
    "uns", "euch", "ihn", "ihm", "ihnen", "mein", "meine", "meinen", "meinem", "meiner", "dein",
    "deine", "deinen", "deinem", "deiner", "sein", "seine", "seinen", "seinem", "seiner", "nicht",
    "nur", "auch", "noch", "schon", "doch", "mal", "ist", "bin", "bist", "sind", "seid", "war",
    "waren", "hab", "habe", "hast", "hat", "haben", "wird", "werden", "wurde", "kann", "kannst",
    "will", "willst", "muss", "musst", "mit", "von", "vom", "zum", "zur", "bei", "aus", "auf",
    "für", "fur", "über", "uber", "unter", "nach", "vor", "bis", "durch", "gegen", "ohne", "um",
    "an", "am", "im", "in", "zu", "wie", "was", "wer", "wo", "wann", "warum", "weil", "wenn",
    "dass", "daß", "als", "oder", "aber", "denn", "so", "da", "dann", "hier", "dort", "jetzt",
    "immer", "alle", "alles", "kein", "keine", "keinen", "man", "sehr", "ganz", "gibt", "geht",
    "ja", "nein", "nee", "oh", "ah", "eh",
];

/// One word of the song with its timing.
#[derive(Debug, Clone, PartialEq)]
pub struct ChainWord {
    pub text: String,
    /// Song position at which the word starts to appear.
    pub appear: Duration,
    pub keyword: bool,
}

/// A placed star: global word index and position in unit coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    pub k: usize,
    pub pos: (f32, f32),
    /// Connected to the previous word (false after a rebuild gap).
    pub linked: bool,
}

/// Lowercased word without surrounding punctuation.
pub fn normalize(word: &str) -> String {
    word.trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase()
}

fn is_candidate(w: &str) -> bool {
    w.chars().count() >= 3 && !STOPWORDS.contains(&w)
}

/// Keywords of a song: score = occurrences x sqrt(word length); filler
/// words and words under 3 letters never count. The best ~15 % of the
/// distinct candidate words are keywords (SPEC §5.4c2).
pub fn keywords(lines: &[LyricLine]) -> HashSet<String> {
    keywords_ranked(lines).into_iter().collect()
}

/// Keywords, best first (see [`keywords`]).
pub fn keywords_ranked(lines: &[LyricLine]) -> Vec<String> {
    let mut counts: HashMap<String, u32> = HashMap::new();
    for l in lines {
        for w in l.text.split_whitespace() {
            let n = normalize(w);
            if is_candidate(&n) {
                *counts.entry(n).or_default() += 1;
            }
        }
    }
    let mut scored: Vec<(String, f32)> = counts
        .into_iter()
        .map(|(w, c)| {
            let len = w.chars().count().min(12) as f32;
            let s = c as f32 * len.sqrt();
            (w, s)
        })
        .collect();
    // Deterministic order: score, then alphabetically.
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let take = (scored.len() as f32 * KEYWORD_SHARE).ceil() as usize;
    scored.into_iter().take(take).map(|(w, _)| w).collect()
}

/// All words of the song in order with their appear times.
pub fn chain_words(lines: &[LyricLine]) -> Vec<ChainWord> {
    let keys = keywords(lines);
    let mut out = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let next = lines.get(i + 1).map(|n| n.time.saturating_sub(l.time));
        let starts = word_starts(&l.text, next);
        for (w, s) in l.text.split_whitespace().zip(starts) {
            out.push(ChainWord {
                text: w.to_string(),
                appear: l.time + Duration::from_secs_f32(s.max(0.0)),
                keyword: keys.contains(&normalize(w)),
            });
        }
    }
    out
}

/// Random free point for the next star: away from all visible stars and
/// within reach of the previous one. Crossing lines are allowed. Of the
/// valid candidates the one with the most room wins (even spread);
/// constraints relax if none fits, so this always returns a point.
pub fn place_next(
    existing: &[(f32, f32)],
    prev: Option<(f32, f32)>,
    seed: u64,
    aspect: f32,
) -> (f32, f32) {
    let aspect = aspect.clamp(0.5, 4.0);
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let (x0, y0, x1, y1) = AREA;
    let near = |c: (f32, f32)| {
        existing
            .iter()
            .map(|p| dist(*p, c, aspect))
            .fold(f32::MAX, f32::min)
    };
    let mut fallback: Option<((f32, f32), f32)> = None;
    let mut relax = 1.0f32;
    for _ in 0..8 {
        let min_d = 0.13 * relax;
        let (seg_min, seg_max) = (0.1 * relax, 0.6 / relax);
        let mut best: Option<((f32, f32), f32)> = None;
        for _ in 0..CANDIDATES {
            let c = (x0 + (x1 - x0) * rng.next(), y0 + (y1 - y0) * rng.next());
            let room = near(c);
            if fallback.is_none_or(|(_, d)| room > d) {
                fallback = Some((c, room));
            }
            if room < min_d {
                continue;
            }
            if let Some(p) = prev {
                let len = dist(p, c, aspect);
                if len < seg_min || len > seg_max {
                    continue;
                }
            }
            if best.is_none_or(|(_, d)| room > d) {
                best = Some((c, room));
            }
        }
        if let Some((c, _)) = best {
            return c;
        }
        relax *= 0.85;
    }
    fallback.map_or(((x0 + x1) / 2.0, (y0 + y1) / 2.0), |(c, _)| c)
}

/// The chain of placed stars, extended as the song plays.
#[derive(Debug, Default)]
pub struct StarChain {
    id: Option<u64>,
    words: Vec<ChainWord>,
    placed: VecDeque<Placed>,
    /// Words revealed at the last update.
    revealed: usize,
    pos: Duration,
}

impl StarChain {
    pub fn new() -> Self {
        Self::default()
    }

    /// Follows the song position `pos` of lyrics `id`. Words get placed
    /// once when they appear; seeking back removes unsung words, a big
    /// jump forward rebuilds the chain from the last 30 words.
    pub fn update(&mut self, id: u64, lines: &[LyricLine], pos: Duration, aspect: f32) {
        if self.id != Some(id) {
            self.id = Some(id);
            self.words = chain_words(lines);
            self.placed.clear();
        }
        self.pos = pos;
        let n = self.words.partition_point(|w| w.appear <= pos);
        self.revealed = n;
        while self.placed.back().is_some_and(|p| p.k >= n) {
            self.placed.pop_back();
        }
        let next = self.placed.back().map_or(0, |p| p.k + 1);
        let start = if n > next + REBUILD_JUMP || (self.placed.is_empty() && n > MAX_WORDS) {
            self.placed.clear();
            n.saturating_sub(MAX_WORDS)
        } else {
            next
        };
        for k in start..n {
            let existing: Vec<(f32, f32)> = self.placed.iter().map(|p| p.pos).collect();
            let prev = self.placed.back().filter(|p| p.k + 1 == k).map(|p| p.pos);
            let seed = id ^ (k as u64).wrapping_mul(0xD1B5_4A32_D192_ED03);
            let pos = place_next(&existing, prev, seed, aspect);
            self.placed.push_back(Placed {
                k,
                pos,
                linked: prev.is_some(),
            });
        }
        while self.placed.front().is_some_and(|p| self.fade(p.k) >= 1.0) {
            self.placed.pop_front();
        }
    }

    /// Fade-out of word `k` (0 = fully visible, 1 = gone): it starts when
    /// the 30th word after it appears.
    pub fn fade(&self, k: usize) -> f32 {
        match self.words.get(k + MAX_WORDS) {
            Some(w) if w.appear <= self.pos => {
                ((self.pos - w.appear).as_secs_f32() / FADE_SECS).min(1.0)
            }
            _ => 0.0,
        }
    }

    /// Seconds since word `k` started to appear.
    pub fn age(&self, k: usize) -> f32 {
        self.words
            .get(k)
            .map_or(0.0, |w| self.pos.saturating_sub(w.appear).as_secs_f32())
    }

    pub fn placed(&self) -> impl Iterator<Item = &Placed> {
        self.placed.iter()
    }

    pub fn word(&self, k: usize) -> Option<&ChainWord> {
        self.words.get(k)
    }

    /// Index of the newest revealed word.
    pub fn newest(&self) -> Option<usize> {
        self.revealed.checked_sub(1)
    }

    pub fn is_empty(&self) -> bool {
        self.placed.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(secs: f32, text: &str) -> LyricLine {
        LyricLine {
            time: Duration::from_secs_f32(secs),
            text: text.into(),
        }
    }

    fn song() -> Vec<LyricLine> {
        (0..40)
            .map(|i| {
                line(
                    i as f32 * 3.0,
                    if i % 4 == 0 {
                        "Midnight river carry me home tonight"
                    } else {
                        "and the city lights are fading slowly away"
                    },
                )
            })
            .collect()
    }

    #[test]
    fn keywords_skip_fillers_and_prefer_repeats() {
        let lines = vec![
            line(0.0, "Thunder thunder, oh yeah, the thunder rolls"),
            line(2.0, "and you and I walk down the road"),
            line(4.0, "Thunder in the night, oh yeah"),
        ];
        let k = keywords(&lines);
        assert!(k.contains("thunder"));
        for filler in ["the", "and", "yeah", "you", "oh", "in"] {
            assert!(!k.contains(filler), "{filler}");
        }
        // 15 % of the distinct candidates.
        assert_eq!(k.len(), 1);
    }

    #[test]
    fn normalize_strips_punctuation() {
        assert_eq!(normalize("Thunder,"), "thunder");
        assert_eq!(normalize("(Hey!)"), "hey");
        assert_eq!(normalize("Straße"), "straße");
    }

    #[test]
    fn words_appear_in_order_within_their_line() {
        let w = chain_words(&song());
        assert!(w.windows(2).all(|p| p[0].appear <= p[1].appear));
        assert_eq!(w[0].appear, Duration::ZERO);
        assert!(w.iter().take(6).all(|x| x.appear < Duration::from_secs(3)));
    }

    #[test]
    fn chain_keeps_at_most_30_plus_fading() {
        let lines = song();
        let mut c = StarChain::new();
        let mut t = 0.0;
        while t < 100.0 {
            c.update(7, &lines, Duration::from_secs_f32(t), 16.0 / 9.0);
            let solid = c.placed().filter(|p| c.fade(p.k) == 0.0).count();
            assert!(solid <= MAX_WORDS, "{solid} at {t}");
            t += 0.1;
        }
        // Long after the start, older words have faded and left.
        assert!(c.placed().count() < MAX_WORDS + 20);
        assert!(
            c.placed()
                .all(|p| p.k + MAX_WORDS + 20 > c.newest().unwrap())
        );
    }

    #[test]
    fn positions_are_stable_while_playing() {
        let lines = song();
        let mut c = StarChain::new();
        c.update(1, &lines, Duration::from_secs(10), 1.78);
        let before: Vec<Placed> = c.placed().copied().collect();
        c.update(1, &lines, Duration::from_secs(11), 1.78);
        for p in &before {
            assert!(c.placed().any(|q| q == p));
        }
    }

    #[test]
    fn seeking_back_removes_unsung_words() {
        let lines = song();
        let mut c = StarChain::new();
        c.update(1, &lines, Duration::from_secs(20), 1.78);
        c.update(1, &lines, Duration::from_secs(5), 1.78);
        let n = c.newest().unwrap();
        assert!(c.placed().all(|p| p.k <= n));
        assert_eq!(c.placed().last().map(|p| p.k), Some(n));
    }

    #[test]
    fn big_jump_rebuilds_from_last_30() {
        let lines = song();
        let mut c = StarChain::new();
        c.update(1, &lines, Duration::from_secs(1), 1.78);
        c.update(1, &lines, Duration::from_secs(100), 1.78);
        assert_eq!(c.placed().count(), MAX_WORDS);
        assert!(!c.placed().next().unwrap().linked);
    }

    #[test]
    fn stars_keep_their_distance() {
        let lines = song();
        let mut c = StarChain::new();
        c.update(3, &lines, Duration::from_secs(30), 16.0 / 9.0);
        let pts: Vec<(f32, f32)> = c.placed().map(|p| p.pos).collect();
        let (x0, y0, x1, y1) = AREA;
        let mut tight = 0;
        for (i, a) in pts.iter().enumerate() {
            assert!((x0..=x1).contains(&a.0) && (y0..=y1).contains(&a.1));
            for b in &pts[i + 1..] {
                if dist(*a, *b, 16.0 / 9.0) < 0.08 {
                    tight += 1;
                }
            }
        }
        assert!(tight <= 2, "{tight} tight pairs");
    }
}
