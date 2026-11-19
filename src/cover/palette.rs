//! Palette extraction from a cover (SPEC §6.3): OKLab k-means (k = 5) on a
//! 64x64 image, discard near-black/white/achromatic clusters unless the cover
//! is monochrome, sort by chroma x share, take 2-3 colors, boost for black.

use crate::color::{LinRgb, Oklab};

const K: usize = 5;
const ITERATIONS: usize = 12;
const MIN_SHARE: f32 = 0.03;
/// Colors closer than this (OKLab distance) count as duplicates.
const MIN_DISTANCE: f32 = 0.08;

#[derive(Debug, Clone, Copy)]
struct Cluster {
    center: Oklab,
    share: f32,
}

/// `bgra` is tightly packed 8-bit BGRA (alpha ignored). Returns 2-3 colors,
/// or `None` if the image is empty.
pub fn extract(bgra: &[u8]) -> Option<Vec<Oklab>> {
    let pixels: Vec<Oklab> = bgra
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| Oklab::from_linear(LinRgb::from_srgb8(p[2], p[1], p[0])))
        .collect();
    if pixels.is_empty() {
        return None;
    }
    let clusters = kmeans(&pixels);

    let colorful = |c: &&Cluster| {
        let (l, ch) = (c.center.l, c.center.chroma());
        l >= 0.18 && !(l > 0.92 && ch < 0.05) && ch >= 0.04
    };
    let mut picked: Vec<&Cluster> = clusters.iter().filter(colorful).collect();
    let monochrome = picked.is_empty();
    if monochrome {
        // Durchgehend monochrom: use the tones themselves, brightest first
        // among the common ones, skipping pure black where possible.
        picked = clusters.iter().filter(|c| c.center.l >= 0.1).collect();
        if picked.is_empty() {
            picked = clusters.iter().collect();
        }
        picked.sort_by(|a, b| b.share.total_cmp(&a.share));
    } else {
        picked.sort_by(|a, b| {
            (b.center.chroma() * b.share).total_cmp(&(a.center.chroma() * a.share))
        });
    }

    let mut out: Vec<Oklab> = Vec::with_capacity(3);
    for c in picked {
        if out.len() == 3 {
            break;
        }
        if c.share < MIN_SHARE && !out.is_empty() {
            continue;
        }
        if out.iter().all(|o| o.dist(c.center) >= MIN_DISTANCE) {
            out.push(c.center);
        }
    }
    if out.len() < 2 {
        // Derive a second color by rotating the hue.
        let base = out[0];
        let h = base.hue() + 0.7;
        out.push(Oklab::from_lch(base.l + 0.05, base.chroma(), h));
    }
    Some(out.into_iter().map(|c| boost(c, monochrome)).collect())
}

/// Raises lightness and chroma so the color glows on black.
fn boost(c: Oklab, monochrome: bool) -> Oklab {
    let l = c.l.clamp(0.6, 0.76);
    let ch = if monochrome {
        c.chroma()
    } else {
        (c.chroma() * 1.5).max(0.16)
    };
    Oklab::from_lch(l, ch, c.hue()).clip_to_gamut()
}

fn kmeans(pixels: &[Oklab]) -> Vec<Cluster> {
    let mut centers = init_centers(pixels);
    let mut assign = vec![0usize; pixels.len()];
    for _ in 0..ITERATIONS {
        let mut changed = false;
        for (i, p) in pixels.iter().enumerate() {
            let best = nearest(&centers, *p);
            if assign[i] != best {
                assign[i] = best;
                changed = true;
            }
        }
        let mut sums = vec![(0.0f32, 0.0f32, 0.0f32, 0usize); centers.len()];
        for (p, &a) in pixels.iter().zip(&assign) {
            let s = &mut sums[a];
            s.0 += p.l;
            s.1 += p.a;
            s.2 += p.b;
            s.3 += 1;
        }
        for (c, s) in centers.iter_mut().zip(&sums) {
            if s.3 > 0 {
                let n = s.3 as f32;
                *c = Oklab {
                    l: s.0 / n,
                    a: s.1 / n,
                    b: s.2 / n,
                };
            }
        }
        if !changed {
            break;
        }
    }
    let mut counts = vec![0usize; centers.len()];
    for &a in &assign {
        counts[a] += 1;
    }
    let total = pixels.len() as f32;
    centers
        .into_iter()
        .zip(counts)
        .filter(|(_, n)| *n > 0)
        .map(|(center, n)| Cluster {
            center,
            share: n as f32 / total,
        })
        .collect()
}

