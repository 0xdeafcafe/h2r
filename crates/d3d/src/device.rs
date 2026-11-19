//! The device model: host state for one `CDevice`, plus the D3D entry points
//! as plain Rust methods over [`GuestMemory`].
//!
//! Ownership split:
//!
//! * **Guest-owned, read at use:** render/texture-stage state arrays,
//!   resource headers and contents, the push-buffer words game code writes,
//!   `PSDEF`s and vertex attribute formats. The device reads them when a draw
//!   needs them, so guest code may change them behind its back.
//! * **Device-owned (host):** surfaces and textures on the backend, the
//!   swap ring, bindings set through the API (textures, streams, shaders,
//!   render target), vertex program memory and constants, the viewport.
//!   Where D3D mirrors these into guest-visible fields that game code reads
//!   (`D3D__pDevice`, device `+0x1A04` render target, the constant shadow,
//!   ...), the device writes the same fields.
//!
//! Every public entry first drains the push buffer
//! ([`Device::sync_push_buffer`]) so inline game-code submissions execute in
//! program order relative to API calls.
//!
//! **Ordering of inline draws and state.** On the Xbox, a state change
//! reaches the GPU only when something pushes it: a setter, the
//! `SetRenderState_Simple` helper, the deferred-state commit, or a draw. A
//! value game code merely stores into `D3D__RenderState` is invisible to
//! words already queued in the push buffer. The device models that with a
//! *pushed* snapshot of both arrays, refreshed exactly at those points;
//! queued inline draws are drained against the snapshot, not against the
//! live arrays. Inline state-mirror methods in the push buffer refresh the
//! snapshot when they execute.
//!
//! **Bounds.** All guest-controlled sizes and addresses go through checked
//! arithmetic, and allocations are bounded ([`MAX_VERTICES`],
//! [`MAX_SPAN`]) before anything is read.

use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::sync::Arc;

use gfx::texture::{DecodeOptions, TextureDesc, decode_texture};
use gfx::vertex::{
    PrimitiveType, StreamLayout, VertexAttribute, VertexDataType, VertexLayout, VertexRegister,
};
use gfx::xdk::RenderStateTable;
use gfx::{Clear, D3DFormat};

use crate::backend::{
    Backend, CachedBytes, CachedIndices, DrawOutcome, IndexData, ResolvedDraw, StreamBytes,
    StreamData, TargetInfo, TextureInfo, Viewport,
};
use crate::dirty::{UploadPolicy, WriteTracker, hash_bytes};
use crate::events::{D3dError, Event, HleStop};
use crate::guest::GuestMemory;
use crate::hw::{HwState, Memories};
use crate::layout::{GuestLayout, RENDER_STATE_SLOTS, device_field as field};
use crate::pushbuf::{self, Effect, Immediate, ImmediateDraw, PushError, SimpleEncode};
use crate::regs::{RegisterValues, RegisterWindow};
use crate::resource::{
    AddressMap, PaletteHeader, PixelContainer, ResourceHeader, ResourceType, SurfaceHeader, common,
};
use crate::shader::{CONSTANTS, Microcode, PROGRAM_SLOTS, ShaderInputs, VertexProcessing};
use crate::state::{DrawState, StateWords, Translator, slot};
use crate::vertex_cache::{IndexCache, Probe, VertexCache};

mod display;
mod fragments;
mod mirror;
mod raster;
mod snap;
mod visibility;

pub use display::{GAMMA_RAMP_BYTES, MINIPORT, VBLANK_SWAPDONE, VBlank};
use raster::FrameTexture;
pub use raster::{RttTarget, scissor_region};
pub use snap::{SECTION as SNAPSHOT_SECTION, SnapshotError};
pub use visibility::{
    D3DERR_TESTINCOMPLETE, VISIBILITY_INDICES, VISIBILITY_PENDING_SWAPS, VisibilityAnswer,
};

/// A range of guest virtual memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GuestRange {
    /// First byte.
    pub addr: u32,
    /// Length in bytes.
    pub len: u32,
}

/// Bytes kept free below the push-buffer end: the largest single inline
/// reservation seen (`DrawIndexedVertices` asks for `0x824` beyond put and
/// checks against threshold + `0x200`) with room to spare.
pub const PUSH_MARGIN: u32 = 0x4000;
/// Scratch bytes the device needs for headers it creates.
pub const SCRATCH_BYTES: u32 = 0x100;
/// Offset in the scratch range of the GPU completion word device `+0x30`
/// points at ([`Device::gpu_completed`]), after the back and depth headers.
pub const GPU_COMPLETED_OFFSET: u32 = 2 * SurfaceHeader::SIZE;
const _: () = assert!(GPU_COMPLETED_OFFSET + 4 <= SCRATCH_BYTES);
/// Offset in the scratch range of the display-surface header (device
/// `+0x1A18`) and, with two back buffers, the third frame-buffer header
/// (`+0x1A1C`), one [`SurfaceHeader::SIZE`] apart.
pub const FRAME_HEADERS_OFFSET: u32 = 0x40;
const _: () = assert!(FRAME_HEADERS_OFFSET >= GPU_COMPLETED_OFFSET + 4);
const _: () = assert!(FRAME_HEADERS_OFFSET + 2 * SurfaceHeader::SIZE <= SCRATCH_BYTES);
/// Device flag (`+0x8`) CreateDevice sets for a copy-swap device
/// (`0x3fc9b7`: multisample type bits `0x3000`, or `D3DSWAPEFFECT_COPY`).
/// Without it, `Swap` flips: it rotates the frame buffers' `Data` through
/// the headers at `+0x1A14..` (`0x3f9a70`).
pub const COPY_SWAP_FLAG: u32 = 0x4000;
/// Dirty bits the deferred-state commit leaves set (`0x401013`).
pub const COMMIT_KEEPS: u32 = 0xC000_0070;
/// Most vertices one draw may use (the NV2A index range is 2^20).
pub const MAX_VERTICES: u32 = 1 << 20;

/// Vertex programs [`Device::microcode`] keeps so a program the game
/// switches back to gets its earlier shared copy.
const MICROCODE_SEEN: usize = 64;
/// Most bytes the device reads from guest memory for one stream or index
/// list.
pub const MAX_SPAN: u64 = 64 << 20;
/// `D3DDevice_SetScissors` accepts at most this many rectangles.
pub const MAX_SCISSORS: u32 = 8;
/// Guest `Data` (a physical address) of the first frame buffer, which the
/// back-buffer header names when the device is created. Frame buffers live
/// on the host only; this address, above any Xbox RAM, names one. Each
/// frame buffer keeps its `Data` for its life: a flipping `Swap` moves the
/// `Data` values through the headers (`0x3f9a70`), not the pixels. At boot
/// (`0x14600`, from `0x127c1`) Halo builds `LIN_A8R8G8B8` texture headers
/// over the back and display surfaces' `Data` (kept at `0x509354` and
/// `0x509358`) and binds them by frame parity through its global texture
/// switch (`0x14fc1`, `0x14fe9`, `0x150a7`): a draw sampling such a texture
/// gets a copy of the frame buffer with that `Data`
/// ([`Backend::snapshot_surface`]), the image being drawn or the one last
/// presented. Guest memory at these addresses is never read or written.
pub const BACK_BUFFER_DATA: u32 = 0x0C00_0000;
/// Guest `Data` of the second frame buffer: the display surface (front
/// buffer) when the device is created. See [`BACK_BUFFER_DATA`].
pub const DISPLAY_BUFFER_DATA: u32 = 0x0E00_0000;
/// Guest `Data` of the third frame buffer (two back buffers).
pub const THIRD_BUFFER_DATA: u32 = 0x0F00_0000;
/// Guest `Data` of the depth/stencil header the device creates (see
/// [`BACK_BUFFER_DATA`]). Halo builds colour textures over it too
/// (`0x509368`, `0x50936C`): draws sample a depth/stencil readback decoded
/// in the texture's own format, including its channel signs.
pub const DEPTH_BUFFER_DATA: u32 = 0x0D00_0000;
/// Bytes from a frame buffer's `Data` ([`BACK_BUFFER_DATA`],
/// [`DISPLAY_BUFFER_DATA`], [`THIRD_BUFFER_DATA`]) or [`DEPTH_BUFFER_DATA`]
/// that belong to that buffer. Colour frame-buffer textures must start at
/// its base; depth aliases also support bounded offsets into the buffer.
pub const FRAME_DATA_SPAN: u32 = 0x0100_0000;
/// Host bound on `D3DDevice_Clear` rectangles (the original loops over any
/// count; this only keeps a guest count from spinning the host).
pub const MAX_CLEAR_RECTS: u32 = 4096;

/// Device configuration supplied by the integration layer.
#[derive(Clone, Copy, Debug)]
pub struct DeviceConfig {
    /// Where the D3D globals live.
    pub layout: GuestLayout,
    /// `Data` field to guest virtual address.
    pub address_map: AddressMap,
    /// Guest memory for the push buffer inline game code writes into.
    /// Must be mapped and larger than [`PUSH_MARGIN`].
    pub push_buffer: GuestRange,
    /// Guest memory for surface headers the device creates (back buffer,
    /// depth buffer); at least [`SCRATCH_BYTES`].
    pub scratch: GuestRange,
    /// Cache validation.
    pub upload: UploadPolicy,
    /// Base of the NV2A register window CreateDevice stores at device
    /// `+0x934`, where game code reads GPU registers after `KickOff`
    /// ([`crate::regs`]). The host picks it: page-aligned, with the two
    /// pages at `base + 0x3000` and `base + 0x400000` free for
    /// [`crate::runtime_glue::map_device_memory`] to map. `None` leaves
    /// `+0x934` alone (zero), so those reads fault.
    pub nv2a_window: Option<u32>,
}

impl DeviceConfig {
    /// Halo 2's layout with Xbox contiguous-memory addressing, tracked
    /// uploads, and the given push-buffer and scratch ranges.
    pub fn halo2(push_buffer: GuestRange, scratch: GuestRange) -> Self {
        Self {
            layout: GuestLayout::HALO2,
            address_map: AddressMap::XboxContiguous,
            push_buffer,
            scratch,
            upload: UploadPolicy::Tracked,
            nv2a_window: None,
        }
    }
}

/// `D3DPRESENT_PARAMETERS` (Xbox layout).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresentParameters {
    /// Back buffer width.
    pub width: u32,
    /// Back buffer height.
    pub height: u32,
    /// Back buffer `D3DFORMAT`.
    pub back_buffer_format: u32,
    /// Back buffers (0 means 1).
    pub back_buffer_count: u32,
    /// `D3DMULTISAMPLE_TYPE`.
    pub multisample_type: u32,
    /// `D3DSWAPEFFECT`.
    pub swap_effect: u32,
    /// Create a depth/stencil surface.
    pub enable_auto_depth_stencil: bool,
    /// Its format.
    pub auto_depth_stencil_format: u32,
    /// `D3DPRESENTFLAG_*`.
    pub flags: u32,
    /// Refresh rate.
    pub refresh_rate: u32,
    /// `D3DPRESENT_INTERVAL_*`.
    pub presentation_interval: u32,
    /// `BufferSurfaces[0]`: title-supplied back buffer (unsupported if set).
    pub buffer_surface: u32,
    /// `DepthStencilSurface`: title-supplied depth buffer (unsupported if set).
    pub depth_stencil_surface: u32,
}

impl PresentParameters {
    /// Size of the structure.
    pub const SIZE: u32 = 0x44;

    /// Reads the structure.
    pub fn read(mem: &dyn GuestMemory, addr: u32) -> Result<Self, D3dError> {
        let w = mem.read_words(addr, 17)?;
        Ok(Self {
            width: w[0],
            height: w[1],
            back_buffer_format: w[2],
            back_buffer_count: w[3],
            multisample_type: w[4],
            swap_effect: w[5],
            // w[6] hDeviceWindow, w[7] Windowed: ignored on the Xbox.
            enable_auto_depth_stencil: w[8] != 0,
            auto_depth_stencil_format: w[9],
            flags: w[10],
            refresh_rate: w[11],
            presentation_interval: w[12],
            buffer_surface: w[13],
            depth_stencil_surface: w[16],
        })
    }

    /// Writes the structure (tests).
    pub fn write(&self, mem: &mut dyn GuestMemory, addr: u32) -> Result<(), D3dError> {
        let w = [
            self.width,
            self.height,
            self.back_buffer_format,
            self.back_buffer_count,
            self.multisample_type,
            self.swap_effect,
            0,
            0,
            self.enable_auto_depth_stencil as u32,
            self.auto_depth_stencil_format,
            self.flags,
            self.refresh_rate,
            self.presentation_interval,
            self.buffer_surface,
            0,
            0,
            self.depth_stencil_surface,
        ];
        for (i, v) in w.into_iter().enumerate() {
            mem.write_u32(at(addr, 4 * i as u64)?, v)?;
        }
        Ok(())
    }
}

/// `SetStreamSource` / `D3DSTREAM_INPUT`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StreamSource {
    /// Vertex buffer header (0 = unbound).
    pub buffer: u32,
    /// Stride in bytes.
    pub stride: u32,
    /// Byte offset of vertex 0.
    pub offset: u32,
}

/// `D3DRECT`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    /// Left.
    pub x1: i32,
    /// Top.
    pub y1: i32,
    /// Right (exclusive).
    pub x2: i32,
    /// Bottom (exclusive).
    pub y2: i32,
}

/// `D3DCLEAR_*` flags.
pub mod clear_flags {
    /// Depth.
    pub const ZBUFFER: u32 = 0x01;
    /// Stencil.
    pub const STENCIL: u32 = 0x02;
    /// All colour channels (`TARGET_R|G|B|A`).
    pub const TARGET: u32 = 0xF0;
}

/// Counters, for tests and diagnostics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Occlusion depth grids recorded ([`Device::request_depth_grid`]).
    pub depth_grids: u64,
    /// Asks not recorded (another depth bound, not W-buffered, the
    /// backend cannot).
    pub depth_grids_skipped: u64,
    /// External passes recorded ([`Device::request_external_pass`]).
    pub external_passes: u64,
    /// External passes dropped (no colour or depth bound, the backend
    /// refused).
    pub external_passes_dropped: u64,
    /// Draws a [`Swallow`] dropped (or, observing, matched).
    pub swallowed_draws: u64,
    /// Swallows ended early: a draw that is not the next one planned, or
    /// one armed over another not finished ([`Device::swallow`]).
    pub swallow_mismatches: u64,
    /// Swallows not armed to drop because their pass could not be issued
    /// ([`Device::swallow`]): the draws stay the D3D path's.
    pub swallow_refusals: u64,
    /// External passes dropped, by [`ExternalRefusal`] (its index in
    /// [`ExternalRefusal::ALL`]).
    pub external_refusals: [u64; ExternalRefusal::ALL.len()],
    /// Draws handed to the backend (including skipped ones).
    pub draws: u64,
    /// Inline (push-buffer) immediate draws among them.
    pub inline_draws: u64,
    /// Inline-array (`INLINE_ARRAY`, `0x1818`) primitives among those.
    pub inline_array_draws: u64,
    /// Push-buffer words consumed.
    pub push_words: u64,
    /// `SetRenderState_Simple` calls (value already in the guest array).
    pub simple_state_calls: u64,
    /// Texture uploads.
    pub texture_uploads: u64,
    /// Texture cache hits.
    pub texture_hits: u64,
    /// NV2A register methods applied from the push buffer ([`crate::hw`]).
    pub hw_methods: u64,
    /// Clears that were not one whole-attachment clear (rectangles or
    /// channel masks), handed to [`Backend::clear_rects`].
    pub partial_clears: u64,
    /// Frame-buffer copies made for draws that sample a texture over one
    /// ([`BACK_BUFFER_DATA`], [`DISPLAY_BUFFER_DATA`]).
    pub frame_snapshots: u64,
    /// Of those, copies of the image last presented: a texture over the
    /// display surface's `Data` ([`DISPLAY_BUFFER_DATA`] at creation),
    /// Halo's previous-parity frame texture.
    pub previous_frame_snapshots: u64,
    /// Draws split into several pieces by scissor rectangles
    /// ([`ResolvedDraw::clip_rects`]).
    pub scissor_split_draws: u64,
    /// Render-to-texture write-backs into guest memory.
    pub rtt_writebacks: u64,
    /// Lazy levels left as targets whose guest bytes were not read again:
    /// the match they held was still those bytes (`H2_D3D_LEAVE_READ`).
    pub rtt_leave_reads_skipped: u64,
    /// Render-to-texture host surfaces filled from guest memory.
    pub rtt_uploads: u64,
    /// Depth-buffer readbacks for textures that alias its memory
    /// ([`DEPTH_BUFFER_DATA`]): one per change of the depth buffer that a
    /// draw then samples.
    pub depth_reads: u64,
    /// Host textures made from those readbacks (one per aliasing header,
    /// effective channel signs and depth-buffer change).
    pub depth_alias_uploads: u64,
    /// Texture stages that sampled a render-to-texture surface in place
    /// (lazy render-to-texture, [`Device::set_rtt_direct`]).
    pub rtt_direct_samples: u64,
    /// Vertical blanks counted ([`Device::vblank`]).
    pub vblanks: u64,
    /// Push-buffer callbacks handed out for delivery
    /// ([`Device::drain_until_callback`]).
    pub push_callbacks: u64,
    /// `EndVisibilityTest` calls.
    pub visibility_tests: u64,
    /// `GetVisibilityTestResult` calls answered `D3DERR_TESTINCOMPLETE`
    /// (the count was not back yet; see `docs/graphics/d3d.md`, "Visibility
    /// tests"). Depends on host timing.
    pub visibility_incomplete: u64,
    /// Resources destroyed ([`Device::destroy_resource`]).
    pub destroys: u64,
    /// Textures made on the backend from depth levels kept on the host
    /// ([`Backend::depth_level_texture`]): one per time a level is drawn
    /// into and then sampled.
    pub depth_level_textures: u64,
    /// [`rtt_writebacks`](Self::rtt_writebacks) by what asked for them.
    pub writeback_causes: WritebackCauses,
    /// Levels moved into the alias mirror instead of written back to guest
    /// memory because another level over the same memory was bound or
    /// sampled ([`Device::set_alias_mirror`]).
    pub mirror_stores: u64,
    /// Levels filled from the alias mirror on the backend.
    pub mirror_loads: u64,
    /// Mirror bytes flushed to guest memory (one read-back each).
    pub mirror_flushes: u64,
    /// Vertex-stream bytes copied out of guest memory for draws (vertex
    /// cache fills included).
    pub vertex_bytes_copied: u64,
    /// Vertex streams drawn from a vertex cache snapshot, without a copy.
    pub vertex_cache_hits: u64,
    /// Bytes those streams held.
    pub vertex_bytes_reused: u64,
    /// Vertex cache snapshots taken.
    pub vertex_cache_fills: u64,
    /// Indexed draws whose index list came from the index cache (already
    /// scanned and rebased).
    pub index_cache_hits: u64,
}

/// What made the device write a render-to-texture level back to guest
/// memory ([`Stats::writeback_causes`]; `H2_D3D_WRITEBACK_TRACE=1` prints
/// each one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WritebackCause {
    /// Another level over the same memory is bound or sampled.
    Overlap,
    /// The level's header was rewritten with another identity.
    Rewritten,
    /// Lazily, the guest stored into the level since it was drawn.
    GuestStores,
    /// A draw samples the level's memory and cannot read the surface in place.
    Sample,
    /// The depth/stencil level stopped being bound (or the colour target changed).
    DepthBinding,
    /// Eagerly, the colour target changed.
    LeaveTarget,
    /// Eagerly, `Swap` or `KickOff`.
    Sync,
    /// `BlockOnResource` on the level or memory it overlaps.
    BlockOnResource,
    /// The busy check `0x3fb710`.
    Busy,
    /// `BlockOnTime`.
    BlockOnTime,
    /// A push-buffer callback.
    Callback,
    /// The level or its texture was destroyed.
    Destroy,
    /// `PersistDisplay`.
    PersistDisplay,
    /// Device teardown.
    Teardown,
    /// A host caller ([`Device::flush_render_targets`]: tests, reports).
    Host,
}

/// Write-backs per [`WritebackCause`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WritebackCauses {
    pub overlap: u64,
    pub rewritten: u64,
    pub guest_stores: u64,
    pub sample: u64,
    pub depth_binding: u64,
    pub leave_target: u64,
    pub sync: u64,
    pub block_on_resource: u64,
    pub busy: u64,
    pub block_on_time: u64,
    pub callback: u64,
    pub destroy: u64,
    pub persist_display: u64,
    pub teardown: u64,
    pub host: u64,
}

impl WritebackCauses {
    pub(crate) fn count(&mut self, cause: WritebackCause) {
        let n = match cause {
            WritebackCause::Overlap => &mut self.overlap,
            WritebackCause::Rewritten => &mut self.rewritten,
            WritebackCause::GuestStores => &mut self.guest_stores,
            WritebackCause::Sample => &mut self.sample,
            WritebackCause::DepthBinding => &mut self.depth_binding,
            WritebackCause::LeaveTarget => &mut self.leave_target,
            WritebackCause::Sync => &mut self.sync,
            WritebackCause::BlockOnResource => &mut self.block_on_resource,
            WritebackCause::Busy => &mut self.busy,
            WritebackCause::BlockOnTime => &mut self.block_on_time,
            WritebackCause::Callback => &mut self.callback,
            WritebackCause::Destroy => &mut self.destroy,
            WritebackCause::PersistDisplay => &mut self.persist_display,
            WritebackCause::Teardown => &mut self.teardown,
            WritebackCause::Host => &mut self.host,
        };
        *n += 1;
    }
}

/// `H2_D3D_WRITEBACK_TRACE=1`: print every render-to-texture write-back.
pub(crate) fn writeback_trace() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("H2_D3D_WRITEBACK_TRACE").is_some_and(|v| v == "1"))
}

/// A guest callback the push buffer asks for: `NV097_NO_OPERATION` with
/// software method 6 or 7 (`D3DDevice_InsertCallback`, inlined at
/// `0x2460e`). On the Xbox the GPU raises an interrupt when it reaches the
/// method and D3D's handler (`0x3ff3c5`) calls `function(context)`, cdecl,
/// with the two operands it loaded into `SET_ZSTENCIL_CLEAR_VALUE` and
/// `SET_COLOR_CLEAR_VALUE`. Everything pushed before it has executed by
/// then, and nothing after it has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PushCallback {
    /// Guest function (`0x1D8C`).
    pub function: u32,
    /// Its one argument (`0x1D90`).
    pub context: u32,
    /// Software method: 6 (`D3DCALLBACK_READ`) or 7 (`D3DCALLBACK_WRITE`).
    pub kind: u32,
    /// Address of the `NO_OPERATION` header.
    pub at: u32,
}

/// Cache key: texture header, for P8 textures the palette header (one
/// header bound with two palettes is two host textures), and the
/// `D3DTSS_COLORSIGN` bits (signed channels are stored biased, see
/// [`DecodeOptions::signed`]).
type TextureKey = (u32, u32, u32);

/// The `D3DTSIGN_*` bits of `D3DTSS_COLORSIGN` (28..31).
const COLOR_SIGN_MASK: u32 = 0xF000_0000;

/// Palette part of a cached texture's identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PaletteIdentity {
    header: u32,
    /// `Common` (carries the size bits).
    common: u32,
    /// Data address (virtual).
    va: u32,
    /// Bytes.
    len: u32,
}

struct CachedTexture<T> {
    identity: [u32; 3],
    palette: Option<PaletteIdentity>,
    va: u32,
    len: u32,
    generation: u64,
    hash: u64,
    info: TextureInfo,
    host: T,
}

struct Swap<S> {
    /// `[back0, back1, ..]`: back0 is what the back-buffer header means now.
    back: Vec<S>,
    front: S,
    /// The surface holding the image on screen (last presented): the front
    /// after a flip; after a `COPY` swap whose image is scanned out at once
    /// back0, until something writes to it (see `copy_pending`); with
    /// three frame buffers, whatever the display header named at the last
    /// `FINISH`.
    shown: S,
    /// A `COPY` swap's copy of back0 into the front is still to be made
    /// (it is deferred to the first write into back0,
    /// [`Device::before_target_write`]): the display header's image is
    /// back0's contents.
    copy_pending: bool,
    /// Host format and size of the ring's surfaces.
    format: D3DFormat,
    size: (u32, u32),
    /// Every frame buffer as (`Data`, surface). A buffer's `Data` is fixed
    /// for its life; which header names it changes when `Swap` flips.
    buffers: Vec<(u32, S)>,
    /// `Swap` flips (device flag [`COPY_SWAP_FLAG`] clear), whatever its
    /// `COPY` bit; otherwise it copies and the ring never moves.
    flip: bool,
}

impl<S: Copy> Swap<S> {
    /// The surfaces the frame-buffer headers name, in header order
    /// (`+0x1A14` back, `+0x1A18` display, `+0x1A1C` the second back
    /// buffer).
    fn header_order(&self) -> Vec<S> {
        let mut v = vec![self.back[0], self.front];
        v.extend(self.back.iter().skip(1).copied());
        v
    }
}

/// `base + offset` as a guest address, or an error if it leaves the 32-bit
/// address space.
/// Guest dwords to write together: reads see the staged values first, and
/// nothing reaches guest memory until [`Staged::commit`]. Reference
/// transitions build one so a stop part-way changes nothing.
#[derive(Default)]
struct Staged {
    words: Vec<(u32, u32)>,
    /// Resources left with no reference: destroyed when the staged writes
    /// commit, in this order ([`Device::destroy_resource`]).
    destroys: Vec<u32>,
}

