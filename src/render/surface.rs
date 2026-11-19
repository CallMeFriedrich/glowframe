//! Per-window swap chain with a D3D11 render target and a D2D target on the
//! same DXGI surface.

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ROUNDED_RECT, ID2D1Bitmap1,
    ID2D1DeviceContext,
};
use windows::Win32::Graphics::Direct3D::D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_VIEWPORT, ID3D11RenderTargetView, ID3D11Texture2D,
};
use windows::Win32::Graphics::DirectWrite::DWRITE_MEASURING_MODE_NATURAL;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET, DXGI_MWA_NO_ALT_ENTER, DXGI_PRESENT,
    DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG,
    DXGI_SWAP_EFFECT_FLIP_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGISurface, IDXGISwapChain1,
};
use windows::core::Result;

use super::GlowParams;
use super::gpu::Gpu;

/// Text overlay drawn on top of the frame.
pub struct Overlay<'a> {
    /// Esc hint text and its opacity.
    pub hint: Option<(&'a str, f32)>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum FrameResult {
    Ok,
    /// The GPU device was removed/reset; everything must be recreated.
    DeviceLost,
}

pub struct Surface {
    swapchain: IDXGISwapChain1,
    d2d: ID2D1DeviceContext,
    rtv: Option<ID3D11RenderTargetView>,
    target: Option<ID2D1Bitmap1>,
    width: u32,
    height: u32,
    scale: f32,
}

impl Surface {
    pub fn new(gpu: &Gpu, hwnd: HWND, width: u32, height: u32, scale: f32) -> Result<Self> {
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: width.max(1),
            Height: height.max(1),
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            ..Default::default()
        };
        // SAFETY: valid device and window handle owned by this thread.
        let (swapchain, d2d) = unsafe {
            let sc =
                gpu.dxgi_factory
                    .CreateSwapChainForHwnd(&gpu.device, hwnd, &desc, None, None)?;
            let _ = gpu
                .dxgi_factory
                .MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER);
            let d2d = gpu
                .d2d_device
                .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            (sc, d2d)
        };
        let mut s = Self {
            swapchain,
            d2d,
            rtv: None,
            target: None,
            width: width.max(1),
            height: height.max(1),
            scale,
        };
        s.create_targets(gpu)?;
        Ok(s)
    }

    fn create_targets(&mut self, gpu: &Gpu) -> Result<()> {
        let dpi = 96.0 * self.scale;
        // SAFETY: buffer 0 of a flip-model swap chain is the current back buffer.
        unsafe {
            let back: ID3D11Texture2D = self.swapchain.GetBuffer(0)?;
            let mut rtv = None;
            gpu.device
                .CreateRenderTargetView(&back, None, Some(&mut rtv))?;
            self.rtv = rtv;

            let surface: IDXGISurface = self.swapchain.GetBuffer(0)?;
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_IGNORE,
                },
                dpiX: dpi,
                dpiY: dpi,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                ..Default::default()
            };
            let target = self
                .d2d
                .CreateBitmapFromDxgiSurface(&surface, Some(&props))?;
            self.d2d.SetTarget(&target);
            self.d2d.SetDpi(dpi, dpi);
            self.target = Some(target);
        }
        Ok(())
    }

    pub fn resize(&mut self, gpu: &Gpu, width: u32, height: u32, scale: f32) -> Result<()> {
        let (width, height) = (width.max(1), height.max(1));
        if (width, height, scale) == (self.width, self.height, self.scale) {
            return Ok(());
        }
        // SAFETY: all references to the back buffer are released before
        // ResizeBuffers, as required.
        unsafe {
            self.d2d.SetTarget(None);
            self.target = None;
            self.rtv = None;
            gpu.context.OMSetRenderTargets(None, None);
            gpu.context.Flush();
            self.swapchain.ResizeBuffers(
                0,
                width,
                height,
                DXGI_FORMAT_UNKNOWN,
                DXGI_SWAP_CHAIN_FLAG(0),
            )?;
        }
        self.width = width;
        self.height = height;
        self.scale = scale;
        self.create_targets(gpu)
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Draws the glow frame and the overlay, then presents with VSync.
    pub fn render(&self, gpu: &Gpu, params: &GlowParams, overlay: &Overlay) -> FrameResult {
        let ctx = &gpu.context;
        // SAFETY: COM calls on valid objects; `params` is a repr(C) struct
        // matching the HLSL cbuffer layout.
        unsafe {
            ctx.UpdateSubresource(
                &gpu.params,
                0,
                None,
                (params as *const GlowParams).cast(),
                0,
                0,
            );
            ctx.OMSetRenderTargets(Some(std::slice::from_ref(&self.rtv)), None);
            ctx.RSSetViewports(Some(&[D3D11_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: self.width as f32,
                Height: self.height as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            }]));
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.IASetInputLayout(None);
            ctx.VSSetShader(&gpu.vs, None);
            ctx.PSSetShader(&gpu.ps, None);
            ctx.PSSetConstantBuffers(0, Some(&[Some(gpu.params.clone())]));
            ctx.Draw(3, 0);

            if let Some((text, alpha)) = overlay.hint
                && alpha > 0.0
            {
                self.d2d.BeginDraw();
                self.draw_hint(gpu, text, alpha);
                if let Err(e) = self.d2d.EndDraw(None, None) {
                    tracing::warn!(error = %e, "D2D EndDraw failed");
                }
            }

            let hr = self.swapchain.Present(1, DXGI_PRESENT(0));
            if hr == DXGI_ERROR_DEVICE_REMOVED || hr == DXGI_ERROR_DEVICE_RESET {
                tracing::warn!(?hr, "GPU device lost");
                return FrameResult::DeviceLost;
            }
        }
        FrameResult::Ok
    }

    /// Bottom-center pill with the "press Esc again" hint (SPEC Â§4).
    unsafe fn draw_hint(&self, gpu: &Gpu, text: &str, alpha: f32) {
        let w = self.width as f32 / self.scale;
        let h = self.height as f32 / self.scale;
        let wide: Vec<u16> = text.encode_utf16().collect();
        let pill_w = 24.0 + 8.2 * text.chars().count() as f32;
        let pill_h = 38.0;
        let rect = D2D_RECT_F {
            left: (w - pill_w) * 0.5,
            top: h - 96.0,
            right: (w + pill_w) * 0.5,
            bottom: h - 96.0 + pill_h,
        };
        // SAFETY: called between BeginDraw and EndDraw on this context.
        unsafe {
            let bg = D2D1_COLOR_F {
                r: 0.08,
                g: 0.08,
                b: 0.1,
                a: 0.72 * alpha,
            };
            let fg = D2D1_COLOR_F {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.82 * alpha,
            };
            if let (Ok(bg), Ok(fg)) = (
                self.d2d.CreateSolidColorBrush(&bg, None),
                self.d2d.CreateSolidColorBrush(&fg, None),
            ) {
                self.d2d.FillRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect,
                        radiusX: pill_h * 0.5,
                        radiusY: pill_h * 0.5,
                    },
                    &bg,
                );
                self.d2d.DrawText(
                    &wide,
                    &gpu.hint_format,
                    &rect,
                    &fg,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }
        }
    }
}
