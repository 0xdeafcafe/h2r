//! Guest-facing Direct3D 8 (Xbox XDK 5849) for Halo 2, at the API level.
//!
//! No NV2A emulation: D3D entry points the game calls are replaced with
//! [`Device`] methods that read guest structures through [`GuestMemory`] and
//! render through a [`Backend`] ([`GfxBackend`] on `gfx`/wgpu).
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
//! * [`backend`], [`gfx_backend`], [`shader`]: the host side;
//! * [`nv2a`]: the [`ShaderProvider`] over `crates/shaders`;
//! * [`threaded`]: a render thread that runs a backend's work off the
//!   guest's thread (opt-in, docs/graphics/render-thread.md);
//! * [`interp`]: in-between frames replayed with interpolated matrices
//!   (opt-in, docs/game/high-framerate.md).

pub mod abi;
pub mod backend;
pub mod device;
pub mod dirty;
pub mod events;
pub mod gfx_backend;
pub mod glyph;
pub mod guest;
pub mod hw;
pub mod interp;
pub mod layout;
pub mod nv2a;
pub mod pipeline_store;
pub mod pushbuf;
pub mod regs;
pub mod resource;
pub mod runtime_glue;
pub mod scaled;
pub mod shader;
pub mod state;
mod texdesc;
pub mod threaded;
pub mod uniform_compare;
pub mod vblank;
pub mod vertex_cache;

pub use backend::{
    Backend, ChannelMask, DepthGridRequest, DepthGridResult, DepthClip, DepthGridSink, DrawOutcome, ExternalPass, ExternalPassRequest, ExternalTarget, SampledLevel, VIEW_CONSTANTS, external_viewport, GuestLayoutKind, PixelRect, Recorded, RecordingBackend,
    RectClear, ResolvedDraw, StreamBytes, StreamData, CachedBytes, CachedIndices, IndexData, Viewport,
};
pub use device::{DepthScale, Device, DeviceConfig, ExternalRefusal, OcclusionSink, GuestRange, PresentParameters, Stats, Swallow, Traced, TracedDraw, WritebackCause, WritebackCauses};
pub use dirty::{UploadPolicy, WriteTracker};
pub use events::{D3dError, Event, HleStop};
pub use gfx_backend::{GfxBackend, SwapFinish};
pub use guest::{GuestError, GuestMemory, SparseMemory};
pub use interp::InterpBackend;
pub use layout::GuestLayout;
pub use nv2a::Nv2aShaders;
pub use resource::AddressMap;
pub use runtime_glue::register;
pub use scaled::{Lighting, LightingAb, ScaledBackend, lighting_ab, render_scale};
pub use shader::{ShaderInputs, ShaderProvider};
pub use threaded::{RENDER_THREAD_START, ThreadedBackend};
pub use vblank::{FrameWait, register_frame_wait};