impl Staged {
    fn read(&self, mem: &dyn GuestMemory, addr: u32) -> Result<u32, D3dError> {
        match self.words.iter().find(|w| w.0 == addr) {
            Some(w) => Ok(w.1),
            None => Ok(mem.read_u32(addr)?),
        }
    }

    fn write(&mut self, addr: u32, value: u32) {
        match self.words.iter_mut().find(|w| w.0 == addr) {
            Some(w) => w.1 = value,
            None => self.words.push((addr, value)),
        }
    }

    fn destroy(&mut self, header: u32) {
        if !self.destroys.contains(&header) {
            self.destroys.push(header);
        }
    }

    /// Writes the words; returns the resources to destroy.
    #[must_use = "the destroys have to reach Device::destroy_resource"]
    fn commit(self, mem: &mut dyn GuestMemory) -> Result<Vec<u32>, D3dError> {
        for (addr, value) in self.words {
            mem.write_u32(addr, value)?;
        }
        Ok(self.destroys)
    }
}

fn at(base: u32, offset: u64) -> Result<u32, D3dError> {
    u32::try_from(base as u64 + offset).map_err(|_| D3dError::Invalid {
        entry: "guest address",
        detail: format!("{base:#010x} + {offset:#x} overflows the address space"),
    })
}

/// `count * size` bytes, bounded by [`MAX_SPAN`].
fn span(entry: &'static str, count: u32, size: u32) -> Result<usize, D3dError> {
    let n = count as u64 * size as u64;
    if n > MAX_SPAN {
        return Err(D3dError::invalid(
            entry,
            format!("{count} x {size} bytes exceeds the {MAX_SPAN:#x}-byte limit"),
        ));
    }
    Ok(n as usize)
}

fn vertex_limit(entry: &'static str, count: u32) -> Result<(), D3dError> {
    if count > MAX_VERTICES {
        return Err(D3dError::invalid(
            entry,
            format!("{count} vertices (limit {MAX_VERTICES})"),
        ));
    }
    Ok(())
}

/// What `Device::vertex_setup` derived from a vertex program's attribute
/// format words (`+0x14`, 64 words): the streams and attributes they read.
/// The strides are filled in from the bound streams at each draw.
struct VertexMemo {
    vaf: [u32; 64],
    streams: Vec<StreamLayout>,
}

/// The W-buffer's parameters when a depth grid was asked for: the stored
/// depth is `floor(view_depth · inv_w_far · z_scale) / z_max`, `z_max` the
/// depth format's full scale (`z_scale` for `D24S8`), so the view depth of
/// a raw grid value `d` is at most `(d · z_max + 1) / (inv_w_far · z_scale)`
/// (docs/graphics/shaders.md, "W-buffering"; `0x3f6840` scales the
/// viewport matrix by `+0x944 · +0x948`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DepthScale {
    pub inv_w_far: f32,
    pub z_scale: f32,
}

/// Where [`Device::request_depth_grid`]'s grids go, with the W-buffer's
/// parameters at the time.
pub type OcclusionSink =
    std::sync::Arc<dyn Fn(crate::backend::DepthGridResult, DepthScale) + Send + Sync>;

/// A pass whose draws the host draws itself ([`Device::swallow`]): the
/// game's own code runs, so it leaves every state, and only its draws are
/// dropped.
pub struct Swallow {
    /// Per draw, in order, the slot of the colour target it draws into:
    /// 0 the target bound when armed; another slot the target its first
    /// draw finds, the same for each of its draws and distinct from every
    /// other slot's.
    pub targets: Vec<u8>,
    /// Every draw's primitive and vertex count.
    pub primitive: PrimitiveType,
    pub vertices: usize,
    /// Drop the draws; `false` draws them and only places `pass` (a
    /// capture of what the game drew).
    pub drop: bool,
    /// Issued after the last draw, into the targets bound then.
    pub pass: Box<dyn crate::backend::ExternalPass>,
}

struct Swallowing {
    targets: Vec<u8>,
    next: usize,
    headers: Vec<Option<u32>>,
    primitive: PrimitiveType,
    vertices: usize,
    drop: bool,
    /// `Stats::draws` when the next planned draw comes: no other draw in
    /// between.
    draws: u64,
    pass: Option<Box<dyn crate::backend::ExternalPass>>,
}

/// What becomes of an inline draw under a swallow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fate {
    /// Not the pass's (or nothing armed): drawn.
    Drawn,
    /// The pass's, dropped.
    Dropped,
    /// The pass's, drawn (observing).
    Observed,
}

/// Why an external pass is not issued ([`Stats::external_refusals`]);
/// each is logged once per process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalRefusal {
    /// A newer ask replaced it before the drain.
    Replaced,
    /// The backend draws no external passes (no GPU backend: a headless
    /// run without `H2_BOOT_GFX`).
    Unsupported,
    /// No colour target bound.
    NoColour,
    /// No depth bound, and the pass is not colour-only.
    NoDepth,
    /// The colour target's header, or the device's Z scale, unreadable.
    Header,
    /// The bound targets have no host surfaces.
    Targets,
    /// The backend refused it (a target it cannot draw into).
    Backend,
    /// The backend failed.
    Failed,
}

impl ExternalRefusal {
    pub const ALL: [ExternalRefusal; 8] = [
        ExternalRefusal::Replaced,
        ExternalRefusal::Unsupported,
        ExternalRefusal::NoColour,
        ExternalRefusal::NoDepth,
        ExternalRefusal::Header,
        ExternalRefusal::Targets,
        ExternalRefusal::Backend,
        ExternalRefusal::Failed,
    ];

    /// Its index in [`ALL`](Self::ALL) and [`Stats::external_refusals`].
    pub fn index(self) -> usize {
        ExternalRefusal::ALL.iter().position(|&r| r == self).unwrap_or(0)
    }

    pub fn describe(self) -> &'static str {
        match self {
            ExternalRefusal::Replaced => "a newer pass replaced it before the drain",
            ExternalRefusal::Unsupported => {
                "the backend draws no external passes (no GPU backend: headless without H2_BOOT_GFX)"
            }
            ExternalRefusal::NoColour => "no colour target is bound",
            ExternalRefusal::NoDepth => "no depth is bound and the pass needs one",
            ExternalRefusal::Header => "the colour target's header or the device's Z scale is unreadable",
            ExternalRefusal::Targets => "the bound targets have no host surfaces",
            ExternalRefusal::Backend => "the backend refused it (a target it cannot draw into)",
            ExternalRefusal::Failed => "the backend failed",
        }
    }
}

/// The refusals logged so far (a bit per [`ExternalRefusal::index`]).
static EXTERNAL_REFUSALS_LOGGED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

struct GridAsk {
    tag: u64,
    width: u32,
    height: u32,
    sink: OcclusionSink,
}

/// One Direct3D device.
pub struct Device<B: Backend> {
    cfg: DeviceConfig,
    /// The slot types of the build's render-state table, resolved once.
    translator: Translator,
    /// The last vertex program's attribute format words and the stream
    /// layout they give (`vertex_setup`).
    vertex_memo: Option<VertexMemo>,
    /// The last state translated, part by part: a part is reused while its
    /// guest words are unchanged.
    translated: crate::state::TranslateMemo,
    /// The host backend.
    pub backend: B,
    created: bool,
    params: Option<PresentParameters>,
    swap: Option<Swap<B::Surface>>,
    depth: Option<B::Surface>,
    back_header: u32,
    /// Frame-buffer headers after the back buffer's: the display surface
    /// (`+0x1A18`), then `+0x1A1C` with two back buffers.
    frame_headers: Vec<u32>,
    depth_header: u32,
    other_surfaces: HashMap<u32, ([u32; 3], B::Surface)>,
    render_target: u32,
    depth_stencil: u32,
    textures: [u32; 4],
    palettes: [u32; 4],
    vertex_shader: u32,
    pixel_shader: u32,
    streams: [StreamSource; 16],
    stream_inputs: Option<Vec<StreamSource>>,
    /// Stream buffers the backend is done with, for the next draws to fill,
    /// by capacity, smallest first.
    stream_pool: Vec<Vec<u8>>,
    stream_pool_bytes: usize,
    stream_scratch: Vec<StreamBytes>,
    /// Snapshots of vertex windows the guest has not written since
    /// (`docs/graphics/d3d.md`, "Vertex cache"); `None` with `H2_VERTEX_CACHE=0`
    /// ([`Device::set_vertex_cache`]).
    vertex_cache: Option<VertexCache>,
    /// Index lists the same way (on and off with `vertex_cache`).
    index_cache: Option<IndexCache>,
    /// Data words of the push-buffer command being executed, reused.
    push_words: Vec<u32>,
    program: Box<[u32; PROGRAM_SLOTS * 4]>,
    program_start: u32,
    /// The last microcode handed to a draw ([`Device::microcode`]).
    microcode_memo: Option<Microcode>,
    /// The programs draws used most recently, oldest first, at most
    /// [`MICROCODE_SEEN`] ([`Device::microcode`]).
    microcode_seen: Vec<Microcode>,
    /// Draws share what they read the same as before: the pushed state
    /// snapshot ([`Device::draw_state`]) and earlier microcode copies
    /// ([`Device::microcode`]). Only sharing changes, never a value. On
    /// unless `H2_D3D_DRAW_SHARING=0` ([`Device::set_draw_sharing`]).
    draw_sharing: bool,
    /// Constant registers as dwords (register r is `[4r, 4r + 4)`).
    constants: Box<[u32; CONSTANTS * 4]>,
    /// Bumped by every write to [`Self::constants`].
    constants_generation: u64,
    /// The block the last draw took and [`Self::constants_generation`]
    /// then: the next draws share it until a register changes.
    draw_constants: Option<(u64, crate::shader::Constants)>,
    /// The last draw's PSDEF and combiner words, shared with the next
    /// draws while they read the same.
    draw_pixel_shader: Option<Arc<[u32; 60]>>,
    draw_combiner: Option<Arc<[u32; 57]>>,
    viewport: Viewport,
    transforms: HashMap<u32, [f32; 16]>,
    scissors: Vec<Rect>,
    /// `SetScissors`' `Exclusive`: draw outside the rectangles.
    scissor_exclusive: bool,
    immediate: Immediate,
    consumed: u32,
    /// State as the GPU has seen it (see the module docs).
    pushed: Option<Arc<StateWords>>,
    /// A latched push-buffer failure.
    fault: Option<String>,
    swap_count: u32,
    fence: u32,
    /// The push-buffer position the last fence was issued at.
    fence_at: u32,
    /// The NV2A register window ([`DeviceConfig::nv2a_window`]).
    regs: Option<RegisterWindow>,
    cache: HashMap<TextureKey, CachedTexture<B::Texture>>,
    /// Guest-write tracking (see [`crate::dirty`]).
    pub tracker: WriteTracker,
    events: Vec<Event>,
    last_stop: Option<HleStop>,
    stats: Stats,
    /// `D3DSIMPLERENDERSTATEENCODE`, read from the image at creation.
    encode: Option<SimpleEncode>,
    /// NV2A register state the push buffer has set ([`crate::hw`]).
    hw: HwState,
    /// An occlusion depth grid asked for ([`Device::request_depth_grid`]),
    /// issued after the next drain.
    pending_grid: Option<GridAsk>,
    /// An external pass asked for ([`Device::request_external_pass`]),
    /// issued after the next drain.
    pending_external: Option<Box<dyn crate::backend::ExternalPass>>,
    /// The draws of a pass being swallowed ([`Device::swallow`]).
    swallowing: Option<Swallowing>,
    /// Display state: flicker filter, persisted surface, vertical blank.
    display: display::Display<B::Surface>,
    /// Render-to-texture targets, by surface header ([`RttTarget`]).
    rtt: HashMap<u32, RttTarget<B::Surface>>,
    /// Bumped whenever [`Self::rtt`] gains or loses a target or a target
    /// becomes dirty: what `flush_for_texture` found for a texture holds
    /// until then.
    rtt_epoch: u64,
    /// `flush_for_texture`'s last answer per texture: its header words,
    /// [`Self::rtt_epoch`] then, and the guest range to flush the mirror
    /// over. Nothing to write back while both still match.
    texture_flushed: rustc_hash::FxHashMap<u32, ([u32; 5], u64, u64, u64)>,
    /// Bumped by every draw or clear that may write the device's own depth
    /// buffer (`raster.rs`, textures over [`DEPTH_BUFFER_DATA`]).
    depth_generation: u64,
    /// A colour surface over the depth buffer's memory (`raster.rs`,
    /// [`raster::DepthColor`]).
    depth_color: Option<raster::DepthColor>,
    /// That colour surface was drawn into after the depth buffer last was:
    /// the memory both name holds its pixels, and textures over the depth
    /// buffer read them.
    depth_color_newer: bool,
    /// The depth buffer as guest bytes, and the generation they were read
    /// at.
    depth_copy: Option<(u64, Vec<u8>)>,
    /// With a backend that makes depth-alias textures itself
    /// ([`Backend::defers_depth_aliases`]): whether each description
    /// decodes under a channel-sign mask, and the generation last counted
    /// as a read.
    depth_alias_decodes: Vec<(gfx::texture::TextureDesc, u32, Result<(), String>)>,
    depth_read_generation: Option<u64>,
    /// Textures over the depth buffer, made from `depth_copy`, by header
    /// and effective channel signs.
    depth_alias: HashMap<raster::DepthAliasKey, raster::DepthAlias<B::Texture>>,
    /// Lazy depth levels: the texture each dirty depth level was last
    /// sampled through ([`Backend::depth_level_texture`]), until it is drawn
    /// into, refilled or dropped.
    depth_level_tex: HashMap<u32, B::Texture>,
    /// Depth levels stay on the host like colour levels (lazy
    /// render-to-texture and a backend that samples depth surfaces);
    /// `H2_D3D_LAZY_DEPTH=0` turns it off.
    lazy_depth: bool,
    /// A level left as a target is read from guest memory every time, even
    /// when its match is already those bytes (`H2_D3D_LEAVE_READ=1`;
    /// `Device::set_leave_read`).
    leave_read: bool,
    /// Each target's level bytes when its host surface and guest memory
    /// last matched (filled from, or written back into, memory).
    rtt_synced: HashMap<u32, Vec<u8>>,
    /// Exact guest stores into each lazily held level since that match,
    /// as (offset, length) in the level ([`Device::note_write`]).
    rtt_stores: HashMap<u32, Vec<(u32, u32)>>,
    /// Overlapping levels moved through the backend's alias mirror
    /// ([`Device::set_alias_mirror`]; `H2_D3D_ALIAS_MIRROR=0` turns it off).
    alias_mirror: bool,
    /// Depth levels filled from the alias mirror on the GPU
    /// ([`Device::set_alias_mirror_depth`]; `H2_D3D_MIRROR_DEPTH=0` turns
    /// it off).
    mirror_depth: bool,
    /// What the mirror holds newer than guest memory.
    mirror: Option<mirror::AliasMirror>,
    /// Clean levels whose memory the mirror took newer bytes over since
    /// they were filled: refilled when next used.
    mirror_stale: HashSet<u32>,
    /// Lazy render-to-texture is wanted ([`Device::set_rtt_direct`]).
    rtt_direct: bool,
    /// The push-buffer callback being delivered, between
    /// [`Device::begin_push_callback`] and [`Device::end_push_callback`].
    active_callback: Option<PushCallback>,
    /// A callback the drain reached whose write-back failed: handed out by
    /// the next [`Device::drain_until_callback`] once the write-back works.
    undelivered: Option<PushCallback>,
    /// Ended visibility tests, by index (the original's report slots).
    visibility: HashMap<u32, visibility::VisibilityTest>,
    /// `GetVisibilityTestResult` answers `D3DERR_TESTINCOMPLETE` for a
    /// recent test whose count is not back instead of waiting for it
    /// ([`Device::set_visibility_poll`]; `H2_D3D_VISIBILITY_POLL=0` turns
    /// it off).
    visibility_poll: bool,
    /// Draws note which stages sample the game's glyph cache
    /// ([`Device::set_glyph_smoothing`]; `H2_SMOOTH_TEXT=1` turns it on; off by default).
    glyph_smoothing: bool,
    /// Destroyed resources the original `D3D_DestroyResource` has yet to
    /// free ([`Device::take_pending_destroys`]).
    pending_destroys: Vec<u32>,
    /// Resources the last teardown destroyed, whose original
    /// `D3D_DestroyResource` may still wait on them after the device is
    /// gone ([`Device::block_on_resource`]).
    torn_down: HashSet<u32>,
}

fn d3dcolor_to_rgba(c: u32) -> [f32; 4] {
    let b = c.to_le_bytes(); // B, G, R, A
    [
        b[2] as f32 / 255.0,
        b[1] as f32 / 255.0,
        b[0] as f32 / 255.0,
        b[3] as f32 / 255.0,
    ]
}

/// What a resolved draw needs before the backend sees it.
struct DrawParts {
    entry: &'static str,
    primitive: PrimitiveType,
    vertex_count: u32,
    layout: VertexLayout,
    vertex: VertexProcessing,
    streams: Vec<StreamBytes>,
    /// Indices into `streams` (indexed draws).
    indices: Option<IndexData>,
    words: Arc<StateWords>,
}

impl<B: Backend> Device<B> {
    /// A device that has not been created yet (`Direct3D_CreateDevice` does that).
    pub fn new(cfg: DeviceConfig, backend: B) -> Self {
        Self {
            cfg,
            translator: Translator::new(&RenderStateTable::default()),
            translated: Default::default(),
            vertex_memo: None,
            backend,
            created: false,
            params: None,
            swap: None,
            depth: None,
            back_header: 0,
            frame_headers: Vec::new(),
            depth_header: 0,
            other_surfaces: HashMap::default(),
            render_target: 0,
            depth_stencil: 0,
            textures: [0; 4],
            palettes: [0; 4],
            vertex_shader: 0,
            pixel_shader: 0,
            streams: [StreamSource::default(); 16],
            stream_inputs: None,
            stream_pool: Vec::new(),
            stream_pool_bytes: 0,
            stream_scratch: Vec::new(),
            vertex_cache: vertex_cache_on()
                .then(|| VertexCache::new(VertexCache::budget_from_env())),
            index_cache: vertex_cache_on()
                .then(|| IndexCache::new(VertexCache::budget_from_env() / 4)),
            push_words: Vec::new(),
            program: Box::new([0; PROGRAM_SLOTS * 4]),
            program_start: 0,
            microcode_memo: None,
            microcode_seen: Vec::new(),
            draw_sharing: std::env::var_os("H2_D3D_DRAW_SHARING").is_none_or(|v| v != "0"),
            constants: Box::new([0; CONSTANTS * 4]),
            constants_generation: 0,
            draw_constants: None,
            draw_pixel_shader: None,
            draw_combiner: None,
            viewport: Viewport {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
                min_z: 0.0,
                max_z: 1.0,
            },
            transforms: HashMap::default(),
            scissors: Vec::new(),
            scissor_exclusive: false,
            immediate: Immediate::default(),
            consumed: cfg.push_buffer.addr,
            pushed: None,
            fault: None,
            swap_count: 0,
            fence: 0,
            fence_at: cfg.push_buffer.addr,
            regs: None,
            cache: HashMap::default(),
            tracker: WriteTracker::new(),
            events: Vec::new(),
            last_stop: None,
            stats: Stats::default(),
            encode: None,
            hw: HwState::default(),
            pending_grid: None,
            pending_external: None,
            swallowing: None,
            display: display::Display::default(),
            rtt: HashMap::default(),
            rtt_epoch: 0,
            texture_flushed: rustc_hash::FxHashMap::default(),
            depth_generation: 0,
            depth_color: None,
            depth_color_newer: false,
            depth_copy: None,
            depth_alias_decodes: Vec::new(),
            depth_read_generation: None,
            depth_alias: HashMap::default(),
            depth_level_tex: HashMap::default(),
            lazy_depth: std::env::var_os("H2_D3D_LAZY_DEPTH").is_none_or(|v| v != "0"),
            leave_read: std::env::var_os("H2_D3D_LEAVE_READ").is_some_and(|v| v == "1"),
            rtt_synced: HashMap::default(),
            rtt_stores: HashMap::default(),
            alias_mirror: std::env::var_os("H2_D3D_ALIAS_MIRROR").is_none_or(|v| v != "0"),
            mirror_depth: std::env::var_os("H2_D3D_MIRROR_DEPTH").is_none_or(|v| v != "0"),
            mirror: None,
            mirror_stale: HashSet::default(),
            rtt_direct: true,
            active_callback: None,
            undelivered: None,
            visibility: HashMap::default(),
            visibility_poll: std::env::var_os("H2_D3D_VISIBILITY_POLL").is_none_or(|v| v != "0"),
            glyph_smoothing: crate::glyph::smoothing_from_env(),
            pending_destroys: Vec::new(),
            torn_down: HashSet::default(),
        }
    }

    /// Configuration.
    pub fn config(&self) -> &DeviceConfig {
        &self.cfg
    }

    /// Counters.
    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// Drains recorded events.
    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    /// Records the call that stopped the guest (the runtime glue does this).
    pub fn note_stop(&mut self, stop: HleStop) {
        self.last_stop = Some(stop);
    }

    /// The last recorded stop.
    pub fn last_stop(&self) -> Option<&HleStop> {
        self.last_stop.as_ref()
    }

    /// Present parameters of the created device.
    pub fn present_parameters(&self) -> Option<&PresentParameters> {
        self.params.as_ref()
    }

    /// The surface the back-buffer header currently stands for.
    pub fn back_buffer(&self) -> Option<B::Surface> {
        self.swap.as_ref().map(|s| s.back[0])
    }

    /// The front buffer (what the last `Swap` showed, after the flip).
    pub fn front_buffer(&self) -> Option<B::Surface> {
        self.swap.as_ref().map(|s| s.front)
    }

    /// Constant register `r` (hardware order, index = register + 96).
    pub fn constant(&self, r: usize) -> [f32; 4] {
        std::array::from_fn(|c| f32::from_bits(self.constants[4 * r + c]))
    }

    /// Every constant register for a draw: the last draw's block while no
    /// register has changed since, else one filled in place in the heap (a
    /// 3 KiB array built on the stack and then moved was a second copy), a
    /// spare block of a draw already run when there is one
    /// ([`crate::shader::spare_constants`]); every register is overwritten.
    fn draw_constants(&mut self) -> crate::shader::Constants {
        if let Some((generation, c)) = &self.draw_constants
            && *generation == self.constants_generation
        {
            return c.clone();
        }
        let mut c = crate::shader::spare_constants();
        let b = Arc::get_mut(&mut c).expect("a spare block is unshared");
        for (reg, words) in b.iter_mut().zip(self.constants.as_chunks::<4>().0) {
            *reg = words.map(f32::from_bits);
        }
        self.draw_constants = Some((self.constants_generation, c.clone()));
        c
    }

    /// `words` shared: the block in `last` when it holds the same words,
    /// else a new one, kept in `last` for the next draw.
    fn shared_words<const N: usize>(
        last: &mut Option<Arc<[u32; N]>>,
        words: &[u32; N],
    ) -> Arc<[u32; N]> {
        match last {
            Some(a) if **a == *words => a.clone(),
            _ => last.insert(Arc::new(*words)).clone(),
        }
    }

    fn live(&self, entry: &'static str) -> Result<(), D3dError> {
        if self.created {
            Ok(())
        } else {
            Err(D3dError::NoDevice(entry))
        }
    }

    fn layout(&self) -> GuestLayout {
        self.cfg.layout
    }

    /// [`SimpleEncode::mirror_slot`] of the table read at creation.
    fn mirror_slot(&self, m: u32) -> Option<u32> {
        self.encode.as_ref().and_then(|e| e.mirror_slot(m))
    }

    // ---------------------------------------------------------------------
    // Device lifetime
    // ---------------------------------------------------------------------

