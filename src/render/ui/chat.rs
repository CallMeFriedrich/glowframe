//! Drawing of the messages view (SPEC §5.4b): contact header, iMessage-style
//! bubbles with tails, word-by-word reveal, typing indicator, map bubbles.

use std::mem::ManuallyDrop;

use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_BEZIER_SEGMENT, D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_LAYER_OPTIONS1_NONE, D2D1_LAYER_PARAMETERS1,
    D2D1_QUADRATIC_BEZIER_SEGMENT, ID2D1Geometry, ID2D1Layer, ID2D1PathGeometry,
};
use windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_RANGE;
use windows::core::IUnknown;

use super::*;
use crate::ui::chat::{mini_map, word_ranges_utf16};
use crate::ui::layout::ChatLayout;

/// Location glyph (Segoe Fluent Icons / MDL2 "Location").
const GLYPH_PIN: char = '\u{E81D}';

/// iOS dark mode colors.
const GRAY: [f32; 3] = [0.17, 0.17, 0.18];
const BLUE: [f32; 3] = [0.04, 0.52, 1.0];
const CARD: [f32; 3] = [0.15, 0.15, 0.16];
/// Opacity of words not sung yet.
const DIM: f32 = 0.42;

pub(super) struct ChatFormats {
    text: IDWriteTextFormat,
    bold: IDWriteTextFormat,
    small: IDWriteTextFormat,
    title: IDWriteTextFormat,
    artist: IDWriteTextFormat,
    icon: IDWriteTextFormat,
}

impl ChatFormats {
    pub(super) fn new(
        dwrite: &IDWriteFactory,
        ui_font: &str,
        icon_font: &str,
        w: f32,
        h: f32,
    ) -> Result<Self> {
        let l = layout::chat(w, h);
        let leading = |weight, size| -> Result<IDWriteTextFormat> {
            let f = text_format(
                dwrite,
                ui_font,
                weight,
                size,
                DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
                true,
            )?;
            // SAFETY: plain setter on a valid format.
            unsafe { f.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)? };
            Ok(f)
        };
        Ok(Self {
            text: leading(DWRITE_FONT_WEIGHT_NORMAL, l.font)?,
            bold: leading(DWRITE_FONT_WEIGHT_SEMI_BOLD, l.font)?,
            small: leading(DWRITE_FONT_WEIGHT_NORMAL, l.font * 0.66)?,
            title: text_format(
                dwrite,
                ui_font,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                l.title_size,
                DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                false,
            )?,
            artist: text_format(
                dwrite,
                ui_font,
                DWRITE_FONT_WEIGHT_NORMAL,
                l.artist_size,
                DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                false,
            )?,
            icon: text_format(
                dwrite,
                icon_font,
                DWRITE_FONT_WEIGHT_NORMAL,
                l.font * 0.62,
                DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                false,
            )?,
        })
    }
}

/// Measured size of one bubble.
struct Geo {
    w: f32,
    h: f32,
    /// Size of the lyric text block.
    text_w: f32,
    text_h: f32,
}

fn text_size(res: &UiResources, s: &str, format: &IDWriteTextFormat, max_w: f32) -> (f32, f32) {
    let Some(tl) = res.text_layout(s, format, max_w, 10_000.0) else {
        return (0.0, 0.0);
    };
    let mut m = DWRITE_TEXT_METRICS::default();
    // SAFETY: out-pointer to a local struct.
    match unsafe { tl.GetMetrics(&mut m) } {
        Ok(()) => (m.width, m.height),
        Err(_) => (0.0, 0.0),
    }
}

