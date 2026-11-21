//! Constellation view (SPEC §5.4c, v1.5): word positions of one lyric line
//! as a zigzag "star sign". Pure logic; drawing lives in `render::ui`.

/// Positions of `n` words in unit coordinates (0..1 both axes): evenly
/// spread from left to right, alternating between an upper and a lower
/// band with some per-line randomness (deterministic by `seed`).
pub fn positions(n: usize, seed: u64) -> Vec<(f32, f32)> {
    if n == 0 {
        return Vec::new();
    }
    let mut rng = seed | 1;
    let mut next = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng >> 40) as f32 / (1u64 << 24) as f32
    };
    // Start up or down at random; a short line uses less height.
    let start_up = next() < 0.5;
    let amp = if n <= 2 { 0.18 } else { 0.34 };
    (0..n)
        .map(|i| {
            let slot = 1.0 / n as f32;
            let x = if n == 1 {
                0.5
            } else {
                // Jitter inside the slot keeps the order (no crossing labels).
                slot * (i as f32 + 0.5) + slot * 0.28 * (next() - 0.5)
            };
            let up = (i % 2 == 0) == start_up;
            let depth = amp * (0.55 + 0.45 * next());
            let y = if up { 0.5 - depth } else { 0.5 + depth };
            (x.clamp(0.0, 1.0), y.clamp(0.0, 1.0))
        })
        .collect()
}

/// Smooth 0..1 appearance of word `k` given the revealed word count.
pub fn word_progress(revealed: f32, k: usize) -> f32 {
    let t = (revealed - k as f32).clamp(0.0, 1.0);
    // Faster than linear so a star is fully there early in its word.
    let t = (t * 2.5).min(1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zigzag_in_order_and_bounds() {
        for n in 1..16 {
            for seed in 0..20u64 {
                let p = positions(n, seed);
                assert_eq!(p.len(), n);
                assert!(
                    p.iter()
                        .all(|&(x, y)| (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y))
                );
                // Left to right, never on top of each other.
                assert!(p.windows(2).all(|w| w[1].0 > w[0].0));
                // Alternating bands.
                for w in p.windows(2) {
                    assert!((w[0].1 - 0.5) * (w[1].1 - 0.5) < 0.0);
                }
            }
        }
        assert_eq!(positions(5, 3), positions(5, 3));
        assert!(positions(0, 1).is_empty());
    }

    #[test]
    fn words_appear_one_after_another() {
        assert_eq!(word_progress(0.0, 0), 0.0);
        assert_eq!(word_progress(1.0, 0), 1.0);
        assert_eq!(word_progress(0.2, 1), 0.0);
        let mid = word_progress(1.2, 1);
        assert!(mid > 0.0 && mid < 1.0);
        assert_eq!(word_progress(5.0, 2), 1.0);
    }
}