    /// `Direct3D_CreateDevice` (`0x3f5240`). `behavior_flags` is the LTCG
    /// `EAX`, `params` the stack `pPresentationParameters`, and `pp_device`
    /// the `ECX` out pointer (0 allowed). Returns the `HRESULT` (0).
    ///
    /// Guest-visible effects match the original: `*pp_device` and
    /// `D3D__pDevice` point at the static device object; device flags get
    /// `behavior & 0x10`; the push-buffer put/threshold fields are set up;
    /// both state arrays get their defaults; render-target, depth and back
    /// buffer fields point at surface headers the device creates in scratch
    /// memory.
    pub fn create_device(
        &mut self,
        mem: &mut dyn GuestMemory,
        behavior_flags: u32,
        params: u32,
        pp_device: u32,
    ) -> Result<u32, D3dError> {
        const E: &str = "Direct3D_CreateDevice";
        if self.created {
            return Err(D3dError::unimplemented(
                E,
                "a second CreateDevice without releasing the first (Halo resets with D3DDevice_Reset instead)",
            ));
        }
        let p = PresentParameters::read(mem, params)?;
        if p.buffer_surface != 0 || p.depth_stencil_surface != 0 {
            return Err(D3dError::unimplemented(
                E,
                "title-supplied BufferSurfaces/DepthStencilSurface",
            ));
        }
        if p.width == 0 || p.height == 0 || p.width > 4096 || p.height > 4096 {
            return Err(D3dError::invalid(
                E,
                format!("back buffer size {}x{}", p.width, p.height),
            ));
        }
        if p.back_buffer_count > 2 {
            return Err(D3dError::invalid(
                E,
                format!("{} back buffers", p.back_buffer_count),
            ));
        }
        let pb = self.cfg.push_buffer;
        if pb.len <= PUSH_MARGIN * 2
            || self.cfg.scratch.len < SCRATCH_BYTES
            || pb.addr as u64 + pb.len as u64 > 1 << 32
        {
            return Err(D3dError::invalid(
                E,
                "push buffer or scratch range invalid in DeviceConfig",
            ));
        }
        let l = self.layout();
        // The method-to-slot map comes from the image, not from this crate.
        let encode = SimpleEncode::read(mem, l.simple_encode)
            .map_err(|e| D3dError::unimplemented(E, e.to_string()))?;
        self.encode = Some(encode);
        // Back buffers are always created linear: the guest header records
        // the title's format, the host surface uses its linear twin.
        let fmt = D3DFormat::from_raw(p.back_buffer_format).ok_or_else(|| {
            D3dError::unimplemented(E, format!("back buffer format {:#x}", p.back_buffer_format))
        })?;
        let host_fmt = linear_twin(fmt);
        let count = p.back_buffer_count.max(1);
        let mut back = Vec::new();
        for _ in 0..count {
            back.push(
                self.backend
                    .create_surface(host_fmt, p.width, p.height, false)
                    .map_err(D3dError::Backend)?,
            );
        }
        let front = self
            .backend
            .create_surface(host_fmt, p.width, p.height, false)
            .map_err(D3dError::Backend)?;
        // `0x3fc9a2..0x3fc9c0`: multisampling or D3DSWAPEFFECT_COPY makes a
        // copy-swap device; anything else flips.
        let copy_swap = p.multisample_type & 0x3000 != 0 || p.swap_effect == 3;
        // Frame buffers in header order (back, display, second back), each
        // with a `Data` of its own.
        let mut buffers = vec![(BACK_BUFFER_DATA, back[0]), (DISPLAY_BUFFER_DATA, front)];
        buffers.extend(back.get(1).map(|&s| (THIRD_BUFFER_DATA, s)));
        self.swap = Some(Swap {
            back,
            front,
            shown: front,
            copy_pending: false,
            format: host_fmt,
            size: (p.width, p.height),
            buffers: buffers.clone(),
            flip: !copy_swap,
        });

        let scratch = self.cfg.scratch.addr;
        self.back_header = scratch;
        self.frame_headers = (0..buffers.len() as u64 - 1)
            .map(|i| {
                at(
                    scratch,
                    FRAME_HEADERS_OFFSET as u64 + i * SurfaceHeader::SIZE as u64,
                )
            })
            .collect::<Result<_, _>>()?;
        let bpp = host_fmt.bytes_per_texel().unwrap_or(4);
        let headers = std::iter::once(self.back_header).chain(self.frame_headers.iter().copied());
        for (header, &(data, _)) in headers.zip(&buffers) {
            SurfaceHeader::linear(host_fmt, p.width, p.height, p.width * bpp, data)
                .write(mem, header)?;
        }
        if p.enable_auto_depth_stencil {
            let dfmt = D3DFormat::from_raw(p.auto_depth_stencil_format)
                .filter(|f| f.is_depth())
                .ok_or_else(|| {
                    D3dError::unimplemented(
                        E,
                        format!("depth format {:#x}", p.auto_depth_stencil_format),
                    )
                })?;
            let dlin = linear_twin(dfmt);
            self.depth = Some(
                self.backend
                    .create_surface(dlin, p.width, p.height, true)
                    .map_err(D3dError::Backend)?,
            );
            self.depth_header = at(scratch, SurfaceHeader::SIZE as u64)?;
            let dbpp = dlin.bytes_per_texel().unwrap_or(4);
            SurfaceHeader::linear(dlin, p.width, p.height, p.width * dbpp, DEPTH_BUFFER_DATA)
                .write(mem, self.depth_header)?;
        }
        self.render_target = self.back_header;
        self.depth_stencil = self.depth_header;
        // Device object.
        let dev = l.device;
        self.store_viewport(
            mem,
            Viewport {
                x: 0,
                y: 0,
                width: p.width,
                height: p.height,
                min_z: 0.0,
                max_z: 1.0,
            },
        )?;
        mem.write_u32(dev + field::REF_COUNT, 1)?;
        mem.write_u32(dev + field::PUT, pb.addr)?;
        mem.write_u32(dev + field::THRESHOLD, pb.addr + pb.len - PUSH_MARGIN)?;
        let copy_flag = if copy_swap { COPY_SWAP_FLAG } else { 0 };
        mem.write_u32(dev + field::FLAGS, behavior_flags & 0x10 | copy_flag)?;
        mem.write_u32(dev + field::RENDER_TARGET, self.back_header)?;
        mem.write_u32(dev + field::DEPTH_STENCIL, self.depth_header)?;
        // `+0x1A10`: frame buffers (back buffers + 1, `0x3fc990`); the
        // header pointers follow from `+0x1A14`.
        mem.write_u32(dev + field::FRAME_BUFFER_COUNT, buffers.len() as u32)?;
        mem.write_u32(dev + field::BACK_BUFFER, self.back_header)?;
        for (i, &h) in self.frame_headers.iter().enumerate() {
            mem.write_u32(dev + field::DISPLAY_SURFACE + 4 * i as u32, h)?;
        }
        mem.write_u32(dev + field::SWAP_COUNT, 0)?;
        // The GPU's completion word: every fence handed out has passed.
        let completed = at(scratch, GPU_COMPLETED_OFFSET as u64)?;
        mem.write_u32(completed, self.fence)?;
        mem.write_u32(dev + field::GPU_COMPLETED, completed)?;
        mem.write_u32(dev + field::GPU_TIME, self.fence)?;
        mem.write_u32(l.p_device, dev)?;
        mem.write_u32(l.dirty_flags, 0)?;
        let defaults = StateWords::defaults(p.enable_auto_depth_stencil);
        defaults.write(mem, &l)?;
        self.pushed = Some(Arc::new(defaults));
        if pp_device != 0 {
            mem.write_u32(pp_device, dev)?;
        }
        self.consumed = pb.addr;
        self.fence_at = pb.addr;
        if let Some(base) = self.cfg.nv2a_window {
            // `0x3fc31d`: the register base; the original's idle wait at
            // `0x3fc3f2` follows, which the window answers at once.
            let w = match self.regs.take() {
                Some(w) if w.base() == base => w,
                _ => RegisterWindow::new(base).map_err(|e| D3dError::invalid(E, e))?,
            };
            self.regs = Some(w);
            mem.write_u32(dev + field::NV2A_REGISTERS, base)?;
            self.sync_registers(mem)?;
        }
        self.params = Some(p);
        // The lost-image state belongs to the old ring.
        self.display.shown_lost = false;
        self.created = true;
        self.torn_down.clear();
        self.init_transform_state(mem)?;
        Ok(0)
    }

    /// The device teardown `0x3fc530` (`EAX` = device), which Halo's inlined
    /// `Release` calls when the reference count at `+0x938` is 1 (boot:
    /// `0x12455`, right after the first CreateDevice and Swap). The original
    /// waits for the GPU, frees the push buffer and releases the bound
    /// resources; the caller then zeroes the device object and
    /// `D3D__pDevice` itself.
    ///
    /// Here: execute what is still queued and write back render-to-texture
    /// levels drawn into (the original's wait for the GPU leaves those texels
    /// in memory), then forget every host object and all device state, so
    /// that a later CreateDevice starts afresh. Guest memory is otherwise not
    /// touched. The backend, configuration, write tracker, counters,
    /// undrained events and the `PersistDisplay` image survive.
    pub fn release_device(
        &mut self,
        mem: &mut dyn GuestMemory,
        device: u32,
    ) -> Result<(), D3dError> {
        const E: &str = "D3D device teardown (0x3fc530)";
        self.live(E)?;
        if device != self.cfg.layout.device {
            return Err(D3dError::invalid(
                E,
                format!("device {device:#010x} is not D3D__Device"),
            ));
        }
        self.sync_push_buffer(mem)?;
        if self.immediate.is_open() {
            return Err(D3dError::invalid(E, "teardown inside Begin/End"));
        }
        self.flush_render_targets_for(mem, WritebackCause::Teardown)?;
        self.release_bindings(mem, E)?;
        self.release_render_targets(mem)?;
        // Every device-owned host surface goes with the device.
        let mut surfaces = Vec::new();
        if let Some(sw) = self.swap.take() {
            surfaces.extend(sw.back);
            surfaces.push(sw.front);
        }
        surfaces.extend(self.depth.take());
        surfaces.extend(self.other_surfaces.drain().map(|(_, (_, s))| s));
        for s in surfaces {
            self.backend.destroy_surface(s).map_err(D3dError::Backend)?;
        }
        self.created = false;
        self.params = None;
        self.swap = None;
        self.depth = None;
        self.back_header = 0;
        self.frame_headers.clear();
        self.depth_header = 0;
        self.other_surfaces.clear();
        self.render_target = 0;
        self.depth_stencil = 0;
        self.textures = [0; 4];
        self.palettes = [0; 4];
        self.vertex_shader = 0;
        self.pixel_shader = 0;
        self.streams = [StreamSource::default(); 16];
        self.stream_inputs = None;
        self.program.fill(0);
        self.program_start = 0;
        self.constants.fill(0);
        self.constants_generation += 1;
        self.viewport = Viewport {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            min_z: 0.0,
            max_z: 1.0,
        };
        self.transforms.clear();
        self.scissors.clear();
        self.scissor_exclusive = false;
        self.immediate = Immediate::default();
        self.consumed = self.cfg.push_buffer.addr;
        self.pushed = None;
        self.fault = None;
        self.swap_count = 0;
        self.fence = 0;
        self.cache.clear();
        if let Some(c) = &mut self.vertex_cache {
            c.clear();
        }
        if let Some(c) = &mut self.index_cache {
            c.clear();
        }
        self.depth_copy = None;
        self.depth_read_generation = None;
        self.depth_alias.clear();
        self.depth_color = None;
        self.depth_color_newer = false;
        self.drop_visibility_tests();
        self.hw.zpass_count_enable = false;
        // The original D3D_DestroyResource runs for these after this returns
        // and first waits on each (block_on_resource).
        self.torn_down = self.pending_destroys.iter().copied().collect();
        Ok(())
    }

    /// Teardown's resource releases. First the render target and then the
    /// depth/stencil surface are stamped and released as by a switch
    /// (`0x3fc720`, called at `0x3fc5cb`: `0x3fb5a0` at `0x3fc765` and
    /// `0x3fc78f`), and both device fields cleared. Then
    /// (`0x3fc5f4..0x3fc69b`) one internal
    /// reference off each bound texture and palette, each `D3D__Streams`
    /// buffer (and, here, each `SetVertexShaderInput` buffer, which this
    /// device also holds a reference to); then `D3D__Streams` (16 records,
    /// `0x30` dwords) is zeroed (`0x3fc704..0x3fc710`). Like the original,
    /// only `Common` changes. A release that drops the last reference
    /// destroys the resource ([`Device::destroy_resource`]) once the whole
    /// set is committed.
    fn release_bindings(
        &mut self,
        mem: &mut dyn GuestMemory,
        entry: &'static str,
    ) -> Result<(), D3dError> {
        let mut refs = Staged::default();
        self.release_surface(&mut refs, mem, entry, self.render_target)?;
        self.release_surface(&mut refs, mem, entry, self.depth_stencil)?;
        let mut held: Vec<u32> = Vec::new();
        held.extend(self.textures);
        held.extend(self.palettes);
        held.extend(self.streams.iter().map(|s| s.buffer));
        if let Some(inputs) = &self.stream_inputs {
            held.extend(inputs.iter().map(|s| s.buffer));
        }
        held.retain(|h| *h != 0);
        // Net count per header, then check before writing anything.
        let mut counts: Vec<(u32, u32)> = Vec::new();
        for h in held {
            match counts.iter_mut().find(|c| c.0 == h) {
                Some(c) => c.1 += 1,
                None => counts.push((h, 1)),
            }
        }
        for (h, n) in counts {
            let word = refs.read(mem, h)?;
            if (word & common::INTREFCOUNT_MASK) < n * common::INTREFCOUNT_ONE {
                return Err(D3dError::invalid(
                    entry,
                    format!(
                        "{h:#010x} is bound {n} time(s) with fewer internal references ({word:#010x})"
                    ),
                ));
            }
            // `0x3fc622..0x3fc63c`: each release stores the decremented
            // word and destroys a resource left with no reference.
            let c = word.wrapping_sub(n.wrapping_mul(common::INTREFCOUNT_ONE));
            refs.write(h, c);
            if c & (common::INTREFCOUNT_MASK | common::REFCOUNT_MASK) == 0 {
                refs.destroy(h);
            }
        }
        let destroys = refs.commit(mem)?;
        self.destroy_resources(mem, destroys)?;
        let dev = self.cfg.layout.device;
        mem.write_u32(dev + field::RENDER_TARGET, 0)?;
        mem.write_u32(dev + field::DEPTH_STENCIL, 0)?;
        let streams = self.cfg.layout.streams;
        mem.write(streams, &[0; 16 * 12])?;
        Ok(())
    }

    /// `D3DDevice_SetShaderConstantMode` (`0x3f6e10`, `EAX` = mode). Device
    /// flag `0x200` follows the `D3DSCM_NORESERVEDCONSTANTS` bit (`0x10`),
    /// and `+0x1928` takes the mode without it. Halo's only call
    /// (`0x12675`) passes `0x11` (192 constants, none reserved): constants
    /// are stored by hardware register here regardless, so nothing else
    /// changes. Mode 0 (96 constants) also reloads the reserved constants
    /// through the push buffer (`0x3f6e3e..`), which is not implemented.
    pub fn set_shader_constant_mode(
        &mut self,
        mem: &mut dyn GuestMemory,
        mode: u32,
    ) -> Result<(), D3dError> {
        const E: &str = "D3DDevice_SetShaderConstantMode";
        self.live(E)?;
        self.sync_push_buffer(mem)?;
        let base = mode & !0x10;
        if base == 0 {
            return Err(D3dError::unimplemented(
                E,
                format!("mode {mode:#x} (96 constants: reserved-constant reload)"),
            ));
        }
        let dev = self.cfg.layout.device;
        let flags = mem.read_u32(dev + field::FLAGS)?;
        let flags = if mode & 0x10 != 0 {
            flags | 0x200
        } else {
            flags & !0x200
        };
        mem.write_u32(dev + field::FLAGS, flags)?;
        mem.write_u32(dev + field::SHADER_CONSTANT_MODE, base)?;
        Ok(())
    }

    // ---------------------------------------------------------------------
    // Push buffer
    // ---------------------------------------------------------------------

    /// Executes whatever game code wrote into the push buffer since the last
    /// call. Every entry point calls this first.
    ///
    /// A failure latches the device: the failing command is not consumed,
    /// and every later call returns [`D3dError::Faulted`] instead of
    /// silently skipping it.
    pub fn sync_push_buffer(&mut self, mem: &mut dyn GuestMemory) -> Result<(), D3dError> {
        if !self.created {
            return Ok(());
        }
        if let Some(f) = &self.fault {
            return Err(D3dError::Faulted(f.clone()));
        }
        if let Some(cb) = self.undelivered {
            return Err(D3dError::unimplemented(
                "push buffer",
                format!(
                    "callback {:#010x}({:#x}) at {:#010x} is waiting for delivery: only the runtime glue can call the guest (Device::drain_until_callback)",
                    cb.function, cb.context, cb.at
                ),
            ));
        }
        match self.drain(mem, false) {
            Ok(_) => Ok(()),
            Err(e) => {
                self.fault = Some(format!(
                    "{e} (push-buffer command at {:#010x})",
                    self.consumed
                ));
                Err(e)
            }
        }
    }

    /// NV2A register state the push buffer has set so far.
    pub fn hw(&self) -> &HwState {
        &self.hw
    }

    /// The latched fault, if any.
    pub fn fault(&self) -> Option<&str> {
        self.fault.as_deref()
    }

    /// Like [`Device::sync_push_buffer`], but stops right after a
    /// push-buffer callback ([`PushCallback`]) and returns it: the caller
    /// runs the guest function, then calls this again to go on. The runtime
    /// glue does this before every entry, so a callback runs exactly where
    /// the GPU would have called it, after the work queued before it and
    /// before the work queued after it.
    ///
    /// Before a callback is returned, render-to-texture targets drawn into
    /// are written back to guest memory: the callback reads what the work
    /// before it produced (Halo's `0x24970` reads a target's texels through
    /// `0x24a70`).
    ///
    /// If that write-back fails, the error is returned and the callback is
    /// kept, not lost: like other write-back failures it is not latched,
    /// and the next call retries the write-back and then returns the same
    /// callback, before anything queued after it runs.
    ///
    /// While a callback is being delivered ([`Device::begin_push_callback`])
    /// this fails with [`D3dError::Invalid`] and drains nothing: the work
    /// after the callback must not run until it returns, and the device
    /// does not support D3D calls from a callback (on the Xbox they run at
    /// interrupt time, where D3D is off limits).
    pub fn drain_until_callback(
        &mut self,
        mem: &mut dyn GuestMemory,
    ) -> Result<Option<PushCallback>, D3dError> {
        if !self.created {
            return Ok(None);
        }
        if let Some(cb) = self.active_callback {
            return Err(D3dError::invalid(
                "push-buffer callback",
                format!(
                    "D3D called from inside the push-buffer callback {:#010x}({:#x}) queued at {:#010x}; the work after it must not run before it returns",
                    cb.function, cb.context, cb.at
                ),
            ));
        }
        if let Some(f) = &self.fault {
            return Err(D3dError::Faulted(f.clone()));
        }
        // A callback whose write-back failed last time: retry the write-back
        // and hand it out before draining anything after it.
        if let Some(cb) = self.undelivered {
            self.flush_render_targets_for(mem, WritebackCause::Callback)?;
            self.undelivered = None;
            return Ok(Some(cb));
        }
        match self.drain(mem, true) {
            Ok(Some(cb)) => {
                self.stats.push_callbacks += 1;
                // Like any other write-back failure (KickOff, Swap, ...),
                // not latched: the targets stay dirty and the next call
                // tries again. The callback waits with them.
                if let Err(e) = self.flush_render_targets_for(mem, WritebackCause::Callback) {
                    self.undelivered = Some(cb);
                    return Err(e);
                }
                Ok(Some(cb))
            }
            Ok(None) => {
                // Everything queued before this entry has been recorded:
                // the depth now is what the draws so far left.
                if let Some(ask) = self.pending_grid.take() {
                    self.issue_depth_grid(mem, ask);
                }
                if let Some(pass) = self.pending_external.take() {
                    self.issue_external_pass(mem, pass);
                }
                Ok(None)
            }
            Err(e) => {
                self.fault = Some(format!(
                    "{e} (push-buffer command at {:#010x})",
                    self.consumed
                ));
                Err(e)
            }
        }
    }

    /// Asks for the occlusion depth grid of the main depth buffer over the
    /// current viewport (docs/graphics/occlusion-culling.md): recorded
    /// after the next drain of the push buffer (the next D3D entry), so it
    /// holds the draws queued before it. The grid reaches `sink` later with
    /// the W-buffer parameters of that moment, without the guest waiting.
    /// A newer ask replaces one not issued yet. Nothing is recorded when
    /// the bound depth is not the device's own depth buffer, the depth is
    /// not W-buffered, or the backend cannot.
    pub fn request_depth_grid(&mut self, tag: u64, width: u32, height: u32, sink: OcclusionSink) {
        self.pending_grid = Some(GridAsk {
            tag,
            width,
            height,
            sink,
        });
    }

    /// The device side of Halo's `render_light_passes` tail
    /// (`0x000314e3`..`0x000314f2`), for a light whose passes the native
    /// renderer draws in their place (docs/graphics/native-renderer.md,
    /// stage 4), so a skipped light leaves the device as a drawn one does.
    /// What is queued drains first, as before any entry (a pending external
    /// pass is issued); then the tail's calls, through the code their
    /// entries run: `SetRenderState_StencilEnable(FALSE)`,
    /// `SetScissors(0, FALSE, NULL)`, and the game's depth-clip restore
    /// `0x00015720`: `clip` (the game's two words) into the programmable
    /// clip range, bit 1 of the clip flags (the game owns it), and the
    /// viewport fragment (`0x3f6190`) at the put pointer.
    ///
    /// `Ok(false)` with nothing set when a push-buffer callback is queued
    /// first (kept, as one whose write-back failed, for the next entry to
    /// deliver where it was queued) or the push buffer has no room for the
    /// fragment (the game would make room first): the caller keeps the
    /// D3D path.
    pub fn light_passes_tail(&mut self, mem: &mut dyn GuestMemory, clip: [u32; 2]) -> Result<bool, D3dError> {
        const E: &str = "render_light_passes tail";
        self.live(E)?;
        let dev = mem.read_u32(self.cfg.layout.p_device)?;
        if mem.read_u32(dev + field::PUT)? >= mem.read_u32(dev + field::THRESHOLD)? {
            return Ok(false);
        }
        if let Some(cb) = self.drain_until_callback(mem)? {
            self.undelivered = Some(cb);
            return Ok(false);
        }
        self.set_render_state(mem, crate::state::slot::STENCILENABLE, 0)?;
        self.set_scissors(mem, 0, 0, 0)?;
        // 0x00015720: the clip range and flag, then the fragment at the put
        // pointer, which it advances.
        let put = mem.read_u32(dev + field::PUT)?;
        if put >= mem.read_u32(dev + field::THRESHOLD)? {
            return Err(D3dError::invalid(E, "no push-buffer room for the viewport fragment"));
        }
        mem.write_u32(dev + field::CLIP_PROGRAM, clip[0])?;
        mem.write_u32(dev + field::CLIP_PROGRAM + 4, clip[1])?;
        let flags = mem.read_u32(dev + field::CLIP_FLAGS)?;
        mem.write_u32(dev + field::CLIP_FLAGS, flags | 2)?;
        let put = self.push_viewport(mem, put, dev)?;
        mem.write_u32(dev + field::PUT, put)?;
        Ok(true)
    }

    /// Whether the backend can draw an external pass: asked before the
    /// guest skips work the pass replaces.
    pub fn can_external_pass(&self) -> bool {
        self.backend.supports_external_pass()
    }

    /// Render-state slot `slot` (guest numbering) as the setters stored it
    /// in the guest's array: the state a draw recorded now would take.
    /// `None` past the slots or when it does not read. Read-only, for a
    /// native pass standing in for the next draws.
    pub fn render_state(&self, mem: &dyn GuestMemory, slot: u32) -> Option<u32> {
        if slot as usize >= RENDER_STATE_SLOTS {
            return None;
        }
        mem.read_u32(self.cfg.layout.render_state_slot(slot)).ok()
    }

    /// The stencil state a draw recorded now would take: the guest's
    /// render-state array through the build's slot table, decoded as
    /// draws decode it. `None` when a slot does not read or holds a value
    /// outside its enum.
    pub fn stencil_state(&self, mem: &dyn GuestMemory) -> Option<gfx::state::StencilState> {
        use gfx::state::RenderStateType as R;
        let table = RenderStateTable::default();
        let mut rs = gfx::state::RenderState::default();
        for t in [
            R::StencilEnable,
            R::StencilFunc,
            R::StencilRef,
            R::StencilMask,
            R::StencilWriteMask,
            R::StencilFail,
            R::StencilZFail,
            R::StencilPass,
        ] {
            let guest = table.canonical_to_guest(t.index())?;
            rs.set_raw(t, self.render_state(mem, guest)?).ok()?;
        }
        Some(rs.stencil)
    }

