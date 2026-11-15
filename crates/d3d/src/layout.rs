//! Where the statically linked D3D library keeps its globals in this XBE.
//!
//! Every address here was recovered from the retail Halo 2 XBE (SHA-256
//! `03215919...935d`, XDK 5849 `D3D8LTCG`) by disassembly; see `docs/d3d.md`
//! for the evidence. They are facts about one binary, so they live in one
//! struct that a different build (or a test) can replace.

/// Guest addresses of the D3D globals and the layout of the device object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GuestLayout {
    /// The `CDevice` object itself (static; `Direct3D_CreateDevice` returns
    /// its address and clears `DEVICE_DWORDS` dwords of it on failure).
    pub device: u32,
    /// `D3D__pDevice`: pointer to the device, read by inlined code.
    pub p_device: u32,
    /// `D3D__RenderState[]`, guest (build 5849) numbering.
    pub render_state: u32,
    /// `D3D__TextureState[4][32]`.
    pub texture_state: u32,
    /// `D3D__DirtyFlags`: deferred-state dirty bits.
    pub dirty_flags: u32,
    /// `D3D__Streams[16]`: 12-byte `{stride, offset, vertex buffer}` records.
    pub streams: u32,
    /// Shadow of the 192 vertex-shader constants (16 bytes each).
    pub vs_constants: u32,
    /// Push-buffer size requested before device creation (`D3D_SetPushBufferSize`).
    pub push_buffer_size: u32,
    /// Kick-off segment size (requests are for half of it).
    pub kickoff_size: u32,
    /// The game's own copy of the device pointer (written by its
    /// `CreateDevice` wrapper through the `ppReturnedDeviceInterface` out
    /// parameter). Informational: the device writes only what D3D writes.
    pub game_device_pointer: u32,
    /// D3D's static vertex-shader object used by `SelectVertexShaderDirect`
    /// (it copies the attribute format to `+0x14` and selects handle
    /// `addr | 1`). Halo's vertex choke point uses it directly.
    pub static_vertex_shader: u32,
    /// `D3DSIMPLERENDERSTATEENCODE[92]`, read-only data in the image; the
    /// device reads it at creation ([`crate::pushbuf::SimpleEncode`]).
    pub simple_encode: u32,
}

/// Number of guest render-state slots in build 5849.
pub const RENDER_STATE_SLOTS: usize = 166;
/// Texture stages.
pub const STAGES: usize = 4;
/// `D3DTSS_*` slots per stage in `D3D__TextureState`.
pub const STAGE_SLOTS: usize = 32;
/// Size of the device object in dwords (`rep stosd` count in CreateDevice).
pub const DEVICE_DWORDS: u32 = 0x928;

