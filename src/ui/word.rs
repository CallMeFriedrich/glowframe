//! One-word view (SPEC §5.4f): a single word on a full-screen background in
//! the inverted color; every word switches to the next color. Pure logic;
//! drawing lives in `render::ui`.

use std::time::Duration;

use crate::ui::chain::ChainWord;

/// Minimum distance (RGB, 0..√3) between two colors of the set.
const DISTINCT: f32 = 0.28;
/// Colors in the set at least (filled up with hue variants).
const MIN_COLORS: usize = 5;

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

pub fn invert(c: [f32; 3]) -> [f32; 3] {
    c.map(|x| 1.0 - x)
}

/// Pushes mid tones towards light or dark so the color and its inverse
/// always contrast clearly (a mid gray would be gray on gray).
pub fn with_contrast(c: [f32; 3]) -> [f32; 3] {
    let l = luma(c);
    const LOW: f32 = 0.3;
    const HIGH: f32 = 0.7;
    if l <= LOW || l >= HIGH {
        return c;
    }
    if l >= 0.5 {
        // Lighten towards white until the luma reaches HIGH.
        let k = (HIGH - l) / (1.0 - l);
        c.map(|x| x + (1.0 - x) * k)
    } else {
        // Darken towards black until the luma reaches LOW.
        let k = LOW / l.max(1e-3);
        c.map(|x| x * k)
    }
}

fn to_hsv(c: [f32; 3]) -> (f32, f32, f32) {
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    let d = max - min;
    let h = if d <= 1e-6 {
        0.0
    } else if max == c[0] {
        ((c[1] - c[2]) / d).rem_euclid(6.0)
    } else if max == c[1] {
        (c[2] - c[0]) / d + 2.0
    } else {
        (c[0] - c[1]) / d + 4.0
    } / 6.0;
    let s = if max <= 0.0 { 0.0 } else { d / max };
    (h, s, max)
}