    /// The `A8R8G8B8` entries of the palette bound at `stage` now
    /// (`SetPalette`), as the D3D path samples a P8 texture there: `None`
    /// when none is bound or it does not read. Read-only, for a native
    /// pass that expands the same indices itself.
    pub fn stage_palette(&self, mem: &dyn GuestMemory, stage: usize) -> Option<Vec<u32>> {
        let header = *self.palettes.get(stage)?;
        if header == 0 {
            return None;
        }
        let p = PaletteHeader::read(mem, header).ok()?;
        if p.resource.kind() != ResourceType::Palette {
            return None;
        }
        let va = self.cfg.address_map.to_virtual(p.resource.data);
        let bytes = mem.read_vec(va, 4 * p.entries() as usize).ok()?;
        Some(bytes.chunks_exact(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect())
    }

    /// Asks for `pass` to draw into the bound colour target and depth with
    /// the current viewport (docs/graphics/native-renderer.md): recorded
    /// after the next drain of the push buffer (the next D3D entry), in
    /// order with the draws queued before it. A newer ask replaces one not
    /// issued yet. Dropped (counted in `external_passes_dropped`) when no
    /// colour or depth is bound or the backend refuses.
    pub fn request_external_pass(&mut self, pass: Box<dyn crate::backend::ExternalPass>) {
        if self.pending_external.replace(pass).is_some() {
            self.refuse_external(ExternalRefusal::Replaced);
        }
    }

    /// Arms `s` (docs/graphics/native-renderer.md, stage 5): the next
    /// inline draws, one per entry of [`Swallow::targets`], are the pass's.
    /// What is queued drains first, as before any entry, so only draws
    /// pushed after this call count. Each must be the next one planned:
    /// an inline `Begin`/`End` of [`Swallow::primitive`] with
    /// [`Swallow::vertices`] vertices, no other draw since the last, into
    /// its slot's colour target (slot 0 the one bound now; another slot the
    /// target its first draw finds, each slot its own target). With
    /// [`Swallow::drop`] each is dropped, else drawn. After the last,
    /// [`Swallow::pass`] is issued into the targets bound then, in its
    /// place. A draw that is not the next planned ends the swallow
    /// ([`Stats::swallow_mismatches`]): it and every draw after it are
    /// drawn, and the pass is not issued. Draws already dropped stay
    /// dropped.
    ///
    /// `Ok(false)` with nothing armed when a push-buffer callback is
    /// queued first (kept for the next entry), no colour target is bound,
    /// the plan is empty, or (dropping) the pass could not be issued into
    /// the targets bound now or is not colour-only
    /// ([`ExternalPass::colour_only`](crate::backend::ExternalPass::colour_only);
    /// [`Stats::swallow_refusals`], the reason counted and logged as its
    /// issue would). Arming over a swallow not
    /// finished ends that one as a mismatch.
    pub fn swallow(&mut self, mem: &mut dyn GuestMemory, s: Swallow) -> Result<bool, D3dError> {
        const E: &str = "swallow";
        self.live(E)?;
        if let Some(cb) = self.drain_until_callback(mem)? {
            self.undelivered = Some(cb);
            return Ok(false);
        }
        if self.swallowing.take().is_some() {
            self.stats.swallow_mismatches += 1;
        }
        if s.targets.is_empty() || self.render_target == 0 {
            return Ok(false);
        }
        // Dropping needs the pass issued in their place: what its issue
        // checks is checked here, before any draw is dropped. Its target is
        // slot 0's, bound now and at the last draw; the depth bound then is
        // not known now, so the pass must be colour-only.
        if s.drop {
            let ready = if s.pass.colour_only() {
                self.external_ready(mem, true).map(|_| ())
            } else {
                Err(ExternalRefusal::NoDepth)
            };
            if let Err(why) = ready {
                self.stats.swallow_refusals += 1;
                self.refuse_external(why);
                return Ok(false);
            }
        }
        let slots = s.targets.iter().map(|&t| usize::from(t)).max().unwrap_or(0) + 1;
        let mut headers = vec![None; slots];
        headers[0] = Some(self.render_target);
        self.swallowing = Some(Swallowing {
            targets: s.targets,
            next: 0,
            headers,
            primitive: s.primitive,
            vertices: s.vertices,
            drop: s.drop,
            draws: self.stats.draws,
            pass: Some(s.pass),
        });
        Ok(true)
    }

    /// Whether a swallow is armed.
    pub fn swallowing(&self) -> bool {
        self.swallowing.is_some()
    }

    /// What becomes of inline draw `d` under the armed swallow.
    fn swallow_fate(&mut self, d: &ImmediateDraw) -> Fate {
        let Some(s) = self.swallowing.as_mut() else { return Fate::Drawn };
        let target = self.render_target;
        let slot = usize::from(s.targets[s.next]);
        let into = match s.headers[slot] {
            Some(h) => h == target,
            None => target != 0 && !s.headers.contains(&Some(target)),
        };
        let fits = d.inline.is_empty()
            && d.primitive == s.primitive
            && d.vertices.len() == s.vertices
            && self.stats.draws == s.draws
            && into;
        if !fits {
            self.swallowing = None;
            self.stats.swallow_mismatches += 1;
            return Fate::Drawn;
        }
        s.headers[slot] = Some(target);
        self.stats.swallowed_draws += 1;
        if s.drop { Fate::Dropped } else { Fate::Observed }
    }

    /// After a swallowed draw (dropped, or drawn when observing): the next
    /// planned, or the pass in place of the last.
    fn swallow_advance(&mut self, mem: &mut dyn GuestMemory) {
        let Some(s) = self.swallowing.as_mut() else { return };
        s.next += 1;
        s.draws = self.stats.draws;
        if s.next < s.targets.len() {
            return;
        }
        let pass = s.pass.take();
        self.swallowing = None;
        if let Some(pass) = pass {
            self.issue_external_pass(mem, pass);
        }
    }

    /// Counts and logs (once per reason) a pass not issued.
    fn refuse_external(&mut self, why: ExternalRefusal) {
        use std::sync::atomic::Ordering;
        self.stats.external_passes_dropped += 1;
        let i = why.index();
        self.stats.external_refusals[i] += 1;
        if EXTERNAL_REFUSALS_LOGGED.fetch_or(1 << i, Ordering::Relaxed) & (1 << i) == 0 {
            eprintln!("d3d: an external pass was dropped: {} (logged once)", why.describe());
        }
    }

    /// What issuing a pass needs that is checked without touching the
    /// targets: the backend, the bound colour (and depth unless
    /// `colour_only`), the colour's header and the Z scale. The request.
    fn external_ready(
        &self,
        mem: &mut dyn GuestMemory,
        colour_only: bool,
    ) -> Result<crate::backend::ExternalPassRequest, ExternalRefusal> {
        if !self.backend.supports_external_pass() {
            return Err(ExternalRefusal::Unsupported);
        }
        if self.render_target == 0 {
            return Err(ExternalRefusal::NoColour);
        }
        if !colour_only && self.depth_stencil == 0 {
            return Err(ExternalRefusal::NoDepth);
        }
        let d = self.header_desc(mem, self.render_target).map_err(|_| ExternalRefusal::Header)?;
        let z_scale = self.guest_z_scale(mem).ok_or(ExternalRefusal::Header)?;
        let v = self.viewport;
        Ok(crate::backend::ExternalPassRequest {
            rect: (v.x, v.y, v.width, v.height),
            surface_size: (d.width, d.height),
            z_scale,
        })
    }

    /// The device's depth full scale (`+0x948`), the D24 one when it is
    /// not a positive finite number; `None` when it does not read.
    fn guest_z_scale(&self, mem: &dyn GuestMemory) -> Option<f32> {
        let dev = mem.read_u32(self.cfg.layout.p_device).ok()?;
        let z_scale = f32::from_bits(mem.read_u32(dev + field::Z_SCALE).ok()?);
        Some(if z_scale.is_finite() && z_scale > 0.0 { z_scale } else { 16_777_215.0 })
    }

    fn issue_external_pass(&mut self, mem: &mut dyn GuestMemory, mut pass: Box<dyn crate::backend::ExternalPass>) {
        let constants: [[f32; 4]; crate::backend::VIEW_CONSTANTS] = std::array::from_fn(|r| self.constant(r));
        pass.view_constants(&constants);
        if let Some(z_max) = self.guest_z_scale(mem) {
            pass.depth_clip(self.depth_clip(mem, z_max));
        }
        let colour_only = pass.colour_only();
        let issued = match self.external_ready(mem, colour_only) {
            Err(why) => Err(why),
            Ok(request) => match self.current_targets(mem, "external pass") {
                Err(_) => Err(ExternalRefusal::Targets),
                // Colour only: the colour as its own depth (the backend
                // gives it a scratch depth; `Backend::external_pass`).
                Ok((color, depth)) => match if colour_only { Some(color) } else { depth } {
                    None => Err(ExternalRefusal::NoDepth),
                    Some(depth) => match self.backend.external_pass(color, depth, request, pass) {
                        Ok(true) => Ok(()),
                        Ok(false) => Err(ExternalRefusal::Backend),
                        Err(_) => Err(ExternalRefusal::Failed),
                    },
                },
            },
        };
        match issued {
            Ok(()) => self.stats.external_passes += 1,
            Err(why) => self.refuse_external(why),
        }
    }

    /// The W-buffer's clip a W-buffered vertex-program draw recorded now
    /// takes, for an external pass standing in for such draws: the pushed
    /// clip range as the D3D path reads it (`nv2a::w_clip_range`, the whole
    /// `[0, z_max]` when empty) and `D3DRS_DEPTHCLIPCONTROL` from the
    /// guest's render-state array (unreadable: no clamp).
    pub fn depth_clip(&self, mem: &dyn GuestMemory, z_max: f32) -> crate::backend::DepthClip {
        let slot = RenderStateTable::default().canonical_to_guest(crate::nv2a::RS_DEPTHCLIPCONTROL);
        let control = slot.and_then(|s| self.render_state(mem, s)).unwrap_or(0);
        crate::backend::DepthClip {
            range: crate::nv2a::w_clip_range(&self.hw, z_max),
            clamp: crate::nv2a::clamps_depth(control),
        }
    }

    fn issue_depth_grid(&mut self, mem: &mut dyn GuestMemory, ask: GridAsk) {
        let Some(scale) = self.depth_grid_scale(mem) else {
            self.stats.depth_grids_skipped += 1;
            return;
        };
        let (Some(depth), Some(p)) = (self.depth, self.params) else {
            self.stats.depth_grids_skipped += 1;
            return;
        };
        if self.depth_stencil == 0 || self.depth_stencil != self.depth_header {
            self.stats.depth_grids_skipped += 1;
            return;
        }
        let v = self.viewport;
        let request = crate::backend::DepthGridRequest {
            tag: ask.tag,
            rect: (v.x, v.y, v.width, v.height),
            surface_size: (p.width, p.height),
            width: ask.width,
            height: ask.height,
        };
        let sink = ask.sink;
        let wrapped: crate::backend::DepthGridSink = std::sync::Arc::new(move |r| sink(r, scale));
        match self.backend.request_depth_grid(depth, request, wrapped) {
            Ok(true) => self.stats.depth_grids += 1,
            _ => self.stats.depth_grids_skipped += 1,
        }
    }

    /// The W-buffer's parameters now: `None` unless the depth is W-buffered
    /// (`SET_CONTROL0` z-perspective) with a finite, positive scale.
    fn depth_grid_scale(&self, mem: &dyn GuestMemory) -> Option<DepthScale> {
        let w = self.hw.control0? & crate::nv2a::CONTROL0_Z_PERSPECTIVE != 0;
        if !w {
            return None;
        }
        let dev = mem.read_u32(self.cfg.layout.p_device).ok()?;
        let inv_w_far = f32::from_bits(mem.read_u32(dev + field::INV_W_FAR).ok()?);
        let z_scale = f32::from_bits(mem.read_u32(dev + field::Z_SCALE).ok()?);
        (inv_w_far.is_finite() && inv_w_far > 0.0 && z_scale.is_finite() && z_scale > 0.0)
            .then_some(DepthScale { inv_w_far, z_scale })
    }

    /// Marks `cb` as being delivered: until [`Device::end_push_callback`],
    /// [`Device::drain_until_callback`] refuses to drain (the runtime glue
    /// calls these around the guest call).
    pub fn begin_push_callback(&mut self, cb: PushCallback) {
        self.active_callback = Some(cb);
    }

    /// The callback returned.
    pub fn end_push_callback(&mut self) {
        self.active_callback = None;
    }

    /// The push-buffer callback being delivered, if any.
    pub fn active_push_callback(&self) -> Option<PushCallback> {
        self.active_callback
    }

    /// `NV097_NO_OPERATION` with a non-zero parameter: on the Xbox the GPU
    /// interrupts and D3D's handler (`0x3ff240`) dispatches on the low five
    /// bits through the table at `0x3ff454`. The operands are the two clear
    /// values. Returns the callback for 6 and 7; 11 does nothing in the
    /// original either. The others drive the swap queue, events and
    /// register writes, which only D3D's own (overridden) code pushes.
    fn software_method(&self, at: u32, data: u32) -> Result<Option<PushCallback>, D3dError> {
        match data & 0x1F {
            6 | 7 => {
                let function = self.hw.zstencil_clear_value;
                if function == 0 {
                    return Err(D3dError::invalid(
                        "push buffer",
                        format!("callback at {at:#010x} with no function"),
                    ));
                }
                Ok(Some(PushCallback {
                    function,
                    context: self.hw.color_clear_value,
                    kind: data & 0x1F,
                    at,
                }))
            }
            11 => Ok(None),
            op => Err(D3dError::unimplemented(
                "push buffer",
                format!("software method {op} (NO_OPERATION {data:#x} at {at:#010x})"),
            )),
        }
    }

    /// Executes queued words. With `stop_at_callback`, returns after the
    /// first push-buffer callback; without, a callback is an error (nothing
    /// could deliver it) and is left unconsumed.
    fn drain(
        &mut self,
        mem: &mut dyn GuestMemory,
        stop_at_callback: bool,
    ) -> Result<Option<PushCallback>, D3dError> {
        let pb = self.cfg.push_buffer;
        let end = pb.addr as u64 + pb.len as u64;
        let put = mem.read_u32(self.cfg.layout.device + field::PUT)?;
        if put == self.consumed {
            return Ok(None);
        }
        let _p = timeline::phase::enter(timeline::phase::Phase::Decode);
        let _t = timeline::span(timeline::Cat::D3d, timeline::Track::Host, "pushbuf drain")
            .arg("bytes", put.wrapping_sub(self.consumed) as u64);
        if put < self.consumed || put as u64 > end || !(put - self.consumed).is_multiple_of(4) {
            return Err(D3dError::invalid(
                "push buffer",
                format!(
                    "put {put:#010x} is not a word boundary in [{:#010x}, {end:#010x}]",
                    self.consumed
                ),
            ));
        }
        while self.consumed < put {
            let (next, call) =
                pushbuf::decode_one_into(mem, self.consumed, put, &mut self.push_words)?;
            if let Some(c) = &call {
                let words = c.data.len();
                for (i, (m, w)) in c.writes().enumerate() {
                    if m == pushbuf::method::NOP && w != 0 {
                        let Some(cb) = self.software_method(c.at, w)? else {
                            continue;
                        };
                        if i + 1 != words {
                            return Err(D3dError::unimplemented(
                                "push buffer",
                                format!("callback at {:#010x} inside a multi-word command", c.at),
                            ));
                        }
                        if !stop_at_callback {
                            return Err(D3dError::unimplemented(
                                "push buffer",
                                format!(
                                    "callback {:#010x}({:#x}) at {:#010x}: only the runtime glue can call the guest (Device::drain_until_callback)",
                                    cb.function, cb.context, cb.at
                                ),
                            ));
                        }
                        self.stats.push_words += 1 + words as u64;
                        self.consumed = next;
                        return Ok(Some(cb));
                    }
                    match self.immediate.apply(c.at, m, w)? {
                        Some(Effect::Draw(d)) => {
                            let fate = self.swallow_fate(&d);
                            if fate != Fate::Dropped {
                                self.stats.inline_draws += 1;
                                self.immediate_draw(mem, "inline Begin/End", d)?;
                            }
                            if fate != Fate::Drawn {
                                self.swallow_advance(mem);
                            }
                        }
                        Some(Effect::None) => {}
                        None if self.mirror_slot(m).is_some() => {
                            // A state the game pushed itself (or through
                            // SetRenderState_Simple): the payload is what
                            // the GPU sees from here on, whatever the array
                            // holds by the time this drains.
                            let slot = self.mirror_slot(m).expect("checked");
                            self.push_slot(mem, slot, w)?;
                        }
                        None if m == crate::hw::method::CLEAR_REPORT_VALUE => {
                            self.clear_report(c.at, w)?;
                            self.stats.hw_methods += 1;
                        }
                        None => {
                            // Any method may load constants.
                            self.constants_generation += 1;
                            let mut mems = Memories {
                                constants: &mut self.constants,
                                program: &mut self.program,
                            };
                            let known = self
                                .hw
                                .apply(m, w, &mut mems)
                                .map_err(|e| D3dError::invalid("push buffer", e.to_string()))?;
                            if !known {
                                return Err(PushError::UnknownMethod {
                                    method: m,
                                    at: c.at,
                                }
                                .into());
                            }
                            if m == crate::hw::method::PROGRAM_START {
                                self.program_start = w;
                            }
                            self.stats.hw_methods += 1;
                        }
                    }
                }
            }
            // Consumed only once fully executed.
            if let Some(c) = call {
                self.stats.push_words += 1 + c.data.len() as u64;
                self.push_words = c.data;
            }
            self.consumed = next;
        }
        Ok(None)
    }

    /// `D3D_MakeRequestedSpace` (`0x3faae0`): called by inline writers when
    /// put reached the threshold. Executes the buffer, rewinds it, and
    /// returns the new put (also stored in the device).
    pub fn make_requested_space(
        &mut self,
        mem: &mut dyn GuestMemory,
        minimum: u32,
        requested: u32,
    ) -> Result<u32, D3dError> {
        const E: &str = "D3D_MakeRequestedSpace";
        self.live(E)?;
        self.sync_push_buffer(mem)?;
        let pb = self.cfg.push_buffer;
        if minimum.max(requested.min(minimum)) > pb.len - PUSH_MARGIN {
            return Err(D3dError::invalid(
                E,
                format!("request of {minimum:#x} bytes exceeds the push buffer"),
            ));
        }
        mem.write_u32(self.cfg.layout.device + field::PUT, pb.addr)?;
        self.consumed = pb.addr;
        Ok(pb.addr)
    }

    /// `D3D_BlockOnResource` (`0x3fad50`, `EAX` = resource): what
    /// [`kick_off`](Self::kick_off) does, while there is a device.
    ///
    /// The original `D3D_DestroyResource` calls it first for every resource
    /// except a surface the title made (`0x3fb537..0x3fb539`), and a
    /// teardown's destroys run after the teardown entry has returned (see
    /// [`Device::take_pending_destroys`]), when there is no device any
    /// more. Those waits are already satisfied: the teardown drained the
    /// push buffer and wrote every render target back before it queued the
    /// resources, and nothing can be queued after it. So each resource that
    /// teardown destroyed may be waited on once with no device; any other
    /// wait with no device is still `NoDevice`.
    pub fn block_on_resource(
        &mut self,
        mem: &mut dyn GuestMemory,
        resource: u32,
    ) -> Result<(), D3dError> {
        if !self.created && self.torn_down.remove(&resource) {
            return Ok(());
        }
        self.live("D3D_BlockOnResource")?;
        self.kick_off(mem)?;
        // The caller is about to touch the resource: lazily kept
        // render-to-texture pixels of it go to guest memory now.
        self.flush_for_texture(mem, resource, WritebackCause::BlockOnResource)
    }

    /// `D3D_BlockOnTime`: what [`kick_off`](Self::kick_off) does, and every
    /// render-to-texture target goes to guest memory (after a wait for a
    /// fence the guest may read anything drawn before it).
    pub fn block_on_time(&mut self, mem: &mut dyn GuestMemory) -> Result<(), D3dError> {
        self.kick_off(mem)?;
        self.flush_render_targets_for(mem, WritebackCause::BlockOnTime)
    }

    /// `CDevice_KickOff`, `D3D_BlockOnTime`, `D3D_BlockOnResource`: execute
    /// pending words (the host GPU is synchronous, so nothing waits).
    /// Render-to-texture targets are written back unless that is lazy
    /// ([`Device::set_rtt_direct`]).
    pub fn kick_off(&mut self, mem: &mut dyn GuestMemory) -> Result<(), D3dError> {
        let _t = timeline::span(timeline::Cat::D3d, timeline::Track::Host, "KickOff");
        self.live("CDevice_KickOff")?;
        self.sync_push_buffer(mem)?;
        self.flush_at_sync(mem)?;
        self.sync_registers(mem)
    }

    /// Writes the NV2A register window's words for the current state and
    /// reports guest stores into it ([`crate::regs`]). Called where the GPU
    /// state the window shows changes or is about to be polled: CreateDevice,
    /// `KickOff` (every game site reads the window right after one),
    /// `SetFence` and `Swap`. Between those the words are the last ones
    /// written, which still describe an idle GPU.
    fn sync_registers(&mut self, mem: &mut dyn GuestMemory) -> Result<(), D3dError> {
        let Some(w) = self.regs.as_mut() else {
            return Ok(());
        };
        let counter = mem.read_u32(self.cfg.layout.device + field::FENCE_TAG)?;
        let values = RegisterValues::synchronous(self.consumed, self.fence, self.fence_at, counter);
        for write in w.sync(mem, &values)? {
            self.events.push(Event::RegisterWrite {
                offset: write.offset,
                value: write.value,
            });
        }
        Ok(())
    }

    /// The NV2A register window's base (device `+0x934`), if configured.
    pub fn register_window(&self) -> Option<u32> {
        self.regs.as_ref().map(RegisterWindow::base)
    }

    /// The deferred-state commit (`0x400fa0`): executes pending words, makes
    /// the current arrays the pushed state, and clears the dirty flags.
    pub fn commit(&mut self, mem: &mut dyn GuestMemory) -> Result<(), D3dError> {
        self.live("D3D_CommitState")?;
        self.sync_push_buffer(mem)?;
        self.refresh_pushed(mem)?;
        // `0x401013`: the commit keeps the bits it does not consume, the
        // vertex-format and binding bits (`0x70`) that `CDevice_SetStateVB`
        // (`0x401030`, native) reads, and the two top bits.
        let dirty = self.cfg.layout.dirty_flags;
        let d = mem.read_u32(dirty)?;
        mem.write_u32(dirty, d & COMMIT_KEEPS)?;
        Ok(())
    }

    /// `D3D_SetFence`: the GPU is synchronous here, so every fence has
    /// passed by the time it is returned. The GPU time (`+0x2C`) and the
    /// completion word (`[+0x30]`, [`Device::gpu_completed`]) both become
    /// the new fence.
    pub fn set_fence(&mut self, mem: &mut dyn GuestMemory) -> Result<u32, D3dError> {
        self.live("D3D_SetFence")?;
        self.sync_push_buffer(mem)?;
        self.fence = self.fence.wrapping_add(1);
        self.fence_at = self.consumed;
        let dev = self.cfg.layout.device;
        mem.write_u32(dev + field::GPU_TIME, self.fence)?;
        mem.write_u32(self.gpu_completed_word()?, self.fence)?;
        self.sync_registers(mem)?;
        Ok(self.fence)
    }

    /// The GPU's completion time: the last fence whose work has executed.
    /// The device executes the push buffer synchronously at every entry, so
    /// once the queued words are drained this is the last fence handed out.
    /// The guest sees it at `[device + 0x30]`.
    pub fn gpu_completed(&self) -> u32 {
        self.fence
    }

    fn gpu_completed_word(&self) -> Result<u32, D3dError> {
        at(self.cfg.scratch.addr, GPU_COMPLETED_OFFSET as u64)
    }

    // ---------------------------------------------------------------------
    // State
    // ---------------------------------------------------------------------

    fn refresh_pushed(&mut self, mem: &dyn GuestMemory) -> Result<(), D3dError> {
        self.pushed = Some(Arc::new(self.state(mem)?));
        Ok(())
    }

    fn pushed_state(&self, mem: &dyn GuestMemory) -> Result<Arc<StateWords>, D3dError> {
        match &self.pushed {
            Some(s) => Ok(s.clone()),
            None => Ok(Arc::new(self.state(mem)?)),
        }
    }

    /// Stores a render-state slot as a D3D setter does: into the guest array
    /// and (because the setter pushes it) into the pushed snapshot.
    fn store_rs(
        &mut self,
        mem: &mut dyn GuestMemory,
        guest_slot: u32,
        value: u32,
    ) -> Result<(), D3dError> {
        mem.write_u32(self.cfg.layout.render_state_slot(guest_slot), value)?;
        if let Some(p) = &mut self.pushed {
            Arc::make_mut(p).render[guest_slot as usize] = value;
        }
        Ok(())
    }

    /// Any `SetRenderState_*` setter: stores into `D3D__RenderState` (guest
    /// slot numbering).
    pub fn set_render_state(
        &mut self,
        mem: &mut dyn GuestMemory,
        guest_slot: u32,
        value: u32,
    ) -> Result<(), D3dError> {
        const E: &str = "D3DDevice_SetRenderState";
        self.live(E)?;
        self.sync_push_buffer(mem)?;
        if guest_slot as usize >= RENDER_STATE_SLOTS {
            return Err(D3dError::invalid(E, format!("slot {guest_slot}")));
        }
        if guest_slot == slot::ZBIAS {
            return self.apply_zbias(mem, value);
        }
        if guest_slot == slot::ZENABLE {
            // `0x3f5546..0x3f5588`: entering or leaving w-buffering redoes
            // the transform fragments.
            let old = mem.read_u32(self.cfg.layout.render_state_slot(guest_slot))?;
            self.store_rs(mem, guest_slot, value)?;
            if old == 2 || value == 2 {
                self.refresh_transform(mem, true, true)?;
            }
            return Ok(());
        }
        self.store_rs(mem, guest_slot, value)
    }

    /// `SetRenderState_ZBias` (`0x3f59b0`). As compiled it derives the
    /// polygon-offset state from the bias and stores all of it:
    /// `ZSLOPESCALE = -bias / 4`, `ZOFFSET = -bias` (bias as an unsigned
    /// integer, converted to float), the three offset enables = `bias != 0`,
    /// and `ZBIAS = bias`.
    pub fn set_zbias(&mut self, mem: &mut dyn GuestMemory, bias: u32) -> Result<(), D3dError> {
        self.live("D3DDevice_SetRenderState_ZBias")?;
        self.sync_push_buffer(mem)?;
        self.apply_zbias(mem, bias)
    }

    fn apply_zbias(&mut self, mem: &mut dyn GuestMemory, bias: u32) -> Result<(), D3dError> {
        let f = -(bias as f64);
        let enable = (bias != 0) as u32;
        self.store_rs(
            mem,
            slot::POLYGON_OFFSET_ZSLOPESCALE,
            ((f * 0.25) as f32).to_bits(),
        )?;
        self.store_rs(mem, slot::POLYGON_OFFSET_ZOFFSET, (f as f32).to_bits())?;
        self.store_rs(mem, slot::POINT_OFFSET_ENABLE, enable)?;
        self.store_rs(mem, slot::WIREFRAME_OFFSET_ENABLE, enable)?;
        self.store_rs(mem, slot::SOLID_OFFSET_ENABLE, enable)?;
        self.store_rs(mem, slot::ZBIAS, bias)
    }

    /// The GPU has seen `value` for guest render-state slot `slot` (an
    /// inline write or `SetRenderState_Simple`). Only the pushed snapshot
    /// changes: the guest array is the game's to store, before or after.
    fn push_slot(&mut self, mem: &dyn GuestMemory, slot: u32, value: u32) -> Result<(), D3dError> {
        if self.pushed.is_none() {
            self.refresh_pushed(mem)?;
        }
        let p = self.pushed.as_mut().expect("just set");
        Arc::make_mut(p).render[slot as usize] = value;
        Ok(())
    }

    /// `SetRenderState_Simple` (`0x3f5cf0`, `ECX` = push header
    /// `1 << 18 | method`, `EDX` = value): writes exactly those two words
    /// into the push buffer (`0x3f5d05`/`0x3f5d08`). Game code stores the
    /// array either before or after the call (`0x1268b` then `0x1269a`), so
    /// the registers are what counts: `EDX` becomes the pushed value of the
    /// slot the method encodes ([`SimpleEncode`], read from the image).
    pub fn set_render_state_simple(
        &mut self,
        mem: &mut dyn GuestMemory,
        header: u32,
        value: u32,
    ) -> Result<(), D3dError> {
        const E: &str = "D3DDevice_SetRenderState_Simple";
        self.live(E)?;
        self.sync_push_buffer(mem)?;
        let method = header & 0x3_FFFF;
        let slot = (header >> 18 == 1)
            .then(|| self.mirror_slot(method))
            .flatten()
            .ok_or_else(|| {
                D3dError::unimplemented(
                    E,
                    format!("header {header:#x} is not a known one-word state method"),
                )
            })?;
        self.push_slot(mem, slot, value)?;
        self.stats.simple_state_calls += 1;
        Ok(())
    }

    /// `SetTextureStageState` and the `SetTextureState_*` setters.
    pub fn set_texture_stage_state(
        &mut self,
        mem: &mut dyn GuestMemory,
        stage: u32,
        state: u32,
        value: u32,
    ) -> Result<(), D3dError> {
        const E: &str = "D3DDevice_SetTextureStageState";
        self.live(E)?;
        self.sync_push_buffer(mem)?;
        if stage >= 4 || state >= 32 {
            return Err(D3dError::invalid(E, format!("stage {stage} state {state}")));
        }
        mem.write_u32(self.cfg.layout.texture_state_slot(stage, state), value)?;
        if let Some(p) = &mut self.pushed {
            Arc::make_mut(p).stages[stage as usize][state as usize] = value;
        }
        Ok(())
    }

    fn add_internal_ref(mem: &mut dyn GuestMemory, header: u32) -> Result<(), D3dError> {
        if header != 0 {
            let c = mem.read_u32(header)?;
            mem.write_u32(header, c.wrapping_add(common::INTREFCOUNT_ONE))?;
        }
        Ok(())
    }

    /// Unbinding `header` (a texture, palette or vertex buffer): drops the
    /// internal reference and stamps `Lock` with the last-use time. With no
    /// reference left the resource is destroyed instead, as by `0x3fb5a0`
    /// (no decrement; [`Device::destroy_resource`]).
    fn drop_internal_ref(
        &mut self,
        mem: &mut dyn GuestMemory,
        entry: &'static str,
        header: u32,
    ) -> Result<(), D3dError> {
        if header == 0 {
            return Ok(());
        }
        let word = mem.read_u32(header)?;
        if word & common::INTREFCOUNT_MASK == 0 {
            return Err(D3dError::invalid(
                entry,
                format!("{header:#010x} is bound without an internal reference ({word:#010x})"),
            ));
        }
        let c = word.wrapping_sub(common::INTREFCOUNT_ONE);
        if c & (common::INTREFCOUNT_MASK | common::REFCOUNT_MASK) == 0 {
            return self.destroy_resource(mem, header);
        }
        mem.write_u32(header, c)?;
        mem.write_u32(at(header, 8)?, self.fence)?; // Lock = last-use time
        Ok(())
    }

    /// `D3D_DestroyResource` for `header`, which has no reference left.
    ///
    /// The original (`0x3fb520`, `EDI` = resource; `ret`) waits for the GPU
    /// (`BlockOnResource`, except for a surface the title made), unlinks the
    /// resource from D3D's list at `0x408644`, frees its data
    /// (`0x2d0a60(Data | 0x80000000, 0xA1800000)` for D3D-created vertex
    /// buffers, palettes, textures and surfaces; `Data` itself for a push
    /// buffer; nothing for index buffers and fixups) and frees the header
    /// (`0x122c0(header, 0x24800000)`). It only touches guest memory and
    /// calls overridden D3D entries, so it runs as compiled
    /// ([`crate::abi::NATIVE`]): the device drops what it keeps for the
    /// header (cached textures, a render-to-texture surface, after writing
    /// its pixels back) and queues the header. The runtime glue calls the
    /// original for each queued header when the entry returns
    /// ([`Device::take_pending_destroys`]).
    pub fn destroy_resource(
        &mut self,
        mem: &mut dyn GuestMemory,
        header: u32,
    ) -> Result<(), D3dError> {
        if header == 0 || !self.title_surface(header) {
            return Err(D3dError::invalid(
                "D3D_DestroyResource",
                format!("{header:#010x} is not a title resource"),
            ));
        }
        // A level drawn into keeps its pixels in the parent's texels, and
        // so do the bytes the alias mirror holds over it.
        let spans: Vec<(u64, u64)> = self
            .rtt
            .iter()
            .filter(|(h, t)| **h == header || t.parent == header)
            .map(|(_, t)| (t.data as u64, t.data as u64 + t.len() as u64))
            .collect();
        for (lo, hi) in spans {
            self.mirror_flush_range(mem, lo, hi)?;
        }
        self.write_back(mem, header, WritebackCause::Destroy)?;
        self.rtt_synced.remove(&header);
        self.rtt_stores.remove(&header);
        self.depth_level_tex.remove(&header);
        self.mirror_stale.remove(&header);
        if let Some(t) = self.rtt.remove(&header) {
            self.rtt_epoch += 1;
            self.backend
                .destroy_surface(t.surface)
                .map_err(D3dError::Backend)?;
        }
        for h in self
            .rtt
            .iter()
            .filter(|(_, t)| t.parent == header)
            .map(|(h, _)| *h)
            .collect::<Vec<_>>()
        {
            self.write_back(mem, h, WritebackCause::Destroy)?;
            self.rtt_synced.remove(&h);
            self.rtt_stores.remove(&h);
            let t = self.rtt.remove(&h).expect("listed");
            self.rtt_epoch += 1;
            self.mirror_stale.remove(&h);
            self.depth_level_tex.remove(&h);
            self.backend
                .destroy_surface(t.surface)
                .map_err(D3dError::Backend)?;
        }
        if let Some((_, s)) = self.other_surfaces.remove(&header) {
            self.backend.destroy_surface(s).map_err(D3dError::Backend)?;
        }
        if self.depth_color.is_some_and(|c| c.header == header) {
            self.depth_color = None;
            self.depth_color_newer = false;
        }
        self.cache.retain(|k, _| k.0 != header && k.1 != header);
        // A texture over the depth buffer holds a host copy of it.
        self.depth_alias.retain(|(h, _), _| *h != header);
        if !self.pending_destroys.contains(&header) {
            self.pending_destroys.push(header);
        }
        self.stats.destroys += 1;
        Ok(())
    }

    fn destroy_resources(
        &mut self,
        mem: &mut dyn GuestMemory,
        headers: Vec<u32>,
    ) -> Result<(), D3dError> {
        for h in headers {
            self.destroy_resource(mem, h)?;
        }
        Ok(())
    }

    /// Resources destroyed by the last entries, oldest first, for the
    /// original `D3D_DestroyResource` (`0x3fb520`, `EDI` = resource) to free.
    /// The runtime glue takes them after every entry and calls it for each
    /// before returning to the caller; a caller driving the device directly
    /// has to do the same.
    pub fn take_pending_destroys(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.pending_destroys)
    }

