//! Color math: sRGB <-> linear <-> OKLab (Björn Ottosson), gamut clipping.

use crate::settings::model::Rgb;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Oklab {
    pub l: f32,
    pub a: f32,
    pub b: f32,
}

/// Linear sRGB, components nominally in 0..=1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinRgb {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg_attr(not(test), allow(dead_code))] // used by settings color pickers (phase 7)
pub fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

impl LinRgb {
    pub fn from_srgb8(r: u8, g: u8, b: u8) -> Self {
        let f = |v: u8| srgb_to_linear(v as f32 / 255.0);
        Self {
            r: f(r),
            g: f(g),
            b: f(b),
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn to_rgb(self) -> Rgb {
        let f = |v: f32| (linear_to_srgb(v) * 255.0).round() as u8;
        Rgb(f(self.r), f(self.g), f(self.b))
    }

    pub fn in_gamut(self) -> bool {
        const EPS: f32 = 1e-4;
        [self.r, self.g, self.b]
            .iter()
            .all(|&c| (-EPS..=1.0 + EPS).contains(&c))
    }
}

impl From<Rgb> for LinRgb {
    fn from(c: Rgb) -> Self {
        Self::from_srgb8(c.0, c.1, c.2)
    }
}

impl Oklab {
    pub fn from_linear(c: LinRgb) -> Self {
        let l = 0.412_221_47 * c.r + 0.536_332_55 * c.g + 0.051_445_995 * c.b;
        let m = 0.211_903_5 * c.r + 0.680_699_5 * c.g + 0.107_396_96 * c.b;
        let s = 0.088_302_46 * c.r + 0.281_718_85 * c.g + 0.629_978_7 * c.b;
        let (l, m, s) = (l.cbrt(), m.cbrt(), s.cbrt());
        Self {
            l: 0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
            a: 1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
            b: 0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
        }
    }

    pub fn to_linear(self) -> LinRgb {
        let l = self.l + 0.396_337_78 * self.a + 0.215_803_76 * self.b;
        let m = self.l - 0.105_561_346 * self.a - 0.063_854_17 * self.b;
        let s = self.l - 0.089_484_18 * self.a - 1.291_485_5 * self.b;
        let (l, m, s) = (l * l * l, m * m * m, s * s * s);
        LinRgb {
            r: 4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
            g: -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
            b: -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
        }
    }

    pub fn from_rgb(c: Rgb) -> Self {
        Self::from_linear(c.into())
    }

    pub fn chroma(self) -> f32 {
        self.a.hypot(self.b)
    }

    pub fn hue(self) -> f32 {
        self.b.atan2(self.a)
    }

    pub fn from_lch(l: f32, c: f32, h: f32) -> Self {
        Self {
            l,
            a: c * h.cos(),
            b: c * h.sin(),
        }
    }

    pub fn lerp(self, o: Self, t: f32) -> Self {
        Self {
            l: self.l + (o.l - self.l) * t,
            a: self.a + (o.a - self.a) * t,
            b: self.b + (o.b - self.b) * t,
        }
    }

    pub fn dist(self, o: Self) -> f32 {
        ((self.l - o.l).powi(2) + (self.a - o.a).powi(2) + (self.b - o.b).powi(2)).sqrt()
    }

    /// Reduces chroma (keeping lightness and hue) until the color fits sRGB.
    pub fn clip_to_gamut(self) -> Self {
        if self.to_linear().in_gamut() {
            return self;
        }
        let (l, h) = (self.l.clamp(0.0, 1.0), self.hue());
        let (mut lo, mut hi) = (0.0f32, self.chroma());
        for _ in 0..20 {
            let mid = (lo + hi) * 0.5;
            if Self::from_lch(l, mid, h).to_linear().in_gamut() {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        Self::from_lch(l, lo, h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_srgb() {
        for c in [
            Rgb(255, 46, 166),
            Rgb(0, 0, 0),
            Rgb(255, 255, 255),
            Rgb(12, 200, 90),
        ] {
            let back = Oklab::from_rgb(c).to_linear().to_rgb();
            assert_eq!(back, c);
        }
    }

    #[test]
    fn white_is_l1() {
        let w = Oklab::from_rgb(Rgb(255, 255, 255));
        assert!((w.l - 1.0).abs() < 1e-3 && w.chroma() < 1e-3);
    }

    #[test]
    fn gamut_clip_keeps_hue() {
        let c = Oklab::from_lch(0.7, 0.5, 1.0);
        assert!(!c.to_linear().in_gamut());
        let clipped = c.clip_to_gamut();
        assert!(clipped.to_linear().in_gamut());
        assert!((clipped.hue() - 1.0).abs() < 1e-3);
        assert!((clipped.l - 0.7).abs() < 1e-6);
    }
}
