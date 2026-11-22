//! Drawing of the code view (SPEC §5.4e): an editor panel with a tab, line
//! numbers and syntax colors from the cover palette; the code is drawn in
//! document space under the camera transform.

use std::mem::ManuallyDrop;

use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_ALIASED, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_BITMAP_OPTIONS_TARGET,
    D2D1_LAYER_OPTIONS1_NONE, D2D1_LAYER_PARAMETERS1, D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES,
    ID2D1Brush, ID2D1Layer,
};
use windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_RANGE;
use windows::core::IUnknown;

use super::*;
use crate::ui::code::{CameraView, CodeDoc, DocLine, GUTTER, Tok};

/// Font size of the code in document space (the camera scales it).
const FONT: f32 = 32.0;
const LINE_H: f32 = FONT * 1.5;
const KINDS: usize = 6;
/// The 3D pass draws a larger plane than the code area (width, height),
/// so the projection never shows its edge.
const EXTENT: (f32, f32) = (1.5, 1.3);
/// Camera distance of the 3D view relative to the code area width.
const EYE_DISTANCE: f32 = 1.25;

fn kind_index(k: Tok) -> usize {
    match k {
        Tok::Keyword => 0,
        Tok::Function => 1,
        Tok::Str => 2,
        Tok::Punct => 3,
        Tok::Comment => 4,
        Tok::Plain => 5,
    }
}

pub(super) struct CodeRes {
    mono: IDWriteTextFormat,
    tab: IDWriteTextFormat,
    tab_size: f32,
    char_w: f32,
    /// One brush per token kind; cached layouts reference them, colors and
    /// opacity are set every frame.
    kinds: Vec<ID2D1SolidColorBrush>,
    /// Not yet sung words of a chorus repetition.
    dim: ID2D1SolidColorBrush,
    /// Full line layouts of the current document.
    lines: RefCell<(u64, HashMap<usize, IDWriteTextLayout>)>,
    numbers: RefCell<HashMap<usize, IDWriteTextLayout>>,
    /// Offscreen target of the 3D view and its pixel size.
    offscreen: RefCell<Option<(ID2D1Bitmap1, u32, u32)>>,
}

impl CodeRes {
    pub(super) fn new(
        dwrite: &IDWriteFactory,
        ctx: &ID2D1DeviceContext,
        ui_font: &str,
        h: f32,
    ) -> Result<Self> {
        let mono_font = pick_family(dwrite, &["Cascadia Mono", "Consolas", "Courier New"]);
        let mono = text_format(
            dwrite,
            &mono_font,
            DWRITE_FONT_WEIGHT_NORMAL,
            FONT,
            DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
            false,
        )?;
        let tab_size = (0.017 * h).clamp(11.0, 30.0);
        let tab = text_format(
            dwrite,
            ui_font,
            DWRITE_FONT_WEIGHT_NORMAL,
            tab_size,
            DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
            false,
        )?;
        // SAFETY: DirectWrite/D2D creation calls with valid arguments.
        unsafe {
            mono.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
            let probe: Vec<u16> = "0000000000".encode_utf16().collect();
            let tl = dwrite.CreateTextLayout(&probe, &mono, 10_000.0, LINE_H)?;
            let mut m = DWRITE_TEXT_METRICS::default();
            tl.GetMetrics(&mut m)?;
            let char_w = (m.widthIncludingTrailingWhitespace / 10.0).max(1.0);
            let white = color(1.0, 1.0, 1.0, 1.0);
            let mut kinds = Vec::with_capacity(KINDS);
            for _ in 0..KINDS {
                kinds.push(ctx.CreateSolidColorBrush(&white, None)?);
            }
            Ok(Self {
                mono,
                tab,
                tab_size,
                char_w,
                kinds,
                dim: ctx.CreateSolidColorBrush(&white, None)?,
                lines: RefCell::new((u64::MAX, HashMap::new())),
                numbers: RefCell::new(HashMap::new()),
                offscreen: RefCell::new(None),
            })
        }
    }
}

