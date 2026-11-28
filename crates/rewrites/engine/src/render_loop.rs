//! Port-grade reads of the rasterizer front-end (docs/engine/feature-engine.md,
//! "7. Rasterizer front-end"; the read is in docs/graphics/render-loop.md):
//! the addresses and record layouts the native renderer reads, and the
//! shader draw's render-state list ([`states`]) as a state machine the
//! native world's walk runs. [`states::RenderStates::start`] and
//! [`states::RenderStates::resume`] run the list up to its next guest call
//! and return [`Step::Call`] with the CPU set up exactly as the original
//! sets it up at that `call`; the driver makes the call and hands the CPU
//! back. The differential test (`tests/states_differential.rs`) checks it
//! against the lifted original.
//!
//! Stage 7a also ported `render_views`, the layer draw and its entry loop,
//! `render_game_view` and `render_scene_passes` as placed implementations.
//! None saved measurable time and each cost a hooked call; they were
//! removed (docs/graphics/render-loop.md).
//!
//! Every address here is a research point from the read; no game data.

pub mod frame_wait;
pub mod layer;
pub mod world_lightmap;
pub mod world_registers;
pub mod opaque_texture;
pub mod opaque_material;
pub mod opaque_animation;
pub mod structure_parameters;
pub mod world_walk;
pub mod occlusion;
pub mod states;
pub mod tick_writes;

/// The registers a port sets and reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Regs {
    pub eax: u32,
    pub ecx: u32,
    pub edx: u32,
    pub ebx: u32,
    pub esp: u32,
    pub ebp: u32,
    pub esi: u32,
    pub edi: u32,
    pub xmm0: u128,
    /// Carried through.
    pub xmm1: u128,
}

/// What the port wants next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Call guest `target` with the registers as they are. `site` is the
    /// original's return address (for logs; the driver pushes its own).
    /// The callee pops `cleanup` bytes of arguments.
    Call {
        target: u32,
        site: u32,
        cleanup: u32,
    },
    /// The port returned: ESP is past the return address and the `ret n`
    /// arguments, EIP is `to`.
    Return { to: u32 },
}

/// The callees of `render_views` (`0x0002b5d0`), by the original's names
/// where known.
pub mod views {
    /// Called on entry when `0x0047989c` is non-zero, and on exit again.
    pub const DEBUG_BEGIN: u32 = 0x0014_6de0;
    pub const DEBUG_END: u32 = 0x0014_6b80;
    /// Begin-frame setup; EAX points at a 0x18-byte frame record.
    pub const FRAME_BEGIN: u32 = 0x0001_2a50;
    /// A view record's predicate (AL).
    pub const VIEW_TEST: u32 = 0x0002_5820;
    /// Takes the view's `+4` in EAX before the game view.
    pub const VIEW_PREPARE: u32 = 0x0002_0720;
    /// `render_game_view(view, 0)`, stdcall.
    pub const GAME_VIEW: u32 = 0x0002_b790;
    /// The alternate view (`[view] != 0`), stdcall with the view.
    pub const ALT_VIEW: u32 = 0x0002_c790;
    /// End of drawing (two calls).
    pub const END_A: u32 = 0x0001_3b90;
    pub const END_B: u32 = 0x0001_3cd0;
    /// Size of one view record.
    pub const VIEW_STRIDE: u32 = 0x118;
}
