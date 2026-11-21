//! D3D11 + D2D rendering for animation monitors (SPEC §3, §5).

pub mod capture;
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
    /// Comet tail length (fraction of the perimeter).
    pub tail_len: f32,
    pub dither_seed: f32,
    /// Comet head size and soft leading edge (fractions of the perimeter).
    pub head_len: f32,
    pub lead_len: f32,
    /// Weights (0..1) of the second and third comet (SPEC §5.5 v1.4).
    pub comet_w: [f32; 2],
    /// Distance between the comets (fraction of the perimeter).
    pub spread: f32,
    pub _pad: [f32; 3],
}

const _: () = assert!(size_of::<GlowParams>().is_multiple_of(16));