/// Lifts a palette color until it is readable on the dark editor.
fn lift(c: [f32; 3]) -> [f32; 3] {
    let pastel = c.map(|x| x + (1.0 - x) * 0.18);
    let luma = 0.2126 * pastel[0] + 0.7152 * pastel[1] + 0.0722 * pastel[2];
    const TARGET: f32 = 0.62;
    if luma >= TARGET {
        return pastel;
    }
    let k = (TARGET - luma) / (1.0 - luma).max(1e-3);
    pastel.map(|x| x + (1.0 - x) * k)
}

/// UTF-16 offset of every character column (plus the end).
fn utf16_columns(text: &str) -> Vec<u32> {
    let mut out = Vec::with_capacity(text.len() + 1);
    let mut pos = 0u32;
    for ch in text.chars() {
        out.push(pos);
        pos += ch.len_utf16() as u32;
    }
    out.push(pos);
    out
}

/// Layout of the first `n` characters of a line with token colors.
/// `dim_from` (column) draws the rest of the line with the dim brush.
fn line_layout(
    p: &Painter,
    line: &DocLine,
    n: usize,
    dim_from: Option<usize>,
) -> Option<IDWriteTextLayout> {
    let res = &p.res.code;
    let text: String = line.text.chars().take(n).collect();
    let cols = utf16_columns(&text);
    let wide: Vec<u16> = text.encode_utf16().collect();
    // SAFETY: DirectWrite calls with a valid format and ranges in the text.
    unsafe {
        let tl = p
            .res
            .dwrite
            .CreateTextLayout(&wide, &res.mono, 100_000.0, LINE_H)
            .ok()?;
        for s in &line.segs {
            let a = s.col.min(n);
            let b = (s.col + s.len).min(n);
            if b <= a {
                continue;
            }
            let brush = match dim_from {
                Some(d) if a >= d => &res.dim,
                _ => &res.kinds[kind_index(s.kind)],
            };
            if let Ok(u) = brush.cast::<IUnknown>() {
                let _ = tl.SetDrawingEffect(
                    &u,
                    DWRITE_TEXT_RANGE {
                        startPosition: cols[a],
                        length: cols[b] - cols[a],
                    },
                );
            }
        }
        Some(tl)
    }
}

fn draw_layout(p: &Painter, tl: &IDWriteTextLayout, x: f32, y: f32) {
    // SAFETY: called between BeginDraw/EndDraw.
    unsafe {
        p.ctx.DrawTextLayout(
            Vector2 { X: x, Y: y },
            tl,
            &p.res.brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
        );
    }
}

fn rotate_about(angle: f32, cx: f32, cy: f32) -> Matrix3x2 {
    let (sin, cos) = angle.sin_cos();
    Matrix3x2 {
        M11: cos,
        M12: sin,
        M21: -sin,
        M22: cos,
        M31: cx - cx * cos + cy * sin,
        M32: cy - cx * sin - cy * cos,
    }
}

