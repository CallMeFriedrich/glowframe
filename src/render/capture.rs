//! Live wallpaper (Wallpaper Engine, Lively, ...) as background (SPEC §5.1
//! v1.5): the desktop window hosting the wallpaper is captured with Windows
//! Graphics Capture and drawn per monitor. Desktop icons are hidden while
//! capturing (they live in the same window on current Windows 11).

use std::path::PathBuf;

use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_BITMAP_OPTIONS_NONE, D2D1_BITMAP_PROPERTIES1, D2D1_DEVICE_CONTEXT_OPTIONS_NONE,
    ID2D1Bitmap1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_SHADER_RESOURCE, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{IDXGIDevice, IDXGISurface};
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, FindWindowW, GetWindowRect, IsWindowVisible, SW_HIDE, SW_SHOW, ShowWindow,
};
use windows::core::{Interface, Result, w};

use super::gpu::Gpu;

/// Where the live wallpaper lives.
struct Host {
    /// Top-level window to capture.
    root: HWND,
    /// Desktop icon list inside `root` (hidden while capturing), if any.
    icons: Option<HWND>,
}

/// Finds a live wallpaper: a `WorkerW` with visible child windows under
/// `Progman` (Windows 11 24H2+) or as its own top-level window (older).
fn find_host() -> Option<Host> {
    // SAFETY: plain window lookups by class name.
    unsafe {
        let progman = FindWindowW(w!("Progman"), None).ok()?;
        let defview = FindWindowExW(Some(progman), None, w!("SHELLDLL_DefView"), None).ok();
        let icons =
            defview.and_then(|d| FindWindowExW(Some(d), None, w!("SysListView32"), None).ok());
        // New layout: WorkerW is a child of Progman.
        let mut child = FindWindowExW(Some(progman), None, w!("WorkerW"), None).ok();
        while let Some(worker) = child {
            if has_visible_child(worker) {
                return Some(Host {
                    root: progman,
                    icons,
                });
            }
            child = FindWindowExW(Some(progman), Some(worker), w!("WorkerW"), None).ok();
        }
        // Old layout: a top-level WorkerW (without icons) holds the wallpaper.
        let mut top = FindWindowExW(None, None, w!("WorkerW"), None).ok();
        while let Some(worker) = top {
            let has_icons = FindWindowExW(Some(worker), None, w!("SHELLDLL_DefView"), None).is_ok();
            if !has_icons && IsWindowVisible(worker).as_bool() && has_visible_child(worker) {
                return Some(Host {
                    root: worker,
                    icons: None,
                });
            }
            top = FindWindowExW(None, Some(worker), w!("WorkerW"), None).ok();
        }
        None
    }
}

fn has_visible_child(parent: HWND) -> bool {
    // SAFETY: window enumeration by handle.
    unsafe {
        let mut c = FindWindowExW(Some(parent), None, None, None).ok();
        while let Some(h) = c {
            if IsWindowVisible(h).as_bool() {
                return true;
            }
            c = FindWindowExW(Some(parent), Some(h), None, None).ok();
        }
        false
    }
}

/// Shows the desktop icons again if a previous run hid them and ended
/// without restoring (crash). `marker` exists while icons are hidden.
pub fn restore_icons_after_crash(marker: &std::path::Path) {
    if !marker.exists() {
        return;
    }
    if let Some(Host { icons: Some(h), .. }) = find_host() {
        // SAFETY: showing a shell window by handle.
        unsafe {
            let _ = ShowWindow(h, SW_SHOW);
        }
    }
    let _ = std::fs::remove_file(marker);
    tracing::info!("desktop icons restored after an unclean exit");
}

pub struct DesktopCapture {
    session: GraphicsCaptureSession,
    pool: Direct3D11CaptureFramePool,
    _item: GraphicsCaptureItem,
    device: IDirect3DDevice,
    size: SizeInt32,
    /// Copy of the latest frame and its D2D view.
    texture: Option<ID3D11Texture2D>,
    bitmap: Option<ID2D1Bitmap1>,
    root: HWND,
    hidden_icons: Option<HWND>,
    marker: PathBuf,
}