fn measure(res: &UiResources, l: &ChatLayout, b: &ChatBubble) -> Geo {
    let line_h = l.font * 1.33;
    match b.kind {
        BubbleKind::Typing => Geo {
            w: l.font * 2.9,
            h: line_h + 2.0 * l.pad_y,
            text_w: 0.0,
            text_h: line_h,
        },
        BubbleKind::Text => {
            let (tw, th) = text_size(res, b.text, &res.chat.text, l.max_text_w);
            let th = th.max(line_h);
            Geo {
                w: tw + 2.0 * l.pad_x,
                h: th + 2.0 * l.pad_y,
                text_w: tw,
                text_h: th,
            }
        }
        BubbleKind::Map { .. } => {
            let inner = l.map_w - 2.0 * l.pad_x;
            let (tw, th) = text_size(res, b.text, &res.chat.bold, inner);
            let label_h = l.font * 0.66 * 1.45;
            Geo {
                w: l.map_w,
                h: l.map_h + l.pad_y * 0.9 + th + l.pad_y * 0.2 + label_h + l.pad_y * 0.9,
                text_w: tw,
                text_h: th,
            }
        }
    }
}

fn gap(l: &ChatLayout, a: Side, b: Side) -> f32 {
    if a == b { l.font * 0.16 } else { l.font * 0.52 }
}

pub(super) fn draw(p: &Painter, m: &UiModel, c: &ChatView, base: &Matrix3x2, alpha: f32) {
    let res = p.res;
    let (w, h) = res.size;
    let l = layout::chat(w, h);
    p.set_transform(base);
    draw_header(p, c, &l, alpha);

    let n = c.bubbles.len();
    if n == 0 {
        return;
    }
    let geos: Vec<Geo> = c.bubbles.iter().map(|b| measure(res, &l, b)).collect();
    // Newest bubble sits on the anchor; while the scroll spring catches up
    // the whole thread is pushed down by the new bubble's height.
    let newest = &c.bubbles[n - 1];
    let step = geos[n - 1].h
        + if n > 1 {
            gap(&l, c.bubbles[n - 2].side, newest.side)
        } else {
            0.0
        };
    let lag = (newest.seq as f32 - c.scroll).max(0.0);
    let mut bottoms = vec![0.0; n];
    bottoms[n - 1] = l.anchor + lag * step;
    for k in (0..n - 1).rev() {
        bottoms[k] =
            bottoms[k + 1] - geos[k + 1].h - gap(&l, c.bubbles[k].side, c.bubbles[k + 1].side);
    }
    // Bubbles fade out by their top edge and are clipped below the header,
    // so tall (map) bubbles never cover the title.
    let fade_zone = h * 0.07;
    let clip_top = l.top - fade_zone * 0.5;
    // SAFETY: clip push/pop pair between BeginDraw/EndDraw.
    unsafe {
        p.ctx.PushAxisAlignedClip(
            &d2d_rect(Rect::new(0.0, clip_top, w, h - clip_top)),
            D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
        );
    }
    for (k, b) in c.bubbles.iter().enumerate() {
        let g = &geos[k];
        let bottom = bottoms[k];
        let top_fade = ((bottom - g.h - clip_top) / fade_zone + 0.5).clamp(0.0, 1.0);
        let pop_a = (b.appear * 2.2).clamp(0.0, 1.0);
        let a = alpha * top_fade * pop_a;
        if a <= 0.0 {
            continue;
        }
        let x = match b.side {
            Side::Left => l.column.x,
            Side::Right => l.column.x + l.column.w - g.w,
        };
        let rect = Rect::new(x, bottom - g.h, g.w, g.h);
        // Pop from the tail corner with spring overshoot.
        let scale = 0.55 + 0.45 * b.appear;
        let (ox, oy) = match b.side {
            Side::Left => (rect.x, bottom),
            Side::Right => (rect.x + rect.w, bottom),
        };
        let local = mat_mul(&scale_about(scale, ox, oy), base);
        p.set_transform(&local);
        let tail = k + 1 == n || c.bubbles[k + 1].side != b.side;
        draw_bubble(p, m, c, b, g, &l, rect, tail, a);
    }
    p.set_transform(base);
    // SAFETY: matches the push above.
    unsafe { p.ctx.PopAxisAlignedClip() };
}