pub(super) fn draw(p: &Painter, m: &UiModel, v: &CodeView, base: &Matrix3x2, alpha: f32) {
    if alpha <= 0.0 {
        return;
    }
    let res = p.res;
    let cr = &res.code;
    let (w, h) = res.size;
    let doc = v.doc;
    let [ar, ag, ab] = lift(m.accent);

    // ----- editor panel and tab -----
    p.set_transform(base);
    let panel = Rect::new(0.035 * w, 0.115 * h, 0.93 * w, 0.835 * h);
    let radius = 0.012 * h;
    p.rounded(panel, radius, color(0.055, 0.058, 0.07, 0.93 * alpha));
    let tab_h = cr.tab_size * 2.6;
    let bar = Rect::new(panel.x, panel.y, panel.w, tab_h);
    p.rounded(bar, radius, color(0.035, 0.037, 0.046, 0.95 * alpha));
    p.rounded(
        Rect::new(bar.x, bar.y + tab_h * 0.5, bar.w, tab_h * 0.5),
        0.0,
        color(0.035, 0.037, 0.046, 0.95 * alpha),
    );
    let tab_w = cr.tab_size * (2.6 + 0.56 * doc.file_name.chars().count() as f32);
    let tab = Rect::new(panel.x + radius, panel.y, tab_w, tab_h);
    p.rounded(tab, 0.0, color(0.055, 0.058, 0.07, alpha));
    p.rounded(
        Rect::new(tab.x, tab.y, tab.w, 2.0),
        0.0,
        color(ar, ag, ab, 0.9 * alpha),
    );
    p.text(
        &doc.file_name,
        &cr.tab,
        tab,
        color(0.88, 0.9, 0.94, 0.9 * alpha),
    );

    // ----- code under the camera -----
    let area = Rect::new(panel.x, panel.y + tab_h, panel.w, panel.h - tab_h);
    // 3D view: the code is drawn flat into an offscreen bitmap first (no
    // clip may be pushed while the target changes), then projected.
    let side = v.cam.yaw.abs() + v.cam.pitch.abs() > 0.003;
    let flat = if side {
        render_offscreen(p, m, v, area, alpha)
    } else {
        None
    };
    // The offscreen pass leaves its camera transform behind; the clip is
    // given in panel space.
    p.set_transform(base);
    // SAFETY: clip push/pop pair between BeginDraw/EndDraw.
    unsafe {
        p.ctx
            .PushAxisAlignedClip(&d2d_rect(area), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
    }
    match flat {
        Some(bmp) => {
            p.set_transform(base);
            // The bitmap is drawn at its own origin; the perspective turns
            // it about its center and the world transform moves that center
            // onto the code area's center (D2D applies the perspective in
            // the destination rectangle's local space).
            let (acx, acy) = area.center();
            let dest = Rect::centered(acx, acy, area.w * EXTENT.0, area.h * EXTENT.1);
            let proj = Projection::new(area, v.cam.yaw, v.cam.pitch);
            // Nothing is cut hard: the plane fades out softly towards the
            // edges of the code area and on its far side.
            let k = ((v.cam.yaw.abs() + v.cam.pitch.abs()) / (40f32).to_radians()).min(1.0);
            let masks = soft_masks(p, area, v.cam.yaw, k);
            // SAFETY: layer push/pop pairs and drawing between
            // BeginDraw/EndDraw with a valid bitmap.
            unsafe {
                for brush in &masks {
                    let params = D2D1_LAYER_PARAMETERS1 {
                        contentBounds: d2d_rect(area),
                        geometricMask: ManuallyDrop::new(None),
                        maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                        maskTransform: scale_about(1.0, 0.0, 0.0),
                        opacity: 1.0,
                        opacityBrush: ManuallyDrop::new(Some(brush.clone())),
                        layerOptions: D2D1_LAYER_OPTIONS1_NONE,
                    };
                    p.ctx.PushLayer(&params, None::<&ID2D1Layer>);
                    // The parameters hold their own reference.
                    let _ = ManuallyDrop::into_inner(params.opacityBrush);
                }
                draw_projected(p, &bmp, dest, &proj, base);
                p.set_transform(base);
                for _ in &masks {
                    p.ctx.PopLayer();
                }
            }
        }
        None if !side => draw_code(p, m, v, base, area, area.h, alpha),
        None => {}
    }
    p.set_transform(base);
    // SAFETY: matches the push above.
    unsafe { p.ctx.PopAxisAlignedClip() };
}

/// Draws the code flat into the (cached) offscreen bitmap.
fn render_offscreen(
    p: &Painter,
    m: &UiModel,
    v: &CodeView,
    area: Rect,
    alpha: f32,
) -> Option<ID2D1Bitmap1> {
    let cr = &p.res.code;
    // Same DPI as the current target: no DPI switch while drawing.
    let mut dpi = (96.0f32, 96.0f32);
    // SAFETY: out-pointers to locals.
    unsafe { p.ctx.GetDpi(&mut dpi.0, &mut dpi.1) };
    let scale = dpi.0 / 96.0;
    let (lw, lh) = (area.w * EXTENT.0, area.h * EXTENT.1);
    let (pw, ph) = (
        (lw * scale).ceil().max(1.0) as u32,
        (lh * scale).ceil().max(1.0) as u32,
    );
    let mut slot = cr.offscreen.borrow_mut();
    if slot.as_ref().is_none_or(|(_, w, h)| (*w, *h) != (pw, ph)) {
        let props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: dpi.0,
            dpiY: dpi.1,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
            ..Default::default()
        };
        // SAFETY: bitmap creation without initial data.
        let bmp = unsafe {
            p.ctx.CreateBitmap(
                D2D_SIZE_U {
                    width: pw,
                    height: ph,
                },
                None,
                0,
                &props,
            )
        }
        .ok()?;
        *slot = Some((bmp, pw, ph));
    }
    let bmp = slot.as_ref()?.0.clone();
    drop(slot);
    // SAFETY: target switch between BeginDraw/EndDraw with no clip or layer
    // pushed; the previous target is restored right after.
    unsafe {
        let prev = p.ctx.GetTarget().ok()?;
        p.ctx.SetTarget(&bmp);
        p.ctx.Clear(Some(&color(0.0, 0.0, 0.0, 0.0)));
        let local = Rect::new(0.0, 0.0, lw, lh);

        draw_code(p, m, v, &scale_about(1.0, 0.0, 0.0), local, area.h, alpha);
        p.ctx.SetTarget(&prev);
        p.set_transform(&scale_about(1.0, 0.0, 0.0));
    }
    Some(bmp)
}

