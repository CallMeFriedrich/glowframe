//! Direct2D/DirectWrite drawing of the fullscreen UI (SPEC §5.2–§5.4, §5.6):
//! player view, cover view, standby screen, pause state and the Esc hint.

use std::cell::RefCell;
use std::sync::Arc;

use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F,
    D2D1_COMPOSITE_MODE_SOURCE_OVER, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    CLSID_D2D1GaussianBlur, CLSID_D2D1Opacity, CLSID_D2D1Scale, D2D1_BITMAP_BRUSH_PROPERTIES1,
    D2D1_BITMAP_OPTIONS_NONE, D2D1_BITMAP_PROPERTIES1, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ELLIPSE,
    D2D1_EXTEND_MODE_CLAMP, D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION,
    D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, D2D1_INTERPOLATION_MODE_LINEAR,
    D2D1_OPACITY_PROP_OPACITY, D2D1_PROPERTY_TYPE_FLOAT, D2D1_PROPERTY_TYPE_VECTOR2,
    D2D1_ROUNDED_RECT, D2D1_SCALE_PROP_SCALE, ID2D1Bitmap1, ID2D1DeviceContext, ID2D1Effect,
    ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_MEASURING_MODE_NATURAL,
    DWRITE_PARAGRAPH_ALIGNMENT, DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_PARAGRAPH_ALIGNMENT_FAR,
    DWRITE_PARAGRAPH_ALIGNMENT_NEAR, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TRIMMING,
    DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_WORD_WRAPPING_NO_WRAP, DWRITE_WORD_WRAPPING_WRAP,
    IDWriteFactory, IDWriteFontCollection, IDWriteTextFormat,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::core::{BOOL, HSTRING, Result};
use windows_numerics::{Matrix3x2, Vector2};

use crate::cover::Image;
use crate::ui::layout::{self, Button, PlayerLayout, Rect};

const GLYPH_PLAY: char = '\u{E768}';
const GLYPH_PAUSE: char = '\u{E769}';
const GLYPH_PREVIOUS: char = '\u{E892}';
const GLYPH_NEXT: char = '\u{E893}';
const GLYPH_SHUFFLE: char = '\u{E8B1}';
const GLYPH_REPEAT_ALL: char = '\u{E8EE}';
const GLYPH_REPEAT_ONE: char = '\u{E8ED}';
const MAX_CACHED_BITMAPS: usize = 6;

pub struct TrackView<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    pub image: Option<&'a Arc<Image>>,
    pub small: Option<&'a Arc<Image>>,
    /// Opacity of this track layer (cover crossfade on track change).
    pub alpha: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct ButtonView {
    pub button: Button,
    pub enabled: bool,
    /// Shuffle on / repeat not off.
    pub active: bool,
    pub repeat_one: bool,
    pub playing: bool,
    pub hover: bool,
    /// Click feedback 0..1.
    pub press: f32,
}

pub struct UiModel<'a> {
    /// 0 = player view, 1 = cover view (eased).
    pub view: f32,
    /// 0..1 blend towards the standby screen.
    pub standby: f32,
    /// 0..1 blend of the pause state.
    pub pause: f32,
    /// Track layers, oldest first.
    pub tracks: Vec<TrackView<'a>>,
    pub buttons: [ButtonView; 5],
    /// Accent color (sRGB 0..1) from the palette.
    pub accent: [f32; 3],
    pub standby_title: &'a str,
    pub standby_hint: &'a str,
    pub esc_hint: Option<(&'a str, f32)>,
}

/// Per-surface D2D/DWrite resources.
pub struct UiResources {
    layout: PlayerLayout,
    size: (f32, f32),
    /// Effects work in pixels for 96-DPI bitmap inputs, positions in DIPs.
    dpi_scale: f32,
    title: IDWriteTextFormat,
    artist: IDWriteTextFormat,
    icon: IDWriteTextFormat,
    icon_big: IDWriteTextFormat,
    standby: IDWriteTextFormat,
    standby_small: IDWriteTextFormat,
    hint: IDWriteTextFormat,
    brush: ID2D1SolidColorBrush,
    scale: ID2D1Effect,
    blur: ID2D1Effect,
    opacity: ID2D1Effect,
    bitmaps: RefCell<Vec<(u64, ID2D1Bitmap1)>>,
}