fn draw_header(p: &Painter, c: &ChatView, l: &ChatLayout, alpha: f32) {
    let res = p.res;
    let r = l.avatar;
    let (cx, cy) = r.center();
    let brush = c
        .avatar
        .and_then(|img| Some((res.bitmap_brush(p.ctx, img)?, img)));
    match brush {
        Some((brush, img)) => {
            // SAFETY: drawing between BeginDraw/EndDraw.
            unsafe {
                brush.SetTransform(&Matrix3x2 {
                    M11: r.w / img.width as f32,
                    M12: 0.0,
                    M21: 0.0,
                    M22: r.h / img.height as f32,
                    M31: r.x,
                    M32: r.y,
                });
                brush.SetOpacity(alpha);
                p.ctx.FillEllipse(
                    &D2D1_ELLIPSE {
                        point: Vector2 { X: cx, Y: cy },
                        radiusX: r.w / 2.0,
                        radiusY: r.h / 2.0,
                    },
                    &brush,
                );
            }
        }
        None => p.circle(cx, cy, r.w / 2.0, color(1.0, 1.0, 1.0, 0.1 * alpha)),
    }
    p.text(
        c.title,
        &res.chat.title,
        l.title,
        color(1.0, 1.0, 1.0, 0.95 * alpha),
    );
    p.text(
        c.artist,
        &res.chat.artist,
        l.artist,
        color(1.0, 1.0, 1.0, 0.5 * alpha),
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_bubble(
    p: &Painter,
    m: &UiModel,
    c: &ChatView,
    b: &ChatBubble,
    g: &Geo,
    l: &ChatLayout,
    rect: Rect,
    tail: bool,
    a: f32,
) {
    let res = p.res;
    let fill = match (b.kind, b.side) {
        (BubbleKind::Map { .. }, _) => CARD,
        (_, Side::Left) => GRAY,
        (_, Side::Right) => BLUE,
    };
    let fill_c = color(fill[0], fill[1], fill[2], a);
    if tail {
        draw_tail(p, rect, b.side, l.radius * 0.55, fill_c);
    }
    p.rounded(rect, l.radius.min(rect.h / 2.0), fill_c);

    match b.kind {
        BubbleKind::Typing => {
            let (cx, cy) = rect.center();
            for k in 0..3 {
                let phase = (m.time * 1.6 - k as f32 * 0.18) * std::f32::consts::TAU;
                let pulse = 0.5 + 0.5 * phase.sin();
                p.circle(
                    cx + (k as f32 - 1.0) * l.font * 0.55,
                    cy,
                    l.font * (0.12 + 0.025 * pulse),
                    color(1.0, 1.0, 1.0, (0.3 + 0.55 * pulse) * a),
                );
            }
        }
        BubbleKind::Text => {
            let origin = (
                rect.x + l.pad_x,
                rect.y + l.pad_y + (g.h - 2.0 * l.pad_y - g.text_h) / 2.0,
            );
            draw_text(p, b.text, &res.chat.text, origin, l.max_text_w, b.reveal, a);
        }
        BubbleKind::Map { label, seed } => {
            let header = Rect::new(rect.x, rect.y, rect.w, l.map_h);
            draw_map(
                p,
                rect,
                header,
                l.radius.min(rect.h / 2.0),
                seed,
                c.maps_label,
                a,
            );
            let ty = header.y + header.h + l.pad_y * 0.9;
            let inner = l.map_w - 2.0 * l.pad_x;
            draw_text(
                p,
                b.text,
                &res.chat.bold,
                (rect.x + l.pad_x, ty),
                inner,
                b.reveal,
                a,
            );
            let ly = ty + g.text_h + l.pad_y * 0.2;
            let small = l.font * 0.66;
            let icon_w = small * 1.3;
            p.text(
                &GLYPH_PIN.to_string(),
                &res.chat.icon,
                Rect::new(rect.x + l.pad_x - small * 0.15, ly, icon_w, small * 1.45),
                color(1.0, 1.0, 1.0, 0.55 * a),
            );
            p.text(
                label,
                &res.chat.small,
                Rect::new(rect.x + l.pad_x + icon_w, ly, inner - icon_w, small * 1.45),
                color(1.0, 1.0, 1.0, 0.55 * a),
            );
            let _ = g.text_w;
        }
    }
}

/// Lyric text; the line being sung reveals its words one after another.
fn draw_text(
    p: &Painter,
    s: &str,
    format: &IDWriteTextFormat,
    origin: (f32, f32),
    max_w: f32,
    reveal: Option<f32>,
    a: f32,
) {
    let res = p.res;
    let words = word_ranges_utf16(s);
    let revealed = reveal.unwrap_or(f32::MAX);
    if revealed >= words.len() as f32 {
        // Fully shown: cached layout.
        if let Some(tl) = res.text_layout(s, format, max_w, 10_000.0) {
            p.layout_at(&tl, origin.0, origin.1, color(1.0, 1.0, 1.0, a));
        }
        return;
    }
    // Fresh layout: drawing effects must not leak into the shared cache.
    let wide: Vec<u16> = s.encode_utf16().collect();
    // SAFETY: DirectWrite calls with a valid format and ranges inside the text.
    unsafe {
        let Ok(tl) = res.dwrite.CreateTextLayout(&wide, format, max_w, 10_000.0) else {
            return;
        };
        let full = revealed.floor().max(0.0) as usize;
        let frac = revealed - full as f32;
        res.brush_hi.SetColor(&color(1.0, 1.0, 1.0, a));
        res.brush_fade
            .SetColor(&color(1.0, 1.0, 1.0, a * (DIM + (1.0 - DIM) * frac)));
        if full > 0
            && let Some(&(start, len)) = words.get(full - 1)
            && let Ok(hi) = res.brush_hi.cast::<IUnknown>()
        {
            let _ = tl.SetDrawingEffect(
                &hi,
                DWRITE_TEXT_RANGE {
                    startPosition: 0,
                    length: start + len,
                },
            );
        }
        if frac > 0.0
            && let Some(&(start, len)) = words.get(full)
            && let Ok(fade) = res.brush_fade.cast::<IUnknown>()
        {
            let _ = tl.SetDrawingEffect(
                &fade,
                DWRITE_TEXT_RANGE {
                    startPosition: start,
                    length: len,
                },
            );
        }
        p.layout_at(&tl, origin.0, origin.1, color(1.0, 1.0, 1.0, a * DIM));
    }
}

fn path(
    p: &Painter,
    build: impl FnOnce(&windows::Win32::Graphics::Direct2D::ID2D1GeometrySink),
) -> Option<ID2D1PathGeometry> {
    // SAFETY: path geometry creation and recording on a valid factory.
    unsafe {
        let geo = p.res.factory.CreatePathGeometry().ok()?;
        let sink = geo.Open().ok()?;
        build(&sink);
        sink.Close().ok()?;
        Some(geo)
    }
}

/// iMessage tail at the bottom corner on the bubble's side.
fn draw_tail(p: &Painter, r: Rect, side: Side, s: f32, c: D2D1_COLOR_F) {
    let (edge, dir) = match side {
        Side::Left => (r.x, -1.0),
        Side::Right => (r.x + r.w, 1.0),
    };
    let y = r.y + r.h;
    let pt = |dx: f32, dy: f32| Vector2 {
        X: edge + dir * dx,
        Y: y + dy,
    };
    let Some(geo) = path(p, |sink| {
        // SAFETY: recording into an open sink.
        unsafe {
            sink.BeginFigure(pt(-s * 1.6, -s * 1.4), D2D1_FIGURE_BEGIN_FILLED);
            sink.AddLine(pt(-s * 0.05, -s * 1.4));
            sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                point1: pt(0.0, -s * 0.55),
                point2: pt(s * 0.15, -s * 0.05),
                point3: pt(s * 0.6, s * 0.02),
            });
            sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                point1: pt(-s * 0.15, s * 0.08),
                point2: pt(-s * 0.6, s * 0.02),
                point3: pt(-s * 1.0, -s * 0.12),
            });
            sink.AddLine(pt(-s * 1.6, -s * 0.12));
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
        }
    }) else {
        return;
    };
    p.fill_color(c);
    // SAFETY: drawing between BeginDraw/EndDraw.
    unsafe { p.ctx.FillGeometry(&geo, &p.res.brush, None) };
}

