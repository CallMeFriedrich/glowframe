//! Image decoding via WIC (SPEC §6.3).

use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICBitmapSource, IWICImagingFactory,
    WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant, WICBitmapPaletteTypeCustom,
    WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::core::{Interface, Result};

/// Premultiplied 8-bit BGRA image.
#[derive(Clone)]
pub struct Image {
    /// Unique per decoded image; used as GPU bitmap cache key.
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Image({}x{})", self.width, self.height)
    }
}

pub struct Wic {
    factory: IWICImagingFactory,
}

impl Wic {
    /// Requires COM to be initialized on the calling thread.
    pub fn new() -> Result<Self> {
        // SAFETY: standard COM activation of the in-proc WIC factory.
        let factory =
            unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)? };
        Ok(Self { factory })
    }

    /// Pixel size of the first frame without decoding the pixels.
    pub fn size(&self, bytes: &[u8]) -> Result<(u32, u32)> {
        // SAFETY: local WIC objects; `bytes` outlives the stream.
        unsafe {
            let stream = self.factory.CreateStream()?;
            stream.InitializeFromMemory(bytes)?;
            let decoder = self.factory.CreateDecoderFromStream(
                &stream,
                std::ptr::null(),
                WICDecodeMetadataCacheOnDemand,
            )?;
            let frame = decoder.GetFrame(0)?;
            let (mut w, mut h) = (0u32, 0u32);
            frame.GetSize(&mut w, &mut h)?;
            Ok((w, h))
        }
    }

    /// Decodes the first frame, scaled down (aspect kept) to fit into
    /// `max_w x max_h` if larger (backgrounds, SPEC §5.1 v1.5).
    pub fn decode_fit(&self, bytes: &[u8], max_w: u32, max_h: u32) -> Result<Image> {
        let (w, h) = self.size(bytes)?;
        let f = (max_w as f32 / w.max(1) as f32)
            .min(max_h as f32 / h.max(1) as f32)
            .min(1.0);
        let target = (
            (w as f32 * f).round().max(1.0) as u32,
            (h as f32 * f).round().max(1.0) as u32,
        );
        self.decode_to(bytes, (f < 1.0).then_some(target))
    }

    /// Decodes the first frame; `size` scales to exactly `size x size`
    /// (high-quality Fant interpolation).
    pub fn decode(&self, bytes: &[u8], size: Option<u32>) -> Result<Image> {
        self.decode_to(bytes, size.map(|s| (s, s)))
    }

    fn decode_to(&self, bytes: &[u8], size: Option<(u32, u32)>) -> Result<Image> {
        // SAFETY: all WIC objects are used locally on this thread; `bytes`
        // outlives the stream, which is dropped at the end of this function.
        unsafe {
            let stream = self.factory.CreateStream()?;
            stream.InitializeFromMemory(bytes)?;
            let decoder = self.factory.CreateDecoderFromStream(
                &stream,
                std::ptr::null(),
                WICDecodeMetadataCacheOnDemand,
            )?;
            let frame = decoder.GetFrame(0)?;

            let converter = self.factory.CreateFormatConverter()?;
            converter.Initialize(
                &frame,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )?;
            let mut source: IWICBitmapSource = converter.cast()?;

            if let Some((sw, sh)) = size {
                let scaler = self.factory.CreateBitmapScaler()?;
                scaler.Initialize(&source, sw, sh, WICBitmapInterpolationModeFant)?;
                source = scaler.cast()?;
            }

            let (mut w, mut h) = (0u32, 0u32);
            source.GetSize(&mut w, &mut h)?;
            let stride = w * 4;
            let mut bgra = vec![0u8; (stride * h) as usize];
            source.CopyPixels(std::ptr::null(), stride, &mut bgra)?;
            static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
            Ok(Image {
                id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                width: w,
                height: h,
                bgra,
            })
        }
    }
}