fn gradient_stops(
    p: &Painter,
    stops: &[(f32, f32)],
) -> Option<windows::Win32::Graphics::Direct2D::ID2D1GradientStopCollection> {
    let stops: Vec<D2D1_GRADIENT_STOP> = stops
        .iter()
        .map(|&(position, a)| D2D1_GRADIENT_STOP {
            position,
            color: color(0.0, 0.0, 0.0, a),
        })
        .collect();
    let rt: &ID2D1RenderTarget = p.ctx;
    // SAFETY: resource creation on a valid context.
    unsafe {
        rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)
            .ok()
    }
}

/// Opacity masks of the 3D view (`k` = 0..1 strength by angle): a soft
/// edge that reaches zero at the code area's borders, and a fade of the
/// far side (right for positive yaw).
fn soft_masks(p: &Painter, area: Rect, yaw: f32, k: f32) -> Vec<ID2D1Brush> {
    let mut out = Vec::new();
    let (cx, cy) = area.center();
    // Edge: elliptical, zero at the middle of each border and beyond.
    let edge = 0.89;
    if let Some(coll) = gradient_stops(p, &[(0.0, 1.0), (edge - 0.3 * k, 1.0), (edge, 0.0)]) {
        let props = D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
            center: Vector2 { X: cx, Y: cy },
            gradientOriginOffset: Vector2 { X: 0.0, Y: 0.0 },
            radiusX: area.w * 0.5 / edge,
            radiusY: area.h * 0.5 / edge,
        };
        // SAFETY: brush creation on a valid context.
        if let Ok(b) = unsafe { p.ctx.CreateRadialGradientBrush(&props, None, &coll) }
            && let Ok(b) = b.cast::<ID2D1Brush>()
        {
            out.push(b);
        }
    }
    // Depth: the far half dims towards its edge.
    let far = if yaw >= 0.0 { area.x + area.w } else { area.x };
    if let Some(coll) = gradient_stops(p, &[(0.0, 1.0 - 0.75 * k), (1.0, 1.0)]) {
        let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
            startPoint: Vector2 { X: far, Y: cy },
            endPoint: Vector2 { X: cx, Y: cy },
        };
        // SAFETY: brush creation on a valid context.
        if let Ok(b) = unsafe { p.ctx.CreateLinearGradientBrush(&props, None, &coll) }
            && let Ok(b) = b.cast::<ID2D1Brush>()
        {
            out.push(b);
        }
    }
    out
}

/// Perspective of the code plane turned by yaw/pitch about the center of
/// the code area, seen from `EYE_DISTANCE` x its width (all in DIPs).
struct Projection {
    cx: f32,
    cy: f32,
    d: f32,
    yaw: (f32, f32),
    pitch: (f32, f32),
}

impl Projection {
    fn new(area: Rect, yaw: f32, pitch: f32) -> Self {
        let (cx, cy) = area.center();
        Self {
            cx,
            cy,
            d: area.w * EYE_DISTANCE,
            yaw: yaw.sin_cos(),
            pitch: pitch.sin_cos(),
        }
    }