fn from_hsv(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h6 = h.rem_euclid(1.0) * 6.0;
    let f = h6 - h6.floor();
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match h6 as u32 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// The color sequence (SPEC §5.4f): white and the cover colors; if the
/// cover gives few distinct colors, hue-shifted variants of its main color
/// are added. All colors keep a clear contrast to their inverse.
pub fn word_colors(palette: [[f32; 3]; 3]) -> Vec<[f32; 3]> {
    let mut out: Vec<[f32; 3]> = vec![[1.0, 1.0, 1.0]];
    let push = |c: [f32; 3], out: &mut Vec<[f32; 3]>| {
        let c = with_contrast(c);
        if out.iter().all(|o| dist(*o, c) >= DISTINCT) {
            out.push(c);
        }
    };
    for c in palette {
        push(c, &mut out);
    }
    // Similar colors: neighbours of the main color on the color wheel,
    // saturated enough to read as colors.
    let (h, s, v) = to_hsv(palette[1]);
    let (s, v) = (s.max(0.55), v.max(0.75));
    for shift in [0.09, -0.09, 0.18, -0.18, 0.3, -0.3, 0.45] {
        if out.len() >= MIN_COLORS {
            break;
        }
        push(from_hsv(h + shift, s, v), &mut out);
    }
    out
}

/// The two palette colors with the strongest contrast (SPEC §5.4f mode
/// "two cover colors"), lighter one first. Too similar colors are pushed
/// apart: the lighter one lighter, the darker one darker.
pub fn cover_pair(palette: [[f32; 3]; 3]) -> ([f32; 3], [f32; 3]) {
    let mut best = (palette[0], palette[1]);
    let mut gap = -1.0;
    for i in 0..3 {
        for j in i + 1..3 {
            let g =
                (luma(palette[i]) - luma(palette[j])).abs() + 0.25 * dist(palette[i], palette[j]);
            if g > gap {
                gap = g;
                best = (palette[i], palette[j]);
            }
        }
    }
    let (mut a, mut b) = if luma(best.0) >= luma(best.1) {
        best
    } else {
        (best.1, best.0)
    };
    // Luma difference of at least 0.45 so the word stays readable.
    const MIN: f32 = 0.45;
    for _ in 0..12 {
        if luma(a) - luma(b) >= MIN {
            break;
        }
        a = a.map(|x| x + (1.0 - x) * 0.18);
        b = b.map(|x| x * 0.82);
    }
    (a, b)
}

/// Word shown at `pos`: index of the newest word that has started.
pub fn current_word(words: &[ChainWord], pos: Duration) -> Option<usize> {
    words.partition_point(|w| w.appear <= pos).checked_sub(1)
}

/// The word without punctuation at its ends (apostrophes inside stay).
pub fn display(word: &str) -> &str {
    let t = word.trim_matches(|c: char| !c.is_alphanumeric());
    if t.is_empty() { word } else { t }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inversion_pairs() {
        assert_eq!(invert([1.0, 1.0, 1.0]), [0.0, 0.0, 0.0]);
        assert_eq!(invert([1.0, 0.0, 0.0]), [0.0, 1.0, 1.0]);
    }

    #[test]
    fn mid_tones_get_contrast() {
        for c in [[0.5, 0.5, 0.5], [0.45, 0.5, 0.55], [0.6, 0.4, 0.5]] {
            let w = with_contrast(c);
            let gap = (luma(w) - luma(invert(w))).abs();
            assert!(gap >= 0.39, "{c:?} -> {w:?}: {gap}");
        }
        // Clear colors stay as they are.
        assert_eq!(with_contrast([1.0, 1.0, 1.0]), [1.0, 1.0, 1.0]);
        assert_eq!(with_contrast([0.0, 0.0, 0.2]), [0.0, 0.0, 0.2]);
    }

    #[test]
    fn few_cover_colors_get_neighbours() {
        let red = [0.9, 0.1, 0.1];
        let set = word_colors([red, red, red]);
        assert!(set.len() >= MIN_COLORS, "{set:?}");
        assert_eq!(set[0], [1.0, 1.0, 1.0]);
        for (i, a) in set.iter().enumerate() {
            for b in &set[i + 1..] {
                assert!(dist(*a, *b) >= DISTINCT);
            }
        }
    }

    #[test]
    fn colorful_cover_keeps_its_colors() {
        let set = word_colors([[0.9, 0.2, 0.1], [0.1, 0.3, 0.9], [0.95, 0.85, 0.1]]);
        assert!(set.len() >= 4);
        assert!(set.contains(&with_contrast([0.1, 0.3, 0.9])));
    }

    #[test]
    fn cover_pair_has_contrast() {
        // Two similar mid blues and a near copy.
        let p = [[0.3, 0.4, 0.8], [0.32, 0.42, 0.78], [0.28, 0.38, 0.82]];
        let (a, b) = cover_pair(p);
        assert!(luma(a) - luma(b) >= 0.45, "{a:?} {b:?}");
        // A dark and a light color stay as they are.
        let (a, b) = cover_pair([[0.95, 0.9, 0.2], [0.1, 0.05, 0.3], [0.5, 0.5, 0.5]]);
        assert_eq!(a, [0.95, 0.9, 0.2]);
        assert_eq!(b, [0.1, 0.05, 0.3]);
    }

    #[test]
    fn display_trims_punctuation() {
        assert_eq!(display("tonight\","), "tonight");
        assert_eq!(display("(Hey!)"), "Hey");
        assert_eq!(display("I've"), "I've");
        assert_eq!(display("…"), "…");
    }

    #[test]
    fn current_word_follows_time() {
        let w = |s: u64| ChainWord {
            text: "x".into(),
            appear: Duration::from_secs(s),
            keyword: false,
        };
        let words = [w(1), w(2), w(4)];
        assert_eq!(current_word(&words, Duration::from_millis(500)), None);
        assert_eq!(current_word(&words, Duration::from_secs(3)), Some(1));
        assert_eq!(current_word(&words, Duration::from_secs(9)), Some(2));
    }
}