/// Deterministic k-means++-style init: start with the pixel farthest from the
/// mean, then repeatedly add the pixel farthest from all chosen centers.
fn init_centers(pixels: &[Oklab]) -> Vec<Oklab> {
    let n = pixels.len() as f32;
    let mean = pixels.iter().fold(
        Oklab {
            l: 0.0,
            a: 0.0,
            b: 0.0,
        },
        |acc, p| Oklab {
            l: acc.l + p.l / n,
            a: acc.a + p.a / n,
            b: acc.b + p.b / n,
        },
    );
    let mut centers = Vec::with_capacity(K);
    let first = pixels
        .iter()
        .copied()
        .max_by(|x, y| x.dist(mean).total_cmp(&y.dist(mean)))
        .unwrap_or(mean);
    centers.push(first);
    while centers.len() < K {
        let next = pixels.iter().copied().max_by(|x, y| {
            let dx = centers.iter().map(|c| c.dist(*x)).fold(f32::MAX, f32::min);
            let dy = centers.iter().map(|c| c.dist(*y)).fold(f32::MAX, f32::min);
            dx.total_cmp(&dy)
        });
        match next {
            Some(p) if centers.iter().all(|c| c.dist(p) > 1e-4) => centers.push(p),
            _ => break, // fewer distinct colors than K
        }
    }
    centers
}

fn nearest(centers: &[Oklab], p: Oklab) -> usize {
    centers
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| a.dist(p).total_cmp(&b.dist(p)))
        .map_or(0, |(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a BGRA image from (r, g, b, pixel count) blocks.
    fn image(blocks: &[(u8, u8, u8, usize)]) -> Vec<u8> {
        blocks
            .iter()
            .flat_map(|&(r, g, b, n)| std::iter::repeat_n([b, g, r, 255], n).flatten())
            .collect()
    }

    fn hue_deg(c: Oklab) -> f32 {
        c.hue().to_degrees().rem_euclid(360.0)
    }

    #[test]
    fn picks_dominant_colors_ignoring_black() {
        let img = image(&[(0, 0, 0, 2000), (230, 30, 40, 1200), (30, 60, 220, 896)]);
        let p = extract(&img).unwrap();
        assert!(p.len() >= 2 && p.len() <= 3);
        // Red (~25-30°) and blue (~260-265°) must be present, no gray.
        assert!(p.iter().any(|c| (10.0..50.0).contains(&hue_deg(*c))));
        assert!(p.iter().any(|c| (240.0..290.0).contains(&hue_deg(*c))));
        assert!(p.iter().all(|c| c.chroma() > 0.1 && c.l >= 0.6));
        assert!(p.iter().all(|c| c.to_linear().in_gamut()));
    }

    #[test]
    fn single_color_gets_second() {
        let p = extract(&image(&[(200, 40, 160, 4096)])).unwrap();
        assert_eq!(p.len(), 2);
        assert!(p[0].dist(p[1]) > 0.05);
    }

    #[test]
    fn monochrome_cover_works() {
        let p = extract(&image(&[(20, 20, 20, 2000), (180, 180, 180, 2096)])).unwrap();
        assert!(p.len() >= 2);
        assert!(p.iter().all(|c| c.l >= 0.6));
    }

    #[test]
    fn all_black_and_empty() {
        assert!(extract(&image(&[(0, 0, 0, 4096)])).is_some());
        assert!(extract(&[]).is_none());
    }
}