impl DesktopCapture {
    /// Starts capturing the live wallpaper host, or `Ok(None)` if there is
    /// no live wallpaper. `marker` records hidden icons (crash safety).
    pub fn start(gpu: &Gpu, marker: PathBuf) -> Result<Option<Self>> {
        if !GraphicsCaptureSession::IsSupported().unwrap_or(false) {
            return Ok(None);
        }
        let Some(host) = find_host() else {
            return Ok(None);
        };
        // SAFETY: WinRT interop on valid D3D/DXGI objects and window handle.
        let (item, device) = unsafe {
            let interop =
                windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
            let item: GraphicsCaptureItem = interop.CreateForWindow(host.root)?;
            let dxgi: IDXGIDevice = gpu.device.cast()?;
            let device: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)?.cast()?;
            (item, device)
        };
        let size = item.Size()?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            size,
        )?;
        let session = pool.CreateCaptureSession(&item)?;
        // Not available everywhere (Windows 10): then a border is drawn
        // around the desktop, hidden behind our window anyway.
        let _ = session.SetIsBorderRequired(false);
        let _ = session.SetIsCursorCaptureEnabled(false);
        session.StartCapture()?;

        // Icons are part of the captured window on current Windows 11.
        let mut hidden_icons = None;
        if let Some(icons) = host.icons {
            // SAFETY: visibility of a shell window by handle.
            unsafe {
                if IsWindowVisible(icons).as_bool() {
                    let _ = std::fs::write(&marker, b"");
                    let _ = ShowWindow(icons, SW_HIDE);
                    hidden_icons = Some(icons);
                }
            }
        }
        tracing::info!(
            width = size.Width,
            height = size.Height,
            icons_hidden = hidden_icons.is_some(),
            "live wallpaper capture started"
        );
        Ok(Some(Self {
            session,
            pool,
            _item: item,
            device,
            size,
            texture: None,
            bitmap: None,
            root: host.root,
            hidden_icons,
            marker,
        }))
    }

    /// Physical screen position of the captured window's top-left corner.
    pub fn origin(&self) -> (i32, i32) {
        let mut r = RECT::default();
        // SAFETY: reading the rectangle of a window by handle.
        let _ = unsafe { GetWindowRect(self.root, &mut r) };
        (r.left, r.top)
    }

    /// Latest frame as a D2D bitmap (96 DPI: DIPs = pixels).
    pub fn bitmap(&self) -> Option<&ID2D1Bitmap1> {
        self.bitmap.as_ref()
    }

    /// Takes the newest captured frame, if any, into the own texture.
    pub fn poll(&mut self, gpu: &Gpu) {
        let mut latest = None;
        while let Ok(frame) = self.pool.TryGetNextFrame() {
            if let Some(old) = latest.replace(frame) {
                let _ = old.Close();
            }
        }
        let Some(frame) = latest else { return };
        if let Err(e) = self.take(gpu, &frame) {
            tracing::debug!(error = %e, "capture frame skipped");
        }
        let _ = frame.Close();
    }

    fn take(
        &mut self,
        gpu: &Gpu,
        frame: &windows::Graphics::Capture::Direct3D11CaptureFrame,
    ) -> Result<()> {
        let size = frame.ContentSize()?;
        if size != self.size {
            // Desktop resized (monitor change): new buffers, new texture.
            self.pool.Recreate(
                &self.device,
                DirectXPixelFormat::B8G8R8A8UIntNormalized,
                2,
                size,
            )?;
            self.size = size;
            self.texture = None;
            self.bitmap = None;
            return Ok(());
        }
        // SAFETY: the frame surface is a D3D11 texture of our device; the
        // copy target matches its size and format.
        unsafe {
            let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
            let src: ID3D11Texture2D = access.GetInterface()?;
            if self.texture.is_none() {
                let mut desc = D3D11_TEXTURE2D_DESC::default();
                src.GetDesc(&mut desc);
                let desc = D3D11_TEXTURE2D_DESC {
                    Width: desc.Width,
                    Height: desc.Height,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                    CPUAccessFlags: 0,
                    MiscFlags: 0,
                };
                let mut tex = None;
                gpu.device.CreateTexture2D(&desc, None, Some(&mut tex))?;
                let tex = tex.ok_or_else(windows::core::Error::empty)?;
                let surface: IDXGISurface = tex.cast()?;
                let ctx = gpu
                    .d2d_device
                    .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
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
                self.bitmap = Some(ctx.CreateBitmapFromDxgiSurface(&surface, Some(&props))?);
                self.texture = Some(tex);
            }
            if let Some(dst) = &self.texture {
                gpu.context.CopyResource(dst, &src);
            }
        }
        Ok(())
    }
}

impl Drop for DesktopCapture {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
        if let Some(icons) = self.hidden_icons.take() {
            // SAFETY: showing the shell window again by handle.
            unsafe {
                let _ = ShowWindow(icons, SW_SHOW);
            }
            let _ = std::fs::remove_file(&self.marker);
            tracing::info!("desktop icons shown again");
        }
    }
}
