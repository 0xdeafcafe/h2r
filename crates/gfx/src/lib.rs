//! Direct3D 8 (Xbox flavour) replacement on wgpu.
//!
//! The crate has three layers; see `docs/graphics.md` for the design.
//!
//! 1. **Pure Xbox data handling**, no GPU needed and fully unit-tested:
//!    [`format`] (the Xbox `D3DFORMAT` set), [`swizzle`] (Morton addressing),
//!    [`dxt`] (CPU DXT decoder), [`convert`] (texel expansion) and
//!    [`texture`] (header parsing, mip/cube/volume layout, the decode
//!    pipeline).
//! 2. **State model**: [`state`] (typed render/texture-stage state),
//!    [`vertex`] (primitive types, FVF, declarations), [`translate`] (state to
//!    wgpu descriptors) and [`pipeline`] (cache keyed by translated state).
//! 3. **Presentation**: [`context`], [`presenter`], [`renderer`] (with
//!    [`clear`] for partial clears and [`program`] for translated shader
//!    programs), [`upload`].
//!    None of it knows about windows; a platform layer hands in a
//!    `wgpu::Surface`.

pub mod blit;
pub mod clear;
pub mod context;
pub mod convert;
pub mod dxt;
pub mod encode;
pub mod format;
pub mod occlusion;
pub mod pipeline;
pub mod presenter;
pub mod program;
pub mod readback;
pub mod renderer;
pub mod repack;
pub mod ring;
pub mod state;
pub mod surface;
pub mod swizzle;
pub mod texture;
pub mod translate;
pub mod upload;
pub mod vertex;
pub mod xdk;

pub use context::{GfxContext, GfxError};
pub use format::{D3DFormat, HostFormat};
pub use presenter::{Acquired, OffscreenTarget, Presenter, RenderTarget};
pub use clear::MaskedClear;
pub use renderer::{BoundGroup, Clear, Draw, DrawViewport, RecordedDraw, Renderer, ScissorRect, VertexStream};
pub use state::RenderState;
pub use texture::{DecodeOptions, TextureDesc, decode_texture};
