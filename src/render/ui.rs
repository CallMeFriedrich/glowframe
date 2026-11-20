//! Direct2D/DirectWrite drawing of the fullscreen UI (SPEC §5.2–§5.6):
//! player view, lyrics, cover view, standby screen, pause state, Esc hint.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F,
    D2D1_COMPOSITE_MODE_SOURCE_OVER, D2D1_GRADIENT_STOP, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    CLSID_D2D1GaussianBlur, CLSID_D2D1Opacity, CLSID_D2D1Scale, D2D1_BITMAP_BRUSH_PROPERTIES1,
    D2D1_BITMAP_OPTIONS_NONE, D2D1_BITMAP_PROPERTIES1, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ELLIPSE,
    D2D1_EXTEND_MODE_CLAMP, D2D1_GAMMA_2_2, D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION,
    D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, D2D1_INTERPOLATION_MODE_LINEAR,
    D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES, D2D1_OPACITY_PROP_OPACITY, D2D1_PROPERTY_TYPE_FLOAT,
    D2D1_PROPERTY_TYPE_VECTOR2, D2D1_ROUNDED_RECT, D2D1_SCALE_PROP_SCALE, ID2D1Bitmap1,
    ID2D1BitmapBrush1, ID2D1DeviceContext, ID2D1Effect, ID2D1LinearGradientBrush,
    ID2D1RenderTarget, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT,
    DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD,
    DWRITE_LINE_METRICS, DWRITE_PARAGRAPH_ALIGNMENT, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_PARAGRAPH_ALIGNMENT_FAR, DWRITE_PARAGRAPH_ALIGNMENT_NEAR, DWRITE_TEXT_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_METRICS, DWRITE_TRIMMING,
    DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_WORD_WRAPPING_NO_WRAP, DWRITE_WORD_WRAPPING_WRAP,
    IDWriteFactory, IDWriteFontCollection, IDWriteTextFormat, IDWriteTextLayout,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::core::{BOOL, HSTRING, Interface, Result};
use windows_numerics::{Matrix3x2, Vector2};

use crate::cover::Image;
use crate::lyrics::LyricLine;
use crate::ui::layout::{self, Button, PlayerLayout, Rect};

const GLYPH_PLAY: char = '\u{E768}';
const GLYPH_PAUSE: char = '\u{E769}';
const GLYPH_PREVIOUS: char = '\u{E892}';
const GLYPH_NEXT: char = '\u{E893}';
const GLYPH_SHUFFLE: char = '\u{E8B1}';
const GLYPH_REPEAT_ALL: char = '\u{E8EE}';
const GLYPH_REPEAT_ONE: char = '\u{E8ED}';
const MAX_CACHED_BITMAPS: usize = 6;
/// Text layouts kept per surface (titles, icons, lyric lines in the cover view).
const MAX_CACHED_LAYOUTS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TextKey {
    text: String,
    format: usize,
    w: i32,
    h: i32,
}

pub struct TrackView<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    pub image: Option<&'a Arc<Image>>,
    pub small: Option<&'a Arc<Image>>,
    /// Opacity of this track layer (cover crossfade on track change).
    pub alpha: f32,
    /// Scale of the cover around its center (spring-in of a new cover).
    pub scale: f32,
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

/// Lyrics in the split player view.
pub struct LyricsView<'a> {
    /// Id of the lyrics (text layout cache key).
    pub id: u64,
    pub lines: &'a [LyricLine],
    /// Smoothly animated line index (fractional; -1 = before the first line).
    pub scroll: f32,
    /// Instrumental gap: animated dots instead of a line.
    pub gap: bool,
    /// Overall opacity (layout transition).
    pub alpha: f32,
}

/// Current/next line at the bottom of the cover view.
pub struct CoverLyricsView<'a> {
    pub current: &'a str,
    pub next: &'a str,
    /// Line shown before the last change (fades out).
    pub previous: &'a str,
    /// 0..1 progress of the last line change.
    pub change: f32,
    pub gap: bool,
    pub alpha: f32,
}