/// Offsets into the device object that guest code reads or writes. Only the
/// ones with evidence are listed; see `docs/d3d.md` for which code touches
/// each.
pub mod device_field {
    /// Push-buffer put pointer (inlined writers advance it).
    pub const PUT: u32 = 0x00;
    /// Push-buffer threshold; inlined writers call `MakeRequestedSpace`
    /// when `put >= threshold`.
    pub const THRESHOLD: u32 = 0x04;
    /// State flags. Bit 0x10 is `BehaviorFlags & 0x10` from CreateDevice
    /// (tested by game code); bit 0x800 is set by inlined `Begin` and draws.
    pub const FLAGS: u32 = 0x08;
    /// Base vertex index passed to `SetStateVB` by `DrawIndexedVertices`.
    pub const BASE_VERTEX: u32 = 0x1C;
    /// GPU time stamp used as the "last used" tag in resource `Lock` words.
    pub const GPU_TIME: u32 = 0x2C;
    /// Current vertex shader object (`+4` flags, `+0x14` attribute formats).
    pub const VERTEX_SHADER: u32 = 0x794;
    /// Mirror of `PSTEXTUREMODES` written by game code alongside the array.
    pub const PS_TEXTURE_MODES: u32 = 0x790;
    /// Embedded pixel-shader object `{refcount, flags, pPSDef}` that game code
    /// fills in and passes to `SetPixelShader`.
    pub const PIXEL_SHADER_OBJECT: u32 = 0x924;
    /// Device reference count. CreateDevice stores 1 (`0x3f528b`, absolute
    /// `0x405918`); Halo's inlined Release (`0x12448`, `0x1423a`) calls the
    /// teardown `0x3fc530` when it is 1 and decrements it otherwise.
    pub const REF_COUNT: u32 = 0x938;
    /// Viewport `{X, Y, Width, Height, MinZ, MaxZ}` as SetViewport stores it
    /// (`0x3f82a1..0x3f82cb`). Halo saves and restores these six words
    /// around full-screen clears (`0x14ac0`), and Clear derives its default
    /// rectangle from them (`0x3fa1a0`).
    pub const VIEWPORT: u32 = 0xEE0;
    /// `SetShaderConstantMode` mode without the `0x10` bit (`0x3f6e32`).
    pub const SHADER_CONSTANT_MODE: u32 = 0x1928;
    /// Stream-input count set by `SetVertexShaderInput`.
    pub const STREAM_INPUT_COUNT: u32 = 0xB04;
    /// Stream inputs (`D3DSTREAM_INPUT[]`, 12 bytes each).
    pub const STREAM_INPUTS: u32 = 0xB08;
    /// Bound textures, one dword per stage.
    pub const TEXTURES: u32 = 0xF88;
    /// Current render-target surface header.
    pub const RENDER_TARGET: u32 = 0x1A04;
    /// Current depth/stencil surface header.
    pub const DEPTH_STENCIL: u32 = 0x1A08;
    /// Back buffer surface header.
    pub const BACK_BUFFER: u32 = 0x1A14;
    /// Display surface used for viewport sizing when the target is the back
    /// buffer (same header unless back-buffer scaling is active).
    pub const DISPLAY_SURFACE: u32 = 0x1A18;
    /// Swap counter returned by `Swap`.
    pub const SWAP_COUNT: u32 = 0x2478;
}

impl GuestLayout {
    /// The retail Halo 2 XBE.
    pub const HALO2: Self = Self {
        device: 0x0040_4FE0,
        p_device: 0x0040_7488,
        render_state: 0x0040_7498,
        texture_state: 0x0040_4DD0,
        dirty_flags: 0x0040_7490,
        streams: 0x0040_8458,
        vs_constants: 0x0040_7738,
        push_buffer_size: 0x0040_864C,
        kickoff_size: 0x0040_8648,
        game_device_pointer: 0x0050_93B0,
        static_vertex_shader: 0x0040_8518,
        simple_encode: 0x0043_E6F0,
    };

    /// Address of render-state slot `slot` (guest numbering).
    pub fn render_state_slot(&self, slot: u32) -> u32 {
        self.render_state + slot * 4
    }

    /// Address of texture-stage state `state` of `stage`.
    pub fn texture_state_slot(&self, stage: u32, state: u32) -> u32 {
        self.texture_state + stage * (STAGE_SLOTS as u32 * 4) + state * 4
    }

    /// Address of a device field.
    pub fn field(&self, offset: u32) -> u32 {
        self.device + offset
    }
}

impl Default for GuestLayout {
    fn default() -> Self {
        Self::HALO2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halo2_slots_match_disassembly() {
        let l = GuestLayout::HALO2;
        // SetRenderState_ZEnable stores to 0x4076D4, CullMode to 0x4076E4.
        assert_eq!(l.render_state_slot(143), 0x4076D4);
        assert_eq!(l.render_state_slot(147), 0x4076E4);
        // SetTextureState_BorderColor stores to stage*0x80 + 0x404E44.
        assert_eq!(l.texture_state_slot(1, 29), 0x404E44 + 0x80);
        // The texture-state array ends right before the device object.
        assert!(l.texture_state_slot(3, 31) < l.device);
        // The render-state array ends before the constant shadow.
        assert!(l.render_state_slot(RENDER_STATE_SLOTS as u32) <= l.vs_constants);
    }
}
