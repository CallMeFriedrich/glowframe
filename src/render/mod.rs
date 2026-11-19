//! D3D11 + D2D rendering for animation monitors (SPEC §3, §5).

pub mod gpu;
pub mod surface;
pub mod ui;

pub use gpu::Gpu;
pub use surface::{FrameResult, Surface};

/// Constant buffer of `glow.hlsl`. Layout must match the HLSL cbuffer
/// (16-byte rows).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct GlowParams {
    pub size: [f32; 2],
    pub time: f32,
    pub phase: f32,
    pub c0: [f32; 4],
    pub c1: [f32; 4],
    pub c2: [f32; 4],
    pub brightness: f32,
    pub width: f32,
    pub beat: f32,
    pub energy: f32,
    pub corner: f32,
    pub base_level: f32,
    pub arc_len: f32,
    pub dither_seed: f32,
}

const _: () = assert!(size_of::<GlowParams>().is_multiple_of(16));