impl UiResources {
    /// `w`/`h` in DIPs; `dpi_scale` = DPI / 96.
    pub fn new(
        dwrite: &IDWriteFactory,
        ctx: &ID2D1DeviceContext,
        w: f32,
        h: f32,
        dpi_scale: f32,
    ) -> Result<Self> {
        let layout = layout::player(w, h);
        let (sb_big, sb_small) = layout::standby_sizes(h);
        let ui_font = pick_family(dwrite, &["Segoe UI Variable Display", "Segoe UI"]);
        let icon_font = pick_family(dwrite, &["Segoe Fluent Icons", "Segoe MDL2 Assets"]);
        let fmt = |family: &str, weight, size, para, wrap| {
            text_format(dwrite, family, weight, size, para, wrap)
        };
        // SAFETY: plain D2D resource creation on a valid device context.
        unsafe {
            let scale = ctx.CreateEffect(&CLSID_D2D1Scale)?;
            let blur = ctx.CreateEffect(&CLSID_D2D1GaussianBlur)?;
            let opacity = ctx.CreateEffect(&CLSID_D2D1Opacity)?;
            blur.SetInput(0, &scale.GetOutput()?, false);
            opacity.SetInput(0, &blur.GetOutput()?, false);
            let white = color(1.0, 1.0, 1.0, 1.0);
            Ok(Self {
                layout,
                size: (w, h),
                dpi_scale,
                title: fmt(
                    &ui_font,
                    DWRITE_FONT_WEIGHT_SEMI_BOLD,
                    layout.title_size,
                    DWRITE_PARAGRAPH_ALIGNMENT_FAR,
                    true,
                )?,
                artist: fmt(
                    &ui_font,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    layout.artist_size,
                    DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
                    false,
                )?,
                icon: fmt(
                    &icon_font,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    layout.icon_size,
                    DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                    false,
                )?,
                icon_big: fmt(
                    &icon_font,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    layout.icon_size_big,
                    DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                    false,
                )?,
                standby: fmt(
                    &ui_font,
                    DWRITE_FONT_WEIGHT_SEMI_BOLD,
                    sb_big,
                    DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                    false,
                )?,
                standby_small: fmt(
                    &ui_font,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    sb_small,
                    DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                    false,
                )?,
                hint: fmt(
                    &ui_font,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    15.0,
                    DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                    false,
                )?,
                brush: ctx.CreateSolidColorBrush(&white, None)?,
                scale,
                blur,
                opacity,
                bitmaps: RefCell::new(Vec::new()),
            })
        }
    }

    pub fn layout(&self) -> &PlayerLayout {
        &self.layout
    }

    /// Uploads an image once and caches the bitmap (by image id).
    fn bitmap(&self, ctx: &ID2D1DeviceContext, img: &Image) -> Option<ID2D1Bitmap1> {
        let mut cache = self.bitmaps.borrow_mut();
        if let Some(i) = cache.iter().position(|(id, _)| *id == img.id) {
            let entry = cache.remove(i);
            let bmp = entry.1.clone();
            cache.push(entry); // most recently used last
            return Some(bmp);
        }
        let props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
            ..Default::default()
        };
        // SAFETY: the pixel buffer matches width * height * 4 with that pitch.
        let bmp = unsafe {
            ctx.CreateBitmap(
                D2D_SIZE_U {
                    width: img.width,
                    height: img.height,
                },
                Some(img.bgra.as_ptr().cast()),
                img.width * 4,
                &props,
            )
        };
        match bmp {
            Ok(b) => {
                if cache.len() >= MAX_CACHED_BITMAPS {
                    cache.remove(0);
                }
                cache.push((img.id, b.clone()));
                Some(b)
            }
            Err(e) => {
                tracing::warn!(error = %e, "bitmap upload failed");
                None
            }
        }
    }
}

