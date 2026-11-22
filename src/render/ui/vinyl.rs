//! Drawing of the record (SPEC §5.4d): black vinyl with grooves and a
//! standing light reflex, the cover as round label that turns.

use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED,
};

use super::*;

/// Label radius relative to the record (a 12" record has a 4" label;
/// slightly larger so the cover stays recognizable).
pub(super) const LABEL: f32 = 0.38;
/// Smooth "dead wax" ring between label and grooves.
const DEAD_WAX: f32 = 0.43;
/// Groove-free bands between tracks (relative radius, half width).
const GAPS: [(f32, f32); 2] = [(0.6, 0.012), (0.79, 0.01)];

pub(super) struct CornerFormats {
    pub title: IDWriteTextFormat,
    pub artist: IDWriteTextFormat,
    pub title_size: f32,
    pub artist_size: f32,
}

impl CornerFormats {
    pub(super) fn new(dwrite: &IDWriteFactory, ui_font: &str, h: f32) -> Result<Self> {
        let title_size = (0.032 * h).clamp(18.0, 68.0);
        let artist_size = title_size * 0.68;
        let make = |weight, size| -> Result<IDWriteTextFormat> {
            let f = text_format(
                dwrite,
                ui_font,
                weight,
                size,
                DWRITE_PARAGRAPH_ALIGNMENT_FAR,
                false,
            )?;
            // SAFETY: plain format setter.
            unsafe { f.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)? };
            Ok(f)
        };
        Ok(Self {
            title: make(DWRITE_FONT_WEIGHT_BOLD, title_size)?,
            artist: make(DWRITE_FONT_WEIGHT_NORMAL, artist_size)?,
            title_size,
            artist_size,
        })
    }
}

/// Title and artist in the bottom left corner of the idle record view.
pub(super) fn draw_corner_text(p: &Painter, title: &str, artist: &str, alpha: f32) {
    if alpha <= 0.0 {
        return;
    }
    let (w, h) = p.res.size;
    let f = &p.res.corner;
    let x = (0.045 * w).max(0.045 * h);
    let bottom = h - 0.07 * h;
    let text_w = w * 0.42;
    let artist_h = f.artist_size * 1.4;
    let title_h = f.title_size * 1.3;
    let artist_r = Rect::new(x, bottom - artist_h, text_w, artist_h);
    let title_r = Rect::new(x, artist_r.y - title_h - 0.004 * h, text_w, title_h);
    p.text(title, &f.title, title_r, color(1.0, 1.0, 1.0, 0.95 * alpha));
    p.text(
        artist,
        &f.artist,
        artist_r,
        color(1.0, 1.0, 1.0, 0.6 * alpha),
    );
}

/// Record filling the square `r`; `angle` turns the label (radians).
pub(super) fn draw_record(
    p: &Painter,
    img: Option<&Image>,
    r: Rect,
    angle: f32,
    pause: f32,
    alpha: f32,
) {
    if alpha <= 0.0 {
        return;
    }
    let (cx, cy) = r.center();
    let rad = r.w / 2.0;
    // Vinyl body with a faint lighter rim.
    p.circle(cx, cy, rad, color(0.03, 0.03, 0.035, alpha));
    ring(p, cx, cy, rad - 0.6, 1.2, color(1.0, 1.0, 1.0, 0.1 * alpha));

    // Grooves: fine concentric lines, skipping the gaps between tracks.
    let (from, to) = (DEAD_WAX + 0.02, 0.965);
    let n = 46;
    for i in 0..=n {
        let k = from + (to - from) * i as f32 / n as f32;
        if GAPS.iter().any(|(g, half)| (k - g).abs() < half * 2.2) {
            continue;
        }
        let a = if i % 3 == 0 { 0.07 } else { 0.035 };
        ring(p, cx, cy, rad * k, 0.7, color(1.0, 1.0, 1.0, a * alpha));
    }
    for (g, half) in GAPS {
        ring(
            p,
            cx,
            cy,
            rad * g,
            rad * half * 2.0,
            color(0.0, 0.0, 0.0, 0.5 * alpha),
        );
    }

    // Standing reflex: two opposite soft wedges of light (top left, bottom
    // right); several thin layers fake the soft falloff.
    for (span, a) in [(0.62, 0.018), (0.4, 0.024), (0.22, 0.03), (0.1, 0.03)] {
        for base in [-2.25_f32, 0.9] {
            wedge(
                p,
                cx,
                cy,
                rad * DEAD_WAX,
                rad * 0.985,
                base - span / 2.0,
                base + span / 2.0,
                color(1.0, 1.0, 1.0, a * alpha),
            );
        }
    }

    // Label: the cover cut round, turning with the platter.
    let lr = rad * LABEL;
    label(p, img, cx, cy, lr, angle, alpha);
    ring(p, cx, cy, lr, 1.2, color(0.0, 0.0, 0.0, 0.35 * alpha));
    // Paused: the label dims like the cover does.
    p.circle(cx, cy, lr, color(0.02, 0.02, 0.03, 0.42 * pause * alpha));
    // Spindle hole.
    p.circle(cx, cy, rad * 0.022, color(0.0, 0.0, 0.0, 0.9 * alpha));
    ring(
        p,
        cx,
        cy,
        rad * 0.022,
        1.0,
        color(1.0, 1.0, 1.0, 0.18 * alpha),
    );
}

