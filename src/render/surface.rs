//! Per-window swap chain with a D3D11 render target and a D2D target on the
//! same DXGI surface.

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, ID2D1Bitmap1,
    ID2D1DeviceContext,
};
use windows::Win32::Graphics::Direct3D::D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_VIEWPORT, ID3D11RenderTargetView, ID3D11Texture2D,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM,
    DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Dxgi::{
    DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET, DXGI_MWA_NO_ALT_ENTER, DXGI_PRESENT,
    DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG,
    DXGI_SWAP_EFFECT_FLIP_DISCARD, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
    DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGISurface, IDXGISwapChain1,
};
use windows::core::Interface;
use windows::core::Result;

use super::GlowParams;
use super::gpu::Gpu;
use super::ui::{self, UiModel, UiResources};

#[derive(Debug, PartialEq, Eq)]
pub enum FrameResult {
    Ok,
    /// The GPU device was removed/reset; everything must be recreated.
    DeviceLost,
}

/// DirectComposition objects of a transparent window (kept alive with it).
struct Composition {
    _device: IDCompositionDevice,
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual,
}

pub struct Surface {
    swapchain: IDXGISwapChain1,
    d2d: ID2D1DeviceContext,
    rtv: Option<ID3D11RenderTargetView>,
    target: Option<ID2D1Bitmap1>,
    ui: UiResources,
    width: u32,
    height: u32,
    scale: f32,
    /// Premultiplied alpha through DirectComposition (SPEC §5.1 v1.5).
    transparent: bool,
    _composition: Option<Composition>,
}

impl Surface {
    pub fn new(
        gpu: &Gpu,
        hwnd: HWND,
        width: u32,
        height: u32,
        scale: f32,
        transparent: bool,
    ) -> Result<Self> {
        let (width, height) = (width.max(1), height.max(1));
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: width,
            Height: height,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: if transparent {
                DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL
            } else {
                DXGI_SWAP_EFFECT_FLIP_DISCARD
            },
            AlphaMode: if transparent {
                DXGI_ALPHA_MODE_PREMULTIPLIED
            } else {
                DXGI_ALPHA_MODE_IGNORE
            },
            ..Default::default()
        };
        // SAFETY: valid device and window handle owned by this thread.
        let (swapchain, d2d, composition) = unsafe {
            let (sc, composition) = if transparent {
                let sc =
                    gpu.dxgi_factory
                        .CreateSwapChainForComposition(&gpu.device, &desc, None)?;
                let dxgi_device: IDXGIDevice = gpu.device.cast()?;
                let device: IDCompositionDevice = DCompositionCreateDevice(&dxgi_device)?;
                let target = device.CreateTargetForHwnd(hwnd, true)?;
                let visual = device.CreateVisual()?;
                visual.SetContent(&sc)?;
                target.SetRoot(&visual)?;
                device.Commit()?;
                (
                    sc,
                    Some(Composition {
                        _device: device,
                        _target: target,
                        _visual: visual,
                    }),
                )
            } else {
                let sc = gpu.dxgi_factory.CreateSwapChainForHwnd(
                    &gpu.device,
                    hwnd,
                    &desc,
                    None,
                    None,
                )?;
                (sc, None)
            };
            let _ = gpu
                .dxgi_factory
                .MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER);
            let d2d = gpu
                .d2d_device
                .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            if transparent {
                // ClearType needs an opaque target.
                d2d.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            }
            (sc, d2d, composition)
        };
        let ui = UiResources::new(
            &gpu.dwrite,
            &d2d,
            width as f32 / scale,
            height as f32 / scale,
            scale,
        )?;
        let mut s = Self {
            swapchain,
            d2d,
            rtv: None,
            target: None,
            ui,
            width,
            height,
            scale,
            transparent,
            _composition: composition,
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
                    alphaMode: if self.transparent {
                        D2D1_ALPHA_MODE_PREMULTIPLIED
                    } else {
                        D2D1_ALPHA_MODE_IGNORE
                    },
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
        self.ui = UiResources::new(
            &gpu.dwrite,
            &self.d2d,
            width as f32 / scale,
            height as f32 / scale,
            scale,
        )?;
        self.create_targets(gpu)
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Size in DIPs (layout and hit testing).
    pub fn size_dip(&self) -> (f32, f32) {
        self.ui.size()
    }

    /// Draws the glow frame and the UI, then presents with VSync.
    pub fn render(&self, gpu: &Gpu, params: &GlowParams, model: &UiModel) -> FrameResult {
        let ctx = &gpu.context;
        // SAFETY: COM calls on valid objects; `params` is a repr(C) struct
        // matching the HLSL cbuffer layout.
        unsafe {
            // 1. Background: black, transparent, or image, plus dimming.
            self.d2d.BeginDraw();
            ui::draw_background(&self.d2d, &self.ui, model, self.transparent);
            if let Err(e) = self.d2d.EndDraw(None, None) {
                tracing::warn!(error = %e, "D2D EndDraw (background) failed");
            }

            // 2. Glow frame, added on top.
            ctx.UpdateSubresource(
                &gpu.params,
                0,
                None,
                (params as *const GlowParams).cast(),
                0,
                0,
            );
            ctx.OMSetRenderTargets(Some(std::slice::from_ref(&self.rtv)), None);
            ctx.OMSetBlendState(&gpu.blend_add, None, u32::MAX);
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
            ctx.OMSetBlendState(None, None, u32::MAX);

            // 3. UI on top.
            self.d2d.BeginDraw();
            ui::draw(&self.d2d, &self.ui, model);
            if let Err(e) = self.d2d.EndDraw(None, None) {
                tracing::warn!(error = %e, "D2D EndDraw failed");
            }

            let hr = self.swapchain.Present(1, DXGI_PRESENT(0));
            if hr == DXGI_ERROR_DEVICE_REMOVED || hr == DXGI_ERROR_DEVICE_RESET {
                tracing::warn!(?hr, "GPU device lost");
                return FrameResult::DeviceLost;
            }
        }
        FrameResult::Ok
    }
}