fn pick_family(dwrite: &IDWriteFactory, candidates: &[&str]) -> String {
    let mut collection: Option<IDWriteFontCollection> = None;
    // SAFETY: out-pointer to a local Option.
    if unsafe { dwrite.GetSystemFontCollection(&mut collection, false) }.is_ok()
        && let Some(c) = collection
    {
        for name in candidates {
            let (mut index, mut exists) = (0u32, BOOL(0));
            // SAFETY: valid collection, out-pointers to locals.
            let ok = unsafe { c.FindFamilyName(&HSTRING::from(*name), &mut index, &mut exists) };
            if ok.is_ok() && exists.as_bool() {
                return (*name).to_string();
            }
        }
    }
    candidates.last().copied().unwrap_or("Segoe UI").to_string()
}

fn text_format(
    dwrite: &IDWriteFactory,
    family: &str,
    weight: DWRITE_FONT_WEIGHT,
    size: f32,
    para: DWRITE_PARAGRAPH_ALIGNMENT,
    wrap: bool,
) -> Result<IDWriteTextFormat> {
    // SAFETY: DirectWrite factory calls with valid strings.
    unsafe {
        let f = dwrite.CreateTextFormat(
            &HSTRING::from(family),
            None,
            weight,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            &HSTRING::from(""),
        )?;
        f.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
        f.SetParagraphAlignment(para)?;
        f.SetWordWrapping(if wrap {
            DWRITE_WORD_WRAPPING_WRAP
        } else {
            DWRITE_WORD_WRAPPING_NO_WRAP
        })?;
        let sign = dwrite.CreateEllipsisTrimmingSign(&f)?;
        let trimming = DWRITE_TRIMMING {
            granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
            delimiter: 0,
            delimiterCount: 0,
        };
        f.SetTrimming(&trimming, &sign)?;
        Ok(f)
    }
}

fn color(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}

fn d2d_rect(r: Rect) -> D2D_RECT_F {
    D2D_RECT_F {
        left: r.x,
        top: r.y,
        right: r.x + r.w,
        bottom: r.y + r.h,
    }
}

fn scale_about(s: f32, cx: f32, cy: f32) -> Matrix3x2 {
    Matrix3x2 {
        M11: s,
        M12: 0.0,
        M21: 0.0,
        M22: s,
        M31: cx * (1.0 - s),
        M32: cy * (1.0 - s),
    }
}

struct Painter<'a> {
    ctx: &'a ID2D1DeviceContext,
    res: &'a UiResources,
}

