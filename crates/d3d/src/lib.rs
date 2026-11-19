//! Guest-facing Direct3D 8 (Xbox XDK 5849) for Halo 2, at the API level.
//!
//! No NV2A emulation: D3D entry points the game calls are replaced with
//! [`Device`] methods that read guest structures through [`GuestMemory`] and
//! render through a [`Backend`] (`GfxBackend` on `gfx`/wgpu).
//!
//! Halo is an LTCG build, so D3D inlines into game code: the game writes the
//! render/texture-state arrays and NV2A push-buffer words itself. The device
//! keeps the guest arrays authoritative (read at every draw) and gives the
//! game a real guest push buffer that it decodes at every entry
//! ([`Device::sync_push_buffer`]). See `docs/graphics/d3d.md` for the inventory.
//!
//! Layout:
//! * [`guest`]: guest-memory trait and a sparse test implementation;
//! * [`layout`]: addresses of the D3D globals and device fields in Halo 2;
//! * [`resource`]: `D3DResource`/`D3DPixelContainer`/`D3DSurface` headers;
//! * [`state`]: render/texture-stage state arrays to `gfx` state;
//! * [`pushbuf`]: decoder for the push-buffer subset game code writes;
//! * [`regs`]: the NV2A register window behind device `+0x934`;
//! * [`dirty`]: the write-tracking contract for lazy uploads;
//! * [`device`]: the device model and entry points;
//! * [`abi`]: LTCG register-argument adapters (TODO-register);
//! * [`runtime_glue`]: [`register`] installs them as `crates/runtime` overrides;
//! * [`backend`], `gfx_backend`, [`shader`]: the host side;
//! * `nv2a`: the `ShaderProvider` over `crates/shaders`;
//! * `threaded`: a render thread that runs a backend's work off the
//!   guest's thread (opt-in, docs/graphics/render-thread.md);
//! * `interp`: in-between frames replayed with interpolated matrices
//!   (opt-in, docs/game/high-framerate.md).

pub mod abi;
pub mod backend;
pub mod device;
pub mod dirty;
pub mod events;
#[cfg(feature = "gpu")]
pub mod gfx_backend;
pub mod glyph;
pub mod guest;
pub mod hw;
#[cfg(feature = "gpu")]
pub mod interp;
pub mod layout;
#[cfg(feature = "gpu")]
pub mod nv2a;
#[cfg(feature = "gpu")]
pub mod pipeline_store;
pub mod pushbuf;
pub mod regs;
pub mod resource;
pub mod runtime_glue;
#[cfg(feature = "gpu")]
pub mod scaled;
pub mod shader;
pub mod state;
mod texdesc;
#[cfg(feature = "gpu")]
pub mod threaded;
#[cfg(feature = "gpu")]
pub mod uniform_compare;
pub mod vblank;
pub mod vertex_cache;

pub use backend::{
    DrawAdmission, DrawSnapshot, ExternalPlan, ExternalRecording, ExternalSlot, ExternalSurfaces, ExternalWrites, SharedReplacement, TargetCopy,
    Backend, ChannelMask, DepthGridRequest, DepthGridResult, DepthClip, DepthGridSink, DrawOutcome, ExternalPass, ExternalPassRequest, VIEW_CONSTANTS, external_viewport, GuestLayoutKind, PixelRect, Recorded, RecordingBackend,
    RectClear, ResolvedDraw, StreamBytes, StreamData, CachedBytes, CachedIndices, IndexData, Viewport,
};
pub use device::{
    DepthScale, Device, DeviceConfig, DrawScope, ExternalRefusal, GuestRange, OcclusionSink, PreparedExternal,
    PresentParameters, Stats, Swallow, SwallowCall, SwallowOriginal, SwallowPass, SwallowRefusal,
    SwallowTarget, Traced, TracedDraw, WritebackCause, WritebackCauses,
};
pub use dirty::{UploadPolicy, WriteTracker};
pub use events::{D3dError, Event, HleStop};
#[cfg(feature = "gpu")]
pub use gfx_backend::{GfxBackend, SwapFinish};
pub use guest::{GuestError, GuestMemory, SparseMemory};
#[cfg(feature = "gpu")]
pub use interp::InterpBackend;
pub use layout::GuestLayout;
#[cfg(feature = "gpu")]
pub use nv2a::Nv2aShaders;
pub use resource::AddressMap;
pub use runtime_glue::register;
#[cfg(feature = "gpu")]
pub use scaled::{LightingAb, ScaledBackend, lighting_ab, render_scale};
#[cfg(feature = "gpu")]
pub use backend::{ExternalColor, ExternalCopies, ExternalTarget, SampledLevel};
pub use shader::ShaderInputs;
#[cfg(feature = "gpu")]
pub use shader::ShaderProvider;
#[cfg(feature = "gpu")]
pub use threaded::{RENDER_THREAD_START, ThreadedBackend};
pub use vblank::{FrameWait, register_frame_wait};

/// The largest render scale: 6x of 640x480 is 3840x2880.
pub const MAX_SCALE: u32 = 6;

/// The view's lighting (docs/graphics/e3-lighting.md): the Video screen's
/// Lighting row, `[graphics] lighting`, `H2_LIGHTING`. Nothing of it
/// reaches the guest: the reference draws retail whatever this is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Lighting {
    /// The game's own: shadow maps at the guest's size, sampled as the
    /// NV2A does (2x2 percentage-closer filtering when the stage filters).
    #[default]
    Retail,
    /// Phase 1 of E3 lighting: shadow maps at `shadow_scale` times their
    /// size in the view, compared with one tap.
    E3,
}
