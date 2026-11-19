//! Shared GPU objects: D3D11 device, DXGI factory, D2D device, DirectWrite,
//! and the compiled glow shader. Created when the mode starts, dropped when
//! it ends (no GPU resources while idle).

use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct2D::{
    D2D1_DEBUG_LEVEL_NONE, D2D1_FACTORY_OPTIONS, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1CreateFactory, ID2D1Device, ID2D1Factory1,
};
use windows::Win32::Graphics::Direct3D::Fxc::{D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCompile};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_10_0,
    D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1, ID3DBlob,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_CONSTANT_BUFFER, D3D11_BUFFER_DESC, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_SDK_VERSION, D3D11_USAGE_DEFAULT, D3D11CreateDevice, ID3D11Buffer, ID3D11Device,
    ID3D11DeviceContext, ID3D11PixelShader, ID3D11VertexShader,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWriteCreateFactory, IDWriteFactory,
};
use windows::Win32::Graphics::Dxgi::{IDXGIAdapter, IDXGIDevice, IDXGIFactory2};
use windows::core::{Interface, PCSTR, Result, s};

use super::GlowParams;

const SHADER_SRC: &str = include_str!("shaders/glow.hlsl");

pub struct Gpu {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    pub dxgi_factory: IDXGIFactory2,
    pub d2d_device: ID2D1Device,
    pub dwrite: IDWriteFactory,
    pub vs: ID3D11VertexShader,
    pub ps: ID3D11PixelShader,
    pub params: ID3D11Buffer,
}

impl Gpu {
    pub fn new() -> Result<Self> {
        let (device, context) = match create_device(D3D_DRIVER_TYPE_HARDWARE) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(error = %e, "hardware device failed, using WARP");
                create_device(D3D_DRIVER_TYPE_WARP)?
            }
        };

        // SAFETY: COM calls on freshly created, valid interfaces.
        unsafe {
            let dxgi_device: IDXGIDevice = device.cast()?;
            let adapter: IDXGIAdapter = dxgi_device.GetAdapter()?;
            let dxgi_factory: IDXGIFactory2 = adapter.GetParent()?;
            if let Ok(desc) = adapter.GetDesc() {
                let name = String::from_utf16_lossy(&desc.Description);
                tracing::info!(adapter = %name.trim_end_matches('\0'), "GPU");
            }

            let options = D2D1_FACTORY_OPTIONS {
                debugLevel: D2D1_DEBUG_LEVEL_NONE,
            };
            let d2d_factory: ID2D1Factory1 =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, Some(&options))?;
            let d2d_device = d2d_factory.CreateDevice(&dxgi_device)?;

            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;

            let vs_code = compile(s!("vs_main"), s!("vs_4_0"))?;
            let ps_code = compile(s!("ps_main"), s!("ps_4_0"))?;
            let mut vs = None;
            device.CreateVertexShader(blob_bytes(&vs_code), None, Some(&mut vs))?;
            let mut ps = None;
            device.CreatePixelShader(blob_bytes(&ps_code), None, Some(&mut ps))?;

            let desc = D3D11_BUFFER_DESC {
                ByteWidth: size_of::<GlowParams>() as u32,
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                ..Default::default()
            };
            let mut params = None;
            device.CreateBuffer(&desc, None, Some(&mut params))?;

            Ok(Self {
                device,
                context,
                dxgi_factory,
                d2d_device,
                dwrite,
                vs: vs.ok_or_else(windows::core::Error::empty)?,
                ps: ps.ok_or_else(windows::core::Error::empty)?,
                params: params.ok_or_else(windows::core::Error::empty)?,
            })
        }
    }
}

fn create_device(driver: D3D_DRIVER_TYPE) -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    let levels = [
        D3D_FEATURE_LEVEL_11_1,
        D3D_FEATURE_LEVEL_11_0,
        D3D_FEATURE_LEVEL_10_1,
        D3D_FEATURE_LEVEL_10_0,
    ];
    let mut device = None;
    let mut context = None;
    // SAFETY: out-pointers reference local Options.
    unsafe {
        D3D11CreateDevice(
            None::<&IDXGIAdapter>,
            driver,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
    }
    match (device, context) {
        (Some(d), Some(c)) => Ok((d, c)),
        _ => Err(windows::core::Error::empty()),
    }
}

fn compile(entry: PCSTR, target: PCSTR) -> Result<ID3DBlob> {
    let mut code = None;
    let mut errors = None;
    // SAFETY: source pointer/length come from a static str.
    let result = unsafe {
        D3DCompile(
            SHADER_SRC.as_ptr().cast(),
            SHADER_SRC.len(),
            s!("glow.hlsl"),
            None,
            None,
            entry,
            target,
            D3DCOMPILE_OPTIMIZATION_LEVEL3,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    if let Err(e) = result {
        let msg = errors
            .as_ref()
            .map(|b| String::from_utf8_lossy(blob_bytes(b)).into_owned())
            .unwrap_or_default();
        tracing::error!(error = %e, %msg, "shader compilation failed");
        return Err(e);
    }
    code.ok_or_else(windows::core::Error::empty)
}

fn blob_bytes(blob: &ID3DBlob) -> &[u8] {
    // SAFETY: the blob owns the buffer for its lifetime, which bounds the slice.
    unsafe { std::slice::from_raw_parts(blob.GetBufferPointer().cast(), blob.GetBufferSize()) }
}