impl Painter<'_> {
    fn fill_color(&self, c: D2D1_COLOR_F) {
        // SAFETY: valid brush.
        unsafe { self.res.brush.SetColor(&c) };
    }

    fn text(&self, s: &str, format: &IDWriteTextFormat, r: Rect, c: D2D1_COLOR_F) {
        if c.a <= 0.0 || s.is_empty() {
            return;
        }
        let wide: Vec<u16> = s.encode_utf16().collect();
        self.fill_color(c);
        // SAFETY: called between BeginDraw/EndDraw.
        unsafe {
            self.ctx.DrawText(
                &wide,
                format,
                &d2d_rect(r),
                &self.res.brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }

    fn rounded(&self, r: Rect, radius: f32, c: D2D1_COLOR_F) {
        if c.a <= 0.0 {
            return;
        }
        self.fill_color(c);
        // SAFETY: called between BeginDraw/EndDraw.
        unsafe {
            self.ctx.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: d2d_rect(r),
                    radiusX: radius,
                    radiusY: radius,
                },
                &self.res.brush,
            );
        }
    }

    fn circle(&self, cx: f32, cy: f32, rad: f32, c: D2D1_COLOR_F) {
        if c.a <= 0.0 {
            return;
        }
        self.fill_color(c);
        // SAFETY: called between BeginDraw/EndDraw.
        unsafe {
            self.ctx.FillEllipse(
                &D2D1_ELLIPSE {
                    point: Vector2 { X: cx, Y: cy },
                    radiusX: rad,
                    radiusY: rad,
                },
                &self.res.brush,
            );
        }
    }

    /// Blurred, enlarged copy of the small cover as colored ambient light.
    fn ambient(&self, small: &Image, r: Rect, grow: f32, alpha: f32) {
        if alpha <= 0.0 {
            return;
        }
        let Some(bmp) = self.res.bitmap(self.ctx, small) else {
            return;
        };
        let target = r.scaled(grow);
        let sx = target.w * self.res.dpi_scale / small.width as f32;
        let sy = target.h * self.res.dpi_scale / small.height as f32;
        let res = self.res;
        // SAFETY: effect property writes with matching types and sizes.
        unsafe {
            res.scale.SetInput(0, &bmp, false);
            let scale = [sx.to_le_bytes(), sy.to_le_bytes()].concat();
            let _ = res.scale.SetValue(
                D2D1_SCALE_PROP_SCALE.0 as u32,
                D2D1_PROPERTY_TYPE_VECTOR2,
                &scale,
            );
            let _ = res.blur.SetValue(
                D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
                D2D1_PROPERTY_TYPE_FLOAT,
                &(r.w * 0.09 * res.dpi_scale).to_le_bytes(),
            );
            let _ = res.opacity.SetValue(
                D2D1_OPACITY_PROP_OPACITY.0 as u32,
                D2D1_PROPERTY_TYPE_FLOAT,
                &alpha.clamp(0.0, 1.0).to_le_bytes(),
            );
            if let Ok(out) = res.opacity.GetOutput() {
                self.ctx.DrawImage(
                    &out,
                    Some(&Vector2 {
                        X: target.x,
                        Y: target.y,
                    }),
                    None,
                    D2D1_INTERPOLATION_MODE_LINEAR,
                    D2D1_COMPOSITE_MODE_SOURCE_OVER,
                );
            }
        }
    }

    /// Cover with rounded corners; placeholder if no image yet.
    fn cover(&self, img: Option<&Image>, r: Rect, alpha: f32) {
        let radius = r.w * 0.035;
        let Some(img) = img else {
            self.rounded(r, radius, color(1.0, 1.0, 1.0, 0.06 * alpha));
            return;
        };
        let Some(bmp) = self.res.bitmap(self.ctx, img) else {
            return;
        };
        let props = D2D1_BITMAP_BRUSH_PROPERTIES1 {
            extendModeX: D2D1_EXTEND_MODE_CLAMP,
            extendModeY: D2D1_EXTEND_MODE_CLAMP,
            interpolationMode: D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
        };
        // SAFETY: called between BeginDraw/EndDraw; brush is local.
        unsafe {
            let Ok(brush) = self.ctx.CreateBitmapBrush(&bmp, Some(&props), None) else {
                return;
            };
            brush.SetTransform(&Matrix3x2 {
                M11: r.w / img.width as f32,
                M12: 0.0,
                M21: 0.0,
                M22: r.h / img.height as f32,
                M31: r.x,
                M32: r.y,
            });
            brush.SetOpacity(alpha);
            self.ctx.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: d2d_rect(r),
                    radiusX: radius,
                    radiusY: radius,
                },
                &brush,
            );
        }
    }

    fn set_transform(&self, m: &Matrix3x2) {
        // SAFETY: plain state change on the device context.
        unsafe { self.ctx.SetTransform(m) };
    }
}