fn label(p: &Painter, img: Option<&Image>, cx: f32, cy: f32, lr: f32, angle: f32, alpha: f32) {
    let Some(img) = img else {
        p.circle(cx, cy, lr, color(0.3, 0.3, 0.34, alpha));
        return;
    };
    let Some(brush) = p.res.bitmap_brush(p.ctx, img) else {
        return;
    };
    // Cover-fit the image into the label square, then turn about its center.
    let s = 2.0 * lr / img.width.min(img.height) as f32;
    let (iw, ih) = (img.width as f32 * s, img.height as f32 * s);
    let fit = Matrix3x2 {
        M11: s,
        M12: 0.0,
        M21: 0.0,
        M22: s,
        M31: cx - iw / 2.0,
        M32: cy - ih / 2.0,
    };
    let (sin, cos) = angle.sin_cos();
    let turn = Matrix3x2 {
        M11: cos,
        M12: sin,
        M21: -sin,
        M22: cos,
        M31: cx - cx * cos + cy * sin,
        M32: cy - cx * sin - cy * cos,
    };
    // SAFETY: called between BeginDraw/EndDraw.
    unsafe {
        brush.SetTransform(&mat_mul(&fit, &turn));
        brush.SetOpacity(alpha);
        p.ctx.FillEllipse(
            &D2D1_ELLIPSE {
                point: Vector2 { X: cx, Y: cy },
                radiusX: lr,
                radiusY: lr,
            },
            &brush,
        );
    }
}

fn ring(p: &Painter, cx: f32, cy: f32, rad: f32, width: f32, c: D2D1_COLOR_F) {
    if c.a <= 0.0 {
        return;
    }
    p.fill_color(c);
    // SAFETY: called between BeginDraw/EndDraw.
    unsafe {
        p.ctx.DrawEllipse(
            &D2D1_ELLIPSE {
                point: Vector2 { X: cx, Y: cy },
                radiusX: rad,
                radiusY: rad,
            },
            &p.res.brush,
            width,
            None,
        );
    }
}

/// Filled ring segment between two radii and two angles (radians).
#[allow(clippy::too_many_arguments)]
fn wedge(p: &Painter, cx: f32, cy: f32, r0: f32, r1: f32, a0: f32, a1: f32, c: D2D1_COLOR_F) {
    if c.a <= 0.0 {
        return;
    }
    const STEPS: usize = 14;
    let at = |r: f32, a: f32| Vector2 {
        X: cx + r * a.cos(),
        Y: cy + r * a.sin(),
    };
    // SAFETY: path geometry creation and drawing between BeginDraw/EndDraw.
    unsafe {
        let Ok(geo) = p.res.factory.CreatePathGeometry() else {
            return;
        };
        let Ok(sink) = geo.Open() else { return };
        sink.BeginFigure(at(r1, a0), D2D1_FIGURE_BEGIN_FILLED);
        for i in 1..=STEPS {
            sink.AddLine(at(r1, a0 + (a1 - a0) * i as f32 / STEPS as f32));
        }
        for i in (0..=STEPS).rev() {
            sink.AddLine(at(r0, a0 + (a1 - a0) * i as f32 / STEPS as f32));
        }
        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
        if sink.Close().is_err() {
            return;
        }
        p.fill_color(c);
        p.ctx.FillGeometry(&geo, &p.res.brush, None);
    }
}