    /// `D3DResource_AddRef` (`0x3fb600`): bumps the title reference count.
    pub fn resource_add_ref(
        &mut self,
        mem: &mut dyn GuestMemory,
        header: u32,
    ) -> Result<u32, D3dError> {
        self.sync_push_buffer(mem)?;
        self.add_ref(mem, header)
    }

    /// `0x3fb600`: the first title reference to a texture level also takes
    /// one on its parent texture (`0x3fb607..0x3fb622`).
    fn add_ref(&mut self, mem: &mut dyn GuestMemory, header: u32) -> Result<u32, D3dError> {
        let c = mem.read_u32(header)?;
        if c & common::REFCOUNT_MASK == 0 {
            let parent = Self::surface_parent(mem, header)?;
            if parent != 0 {
                self.add_ref(mem, parent)?;
            }
        }
        let n = (c & common::REFCOUNT_MASK).wrapping_add(1) & common::REFCOUNT_MASK;
        mem.write_u32(header, (c & !common::REFCOUNT_MASK) | n)?;
        Ok(n)
    }

    /// `D3DResource_Release` (`0x3fb640`). As compiled: releasing the last
    /// title reference to a texture level releases its parent first
    /// (`0x3fb654..0x3fb668`), and a resource left with no internal
    /// reference either is destroyed and 0 returned (`0x3fb66d..0x3fb67b`,
    /// [`Device::destroy_resource`]). Otherwise the count drops by one.
    pub fn resource_release(
        &mut self,
        mem: &mut dyn GuestMemory,
        header: u32,
    ) -> Result<u32, D3dError> {
        self.sync_push_buffer(mem)?;
        self.release(mem, header)
    }

    fn release(&mut self, mem: &mut dyn GuestMemory, header: u32) -> Result<u32, D3dError> {
        const E: &str = "D3DResource_Release";
        let c = mem.read_u32(header)?;
        match c & common::REFCOUNT_MASK {
            0 => Err(D3dError::invalid(
                E,
                format!("{header:#010x} has no reference to release ({c:#010x})"),
            )),
            1 => {
                let parent = Self::surface_parent(mem, header)?;
                if parent != 0 {
                    self.release(mem, parent)?;
                }
                if c & common::INTREFCOUNT_MASK == 0 {
                    self.destroy_resource(mem, header)?;
                    return Ok(0);
                }
                mem.write_u32(header, c - 1)?;
                Ok(0)
            }
            n => {
                mem.write_u32(header, c - 1)?;
                Ok(n - 1)
            }
        }
    }

    /// Reports a guest store the device did not make, exactly: `len`
    /// bytes at `addr` were stored (see [`crate::dirty`]). A lazily held
    /// render-to-texture level takes those bytes from guest memory when
    /// it is merged, whatever their value.
    pub fn note_write(&mut self, addr: u32, len: u32) {
        self.tracker.note_write(addr, len);
        self.note_rtt_store(addr, len);
        self.note_mirror_store(addr, len);
    }

    /// Reports pages the guest may have stored into (the runtime's
    /// page-granular write barrier; may include the device's own stores).
    /// Uploads are refreshed as for [`note_write`](Self::note_write), but a
    /// lazily held render-to-texture level can only take the bytes that
    /// differ from what it last matched: a store of the value memory
    /// already held there is not seen (`docs/graphics/d3d.md`).
    pub fn note_written_pages(&mut self, addr: u32, len: u32) {
        self.tracker.note_write(addr, len);
    }

    /// `D3DDevice_SetTexture` (`0x3f8320`, `EAX` = stage).
    pub fn set_texture(
        &mut self,
        mem: &mut dyn GuestMemory,
        stage: u32,
        texture: u32,
    ) -> Result<(), D3dError> {
        const E: &str = "D3DDevice_SetTexture";
        self.live(E)?;
        self.sync_push_buffer(mem)?;
        if stage >= 4 {
            return Err(D3dError::invalid(E, format!("stage {stage}")));
        }
        let old = self.textures[stage as usize];
        if old == texture {
            return Ok(());
        }
        Self::add_internal_ref(mem, texture)?;
        self.drop_internal_ref(mem, E, old)?;
        self.textures[stage as usize] = texture;
        mem.write_u32(
            self.cfg.layout.device + field::TEXTURES + 4 * stage,
            texture,
        )?;
        Ok(())
    }

    /// `D3DDevice_SetPalette` (`0x3f84d0`, `EAX` = stage).
    pub fn set_palette(
        &mut self,
        mem: &mut dyn GuestMemory,
        stage: u32,
        palette: u32,
    ) -> Result<(), D3dError> {
        const E: &str = "D3DDevice_SetPalette";
        self.live(E)?;
        self.sync_push_buffer(mem)?;
        if stage >= 4 {
            return Err(D3dError::invalid(E, format!("stage {stage}")));
        }
        let old = self.palettes[stage as usize];
        if old != palette {
            Self::add_internal_ref(mem, palette)?;
            self.drop_internal_ref(mem, E, old)?;
            self.palettes[stage as usize] = palette;
        }
        Ok(())
    }

    /// `D3DDevice_SetStreamSource` (inlined/absent in Halo; kept for the API).
    pub fn set_stream_source(
        &mut self,
        mem: &mut dyn GuestMemory,
        stream: u32,
        buffer: u32,
        stride: u32,
    ) -> Result<(), D3dError> {
        const E: &str = "D3DDevice_SetStreamSource";
        self.live(E)?;
        self.sync_push_buffer(mem)?;
        if stream >= 16 {
            return Err(D3dError::invalid(E, format!("stream {stream}")));
        }
        let old = self.streams[stream as usize].buffer;
        if old != buffer {
            Self::add_internal_ref(mem, buffer)?;
            self.drop_internal_ref(mem, E, old)?;
        }
        self.streams[stream as usize] = StreamSource {
            buffer,
            stride,
            offset: 0,
        };
        let rec = self.cfg.layout.streams + 12 * stream;
        mem.write_u32(rec, stride)?;
        mem.write_u32(rec + 4, 0)?;
        mem.write_u32(rec + 8, buffer)?;
        Ok(())
    }

    /// `D3DDevice_SetVertexShader` (`0x3f7060`). Even handles are FVF codes,
    /// odd ones a vertex-shader object address plus one.
    pub fn set_vertex_shader(
        &mut self,
        mem: &mut dyn GuestMemory,
        handle: u32,
    ) -> Result<(), D3dError> {
        self.live("D3DDevice_SetVertexShader")?;
        self.sync_push_buffer(mem)?;
        self.vertex_shader = handle;
        Ok(())
    }

    /// `D3DDevice_SetVertexShaderInput` (`0x3f7440`, stdcall `Handle,
    /// StreamCount, pStreamInputs`). Halo's only vertex-binding path: it
    /// sets the shader and overrides `SetStreamSource` until called with 0.
    pub fn set_vertex_shader_input(
        &mut self,
        mem: &mut dyn GuestMemory,
        handle: u32,
        count: u32,
        inputs: u32,
    ) -> Result<(), D3dError> {
        const E: &str = "D3DDevice_SetVertexShaderInput";
        self.live(E)?;
        self.sync_push_buffer(mem)?;
        if count > 16 {
            return Err(D3dError::invalid(E, format!("{count} streams")));
        }
        let mut new = Vec::new();
        if handle != 0 {
            for i in 0..count {
                let w = crate::guest::read_array::<3>(mem, at(inputs, 12 * i as u64)?)?;
                new.push(StreamSource {
                    buffer: w[0],
                    stride: w[1],
                    offset: w[2],
                });
            }
            for s in &new {
                Self::add_internal_ref(mem, s.buffer)?;
            }
        }
        for s in self.stream_inputs.take().unwrap_or_default() {
            self.drop_internal_ref(mem, E, s.buffer)?;
        }
        if handle != 0 {
            self.vertex_shader = handle;
            self.stream_inputs = Some(new);
            if handle & 1 != 0 {
                self.select_shader_object(mem, handle)?;
            }
        }
        Ok(())
    }

    /// `D3DDevice_LoadVertexShaderProgram` (`0x3f6c60`, stdcall `pFunction,
    /// Address`): copies microcode into program memory. The function starts
    /// with a 4-byte header whose third byte is the instruction count
    /// (Cxbx `X_VSH_SHADER_HEADER`), followed by 4 dwords per instruction.
    pub fn load_vertex_shader_program(
        &mut self,
        mem: &mut dyn GuestMemory,
        function: u32,
        address: u32,
    ) -> Result<(), D3dError> {
        const E: &str = "D3DDevice_LoadVertexShaderProgram";
        self.live(E)?;
        self.sync_push_buffer(mem)?;
        let header = mem.read_u32(function)?;
        let n = (header >> 16) & 0xFF;
        if address as u64 + n as u64 > PROGRAM_SLOTS as u64 {
            return Err(D3dError::invalid(
                E,
                format!("{n} instructions at slot {address}"),
            ));
        }
        let words = mem.read_words(at(function, 4)?, 4 * n as usize)?;
        self.program[4 * address as usize..4 * (address + n) as usize].copy_from_slice(&words);
        Ok(())
    }

    /// `D3DDevice_SelectVertexShaderDirect` (`0x3f6cf0`): runs the program
    /// at `address` with the attribute format at `vaf` (guest
    /// `D3DVERTEXATTRIBUTEFORMAT`, 16 x 16 bytes; null keeps the current
    /// shader). As compiled, the format is copied into D3D's static shader
    /// object (`GuestLayout::static_vertex_shader`), which becomes current.
    pub fn select_vertex_shader_direct(
        &mut self,
        mem: &mut dyn GuestMemory,
        vaf: u32,
        address: u32,
    ) -> Result<(), D3dError> {
        const E: &str = "D3DDevice_SelectVertexShaderDirect";
        self.live(E)?;
        self.sync_push_buffer(mem)?;
        if address >= PROGRAM_SLOTS as u32 {
            return Err(D3dError::invalid(E, format!("slot {address}")));
        }
        self.program_start = address;
        if vaf != 0 {
            let obj = self.cfg.layout.static_vertex_shader;
            let words = mem.read_words(vaf, 64)?;
            for (i, w) in words.into_iter().enumerate() {
                mem.write_u32(obj + 0x14 + 4 * i as u32, w)?;
            }
            self.vertex_shader = obj | 1;
            self.select_shader_object(mem, obj | 1)?;
        }
        // `0x3f6d34`: SET_TRANSFORM_PROGRAM_START, mirrored at `+0x79C`.
        let dev = self.cfg.layout.device;
        let put = self.reserve(mem)?;
        Self::write_words(
            mem,
            put,
            &[0x0004_0000 | crate::hw::method::PROGRAM_START, address],
        )?;
        mem.write_u32(dev + 0x79C, address)?;
        mem.write_u32(dev + field::PUT, put + 8)?;
        Ok(())
    }

    /// `SetVertexShaderConstant1`/`1Fast`/`4` (`0x3f71d0`, `0x3f7230`,
    /// `0x3f7280`): `count` float4 registers from `register` (already biased
    /// by 96, 0..191). `shadow` says whether the original also updates the
    /// guest shadow at `0x407738` (`1` and `4` do, `1Fast` does not).
    pub fn set_vertex_shader_constant(
        &mut self,
        mem: &mut dyn GuestMemory,
        register: u32,
        data: u32,
        count: u32,
        shadow: bool,
    ) -> Result<(), D3dError> {
        const E: &str = "D3DDevice_SetVertexShaderConstant";
        if count as u64 * 4 > u32::MAX as u64 {
            return Err(D3dError::invalid(E, format!("{count} registers")));
        }
        self.constants_dwords(mem, E, register, data, count * 4, shadow)
    }

    /// `SetVertexShaderConstantNotInlineFast` (`0x3f7330`): `dwords` is a
    /// DWORD count (the original pushes them in batches of 16); writes start
    /// at the first component of `register` and may end mid-register. It
    /// does not update the guest shadow.
    pub fn set_vertex_shader_constant_dwords(
        &mut self,
        mem: &mut dyn GuestMemory,
        register: u32,
        data: u32,
        dwords: u32,
    ) -> Result<(), D3dError> {
        self.constants_dwords(
            mem,
            "D3DDevice_SetVertexShaderConstantNotInlineFast",
            register,
            data,
            dwords,
            false,
        )
    }

    /// `SetVertexShaderConstantNotInline` (`0x3f7410`; `EBX` = register +
    /// 96, `EDX` = data, `EAX` = DWORD count). Unless device flag `0x10` is
    /// set, it copies the dwords into the guest shadow at `0x407738` (`rep
    /// movsd` to `0x407738 + 16 * EBX`, `0x3f741b..0x3f742a`), then pushes
    /// them through `NotInlineFast` (`0x3f7431`). Halo calls it once, at
    /// `0x40d01`, with 20 dwords for register 81.
    pub fn set_vertex_shader_constant_not_inline(
        &mut self,
        mem: &mut dyn GuestMemory,
        register: u32,
        data: u32,
        dwords: u32,
    ) -> Result<(), D3dError> {
        const E: &str = "D3DDevice_SetVertexShaderConstantNotInline";
        self.live(E)?;
        let shadow = mem.read_u32(self.cfg.layout.device + field::FLAGS)? & 0x10 == 0;
        self.constants_dwords(mem, E, register, data, dwords, shadow)
    }

    fn constants_dwords(
        &mut self,
        mem: &mut dyn GuestMemory,
        entry: &'static str,
        register: u32,
        data: u32,
        dwords: u32,
        shadow: bool,
    ) -> Result<(), D3dError> {
        self.live(entry)?;
        self.sync_push_buffer(mem)?;
        let first = register as u64 * 4;
        if first + dwords as u64 > (CONSTANTS * 4) as u64 {
            return Err(D3dError::invalid(
                entry,
                format!("{dwords} dwords from register {register} overrun the 192 registers"),
            ));
        }
        let words = mem.read_words(data, dwords as usize)?;
        let first = first as usize;
        self.constants[first..first + words.len()].copy_from_slice(&words);
        self.constants_generation += 1;
        if shadow {
            let base = self.cfg.layout.vs_constants + 4 * first as u32;
            let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
            // A write checks its whole range before changing anything, so
            // when it is refused (and not recorded as a fault: `try_write`)
            // the per-word stores below write the words before the fault
            // and report the faulting word, as ever.
            if !mem.try_write(base, &bytes) {
                for (i, w) in words.iter().enumerate() {
                    mem.write_u32(base + 4 * i as u32, *w)?;
                }
            }
        }
        Ok(())
    }

    /// `D3DDevice_SetPixelShader` (`0x3fa3d0`, `EAX` = handle). The handle is
    /// an object `{RefCount, Flags, pPSDef}` (game code builds one inside the
    /// device at `+0x924`); 0 selects the fixed-function combiner.
    ///
    /// As compiled: device `+0x784` = handle. With a shader, `+0x788` =
    /// `PSDEF[8] | PSDEF[9]` (final-combiner inputs in use), `+0x78C` =
    /// `PSDEF[59] & 0x100`, `+0x790` = `PSDEF[54]` (PSTextureModes); and
    /// unless device flags bit `0x10` is set (`BehaviorFlags & 0x10`; Halo
    /// passes `0x40`, so it is clear), the first 57 PSDEF dwords are copied
    /// into render-state slots 0..57 and `PSDEF[54]` into slot 136.
    pub fn set_pixel_shader(
        &mut self,
        mem: &mut dyn GuestMemory,
        handle: u32,
    ) -> Result<(), D3dError> {
        self.live("D3DDevice_SetPixelShader")?;
        self.sync_push_buffer(mem)?;
        let dev = self.cfg.layout.device;
        self.pixel_shader = handle;
        mem.write_u32(dev + 0x784, handle)?;
        if handle == 0 {
            return Ok(());
        }
        let def_addr = mem.read_u32(at(handle, 8)?)?;
        let def = mem.read_words(def_addr, 60)?;
        mem.write_u32(dev + 0x788, def[8] | def[9])?;
        mem.write_u32(dev + 0x78C, def[59] & 0x100)?;
        mem.write_u32(dev + field::PS_TEXTURE_MODES, def[54])?;
        if mem.read_u32(dev + field::FLAGS)? & 0x10 == 0 {
            for (i, w) in def[..slot::PS_DEF_SLOTS as usize].iter().enumerate() {
                self.store_rs(mem, i as u32, *w)?;
            }
            self.store_rs(mem, slot::PS_TEXTURE_MODES, def[54])?;
        }
        Ok(())
    }

    /// `D3DDevice_SetTransform`: stored for a fixed-function vertex path.
    /// Halo never calls it from game code.
    pub fn set_transform(
        &mut self,
        mem: &mut dyn GuestMemory,
        state: u32,
        matrix: u32,
    ) -> Result<(), D3dError> {
        self.live("D3DDevice_SetTransform")?;
        self.sync_push_buffer(mem)?;
        let w = mem.read_words(matrix, 16)?;
        self.transforms
            .insert(state, std::array::from_fn(|i| f32::from_bits(w[i])));
        Ok(())
    }