pub fn draw(ctx: &ID2D1DeviceContext, res: &UiResources, m: &UiModel) {
    let p = Painter { ctx, res };
    let (w, h) = res.size;
    let (cx, cy) = (w / 2.0, h / 2.0);
    let content = 1.0 - m.standby;

    // ----- standby screen -----
    if m.standby > 0.0 {
        let (big, small) = layout::standby_sizes(h);
        let a = m.standby;
        p.text(
            m.standby_title,
            &res.standby,
            Rect::centered(cx, cy - small * 0.6, w, big * 1.6),
            color(1.0, 1.0, 1.0, 0.86 * a),
        );
        p.text(
            m.standby_hint,
            &res.standby_small,
            Rect::centered(cx, cy + big * 0.9, w, small * 1.6),
            color(1.0, 1.0, 1.0, 0.42 * a),
        );
    }

    // ----- player view -----
    let player_a = (1.0 - m.view) * content;
    if player_a > 0.0 {
        p.set_transform(&scale_about(1.0 - 0.04 * m.view, cx, cy));
        let l = res.layout;
        for t in &m.tracks {
            let a = player_a * t.alpha;
            if let Some(s) = t.small {
                p.ambient(s, l.cover, 1.25, 0.55 * a);
            }
            p.cover(t.image.map(|i| i.as_ref()), l.cover, a);
            p.rounded(
                l.cover,
                l.cover.w * 0.035,
                color(0.02, 0.02, 0.03, 0.42 * m.pause * a),
            );
            p.text(t.title, &res.title, l.title, color(1.0, 1.0, 1.0, 0.95 * a));
            p.text(
                t.artist,
                &res.artist,
                l.artist,
                color(1.0, 1.0, 1.0, 0.55 * a),
            );
        }
        // Pause symbol on the cover.
        if m.pause > 0.0 {
            let (ccx, ccy) = l.cover.center();
            let rad = l.cover.w * 0.11;
            p.circle(
                ccx,
                ccy,
                rad,
                color(0.0, 0.0, 0.0, 0.45 * m.pause * player_a),
            );
            p.text(
                &GLYPH_PAUSE.to_string(),
                &res.icon_big,
                Rect::centered(ccx, ccy, rad * 2.0, rad * 2.0),
                color(1.0, 1.0, 1.0, 0.9 * m.pause * player_a),
            );
        }
        draw_buttons(&p, m, player_a);
    }

    // ----- cover view -----
    let cover_a = m.view * content;
    if cover_a > 0.0 {
        p.set_transform(&scale_about(1.04 - 0.04 * m.view, cx, cy));
        let r = layout::cover_view(w, h);
        for t in &m.tracks {
            let a = cover_a * t.alpha;
            if let Some(s) = t.small {
                p.ambient(s, r, 1.35, 0.6 * a);
            }
            p.cover(t.image.map(|i| i.as_ref()), r, a);
            p.rounded(r, r.w * 0.035, color(0.02, 0.02, 0.03, 0.42 * m.pause * a));
        }
    }
    p.set_transform(&scale_about(1.0, 0.0, 0.0));

    // ----- Esc hint -----
    if let Some((text, a)) = m.esc_hint
        && a > 0.0
    {
        let pill_w = 24.0 + 8.2 * text.chars().count() as f32;
        let r = Rect::new((w - pill_w) / 2.0, h - 96.0, pill_w, 38.0);
        p.rounded(r, 19.0, color(0.08, 0.08, 0.1, 0.72 * a));
        p.text(text, &res.hint, r, color(1.0, 1.0, 1.0, 0.82 * a));
    }
}

fn draw_buttons(p: &Painter, m: &UiModel, alpha: f32) {
    let [ar, ag, ab] = m.accent;
    for (bv, (_, rect)) in m.buttons.iter().zip(p.res.layout.buttons.iter()) {
        let r = rect.scaled(1.0 - 0.08 * bv.press);
        let (cx, cy) = r.center();
        let is_play = bv.button == Button::PlayPause;
        let enabled = if bv.enabled { 1.0 } else { 0.35 };
        let base = if is_play { 0.17 } else { 0.07 };
        let hover = if bv.hover && bv.enabled { 0.08 } else { 0.0 };
        let fill = (base + hover + 0.1 * bv.press) * alpha * enabled;
        p.circle(cx, cy, r.w / 2.0, color(1.0, 1.0, 1.0, fill));

        let glyph = match bv.button {
            Button::Shuffle => GLYPH_SHUFFLE,
            Button::Previous => GLYPH_PREVIOUS,
            Button::PlayPause if bv.playing => GLYPH_PAUSE,
            Button::PlayPause => GLYPH_PLAY,
            Button::Next => GLYPH_NEXT,
            Button::Repeat if bv.repeat_one => GLYPH_REPEAT_ONE,
            Button::Repeat => GLYPH_REPEAT_ALL,
        };
        let format = if is_play {
            &p.res.icon_big
        } else {
            &p.res.icon
        };
        let icon_color = if bv.active {
            color(ar, ag, ab, alpha * enabled)
        } else {
            color(1.0, 1.0, 1.0, 0.92 * alpha * enabled)
        };
        p.text(&glyph.to_string(), format, r, icon_color);
        if bv.active {
            p.circle(
                cx,
                r.y + r.h + r.h * 0.12,
                r.w * 0.04,
                color(ar, ag, ab, alpha * enabled),
            );
        }
    }
}