    /// Screen point of the plane point (x, y).
    fn at(&self, x: f32, y: f32) -> (f32, f32) {
        let (sy, cyw) = self.yaw;
        let (sp, cp) = self.pitch;
        let (x0, y0) = (x - self.cx, y - self.cy);
        // Yaw about the vertical axis, then pitch about the horizontal one.
        let (x1, z1) = (x0 * cyw, -x0 * sy);
        let (y2, z2) = (y0 * cp - z1 * sp, y0 * sp + z1 * cp);
        let w = (1.0 - z2 / self.d).max(0.2);
        (self.cx + x1 / w, self.cy + y2 / w)
    }
}

/// Number of vertical strips the projected plane is drawn in; each strip
/// is two triangles with exact affine mappings.
const STRIPS: usize = 48;
/// Rows of the grid; more rows keep horizontal edges straight (the affine
/// error inside a triangle grows with the square of its height).
const ROWS: usize = 6;

/// Affine transform mapping three local points onto three screen points.
fn affine(l: [(f32, f32); 3], s: [(f32, f32); 3]) -> Option<Matrix3x2> {
    let (dx1, dy1) = (l[1].0 - l[0].0, l[1].1 - l[0].1);
    let (dx2, dy2) = (l[2].0 - l[0].0, l[2].1 - l[0].1);
    let det = dx1 * dy2 - dx2 * dy1;
    if det.abs() < 1e-6 {
        return None;
    }
    let (sx1, sy1) = (s[1].0 - s[0].0, s[1].1 - s[0].1);
    let (sx2, sy2) = (s[2].0 - s[0].0, s[2].1 - s[0].1);
    // Linear part M solves [d1; d2] * M = [s1; s2] (row vectors).
    let m11 = (sx1 * dy2 - sx2 * dy1) / det;
    let m21 = (sx2 * dx1 - sx1 * dx2) / det;
    let m12 = (sy1 * dy2 - sy2 * dy1) / det;
    let m22 = (sy2 * dx1 - sy1 * dx2) / det;
    Some(Matrix3x2 {
        M11: m11,
        M12: m12,
        M21: m21,
        M22: m22,
        M31: s[0].0 - l[0].0 * m11 - l[0].1 * m21,
        M32: s[0].1 - l[0].0 * m12 - l[0].1 * m22,
    })
}

/// Draws `bmp` (covering `dest`) projected: the plane is cut into
/// triangles, each filled with the bitmap through the affine transform of
/// its projected corners. Shared edges match exactly, so there are no
/// seams or steps; clips and DPI behave as for any other drawing.
fn draw_projected(
    p: &Painter,
    bmp: &ID2D1Bitmap1,
    dest: Rect,
    proj: &Projection,
    base: &Matrix3x2,
) {
    let props = D2D1_BITMAP_BRUSH_PROPERTIES1 {
        extendModeX: D2D1_EXTEND_MODE_CLAMP,
        extendModeY: D2D1_EXTEND_MODE_CLAMP,
        interpolationMode: D2D1_INTERPOLATION_MODE_LINEAR,
    };
    // SAFETY: brush creation on a valid context.
    let Ok(brush) = (unsafe { p.ctx.CreateBitmapBrush(bmp, Some(&props), None) }) else {
        return;
    };
    p.set_transform(base);
    let (w, h) = (dest.w, dest.h);
    let at = |u: f32, v: f32| proj.at(dest.x + u, dest.y + v);
    // Aliased: neighbouring triangles share their edges exactly, so no
    // pixel is blended twice (the outer edges are faded out anyway).
    // SAFETY: plain state setter, restored below.
    unsafe { p.ctx.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED) };
    // Projected grid points, row by row.
    let grid: Vec<Vec<(f32, f32)>> = (0..=ROWS)
        .map(|r| {
            let v = h * r as f32 / ROWS as f32;
            (0..=STRIPS)
                .map(|c| at(w * c as f32 / STRIPS as f32, v))
                .collect()
        })
        .collect();
    for r in 0..ROWS {
        let v0 = h * r as f32 / ROWS as f32;
        let v1 = h * (r + 1) as f32 / ROWS as f32;
        for c in 0..STRIPS {
            let u0 = w * c as f32 / STRIPS as f32;
            let u1 = w * (c + 1) as f32 / STRIPS as f32;
            let (s00, s10) = (grid[r][c], grid[r][c + 1]);
            let (s01, s11) = (grid[r + 1][c], grid[r + 1][c + 1]);
            let tris = [
                ([(u0, v0), (u1, v0), (u0, v1)], [s00, s10, s01]),
                ([(u1, v0), (u1, v1), (u0, v1)], [s10, s11, s01]),
            ];
            for (l, s) in tris {
                let Some(m) = affine(l, s) else { continue };
                // SAFETY: geometry creation and filling between
                // BeginDraw/EndDraw.
                unsafe {
                    let Ok(geo) = p.res.factory.CreatePathGeometry() else {
                        continue;
                    };
                    let Ok(sink) = geo.Open() else { continue };
                    let pt = |q: (f32, f32)| Vector2 { X: q.0, Y: q.1 };
                    sink.BeginFigure(pt(s[0]), D2D1_FIGURE_BEGIN_FILLED);
                    sink.AddLine(pt(s[1]));
                    sink.AddLine(pt(s[2]));
                    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                    if sink.Close().is_err() {
                        continue;
                    }
                    brush.SetTransform(&m);
                    p.ctx.FillGeometry(&geo, &brush, None);
                }
            }
        }
    }
    // SAFETY: plain state setter.
    unsafe { p.ctx.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE) };
}