    /// A stored transform.
    pub fn transform(&self, state: u32) -> Option<&[f32; 16]> {
        self.transforms.get(&state)
    }

    /// `D3DDevice_SetViewport` (`0x3f8100`, stdcall `pViewport`).
    ///
    /// As the original: a non-null viewport is clipped to the render target
    /// (right and bottom edges at most the target size, `0x3f8172..0x3f81ed`),
    /// stored at device `+0xEE0..+0xEF4` where Halo saves and restores it
    /// (`0x14ac0`), and marks dirty bit `0x100`. A null pointer keeps the
    /// current viewport (`0x3f8110` jumps past the stores). Either way the
    /// scissors are reset (`SetScissors(0, 0, 0)` at `0x3f82e2`).
    ///
    /// Recorded; a draw with a viewport that is not the whole target stops
    /// (gfx cannot apply one yet).
    pub fn set_viewport(
        &mut self,
        mem: &mut dyn GuestMemory,
        viewport: u32,
    ) -> Result<(), D3dError> {
        self.live("D3DDevice_SetViewport")?;
        self.sync_push_buffer(mem)?;
        if viewport != 0 {
            let w = mem.read_words(viewport, 6)?;
            let (tw, th) = self.target_size(mem)?;
            // The original keeps X and Y and clips the far edges; an origin
            // past the edge would wrap there, here it gives an empty extent.
            let right = (w[0] as u64 + w[2] as u64).min(tw as u64) as u32;
            let bottom = (w[1] as u64 + w[3] as u64).min(th as u64) as u32;
            let v = Viewport {
                x: w[0],
                y: w[1],
                width: right.saturating_sub(w[0]),
                height: bottom.saturating_sub(w[1]),
                min_z: f32::from_bits(w[4]),
                max_z: f32::from_bits(w[5]),
            };
            self.store_viewport(mem, v)?;
            let dirty = self.cfg.layout.dirty_flags;
            let d = mem.read_u32(dirty)?;
            mem.write_u32(dirty, d | 0x100)?;
        }
        // `0x3f82e2..0x3f830d`, also for a null viewport: SetScissors(0,
        // 0, 0), the composite update and the viewport push.
        self.scissors.clear();
        self.scissor_exclusive = false;
        self.update_projection_viewport(mem)?;
        let put = self.reserve(mem)?;
        let put = self.push_viewport(mem, put, self.cfg.layout.device)?;
        mem.write_u32(self.cfg.layout.device + field::PUT, put)?;
        Ok(())
    }

    /// Sets the viewport and mirrors it into device `+0xEE0`.
    fn store_viewport(&mut self, mem: &mut dyn GuestMemory, v: Viewport) -> Result<(), D3dError> {
        self.viewport = v;
        let words = [
            v.x,
            v.y,
            v.width,
            v.height,
            v.min_z.to_bits(),
            v.max_z.to_bits(),
        ];
        let base = self.cfg.layout.device + field::VIEWPORT;
        for (i, w) in words.iter().enumerate() {
            mem.write_u32(base + 4 * i as u32, *w)?;
        }
        Ok(())
    }

    /// The current viewport.
    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    /// The bound target as words: the colour and depth surfaces (guest
    /// header addresses, 0 for none) and the viewport (`x, y, width,
    /// height`, then `min_z` and `max_z` as bits). Equal words, the same
    /// target: what a caller compares when it skips work that would have
    /// set the target back (Halo's light passes,
    /// docs/graphics/native-renderer.md, stage 4).
    pub fn bound_target(&self) -> [u32; 8] {
        let v = self.viewport;
        [
            self.render_target,
            self.depth_stencil,
            v.x,
            v.y,
            v.width,
            v.height,
            v.min_z.to_bits(),
            v.max_z.to_bits(),
        ]
    }

    /// `D3DDevice_SetScissors` (`0x3f8760`, stdcall `Count, Exclusive,
    /// pRects`). Recorded for the draws' clip area
    /// ([`ResolvedDraw::clip`], [`ResolvedDraw::clip_rects`]). As compiled,
    /// no rectangles clips to the viewport (`0x3f87a7`); one inclusive
    /// rectangle becomes the surface clip (`0x3f886c`); anything else sets
    /// the NV2A window clip (`0x3f8928`: type = `Exclusive`, rectangle `i`
    /// in `SET_WINDOW_CLIP_HORIZONTAL/VERTICAL(i)` with inclusive maxima),
    /// which keeps pixels inside any rectangle, or outside all of them.
    pub fn set_scissors(
        &mut self,
        mem: &mut dyn GuestMemory,
        count: u32,
        exclusive: u32,
        rects: u32,
    ) -> Result<(), D3dError> {
        const E: &str = "D3DDevice_SetScissors";
        self.live(E)?;
        self.sync_push_buffer(mem)?;
        if count > MAX_SCISSORS {
            return Err(D3dError::invalid(E, format!("{count} rectangles")));
        }
        self.scissors.clear();
        self.scissor_exclusive = exclusive != 0;
        for i in 0..count {
            let w = mem.read_words(at(rects, 16 * i as u64)?, 4)?;
            self.scissors.push(Rect {
                x1: w[0] as i32,
                y1: w[1] as i32,
                x2: w[2] as i32,
                y2: w[3] as i32,
            });
        }
        Ok(())
    }

    /// `D3DDevice_SetBackBufferScale` (`0x3f9df0`). Only `1 x 1` is
    /// supported; anything else stops.
    pub fn set_back_buffer_scale(
        &mut self,
        mem: &mut dyn GuestMemory,
        x: f32,
        y: f32,
    ) -> Result<(), D3dError> {
        const E: &str = "D3DDevice_SetBackBufferScale";
        self.live(E)?;
        self.sync_push_buffer(mem)?;
        if x != 1.0 || y != 1.0 {
            return Err(D3dError::unimplemented(
                E,
                format!("back buffer scale {x} x {y}"),
            ));
        }
        Ok(())
    }

    // ---------------------------------------------------------------------
    // Render targets, clear, swap
    // ---------------------------------------------------------------------

    fn surface_for(
        &mut self,
        mem: &mut dyn GuestMemory,
        entry: &'static str,
        header: u32,
        depth: bool,
    ) -> Result<B::Surface, D3dError> {
        if header == self.back_header && !depth {
            return Ok(self.swap.as_ref().expect("created").back[0]);
        }
        if let Some(i) = self.frame_headers.iter().position(|&h| h == header)
            && !depth
        {
            // The display surface (or the second back buffer) as a target:
            // the frame buffer that header names now.
            return Ok(self.swap.as_ref().expect("created").header_order()[i + 1]);
        }
        if header == self.depth_header && depth {
            return self
                .depth
                .ok_or_else(|| D3dError::invalid(entry, "no depth buffer"));
        }
        let h = SurfaceHeader::read(mem, header)?;
        if h.pixels.resource.kind() != ResourceType::Surface {
            return Err(D3dError::invalid(
                entry,
                format!("{header:#010x} is not a surface"),
            ));
        }
        if h.parent != 0 {
            // A level of a texture: its own host surface, written back into
            // the level's memory when the guest can look (`raster.rs`).
            return self.rtt_surface(mem, entry, header, &h, depth);
        }
        let id = h.pixels.identity();
        if let Some((i, s)) = self.other_surfaces.get(&header)
            && *i == id
        {
            return Ok(*s);
        }
        let desc = crate::texdesc::from_header(h.pixels.format, h.pixels.size)
            .map_err(|e| D3dError::unimplemented(entry, format!("surface header: {e}")))?;
        let s = self
            .backend
            .create_surface(desc.format, desc.width, desc.height, depth)
            .map_err(D3dError::Backend)?;
        if let Some((_, old)) = self.other_surfaces.insert(header, (id, s)) {
            self.backend
                .destroy_surface(old)
                .map_err(D3dError::Backend)?;
        }
        self.note_depth_color(header, &h, &desc, depth);
        Ok(s)
    }

    fn header_desc(&self, mem: &dyn GuestMemory, header: u32) -> Result<TextureDesc, D3dError> {
        let h = PixelContainer::read(mem, header)?;
        crate::texdesc::from_header(h.format, h.size)
            .map_err(|e| D3dError::invalid("render target", e.to_string()))
    }

    fn target_size(&self, mem: &dyn GuestMemory) -> Result<(u32, u32), D3dError> {
        if self.render_target == self.back_header {
            let p = self.params.expect("created");
            return Ok((p.width, p.height));
        }
        let d = self.header_desc(mem, self.render_target)?;
        Ok((d.width, d.height))
    }

    fn target_info(&self, mem: &dyn GuestMemory) -> Result<TargetInfo, D3dError> {
        let c = self.header_desc(mem, self.render_target)?;
        let depth_format = if self.depth_stencil != 0 {
            Some(self.header_desc(mem, self.depth_stencil)?.format)
        } else {
            None
        };
        Ok(TargetInfo {
            width: c.width,
            height: c.height,
            format: c.format,
            depth_format,
        })
    }

    /// `D3DDevice_SetRenderTarget` (`0x3f7870`, stdcall `pRenderTarget,
    /// pNewZStencil`; a null target keeps the current one). A surface that
    /// is a texture level gets its own host surface, colour or
    /// depth/stencil (render-to-texture, `raster.rs`).
    pub fn set_render_target(
        &mut self,
        mem: &mut dyn GuestMemory,
        target: u32,
        z_stencil: u32,
    ) -> Result<(), D3dError> {
        const E: &str = "D3DDevice_SetRenderTarget";
        self.live(E)?;
        self.sync_push_buffer(mem)?;
        let target = if target == 0 {
            self.render_target
        } else {
            target
        };
        self.switch_targets(mem, E, target, z_stencil)?;
        let dev = self.cfg.layout.device;
        // `0x3f793f` (depth scale), `0x3f6741` (target size), `0x3f682a`
        // (pass-through reload), then `0x3f6836` tail-calls SetViewport
        // with `{0, 0, 0x7FFFFFFF, 0x7FFFFFFF, 0, 1}` (`0x403B88`), which
        // clips to the whole new target.
        self.store_target_fields(mem)?;
        self.load_passthrough(mem, dev)?;
        let (w, h) = self.target_size(mem)?;
        self.store_viewport(
            mem,
            Viewport {
                x: 0,
                y: 0,
                width: w,
                height: h,
                min_z: 0.0,
                max_z: 1.0,
            },
        )?;
        let dirty = self.cfg.layout.dirty_flags;
        let d = mem.read_u32(dirty)?;
        mem.write_u32(dirty, d | 0x100)?;
        self.scissors.clear();
        self.scissor_exclusive = false;
        self.update_projection_viewport(mem)?;
        let put = self.reserve(mem)?;
        let put = self.push_viewport(mem, put, dev)?;
        mem.write_u32(dev + field::PUT, put)?;
        Ok(())
    }

    /// The same-size SetRenderTarget (`0x3f7b30`: `[esp+4]` = target, `EBX`
    /// = depth/stencil, `ret 4`; one caller). Halo calls it only for a
    /// target the size and format of the current one, so, unlike
    /// `0x3f7870`, it touches neither the target-size fields, the depth
    /// scale, nor the viewport (a different size is not checked, as in the
    /// original: the old viewport stays): it swaps the two surface pointers (internal
    /// references moved through `0x3fb5a0`), pushes their offsets and the
    /// depth/stencil test enables (`0x3f7bd6..0x3f7c6a`), and clears device
    /// flag `0x100` when the target is the back buffer (`0x3f7c6d`). The
    /// target may not be null (the original dereferences it).
    pub fn set_render_target_same_size(
        &mut self,
        mem: &mut dyn GuestMemory,
        target: u32,
        z_stencil: u32,
    ) -> Result<(), D3dError> {
        const E: &str = "D3DDevice_SetRenderTarget (same size, 0x3f7b30)";
        self.live(E)?;
        self.sync_push_buffer(mem)?;
        if target == 0 {
            return Err(D3dError::invalid(E, "null render target"));
        }
        self.switch_targets(mem, E, target, z_stencil)?;
        let dev = self.cfg.layout.device;
        let flags = mem.read_u32(dev + field::FLAGS)?;
        if flags & 0x100 != 0 && target == mem.read_u32(dev + field::BACK_BUFFER)? {
            mem.write_u32(dev + field::FLAGS, flags & !0x100)?;
        }
        Ok(())
    }

    /// The target switch both SetRenderTarget paths share: checks both
    /// surfaces, writes a render-to-texture target being left back, and
    /// stores the device fields.
    fn switch_targets(
        &mut self,
        mem: &mut dyn GuestMemory,
        entry: &'static str,
        target: u32,
        z_stencil: u32,
    ) -> Result<(), D3dError> {
        self.surface_for(mem, entry, target, false)?;
        if z_stencil != 0 {
            self.surface_for(mem, entry, z_stencil, true)?;
        }
        // Internal references move with the binding (`0x3f7b53..0x3f7b82`
        // for the target, `0x3f7b87..0x3f7c29` for the depth buffer): the
        // new one is acquired before the old one is stamped and released.
        // Staged, so a release that stops leaves every count and the
        // binding as they were.
        let mut refs = Staged::default();
        self.acquire_surface(&mut refs, mem, target)?;
        self.release_surface(&mut refs, mem, entry, self.render_target)?;
        self.release_surface(&mut refs, mem, entry, self.depth_stencil)?;
        self.acquire_surface(&mut refs, mem, z_stencil)?;
        if target != self.render_target {
            self.leave_target(mem)?;
        }
        // Eagerly, a depth level is written back whenever either binding
        // changes, as the colour level is (a target change is a point where
        // the guest may look), after the colour level. Lazily it stays on
        // the host like a colour level, when the backend can sample it.
        if target != self.render_target || z_stencil != self.depth_stencil {
            let old = self.depth_stencil;
            if self.rtt_lazy_depth() {
                if z_stencil != old {
                    self.leave_depth(mem, old)?;
                }
            } else {
                self.write_back(mem, old, WritebackCause::DepthBinding)?;
            }
        }
        let destroys = refs.commit(mem)?;
        self.destroy_resources(mem, destroys)?;
        self.render_target = target;
        self.depth_stencil = z_stencil;
        let dev = self.cfg.layout.device;
        mem.write_u32(dev + field::RENDER_TARGET, target)?;
        mem.write_u32(dev + field::DEPTH_STENCIL, z_stencil)?;
        Ok(())
    }

    /// Whether `header` is a surface the title owns (not the device's own
    /// back and depth headers, which hold no references).
    fn title_surface(&self, header: u32) -> bool {
        header != 0
            && header != self.back_header
            && header != self.depth_header
            && !self.frame_headers.contains(&header)
    }

    /// The parent texture of surface `header`, if it is a texture level.
    fn surface_parent(mem: &dyn GuestMemory, header: u32) -> Result<u32, D3dError> {
        let c = mem.read_u32(header)?;
        if (c & common::TYPE_MASK) >> common::TYPE_SHIFT != ResourceType::Surface.raw() {
            return Ok(0);
        }
        Ok(mem.read_u32(at(header, 0x14)?)?)
    }

    /// Binding a surface: one internal reference, plus one on its parent
    /// texture when the surface held none (`0x3f7b38..0x3f7b60`).
    fn acquire_surface(
        &self,
        refs: &mut Staged,
        mem: &dyn GuestMemory,
        header: u32,
    ) -> Result<(), D3dError> {
        if !self.title_surface(header) {
            return Ok(());
        }
        let c = refs.read(mem, header)?;
        if c & common::INTREFCOUNT_MASK == 0 {
            let parent = Self::surface_parent(mem, header)?;
            if parent != 0 {
                let p = refs.read(mem, parent)?;
                refs.write(parent, p.wrapping_add(common::INTREFCOUNT_ONE));
            }
        }
        refs.write(header, c.wrapping_add(common::INTREFCOUNT_ONE));
        Ok(())
    }

    /// Unbinding a surface: stamps `Lock` (of the parent, for a level) with
    /// the GPU time (`0x3f7b67..0x3f7b79`), then drops the internal
    /// reference as `0x3fb5a0` does. The last internal reference also
    /// drops the parent's; a resource left with no references at all is
    /// staged for destruction (see [`Device::destroy_resource`]).
    ///
    /// Everything goes into `refs`; nothing is written unless the whole
    /// release can be done.
    fn release_surface(
        &self,
        refs: &mut Staged,
        mem: &dyn GuestMemory,
        entry: &'static str,
        header: u32,
    ) -> Result<(), D3dError> {
        if !self.title_surface(header) {
            return Ok(());
        }
        let parent = Self::surface_parent(mem, header)?;
        let time = mem.read_u32(self.cfg.layout.device + field::GPU_TIME)?;
        refs.write(at(if parent != 0 { parent } else { header }, 8)?, time);
        let c = refs.read(mem, header)?;
        match c & common::INTREFCOUNT_MASK {
            0 => {
                return Err(D3dError::invalid(
                    entry,
                    format!("surface {header:#010x} is bound without an internal reference"),
                ));
            }
            common::INTREFCOUNT_ONE => {
                // `0x3fb5b3..0x3fb5d9`: the parent's reference goes, and
                // with nothing left the parent is destroyed (the
                // decremented word is stored first, as the original does).
                if parent != 0 {
                    let p = refs
                        .read(mem, parent)?
                        .wrapping_sub(common::INTREFCOUNT_ONE);
                    refs.write(parent, p);
                    if p & (common::INTREFCOUNT_MASK | common::REFCOUNT_MASK) == 0 {
                        refs.destroy(parent);
                    }
                }
                // `0x3fb5de..0x3fb5e9`: no title reference either: destroyed
                // as it is, without the decrement.
                if c & common::REFCOUNT_MASK == 0 {
                    refs.destroy(header);
                    return Ok(());
                }
            }
            _ => {}
        }
        refs.write(header, c - common::INTREFCOUNT_ONE);
        Ok(())
    }

    fn current_targets(
        &mut self,
        mem: &mut dyn GuestMemory,
        entry: &'static str,
    ) -> Result<(B::Surface, Option<B::Surface>), D3dError> {
        let c = self.surface_for(mem, entry, self.render_target, false)?;
        let d = if self.depth_stencil != 0 {
            Some(self.surface_for(mem, entry, self.depth_stencil, true)?)
        } else {
            None
        };
        Ok((c, d))
    }

    /// `0x3f9a70`: each frame-buffer header takes the `Data` and `Lock` of
    /// the one before it, and the back buffer's header those of the last.
    fn rotate_frame_headers(&mut self, mem: &mut dyn GuestMemory) -> Result<(), D3dError> {
        let headers: Vec<u32> = std::iter::once(self.back_header)
            .chain(self.frame_headers.iter().copied())
            .collect();
        let words = headers
            .iter()
            .map(|&h| Ok((mem.read_u32(at(h, 4)?)?, mem.read_u32(at(h, 8)?)?)))
            .collect::<Result<Vec<_>, D3dError>>()?;
        for (i, &h) in headers.iter().enumerate() {
            let (data, lock) = words[(i + words.len() - 1) % words.len()];
            mem.write_u32(at(h, 4)?, data)?;
            mem.write_u32(at(h, 8)?, lock)?;
        }
        Ok(())
    }

    /// `D3DDevice_Swap` (`0x3f9d00`, `EAX` = flags). As compiled, flags 0 is
    /// replaced by 5 (`COPY | FINISH`). Returns the swap count, which is also
    /// stored at device `+0x2478` where game code reads it.
    ///
    /// The device's kind decides how, not the `COPY` bit alone:
    /// - **Flip** (created without [`COPY_SWAP_FLAG`]; Halo's device: no
    ///   multisampling, swap effect not `COPY`). With `FINISH` (4) the swap
    ///   sets a fence, counts and flips (`0x3f9d75` -> `0x3f9a70`),
    ///   `Swap(0)` included: `Data` and `Lock` rotate through the
    ///   frame-buffer headers, the display surface (`+0x1A18`) taking the
    ///   back buffer's and the back buffer (`+0x1A14`) the last header's.
    ///   The image just drawn is then the front, and the old front is drawn
    ///   into next. Then `0x3f9cc0` ([`Device::finish_swap`]). Without
    ///   `FINISH` the original returns the count and does nothing else
    ///   (`0x3f9d7a`), and so does this: nothing is presented.
    /// - **Copy** ([`COPY_SWAP_FLAG`] set). `COPY` (1) or `BYPASSCOPY` (2)
    ///   set a fence and count (`0x3f9d35..0x3f9d5e`); with three frame
    ///   buffers the display and third headers then exchange `Data` and
    ///   `Lock` (`0x3f991c..0x3f9944`), and with `COPY` back0 is copied
    ///   into the display buffer (`0x3f995d`; deferred here). With fewer
    ///   than three, scanout moves to the display buffer at once
    ///   (`0x3f998c..0x3f99d4`). With three it moves only on `FINISH`
    ///   (`0x3f9a16..0x3f9a57`), which does that even with neither copy
    ///   bit, and does not count. `FINISH` then runs `0x3f9cc0`.
    pub fn swap(&mut self, mem: &mut dyn GuestMemory, flags: u32) -> Result<u32, D3dError> {
        const E: &str = "D3DDevice_Swap";
        let _t = timeline::span(timeline::Cat::D3d, timeline::Track::Host, "Swap");
        self.live(E)?;
        self.sync_push_buffer(mem)?;
        if self.immediate.is_open() {
            return Err(D3dError::invalid(E, "Swap inside Begin/End"));
        }
        let flags = if flags == 0 { 5 } else { flags };
        let flip = self.swap.as_ref().expect("created").flip;
        let finish = flags & 4 != 0;
        if flip {
            if !finish {
                return Ok(self.swap_count);
            }
            self.flush_at_sync(mem)?;
            self.set_fence(mem)?;
            self.count_swap(mem)?;
            let sw = self.swap.as_mut().expect("created");
            let shown = sw.back[0];
            // The shown buffer becomes the front, the old front the last
            // back buffer.
            let old_front = std::mem::replace(&mut sw.front, shown);
            sw.back.remove(0);
            sw.back.push(old_front);
            self.rotate_frame_headers(mem)?;
            self.scan_out(flags)?;
            self.finish_swap(mem)?;
            return Ok(self.swap_count);
        }
        let three = self.frame_headers.len() > 1;
        if flags & 3 != 0 {
            self.flush_at_sync(mem)?;
            self.set_fence(mem)?;
            self.count_swap(mem)?;
            if three {
                // The display and third headers trade `Data`, and with them
                // the host surfaces. A copy still waiting in back0 for the
                // old front is made first, so the buffer that becomes the
                // third holds its image.
                let back0 = self.swap.as_ref().expect("created").back[0];
                self.before_target_write(back0)?;
                let sw = self.swap.as_mut().expect("created");
                std::mem::swap(&mut sw.front, &mut sw.back[1]);
                self.exchange_display_headers(mem)?;
            }
            if flags & 1 != 0 {
                // The copy into the display buffer, made before back0 is
                // next written (`Device::before_target_write`), so a frame
                // never drawn over costs nothing.
                self.swap.as_mut().expect("created").copy_pending = true;
            }
            if !three {
                self.scan_out(flags)?;
            }
        }
        if finish {
            if three {
                self.scan_out(flags)?;
            }
            self.finish_swap(mem)?;
        }
        Ok(self.swap_count)
    }

    /// Counts a swap (`+0x2478`) and the vertical blanks that follow it.
    fn count_swap(&mut self, mem: &mut dyn GuestMemory) -> Result<(), D3dError> {
        if timeline::phase::enabled() {
            self.backend.frame_stats();
        }
        timeline::frame();
        self.display.swapped = true;
        self.swap_count = self.swap_count.wrapping_add(1);
        if let Some(c) = &mut self.vertex_cache {
            c.end_frame();
        }
        if let Some(c) = &mut self.index_cache {
            c.end_frame();
        }
        mem.write_u32(self.cfg.layout.device + field::SWAP_COUNT, self.swap_count)?;
        self.sync_registers(mem)?;
        self.vblanks_after_swap(mem)
    }

    /// Scanout moves to what the display header names: the front, or back0
    /// while the copy into the front is pending.
    fn scan_out(&mut self, flags: u32) -> Result<(), D3dError> {
        let sw = self.swap.as_mut().expect("created");
        let shown = if sw.copy_pending {
            sw.back[0]
        } else {
            sw.front
        };
        sw.shown = shown;
        if sw.copy_pending || sw.flip {
            // A fresh image; a front a failed copy left stale stays lost.
            self.display.shown_lost = false;
        }
        self.backend
            .present_with_gamma(shown, flags, self.display.gamma.as_deref())
            .map_err(D3dError::Backend)
    }

    /// `0x3f9cc0`, at the end of a `FINISH` swap: the display header's
    /// `Lock` (its parent's, for a level) takes the GPU time, and a new
    /// fence is recorded at `+0x1974[swap count & 1]`.
    ///
    /// The order of stamp, fence and slot matches the original, the numbers
    /// do not: retail `D3D_SetFence` (`0x3fa69e..0x3fa73b`) returns the old
    /// GPU time and advances `+0x2C` by 2, while [`Device::set_fence`]
    /// advances it by 1 and returns the new time (the completion word
    /// always equals it, so no wait can stall). From zero, a flip's
    /// pre-flip fence plus this leave `Lock` 2, slot 2, `+0x2C` 4 on the
    /// Xbox and `Lock` 1, slot 2, `+0x2C` 2 here.
    fn finish_swap(&mut self, mem: &mut dyn GuestMemory) -> Result<(), D3dError> {
        let dev = self.cfg.layout.device;
        let display = mem.read_u32(dev + field::DISPLAY_SURFACE)?;
        let parent = mem.read_u32(at(display, 0x14)?)?;
        let target = if parent != 0 { parent } else { display };
        let time = mem.read_u32(dev + field::GPU_TIME)?;
        mem.write_u32(at(target, 8)?, time)?;
        let fence = self.set_fence(mem)?;
        mem.write_u32(dev + field::SWAP_FENCES + 4 * (self.swap_count & 1), fence)?;
        Ok(())
    }

