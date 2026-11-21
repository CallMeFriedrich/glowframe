//! Drawing of the constellation view (SPEC §5.4c): the words of the current
//! line as four-pointed stars, connected one after another by thin lines.

use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED,
};

use super::*;
use crate::ui::anim::spring_step;
use crate::ui::stars::positions;

pub(super) struct StarFormats {
    word: IDWriteTextFormat,
    current: IDWriteTextFormat,
}

impl StarFormats {
    pub(super) fn new(dwrite: &IDWriteFactory, ui_font: &str, h: f32) -> Result<Self> {
        let (small, big) = sizes(h);
        Ok(Self {
            word: text_format(
                dwrite,
                ui_font,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                small,
                DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
                false,
            )?,
            current: text_format(
                dwrite,
                ui_font,
                DWRITE_FONT_WEIGHT_BOLD,
                big,
                DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
                false,
            )?,
        })
    }
}

/// Label sizes: older words, current word.
fn sizes(h: f32) -> (f32, f32) {
    let small = (0.021 * h).clamp(13.0, 40.0);
    (small, small * 1.55)
}

/// Seconds of spring time per revealed word (words are revealed in word
/// units; this turns them into spring time for the pop and line growth).
const WORD_SECS: f32 = 0.6;

/// Spring progress of word `k`: (star pop with overshoot, line growth).
fn word_springs(revealed: f32, k: usize) -> (f32, f32) {
    let t = (revealed - k as f32).max(0.0) * WORD_SECS;
    if revealed <= 0.0 && k == 0 {
        return (0.0, 0.0);
    }
    (
        spring_step(t, 0.42, 0.6),
        spring_step(t, 0.5, 0.92).min(1.0),
    )
}

/// Words of a line (whitespace separated).
fn words(s: &str) -> Vec<&str> {
    s.split_whitespace().collect()
}

pub(super) fn draw(p: &Painter, m: &UiModel, v: &StarsView, base: &Matrix3x2, alpha: f32) {
    let (w, h) = p.res.size;
    p.set_transform(base);
    let accent = m.accent;
    let change = v.change.clamp(0.0, 1.0);
    // Previous line: fades out and drifts up while the new one builds.
    if change < 1.0 && !v.previous.is_empty() {
        let drift = translate(0.0, -h * 0.03 * change);
        p.set_transform(&mat_mul(&drift, base));
        let n = words(v.previous).len() as f32;
        constellation(
            p,
            v.previous,
            v.previous_seed,
            n,
            None,
            accent,
            alpha * (1.0 - change),
        );
        p.set_transform(base);
    }
    if v.gap {
        let size = sizes(h).1;
        draw_dots(
            p,
            w / 2.0 - size * 0.65,
            h / 2.0,
            size,
            m.time,
            alpha * change,
        );
        return;
    }
    let current = v.revealed.floor().max(0.0) as usize;
    constellation(
        p,
        v.current,
        v.current_seed,
        v.revealed,
        Some(current),
        accent,
        alpha,
    );
}

/// One line as stars + connecting lines. `revealed` words are visible;
/// `current` is highlighted.
fn constellation(
    p: &Painter,
    text: &str,
    seed: u64,
    revealed: f32,
    current: Option<usize>,
    accent: [f32; 3],
    alpha: f32,
) {
    if alpha <= 0.0 {
        return;
    }
    let res = p.res;
    let (w, h) = res.size;
    let words = words(text);
    // Random points of the whole screen, new pattern per line.
    let pos = positions(words.len(), seed, w / h.max(1.0));
    let at = |i: usize| {
        let (u, v) = pos[i];
        (u * w, v * h)
    };
    let [r, g, b] = accent;
    let star_r = h * 0.0085;
    let line_w = (h * 0.0018).max(1.0);

    // Lines first, so stars sit on top.
    for k in 1..words.len() {
        let t = word_springs(revealed, k).1;
        if t <= 0.0 {
            continue;
        }
        let (x0, y0) = at(k - 1);
        let (x1, y1) = at(k);
        p.fill_color(color(r, g, b, 0.75 * alpha));
        // SAFETY: drawing between BeginDraw/EndDraw.
        unsafe {
            p.ctx.DrawLine(
                Vector2 { X: x0, Y: y0 },
                Vector2 {
                    X: x0 + (x1 - x0) * t,
                    Y: y0 + (y1 - y0) * t,
                },
                &res.brush,
                line_w,
                None,
            );
        }
    }

    let (small, big) = sizes(h);
    for (k, word) in words.iter().enumerate() {
        // Pop with a little overshoot; fully opaque early in the pop.
        let pop = word_springs(revealed, k).0;
        if pop <= 0.0 {
            continue;
        }
        let t = (pop * 2.5).clamp(0.0, 1.0);
        let (x, y) = at(k);
        let is_current = current == Some(k);
        let rad = star_r * if is_current { 1.7 } else { 1.0 } * pop.max(0.0);
        let a = alpha * t * if is_current { 1.0 } else { 0.78 };
        if is_current {
            p.circle(x, y, rad * 2.4, color(r, g, b, 0.18 * a));
            p.circle(x, y, rad * 1.5, color(r, g, b, 0.22 * a));
        }
        draw_star(p, x, y, rad, color(r, g, b, a));
        // Label under the star.
        let (format, size) = if is_current {
            (&res.stars.current, big)
        } else {
            (&res.stars.word, small)
        };
        let label = Rect::centered(x, y + rad * 1.6 + size * 0.75, w * 0.24, size * 1.5);
        // Labels mostly white with a hint of the accent: readable on images.
        let tint = |c: f32| c + (1.0 - c) * if is_current { 0.7 } else { 0.55 };
        p.text(word, format, label, color(tint(r), tint(g), tint(b), a));
    }
}

/// Four-pointed star (like a sparkle) centered at x/y.
fn draw_star(p: &Painter, x: f32, y: f32, r: f32, c: D2D1_COLOR_F) {
    // SAFETY: path geometry creation and drawing between BeginDraw/EndDraw.
    unsafe {
        let Ok(geo) = p.res.factory.CreatePathGeometry() else {
            return;
        };
        let Ok(sink) = geo.Open() else { return };
        let inner = r * 0.3;
        let pt = |dx: f32, dy: f32| Vector2 {
            X: x + dx,
            Y: y + dy,
        };
        sink.BeginFigure(pt(0.0, -r), D2D1_FIGURE_BEGIN_FILLED);
        for (dx, dy) in [
            (inner * 0.7, -inner * 0.7),
            (r, 0.0),
            (inner * 0.7, inner * 0.7),
            (0.0, r),
            (-inner * 0.7, inner * 0.7),
            (-r, 0.0),
            (-inner * 0.7, -inner * 0.7),
        ] {
            sink.AddLine(pt(dx, dy));
        }
        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
        if sink.Close().is_err() {
            return;
        }
        p.fill_color(c);
        p.ctx.FillGeometry(&geo, &p.res.brush, None);
    }
}
