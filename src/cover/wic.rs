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

    /// Decodes the first frame; `size` scales to exactly `size x size`
    /// (high-quality Fant interpolation).
    pub fn decode(&self, bytes: &[u8], size: Option<u32>) -> Result<Image> {
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

            if let Some(size) = size {
                let scaler = self.factory.CreateBitmapScaler()?;
                scaler.Initialize(&source, size, size, WICBitmapInterpolationModeFant)?;
                source = scaler.cast()?;
            }

            let (mut w, mut h) = (0u32, 0u32);
            source.GetSize(&mut w, &mut h)?;
            let stride = w * 4;
            let mut bgra = vec![0u8; (stride * h) as usize];
            source.CopyPixels(std::ptr::null(), stride, &mut bgra)?;
            Ok(Image {
                width: w,
                height: h,
                bgra,
            })
        }
    }
}