    /// `0x3f991c..0x3f9944`: the display (`+0x1A18`) and third (`+0x1A1C`)
    /// headers exchange `Data` and `Lock`.
    fn exchange_display_headers(&mut self, mem: &mut dyn GuestMemory) -> Result<(), D3dError> {
        let (d, t) = (self.frame_headers[0], self.frame_headers[1]);
        for off in [4, 8] {
            let (a, b) = (mem.read_u32(at(d, off)?)?, mem.read_u32(at(t, off)?)?);
            mem.write_u32(at(d, off)?, b)?;
            mem.write_u32(at(t, off)?, a)?;
        }
        Ok(())
    }

    // ---------------------------------------------------------------------
    // Draws
    // ---------------------------------------------------------------------

    /// Builds the vertex layout and processing mode for the current shader,
    /// plus the stream bindings it reads.
    fn vertex_setup(
        &mut self,
        mem: &dyn GuestMemory,
        entry: &'static str,
    ) -> Result<(VertexLayout, VertexProcessing, Vec<StreamSource>), D3dError> {
        let h = self.vertex_shader;
        let bound = |i: usize| -> StreamSource {
            match &self.stream_inputs {
                Some(v) => v.get(i).copied().unwrap_or_default(),
                None => self.streams.get(i).copied().unwrap_or_default(),
            }
        };
        if h & 1 == 0 {
            if h == 0 {
                return Err(D3dError::invalid(entry, "no vertex shader set"));
            }
            let layout = VertexLayout::from_fvf(h)
                .map_err(|e| D3dError::unimplemented(entry, format!("FVF {h:#x}: {e}")))?;
            let streams = layout
                .streams
                .iter()
                .map(|s| bound(s.stream as usize))
                .collect();
            return Ok((layout, VertexProcessing::FixedFunction { fvf: h }, streams));
        }
        let obj = h & !1;
        let vaf = crate::guest::read_array::<64>(mem, at(obj, 0x14)?)?;
        // The streams and attributes are a function of the attribute
        // format words alone; only the strides come from the bound streams.
        if let Some(m) = &self.vertex_memo
            && m.vaf == vaf
        {
            let mut by_stream = m.streams.clone();
            for s in &mut by_stream {
                s.stride = bound(s.stream as usize).stride;
            }
            let streams = by_stream.iter().map(|s| bound(s.stream as usize)).collect();
            return Ok((
                VertexLayout {
                    streams: by_stream,
                    pretransformed: false,
                },
                VertexProcessing::Program {
                    start: self.program_start,
                    microcode: self.microcode(),
                },
                streams,
            ));
        }
        let mut by_stream: Vec<StreamLayout> = Vec::new();
        for reg in 0..16usize {
            let (stream, offset, format) =
                (vaf[4 * reg], vaf[4 * reg + 1], vaf[4 * reg + 2] & 0xFF);
            if format == 0x02 {
                continue; // D3DVSDT_NONE
            }
            if stream >= 16 {
                return Err(D3dError::invalid(
                    entry,
                    format!("vertex register {reg} reads stream {stream}"),
                ));
            }
            let data_type = VertexDataType::from_xbox(format).ok_or_else(|| {
                D3dError::unimplemented(
                    entry,
                    format!("vertex register {reg} data type {format:#x}"),
                )
            })?;
            let attr = VertexAttribute {
                register: VertexRegister(reg as u8),
                data_type,
                offset,
            };
            match by_stream.iter_mut().find(|s| s.stream == stream) {
                Some(s) => s.attributes.push(attr),
                None => by_stream.push(StreamLayout {
                    stream,
                    attributes: vec![attr],
                    stride: bound(stream as usize).stride,
                }),
            }
        }
        by_stream.sort_by_key(|s| s.stream);
        let streams = by_stream.iter().map(|s| bound(s.stream as usize)).collect();
        self.vertex_memo = Some(VertexMemo {
            vaf,
            streams: by_stream.clone(),
        });
        let layout = VertexLayout {
            streams: by_stream,
            pretransformed: false,
        };
        Ok((
            layout,
            VertexProcessing::Program {
                start: self.program_start,
                microcode: self.microcode(),
            },
            streams,
        ))
    }

    /// The program from `program_start` up to and including its FINAL
    /// instruction (or the last slot). While program memory holds the same
    /// words, every draw gets the same shared copy, so the shader keys built
    /// from consecutive draws compare by pointer and hash nothing again.
    /// Checked against program memory each time (one compare of the
    /// words), so nothing that writes program memory has to say so.
    fn microcode(&mut self) -> Microcode {
        let start = (self.program_start as usize).min(PROGRAM_SLOTS);
        let end = (start..PROGRAM_SLOTS)
            .find(|&s| self.program[4 * s + 3] & 1 != 0) // FINAL
            .map_or(PROGRAM_SLOTS, |s| s + 1);
        let words = &self.program[4 * start..4 * end];
        if let Some(m) = &self.microcode_memo
            && **m == *words
        {
            return m.clone();
        }
        // A program seen before gets the copy it had then, so keys made
        // for it (the judge's, the provider's) still compare by pointer
        // when the game switches back to it (A, B, A, ...).
        let m = if self.draw_sharing {
            let hash = Microcode::hash_words(words);
            match self
                .microcode_seen
                .iter()
                .find(|m| m.words_hash() == hash && ***m == *words)
            {
                Some(m) => m.clone(),
                None => {
                    let m = Microcode::new(words.into());
                    if self.microcode_seen.len() >= MICROCODE_SEEN {
                        self.microcode_seen.remove(0);
                    }
                    self.microcode_seen.push(m.clone());
                    m
                }
            }
        } else {
            Microcode::new(words.into())
        };
        self.microcode_memo = Some(m.clone());
        m
    }

    fn shader_inputs(
        &mut self,
        mem: &dyn GuestMemory,
        vertex: VertexProcessing,
        layout: &VertexLayout,
        words: &StateWords,
    ) -> Result<ShaderInputs, D3dError> {
        let provided = layout
            .streams
            .iter()
            .flat_map(|s| &s.attributes)
            .fold(0u16, |m, a| m | 1 << (a.register.0 & 15));
        let pixel_shader = if self.pixel_shader != 0 {
            let def = mem.read_u32(at(self.pixel_shader, 8)?)?;
            let words = crate::guest::read_array::<60>(mem, def)?;
            Some(Self::shared_words(&mut self.draw_pixel_shader, &words))
        } else {
            None
        };
        let combiner: &[u32; 57] = words.render[..57].try_into().expect("57 slots");
        Ok(ShaderInputs {
            vertex,
            provided_inputs: provided,
            vertex_defaults: *self.immediate.registers(),
            constants: self.draw_constants(),
            pixel_shader,
            combiner_state: Self::shared_words(&mut self.draw_combiner, combiner),
            final_combiner_latch: mem.read_u32(self.cfg.layout.device + 0x788)? != 0,
            affine_projection: mem.read_u32(self.cfg.layout.device + field::FLAGS)? & 0x02 != 0,
        })
    }

    /// Makes sure the host copy of the texture bound at `stage` is current;
    /// returns its cache key and description.
    fn resolve_texture(
        &mut self,
        mem: &dyn GuestMemory,
        entry: &'static str,
        header: u32,
        palette: u32,
        color_sign: u32,
    ) -> Result<(TextureKey, TextureInfo), D3dError> {
        let pc = PixelContainer::read(mem, header)?;
        if pc.resource.kind() != ResourceType::Texture {
            return Err(D3dError::unimplemented(
                entry,
                format!(
                    "binding a {:?} ({header:#010x}) as a texture",
                    pc.resource.kind()
                ),
            ));
        }
        let desc = crate::texdesc::from_header(pc.format, pc.size)
            .map_err(|e| D3dError::unimplemented(entry, format!("texture {header:#010x}: {e}")))?;
        let total = crate::texdesc::total_size(&desc)
            .map_err(|e| D3dError::unimplemented(entry, e.to_string()))?;
        if total as u64 > MAX_SPAN {
            return Err(D3dError::invalid(
                entry,
                format!("texture {header:#010x} of {total} bytes"),
            ));
        }
        let len = total as u32;
        let va = self.cfg.address_map.to_virtual(pc.resource.data);
        at(va, len as u64)?;
        let pal = if desc.format == D3DFormat::P8 {
            if palette == 0 {
                return Err(D3dError::invalid(
                    entry,
                    format!("P8 texture {header:#010x} without a palette"),
                ));
            }
            let p = PaletteHeader::read(mem, palette)?;
            if p.resource.kind() != ResourceType::Palette {
                return Err(D3dError::invalid(
                    entry,
                    format!("{palette:#010x} is not a palette"),
                ));
            }
            let pva = self.cfg.address_map.to_virtual(p.resource.data);
            let plen = 4 * p.entries();
            at(pva, plen as u64)?;
            Some(PaletteIdentity {
                header: palette,
                common: p.resource.common,
                va: pva,
                len: plen,
            })
        } else {
            None
        };
        let color_sign = texture_color_sign(desc.format, color_sign);
        let key = (header, pal.map_or(0, |p| p.header), color_sign);
        let info = TextureInfo {
            header,
            palette: key.1,
            format: desc.format,
            kind: desc.kind,
            width: desc.width,
            height: desc.height,
            depth: desc.depth,
            levels: desc.levels,
        };
        let identity = pc.identity();
        let policy = self.cfg.upload;
        let read_all = |mem: &dyn GuestMemory| -> Result<(Vec<u8>, Option<Vec<u8>>), D3dError> {
            let t = mem.read_vec(va, len as usize)?;
            let p = match pal {
                Some(p) => Some(mem.read_vec(p.va, p.len as usize)?),
                None => None,
            };
            Ok((t, p))
        };
        let hash_of = |t: &[u8], p: &Option<Vec<u8>>| {
            let mut all = t.to_vec();
            if let Some(p) = p {
                all.extend_from_slice(p);
            }
            hash_bytes(&all)
        };
        let mut prefetched = None;
        if let Some(c) = self.cache.get(&key) {
            let same = c.identity == identity && c.palette == pal && c.va == va && c.len == len;
            let clean = same
                && policy != UploadPolicy::Always
                && !self.tracker.dirty_since(va, len, c.generation)
                && pal.is_none_or(|p| !self.tracker.dirty_since(p.va, p.len, c.generation));
            if clean && policy == UploadPolicy::Verify {
                let bytes = read_all(mem)?;
                if hash_of(&bytes.0, &bytes.1) == c.hash {
                    self.stats.texture_hits += 1;
                    return Ok((key, info));
                }
                self.events.push(Event::MissedInvalidation {
                    header,
                    addr: va,
                    len,
                });
                prefetched = Some(bytes);
            } else if clean {
                self.stats.texture_hits += 1;
                return Ok((key, info));
            }
        }
        let (bytes, pal_bytes) = match prefetched {
            Some(b) => b,
            None => read_all(mem)?,
        };
        let palette_entries: Option<Vec<u32>> = pal_bytes.as_ref().map(|p| {
            p.as_chunks::<4>()
                .0
                .iter()
                .map(|c| u32::from_le_bytes(*c))
                .collect()
        });
        let decoded = decode_texture(
            &desc,
            &bytes,
            palette_entries.as_deref(),
            DecodeOptions {
                bc_supported: self.backend.bc_supported(),
                ..Default::default()
            }
            .with_color_sign(color_sign),
        )
        .map_err(|e| D3dError::unimplemented(entry, format!("texture {header:#010x}: {e}")))?;
        let host = self
            .backend
            .upload_texture(&decoded)
            .map_err(D3dError::Backend)?;
        self.tracker.watch(va, len);
        if let Some(p) = pal {
            self.tracker.watch(p.va, p.len);
        }
        let hash = if policy == UploadPolicy::Verify {
            hash_of(&bytes, &pal_bytes)
        } else {
            0
        };
        self.cache.insert(
            key,
            CachedTexture {
                identity,
                palette: pal,
                va,
                len,
                generation: self.tracker.generation(),
                hash,
                info,
                host,
            },
        );
        self.stats.texture_uploads += 1;
        Ok((key, info))
    }

    fn read_stream(
        &mut self,
        mem: &dyn GuestMemory,
        entry: &'static str,
        s: &StreamSource,
        first: u32,
        count: u32,
    ) -> Result<StreamBytes, D3dError> {
        if s.buffer == 0 {
            return Err(D3dError::invalid(entry, "draw from an unbound stream"));
        }
        let vb = ResourceHeader::read(mem, s.buffer)?;
        if vb.kind() != ResourceType::VertexBuffer {
            return Err(D3dError::invalid(
                entry,
                format!(
                    "{:#010x} is a {:?}, not a vertex buffer",
                    s.buffer,
                    vb.kind()
                ),
            ));
        }
        let len = span(entry, count, s.stride)?;
        let va = at(
            self.cfg.address_map.to_virtual(vb.data),
            s.offset as u64 + first as u64 * s.stride as u64,
        )?;
        at(va, len as u64)?;
        self.cached_stream(mem, s.buffer, va, len, s.stride)
    }

    /// [`copy_stream`](Self::copy_stream) through the vertex cache: a
    /// window whose pages are unwritten since it was last read comes back
    /// as the same shared snapshot. `header` names the vertex buffer in a
    /// [`Event::MissedInvalidation`].
    fn cached_stream(
        &mut self,
        mem: &dyn GuestMemory,
        header: u32,
        va: u32,
        len: usize,
        stride: u32,
    ) -> Result<StreamBytes, D3dError> {
        let policy = self.cfg.upload;
        let Some(cache) = self
            .vertex_cache
            .as_mut()
            .filter(|_| policy != UploadPolicy::Always)
        else {
            return self.copy_stream(mem, va, len, stride);
        };
        crate::guest::check_range(va, len)?;
        let len32 = len as u32;
        match cache.probe(va, len32, &self.tracker) {
            Probe::Hit(snapshot) => {
                if policy == UploadPolicy::Verify {
                    let copy = self.copy_stream(mem, va, len, stride)?;
                    if *copy.data != *snapshot.bytes {
                        self.events.push(Event::MissedInvalidation {
                            header,
                            addr: va,
                            len: len32,
                        });
                        let generation = self.tracker.generation();
                        if let Some(c) = &mut self.vertex_cache {
                            c.invalidate(va, len32, generation);
                        }
                        return Ok(copy);
                    }
                    self.recycle_stream(copy.data);
                }
                self.stats.vertex_cache_hits += 1;
                self.stats.vertex_bytes_reused += len as u64;
                count_vertex_bytes(timeline::phase::Counter::VertexReused, len);
                Ok(StreamBytes {
                    data: StreamData::Cached(snapshot),
                    stride,
                })
            }
            Probe::Fill => {
                let copy = self.copy_stream(mem, va, len, stride)?;
                let bytes: Arc<[u8]> = Arc::from(&*copy.data);
                self.recycle_stream(copy.data);
                self.tracker.watch(va, len32);
                let generation = self.tracker.generation();
                let cache = self.vertex_cache.as_mut().expect("probed above");
                let snapshot = cache.fill(va, len32, generation, |id| CachedBytes { id, bytes });
                self.stats.vertex_cache_fills += 1;
                Ok(StreamBytes {
                    data: StreamData::Cached(snapshot),
                    stride,
                })
            }
            Probe::Copy => {
                self.tracker.watch(va, len32);
                self.copy_stream(mem, va, len, stride)
            }
        }
    }

    fn copy_stream(
        &mut self,
        mem: &dyn GuestMemory,
        va: u32,
        len: usize,
        stride: u32,
    ) -> Result<StreamBytes, D3dError> {
        crate::guest::check_range(va, len)?;
        let mut data = self.stream_buffer(len);
        if let Err(e) = mem.read_append(va, len, &mut data) {
            self.recycle_stream(data.into());
            return Err(e.into());
        }
        self.stats.vertex_bytes_copied += len as u64;
        count_vertex_bytes(timeline::phase::Counter::VertexCopied, len);
        Ok(StreamBytes {
            data: data.into(),
            stride,
        })
    }

    /// Turns the vertex cache on (with `budget` bytes of snapshots) or off
    /// (every draw copies its streams: `H2_VERTEX_CACHE=0`). Starts empty.
    pub fn set_vertex_cache(&mut self, budget: Option<usize>) {
        self.vertex_cache = budget.map(VertexCache::new);
        self.index_cache = budget.map(|b| IndexCache::new(b / 4));
    }

    /// Bytes the vertex cache holds in snapshots (0 when it is off).
    pub fn vertex_cache_bytes(&self) -> usize {
        self.vertex_cache.as_ref().map_or(0, VertexCache::bytes)
    }

    /// An empty pooled buffer with room for `len` bytes: the smallest that
    /// fits, since a draw's streams differ in size and growing a buffer
    /// copies its old bytes. Without one, a new buffer replaces the
    /// largest, so the pool's buffers grow to the draws' sizes.
    fn stream_buffer(&mut self, len: usize) -> Vec<u8> {
        let i = self.stream_pool.partition_point(|b| b.capacity() < len);
        if i < self.stream_pool.len() {
            let mut b = self.stream_pool.remove(i);
            self.stream_pool_bytes -= b.capacity();
            b.clear();
            return b;
        }
        if let Some(b) = self.stream_pool.pop() {
            self.stream_pool_bytes -= b.capacity();
        }
        Vec::with_capacity(len)
    }

    fn recycle_stream(&mut self, data: StreamData) {
        // Retain ordinary draws without keeping an exceptional 64 MiB span.
        const MAX_POOL_BYTES: usize = 16 << 20;
        const MAX_POOL_BUFFERS: usize = 64;
        // A cache snapshot is shared, not the device's to refill.
        let Some(data) = data.into_owned_buffer() else {
            return;
        };
        let c = data.capacity();
        if c > 0
            && self.stream_pool.len() < MAX_POOL_BUFFERS
            && c <= MAX_POOL_BYTES - self.stream_pool_bytes
        {
            let i = self.stream_pool.partition_point(|b| b.capacity() < c);
            self.stream_pool_bytes += c;
            self.stream_pool.insert(i, data);
        }
    }

    fn recycle_streams(&mut self, mut streams: Vec<StreamBytes>) {
        for s in streams.drain(..) {
            self.recycle_stream(s.data);
        }
        self.stream_scratch = streams;
    }

    fn submit(&mut self, mem: &mut dyn GuestMemory, mut parts: DrawParts) -> Result<(), D3dError> {
        let mut streams = std::mem::take(&mut parts.streams);
        let result = self.submit_inner(mem, parts, &mut streams);
        self.recycle_streams(streams);
        result
    }

    fn submit_inner(
        &mut self,
        mem: &mut dyn GuestMemory,
        parts: DrawParts,
        streams: &mut Vec<StreamBytes>,
    ) -> Result<(), D3dError> {
        let entry = parts.entry;
        if parts.vertex_count == 0 {
            return Ok(());
        }
        let Some((clip, clip_rects)) = self.draw_clip(mem)? else {
            // The viewport and scissors leave nothing to draw into.
            return Ok(());
        };
        if !clip_rects.is_empty() {
            self.stats.scissor_split_draws += 1;
        }
        let mut keys = [None; 4];
        let mut texture_info = [None; 4];
        let mut glyph_cache = 0u8;
        let mut snapshots: [Option<B::Texture>; 4] = Default::default();
        let mut aliases: [Option<raster::DepthAliasKey>; 4] = [None; 4];
        let mut depth_levels: [Option<u32>; 4] = [None; 4];
        let mut bound = self.textures;
        let shader = self.shader_inputs(mem, parts.vertex, &parts.layout, &parts.words)?;
        let state = self.translate_state(&parts.words);
        let sampled = sampled_stages(&shader, &parts.words, &state);
        let mut extras = parts.words.stage_extras();
        for stage in 0..4 {
            let header = self.textures[stage];
            if header != 0 {
                match self.frame_texture(mem, header, sampled[stage], extras[stage].color_sign)? {
                    FrameTexture::No => {}
                    FrameTexture::Snapshot(info, t) => {
                        // Snapshots are not stored biased, so COLORSIGN
                        // cannot apply to them (docs/graphics/shaders.md).
                        extras[stage].color_sign = 0;
                        texture_info[stage] = Some(info);
                        snapshots[stage] = Some(t);
                        continue;
                    }
                    FrameTexture::DepthAlias(info, key) => {
                        extras[stage].color_sign = key.1;
                        texture_info[stage] = Some(info);
                        aliases[stage] = Some(key);
                        continue;
                    }
                    FrameTexture::Skip(reason) if sampled[stage] => {
                        self.stats.draws += 1;
                        self.events.push(Event::DrawSkipped { entry, reason });
                        return Ok(());
                    }
                    // A stage the draw does not read: left unbound.
                    FrameTexture::Skip(_) => {
                        bound[stage] = 0;
                        continue;
                    }
                }
                if let Some((info, t)) = self.rtt_texture(mem, header, extras[stage].color_sign)? {
                    texture_info[stage] = Some(info);
                    snapshots[stage] = Some(t);
                    continue;
                }
                if let Some((info, level)) =
                    self.rtt_depth_texture(mem, header, extras[stage].color_sign)?
                {
                    texture_info[stage] = Some(info);
                    depth_levels[stage] = Some(level);
                    continue;
                }
                self.flush_for_texture(mem, header, WritebackCause::Sample)?;
                let (k, i) = self.resolve_texture(
                    mem,
                    entry,
                    header,
                    self.palettes[stage],
                    extras[stage].color_sign,
                )?;
                extras[stage].color_sign = k.2;
                keys[stage] = Some(k);
                texture_info[stage] = Some(i);
                if self.glyph_smoothing && crate::glyph::is_glyph_cache(mem, &i) {
                    glyph_cache |= 1 << stage;
                }
            }
        }
        let target = self.target_info(mem)?;
        let (color, depth) = self.current_targets(mem, entry)?;
        let writes_depth = writes_depth_or_stencil(&state.render, target.depth_format);
        // The presented image is kept before anything draws over it; what
        // the draw dirties is noted only once it has drawn.
        self.before_target_write(color)?;
        for b in &state.bad {
            self.events.push(Event::BadState(b.clone()));
        }
        let draw = ResolvedDraw {
            entry,
            primitive: parts.primitive,
            vertex_count: parts.vertex_count,
            layout: parts.layout,
            streams: std::mem::take(streams),
            indices: parts.indices,
            state,
            extras,
            raw: parts.words,
            shader,
            viewport: self.viewport,
            clip: Some(clip),
            clip_rects,
            hw: self.hw,
            target,
            textures: bound.map(|t| (t != 0).then_some(t)),
            texture_info,
            glyph_cache,
        };
        let textures: [Option<&B::Texture>; 4] = std::array::from_fn(|i| {
            snapshots[i]
                .as_ref()
                .or_else(|| {
                    aliases[i]
                        .and_then(|k| self.depth_alias.get(&k))
                        .map(|a| &a.texture)
                })
                .or_else(|| depth_levels[i].and_then(|h| self.depth_level_tex.get(&h)))
                .or_else(|| keys[i].and_then(|k| self.cache.get(&k)).map(|c| &c.host))
        });
        self.stats.draws += 1;
        let (outcome, spare) = self.backend.draw_owned(color, depth, draw, textures);
        *streams = spare;
        match outcome.map_err(D3dError::Backend)? {
            DrawOutcome::Drawn => self.mark_written(writes_depth),
            DrawOutcome::Skipped(reason) => self.events.push(Event::DrawSkipped { entry, reason }),
            DrawOutcome::Unsupported(detail) => {
                return Err(D3dError::Unimplemented { entry, detail });
            }
        }
        Ok(())
    }

    /// Whether draws mark the stages that sample the game's glyph cache
    /// ([`ResolvedDraw::glyph_cache`], `crate::glyph`), so a backend that
    /// renders above the guest's size redraws text sharp. On by default;
    /// `H2_SMOOTH_TEXT=1` turns it on; off by default. Nothing changes at 1x either way.
    pub fn set_glyph_smoothing(&mut self, on: bool) {
        self.glyph_smoothing = on;
    }

    /// Whether draws share the pushed state snapshot and earlier microcode
    /// copies when they read the same words (on by default;
    /// `H2_D3D_DRAW_SHARING=0` turns it off). Changes which allocations
    /// back a draw, never what it holds or what any backend sees.
    pub fn set_draw_sharing(&mut self, on: bool) {
        self.draw_sharing = on;
        self.microcode_seen.clear();
    }

    /// Info about the host texture cached for `header` (with `palette` for
    /// P8), if any, under any `COLORSIGN`.
    pub fn cached_texture(&self, header: u32, palette: u32) -> Option<TextureInfo> {
        self.cache
            .iter()
            .find(|(k, _)| k.0 == header && k.1 == palette)
            .map(|(_, c)| c.info)
    }

    /// [`StateWords::translate`] through the memo: consecutive draws mostly
    /// see the same arrays.
    fn translate_state(&mut self, words: &StateWords) -> DrawState {
        self.translated.translate(&self.translator, words)
    }

