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
//! * [`runtime_glue`]: [`register`] installs them as `crates/runtime/runtime` overrides;
//! * [`backend`], `gfx_backend`, [`shader`]: the host side;
//! * `nv2a`: the `ShaderProvider` over `crates/formats/shaders`;
//! * `threaded`: a render thread that runs a backend's work off the
//!   guest's thread (opt-in, docs/graphics/render-thread.md);
//! * [`scene_stage`]: the typed scene stages it prepares and records
//!   (docs/graphics/framescene.md, slice 2);
//! * [`scene_sink`]: the frame's carried part, filled from the guest's
//!   work by slot (docs/graphics/framescene.md, slice 6);
//! * `clip_space`: where a vertex program puts its clip position, which
//!   the carried draws' camera classification reads;
//! * [`gap`]: the guest draws no native stage takes, drawn by nothing
//!   and counted by caller;
//! * [`screen_rect`]: where a small draw's vertices land in the guest
//!   window, for the UI fit and `render.frame_layout`.

pub mod abi;
pub mod backend;
#[cfg(feature = "gpu")]
pub mod clip_space;
pub mod descriptor;
pub mod device;
pub mod dirty;
pub mod events;
pub mod gap;
#[cfg(feature = "gpu")]
pub mod gfx_backend;
pub mod glyph;
pub mod guest;
pub mod hw;
pub mod layout;
#[cfg(feature = "gpu")]
pub mod nv2a;
#[cfg(feature = "gpu")]
pub use gfx::pipeline_store;
pub use gfx::present_camera;
pub mod pushbuf;
pub mod regs;
pub mod residual;
pub mod resource;
pub mod runtime_glue;
#[cfg(feature = "gpu")]
pub mod scaled;
pub mod scene_sink;
pub mod scene_stage;
pub mod screen_rect;
pub mod shader;
pub mod state;
mod texdesc;
#[cfg(feature = "gpu")]
pub mod threaded;
pub mod vblank;
pub mod vertex_cache;

pub use backend::{
    DrawAdmission, DrawSnapshot, ExternalPlan, ExternalRecording, ExternalSlot, ExternalSurfaces, ExternalWrites, SharedReplacement, TargetCopy,
    Backend, ChannelMask, DepthClip, DrawOutcome, ExternalPass, ExternalPassRequest, VIEW_CONSTANTS, external_viewport, GuestLayoutKind, PixelRect, Recorded, RecordingBackend,
    RectClear, ResolvedDraw, SceneStage, StreamBytes, StreamData, CachedBytes, CachedIndices, IndexData, TargetDesc, Viewport,
};
pub use device::{
    Device, DeviceConfig, DrawScope, ExternalRefusal, NativeRefusal, GuestRange, PreparedExternal, SceneReservation, SuppliedState,
    ExternalStep, ExternalTargets, PreparedSequence,
    PresentParameters, Stats, Swallow, SwallowCall, SwallowOriginal, SwallowPass, SwallowRefusal,
    SwallowTarget, WritebackCause, WritebackCauses,
};
pub use dirty::{UploadPolicy, WriteTracker};
pub use scene_sink::SceneSink;
pub use events::{D3dError, Event, HleStop};
#[cfg(feature = "gpu")]
pub use gfx_backend::{GfxBackend, SwapFinish};
pub use guest::{GuestError, GuestMemory, SparseMemory};
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

pub use gfx::external::{Lighting, MAX_SCALE};
