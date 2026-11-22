//! Constellation view (SPEC §5.4c, v1.5): word positions of one lyric line
//! as a "star sign" at random points of the whole screen. Pure logic;
//! drawing lives in `render::ui`.

/// Safe area in unit coordinates: below the clock, away from the edges
/// (the glowing frame) and with room for the label under each star.
pub(crate) const AREA: (f32, f32, f32, f32) = (0.07, 0.2, 0.93, 0.86);
const CANDIDATES: usize = 60;

pub(crate) struct Rng(pub(crate) u64);

impl Rng {
    pub(crate) fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Distance in screen proportions (x stretched by the aspect ratio).
pub(crate) fn dist(a: (f32, f32), b: (f32, f32), aspect: f32) -> f32 {
    ((a.0 - b.0) * aspect).hypot(a.1 - b.1)
}

fn orient(a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> f32 {
    (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
}

/// Proper intersection of segments a-b and c-d (shared endpoints excluded).
pub fn segments_cross(a: (f32, f32), b: (f32, f32), c: (f32, f32), d: (f32, f32)) -> bool {
    let (d1, d2) = (orient(c, d, a), orient(c, d, b));
    let (d3, d4) = (orient(a, b, c), orient(a, b, d));
    d1 * d2 < 0.0 && d3 * d4 < 0.0
}

/// Distance of point `p` to segment a-b (aspect corrected).
fn point_segment_dist(p: (f32, f32), a: (f32, f32), b: (f32, f32), aspect: f32) -> f32 {
    let s = |q: (f32, f32)| (q.0 * aspect, q.1);
    let (p, a, b) = (s(p), s(a), s(b));
    let ab = (b.0 - a.0, b.1 - a.1);
    let len2 = ab.0 * ab.0 + ab.1 * ab.1;
    let t = if len2 > 0.0 {
        (((p.0 - a.0) * ab.0 + (p.1 - a.1) * ab.1) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p.0 - (a.0 + ab.0 * t)).hypot(p.1 - (a.1 + ab.1 * t))
}

/// Positions of `n` words in unit coordinates (0..1 both axes) on a screen
/// with `aspect` = width / height. Random per line (`seed`), but clean:
/// minimum distance between stars, limited segment length, no crossing
/// segments and no star sitting on a line. Constraints relax step by step
/// if a placement is impossible, so this always returns `n` points.
/// Last resort when the area is crowded: never cross a line (the hard
/// rule), but allow short segments and tight spacing. A crossing-free
/// point always exists close to the previous star.
fn fallback(pts: &[(f32, f32)], rng: &mut Rng, aspect: f32) -> (f32, f32) {
    let (x0, y0, x1, y1) = AREA;
    let Some(&prev) = pts.last() else {
        return ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    };
    let mut best: Option<((f32, f32), f32)> = None;
    for k in 0..600 {
        // Search outward from the previous star.
        let radius = 0.04 + 0.5 * (k as f32 / 600.0);
        let c = (
            (prev.0 + (rng.next() - 0.5) * 2.0 * radius / aspect).clamp(x0, x1),
            (prev.1 + (rng.next() - 0.5) * 2.0 * radius).clamp(y0, y1),
        );
        if pts.windows(2).any(|w| segments_cross(w[0], w[1], prev, c)) {
            continue;
        }
        let near = pts
            .iter()
            .map(|p| dist(*p, c, aspect))
            .fold(f32::MAX, f32::min);
        if best.is_none_or(|(_, d)| near > d) {
            best = Some((c, near));
        }
    }
    best.map_or(prev, |(c, _)| c)
}

pub fn positions(n: usize, seed: u64, aspect: f32) -> Vec<(f32, f32)> {
    let aspect = aspect.clamp(0.5, 4.0);
    // Mix the seed so neighbouring seeds give unrelated patterns.
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let (x0, y0, x1, y1) = AREA;
    let mut pts: Vec<(f32, f32)> = Vec::with_capacity(n);
    // Fewer words -> more space each.
    let base_min = (0.75 / (n.max(1) as f32).sqrt()).clamp(0.16, 0.42);
    for _ in 0..n {
        let mut relax = 1.0f32;
        let placed = loop {
            let min_d = base_min * relax;
            let (seg_min, seg_max) = (0.14 * relax, 0.62 / relax.max(0.3));
            let mut best: Option<((f32, f32), f32)> = None;
            for _ in 0..CANDIDATES {
                let c = (x0 + (x1 - x0) * rng.next(), y0 + (y1 - y0) * rng.next());
                let near = pts
                    .iter()
                    .map(|p| dist(*p, c, aspect))
                    .fold(f32::MAX, f32::min);
                if near < min_d {
                    continue;
                }
                if let Some(&prev) = pts.last() {
                    let len = dist(prev, c, aspect);
                    if len < seg_min || len > seg_max {
                        continue;
                    }
                    // The new segment must not cross earlier ones or run
                    // through an earlier star.
                    let crosses = pts.windows(2).any(|w| segments_cross(w[0], w[1], prev, c));
                    let grazes = pts[..pts.len() - 1]
                        .iter()
                        .any(|p| point_segment_dist(*p, prev, c, aspect) < min_d * 0.45);
                    if crosses || grazes {
                        continue;
                    }
                }
                // Prefer the candidate with the most room around it.
                if best.is_none_or(|(_, d)| near > d) {
                    best = Some((c, near));
                }
            }
            if let Some((c, _)) = best {
                break c;
            }
            if relax < 0.2 {
                break fallback(&pts, &mut rng, aspect);
            }
            relax *= 0.8;
        };
        pts.push(placed);
    }
    pts
}

#[cfg(test)]
mod tests {
    use super::*;

    const ASPECT: f32 = 16.0 / 9.0;

    #[test]
    fn random_but_clean() {
        let mut crossings = 0;
        let mut patterns = std::collections::HashSet::new();
        for n in 1..13 {
            for seed in 0..40u64 {
                let p = positions(n, seed, ASPECT);
                assert_eq!(p.len(), n);
                let (x0, y0, x1, y1) = AREA;
                assert!(
                    p.iter()
                        .all(|&(x, y)| (x0..=x1).contains(&x) && (y0..=y1).contains(&y))
                );
                for i in 1..n {
                    for j in 1..i.saturating_sub(1) {
                        if segments_cross(p[j - 1], p[j], p[i - 1], p[i]) {
                            crossings += 1;
                        }
                    }
                }
                if n == 6 {
                    patterns.insert(format!("{:.2}", p[3].0 + p[3].1));
                }
            }
        }
        // Lines never cross.
        assert_eq!(crossings, 0);
        // Different lines give different patterns.
        assert!(patterns.len() > 30);
        assert_eq!(positions(5, 3, ASPECT), positions(5, 3, ASPECT));
        assert!(positions(0, 1, ASPECT).is_empty());
    }

    #[test]
    fn stars_keep_their_distance() {
        for seed in 0..50u64 {
            let p = positions(7, seed, ASPECT);
            for i in 0..p.len() {
                for j in 0..i {
                    assert!(dist(p[i], p[j], ASPECT) > 0.05, "seed {seed}");
                }
            }
        }
    }

    #[test]
    fn crossing_detection() {
        assert!(segments_cross(
            (0.0, 0.0),
            (1.0, 1.0),
            (0.0, 1.0),
            (1.0, 0.0)
        ));
        assert!(!segments_cross(
            (0.0, 0.0),
            (1.0, 0.0),
            (0.0, 1.0),
            (1.0, 1.0)
        ));
        // Shared endpoint is not a crossing.
        assert!(!segments_cross(
            (0.0, 0.0),
            (1.0, 1.0),
            (1.0, 1.0),
            (2.0, 0.0)
        ));
    }
}