    fn state(&self, mem: &dyn GuestMemory) -> Result<StateWords, D3dError> {
        Ok(StateWords::read(mem, &self.cfg.layout)?)
    }

    /// State for an API draw: D3D commits all pending state first, so the
    /// live arrays are what the GPU sees, and they become the pushed state.
    /// The draw and the pushed snapshot share the words until either side
    /// changes (a setter copies the snapshot then, `Arc::make_mut`).
    /// When the arrays still hold the pushed snapshot's words (no state
    /// changed since the last draw or commit), the draw shares that
    /// snapshot instead of a new copy.
    fn draw_state(&mut self, mem: &dyn GuestMemory) -> Result<Arc<StateWords>, D3dError> {
        let s = self.state(mem)?;
        if self.draw_sharing
            && let Some(p) = &self.pushed
            && **p == s
        {
            return Ok(p.clone());
        }
        let s = Arc::new(s);
        self.pushed = Some(s.clone());
        Ok(s)
    }

    fn primitive(entry: &'static str, v: u32) -> Result<PrimitiveType, D3dError> {
        PrimitiveType::from_xbox(v)
            .ok_or_else(|| D3dError::invalid(entry, format!("primitive type {v}")))
    }

    /// `D3DDevice_DrawVertices` (`0x3faed0`: `EAX` = VertexCount, `ECX` =
    /// StartVertex, stack PrimitiveType).
    pub fn draw_vertices(
        &mut self,
        mem: &mut dyn GuestMemory,
        primitive: u32,
        start: u32,
        count: u32,
    ) -> Result<(), D3dError> {
        const E: &str = "D3DDevice_DrawVertices";
        self.live(E)?;
        self.sync_push_buffer(mem)?;
        let primitive = Self::primitive(E, primitive)?;
        vertex_limit(E, count)?;
        let (layout, vertex, sources) = self.vertex_setup(mem, E)?;
        let mut streams = std::mem::take(&mut self.stream_scratch);
        for s in &sources {
            match self.read_stream(mem, E, s, start, count) {
                Ok(stream) => streams.push(stream),
                Err(e) => {
                    self.recycle_streams(streams);
                    return Err(e);
                }
            }
        }
        let words = match self.draw_state(mem) {
            Ok(words) => words,
            Err(e) => {
                self.recycle_streams(streams);
                return Err(e);
            }
        };
        self.submit(
            mem,
            DrawParts {
                entry: E,
                primitive,
                vertex_count: count,
                layout,
                vertex,
                streams,
                indices: None,
                words,
            },
        )
    }

    /// `D3DDevice_DrawIndexedVertices` (`0x3fafa0`, stdcall `PrimitiveType,
    /// VertexCount, pIndexData`: 16-bit indices in guest memory, offset by
    /// the base vertex at device `+0x1C`). The streams hold the vertices the
    /// indices reach; the indices go to the backend rebased onto them.
    pub fn draw_indexed_vertices(
        &mut self,
        mem: &mut dyn GuestMemory,
        primitive: u32,
        count: u32,
        indices: u32,
    ) -> Result<(), D3dError> {
        const E: &str = "D3DDevice_DrawIndexedVertices";
        self.live(E)?;
        self.sync_push_buffer(mem)?;
        let primitive = Self::primitive(E, primitive)?;
        vertex_limit(E, count)?;
        let base = mem.read_u32(self.cfg.layout.device + field::BASE_VERTEX)?;
        let index_bytes = span(E, count, 2)?;
        if index_bytes < 2 {
            crate::guest::check_range(indices, index_bytes)?;
            return Ok(());
        }
        let (lo16, hi16, list) = self.index_list(mem, indices, index_bytes)?;
        // Every index overflows alike or none does: the largest decides.
        let overflow = || D3dError::invalid(E, format!("base vertex {base:#x} + index overflows"));
        let hi = u32::try_from(base as u64 + hi16 as u64).map_err(|_| overflow())?;
        let lo = base + lo16 as u32;
        let (layout, vertex, sources) = self.vertex_setup(mem, E)?;
        // The vertices `lo..=hi` the indices reach (at most 65536: 16-bit
        // indices), with the indices rebased onto them.
        let window = hi - lo + 1;
        let mut streams = std::mem::take(&mut self.stream_scratch);
        for s in &sources {
            match self.read_stream(mem, E, s, lo, window) {
                Ok(stream) => streams.push(stream),
                Err(e) => {
                    self.recycle_streams(streams);
                    return Err(e);
                }
            }
        }
        let words = match self.draw_state(mem) {
            Ok(words) => words,
            Err(e) => {
                self.recycle_streams(streams);
                return Err(e);
            }
        };
        self.submit(
            mem,
            DrawParts {
                entry: E,
                primitive,
                vertex_count: window,
                layout,
                vertex,
                streams,
                indices: Some(list),
                words,
            },
        )
    }

    /// The 16-bit index list `[va, va + len)`: its smallest and largest
    /// index and the list rebased by the smallest, through the index cache
    /// when `va` is in the alias the write tracker reports (the contiguous
    /// view; a list elsewhere is read every time).
    fn index_list(
        &mut self,
        mem: &dyn GuestMemory,
        va: u32,
        len: usize,
    ) -> Result<(u16, u16, IndexData), D3dError> {
        fn scan(raw: &[u8]) -> (u16, u16, Vec<u32>) {
            let (mut lo, mut hi) = (u16::MAX, 0u16);
            for c in raw.as_chunks::<2>().0 {
                let i = u16::from_le_bytes(*c);
                lo = lo.min(i);
                hi = hi.max(i);
            }
            let rebased = raw
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| (u16::from_le_bytes(*c) - lo) as u32)
                .collect();
            (lo, hi, rebased)
        }
        let map = self.cfg.address_map;
        let tracked = map.to_virtual(map.to_data(va)) == va;
        let policy = self.cfg.upload;
        let Some(cache) = self
            .index_cache
            .as_mut()
            .filter(|_| tracked && policy != UploadPolicy::Always)
        else {
            let (lo, hi, list) = scan(&mem.read_vec(va, len)?);
            return Ok((lo, hi, list.into()));
        };
        crate::guest::check_range(va, len)?;
        let len32 = len as u32;
        match cache.probe(va, len32, &self.tracker) {
            Probe::Hit(snapshot) => {
                if policy == UploadPolicy::Verify {
                    let (lo, hi, list) = scan(&mem.read_vec(va, len)?);
                    if (lo, hi) != (snapshot.lo, snapshot.hi) || *list != *snapshot.rebased {
                        self.events.push(Event::MissedInvalidation {
                            header: 0,
                            addr: va,
                            len: len32,
                        });
                        let generation = self.tracker.generation();
                        if let Some(c) = &mut self.index_cache {
                            c.invalidate(va, len32, generation);
                        }
                        return Ok((lo, hi, list.into()));
                    }
                }
                self.stats.index_cache_hits += 1;
                Ok((snapshot.lo, snapshot.hi, IndexData::Cached(snapshot)))
            }
            Probe::Fill => {
                let (lo, hi, list) = scan(&mem.read_vec(va, len)?);
                self.tracker.watch(va, len32);
                let generation = self.tracker.generation();
                let cache = self.index_cache.as_mut().expect("probed above");
                let snapshot = cache.fill(va, len32, generation, |id| CachedIndices {
                    id,
                    lo,
                    hi,
                    rebased: Arc::from(list),
                });
                Ok((lo, hi, IndexData::Cached(snapshot)))
            }
            Probe::Copy => {
                self.tracker.watch(va, len32);
                let (lo, hi, list) = scan(&mem.read_vec(va, len)?);
                Ok((lo, hi, list.into()))
            }
        }
    }

    /// `D3DDevice_DrawVerticesUP`: vertices for stream 0 straight from guest
    /// memory. Not present as a function in Halo's XBE; kept for the API.
    pub fn draw_vertices_up(
        &mut self,
        mem: &mut dyn GuestMemory,
        primitive: u32,
        count: u32,
        data: u32,
        stride: u32,
    ) -> Result<(), D3dError> {
        const E: &str = "D3DDevice_DrawVerticesUP";
        self.live(E)?;
        self.sync_push_buffer(mem)?;
        let primitive = Self::primitive(E, primitive)?;
        vertex_limit(E, count)?;
        let (layout, vertex, _) = self.vertex_setup(mem, E)?;
        if layout.streams.len() != 1 || layout.streams[0].stream != 0 {
            return Err(D3dError::invalid(E, "UP draws use exactly stream 0"));
        }
        let stream = self.copy_stream(mem, data, span(E, count, stride)?, stride)?;
        let mut streams = std::mem::take(&mut self.stream_scratch);
        streams.push(stream);
        let words = match self.draw_state(mem) {
            Ok(words) => words,
            Err(e) => {
                self.recycle_streams(streams);
                return Err(e);
            }
        };
        self.submit(
            mem,
            DrawParts {
                entry: E,
                primitive,
                vertex_count: count,
                layout,
                vertex,
                streams,
                indices: None,
                words,
            },
        )
    }

    /// `D3DDevice_Begin` (`0x3fb440`). It calls the deferred-state commit
    /// first, so the current arrays become the pushed state.
    pub fn begin(&mut self, mem: &mut dyn GuestMemory, primitive: u32) -> Result<(), D3dError> {
        const E: &str = "D3DDevice_Begin";
        self.live(E)?;
        self.sync_push_buffer(mem)?;
        let p = Self::primitive(E, primitive)?;
        self.refresh_pushed(mem)?;
        self.immediate.begin(p, 0)?;
        Ok(())
    }

    /// `D3DDevice_SetVertexData*`: `register` -1 means position
    /// (`SET_VERTEX4F`; the 4f form only).
    pub fn set_vertex_data(
        &mut self,
        mem: &mut dyn GuestMemory,
        register: i32,
        values: [f32; 4],
    ) -> Result<(), D3dError> {
        const E: &str = "D3DDevice_SetVertexData";
        self.live(E)?;
        self.sync_push_buffer(mem)?;
        let reg = match register {
            -1 => 0,
            0..=15 => register as usize,
            _ => return Err(D3dError::invalid(E, format!("register {register}"))),
        };
        if self.immediate.is_open() && reg == 0 && self.immediate_vertices() >= MAX_VERTICES {
            return Err(D3dError::invalid(E, "too many immediate-mode vertices"));
        }
        self.immediate.set(0, reg, &values, 0, true)?;
        Ok(())
    }

    fn immediate_vertices(&self) -> u32 {
        self.immediate.vertex_count() as u32
    }

    /// `D3DDevice_End` (`0x3fb480`).
    pub fn end(&mut self, mem: &mut dyn GuestMemory) -> Result<(), D3dError> {
        const E: &str = "D3DDevice_End";
        self.live(E)?;
        self.sync_push_buffer(mem)?;
        let d = self.immediate.end(0)?;
        self.immediate_draw(mem, E, d)
    }

    /// Turns an assembled immediate-mode primitive into a draw, against the
    /// pushed state.
    fn immediate_draw(
        &mut self,
        mem: &mut dyn GuestMemory,
        entry: &'static str,
        d: ImmediateDraw,
    ) -> Result<(), D3dError> {
        if !d.inline.is_empty() {
            return self.inline_array_draw(mem, entry, d);
        }
        let (layout, vertex) = if self.vertex_shader & 1 == 0 {
            let (l, v, _) = self.vertex_setup(mem, entry)?;
            (l, v)
        } else {
            // Programs read registers: give every register as float4.
            let attributes = (0..16u8)
                .filter(|r| d.written & (1 << r) != 0)
                .map(|r| VertexAttribute {
                    register: VertexRegister(r),
                    data_type: VertexDataType::Float4,
                    offset: 16 * r as u32,
                })
                .collect();
            let l = VertexLayout {
                streams: vec![StreamLayout {
                    stream: 0,
                    attributes,
                    stride: 256,
                }],
                pretransformed: false,
            };
            (
                l,
                VertexProcessing::Program {
                    start: self.program_start,
                    microcode: self.microcode(),
                },
            )
        };
        let mut streams = std::mem::take(&mut self.stream_scratch);
        for s in &layout.streams {
            let stride = s.stride as usize;
            let mut data = self.stream_buffer(span(entry, d.vertices.len() as u32, s.stride)?);
            for v in &d.vertices {
                let start = data.len();
                data.resize(start + stride, 0);
                for a in &s.attributes {
                    let reg = v[(a.register.0 & 15) as usize];
                    let o = start + a.offset as usize;
                    let size = a.data_type.byte_size() as usize;
                    if a.offset as usize + size > stride {
                        return Err(D3dError::invalid(
                            entry,
                            format!("attribute at offset {} past stride {stride}", a.offset),
                        ));
                    }
                    match a.data_type {
                        VertexDataType::Float1
                        | VertexDataType::Float2
                        | VertexDataType::Float3
                        | VertexDataType::Float4 => {
                            for c in 0..size / 4 {
                                data[o + 4 * c..o + 4 * c + 4]
                                    .copy_from_slice(&reg[c].to_le_bytes());
                            }
                        }
                        VertexDataType::D3dColor => {
                            // Register (r, g, b, a) -> D3DCOLOR bytes B, G, R, A.
                            let q = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
                            data[o..o + 4].copy_from_slice(&[
                                q(reg[2]),
                                q(reg[1]),
                                q(reg[0]),
                                q(reg[3]),
                            ]);
                        }
                        other => {
                            return Err(D3dError::unimplemented(
                                entry,
                                format!("immediate-mode vertex attribute type {other:?}"),
                            ));
                        }
                    }
                }
            }
            streams.push(StreamBytes {
                data: data.into(),
                stride: s.stride,
            });
        }
        let words = self.pushed_state(mem)?;
        self.submit(
            mem,
            DrawParts {
                entry,
                primitive: d.primitive,
                vertex_count: d.vertices.len() as u32,
                layout,
                vertex,
                streams,
                indices: None,
                words,
            },
        )
    }
}

impl<B: Backend> Device<B> {
    /// A `BEGIN_END`/`INLINE_ARRAY`/`BEGIN_END` primitive (`0x4b220` after
    /// BeginPush `0x3fae10`): the vertices are the inline words, unpacked
    /// with the `SET_VERTEX_DATA_ARRAY_FORMAT` registers as the NV2A does
    /// (the xemu model): the enabled registers in register order, each
    /// aligned to its component size and packed with no gaps. The format's
    /// stride field is not used. Drawn against the pushed state, like the
    /// other inline draws.
    fn inline_array_draw(
        &mut self,
        mem: &mut dyn GuestMemory,
        entry: &'static str,
        d: ImmediateDraw,
    ) -> Result<(), D3dError> {
        let (attributes, size) = inline_array_layout(&self.hw.vertex_format)
            .map_err(|e| D3dError::unimplemented(entry, e))?;
        let bytes = d.inline.len() as u32 * 4;
        if size == 0 {
            // No enabled register: the formats were never pushed.
            return Err(D3dError::invalid(
                entry,
                format!("{bytes} inline-array bytes with every vertex array format disabled"),
            ));
        }
        // Whole vertices only; a trailing partial one is dropped, as the
        // NV2A model does (xemu, `pgraph_gl_bind_inline_array`: the count is
        // the byte count over the vertex size).
        let count = bytes / size;
        if count == 0 {
            return Ok(());
        }
        vertex_limit(entry, count)?;
        let data: Vec<u8> = d
            .inline
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .take((count * size) as usize)
            .collect();
        // The attribute registers keep the last vertex's values, which a
        // later draw with the register disabled reads (xemu carries them
        // the same way in `bind_vertex_attributes`).
        let last = &data[((count - 1) * size) as usize..];
        for a in &attributes {
            let o = a.offset as usize;
            let value =
                attribute_value(a.data_type, &last[o..o + a.data_type.byte_size() as usize]);
            self.immediate.carry(a.register.0 as usize & 15, value);
        }
        let vertex = if self.vertex_shader & 1 == 0 {
            if self.vertex_shader == 0 {
                return Err(D3dError::invalid(entry, "no vertex shader set"));
            }
            VertexProcessing::FixedFunction {
                fvf: self.vertex_shader,
            }
        } else {
            VertexProcessing::Program {
                start: self.program_start,
                microcode: self.microcode(),
            }
        };
        let pretransformed = self.vertex_shader & 1 == 0
            && self.vertex_shader & gfx::vertex::fvf::POSITION_MASK == gfx::vertex::fvf::XYZRHW;
        let layout = VertexLayout {
            streams: vec![StreamLayout {
                stream: 0,
                attributes,
                stride: size,
            }],
            pretransformed,
        };
        let words = self.pushed_state(mem)?;
        self.stats.inline_array_draws += 1;
        self.submit(
            mem,
            DrawParts {
                entry,
                primitive: d.primitive,
                vertex_count: count,
                layout,
                vertex,
                streams: vec![StreamBytes {
                    data: data.into(),
                    stride: size,
                }],
                indices: None,
                words,
            },
        )
    }
}

/// One attribute of guest vertex data as the float4 the vertex unit
/// reads: missing components are `(0, 0, 0, 1)`, normalised types are
/// scaled as the formats say.
pub(crate) fn attribute_value(ty: VertexDataType, b: &[u8]) -> [f32; 4] {
    use VertexDataType as T;
    let f = |i: usize| f32::from_le_bytes(b[4 * i..4 * i + 4].try_into().expect("4 bytes"));
    let s = |i: usize| i16::from_le_bytes([b[2 * i], b[2 * i + 1]]);
    let sn = |v: i32, max: f32| (v as f32 / max).max(-1.0);
    let mut out = [0.0, 0.0, 0.0, 1.0];
    match ty {
        T::Float1 | T::Float2 | T::Float3 | T::Float4 => {
            for (i, o) in out.iter_mut().enumerate().take(ty.byte_size() as usize / 4) {
                *o = f(i);
            }
        }
        T::Float2H => out = [f(0), f(1), 0.0, f(2)],
        T::D3dColor => out = [b[2], b[1], b[0], b[3]].map(|x| x as f32 / 255.0),
        T::PByte1 | T::PByte2 | T::PByte3 | T::PByte4 => {
            for (o, x) in out.iter_mut().zip(b) {
                *o = *x as f32 / 255.0;
            }
        }
        T::Short1 | T::Short2 | T::Short3 | T::Short4 => {
            for (i, o) in out.iter_mut().enumerate().take(b.len() / 2) {
                *o = s(i) as f32;
            }
        }
        T::NormShort1 | T::NormShort2 | T::NormShort3 | T::NormShort4 => {
            for (i, o) in out.iter_mut().enumerate().take(b.len() / 2) {
                *o = sn(s(i) as i32, 32767.0);
            }
        }
        T::NormPacked3 => {
            let w = u32::from_le_bytes(b[..4].try_into().expect("4 bytes"));
            let x = ((w << 21) as i32) >> 21;
            let y = ((w << 10) as i32) >> 21;
            let z = (w as i32) >> 22;
            out = [sn(x, 1023.0), sn(y, 1023.0), sn(z, 511.0), 1.0];
        }
    }
    out
}

/// The attributes of an inline-array vertex and its size in bytes, from the
/// 16 `SET_VERTEX_DATA_ARRAY_FORMAT` registers (see
/// [`Device::inline_array_draw`]).
pub(crate) fn inline_array_layout(
    formats: &[u32; 16],
) -> Result<(Vec<VertexAttribute>, u32), String> {
    let mut attributes = Vec::new();
    let mut offset = 0u32;
    for (reg, &f) in formats.iter().enumerate() {
        let format = f & 0xFF;
        if (format >> 4) & 0xF == 0 {
            continue; // no components: disabled
        }
        let data_type = VertexDataType::from_xbox(format)
            .ok_or_else(|| format!("vertex register {reg} array format {format:#x}"))?;
        let component = match format & 0xF {
            0 | 4 => 1, // UB_D3D, UB_OGL
            1 | 5 => 2, // S1, S32K
            _ => 4,     // F, CMP
        };
        offset = offset.next_multiple_of(component);
        attributes.push(VertexAttribute {
            register: VertexRegister(reg as u8),
            data_type,
            offset,
        });
        offset = (offset + data_type.byte_size()).next_multiple_of(component);
    }
    Ok((attributes, offset))
}

/// Whether a draw with render state `r` can change the bound depth/stencil
/// surface (of format `depth`): depth writes with the depth test on, or a
/// stencil test that can store something (the format has stencil, the
/// write mask is not zero, and some outcome's operation is not `KEEP`). A
/// read-only test changes nothing.
fn writes_depth_or_stencil(r: &gfx::RenderState, depth: Option<D3DFormat>) -> bool {
    use gfx::state::{DepthBufferMode, StencilOp};
    let Some(f) = depth else {
        return false;
    };
    let z = r.depth_mode != DepthBufferMode::Disabled && r.depth_write;
    let s = &r.stencil;
    let has_stencil = matches!(
        f,
        D3DFormat::D24S8 | D3DFormat::LinD24S8 | D3DFormat::F24S8 | D3DFormat::LinF24S8
    );
    let stencil = s.enable
        && has_stencil
        && s.write_mask != 0
        && [s.fail, s.depth_fail, s.pass]
            .iter()
            .any(|op| *op != StencilOp::Keep);
    z || stencil
}

/// Depth textures feed raw words to shadow comparison. Only colour
/// textures store signed channels biased for the shader's signed transfer.
fn texture_color_sign(format: D3DFormat, color_sign: u32) -> u32 {
    if format.is_depth() {
        0
    } else {
        color_sign & COLOR_SIGN_MASK
    }
}

/// The linear twin of a swizzled surface format (render targets are linear).
fn linear_twin(f: D3DFormat) -> D3DFormat {
    match f {
        D3DFormat::A8R8G8B8 => D3DFormat::LinA8R8G8B8,
        D3DFormat::X8R8G8B8 => D3DFormat::LinX8R8G8B8,
        D3DFormat::R5G6B5 => D3DFormat::LinR5G6B5,
        D3DFormat::X1R5G5B5 => D3DFormat::LinX1R5G5B5,
        D3DFormat::A1R5G5B5 => D3DFormat::LinA1R5G5B5,
        D3DFormat::A4R4G4B4 => D3DFormat::LinA4R4G4B4,
        D3DFormat::D24S8 => D3DFormat::LinD24S8,
        D3DFormat::D16 => D3DFormat::LinD16,
        D3DFormat::F24S8 => D3DFormat::LinF24S8,
        D3DFormat::F16 => D3DFormat::LinF16,
        other => other,
    }
}

/// The texture stages whose image a draw reads: with a pixel shader, the
/// stages whose `PSTEXTUREMODES` mode (slot 136, five bits a stage)
/// samples a texture (`TexMode::texture_dim`); for the
/// fixed-function combiner, the stages before the first `COLOROP =
/// DISABLE` (where [`crate::nv2a::fixed_function_def`] stops).
pub(crate) fn sampled_stages(
    shader: &ShaderInputs,
    words: &StateWords,
    state: &crate::state::DrawState,
) -> [bool; 4] {
    if shader.pixel_shader.is_some() {
        // Only modes that fetch a texel read the image: CLIPPLANE (texkill),
        // PASSTHRU and the dot-only modes do not.
        let tm = words.render[slot::PS_TEXTURE_MODES as usize];
        return std::array::from_fn(|n| {
            shaders::psh::TexMode::from_bits((tm >> (5 * n)) & 0x1F)
                .texture_dim()
                .is_some()
        });
    }
    let mut on = true;
    std::array::from_fn(|n| {
        on &= state.stages[n].color_op != gfx::state::TextureOp::Disable;
        on
    })
}

/// The vertex and index caches are on unless `H2_VERTEX_CACHE=0`.
fn vertex_cache_on() -> bool {
    std::env::var_os("H2_VERTEX_CACHE").is_none_or(|v| v != "0")
}

/// Adds `len` to a frame-stats byte counter (when frame stats are on).
fn count_vertex_bytes(counter: timeline::phase::Counter, len: usize) {
    timeline::phase::count(counter, len as u64);
}

#[cfg(test)]
mod depth_write_tests {
    use super::*;
    use gfx::state::{DepthBufferMode, StencilOp};

    /// Review P3: a stencil test that can store nothing does not change the
    /// depth/stencil surface.
    #[test]
    fn only_storing_stencil_tests_and_depth_writes_count() {
        let d24 = Some(D3DFormat::LinD24S8);
        let mut r = gfx::RenderState {
            depth_mode: DepthBufferMode::Z,
            depth_write: false,
            ..Default::default()
        };
        assert!(!writes_depth_or_stencil(&r, d24));
        r.depth_write = true;
        assert!(writes_depth_or_stencil(&r, d24));
        assert!(!writes_depth_or_stencil(&r, None), "nothing bound");
        r.depth_mode = DepthBufferMode::Disabled;
        assert!(!writes_depth_or_stencil(&r, d24), "no test, no write");
        // Stencil on with every operation KEEP: read-only.
        r.stencil.enable = true;
        assert!(!writes_depth_or_stencil(&r, d24));
        r.stencil.pass = StencilOp::Replace;
        assert!(writes_depth_or_stencil(&r, d24));
        // A zero write mask, or a format without stencil, stores nothing.
        r.stencil.write_mask = 0;
        assert!(!writes_depth_or_stencil(&r, d24));
        r.stencil.write_mask = 0xFF;
        assert!(!writes_depth_or_stencil(&r, Some(D3DFormat::LinD16)));
        r.stencil.pass = StencilOp::Keep;
        r.stencil.depth_fail = StencilOp::Zero;
        assert!(writes_depth_or_stencil(&r, Some(D3DFormat::F24S8)));
    }
}