/// `fit_h`: height (DIPs) that shows `cam.lines` lines; `area` may be
/// larger (3D pass) and only widens what is drawn.
fn draw_code(
    p: &Painter,
    m: &UiModel,
    v: &CodeView,
    base: &Matrix3x2,
    area: Rect,
    fit_h: f32,
    alpha: f32,
) {
    let res = p.res;
    let cr = &res.code;
    let doc: &CodeDoc = v.doc;
    let cam: CameraView = v.cam;
    let t = v.t;
    let cw = cr.char_w;

    // Syntax colors from the palette (SPEC §5.4e).
    let pal = m.palette.map(lift);
    let fixed = [[0.7, 0.72, 0.78], [0.46, 0.49, 0.56], [0.87, 0.89, 0.93]];
    let colors = [pal[0], pal[1], pal[2], fixed[0], fixed[1], fixed[2]];
    for (b, c) in cr.kinds.iter().zip(colors) {
        // SAFETY: plain brush setter.
        unsafe { b.SetColor(&color(c[0], c[1], c[2], alpha)) };
    }
    // SAFETY: plain brush setter.
    unsafe {
        cr.dim
            .SetColor(&color(pal[2][0], pal[2][1], pal[2][2], 0.38 * alpha))
    };

    // Camera: document point (cam.x, cam.y) at the center of the area.
    let s = fit_h / (cam.lines.max(1.0) * LINE_H);
    let (cx, cy) = area.center();
    let wx = (GUTTER + cam.x) * cw;
    let wy = cam.y * LINE_H + LINE_H * 0.5;
    let to_screen = Matrix3x2 {
        M11: s,
        M12: 0.0,
        M21: 0.0,
        M22: s,
        M31: cx - wx * s,
        M32: cy - wy * s,
    };
    let m_cam = mat_mul(&mat_mul(&to_screen, &rotate_about(cam.tilt, cx, cy)), base);
    p.set_transform(&m_cam);

    let existing = doc.visible_lines(t);
    let half = area.h / (s * LINE_H) * 0.5 + 1.5;
    let first = (cam.y - half).floor().max(0.0) as usize;
    let last = ((cam.y + half).ceil().max(0.0) as usize).min(existing);
    let focus = doc.focus(t);
    let (cur_line, cur_col) = doc.cursor(t);
    let highlight = focus.replay.map_or(cur_line, |(l, _)| l);
    let row_w = ((cam.lines * 4.0 + cam.x + GUTTER) * cw).max(area.w / s);

    {
        let mut cache = cr.lines.borrow_mut();
        if cache.0 != doc.id {
            *cache = (doc.id, HashMap::new());
        }
    }
    for (i, line) in doc.lines.iter().enumerate().take(last).skip(first) {
        let y = i as f32 * LINE_H;
        // Current line background.
        if i == highlight {
            let c = if focus.replay.is_some() {
                color(pal[2][0], pal[2][1], pal[2][2], 0.1 * alpha)
            } else {
                color(1.0, 1.0, 1.0, 0.045 * alpha)
            };
            p.rounded(Rect::new(-cw, y, row_w, LINE_H), 0.0, c);
        }
        // Line number.
        let number = {
            let mut nums = cr.numbers.borrow_mut();
            if let Some(tl) = nums.get(&i) {
                Some(tl.clone())
            } else {
                let label: Vec<u16> = format!("{:>4}", i + 1).encode_utf16().collect();
                // SAFETY: DirectWrite call with a valid format.
                let tl = unsafe {
                    res.dwrite
                        .CreateTextLayout(&label, &cr.mono, 100_000.0, LINE_H)
                }
                .ok();
                if let Some(tl) = &tl {
                    nums.insert(i, tl.clone());
                }
                tl
            }
        };
        if let Some(tl) = number {
            let a = if i == highlight { 0.8 } else { 0.32 };
            p.fill_color(color(0.85, 0.87, 0.92, a * alpha));
            draw_layout(p, &tl, 0.0, y);
        }
        // Code.
        let x0 = GUTTER * cw;
        let n = line.revealed(t);
        if n == 0 {
            continue;
        }
        let full = n >= line.text.chars().count();
        let replay = focus.replay.filter(|(l, _)| *l == i);
        let layout = match replay {
            Some((_, sung)) => {
                // Sung words bright, the rest dim.
                let k = sung.floor() as usize;
                let dim_from = line.words.get(k).map(|(c, _)| *c);
                line_layout(p, line, n, dim_from)
            }
            None if full => {
                let mut cache = cr.lines.borrow_mut();
                match cache.1.get(&i) {
                    Some(tl) => Some(tl.clone()),
                    None => {
                        let tl = line_layout(p, line, n, None);
                        if let Some(tl) = &tl {
                            cache.1.insert(i, tl.clone());
                        }
                        tl
                    }
                }
            }
            None => line_layout(p, line, n, None),
        };
        if let Some(tl) = layout {
            draw_layout(p, &tl, x0, y);
        }
    }

    // Blinking bar cursor, solid while typing.
    let typing = doc.lines.get(cur_line).is_some_and(|l| l.typing(t));
    let on = typing || (m.time * 0.95).fract() < 0.55;
    if on && cur_line < existing {
        let x = (GUTTER + cur_col as f32) * cw;
        let y = cur_line as f32 * LINE_H;
        p.rounded(
            Rect::new(x, y + LINE_H * 0.14, (cw * 0.12).max(2.0), LINE_H * 0.72),
            0.0,
            color(0.95, 0.96, 1.0, 0.95 * alpha),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn affine_maps_all_three_corners() {
        let l = [(10.0, 0.0), (30.0, 0.0), (10.0, 50.0)];
        let s = [(100.0, 20.0), (140.0, 25.0), (95.0, 130.0)];
        let m = affine(l, s).unwrap();
        for (a, b) in l.iter().zip(s) {
            let x = a.0 * m.M11 + a.1 * m.M21 + m.M31;
            let y = a.0 * m.M12 + a.1 * m.M22 + m.M32;
            assert!((x - b.0).abs() < 1e-3 && (y - b.1).abs() < 1e-3);
        }
        assert!(affine([(0.0, 0.0), (1.0, 1.0), (2.0, 2.0)], s).is_none());
    }

    #[test]
    fn frontal_projection_is_identity() {
        let p = Projection::new(Rect::new(0.0, 0.0, 100.0, 50.0), 0.0, 0.0);
        let (x, y) = p.at(12.0, 34.0);
        assert!((x - 12.0).abs() < 1e-4 && (y - 34.0).abs() < 1e-4);
    }
}