/// Procedural mini map clipped to the card's rounded top.
fn draw_map(p: &Painter, card: Rect, r: Rect, radius: f32, seed: u64, maps_label: &str, a: f32) {
    let res = p.res;
    let mm = mini_map(seed);
    let (bg, water, road) = match mm.theme {
        0 => ([0.12, 0.15, 0.21], [0.09, 0.19, 0.32], [0.36, 0.40, 0.49]),
        1 => ([0.13, 0.21, 0.17], [0.09, 0.23, 0.29], [0.40, 0.47, 0.42]),
        _ => ([0.19, 0.15, 0.25], [0.13, 0.17, 0.33], [0.46, 0.41, 0.53]),
    };
    let at = |u: f32, v: f32| Vector2 {
        X: r.x + u * r.w,
        Y: r.y + v * r.h,
    };
    // SAFETY: geometry creation and layer push/pop between BeginDraw/EndDraw.
    unsafe {
        let Ok(mask) = res
            .factory
            .CreateRoundedRectangleGeometry(&D2D1_ROUNDED_RECT {
                rect: d2d_rect(card),
                radiusX: radius,
                radiusY: radius,
            })
        else {
            return;
        };
        let Ok(mask) = mask.cast::<ID2D1Geometry>() else {
            return;
        };
        let params = D2D1_LAYER_PARAMETERS1 {
            contentBounds: d2d_rect(r),
            geometricMask: ManuallyDrop::new(Some(mask)),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: scale_about(1.0, 0.0, 0.0),
            opacity: 1.0,
            opacityBrush: ManuallyDrop::new(None),
            layerOptions: D2D1_LAYER_OPTIONS1_NONE,
        };
        p.ctx.PushLayer(&params, None::<&ID2D1Layer>);

        p.fill_color(color(bg[0], bg[1], bg[2], a));
        p.ctx.FillRectangle(&d2d_rect(r), &res.brush);
        if let Some(poly) = &mm.water
            && let Some(geo) = path(p, |sink| {
                sink.BeginFigure(at(poly[0].0, poly[0].1), D2D1_FIGURE_BEGIN_FILLED);
                for q in &poly[1..] {
                    sink.AddLine(at(q.0, q.1));
                }
                sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            })
        {
            p.fill_color(color(water[0], water[1], water[2], a));
            p.ctx.FillGeometry(&geo, &res.brush, None);
        }
        for (seg, wr) in mm.roads.iter().zip(&mm.road_widths) {
            let Some(geo) = path(p, |sink| {
                sink.BeginFigure(at(seg[0].0, seg[0].1), D2D1_FIGURE_BEGIN_FILLED);
                sink.AddQuadraticBezier(&D2D1_QUADRATIC_BEZIER_SEGMENT {
                    point1: at(seg[1].0, seg[1].1),
                    point2: at(seg[2].0, seg[2].1),
                });
                sink.EndFigure(windows::Win32::Graphics::Direct2D::Common::D2D1_FIGURE_END_OPEN);
            }) else {
                continue;
            };
            p.fill_color(color(road[0], road[1], road[2], 0.9 * a));
            p.ctx.DrawGeometry(&geo, &res.brush, r.h * 0.055 * wr, None);
        }
        // Pin with white ring and soft shadow.
        let (px, py) = (r.x + mm.pin.0 * r.w, r.y + mm.pin.1 * r.h);
        let pr = r.h * 0.1;
        p.circle(px, py + pr * 0.35, pr * 1.45, color(0.0, 0.0, 0.0, 0.3 * a));
        p.circle(px, py, pr * 1.18, color(1.0, 1.0, 1.0, a));
        p.circle(px, py, pr, color(1.0, 0.27, 0.23, a));
        p.circle(px, py, pr * 0.36, color(1.0, 1.0, 1.0, a));
        // Small source tag in the corner.
        let tag_size = r.h * 0.2;
        p.text(
            maps_label,
            &res.chat.small,
            Rect::new(
                r.x + tag_size * 0.6,
                r.y + tag_size * 0.35,
                r.w * 0.6,
                tag_size * 1.4,
            ),
            color(1.0, 1.0, 1.0, 0.75 * a),
        );

        p.ctx.PopLayer();
        drop(ManuallyDrop::into_inner(params.geometricMask));
    }
}