pub struct UiModel<'a> {
    /// Seconds since mode start (dot animation).
    pub time: f32,
    /// Player layout of this frame (centered/split transition).
    pub player: PlayerLayout,
    /// Entry animation when the mode starts (spring, 0 -> 1).
    pub intro: f32,
    pub lyrics: Option<LyricsView<'a>>,
    pub cover_lyrics: Option<CoverLyricsView<'a>>,
    /// 0..1: cover view makes room for the lyrics lines.
    pub cover_lyrics_mix: f32,
    /// 0 = player view, 1 = cover view (spring, may overshoot slightly).
    pub view: f32,
    /// Idle view: 0 = cover view, 1 = lyrics focus view (SPEC §5.4a).
    pub focus: f32,
    /// Clock line visibility 0..1 (setting, faded).
    pub clock: f32,
    /// Clock line (SPEC §5.2): bold time and dimmed date.
    pub clock_time: &'a str,
    pub clock_date: &'a str,
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

/// Text layouts of the lyric lines of one track, with their heights.
struct LyricLayouts {
    id: u64,
    lines: Vec<(Option<IDWriteTextLayout>, f32)>,
}

/// Per-surface D2D/DWrite resources.
pub struct UiResources {
    size: (f32, f32),
    dwrite: IDWriteFactory,
    lyric: IDWriteTextFormat,
    lyric_size: f32,
    cover_line: IDWriteTextFormat,
    cover_next: IDWriteTextFormat,
    lyric_layouts: RefCell<Option<LyricLayouts>>,
    /// Effects work in pixels for 96-DPI bitmap inputs, positions in DIPs.
    dpi_scale: f32,
    title: IDWriteTextFormat,
    artist: IDWriteTextFormat,
    icon: IDWriteTextFormat,
    icon_big: IDWriteTextFormat,
    standby: IDWriteTextFormat,
    standby_small: IDWriteTextFormat,
    hint: IDWriteTextFormat,
    clock_time: IDWriteTextFormat,
    clock_date: IDWriteTextFormat,
    focus_title: IDWriteTextFormat,
    focus_artist: IDWriteTextFormat,
    brush: ID2D1SolidColorBrush,
    scale: ID2D1Effect,
    blur: ID2D1Effect,
    opacity: ID2D1Effect,
    bitmaps: RefCell<Vec<(u64, ID2D1Bitmap1)>>,
    brushes: RefCell<Vec<(u64, ID2D1BitmapBrush1)>>,
    text_layouts: RefCell<HashMap<TextKey, IDWriteTextLayout>>,
    gradient: RefCell<Option<ID2D1LinearGradientBrush>>,
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
        let (lyric_size, cover_size, cover_next_size) = layout::lyrics_sizes(h);
        let (_, clock_size) = layout::clock(h);
        let focus = layout::focus(w, h);
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
            let lyric = fmt(
                &ui_font,
                DWRITE_FONT_WEIGHT_BOLD,
                lyric_size,
                DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
                true,
            )?;
            lyric.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
            let clock_time = fmt(
                &ui_font,
                DWRITE_FONT_WEIGHT_BOLD,
                clock_size,
                DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
                false,
            )?;
            clock_time.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
            let clock_date = fmt(
                &ui_font,
                DWRITE_FONT_WEIGHT_NORMAL,
                clock_size * 0.6,
                DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
                false,
            )?;
            clock_date.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
            let white = color(1.0, 1.0, 1.0, 1.0);
            Ok(Self {
                size: (w, h),
                dwrite: dwrite.clone(),
                lyric,
                lyric_size,
                cover_line: fmt(
                    &ui_font,
                    DWRITE_FONT_WEIGHT_SEMI_BOLD,
                    cover_size,
                    DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                    false,
                )?,
                cover_next: fmt(
                    &ui_font,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    cover_next_size,
                    DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                    false,
                )?,
                lyric_layouts: RefCell::new(None),
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
                clock_time,
                clock_date,
                focus_title: fmt(
                    &ui_font,
                    DWRITE_FONT_WEIGHT_SEMI_BOLD,
                    focus.title_size,
                    DWRITE_PARAGRAPH_ALIGNMENT_FAR,
                    true,
                )?,
                focus_artist: fmt(
                    &ui_font,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    focus.artist_size,
                    DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
                    false,
                )?,
                brush: ctx.CreateSolidColorBrush(&white, None)?,
                scale,
                blur,
                opacity,
                bitmaps: RefCell::new(Vec::new()),
                brushes: RefCell::new(Vec::new()),
                text_layouts: RefCell::new(HashMap::new()),
                gradient: RefCell::new(None),
            })
        }
    }

    /// Size in DIPs.
    pub fn size(&self) -> (f32, f32) {
        self.size
    }

    /// Text layouts and heights of all lyric lines (cached per lyrics id).
    fn with_lyric_layouts<R>(
        &self,
        view: &LyricsView,
        width: f32,
        f: impl FnOnce(&[(Option<IDWriteTextLayout>, f32)]) -> R,
    ) -> R {
        let mut cache = self.lyric_layouts.borrow_mut();
        if cache.as_ref().is_none_or(|c| c.id != view.id) {
            let empty_h = self.lyric_size * 0.9;
            let lines = view
                .lines
                .iter()
                .map(|l| {
                    if l.text.is_empty() {
                        return (None, empty_h);
                    }
                    let wide: Vec<u16> = l.text.encode_utf16().collect();
                    // SAFETY: DirectWrite layout creation with a valid format.
                    let layout = unsafe {
                        self.dwrite
                            .CreateTextLayout(&wide, &self.lyric, width, 10_000.0)
                    };
                    match layout {
                        Ok(tl) => {
                            let mut m = DWRITE_TEXT_METRICS::default();
                            // SAFETY: out-pointer to a local struct.
                            let h = unsafe { tl.GetMetrics(&mut m) }
                                .map_or(self.lyric_size * 1.3, |()| m.height);
                            (Some(tl), h)
                        }
                        Err(_) => (None, empty_h),
                    }
                })
                .collect();
            *cache = Some(LyricLayouts { id: view.id, lines });
        }
        f(cache.as_ref().map_or(&[], |c| c.lines.as_slice()))
    }

    /// Cached text layout for a string in a box of `w` x `h` DIPs.
    fn text_layout(
        &self,
        s: &str,
        format: &IDWriteTextFormat,
        w: f32,
        h: f32,
    ) -> Option<IDWriteTextLayout> {
        let key = TextKey {
            text: s.to_string(),
            format: format.as_raw() as usize,
            w: w.round() as i32,
            h: h.round() as i32,
        };
        let mut cache = self.text_layouts.borrow_mut();
        if let Some(l) = cache.get(&key) {
            return Some(l.clone());
        }
        if cache.len() >= MAX_CACHED_LAYOUTS {
            cache.clear();
        }
        let wide: Vec<u16> = s.encode_utf16().collect();
        // SAFETY: DirectWrite layout creation with a valid format.
        let layout = unsafe {
            self.dwrite
                .CreateTextLayout(&wide, format, w.max(1.0), h.max(1.0))
        }
        .ok()?;
        cache.insert(key, layout.clone());
        Some(layout)
    }

    /// Bitmap brush for an image, created once per image.
    fn bitmap_brush(&self, ctx: &ID2D1DeviceContext, img: &Image) -> Option<ID2D1BitmapBrush1> {
        if let Some((_, b)) = self.brushes.borrow().iter().find(|(id, _)| *id == img.id) {
            return Some(b.clone());
        }
        let bmp = self.bitmap(ctx, img)?;
        let props = D2D1_BITMAP_BRUSH_PROPERTIES1 {
            extendModeX: D2D1_EXTEND_MODE_CLAMP,
            extendModeY: D2D1_EXTEND_MODE_CLAMP,
            interpolationMode: D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
        };
        // SAFETY: brush creation on a valid context.
        let brush = unsafe { ctx.CreateBitmapBrush(&bmp, Some(&props), None) }.ok()?;
        let mut cache = self.brushes.borrow_mut();
        if cache.len() >= MAX_CACHED_BITMAPS {
            cache.remove(0);
        }
        cache.push((img.id, brush.clone()));
        Some(brush)
    }

    /// Vertical transparent -> black gradient for the bottom 30 % (created once).
    fn bottom_gradient(&self, ctx: &ID2D1DeviceContext) -> Option<ID2D1LinearGradientBrush> {
        if let Some(b) = self.gradient.borrow().as_ref() {
            return Some(b.clone());
        }
        let h = self.size.1;
        let stops = [
            D2D1_GRADIENT_STOP {
                position: 0.0,
                color: color(0.0, 0.0, 0.0, 0.0),
            },
            D2D1_GRADIENT_STOP {
                position: 1.0,
                color: color(0.0, 0.0, 0.0, 0.8),
            },
        ];
        let rt: &ID2D1RenderTarget = ctx;
        let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
            startPoint: Vector2 { X: 0.0, Y: h * 0.7 },
            endPoint: Vector2 { X: 0.0, Y: h },
        };
        // SAFETY: resource creation on a valid context.
        let brush = unsafe {
            let coll = rt
                .CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)
                .ok()?;
            ctx.CreateLinearGradientBrush(&props, None, &coll).ok()?
        };
        *self.gradient.borrow_mut() = Some(brush.clone());
        Some(brush)
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

    /// Draws text in a box. Layouts are cached (per string, format and box
    /// size) because creating them every frame dominated the CPU time.
    fn text(&self, s: &str, format: &IDWriteTextFormat, r: Rect, c: D2D1_COLOR_F) {
        if c.a <= 0.0 || s.is_empty() {
            return;
        }
        let Some(layout) = self.res.text_layout(s, format, r.w, r.h) else {
            return;
        };
        self.fill_color(c);
        // SAFETY: called between BeginDraw/EndDraw.
        unsafe {
            self.ctx.DrawTextLayout(
                Vector2 { X: r.x, Y: r.y },
                &layout,
                &self.res.brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
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
        let Some(brush) = self.res.bitmap_brush(self.ctx, img) else {
            return;
        };
        // SAFETY: called between BeginDraw/EndDraw.
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

    /// Single-line layout with its ink width and baseline offset.
    fn measured(
        &self,
        s: &str,
        format: &IDWriteTextFormat,
        box_h: f32,
    ) -> Option<(IDWriteTextLayout, f32, f32)> {
        if s.is_empty() {
            return None;
        }
        let tl = self.res.text_layout(s, format, self.res.size.0, box_h)?;
        let mut m = DWRITE_TEXT_METRICS::default();
        let mut lines = [DWRITE_LINE_METRICS::default()];
        let mut count = 0u32;
        // SAFETY: out-pointers to locals; one line fits the slice (no wrapping).
        unsafe {
            tl.GetMetrics(&mut m).ok()?;
            tl.GetLineMetrics(Some(&mut lines), &mut count).ok()?;
        }
        Some((tl, m.width, lines[0].baseline))
    }

    fn layout_at(&self, tl: &IDWriteTextLayout, x: f32, y: f32, c: D2D1_COLOR_F) {
        if c.a <= 0.0 {
            return;
        }
        self.fill_color(c);
        // SAFETY: called between BeginDraw/EndDraw.
        unsafe {
            self.ctx.DrawTextLayout(
                Vector2 { X: x, Y: y },
                tl,
                &self.res.brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
        }
    }

    fn set_transform(&self, m: &Matrix3x2) {
        // SAFETY: plain state change on the device context.
        unsafe { self.ctx.SetTransform(m) };
    }
}

fn mat_mul(a: &Matrix3x2, b: &Matrix3x2) -> Matrix3x2 {
    Matrix3x2 {
        M11: a.M11 * b.M11 + a.M12 * b.M21,
        M12: a.M11 * b.M12 + a.M12 * b.M22,
        M21: a.M21 * b.M11 + a.M22 * b.M21,
        M22: a.M21 * b.M12 + a.M22 * b.M22,
        M31: a.M31 * b.M11 + a.M32 * b.M21 + b.M31,
        M32: a.M31 * b.M12 + a.M32 * b.M22 + b.M32,
    }
}

/// Entry scale: content grows from 96 % to 100 % with the intro spring.
fn intro_scale(m: &UiModel) -> f32 {
    0.96 + 0.04 * m.intro
}

fn player_transform(m: &UiModel, w: f32, h: f32) -> Matrix3x2 {
    scale_about((1.0 - 0.04 * m.view) * intro_scale(m), w / 2.0, h / 2.0)
}

fn cover_transform(m: &UiModel, w: f32, h: f32) -> Matrix3x2 {
    scale_about((1.04 - 0.04 * m.view) * intro_scale(m), w / 2.0, h / 2.0)
}

/// Placement of one lyric line.
#[derive(Debug, Clone, Copy)]
struct LineGeo {
    x: f32,
    y: f32,
    scale: f32,
    alpha: f32,
    /// Close to the current position: drawn sharp, otherwise softened.
    sharp: bool,
}

/// Top of the current line (~40 % of the lyrics area, SPEC §5.3).
fn lyrics_anchor(res: &UiResources, area: Rect) -> f32 {
    area.y + area.h * 0.40 - res.lyric_size * 0.65
}

/// Calls `f` for every visible lyric line with its layout and placement.
fn for_each_line(
    res: &UiResources,
    v: &LyricsView,
    mut f: impl FnMut(&IDWriteTextLayout, LineGeo),
) {
    let (w, h) = res.size;
    let area = layout::lyrics_area(w, h);
    let anchor = lyrics_anchor(res, area);
    res.with_lyric_layouts(v, area.w, |lines| {
        if lines.is_empty() {
            return;
        }
        let gap = res.lyric_size * 0.55;
        let mut prefix = Vec::with_capacity(lines.len() + 1);
        let mut acc = 0.0;
        for (_, lh) in lines {
            prefix.push(acc);
            acc += lh + gap;
        }
        prefix.push(acc);
        // Fractional scroll position in pixels.
        let pos = if v.scroll < 0.0 {
            v.scroll * (lines[0].1 + gap)
        } else {
            let s = v.scroll.min((lines.len() - 1) as f32);
            let i = s.floor() as usize;
            let next = prefix[(i + 1).min(lines.len() - 1)];
            prefix[i] + (next - prefix[i]) * s.fract()
        };
        let fade_zone = area.h * 0.18;
        for (i, (tl, lh)) in lines.iter().enumerate() {
            let y = anchor + prefix[i] - pos;
            if y + lh < area.y || y > area.y + area.h {
                continue;
            }
            let Some(tl) = tl else { continue };
            let d = (i as f32 - v.scroll).abs().min(1.0);
            let fade = ((y + lh - area.y) / fade_zone).clamp(0.0, 1.0)
                * ((area.y + area.h - y) / fade_zone).clamp(0.0, 1.0);
            f(
                tl,
                LineGeo {
                    x: area.x,
                    y,
                    scale: 1.0 - 0.2 * d,
                    alpha: (1.0 - 0.66 * d) * fade,
                    sharp: d < 0.5,
                },
            );
        }
    });
}

/// Draws one lyric line scaled around its top-left corner.
fn draw_line(
    ctx: &ID2D1DeviceContext,
    res: &UiResources,
    base: &Matrix3x2,
    tl: &IDWriteTextLayout,
    g: LineGeo,
    alpha: f32,
) {
    if alpha <= 0.0 {
        return;
    }
    let local = scale_about(g.scale, g.x, g.y);
    // SAFETY: called between BeginDraw/EndDraw on this context.
    unsafe {
        ctx.SetTransform(&mat_mul(&local, base));
        res.brush.SetColor(&color(1.0, 1.0, 1.0, alpha));
        ctx.DrawTextLayout(
            Vector2 { X: g.x, Y: g.y },
            tl,
            &res.brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
        );
        ctx.SetTransform(base);
    }
}

/// Slightly soft line: four sub-pixel offset copies at reduced opacity.
/// Replaces a Gaussian blur effect, whose 4K intermediate surfaces cost
/// ~33 MB RAM and noticeable CPU per frame (phase 9 measurement).
fn draw_soft_line(
    ctx: &ID2D1DeviceContext,
    res: &UiResources,
    base: &Matrix3x2,
    tl: &IDWriteTextLayout,
    g: LineGeo,
    alpha: f32,
) {
    let d = 0.9 * g.scale;
    for (dx, dy) in [(-d, -d * 0.4), (d, d * 0.4), (-d * 0.4, d), (d * 0.4, -d)] {
        let shifted = LineGeo {
            x: g.x + dx,
            y: g.y + dy,
            ..g
        };
        draw_line(ctx, res, base, tl, shifted, alpha * 0.34);
    }
}

/// Three pulsing dots for instrumental gaps.
fn draw_dots(p: &Painter, x: f32, cy: f32, size: f32, time: f32, alpha: f32) {
    for k in 0..3 {
        let phase = (time * 1.4 - k as f32 * 0.22) * std::f32::consts::TAU;
        let a = 0.3 + 0.7 * (0.5 + 0.5 * phase.sin());
        let r = size * (0.11 + 0.02 * (0.5 + 0.5 * phase.sin()));
        p.circle(
            x + size * 0.15 + k as f32 * size * 0.5,
            cy,
            r,
            color(1.0, 1.0, 1.0, a * alpha),
        );
    }
}

/// Scrolling lyrics on the right (split player view and lyrics focus view),
/// drawn under `base` with the final opacity `a`.
fn draw_lyrics(p: &Painter, m: &UiModel, base: &Matrix3x2, a: f32) {
    let Some(v) = &m.lyrics else { return };
    if a <= 0.0 {
        return;
    }
    let res = p.res;
    let (w, h) = res.size;
    p.set_transform(base);
    for_each_line(res, v, |tl, g| {
        if g.sharp {
            draw_line(p.ctx, res, base, tl, g, g.alpha * a);
        } else {
            draw_soft_line(p.ctx, res, base, tl, g, g.alpha * a);
        }
    });
    if v.gap {
        let area = layout::lyrics_area(w, h);
        let cy = lyrics_anchor(res, area) + res.lyric_size * 0.62;
        draw_dots(p, area.x, cy, res.lyric_size, m.time, a);
    }
}

/// Current and next line at the bottom of the cover view (SPEC §5.4).
fn draw_cover_lyrics(p: &Painter, m: &UiModel, alpha: f32) {
    let Some(c) = &m.cover_lyrics else { return };
    let a = c.alpha * alpha;
    if a <= 0.0 {
        return;
    }
    let res = p.res;
    let (w, h) = res.size;
    // Subtle gradient towards black for readability (brush created once).
    if let Some(brush) = res.bottom_gradient(p.ctx) {
        // SAFETY: drawing between BeginDraw/EndDraw.
        unsafe {
            brush.SetOpacity(a);
            p.ctx
                .FillRectangle(&d2d_rect(Rect::new(0.0, h * 0.7, w, h * 0.3)), &brush);
        }
    }
    let (_, size, next_size) = layout::lyrics_sizes(h);
    let y_cur = h * 0.875;
    let y_next = h * 0.935;
    let slide = h * 0.022;
    let t = c.change.clamp(0.0, 1.0);
    let line = |y: f32, s: f32| Rect::centered(w / 2.0, y, w * 0.9, s * 1.6);
    if c.gap {
        draw_dots(p, w / 2.0 - size * 0.65, y_cur, size, m.time, a * t);
    } else {
        p.text(
            c.current,
            &res.cover_line,
            line(y_cur + (1.0 - t) * slide, size),
            color(1.0, 1.0, 1.0, a * t),
        );
    }
    if t < 1.0 {
        p.text(
            c.previous,
            &res.cover_line,
            line(y_cur - t * slide, size),
            color(1.0, 1.0, 1.0, a * (1.0 - t)),
        );
    }
    p.text(
        c.next,
        &res.cover_next,
        line(y_next, next_size),
        color(1.0, 1.0, 1.0, 0.45 * a * t),
    );
}

pub fn draw(ctx: &ID2D1DeviceContext, res: &UiResources, m: &UiModel) {
    let p = Painter { ctx, res };
    let (w, h) = res.size;
    let (cx, cy) = (w / 2.0, h / 2.0);
    let intro = m.intro.clamp(0.0, 1.0);
    let content = (1.0 - m.standby).clamp(0.0, 1.0) * intro;
    let view = m.view.clamp(0.0, 1.0);

    // ----- standby screen -----
    if m.standby > 0.0 {
        let (big, small) = layout::standby_sizes(h);
        let a = m.standby.clamp(0.0, 1.0) * intro;
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
    let player_a = (1.0 - view) * content;
    if player_a > 0.0 {
        p.set_transform(&player_transform(m, w, h));
        let l = m.player;
        for t in &m.tracks {
            let a = player_a * t.alpha;
            let cr = l.cover.scaled(t.scale);
            if let Some(s) = t.small {
                p.ambient(s, cr, 1.25, 0.55 * a);
            }
            p.cover(t.image.map(|i| i.as_ref()), cr, a);
            p.rounded(
                cr,
                cr.w * 0.035,
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
        let la = m.lyrics.as_ref().map_or(0.0, |v| v.alpha);
        draw_lyrics(&p, m, &player_transform(m, w, h), la * player_a);
    }

    // ----- idle views: cover view <-> lyrics focus view -----
    // The cover morphs between both places (shared element), the rest
    // crossfades: bottom lyric lines out, title/artist and lyrics in.
    let idle_a = view * content;
    if idle_a > 0.0 {
        let base = cover_transform(m, w, h);
        p.set_transform(&base);
        let f = m.focus;
        let fa = f.clamp(0.0, 1.0);
        let fl = layout::focus(w, h);
        let r = lerp_rect(
            layout::cover_view(w, h, m.cover_lyrics_mix.clamp(0.0, 1.0)),
            fl.cover,
            f,
        );
        let grow = 1.35 - 0.1 * fa;
        let rise = (1.0 - fa) * h * 0.015;
        for t in &m.tracks {
            let a = idle_a * t.alpha;
            let cr = r.scaled(t.scale);
            if let Some(s) = t.small {
                p.ambient(s, cr, grow, 0.6 * a);
            }
            p.cover(t.image.map(|i| i.as_ref()), cr, a);
            p.rounded(
                cr,
                cr.w * 0.035,
                color(0.02, 0.02, 0.03, 0.42 * m.pause * a),
            );
            let ta = a * fa;
            if ta > 0.0 {
                let up = |x: Rect| Rect { y: x.y + rise, ..x };
                p.text(
                    t.title,
                    &res.focus_title,
                    up(fl.title),
                    color(1.0, 1.0, 1.0, 0.95 * ta),
                );
                p.text(
                    t.artist,
                    &res.focus_artist,
                    up(fl.artist),
                    color(1.0, 1.0, 1.0, 0.55 * ta),
                );
            }
        }
        draw_cover_lyrics(&p, m, idle_a * (1.0 - fa));
        if fa > 0.0 {
            let slide = translate((1.0 - fa) * w * 0.03, 0.0);
            draw_lyrics(&p, m, &mat_mul(&slide, &base), idle_a * fa);
        }
    }
    p.set_transform(&scale_about(1.0, 0.0, 0.0));

    // ----- clock -----
    draw_clock(&p, m, intro * m.clock.clamp(0.0, 1.0));

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

fn lerp_rect(a: Rect, b: Rect, t: f32) -> Rect {
    Rect {
        x: a.x + (b.x - a.x) * t,
        y: a.y + (b.y - a.y) * t,
        w: a.w + (b.w - a.w) * t,
        h: a.h + (b.h - a.h) * t,
    }
}

fn translate(dx: f32, dy: f32) -> Matrix3x2 {
    Matrix3x2 {
        M11: 1.0,
        M12: 0.0,
        M21: 0.0,
        M22: 1.0,
        M31: dx,
        M32: dy,
    }
}

/// Clock line at the top center in the style of the iOS lock screen:
/// bold time, smaller dimmed date, sharing one baseline.
fn draw_clock(p: &Painter, m: &UiModel, alpha: f32) {
    if alpha <= 0.0 {
        return;
    }
    let res = p.res;
    let (w, h) = res.size;
    let (cy, size) = layout::clock(h);
    let box_h = size * 1.6;
    let Some((time, tw, tb)) = p.measured(m.clock_time, &res.clock_time, box_h) else {
        return;
    };
    let date = p.measured(m.clock_date, &res.clock_date, box_h);
    let gap = size * 0.4;
    let total = tw + date.as_ref().map_or(0.0, |(_, dw, _)| gap + dw);
    let x = (w - total) / 2.0;
    // Digits are ~0.7 em tall: center them on `cy`.
    let baseline = cy + size * 0.35;
    p.layout_at(&time, x, baseline - tb, color(1.0, 1.0, 1.0, 0.94 * alpha));
    if let Some((dl, _, db)) = date {
        p.layout_at(
            &dl,
            x + tw + gap,
            baseline - db,
            color(1.0, 1.0, 1.0, 0.62 * alpha),
        );
    }
}

fn draw_buttons(p: &Painter, m: &UiModel, alpha: f32) {
    let [ar, ag, ab] = m.accent;
    for (bv, (_, rect)) in m.buttons.iter().zip(m.player.buttons.iter()) {
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
