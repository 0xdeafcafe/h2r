//! Named guest locations: the one table of hook points and globals.
//!
//! Every guest function a feature hooks and every global more than one
//! module reads is named here once, keyed by its address in the base (1.0)
//! image, with the page that records the evidence. Code elsewhere refers to
//! the name (`points::GAME_TICK.base`), never to a second copy of the
//! number. This is the anchor list docs/runtime/title-updates.md section 3.3 asks
//! for; a title update maps these through `ImageAddrs` (docs/engine/feature-engine.md
//! "Version key").
//!
//! No game data here: addresses and argument locations are research notes,
//! and each carries a doc reference.
//!
//! ```text
//! points! {
//!     /// The game tick.
//!     GAME_TICK @ 0x0013_7fe0: [fn() ret 0], hot; "docs/engine/engine-map.md";
//!     /// Not read yet.
//!     SOMETHING @ 0x0012_3450: [opaque]; "docs/x.md";
//! }
//! ```

pub mod ai_gates;
pub mod game_types;
pub mod render_aspect;
pub mod scene_effects;
pub mod skull_gates;

/// Vertical pixels per projection unit in the current retail view.
pub const PIXEL_SCALE: u32 = 0x004b_9ed0;

/// Where one argument or the result lives at the function's entry or exit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loc {
    Eax,
    Al,
    Ecx,
    Edx,
    Ebx,
    Esi,
    Edi,
    Xmm0,
    /// The n-th dword argument on the stack, 0-based: `[esp + 4 + 4n]` at
    /// entry (the return address is at `[esp]`).
    Stack(u8),
}

/// A point's calling convention, as far as it has been read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sig {
    /// Not read: observers and after-hooks only, never typed arguments.
    Opaque,
    /// An instruction boundary inside a function (a researched resume
    /// point), not a call: no arguments, and a before-hook may only
    /// continue or resume a researched transfer, never return a value.
    Site,
    Fn {
        args: &'static [(&'static str, Loc)],
        ret: Option<Loc>,
        /// Bytes the callee pops (`ret n`).
        pop: u16,
    },
}

impl Sig {
    pub fn is_opaque(&self) -> bool {
        matches!(self, Sig::Opaque)
    }
    /// The location of the named argument.
    pub fn arg(&self, name: &str) -> Option<Loc> {
        match self {
            Sig::Opaque | Sig::Site => None,
            Sig::Fn { args, .. } => args.iter().find(|(n, _)| *n == name).map(|(_, l)| *l),
        }
    }
}

/// Whether an image without this point can still run the features on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// A hook on it is required: missing, its feature is turned off.
    Required,
    /// A hook on it is dropped with a note when missing.
    Optional,
}

/// Flags the `points!` macro takes after the signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flag {
    /// Called thousands of times a frame: a hook leaves the direct-call fast
    /// path, and the plan warns.
    Hot,
    /// [`Class::Optional`].
    Optional,
}

/// A named guest function.
#[derive(Debug, PartialEq, Eq)]
pub struct Point {
    pub name: &'static str,
    /// Address in the base (1.0) image.
    pub base: u32,
    pub sig: Sig,
    pub class: Class,
    pub hot: bool,
    /// The page that records the evidence.
    pub doc: &'static str,
}

impl Point {
    pub const fn new(name: &'static str, base: u32, sig: Sig, doc: &'static str) -> Self {
        Point {
            name,
            base,
            sig,
            class: Class::Required,
            hot: false,
            doc,
        }
    }
    pub const fn with_flags(mut self, flags: &[Flag]) -> Self {
        let mut i = 0;
        while i < flags.len() {
            match flags[i] {
                Flag::Hot => self.hot = true,
                Flag::Optional => self.class = Class::Optional,
            }
            i += 1;
        }
        self
    }
}

/// Beginning and end of the index build within the sorted dispatcher.
pub const TRANSPARENT_ORDER_BEGIN: Point = Point::new(
    "TRANSPARENT_ORDER_BEGIN",
    0x0001_e381,
    Sig::Site,
    "docs/graphics/native-renderer.md",
);
pub const TRANSPARENT_ORDER_READY: Point = Point::new(
    "TRANSPARENT_ORDER_READY",
    0x0001_e3bc,
    Sig::Site,
    "docs/graphics/native-renderer.md",
);

/// Return sites on the retail depleting damage report's kill-credit path.
/// Used to identify that caller's stack layout at PLAYER_KILLED entry.
pub mod kill_credit {
    pub const RETURN_TO_ATTRIBUTION: u32 = 0x001e_a465;
    pub const RETURN_TO_DAMAGE_REPORT: u32 = 0x000d_9acf;
}

/// The game's announcer queue (docs/audio/audio.md, "The announcer
/// queue"): up to five 12-byte lines from `QUEUE`,
/// each a sound tag (`+0`), the playing sound (`+4`), a playing byte
/// (`+8`) and an i16 delay in ticks (`+0xa`); the head is the line playing
/// or next. `0x0023_ef80` queues a game event's sound through
/// `0x0023_f140`, which drops a line when five wait.
pub mod announcer_queue {
    /// The head line; the others follow it, 12 bytes apart.
    pub const QUEUE: u32 = 0x0050_2350;
    /// The head's playing byte: set when its sound starts, cleared by the
    /// sound's end callback (`0x0023_f120`).
    pub const HEAD_PLAYING: u32 = 0x0050_2358;
    /// The head's delay, i16 ticks: counted down to -1, which plays it.
    pub const HEAD_DELAY: u32 = 0x0050_235a;
    /// The lines queued, the head included (i32).
    pub const COUNT: u32 = 0x0050_23ec;
}

/// Return addresses for the retail lobby leave and party leader queries.
pub mod party_leave {
    pub const LOBBY_BACK_COUNT: u32 = 0x0025_11c5;
    pub const PAUSE_LEAVE_COUNT: u32 = 0x0023_29c8;
    pub const LEADER_ROW_LABEL: u32 = 0x002b_8415;
    pub const LEADER_ROW_BUILD: u32 = 0x002b_8459;
    pub const LEADER_ROW_PRESS: u32 = 0x002b_84c0;
    pub const LEADER_AUTO_SELECT: u32 = 0x0019_b4f2;
    pub const LEADER_APPOINT: u32 = 0x0019_a1a0;
}

/// The object section loop's call to the shared record writer.
pub const OBJECT_RECORD_APPEND: Point = Point::new(
    "OBJECT_RECORD_APPEND",
    0x0013_41d0,
    Sig::Site,
    "docs/graphics/native-renderer.md",
);
/// Return from that call, routed through the generated resume table.
pub const OBJECT_RECORD_APPENDED: u32 = 0x0013_41d5;

/// The geometry submitter's transparent-only append branch and epilogue.
pub const TRANSPARENT_GEOMETRY_ENTRY: Point = Point::new(
    "TRANSPARENT_GEOMETRY_ENTRY",
    0x0004_1381,
    Sig::Site,
    "docs/graphics/native-renderer.md",
);
pub const TRANSPARENT_GEOMETRY_QUEUED: Point = Point::new(
    "TRANSPARENT_GEOMETRY_QUEUED",
    0x0004_1477,
    Sig::Site,
    "docs/graphics/native-renderer.md",
);

/// Entry points of the UI render walk. The feature asks image analysis
/// to expose the interior sites before lifting.
pub mod ui_render {
    use super::{Point, Sig};
    pub use super::{
        UI_FADE_DRAW as FADE_DRAW, UI_HUD_DRAW as HUD_DRAW, UI_LOADING_DRAW as LOADING_DRAW,
        UI_OVERLAY_RECT_DRAW as OVERLAY_RECT_DRAW,
    };

    const DOC: &str = "docs/graphics/native-renderer.md";
    pub const BACKDROP_WASH: Point = Point::new("UI_BACKDROP_WASH", 0x0025_4b95, Sig::Site, DOC);
    pub const BACKDROP_WASH_TAIL: u32 = 0x0025_4dc0;
    pub const BACKDROP_TINT: Point = Point::new("UI_BACKDROP_TINT", 0x0025_4e28, Sig::Site, DOC);
    pub const BACKDROP_TINT_TAIL: u32 = 0x0025_4ee0;
    pub const BACKDROP_COMPOSITE: Point =
        Point::new("UI_BACKDROP_COMPOSITE", 0x0025_5083, Sig::Site, DOC);
    pub const BACKDROP_COMPOSITE_TAIL: u32 = 0x0025_5191;
    pub const MODEL_WIDGET_DRAW: u32 = 0x002b_0baa;
    pub const MODEL_VIEW_CALL: Point =
        Point::new("UI_MODEL_VIEW_CALL", 0x002b_0e12, Sig::Site, DOC);
    pub const MODEL_VIEW_CONTINUATION: u32 = 0x002b_0e17;
    pub const MODEL_VIEW_RETURN: Point =
        Point::new("UI_MODEL_VIEW_RETURN", 0x002b_0e1b, Sig::Site, DOC);
    pub const MODEL_GEOMETRY_CALL: Point =
        Point::new("UI_MODEL_GEOMETRY_CALL", 0x0004_d621, Sig::Site, DOC);
    pub const MODEL_GEOMETRY_RETURN: u32 = 0x0004_d626;
    pub const MODEL_GEOMETRY_EMITTER: u32 = 0x0004_daa0;
    pub const MODEL_TEXTURE_COMMIT: u32 = 0x0001_cf50;
    pub const ITEM_CONTINUATION: u32 = 0x0023_5448;
    /// `0x002353a5` with its widget list sorted, the screen's pre-draw
    /// callback run and the depth window at `[ebp-8]`, `[ebp-0xc]`:
    /// before its first widget draw. Reached only with widgets listed.
    pub const LIST_READY: Point = Point::new("UI_LIST_READY", 0x0023_5428, Sig::Site, DOC);
    /// `0x002353a5`'s draws done, before its post-draw callback: the
    /// empty list and the end of the loop join here.
    pub const LIST_DONE: Point = Point::new("UI_LIST_DONE", 0x0023_545c, Sig::Site, DOC);
    pub const OUTLINE_VERTICES: Point =
        Point::new("UI_OUTLINE_VERTICES", 0x0001_ed98, Sig::Site, DOC);
    pub const OUTLINE_VERTICES_TAIL: u32 = 0x0001_ee3a;
    pub const OUTLINE_SUBMIT_RETURN: u32 = 0x0023_5e55;
    pub const LAYERS_RETURN: Point = Point::new("UI_LAYERS_RETURN", 0x0014_7cb9, Sig::Site, DOC);
    pub const ITEM_CALL: Point = Point::new("UI_ITEM_CALL", 0x0023_5445, Sig::Site, DOC);
    pub const ITEM_RETURN: Point = Point::new("UI_ITEM_RETURN", 0x0023_544d, Sig::Site, DOC);
    pub const HUD_RETURN: Point = Point::new("UI_HUD_RETURN", 0x0013_96b9, Sig::Site, DOC);
    pub const HUD_HIDDEN_RETURN: Point =
        Point::new("UI_HUD_HIDDEN_RETURN", 0x0013_96c6, Sig::Site, DOC);
    // The frame tail calls both helpers with two stack arguments. Each
    // return site is before RET 8, with ESP back at its entry value.
    pub const TAIL_COPY: Point = Point::new("TAIL_COPY", 0x0003_51a0, Sig::Site, DOC);
    pub const TAIL_COPY_END: Point = Point::new("TAIL_COPY_END", 0x0003_52da, Sig::Site, DOC);
    pub const TAIL_COMPOSITE: Point = Point::new("TAIL_COMPOSITE", 0x0002_2070, Sig::Site, DOC);
    pub const TAIL_COMPOSITE_END: Point = Point::new("TAIL_COMPOSITE_END", 0x0002_229d, Sig::Site, DOC);
    pub const TAIL_COPY_CALLER: u32 = 0x0001_3bf7;
    pub const TAIL_COMPOSITE_CALLER: u32 = 0x0001_3c16;
    pub const FADE_RETURN: Point = Point::new("UI_FADE_RETURN", 0x0004_0dbf, Sig::Site, DOC);
    pub const OVERLAY_RECT_RETURN: Point =
        Point::new("UI_OVERLAY_RECT_RETURN", 0x0013_c1db, Sig::Site, DOC);
    pub const LOADING_RETURN: Point = Point::new("UI_LOADING_RETURN", 0x0002_906d, Sig::Site, DOC);
    pub const LOADING_EMPTY_RETURN: Point =
        Point::new("UI_LOADING_EMPTY_RETURN", 0x0002_9075, Sig::Site, DOC);
    pub const LOADING_SPINNER_READY: Point =
        Point::new("UI_LOADING_SPINNER_READY", 0x0002_a1f4, Sig::Site, DOC);
    pub const LOADING_SPINNER_TAIL: u32 = 0x0002_a2dc;
    pub const LOADING_BITMAP_READY: Point =
        Point::new("UI_LOADING_BITMAP_READY", 0x0002_ae60, Sig::Site, DOC);
    pub const LOADING_BITMAP_TAIL: u32 = 0x0002_af87;
    /// Return from the common inline quad uploader to the overlay/bitmap helper.
    pub const RECT_SUBMIT_RETURN: u32 = 0x0005_2ce7;
    pub const SCREEN_QUAD_READY: Point =
        Point::new("UI_SCREEN_QUAD_READY", 0x0003_63c4, Sig::Site, DOC);
    pub const FADE_FAN_READY: Point = Point::new("UI_FADE_FAN_READY", 0x0004_0d86, Sig::Site, DOC);
    pub const FADE_FAN_TAIL: u32 = 0x0004_0db8;
}

/// The named guest globals live in `guest_layout::global` with the
/// structure layouts; they are re-exported here, so `points::GAME_GLOBALS`
/// and `points::GLOBALS` are unchanged.
pub use guest_layout::global::*;

#[doc(hidden)]
#[macro_export]
macro_rules! point_loc {
    (eax) => {
        $crate::points::Loc::Eax
    };
    (al) => {
        $crate::points::Loc::Al
    };
    (ecx) => {
        $crate::points::Loc::Ecx
    };
    (edx) => {
        $crate::points::Loc::Edx
    };
    (ebx) => {
        $crate::points::Loc::Ebx
    };
    (esi) => {
        $crate::points::Loc::Esi
    };
    (edi) => {
        $crate::points::Loc::Edi
    };
    (xmm0) => {
        $crate::points::Loc::Xmm0
    };
    (stack $n:literal) => {
        $crate::points::Loc::Stack($n)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! point_sig {
    (opaque) => {
        $crate::points::Sig::Opaque
    };
    (fn ( $($arg:ident : $loc:ident $($n:literal)?),* ) $(-> $ret:ident)? ret $pop:literal) => {
        $crate::points::Sig::Fn {
            args: &[$((stringify!($arg), $crate::point_loc!($loc $($n)?))),*],
            ret: $crate::point_ret!($($ret)?),
            pop: $pop,
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! point_ret {
    () => {
        None
    };
    ($r:ident) => {
        Some($crate::point_loc!($r))
    };
}

/// Declares named [`Point`]s (see the module docs for the grammar) and adds
/// them to a slice named by the first token (`points! { ALL; ... }`).
#[macro_export]
macro_rules! points {
    ($all:ident; $( $(#[doc = $d:literal])* $name:ident @ $addr:literal : [$($sig:tt)*] $(, $flag:ident)* ; $doc:literal ; )*) => {
        $(
            $(#[doc = $d])*
            pub static $name: $crate::points::Point = $crate::points::Point::new(
                stringify!($name),
                $addr,
                $crate::point_sig!($($sig)*),
                $doc,
            )
            .with_flags(&[$($crate::points::Flag::$flag),*]);
        )*
        /// Every point declared in this table.
        pub static $all: &[&$crate::points::Point] = &[$(&$name),*];
    };
}

points! { ALL;
    /// Length of the vector at EAX. The original returns the float in x87 ST(0).
    VECTOR_LENGTH_3D @ 0x0002_4590: [fn(vector: eax) ret 0], Hot; "docs/halo1/objects.md";
    /// Initialize the damage packet at EDX from the damage effect datum on the stack.
    DAMAGE_DATA_INIT @ 0x000d_6660: [fn(packet: edx, effect: stack 0) ret 4]; "docs/halo1/objects.md";
    SIM_UNIT_POSE @ 0x0011_2070: [fn(unit: stack 0, nodes: stack 1, source: stack 2, target: stack 3) ret 16], Hot; "docs/perf/sim-budget.md";
    SIM_ANIMATION_STREAM @ 0x0027_98a0: [opaque], Hot; "docs/perf/sim-budget.md";
    SIM_GRAPH_RESOLVE @ 0x001d_d9d0: [fn(graph: edx, clip: stack 0) ret 4], Hot; "docs/perf/sim-budget.md";
    SIM_MARKER_TRANSFORMS @ 0x000b_8d30: [fn(out: ebx, object: stack 0, name: stack 1, capacity: stack 2) -> eax ret 12], Hot; "docs/perf/sim-budget.md";
    COLLISION_BSP_SEGMENT @ 0x001d_e6d0: [fn(query: stack 0, node: stack 1, lo: stack 2, hi: stack 3) -> al ret 16], Hot; "docs/engine/collision-queries.md";
    /// BSP vector test. The parity observers gather at entry and compare AL and the result record at return.
    COLLISION_VECTOR_TEST @ 0x001d_e630: [fn(out: eax, flags: ecx, bsp: edx, limit: xmm0, breakable_count: stack 0, breakable_mask: stack 1, start: stack 2, delta: stack 3) -> al ret 16], Hot; "docs/engine/collision-queries.md";
    /// The instanced-geometry trace in one cluster.
    COLLISION_INSTANCE_TRACE @ 0x0024_4980: [fn(cluster: eax, start: esi, delta: edi, flags: stack 0, filter: stack 1, out: stack 2) -> al ret 12], Hot; "docs/engine/collision-queries.md";
    /// Sphere cluster flood: the clusters a sphere reaches through portals from `cluster`, written to `out` while `capacity` lasts; returns the count in AX.
    COLLISION_CLUSTER_FLOOD @ 0x0014_a5b0: [fn(cluster: stack 0, center: stack 1, radius: stack 2, capacity: stack 3, out: stack 4) -> eax ret 20], Hot; "docs/engine/collision-queries.md";
    /// Location of a point in the structure BSP: leaf, cluster, BSP index.
    COLLISION_LOCATION @ 0x0011_bed0: [fn(point: edx, out: esi) ret 0], Hot; "docs/engine/collision-queries.md";
    SIM_COLLISION_QUERY @ 0x0016_97c0: [fn(flags: stack 0, start: stack 1, delta: stack 2, ignore_a: stack 3, ignore_b: stack 4, out: stack 5) -> al ret 24], Hot; "docs/perf/sim-budget.md";
    BASE_OBJECT_IMPORT @ 0x000b_cc20: [fn(object: stack 0, name: stack 1, value: stack 2, active: stack 3) -> al ret 16], Hot; "docs/perf/sim-budget.md";
    /// The integer MOPP box query: setup, then the interpreter below.
    COLLISION_MOPP_BOX @ 0x0030_0150: [fn(vm: ecx, code: stack 0, aabb: stack 1, array: stack 2) ret 12], Hot; "docs/engine/collision-queries.md";
    /// The general MOPP ray: setup, then the float interpreter below; the result goes through the collector.
    COLLISION_MOPP_RAY @ 0x0030_1800: [fn(vm: ecx, collection: stack 0, code: stack 1, input: stack 2, parent: stack 3, collector: stack 4) ret 20], Hot; "docs/engine/collision-queries.md";
    /// The instanced-geometry collection's child method, called once per accepted MOPP key.
    COLLISION_INSTANCE_CHILD @ 0x0024_4bb0: [fn(collection: ecx, key: stack 0, buffer: stack 1) -> eax ret 8], Hot; "docs/engine/collision-queries.md";
    SIM_MOPP_BOX @ 0x002f_f8b0: [fn(query: ecx, state: stack 0, program: stack 1) ret 8], Hot; "docs/perf/sim-budget.md";
    SIM_MOPP_RAY @ 0x0030_09e0: [fn(query: ecx, state: stack 0, program: stack 1, segment: stack 2) ret 12], Hot; "docs/perf/sim-budget.md";
    OBJECT_TRANSFORM_COMPOSE @ 0x0014_2a60: [fn(parent: ecx, child: edx, out: stack 0) -> eax ret 4], Hot; "docs/perf/sim-budget.md";
    OBJECT_POSE_MATRIX @ 0x0014_21f0: [fn(out: stack 0, pose: stack 1) ret 8], Hot; "docs/perf/sim-budget.md";
    /// The animation graph's node walk: every node reached from node 0
    /// through the child and sibling links posed and composed with its
    /// parent's matrix (`root` for node 0) into `matrices`.
    ANIM_GRAPH_MATRICES @ 0x001d_d1c0: [fn(graph: stack 0, matrices: stack 1, records: stack 2, root: stack 3) ret 16], Hot; "docs/engine/skeleton-walk.md";
    /// Root node transform from two axes, scale and an optional parent.
    OBJECT_ROOT_TRANSFORM @ 0x000b_dc40: [fn(forward: eax, up: ecx, position: stack 0, scale: stack 1, mirror: stack 2, parent: stack 3, out: stack 4) ret 20], Hot; "docs/perf/sim-budget.md";
    ANIM_ROTATION_CODEC @ 0x0028_c530: [fn() ret 0], Hot; "docs/perf/sim-budget.md";
    /// Animation channel codecs the node evaluators call per node.
    ANIM_STATIC_ROTATION @ 0x0028_c470: [fn() ret 0], Hot; "docs/perf/sim-budget.md";
    ANIM_FRAME_ROTATION @ 0x0028_c960: [fn() ret 0], Hot; "docs/perf/sim-budget.md";
    ANIM_BLEND_ROTATION @ 0x0028_c790: [fn() ret 0], Hot; "docs/perf/sim-budget.md";
    ANIM_KEYED_TRANSLATION @ 0x0028_cb70: [fn() ret 0], Hot; "docs/perf/sim-budget.md";
    /// Additive-layer node evaluator: blends two samples per channel into the pose.
    ANIM_ADDITIVE_NODES @ 0x0028_0ac0: [fn() ret 0], Hot; "docs/perf/sim-budget.md";
    COLLISION_POINT_TO_LEAF @ 0x0014_a280: [fn(node: eax, bsp: ecx, point: edx) -> eax ret 0], Hot; "docs/perf/sim-budget.md";
    COLLISION_GEOMETRY_TEST @ 0x0011_e5e0: [fn(origin: eax, center: ecx, direction: edx, radius: stack 0) -> al ret 4], Hot; "docs/perf/sim-budget.md";
    /// The HaloScript evaluator shared by argument-evaluating discarded builtins.
    HS_DISCARDED_EVALUATE @ 0x002a_b6c0: [fn(opcode: stack 0, thread: stack 1, first: stack 2) ret 12]; "docs/halo1/ai-control.md";
    /// How strongly the observing actor prioritizes one candidate.
    AI_TARGET_SCORE @ 0x0026_5d30: [fn(actor: stack 0, interest: stack 1) -> xmm0 ret 8], Hot; "docs/halo1/ai-control.md";
    /// Finalizes the actor's current interest after examining perception records.
    AI_SELECT_INTEREST @ 0x0026_6640: [fn(actor: stack 0) ret 4], Hot; "docs/halo1/ai-control.md";
    /// The actor's contribution to outgoing damage calculation.
    AI_DAMAGE_MODIFIER @ 0x001c_8440: [fn(actor: ecx, multiplier: edx, packet: stack 0) -> al ret 4]; "docs/halo1/ai-control.md";
    /// Selects the uncover behavior from presearch when a tracked target is available.
    AI_PRESEARCH_UNCOVER @ 0x001b_44d0: [fn(actor: stack 0, state: stack 1) -> eax ret 8], Hot; "docs/halo1/ai-control.md";
    /// Ranks uncover from its own behavior descriptor.
    AI_UNCOVER_RANK @ 0x001b_84a0: [fn(actor: stack 0) -> eax ret 4], Hot; "docs/halo1/ai-control.md";
    AI_UNCOVER_ENTER @ 0x001b_8540: [fn(actor: stack 0, record: stack 1) -> al ret 8]; "docs/halo1/ai-control.md";
    AI_UNCOVER_BEGIN @ 0x001b_85a0: [fn(actor: stack 0, record: stack 1) -> al ret 8]; "docs/halo1/ai-control.md";
    AI_UNCOVER_UPDATE @ 0x001b_89d0: [fn(actor: stack 0, record: stack 1) ret 8]; "docs/halo1/ai-control.md";
    AI_UNCOVER_CONTROL @ 0x001b_8ae0: [fn(actor: stack 0, record: stack 1) ret 8]; "docs/halo1/ai-control.md";
    /// Starts a firing burst, setting the actor timer and angular spread.
    AI_FIRE_BURST @ 0x001f_e620: [fn(actor: eax, firing: ebx, weapon: stack 0) ret 4], Hot; "docs/halo1/ai-control.md";
    /// Chooses the pause between bursts and saves it in the actor timer.
    AI_FIRE_PAUSE @ 0x001f_ed70: [fn(actor: eax, firing: ecx) ret 0], Hot; "docs/halo1/ai-control.md";
    /// Stages the actor's movement, aim and trigger commands for its unit.
    AI_SUBMIT_UNIT_CONTROL @ 0x001e_4390: [fn(actor: eax) ret 0], Hot; "docs/halo1/ai-control.md";
    /// One actor's queued behaviors update: walks the 0x40-stride records
    /// at actor +0x90 through the dispatch table at 0x46eeb8.
    AI_UPDATE_BEHAVIORS @ 0x001a_7cc0: [fn(actor: stack 0) ret 4], Hot; "docs/halo1/ai-control.md";
    HS_UNIT_GRENADE_COUNT @ 0x002a_4310: [fn(opcode: stack 0, thread: stack 1, first: stack 2) ret 12]; "docs/halo1/ai-control.md";
    /// Scripted vehicle hold. The boolean is passed in CL.
    VEHICLE_HOVER @ 0x000f_0fd0: [fn(vehicle: eax, on: ecx) ret 0], Optional; "docs/game/notes/2026-11-24-vehicle-actions.md";
    /// Request exit from occupied seats matching the mapping (-1 for all).
    /// The accepted action count occupies the low word of EAX.
    VEHICLE_UNLOAD @ 0x0011_b2b0: [fn(vehicle: edx, mapping: ecx) -> eax ret 0], Optional; "docs/game/notes/2026-11-24-vehicle-actions.md";
    /// Seat the units of an HS object list in the vehicle's empty compatible
    /// seats (`vehicle_load_magic`'s body). The vehicle is in EDX; the seat
    /// mapping (-1 for all) and the object list datum are on the stack. The
    /// accepted count occupies the low word of EAX. The stack layout and
    /// the pop are read from the call at `0x2a411c`, not yet confirmed by a run.
    VEHICLE_LOAD @ 0x0011_b0c0: [fn(vehicle: edx, mapping: stack 0, list: stack 1) -> eax ret 8], Optional; "docs/game/notes/2026-11-24-vehicle-actions.md";
    /// `unit_enter_vehicle`'s body: the unit enters the seat of the vehicle
    /// whose label (a string ID) matches. The vehicle is in EBX; the unit
    /// datum and the seat label are on the stack. A unit already in another
    /// vehicle leaves it first (action `0x1e` of `0xe68c0`). Read from the
    /// disassembly of the caller at `0x2a3ad0`, not yet confirmed by a run.
    UNIT_ENTER_VEHICLE @ 0x0011_ade0: [fn(vehicle: ebx, unit: stack 0, label: stack 1) ret 8], Optional; "docs/game/notes/2026-11-24-vehicle-actions.md";
    /// The HS cast of an AI reference to an object list of its actors: the
    /// reference (a plain squad's index) on the stack, the list datum
    /// returned in EAX. The HS caller at `0x2a5b5b` pushes the reference
    /// and does not pop it; the body reads it at `0x272ea4` and writes the
    /// new list datum over that same stack slot at `0x272ebf`, so a call
    /// that passes no stack argument loses its caller's return address.
    OBJECT_LIST_FROM_AI @ 0x0027_2ea0: [fn(ai: stack 0) -> eax ret 4], Optional; "docs/engine/hs_vm.md";
    /// HaloScript's object awareness evaluator, index 305.
    HS_AI_SEE_OBJECT @ 0x002a_53b0: [fn(opcode: stack 0, thread: stack 1, first: stack 2) ret 12]; "docs/halo1/ai-control.md";
    /// HaloScript's two-argument `ai_place` evaluator (291): a squad and a
    /// count. It places a plain squad (the top two bits of the `ai` value
    /// clear) through [`AI_SQUAD_PLACE`] with any start, not forced, spawn
    /// flag 1, and stores the `ai` value at AI globals `+0x364`.
    HS_AI_PLACE_COUNTED @ 0x002a_5010: [fn(opcode: stack 0, thread: stack 1, first: stack 2) ret 12], Optional; "docs/game/firefight.md";
    /// HaloScript's `ai_set_team` evaluator (322): calls [`AI_SET_TEAM`].
    HS_AI_SET_TEAM @ 0x002a_5850: [fn(opcode: stack 0, thread: stack 1, first: stack 2) ret 12], Optional; "docs/game/firefight.md";
    /// HaloScript's `ai_erase` evaluator (297): calls [`AI_ERASE`].
    HS_AI_ERASE @ 0x002a_5210: [fn(opcode: stack 0, thread: stack 1, first: stack 2) ret 12], Optional; "docs/game/firefight.md";
    /// HaloScript's `ai_set_orders` evaluator (333): calls [`AI_SET_ORDERS`].
    HS_AI_SET_ORDERS @ 0x002a_5c10: [fn(opcode: stack 0, thread: stack 1, first: stack 2) ret 12], Optional; "docs/game/firefight.md";
    /// Place `count` actors of a scenario squad. The character is read per
    /// actor at placement, from the start's character or else the squad's
    /// (`+0x36`). `location` -1 lets [`AI_CHOOSE_SQUAD_START`] pick each
    /// start; a given location places one actor. `force` (a byte) skips the
    /// location's availability check and the emitter's deferred placement action;
    /// `spawn_flags` goes to [`AI_SPAWN_AT_SQUAD_START`]. Returns at once
    /// while the AI globals' initialised byte is clear.
    AI_SQUAD_PLACE @ 0x0020_1520: [fn(count: eax, squad: stack 0, location: stack 1, force: stack 2, spawn_flags: stack 3) ret 16]; "docs/game/firefight.md";
    /// Delete every actor of an `ai` value (a squad, or a group as
    /// `0x4000_0000 | index`) with its unit, then the objects the squad
    /// placed (runtime squad `+0x70`) that nothing else keeps.
    AI_ERASE @ 0x0027_36c0: [fn(ai: stack 0) ret 4], Optional; "docs/game/firefight.md";
    /// Set the team of a squad or group: runtime squad `+0x76`, each
    /// actor's `+0x24` and its unit's `+0x138`.
    AI_SET_TEAM @ 0x0027_5b60: [fn(ai: eax, team: ebx) ret 0], Optional; "docs/game/firefight.md";
    /// Give a squad or group a scenario order (the order block at scenario
    /// `+0x240`); an index outside the block is ignored.
    AI_SET_ORDERS @ 0x0027_5cb0: [fn(ai: eax, order: ecx) ret 0], Optional; "docs/game/firefight.md";
    /// The AI's clear-line test between two points: the collision query
    /// with flags `0x0180_8c2d` (structure, instanced geometry and
    /// objects). True when nothing is hit, or the hit is within about a
    /// third of a world unit of `to`.
    AI_LINE_OF_SIGHT @ 0x0029_dd50: [fn(from: esi, to: edi, ignore_a: edx, ignore_b: ecx) -> al ret 0], Optional; "docs/game/firefight.md";
    HS_CONVERSATION_UNIT_NEW @ 0x000c_4410: [fn(unit: stack 0) ret 12]; "docs/halo1/ai.md";
    /// Initializes the unit's speech block from its model and variant name.
    UNIT_DIALOGUE_SET_VARIANT @ 0x0011_4ec0: [fn(unit: eax, variant: stack 0) ret 4]; "docs/halo1/ai.md";
    /// Enqueue the selected candidate in the AI dialogue scheduler. Its first
    /// word is the vocabulary index and its dword at +4 is the speaking unit.
    /// AL is nonzero when the immediate speech or queue insertion succeeds.
    AI_DIALOGUE_ENQUEUE @ 0x0020_d0c0: [fn(vocabulary: stack 0, time: stack 1, stimulus: stack 2, candidate: stack 3, context: stack 4, parent: stack 5, queued: stack 6) -> al ret 28]; "docs/halo1/ai.md";
    /// Check whether a unit can play a line and resolve its sound.
    UNIT_DIALOGUE_CHECK @ 0x0011_4520: [fn(unit: eax, vocalization: edx, priority: stack 0, speech_time: stack 1, sound: stack 2) -> al ret 12]; "docs/halo1/ai.md";
    /// Installs the prepared utterance into the per-unit speech block.
    UNIT_DIALOGUE_SET_LINE @ 0x0011_4680: [fn(unit: eax, line: ebx) ret 0]; "docs/halo1/ai.md";
    /// Update the unit's playing and queued speech.
    UNIT_DIALOGUE_UPDATE @ 0x0011_4c60: [fn(unit: eax) ret 0], Hot; "docs/halo1/ai.md";
    /// Clears body depletion and restores a reanimating unit.
    UNIT_REANIMATE @ 0x000c_af00: [fn(unit: eax) ret 0]; "docs/halo1/ai.md";
    /// Reports a nonlethal hit to the victim's and aggressor's AI.
    AI_INJURY_NOTICE @ 0x001c_9c80: [fn(victim: eax, owner: edx, category: stack 0, amount: stack 1, direction: stack 2, deferred: stack 3) ret 16]; "docs/halo1/ai.md";
    /// Reports a depleted unit body and the player or unit responsible.
    AI_KILL_NOTICE @ 0x001c_95d0: [fn(victim: stack 0, owner: stack 1, category: stack 2, amount: stack 3) ret 16]; "docs/halo1/ai.md";
    /// Announces a vocalization event from an actor and its perceived object.
    AI_ACTOR_VOCALIZE @ 0x001f_b7e0: [fn(event: eax, actor: ecx, context: edx, object: stack 0, other: stack 1) -> al ret 8]; "docs/halo1/ai.md";
    /// Notifies the AI when native dismount action 29 has been accepted.
    AI_DISMOUNT_NOTICE @ 0x001b_ba20: [fn(actor: eax) ret 0]; "docs/halo1/ai.md";
    /// Enters the leaping variant of the native melee charge.
    AI_MELEE_LEAP_ENTER @ 0x001a_b380: [fn(actor: stack 0, state: stack 1) -> al ret 8]; "docs/halo1/ai.md";
    /// Dispatches the global vocalization stimulus and its two participants.
    AI_DIALOGUE_STIMULUS @ 0x0020_ba60: [fn(event: stack 0, speaker: stack 1, object: stack 2, relation: stack 3, other: stack 4, context: stack 5) -> al ret 24]; "docs/halo1/ai.md";
    /// Enters native postcombat behavior.
    AI_POSTCOMBAT_ENTER @ 0x001b_4240: [fn(actor: stack 0, state: stack 1) -> al ret 8]; "docs/halo1/ai.md";
    HS_CONVERSATION_MAP_DISPOSE @ 0x0020_9290: [fn() ret 0]; "docs/halo1/ai.md";
    /// `game_tick`: the per-tick phases, then the tick count. Called once per
    /// simulated tick by `game_update_ticks`.
    GAME_TICK @ 0x0013_7fe0: [fn() ret 0]; "docs/engine/engine-map.md";
    /// Choose and reserve an available actor starting location.
    AI_CHOOSE_SQUAD_START @ 0x0020_39d0: [fn(squad: eax, available: stack 0, excluded: stack 1) -> eax ret 8]; "docs/halo1/ai-control.md";
    /// Spawn an actor using the chosen starting location index.
    AI_SPAWN_AT_SQUAD_START @ 0x0020_3780: [fn(context: eax, start: ecx, squad: stack 0, flags: stack 1) -> eax ret 8]; "docs/halo1/ai-control.md";
    AI_SQUADS_ALLOCATE @ 0x0020_0930: [fn() ret 0]; "docs/halo1/ai-control.md";
    AI_SQUADS_INITIALIZE @ 0x0020_0b30: [fn() ret 0]; "docs/halo1/ai.md";
    /// The frame's local tick batch and its elapsed seconds output.
    GAME_UPDATE_TICKS @ 0x0013_7f70: [fn(ticks: eax, elapsed: ebx) ret 0]; "docs/halo1/script-builtins.md";
    /// Per-frame gameplay updates after the local simulation batch.
    GAME_UPDATE_FRAME @ 0x0013_7ed0: [fn(elapsed: stack 0) ret 4]; "docs/halo1/script-builtins.md";
    /// The shared HaloScript evaluator for discarded retail commands.
    OBJECT_FUNCTION_IMPORT @ 0x0010_8d90: [fn(object: stack 0, name: stack 1, value: ebx, active: edi) -> al ret 8]; "docs/halo1/script-builtins.md";
    HS_UNIT_FLAG_SET @ 0x002a_3200: [fn(index: stack 0, thread: stack 1, first: stack 2) ret 12]; "docs/halo1/script-builtins.md";
    HS_UNIT_FLAG_GET @ 0x002a_3a10: [fn(index: stack 0, thread: stack 1, first: stack 2) ret 12]; "docs/halo1/script-builtins.md";
    HS_AI_LIST_FLAG_SET @ 0x002a_55f0: [fn(index: stack 0, thread: stack 1, first: stack 2) ret 12]; "docs/halo1/script-builtins.md";
    OBJECT_FUNCTION_VALUE @ 0x000b_ab40: [fn(object: stack 0, name: stack 1, value: stack 2) -> al ret 12]; "docs/halo1/script-builtins.md";
    HS_SHORT_TO_LONG_EVALUATOR @ 0x002a_b760: [fn(index: stack 0, thread: stack 1, first: stack 2) ret 12]; "docs/halo1/script-builtins.md";
    /// The shared HaloScript evaluator for discarded retail commands.
    HS_UNUSED_EVALUATOR @ 0x002a_b130: [fn(index: stack 0, thread: stack 1, first: stack 2) ret 12]; "docs/halo1/script-builtins.md";
    HS_LOOK_ANY_DIRECTION @ 0x002a_9340: [fn(index: stack 0, thread: stack 1, first: stack 2) ret 12]; "docs/halo1/script-builtins.md";
    HS_MOVE_ANY_DIRECTION @ 0x002a_9370: [fn(index: stack 0, thread: stack 1, first: stack 2) ret 12]; "docs/halo1/script-builtins.md";
    HS_FALSE_EVALUATOR @ 0x002a_5d50: [fn(index: stack 0, thread: stack 1, first: stack 2) ret 12]; "docs/halo1/script-builtins.md";
    HS_CONSTANT_LONG_EVALUATOR @ 0x002a_c600: [fn(index: stack 0, thread: stack 1, first: stack 2) ret 12]; "docs/halo1/script-builtins.md";
    /// Game activation after the scenario's cache is loaded: options in EAX,
    /// AL 1 once the map and its first BSP are in.
    GAME_ACTIVATE @ 0x0018_f000: [fn(options: eax) -> al ret 0]; "docs/runtime/level-boot.md";
    CACHE_TAGS_LOAD @ 0x0012_28f0: [opaque], Optional; "docs/engine/engine-map.md";
    /// Reserve the physical arena the tag cache loads into and set its
    /// cursors (once, at startup); the `tag-arena` feature grows it.
    PHYSICAL_ARENA_INITIALIZE @ 0x0012_b400: [opaque]; "docs/formats/map-format.md";
    /// XAPI `XPhysicalAlloc`: MmAllocateContiguousMemoryEx over
    /// `[address, address + size)`, or anywhere for address -1.
    XPHYSICAL_ALLOC @ 0x002d_156a: [fn(size: stack 0, address: stack 1, alignment: stack 2, protect: stack 3) -> eax ret 16]; "docs/formats/map-format.md";
    CACHE_BSP_FIXUP @ 0x0012_2b40: [opaque], Optional; "docs/engine/engine-map.md";
    TAG_LOOKUP @ 0x0012_2c10: [opaque], Optional; "docs/engine/tag_get_checked.md";
    OBJECTS_PRE_PHYSICS @ 0x000b_6f10: [opaque], Optional; "docs/engine/engine-map.md";
    WORLD_PHYSICS @ 0x001c_3d30: [opaque], Optional; "docs/engine/engine-map.md";
    OBJECTS_POST_PHYSICS @ 0x000b_7060: [opaque], Optional; "docs/engine/engine-map.md";
    FIRST_PERSON_POSE_UPDATE @ 0x0016_760c: [opaque], Hot; "docs/perf/sim-budget.md";
    FIRST_PERSON_FILTER_UPDATE @ 0x0016_6d75: [opaque], Hot; "docs/perf/sim-budget.md";
    LIGHT_RECORD_UPDATE @ 0x000c_0c80: [fn(light: stack 0) ret 4], Hot; "docs/perf/sim-budget.md";
    HAVOK_WELD_CONTACTS @ 0x002f_a2f0: [opaque], Hot; "docs/perf/sim-budget.md";
    HAVOK_SOLVER @ 0x0031_c940: [opaque], Hot; "docs/perf/sim-budget.md";
    /// The Havok world step: collide, then integrate, each phase a guest
    /// call resumed inside the wrapper (sim-rewrite-havok.md §2).
    HAVOK_WORLD_STEP @ 0x0031_1100: [fn(world: ecx, step: stack 0) ret 4], Hot; "docs/engine/sim-rewrite-havok.md";
    HAVOK_ISLAND_COLLIDE @ 0x0031_7d30: [fn(island: ecx, broadphase: stack 0, input: stack 1) ret 8], Hot; "docs/engine/sim-rewrite-havok.md";
    HAVOK_ISLAND_INTEGRATE @ 0x0031_6a20: [fn(island: ecx, input: stack 0) ret 4], Hot; "docs/engine/sim-rewrite-havok.md";
    /// The integrate phase and the island deactivation test it runs per
    /// island (docs/engine/havok-integrate.md).
    HAVOK_INTEGRATE_PHASE @ 0x0030_fef0: [fn(world: ecx, step: stack 0) ret 4], Hot; "docs/engine/havok-integrate.md";
    /// The structure polygon builder every child-shape getter of the
    /// structure's containers calls (docs/engine/havok-structure.md).
    HAVOK_STRUCTURE_POLYGON @ 0x001e_dd00: [fn(shape: ecx, transform: edi, bsp: stack 0, flag: stack 1, owner: stack 2, surface: stack 3, tail: stack 4) -> eax ret 20], Hot; "docs/engine/havok-structure.md";
    /// The convex agent's process call, which the convex agent
    /// `0x00412c48` and the biped wrapper's agent call
    /// (docs/engine/havok-convex.md).
    HAVOK_CONVEX_PROCESS @ 0x002e_eb40: [fn(agent: ecx, a: stack 0, b: stack 1, input: stack 2, output: stack 3) ret 16], Hot; "docs/engine/havok-convex.md";
    /// The convex agents' closest-feature query (docs/engine/havok-convex.md).
    /// The convex agents' add-point call, cdecl (docs/engine/havok-convex.md).
    HAVOK_ADD_POINT @ 0x0030_2c70: [fn(a: stack 0, b: stack 1, input: stack 2, point: stack 3, manager: stack 4, slots: stack 5, count: stack 6) -> eax ret 0], Hot; "docs/engine/havok-convex.md";
    /// The contact manager's add (vtable `0x00415074` slot 1), which the
    /// add-point call makes for a new slot (docs/engine/havok-convex.md).
    HAVOK_CONTACT_COMMIT @ 0x0031_8c10: [fn(manager: ecx, a: stack 0, b: stack 1, input: stack 2, contact: stack 3) -> eax ret 16], Hot; "docs/engine/havok-convex.md";
    /// The world's contact listener walk, cdecl (docs/engine/havok-convex.md).
    HAVOK_CONTACT_WALK @ 0x0031_8320: [fn(world: stack 0, event: stack 1) ret 0], Hot; "docs/engine/havok-convex.md";
    HAVOK_CLOSEST_QUERY @ 0x0030_48a0: [fn(state: ecx, a: stack 0, b: stack 1, transform: stack 2, cache: stack 3, out: stack 4) -> eax ret 20], Hot; "docs/engine/havok-convex.md";
    HAVOK_DEACTIVATION_TEST @ 0x0031_6b30: [fn(island: ecx, out: stack 0, step: stack 1) -> eax ret 8], Hot; "docs/engine/havok-integrate.md";
    /// The island solve, cdecl (docs/engine/havok-step.md, "The island solve").
    HAVOK_ISLAND_SOLVE @ 0x0031_c100: [fn(step: stack 0, info: stack 1, pairs: stack 2, gravity: stack 3, island: stack 4, entities: stack 5, entity_count: stack 6, constraints: stack 7, constraint_count: stack 8) -> eax ret 0], Hot; "docs/engine/havok-step.md";
    /// The memory system's virtuals (vtable `0x004576a4`, docs/engine/havok-step.md,
    /// "Allocator"): the directory pool's allocate and free, and the LIFO
    /// push and pop on any pool's descriptor.
    HAVOK_ALLOC @ 0x0022_c300: [fn(pool: ecx, size: stack 0, tag: stack 1) -> eax ret 8], Hot; "docs/engine/havok-step.md";
    HAVOK_FREE @ 0x0022_c320: [fn(pool: ecx, block: stack 0, size: stack 1, tag: stack 2) ret 12], Hot; "docs/engine/havok-step.md";
    HAVOK_LIFO_ALLOC @ 0x002d_7020: [fn(pool: ecx, request: stack 0) -> eax ret 4], Hot; "docs/engine/havok-step.md";
    HAVOK_LIFO_FREE @ 0x002d_7090: [fn(pool: ecx, block: stack 0, request: stack 1) ret 8], Hot; "docs/engine/havok-step.md";
    /// The world broadphase's batch update, the sweep's virtual at slot
    /// `+0x14` of the broadphase behind `W+0xc4`.
    HAVOK_BROADPHASE_BATCH @ 0x002e_0350: [opaque], Hot; "docs/engine/sim-rewrite-havok.md";
    /// The two unconstrained-motion methods types 4 and 5 share: the pose,
    /// vtable slot `+0x04` (step; `ret 4`), and the forces, slot `+0x08`
    /// (step, gravity; `ret 8`).
    HAVOK_MOTION_POSE @ 0x0031_b060: [opaque], Hot; "docs/engine/sim-rewrite-havok.md";
    HAVOK_MOTION_FORCES @ 0x0031_b1c0: [opaque], Hot; "docs/engine/sim-rewrite-havok.md";
    OBJECT_SPATIAL_VISIT @ 0x000b_b050: [fn(lists: stack 0, types: stack 1, location: stack 2, center: stack 3, radius: stack 4, out: stack 5, capacity: stack 6) ret 28], Hot; "docs/perf/sim-budget.md";
    OBSERVER_RESULT @ 0x0017_1d90: [fn(observer: esi) ret 0], Hot; "docs/perf/sim-budget.md";
    OBJECT_SPATIAL_PORTAL @ 0x0014_a370: [fn(portal: eax, center: ecx, bsp: edx, radius: stack 0) -> al ret 4], Hot; "docs/perf/sim-budget.md";
    OBJECT_CHANGE_COLORS @ 0x000b_e8e0: [fn(object: stack 0) -> al ret 4], Hot; "docs/perf/sim-budget.md";
    OBJECT_PRE_UPDATE @ 0x000b_c470: [fn(object: stack 0) -> al ret 4], Hot; "docs/perf/sim-budget.md";
    OBJECT_POST_UPDATE @ 0x000b_c820: [fn(object: stack 0) ret 4], Hot; "docs/perf/sim-budget.md";
    /// Per-object node matrices: orientation block, root transform, skeleton walk.
    OBJECT_NODE_MATRICES @ 0x000b_d090: [fn(object: stack 0) ret 4], Hot; "docs/perf/sim-budget.md";
    UNIT_AIM_UPDATE @ 0x000c_7840: [opaque], Hot; "docs/perf/sim-budget.md";
    UNIT_UPDATE @ 0x000c_6b30: [fn(unit: stack 0) -> al ret 4], Hot; "docs/perf/sim-budget.md";
    OBJECTS_FINALIZE @ 0x000b_7150: [opaque], Optional; "docs/engine/engine-map.md";
    /// Type update dispatch: slot `+40` of every ancestry level; `al` is the OR of the results.
    OBJECT_TYPE_UPDATE @ 0x0010_8c60: [fn(object: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    /// Type post-physics dispatch: slot `+44` of every ancestry level.
    OBJECT_TYPE_POST_UPDATE @ 0x0010_8cd0: [fn(object: edi) ret 0], Hot; "docs/engine/object-update.md";
    /// Type pose dispatch: slot `+6c` of every ancestry level.
    OBJECT_TYPE_POSE @ 0x0010_9050: [fn(object: ebx, a: stack 0, b: stack 1, c: stack 2) ret 12], Hot; "docs/engine/object-update.md";
    /// Type after-root dispatch: slot `+70`, called with the root node matrix.
    OBJECT_TYPE_AFTER_ROOT @ 0x0010_90d0: [fn(object: ebx, matrix: stack 0) ret 4], Hot; "docs/engine/object-update.md";
    /// Unit after-root: slot `+70` of `unit`; composes the matrix with the parent's root node matrix.
    UNIT_AFTER_ROOT @ 0x0011_2dc0: [fn(object: stack 0, matrix: stack 1) ret 8], Hot; "docs/engine/object-update.md";
    /// Scenery after-root: slot `+70` of `scen`; composes the matrix with the parent's root node matrix.
    SCENERY_AFTER_ROOT @ 0x0010_a390: [fn(object: stack 0, matrix: stack 1) ret 8], Hot; "docs/engine/object-update.md";
    /// Set AWAKE on an object and its parent chain.
    OBJECT_WAKE @ 0x000b_7360: [fn(object: eax) ret 0], Hot; "docs/engine/object-update.md";
    /// The root ancestor of an object.
    OBJECT_ROOT @ 0x000b_af80: [fn(object: ecx) -> eax ret 0], Hot; "docs/engine/object-update.md";
    /// Wake an object (its root ancestor) for the post-physics walk.
    OBJECT_WAKE_FOR_POST @ 0x000b_ba20: [fn(object: eax) ret 0], Hot; "docs/engine/object-update.md";
    /// Wake the bipeds, vehicles and creatures in an object's subtree.
    OBJECT_WAKE_CHILDREN @ 0x000b_c7b0: [fn(object: stack 0) ret 4], Hot; "docs/engine/object-update.md";
    /// The node matrix record of an object's model: `+116` of its render model plus `0x34` per node.
    OBJECT_NODE_MATRIX @ 0x000b_8bd0: [fn(object: eax, node: stack 0) -> eax ret 4], Hot; "docs/engine/object-update.md";
    /// Whether the object's render model names an animation graph (`+12a` not none).
    OBJECT_HAS_ANIMATION @ 0x000b_f5a0: [fn(object: eax) -> al ret 0], Hot; "docs/engine/object-update.md";
    /// Whether the object is attached to something (`+b3` nonzero on its body).
    OBJECT_ATTACHED @ 0x000b_fc90: [fn(object: ecx) -> al ret 0], Hot; "docs/engine/object-update.md";
    /// The body of a live object whose type bit is set in `types`, else 0 (the handle none, out of range or stale).
    OBJECT_GET_TYPED @ 0x000b_adc0: [fn(object: eax, types: stack 0) -> eax ret 4], Hot; "docs/engine/object-update.md";
    /// Whether all eight words of a node mask are zero.
    NODE_MASK_EMPTY @ 0x000b_ff10: [fn(mask: ecx) -> al ret 0], Hot; "docs/engine/object-update.md";
    /// Invert a node mask (eight words; the last keeps its low byte).
    NODE_MASK_INVERT @ 0x000b_fe80: [fn(mask: eax) ret 0], Hot; "docs/engine/object-update.md";
    /// Clear a node mask's words and bits past `count` bits; the mask is in `esi`.
    NODE_MASK_TRIM @ 0x000b_fed0: [fn(count: eax, mask: esi) ret 0], Hot; "docs/engine/object-update.md";
    /// Set or clear one bit of a node mask word (`cl` the bit, `dl` set or clear).
    NODE_MASK_SET @ 0x000b_fe20: [fn(word: eax, bit: ecx, on: edx) ret 0], Hot; "docs/engine/object-update.md";
    /// Whether an animation record is set: dword `+68`, dword `+0` and word `+6` are all not none.
    ANIMATION_VALID @ 0x000b_6780: [fn(record: eax) -> eax ret 0], Hot; "docs/engine/object-update.md";
    /// Wake what an early mover's sphere touches.
    OBJECT_EARLY_MOVER_WAKE @ 0x0010_9580: [fn(mover: stack 0) ret 4], Hot; "docs/engine/object-update.md";
    // The walks' callouts in Rust (docs/engine/object-update.md, "Walk callouts").
    /// The type new dispatch: each ancestry slot `+2c` of the object's descriptor, `(object, a, b)`, the AND of the results.
    OBJECT_TYPE_NEW @ 0x0010_8a90: [fn(object: ebx, a: stack 0, b: stack 1) -> al ret 8], Hot; "docs/engine/object-update.md";
    /// Delete an object and its children: `OBJECT_DELETE`, the eight delete callbacks at `468664`, then each child.
    OBJECT_DELETE_TREE @ 0x000b_83b0: [fn(object: stack 0, top: stack 1) ret 8], Hot; "docs/engine/object-update.md";
    /// Free a deleted object and its children: detach, unlink, the subsystems' frees, then the datum.
    OBJECT_FREE_TREE @ 0x000b_8460: [fn(object: stack 0, top: stack 1) ret 8], Hot; "docs/engine/object-update.md";
    /// The weapons' deferred fire: each weapon with body `+16c` bit 0 through `fd980`.
    WEAPONS_DEFERRED_FIRE @ 0x000f_d910: [fn() ret 0], Hot; "docs/engine/object-update.md";
    /// Object memory collection, finalize's tail.
    OBJECTS_COLLECT @ 0x000b_f380: [fn() ret 0], Hot; "docs/engine/object-update.md";
    /// The early-mover record of list index `index` for `mover`.
    OBJECT_EARLY_MOVER_RECORD @ 0x0010_9660: [fn(index: ecx, mover: eax) ret 0], Hot; "docs/engine/object-update.md";
    /// Apply a pending replicated transform (body `+d8` bit 0).
    OBJECT_REPLICATED_TRANSFORM @ 0x000a_9570: [fn(object: eax) ret 0], Hot; "docs/engine/object-update.md";
    // Slice 3 of docs/engine/sim-rewrite-objects.md: function values and change colours.
    /// Evaluate one function export of an object's definition.
    OBJECT_FUNCTION_EVALUATE @ 0x000b_a8c0: [fn(record: eax, object: stack 0, value: stack 1) -> al ret 8], Hot; "docs/engine/object-update.md";
    /// The `unit` level's function import (slot `+4c`).
    UNIT_FUNCTION_IMPORT @ 0x000c_6b90: [fn(object: stack 0, name: stack 1, value: stack 2, active: stack 3) -> al ret 16], Hot; "docs/engine/object-update.md";
    /// A unit import helper: a fraction from the unit's body `+334`/`+338`.
    UNIT_IMPORT_FRACTION @ 0x000d_1210: [fn(unit: eax) -> xmm0 ret 0], Hot; "docs/engine/object-update.md";
    /// A unit import helper: a value from the unit's animation state.
    UNIT_IMPORT_ANIMATION @ 0x000d_1410: [fn(unit: eax) -> xmm0 ret 0], Hot; "docs/engine/object-update.md";
    /// The `bipd` level's function import (slot `+4c`).
    BIPED_FUNCTION_IMPORT @ 0x000d_e3b0: [fn(object: stack 0, name: stack 1, value: stack 2, active: stack 3) -> al ret 16], Hot; "docs/engine/object-update.md";
    /// The `vehi` level's function import (slot `+4c`).
    VEHICLE_FUNCTION_IMPORT @ 0x000f_6d10: [fn(object: stack 0, name: stack 1, value: stack 2, active: stack 3) -> al ret 16], Hot; "docs/engine/object-update.md";
    /// The `weap` level's function import (slot `+4c`).
    WEAPON_FUNCTION_IMPORT @ 0x000f_f5f0: [fn(object: stack 0, name: stack 1, value: stack 2, active: stack 3) -> al ret 16], Hot; "docs/engine/object-update.md";
    /// The `proj` level's function import (slot `+4c`).
    PROJECTILE_FUNCTION_IMPORT @ 0x000f_be70: [fn(object: stack 0, name: stack 1, value: stack 2, active: stack 3) -> al ret 16], Hot; "docs/engine/object-update.md";
    /// The `devi` level's function import (slot `+4c`).
    DEVICE_FUNCTION_IMPORT @ 0x0010_6d70: [fn(object: stack 0, name: stack 1, value: stack 2, active: stack 3) -> al ret 16], Hot; "docs/engine/object-update.md";
    /// The `mach` and `bloc` levels' function import (slot `+4c`).
    MACHINE_FUNCTION_IMPORT @ 0x0011_bd60: [fn(object: stack 0, name: stack 1, value: stack 2, active: stack 3) -> al ret 16], Hot; "docs/engine/object-update.md";
    AI_UPDATE @ 0x001c_83c0: [opaque], Optional; "docs/engine/engine-map.md";
    /// Queue one replication event in a distributed game (world mode 4 or
    /// 5; a no-op otherwise): the event type, the number of entities it
    /// refers to, their object handles, the payload's size and bytes, and
    /// a context word. A sixth register argument (EAX) is not read here.
    SIM_EVENT_GENERATE @ 0x000b_5a70: [fn(kind: stack 0, references: stack 1, objects: stack 2, size: stack 3, payload: stack 4, context: stack 5) ret 24], Optional; "docs/game/notes/ai-networking.md";
    /// Populate the simulation registry and return the two occupied counts.
    SIM_REGISTER_REPLICATION_TYPES @ 0x0008_2240: [fn(registry: eax, event_count: ecx, entity_count: stack 0) ret 4], Optional; "docs/game/notes/creature-entity.md";
    /// Choose an entity type for an object's loaded definition tag.
    SIM_OBJECT_ENTITY_TYPE @ 0x000b_5990: [fn(definition: eax, root: edx) -> eax ret 0], Optional; "docs/game/notes/creature-entity.md";
    /// Enroll a host object and bind its new entity to it.
    SIM_ENROLL_OBJECT @ 0x000a_7870: [fn(object: stack 0) ret 4], Optional; "docs/game/notes/creature-entity.md";
    /// Collect and distribute the entity database's pending dirty masks.
    SIM_ENTITY_COLLECT_UPDATES @ 0x0008_a090: [fn(database: stack 0) ret 4], Optional; "docs/game/notes/creature-entity.md";
    /// Compute an entity creation's priority and payload size for a view.
    SIM_ENTITY_CREATION_RELEVANCE @ 0x0008_a460: [fn(database: ecx, entity: stack 0, mask: stack 1, view: stack 2, priority: stack 3, bits: stack 4) ret 20], Optional; "docs/game/notes/creature-entity.md";
    /// Compute update priority with optional distance and period outputs.
    SIM_ENTITY_UPDATE_RELEVANCE @ 0x000a_bac0: [fn(distance: eax, record: stack 0, context: stack 1, period: stack 2) -> xmm0 ret 12], Optional; "docs/game/notes/creature-entity.md";
    /// Append one entity update operation to a peer view's replication batch.
    SIM_VIEW_WRITE_ENTITY_UPDATE @ 0x0009_89f0: [fn(stream: eax, slot: ecx, view: stack 0, context: stack 1, reserve: stack 2) -> al ret 12], Optional; "docs/game/notes/creature-entity.md";
    /// The object entity adapter that creates and binds a local replica.
    SIM_OBJECT_CREATE_REPLICA @ 0x000a_5a60: [fn(definition: ecx, record: stack 0, creation_size: stack 1, creation: stack 2, mask: stack 3, state_size: stack 4, state: stack 5) -> al ret 24], Optional; "docs/game/notes/creature-entity.md";
    /// Initialize an object placement record from a local definition datum.
    CREATURE_PROBE_PLACEMENT_NEW @ 0x000b_7930: [fn(placement: esi, definition: stack 0, owner: stack 1, source: stack 2) ret 12], Optional; "docs/game/notes/creature-entity.md";
    /// Allocate and place an object from a placement record.
    CREATURE_PROBE_OBJECT_NEW @ 0x000b_7b40: [fn(placement: stack 0) -> eax ret 4], Optional; "docs/game/notes/creature-entity.md";
    /// Apply an object transform and update its placement in the world.
    CREATURE_PROBE_OBJECT_POSITION @ 0x000b_75a0: [fn(position: eax, forward: ecx, up: edx, object: stack 0, a: stack 1, b: stack 2) ret 12], Optional; "docs/game/notes/creature-entity.md";
    /// Select replica physics for an object at its initial placement.
    CREATURE_PROBE_REPLICA_PHYSICS @ 0x000b_9b90: [fn(replica: edx, object: stack 0) ret 4], Optional; "docs/game/notes/creature-entity.md";
    HS_THREAD_EXECUTE @ 0x0020_9850: [fn(thread: edi) ret 0], Optional; "docs/engine/hs_vm.md";
    HS_EVALUATE @ 0x0020_99f0: [fn(thread: edx, expr: stack 0, dest: ebx) ret 4], Optional; "docs/engine/hs_vm.md";
    HS_GLOBAL_READ @ 0x0020_9bc0: [fn(index: esi) -> eax ret 0], Optional; "docs/engine/hs_vm.md";
    HS_EVALUATE_ARGUMENTS @ 0x0020_9d50: [fn(thread: stack 0, count: stack 1, parameters: stack 2, first: stack 3) -> eax ret 16], Optional; "docs/engine/hs_vm.md";
    SCRIPT_UPDATE @ 0x0020_9310: [opaque], Optional; "docs/engine/engine-map.md";
    /// The effect part switch: EAX and ECX name the part's source (EDI
    /// reads through EAX at `0x17aba2`, ECX `+2` and `+8` at `0x17a8fb`),
    /// the stack the effect (`+0x48`, `+0x64`), the part record (`+8` its
    /// group, `+0x10` its tag), and three more words (`0x17a91f`, `0x17a9d1`).
    EFFECT_PART_CREATE @ 0x0017_a8a0: [fn(source: eax, location: ecx, effect: stack 0, part: stack 1, a: stack 2, b: stack 3, c: stack 4) ret 20], Optional; "docs/engine/engine-map.md";
    /// Area damage from a damage record (`0xd6660`'s `0x88` bytes) and an
    /// object it excepts (`-1` for none). The effect part switch's `jpt!`
    /// arm calls it from `0x17aa4f` (docs/engine/effects-parts.md).
    DAMAGE_AREA_APPLY @ 0x000d_6c80: [fn(record: stack 0, except: stack 1) ret 8], Optional; "docs/engine/effects-parts.md";
    /// The effect part switch's `obje` arm places its object (`0x17abe2`):
    /// the shared random stream, the direction, two outputs and five part
    /// words. It draws from the stream.
    EFFECT_PART_PLACE @ 0x0017_7d10: [fn(effect: eax, rng: stack 0, direction: stack 1, out: stack 2, placed: stack 3, a: stack 4, b: stack 5, c: stack 6, d: stack 7, e: stack 8) ret 36], Optional; "docs/engine/effects-parts.md";
    /// The `obje` arm's velocity (`0x17ac3a`): the stream, the output and
    /// two part words. It draws from the stream.
    EFFECT_PART_VELOCITY @ 0x0017_7f60: [fn(rng: stack 0, out: stack 1, a: stack 2, b: stack 3) ret 16], Optional; "docs/engine/effects-parts.md";
    /// The `snd!` arm's sound on an object (`0x17ad24`): EAX and ECX the
    /// location's `+0xc` and `+0x30`, EBX the sound tag, the object, its
    /// node and the scale on the stack.
    SOUND_START_ON_OBJECT @ 0x0018_9060: [fn(at: eax, axes: ecx, tag: ebx, object: stack 0, node: stack 1, scale: stack 2) ret 12], Optional; "docs/engine/effects-parts.md";
    /// The `snd!` arm's sound at a point (`0x17ad9a`): EDX a `0x24`-byte
    /// record, XMM0 the scale, the tag on the stack.
    SOUND_START_AT_POINT @ 0x0018_95f0: [fn(record: edx, scale: xmm0, tag: stack 0) ret 4], Optional; "docs/engine/effects-parts.md";
    /// The `char` arm's character effect (`0x17a93d`): the ticks, the
    /// effect's object, its node, two part words and location `+8`; ESI the
    /// part's tag.
    EFFECT_PART_CHARACTER @ 0x001c_a290: [fn(tag: esi, ticks: stack 0, object: stack 1, node: stack 2, a: stack 3, b: stack 4, location: stack 5) ret 24], Optional; "docs/engine/effects-parts.md";
    /// The decal system's create (`0x17ab0b`, the `deca` arm): ECX
    /// `[0x510c78]`, the tag, the placement, a drawn `f32` and `0, -1, 0`.
    DECAL_CREATE @ 0x0017_e670: [fn(decals: ecx, tag: stack 0, placement: stack 1, value: stack 2, a: stack 3, b: stack 4, c: stack 5) ret 24], Optional; "docs/engine/effects-parts.md";
    /// The light creator (`0x17acab` and `0x17acdd`, the `ligh` arm): the
    /// tag, an object and node or -1, -1, and a location or source; XMM0 the
    /// scale.
    LIGHT_CREATE @ 0x000c_0350: [fn(scale: xmm0, tag: stack 0, object: stack 1, node: stack 2, at: stack 3) ret 16], Optional; "docs/engine/effects-parts.md";
    /// The effect walk's first per-effect call: the effect's object
    /// bookkeeping; true when the effect is gone (a Rust body,
    /// `engine::effect_update`).
    EFFECT_ALIVE @ 0x0017_94e0: [fn(effect: stack 0) -> al ret 4], Optional; "docs/engine/effects-parts.md";
    /// The effect walk's cull: true when the walk skips the effect.
    EFFECT_CULLED @ 0x0017_9730: [fn(effect: ebx) -> al ret 0], Optional; "docs/engine/effects-parts.md";
    /// Free an effect's datum and its links.
    EFFECT_DELETE @ 0x0017_74f0: [fn(effect: eax) ret 0], Optional; "docs/engine/effects-parts.md";
    /// One effect's event clock for `dt` seconds; true when it retired.
    EFFECT_UPDATE @ 0x0017_92b0: [fn(effect: stack 0, dt: stack 1) -> al ret 8], Optional; "docs/engine/effects-parts.md";
    PARTICLE_SYSTEM_UPDATE @ 0x0024_7140: [opaque], Optional; "docs/engine/engine-map.md";
    /// `animation_resource_get`: EAX a graph's resource record; loads it
    /// through the animation cache when needed and stamps the cache entry's
    /// use (`+0x14`). The tick requests (`0x001236f0`), the render gets.
    ANIMATION_RESOURCE_GET @ 0x0012_37e0: [opaque], Optional, Hot; "docs/engine/engine-map.md";
    /// The particle systems' per-view update the render reaches from the
    /// object and effect draw paths: their visibility and location stamps.
    PARTICLE_VIEW_UPDATE @ 0x0017_4420: [fn(a0: stack 0, a1: stack 1, a2: stack 2, a3: stack 3) ret 16], Optional; "docs/graphics/native-particles.md";
    /// The lightmap sample at a collision result: four stack arguments,
    /// EAX 0 when sampled, `ret 0x10`. It reads the BSP's lightmap through
    /// the geometry and texture caches; under `impl.sim.lightmap-store`
    /// the lightmap store and a Rust port answer it.
    LIGHTMAP_SAMPLE @ 0x000d_1e10: [fn(a0: stack 0, a1: stack 1, a2: stack 2, a3: stack 3) ret 16], Optional; "docs/engine/sim-render-split.md";
    /// The render-state record of an object: one stack argument (its
    /// render-state handle); EAX the record's handle or -1, `ret 4`.
    RENDER_STATE_RECORD @ 0x0003_ddd0: [fn(handle: stack 0) ret 4], Optional; "docs/engine/sim-render-split.md";
    /// A particle location's per-frame move (EBX the location), from the
    /// director's particle update (`0x00176f60` -> `0x00174990` ->
    /// `0x00174a30`): its emitters follow it when its stamp `+0x30` is the
    /// last frame's (`0x00248870`).
    PARTICLE_LOCATION_MOVE @ 0x0024_8750: [opaque], Optional; "docs/engine/sim-render-split.md";
    /// One particle's emission: the particle draw's per-particle callback
    /// (`ret 0x1c`), which evaluates its properties and calls the emitter.
    PARTICLE_EVALUATE @ 0x0022_4550: [fn(a0: stack 0, a1: stack 1, a2: stack 2, a3: stack 3, a4: stack 4, a5: stack 5, a6: stack 6) ret 28], Optional; "docs/graphics/native-particles.md";
    /// The particle emitter: eight words on the stack, EBX the corners.
    PARTICLE_EMIT @ 0x0023_ed30: [opaque], Optional; "docs/graphics/native-particles.md";
    /// A contrail's ribbon: the vertex builder the dynamic draw calls
    /// (`ret 0xc`), two vertices of 24 bytes a point.
    CONTRAIL_RIBBON @ 0x0004_2cf0: [fn(out: stack 0, bytes: stack 1, ribbon: stack 2) -> al ret 12], Optional; "docs/graphics/native-particles.md";
    CAMERA_DIRECTOR @ 0x0015_54b0: [opaque], Optional; "docs/engine/engine-map.md";
    CAMERA_OBSERVER @ 0x0016_f280: [opaque], Optional; "docs/engine/engine-map.md";
    CAMERA_FIRST_PERSON @ 0x0023_c110: [opaque], Optional; "docs/engine/engine-map.md";
    /// The per-frame UI update: one float argument, the elapsed time.
    UI_UPDATE @ 0x0014_79c3: [fn(elapsed: stack 0) ret 4], Optional; "docs/engine/engine-map.md";
    /// Updates the menu music gate and returns true when the attract movie is due.
    UI_ATTRACT_TIMER @ 0x0022_3976: [fn() -> al ret 0]; "docs/audio/bink-hle.md";
    /// Starts the attract movie through the retail Bink wrapper.
    UI_ATTRACT_START @ 0x0022_3a21: [fn() ret 0]; "docs/audio/bink-hle.md";
    /// Closes the finished or dismissed movie and resets the idle clock.
    UI_MOVIE_RETURN @ 0x0015_65a0: [fn() ret 0]; "docs/audio/bink-hle.md";
    UI_RENDER_LAYERS @ 0x0014_7af1: [fn(user: eax, fallback_user: stack 0, bounds: stack 1) ret 8], Optional; "docs/engine/engine-map.md";
    UI_HUD_DRAW @ 0x0019_17eb: [fn() ret 0], Optional; "docs/graphics/native-renderer.md";
    UI_FADE_DRAW @ 0x0004_0890: [fn() ret 0], Optional; "docs/graphics/native-renderer.md";
    UI_OVERLAY_RECT_DRAW @ 0x0013_c050: [fn(bounds: ecx, color: eax) ret 0], Optional; "docs/graphics/native-renderer.md";
    UI_LOADING_DRAW @ 0x0002_89e0: [fn() -> al ret 0], Optional; "docs/graphics/native-renderer.md";
    NETWORK_PRE_UPDATE @ 0x0008_df50: [opaque], Optional; "docs/engine/engine-map.md";
    NETWORK_POST_UPDATE @ 0x0008_dfc0: [opaque], Optional; "docs/engine/engine-map.md";
    /// The game-end path; the end latch is game globals `+0x1128`. Sets
    /// the latch once, the end delay at `+0x112c` from the tick rate, and
    /// clears `+0x1129`; a second call does nothing.
    GAME_END @ 0x0013_89c0: [fn() ret 0]; "docs/live/live-gamestate.md";
    /// Add `delta` (its low 16 bits, signed) to a player's statborg score
    /// (`+0x308 + 0x1c * slot` in the game engine globals), clamped to
    /// ±30,000, then the team score and the score limit. True when it
    /// wrote: a game engine, its state `+0x6c` 1, and the engine's
    /// authority (`+0xc04` 1, or game globals `+0x0c` 4).
    SCORE_ADD @ 0x0015_b7c0: [fn(delta: eax, player: stack 0) -> al ret 4], Optional; "docs/game/firefight.md";
    /// The gate [`SCORE_ADD`] opens with: true when that machine's engine
    /// keeps scores and lives.
    GAME_ENGINE_AUTHORITATIVE @ 0x0015_b2f0: [fn() -> al ret 0], Optional; "docs/game/firefight.md";
    /// Mark fields of the player's game-engine player entity for
    /// replication; bit `0x20` is lives remaining (player `+0x1ac`).
    PLAYER_ENTITY_DIRTY @ 0x000a_7840: [fn(player: stack 0, mask: stack 1) ret 8], Optional; "docs/game/firefight.md";
    /// True when the variant's lives setting (game globals `+0x1b8`) is
    /// positive and the unitless player has no lives left (`+0x1ac` 0).
    PLAYER_OUT_OF_LIVES @ 0x0015_db30: [fn(player: ecx) -> al ret 0], Optional; "docs/game/firefight.md";
    /// Whether a waiting player respawns this tick: counts its respawn
    /// delay (`+0x164`) down, and refuses a player out of lives.
    PLAYER_RESPAWN_READY @ 0x0015_dd40: [fn(player: eax) -> al ret 0], Optional; "docs/game/firefight.md";
    /// A player was killed: `(attacker player, victim player, flag, packed
    /// damage)` on the stack. The flag is not a betrayal flag (it is set in
    /// free-for-all too). The low byte of the fourth argument is the
    /// credited damage reporting type and modifier.
    PLAYER_KILLED @ 0x0015_ce70: [opaque]; "docs/live/live-gamestate.md";
    /// The game awards a medal: medal index EAX (its row in the medal
    /// table at `0x0046e120`) to the player in absolute slot ECX, EBX
    /// times (always 1). Every award the game makes is a call here: the
    /// kill medals from `PLAYER_KILLED` (`0x15d45f` to `0x15d524`), Carjack
    /// (`0x15e8d2`) and the flag and bomb medals (`0x240f59` on). It adds
    /// to the post-game statistics when they are on (`[0x510ca0]` set and
    /// `[0x510ca1]` clear).
    GAME_MEDAL @ 0x0019_70a0: [fn(medal: eax, player: ecx, count: ebx) ret 0], Optional; "docs/live/live-gamestate.md";
    /// The game engine's per-tick update, from `GAME_TICK` at `0x13803c`.
    /// With an engine object it runs each player's engine work with the
    /// engine's per-player slot `+0x3c`, then its per-tick slot `+0x48`,
    /// then the engine's shared steps (`0x15c000` and on). No engine
    /// object, nothing.
    ENGINE_UPDATE @ 0x0015_c150: [fn() ret 0], Optional; "docs/engine/notes/2026-11-21-engine-vtables.md";
    /// The per-player movement-speed step (`0x15c000`, called from
    /// `ENGINE_UPDATE` and from `0x162060`): for each player it asks the
    /// engine's slot `+0x4c` for a target multiplier, then ramps the
    /// player's `+0x19c` toward it (up by a fixed step a tick, snapping
    /// down, never below 0 or above the target). `+0x19c` is what the
    /// player's replicated state carries into the unit's movement.
    ENGINE_PLAYER_SPEEDS @ 0x0015_c000: [fn() ret 0], Optional; "docs/game/game-options.md";
    /// Slayer's kill callback, vtable `0x45c6d8` slot `+0x78`, which
    /// `PLAYER_KILLED` calls last (`0x15d6a4`): thiscall with the engine
    /// object, then that routine's attacker, victim and flag, and a kill
    /// kind it works out. It is all of Slayer's kill scoring, through
    /// `SCORE_ADD`: +1 to the attacker, or -1 when the flag is set (a
    /// suicide only with type-area flag bit 1); -1 to the victim with bit
    /// 2; a bonus to the attacker with bit 0. Nothing in a distributed
    /// client (`[0x4e6948]+0xc` 4).
    SLAYER_KILL @ 0x002b_c000: [fn(engine: ecx, attacker: stack 0, victim: stack 1, flag: stack 2, kind: stack 3) ret 16], Optional; "docs/engine/notes/2026-11-21-engine-vtables.md";
    /// Juggernaut's kill callback, slot `+0x78` of vtable `0x453d38`, as
    /// [`SLAYER_KILL`]: moves the Juggernaut role and scores through
    /// `SCORE_ADD` (`0x19238c`, `0x1923db`).
    JUGGERNAUT_KILL @ 0x0019_22a0: [fn(engine: ecx, attacker: stack 0, victim: stack 1, flag: stack 2, kind: stack 3) ret 16], Optional; "docs/engine/notes/2026-11-21-engine-vtables.md";
    /// Oddball's per-tick slot `+0x48`: each ball's carrier scores through
    /// `0x2bc990` and `0x2bce70`, which calls `SCORE_ADD`.
    ODDBALL_TICK @ 0x002b_cbb0: [fn(engine: ecx) ret 0], Optional; "docs/engine/notes/2026-11-21-engine-vtables.md";
    /// King of the Hill's per-tick slot `+0x48`; its hill scoring
    /// `0x2beee0` (from `0x2bd729`) calls `SCORE_ADD`.
    KOTH_TICK @ 0x002b_d680: [fn(engine: ecx) ret 0], Optional; "docs/engine/notes/2026-11-21-engine-vtables.md";
    /// Territories' per-tick slot `+0x48`; it ends in a tail jump to its
    /// scoring `0x2c0a90`, which calls `SCORE_ADD`.
    TERRITORIES_TICK @ 0x002c_0230: [fn(engine: ecx) ret 0], Optional; "docs/engine/notes/2026-11-21-engine-vtables.md";
    /// Juggernaut's per-tick slot `+0x48`.
    JUGGERNAUT_TICK @ 0x0019_2530: [fn(engine: ecx) ret 0], Optional; "docs/engine/notes/2026-11-21-engine-vtables.md";
    /// A starting location's weight for the spawn chooser: the influence
    /// sum `0x23b060` over the context at its position, plus the random
    /// term (the game seed's top 16 bits / 65535, times the multiplayer
    /// globals' `+0x530` element's first float). Its one caller is
    /// `PLAYER_SPAWN_LOCATION` (`0x14efb1`); EAX is the location
    /// (`scnr+0x104` + `0x34` × index), ESI the seed.
    SPAWN_LOCATION_WEIGHT @ 0x0023_ba90: [fn(location: eax, player: stack 0, flag: stack 1, context: stack 2) -> xmm0 ret 12], Optional; "docs/engine/notes/2026-11-27-forge-spawner-rows.md";
    /// The netgame item spawner: makes entry `index`'s object
    /// (`scnr+0x120`, stride `0x90`), stores its handle at the entry's
    /// state (`state + 8 * index`) and the index at the object's `+0xaf`.
    /// Its one caller is the per-tick update `0x15ae70` (`0x15b1aa`), on
    /// the authority only.
    NETGAME_ITEM_SPAWN @ 0x0015_aad0: [fn(state: stack 0, index: stack 1) ret 8], Optional; "docs/engine/notes/2026-11-27-forge-spawner-rows.md";
    /// The game engine's round reset: clears the engine globals' round
    /// block (`E+0x74`, 27 dwords), resets each player's round bytes
    /// (`0x157a80`) and ends in a tail jump to the engine's slot `+0x0c`.
    /// No engine, nothing. Called once from the game's start inside
    /// `GAME_ACTIVATE` (`0x18f05e` → `0x158530` → `0x158556`) and from
    /// the round state machine `0x15c550` as each later round begins
    /// (`0x15c5de`); neither call runs in a distributed client.
    ENGINE_ROUND_RESET @ 0x0015_7ae0: [fn() ret 0], Optional; "docs/engine/notes/2026-11-23-round-start-and-player-quit.md";
    /// The game engine's round end, the game going on: outside a
    /// distributed client it asks the round state machine for state 2
    /// (`E+0xc04`, `0x15b997`), and with the end latch clear and `quiet`
    /// zero it posts the round-over event (kind `0x1f`). The machine then
    /// runs the round's end and the gap between rounds and starts the next
    /// one through [`ENGINE_ROUND_RESET`]. Its one caller is the round
    /// resolution `0x15b3a0`, when the variant's rounds go on.
    ROUND_END @ 0x0015_b980: [fn(quiet: stack 0) ret 4], Optional; "docs/engine/notes/2026-11-27-next-round.md";
    /// The game engine's player-left notice: ESI the salted player datum
    /// of a player `0x152df0` has just taken out of the game (its unit
    /// detached, player `+2` bit 1 set, bit 0 cleared). It calls the
    /// engine's slot `+0x1c` with the player (Juggernaut's `0x1921f0`
    /// clears the player's role bit) and posts an engine event
    /// (`0x19eb30`). No engine, nothing.
    ENGINE_PLAYER_LEFT @ 0x0015_7de0: [fn(player: esi) ret 0], Optional; "docs/engine/notes/2026-11-23-round-start-and-player-quit.md";
    /// Initialize an untargeted game-engine event in the caller's record.
    GAME_EVENT_NEW @ 0x0019_ebb0: [fn(event: eax, kind: ecx, index: edx) ret 0]; "docs/game/notes/2026-11-24-game-type-engine-reads.md";
    /// Posts an event to the simulation's remote machines and shows it locally.
    GAME_EVENT_POST @ 0x0019_eb90: [fn(event: eax) ret 0]; "docs/game/notes/2026-11-24-game-type-engine-reads.md";
    /// Network and local delivery of a game-engine event.
    GAME_EVENT_SEND @ 0x000a_7c50: [fn(event: edx) ret 0]; "docs/game/notes/2026-11-24-game-type-engine-reads.md";
    GAME_EVENT_SHOW @ 0x0019_eb30: [fn(event: edi) ret 0]; "docs/game/notes/2026-11-24-game-type-engine-reads.md";
    /// A local player's announcement of an event: the row's HUD text and
    /// the announcer sound. `GAME_EVENT_SHOW` reaches it for the event's
    /// player.
    /// The event's player's announcement row, by the event's subjects in
    /// order, announced through `GAME_EVENT_ANNOUNCE`.
    GAME_EVENT_SUBJECTS @ 0x0019_de80: [fn(event: eax, player: esi) ret 0]; "docs/engine/sim-rewrite-sketches.md";
    /// The first announcement row of the event type's block that matches
    /// the event, the kind and the player, or 0.
    GAME_EVENT_ROW @ 0x0019_df10: [fn(player: eax, event: edi, kind: stack 0) -> eax ret 4]; "docs/engine/sim-rewrite-sketches.md";
    GAME_EVENT_ANNOUNCE @ 0x0019_e890: [fn(player: eax, row: ecx, event: stack 0) ret 4]; "docs/engine/sim-rewrite-sketches.md";
    /// The per-tick slot `+0x48` that CTF and Assault share (vtable
    /// `0x459d18`). Their capture scoring `0x242e00` runs from the
    /// per-player slot `+0x3c` and from `0x242741`, not from here.
    CTF_TICK @ 0x0024_0d70: [fn(engine: ecx) ret 0], Optional; "docs/engine/notes/2026-11-21-engine-vtables.md";
    /// The base trait query: Slayer's slot `+0x8c`, and where every other
    /// engine's `+0x8c` ends for the traits it does not answer itself
    /// (Juggernaut `0x19273c`, CTF and Assault `0x242084`, Oddball
    /// `0x2bcd03`, KotH `0x2bda4e`, Territories' `0x2c02e0` is a jump).
    /// True when the variant gives `player` the trait: 1 active camo
    /// (record flag bit 2), 2 extra damage (bit 12), 3 damage resistance
    /// (bit 13). The slot is called at `0x15b28e`, `0x2404ad`, `0x2bc967`.
    ENGINE_TRAIT_QUERY @ 0x0024_dda0: [fn(engine: ecx, player: stack 0, trait: stack 1) -> al ret 8], Optional; "docs/engine/notes/2026-11-21-engine-vtables.md";
    // Retail overrides of the same slot. CTF and Assault share their body.
    CTF_TRAIT_QUERY @ 0x0024_2010: [fn(engine: ecx, player: stack 0, trait: stack 1) -> al ret 8], Optional; "docs/engine/notes/2026-11-21-engine-vtables.md";
    ODDBALL_TRAIT_QUERY @ 0x002b_cc90: [fn(engine: ecx, player: stack 0, trait: stack 1) -> al ret 8], Optional; "docs/engine/notes/2026-11-21-engine-vtables.md";
    KING_TRAIT_QUERY @ 0x002b_d9d0: [fn(engine: ecx, player: stack 0, trait: stack 1) -> al ret 8], Optional; "docs/engine/notes/2026-11-21-engine-vtables.md";
    JUGGERNAUT_TRAIT_QUERY @ 0x0019_26c0: [fn(engine: ecx, player: stack 0, trait: stack 1) -> al ret 8], Optional; "docs/engine/notes/2026-11-21-engine-vtables.md";
    TERRITORIES_TRAIT_QUERY @ 0x002c_02e0: [fn(engine: ecx, player: stack 0, trait: stack 1) -> al ret 8], Optional; "docs/engine/notes/2026-11-21-engine-vtables.md";
    /// The game variant record clean-up: EBX the `0x130`-byte record.
    /// Clamps every field, the engine index (`+0x44`) to 1..9 and the type
    /// area by engine (`0x19de54`; 5 and 6 always fail), then compares the
    /// record with its copy from entry: unchanged, AL 1 (EAX 1, ECX and EDX
    /// 0, `0x19de4b`); changed, the whole record becomes a default Slayer
    /// record and AL 0. Called by the codec's decode (`0x7d9b7`), the
    /// options copy at game start (`0x137e05`) and the content reads.
    VARIANT_CLEAN_UP @ 0x0019_d650: [fn(record: ebx) -> al ret 0], Optional; "docs/game/notes/2026-11-24-game-type-engine-reads.md";
    /// The variant record's wire encode: the bit stream (`+0` words, `+4`
    /// size in bytes, `+0x10` bit position) and the record on the stack.
    /// Writes the index (4 bits), then for a nonzero one the common fields
    /// and the type area through a jump table with no bounds check
    /// (`0x7d151`, table `0x7d4f8`): 5, 6 and anything out of 1..9 jump
    /// through a zero or a stray entry.
    VARIANT_ENCODE @ 0x0007_cc50: [fn(writer: stack 0, record: stack 1) ret 8], Optional; "docs/game/notes/2026-11-24-game-type-engine-reads.md";
    /// The variant record's wire decode: EAX the record out, ECX the bit
    /// stream. Reads the fields the encode writes, refuses 5, 6 and
    /// anything out of 1..9 at the type area (`0x7d82a`), zeroes the
    /// record for index 0 (AL 1), and otherwise answers the clean-up's AL
    /// on a copy (`0x7d9b7`).
    VARIANT_DECODE @ 0x0007_d520: [fn(record: eax, reader: ecx) -> al ret 0], Optional; "docs/game/notes/2026-11-24-game-type-engine-reads.md";
    /// The lobby protocol map: ECX the variant record; EAX 2 (distributed)
    /// for `+0x44` 1..4 and 7..9, 1 (synchronous) for 5, 6 and anything out
    /// of range, through a byte table (`0x161cbc`). Called from the
    /// protocol choice `0x64060` (`0x640a7`) and `0x71480` (`0x7155d`).
    VARIANT_PROTOCOL @ 0x0016_1c90: [fn(record: ecx) -> eax ret 0], Optional; "docs/live/live-coop.md";
    /// The content category a saved or listed variant is filed under: EAX
    /// the record; EAX 7 CTF, 8 Assault, 1 Slayer, 4 Oddball, 2 KotH, 9
    /// Territories, 5 Juggernaut, through a jump on `+0x44` with no bounds
    /// check (table `0x216a88`, zero for 5 and 6). Called from `0x2170af`,
    /// `0x2baa29`, `0x2ca45d` and `0x2ca49b`.
    VARIANT_CATEGORY @ 0x0021_6a50: [fn(record: eax) -> eax ret 0], Optional; "docs/game/notes/2026-11-24-game-type-engine-reads.md";
    /// Juggernaut's role setter: sets (ECX's low byte nonzero) or clears
    /// the player's bit of the role mask `[JUGGERNAUT_GLOBALS]+0` (`u16`;
    /// EAX the salted player datum, its index the bit). Setting posts the
    /// engine event that announces the Juggernaut (`0xa7c50`, `0x19eb30`),
    /// except in a distributed client; either way it marks the
    /// engine-globals entity (`E+0x24`, bit `0x20`) and rebuilds the teams'
    /// mask at `+2` (`0x192b20`). Juggernaut's trait query `0x1926c0`
    /// answers the role's traits from the mask. [`JUGGERNAUT_KILL`] and the
    /// role pick `0x192a30` call it.
    JUGGERNAUT_ROLE_SET @ 0x0019_2970: [fn(player: eax, on: ecx) ret 0], Optional; "docs/engine/notes/2026-11-22-script-scoring-and-roles.md";
    /// Oddball's ball return: ball `n` (EAX; its objective record's team,
    /// `+4`) back to its spawn flag (`[ODDBALL_GLOBALS]+0xc+4n`, a netgame
    /// flag's position) through [`OBJECTIVE_RETURN`]. Its idle timer
    /// (`0x2bc990`) calls it; the caller checks the ball has a spawn.
    ODDBALL_BALL_RETURN @ 0x002b_c340: [fn(ball: eax) ret 0], Optional; "docs/engine/notes/2026-11-22-script-scoring-and-roles.md";
    /// The objective return the engines share (Oddball `0x2bc361`, CTF
    /// `0x242b41`, `0x242c7e`): drops the objective object (ESI) from the
    /// unit carrying it, moves it to the position (a pointer to three
    /// floats) with the default orientation, and clears its `+0x14c` and
    /// `+0x150`. No engine, nothing.
    OBJECTIVE_RETURN @ 0x0015_e360: [fn(object: esi, position: stack 0) ret 4], Optional; "docs/engine/notes/2026-11-22-script-scoring-and-roles.md";
    /// One weapon barrel fires: stdcall `(weapon object datum, barrel
    /// index)`. One call is one shot (likely).
    WEAPON_FIRE @ 0x0010_4150: [fn(weapon: stack 0, barrel: stack 1) ret 8], Optional; "docs/live/live-gamestate.md";
    /// Map a packed reporting code to its multiplayer weapon statistics bucket.
    WEAPON_STAT_BUCKET @ 0x0019_6ef0: [fn(code: ecx) -> eax ret 0], Optional; "docs/runtime/sim.md";
    /// Object deletion before the salted header and body are freed.
    OBJECT_DELETE @ 0x000b_8540: [fn(object: stack 0) ret 4], Optional; "docs/game/skulls.md";
    /// Retail calls this after object_new succeeds for a barrel projectile.
    BARREL_PROJECTILE @ 0x000f_a100: [fn(projectile: ecx) ret 0]; "docs/runtime/agent-api.md";
    /// Projectile update passes the completed collision result before resolving damage.
    PROJECTILE_IMPACT @ 0x000f_aa60: [opaque]; "docs/runtime/agent-api.md";
    /// Attach a weapon to a unit, or detach when the unit is -1.
    ITEM_SET_UNIT @ 0x0010_ca80: [fn(item: eax, unit: stack 0) ret 4], Optional; "docs/game/bots.md";
    /// Successful power-up application before the consumed equipment is deleted.
    POWERUP_TAKEN @ 0x000a_89c0: [fn(unit: ecx, definition: edx) ret 0], Optional; "docs/game/bots.md";
    /// A damage report is applied to its victim: the report in EDI, the
    /// victim object datum on the stack. Called once per damage instance.
    DAMAGE_REPORT @ 0x000d_9640: [fn(report: edi, victim: stack 0) ret 4], Optional; "docs/live/live-gamestate.md";
    /// A damage instance's shield step (`0xd7b80` calls it once, at
    /// `0xd80de`): stdcall `(object datum, ?, damage data, record)`. It
    /// takes the shield's share of the amount at `record+0x1c` (divided by
    /// the object's shield vitality) and leaves what is left there.
    SHIELD_DAMAGE @ 0x000d_9110: [fn(object: stack 0, record: stack 3) ret 16], Optional; "docs/game/game-options.md";
    /// A damage instance's body step (`0xd7b80` calls it once, at
    /// `0xd8184`): the record in EDI, stdcall `(object datum, ?, damage
    /// data)`. Reads the amount at `record+0x1c` through `0xd9020`.
    BODY_DAMAGE @ 0x000d_8cb0: [fn(record: edi, object: stack 0) ret 12], Optional; "docs/game/game-options.md";
    /// Wheel and tread controllers after the driver inputs have been read.
    VEHICLE_TREAD_UPDATE @ 0x000f_1cd0: [fn(object: ebx, contacts: stack 0) ret 4]; "docs/halo1/role-audit.md";
    UNIT_CONTROL_REFRESH @ 0x000c_e0c0: [fn(object: edi) ret 0]; "docs/halo1/role-audit.md";
    VEHICLE_GRAVITY_DELTA @ 0x000f_7ed0: [fn(object: eax, delta: esi) ret 0]; "docs/halo1/role-audit.md";
    VEHICLE_CONTACT_FORCES @ 0x0020_6090: [fn(contacts: stack 0, force: stack 1, torque: stack 2) -> al ret 12]; "docs/halo1/role-audit.md";
    VEHICLE_CONTACT_SOLVE @ 0x0020_56e0: [fn(object: stack 0, contacts: stack 1, fraction: stack 2, force: stack 3, torque: stack 4) ret 20]; "docs/halo1/role-audit.md";
    VEHICLE_WHEEL_UPDATE @ 0x000f_1f80: [fn(object: stack 0, contacts: stack 1) ret 8]; "docs/halo1/role-audit.md";
    /// An object's damage update: stdcall `(object datum)`, AL whether
    /// anything changed. Called once per object per tick by
    /// `object_update` (`0xbc470`, its only caller): stun timers, shield
    /// recharge, and the body's regeneration (`0xd63de`..`0xd64b5`).
    OBJECT_DAMAGE_UPDATE @ 0x000d_5de0: [fn(object: stack 0) -> al ret 4]; "docs/halo1/health.md";
    /// Update one effect's native particle systems for this event step.
    EFFECT_STEP_EMITTERS @ 0x0017_9020: [fn(effect: stack 0, elapsed: stack 1, dt: stack 2) ret 12]; "docs/halo1/objects.md";
    /// Dispatch one native movement operator over an emitter's particle list.
    PARTICLE_MOVEMENT_STAGE @ 0x002b_8da0: [fn(stage: eax, head: esi, dt: stack 1) ret 12]; "docs/halo1/objects.md";
    /// Cloth geometry update, per-node integration and material lookup.
    CLOTH_UPDATE @ 0x0011_68a0: [fn() ret 0]; "docs/halo1/objects.md";
    CLOTH_INTEGRATE @ 0x0011_8140: [fn(cloth: eax) ret 0]; "docs/halo1/objects.md";
    CLOTH_SHADER @ 0x0011_6ac0: [fn(cloth: stack 0) -> eax ret 4]; "docs/halo1/objects.md";
    /// Queue an attachment endpoint with its animated marker transform (0x41bb3).
    LIGHTNING_QUEUE @ 0x0004_29a0: [fn(owner: stack 0, tag: stack 1) ret 24]; "docs/halo1/objects.md";
    /// Submit 0x18-byte light-volume samples (0x4f792).
    LIGHT_VOLUME_SPRITES @ 0x0004_f7e0: [fn(bitmap: ecx, samples: stack 0, count: stack 1) -> al ret 32]; "docs/halo1/objects.md";
    /// Initialise a particle's local position and velocity at birth.
    PARTICLE_INITIALISE @ 0x0024_9170: [fn(emitter: stack 0, particle: edi) ret 8]; "docs/halo1/objects.md";
    /// Begin, update and remove a command on the Halo 2 AI scheduler.
    AI_COMMAND_BEGIN @ 0x0025_8cf0: [fn(actor: stack 0, unit: stack 1, state: stack 2, command: stack 3) -> eax ret 16]; "docs/halo1/ai.md";
    AI_COMMAND_UPDATE @ 0x0025_9430: [fn(actor: stack 0, unit: stack 1, state: stack 2, command: stack 3) -> eax ret 16]; "docs/halo1/ai.md";
    AI_COMMAND_REMOVE @ 0x0025_8540: [fn(actor: ecx, command: stack 0) ret 4]; "docs/halo1/ai.md";
    OBJECT_ANIMATION_BLEND @ 0x000b_a350: [fn(unit: esi, duration: stack 0) ret 4]; "docs/halo1/ai.md";
    /// Finish the character physics mode's velocity calculation.
    CHARACTER_MOTION_STATE @ 0x001e_5b50: [fn(input: eax, state: ecx, output: esi) ret 0]; "docs/halo1/ai.md";
    AI_COMMAND_TARGETS @ 0x0025_a380: [fn(actor: stack 0) ret 4]; "docs/halo1/ai.md";
    /// Clear HS threads and instantiate scenario globals and persistent scripts.
    HS_NEW_MAP @ 0x0020_90a0: [fn() ret 0]; "docs/halo1/scripts.md";
    /// Attach the new scenario's syntax nodes (`0x16b6b0`) and strings,
    /// then tail-jump to `HS_NEW_MAP`.
    HS_MAP_ATTACH @ 0x0020_a750: [fn() ret 0]; "docs/halo1/scripts.md";

    /// HS global write-back notification (EAX is the variable index).
    HS_GLOBAL_STORED @ 0x0020_a490: [fn(index: eax) ret 0]; "docs/halo1/ai.md";
    /// Restore an object's body to full (EAX the object datum), AL 1 when
    /// it did (refused when full or depleted). Its only caller is the
    /// powerup pickup's health case (`0x151f73`).
    BODY_RESTORE @ 0x000d_6790: [fn(object: eax) -> al ret 0]; "docs/halo1/health.md";
    /// `locale_string_id_find`: the string for the packed id in EDI, its
    /// address in EAX.
    LOCALE_FIND @ 0x001a_02d0: [fn(id: edi) -> eax ret 0], Hot; "docs/engine/locale_string_id_find.md";
    /// `locale_string_id_exists`: whether the packed id has a string (AL).
    LOCALE_EXISTS @ 0x001a_0310: [opaque]; "docs/engine/locale_string_id_find.md";
    /// `ice_cream_flavor_stock`'s worker: turns skull ESI on (byte ESI of
    /// the 15-byte array at `0x4f55dc`, only when it was 0), then the HUD
    /// message and sound.
    SKULL_STOCK @ 0x001d_f3e0: [fn(index: esi) ret 0]; "docs/game/skulls.md";
    /// Queue new game options (EAX = the options; `+0` the mode, 1 a
    /// campaign). The menu's campaign start and level boot both end here.
    OPTIONS_QUEUE @ 0x0018_e790: [fn(options: eax) ret 0]; "docs/game/skulls.md";
    /// `render_frame_and_present`: renders the game views (or the fallback
    /// screen) and presents. Once a frame, from the main loop.
    RENDER_FRAME @ 0x0022_3240: [fn() ret 0]; "docs/engine/engine-map.md";
    /// `render_game_views`: builds the per-player view records and cameras,
    /// then calls `render_views` (from `0x00223714`). No arguments; it
    /// keeps one local and four saved registers, `ret`.
    /// `render_bridge::native_scene_lists::view_records` implements it.
    RENDER_GAME_VIEWS @ 0x0022_34e0: [fn() ret 0]; "docs/graphics/native-visibility.md";
    /// `render_views`: begin-frame setup, the loop over view records, end of
    /// drawing.
    RENDER_VIEWS @ 0x0002_b5d0: [fn(count: eax, context: ecx, arg0: stack 0, arg1: stack 1, views: stack 2) ret 12]; "docs/engine/engine-map.md";
    /// `render_game_view`: one game view (camera, render state, scene
    /// passes, UI composition).
    RENDER_GAME_VIEW @ 0x0002_b790: [fn(view: stack 0, flags: stack 1) ret 8]; "docs/engine/engine-map.md";
    /// The alternate view path selected by the render view record.
    RENDER_ALTERNATE_VIEW @ 0x0002_c790: [opaque]; "docs/graphics/render-loop.md";
    /// End-of-frame rendering after the view loop, before the final blit:
    /// the freeze frame's copy and composite. No arguments, `ret`.
    /// `render_bridge::frame_tail` implements it.
    RENDER_FRAME_TAIL @ 0x0001_3b90: [opaque]; "docs/graphics/retire-d3d-display.md";
    /// The frame's finish after the tail (`render_views` `0x0002b6f7`, the
    /// alternate frames `0x0002c9e9`, `0x0002cb14`): the four texture
    /// stages, the index buffer and the pixel shader reset. No arguments,
    /// `ret` (its tail jump to `D3DDevice_SetPixelShader`).
    /// `render_bridge::native_targets` implements it.
    RENDER_FRAME_FINISH @ 0x0001_3cd0: [fn() ret 0]; "docs/graphics/retire-d3d-display.md";
    /// The view setup: the view's `0x298` bytes into the render context
    /// `0x00485600`, the stencil off, the transform constants, the view's
    /// target, clear and viewport, the fill mode, the draw state's reset
    /// and the fog. `ret 4`. `render_bridge::view_setup` implements it.
    RENDER_VIEW_SETUP @ 0x0001_32d0: [fn(view: stack 0) ret 4]; "docs/graphics/retire-d3d-display.md";
    /// The view end at the tail of each view: the alpha-only clear, the
    /// view stack's pop and the setup and camera of the view left. No
    /// arguments, `ret`. `render_bridge::view_setup` implements it.
    RENDER_VIEW_END @ 0x0001_3c20: [fn() ret 0]; "docs/graphics/retire-d3d-display.md";
    /// The frame outside a game view: the menus, the loading screen and
    /// the movie, through `0x0002c8a0`. No arguments, `ret`. Called by the
    /// frame step and the map load's two frame loops (`0x00163a1a`,
    /// `0x00163acd`).
    RENDER_OUTSIDE_VIEW @ 0x0022_32a0: [fn() ret 0]; "docs/graphics/retire-d3d-display.md";
    /// The present: the optional readback (`readback` 0 asks none), the
    /// border `0x00013d80`, the swap, the display latch and the present
    /// counter; `ret 8`. Callers: the frame step (`0x00223296`) and three
    /// frame loops outside it (`0x0012b784`, `0x00163a28`, `0x00163adb`).
    RENDER_PRESENT @ 0x0001_3f10: [fn(readback: stack 0, flags: stack 1) ret 8]; "docs/graphics/retire-d3d-display.md";
    /// A frame loop outside `0x00223240`: one view through `0x0002ca00`
    /// with the callback `0x002233a0`, the frame wait and the present.
    /// One stack argument, `ret 4`.
    RENDER_FRAME_LOOP @ 0x0012_b6f0: [opaque]; "docs/graphics/retire-d3d-display.md";
    /// `render_scene_passes`: the scene pass sequence of one view.
    RENDER_SCENE_PASSES @ 0x0002_bcd0: [opaque]; "docs/graphics/e3-lighting.md";
    /// The water end after the scene's layer 6 (`0x0002c132`): the ocean
    /// of the active BSP's first water definition, drawn when the first
    /// argument's byte is set, the bound target's alpha cleared before it
    /// when the second's is. Two stack arguments, `ret 8`, AL 1.
    WATER_TEARDOWN @ 0x0003_c3c0: [opaque]; "docs/graphics/retire-d3d-display.md";
    /// Selects the current view's sky and draws its model. Reads no
    /// arguments and returns with `ret` (`h2 disasm 0x3ede0`), so the
    /// sky's cut passes over it (`render_bridge::native_sky::before_sky`).
    RENDER_SKY @ 0x0003_ede0: [fn() ret 0]; "docs/graphics/native-renderer.md";
    /// Builds the viewport clear flags and issues the device clear.
    SCENE_CLEAR @ 0x0001_4b60: [opaque]; "docs/graphics/native-renderer.md";
    /// Selects the colour and depth targets, including cached selections.
    SCENE_TARGET_SELECT @ 0x0001_cd30: [opaque]; "docs/graphics/native-renderer.md";
    /// The flare array's walker: for each entry of a view and class mask,
    /// polls, draws and ends its visibility test (the query pass's call),
    /// or draws it (the draw pass's). Six stack arguments, `ret 0x18`.
    VISIBILITY_WALK @ 0x0002_0bb0: [opaque]; "docs/graphics/d3d-visibility.md";
    /// The flare array's reader: each entry's byte from its test two
    /// frames back. No arguments, `ret`.
    VISIBILITY_READ @ 0x0002_0770: [opaque]; "docs/graphics/d3d-visibility.md";
    /// One late-list glow of the flare draw pass (`eax` the lens datum,
    /// `ecx` its position), called from `0x0002e1c5`. `ret`.
    FLARE_GLOW @ 0x0004_6ea0: [opaque]; "docs/graphics/d3d-visibility.md";
    /// The planar fog's screen quad, called between the sky and layer 16
    /// (`0x0002c0d1`): its setup by view mode, the fan `0x00034420`, then
    /// the depth clip control set to 0. No arguments, `ret`.
    PLANAR_FOG @ 0x0002_5d50: [fn() ret 0]; "docs/graphics/retire-d3d-fog-quad.md";
    /// The second fog's screen fans, called after the transparents
    /// (`0x0002c19a`, view kind not 2): with the datum `[0x004857b8]` set
    /// and `[0x004857d8]` above 0, the fans `0x00034420` of the setups
    /// `0x00026880` and `0x00026e50` (`cl` set) or `0x000276d0` (`dl`).
    /// `ret`.
    FOG_FANS @ 0x0002_5dc0: [opaque]; "docs/graphics/retire-d3d-fog-fans.md";
    /// The second fog's composite setup (`dl` alone), which the fan
    /// `0x00034420` calls through EDI (return `0x0003452e`): one stack
    /// argument, `ret 4`, AL whether the fan draws.
    FOG_COMPOSITE_SETUP @ 0x0002_76d0: [opaque]; "docs/graphics/retire-d3d-fog-fans.md";
    /// The second fog's layers setup (`cl`) and their composite's, which
    /// the fan `0x00034420` calls through EDI (return `0x0003452e`): one
    /// stack argument, `ret 4`, AL whether the fan draws.
    FOG_LAYERS_SETUP @ 0x0002_6880: [opaque]; "docs/graphics/retire-d3d-fog-fans.md";
    FOG_LAYERS_COMPOSITE_SETUP @ 0x0002_6e50: [opaque]; "docs/graphics/retire-d3d-fog-fans.md";
    /// The planar fog quad's setup for view modes 2 and 3, which the fan
    /// `0x00034420` calls through EDI (return `0x0003452e`): one stack
    /// argument, `ret 4`, AL whether the fan draws.
    PLANAR_FOG_SETUP @ 0x0002_6520: [opaque]; "docs/graphics/retire-d3d-fog-quad.md";
    /// The planar fog quad's setup for view modes 1, 4 and 5, which the
    /// fan `0x00034420` calls through EDI (return `0x0003452e`): one stack
    /// argument, `ret 4`, AL whether the fan draws.
    PLANAR_FOG_OTHER_SETUP @ 0x0002_5e50: [opaque]; "docs/graphics/retire-d3d-fog-quad.md";
    /// Resets the record counters immediately before the two structure walks.
    RESET_SCENE_RECORDS @ 0x0002_cb30: [fn() ret 0]; "docs/graphics/occlusion-culling.md";
    /// Main view's visible structure cluster and placement record walks.
    WORLD_CLUSTER_RECORDS @ 0x0013_3390: [fn(visibility: ebx) ret 0]; "docs/graphics/native-renderer.md";
    /// One visible row entering the gather. A cold neighbour can require
    /// the surrounding walk to stay in the guest.
    WORLD_VISIBILITY_FLAGS @ 0x0013_32f0: [fn(visibility: stack 0) -> eax ret 4]; "docs/graphics/native-renderer.md";
    WORLD_PLACEMENT_RECORDS @ 0x0013_3520: [fn(visibility: stack 0) ret 4]; "docs/graphics/native-renderer.md";
    /// Sets the render stencil mode, including after the scene passes return.
    RENDER_STENCIL_MODE @ 0x0001_5370: [opaque]; "docs/graphics/native-renderer.md";
    /// The additional shadow-caster pass (layer 10/11 pair after the main lights).
    ADDITIONAL_SHADOW_CASTERS @ 0x0003_0ed0: [opaque]; "docs/graphics/native-shadows.md";
    /// Target selection helper. The light tail restores its view target here
    /// after the last geometry, including on a light with no active layer.
    LIGHT_RESTORE_TARGET @ 0x0001_4bc0: [opaque]; "docs/graphics/native-renderer.md";
    /// The scene call that walks the object shadow list.
    OBJECT_SHADOW_LIST @ 0x0003_35e0: [fn(mode: stack 0) ret 4]; "docs/graphics/native-shadows.md";
    /// Gathers the object shadow candidates.
    GATHER_OBJECT_SHADOWS @ 0x0003_3400: [fn(list: ebx, rows: stack 0, mode: stack 1) -> eax ret 8]; "docs/graphics/native-shadows.md";
    /// One object's shadow (caster and receiver layers).
    OBJECT_SHADOW @ 0x0003_2350: [fn(caster: stack 0, fade: stack 1) -> al ret 8]; "docs/graphics/native-shadows.md";
    /// The colour-mask filter between an object shadow's layers 10 and 11.
    OBJECT_SHADOW_FILTER @ 0x0002_22a0: [fn() ret 0]; "docs/graphics/native-shadow-layers.md";
    /// One dynamic decal callback: `0x00163cd0` calls it in stored list order
    /// after `0x00043a40` selects the mode and its state. Stack `(list, unused)`,
    /// `ret 8`; the only native decal replacement point.
    DECAL_RENDER @ 0x0004_4040: [fn(list: stack 0, unused: stack 1) ret 8]; "docs/graphics/native-decals-research.md";
    /// The draw of decal decorators (types 1 and 2): one layer-15 pass of
    /// one placement, called by the sorted walk `0x00040f60` after the
    /// build `0x0004dfa0`. Stack `(shader, 0, 0, layer, pass, argument,
    /// entry)`, `ret 0x1c`; the native stage's cut point.
    DECAL_DECORATOR_DRAW @ 0x0004_ebd0: [fn(shader: stack 0, zero: stack 1, zero2: stack 2, layer: stack 3, pass: stack 4, argument: stack 5, entry: stack 6) ret 28]; "docs/graphics/retire-d3d-decal-decorators.md";
    /// `render_decal_mode` (`0x0002c370`): one mode of the dynamic decals,
    /// EAX the mode 0 to 6: the mode's state (`0x00043a40`), then
    /// `DECAL_RENDER` per entry of `[0x00547f94]`. `render_scene_passes`
    /// calls it seven times for view kinds 1 and 2; mode 1 (return
    /// `0x0002c04d`) is the next call after `RENDER_DYNAMIC_LIGHTS`
    /// returns, with no test between them.
    RENDER_DECAL_MODE @ 0x0002_c370: [opaque]; "docs/graphics/native-decals-research.md";
    /// `render_dynamic_lights`: the visible lights, each gathered and drawn
    /// with `RENDER_LIGHT_PASSES` (`render_scene_passes` calls it after
    /// layer 7).
    RENDER_DYNAMIC_LIGHTS @ 0x0003_1c80: [opaque]; "docs/graphics/native-renderer.md";
    /// The first-person draw of a game view: `eax` the view, the call (0 or
    /// 1) on the stack, `ret 4`. The native first person cuts it
    /// (`render_bridge::cut::FIRST_PERSON`) and render-timing times it.
    FIRST_PERSON_DRAW @ 0x0003_fdf0: [opaque]; "docs/graphics/native-objects.md";
    /// `render_light_passes`: draws one dynamic light's passes (layers 10,
    /// 11, 14, 12, 13 on the light's list). EAX the `ligh` datum, ECX a
    /// context, stack `(light datum, &position, shadow byte, weight)`,
    /// `ret 0x10`; its only caller is `RENDER_DYNAMIC_LIGHTS`.
    RENDER_LIGHT_PASSES @ 0x0003_1170: [fn(definition: eax, context: ecx, light: stack 0, position: stack 1, shadow: stack 2, weight: stack 3) ret 16]; "docs/graphics/e3-lighting.md";
    /// `0x00040e90`: the item lists of the visibility struct the records
    /// were written for (`[0x004c0b80]`); EAX the mode (0: every record).
    /// In the main view's kind (`[vis+0x2a78]` 0) list 0; in a light's,
    /// lists 1 and 2 and the first-person table. `RENDER_LIGHT_PASSES`
    /// calls it first, with EAX 0.
    SCENE_ITEMS @ 0x0004_0e90: [fn(mode: eax) ret 0]; "docs/graphics/native-light-gather.md";
    /// `0x000404b0`: the first-person lights, from `0x0002c2b9` between
    /// the two first-person draws. Per slot of the first-person table with
    /// records: the light volume, the slot's lists
    /// (`FIRST_PERSON_LIGHT_RECORDS`), the constants, the flags, layers
    /// 11, 14, 12 and 13 on the light's list and the passes' tail. Stack
    /// `(mode)`, `ret 4`.
    FIRST_PERSON_LIGHTS @ 0x0004_04b0: [fn(mode: stack 0) ret 4]; "docs/graphics/native-light-gather.md";
    /// `0x000402b0`: one first-person light's lists 1 and 2 from its
    /// slot's records, copied after the main view's records. EDX the slot,
    /// stack `(mode)`, `ret 4`; its only caller is `FIRST_PERSON_LIGHTS`.
    FIRST_PERSON_LIGHT_RECORDS @ 0x0004_02b0: [fn(slot: edx, mode: stack 0) ret 4]; "docs/graphics/native-light-gather.md";
    /// `0x0004a1e0`: one light's constants (`engine::light_constants`).
    /// EAX the frame block, ECX the volume, EDX the light datum, stack
    /// `(definition, unused, shadow, weight)`, `ret 0x10`.
    /// `RENDER_LIGHT_PASSES` calls it from `0x000311c3` and
    /// `FIRST_PERSON_LIGHTS` from `0x0004057e`. The typed light passes put
    /// `before` hooks on it, each acting only when the return word is its
    /// own caller's site, so several features share the point.
    LIGHT_CONSTANTS @ 0x0004_a1e0: [fn(frame: eax, context: ecx, light: edx, definition: stack 0, unused: stack 1, shadow: stack 2, weight: stack 3) ret 16]; "docs/graphics/native-light-gather.md";
    /// `render_screen_effects`: the view's post passes, the last call of
    /// `render_scene_passes` (`0x0002c322`, after layer 17 and
    /// `0x00020770`): the colour pass over the view's target (`0x000214f0`,
    /// when `colour` is set), the scenario's screen effects (`0x0002d140`
    /// per entry) and `0x0002df00` (when `late` is set). Stack `(late byte,
    /// colour byte)`, `ret 8`.
    RENDER_SCREEN_EFFECTS @ 0x0001_3870: [fn(late: stack 0, colour: stack 1) ret 8]; "docs/graphics/native-renderer.md";
    /// `render_screen_effects`' colour pass, a glow (`0x000214f0`): the
    /// view's target down-sampled into target 7, blurred between 7 and 8
    /// (`0x000211a0`) and blended back. Stack: the seven words
    /// `0x000138b3`..`0x000139b5` push (`engine::colour_pass::Arguments`),
    /// `ret 0x1c`.
    COLOUR_PASS @ 0x0002_14f0: [fn(a0: stack 0, a1: stack 1, a2: stack 2, a3: stack 3, a4: stack 4, a5: stack 5, a6: stack 6) ret 28]; "docs/graphics/native-renderer.md";
    /// One of the scenario's screen effects (`0x0002d140`, from the loop of
    /// `render_screen_effects`): EAX the `egor` datum, stack `(&values,
    /// columns, rows, &corners)` (`engine::screen_effect::plan`), `ret
    /// 0x10`.
    SCREEN_EFFECT @ 0x0002_d140: [fn(datum: eax, values: stack 0, columns: stack 1, rows: stack 2, corners: stack 3) ret 16]; "docs/graphics/native-renderer.md";
    /// Screen convolution blur, also called by the colour pass. From
    /// 0x0002da96, stack arguments 0 and 1 are the steps and tap spread.
    SCREEN_CONVOLUTION_BLUR @ 0x0002_11a0: [fn(source: eax, steps: stack 0, spread: stack 1, growth: stack 2, dest: stack 3, other: stack 4, last: stack 5, combine: stack 6, arg7: stack 7, arg8: stack 8, arg9: stack 9) -> eax ret 40]; "docs/graphics/native-renderer.md";
    /// The blur under a dialog: XMM0 the screen's transition fraction,
    /// times `[0x0045de14]` as the steps of one call of
    /// [`SCREEN_CONVOLUTION_BLUR`] (returning to `0x00235794`), then a
    /// bare RET. Called from the window stack's render at `0x00235128`.
    UI_MODAL_BLUR @ 0x0023_5756: [fn(fraction: xmm0) ret 0]; "docs/graphics/native-post-hud.md";
    /// A unit's integrated-light update, called at the end of its tick.
    UNIT_LIGHT_UPDATE @ 0x000c_50a0: [fn(unit: eax) ret 0]; "docs/halo1/hud.md";
    /// A shader template's pass set up (`0x00016610`, called by
    /// `0x0002d140` at `0x0002d4b7` per pass): stack `(shader, layer,
    /// implementation, selector, target, 0)`, AL whether the pass draws
    /// (`engine::screen_effect::template_draws`; `0x0002d4be` skips the
    /// pass when it does not), `ret 0x18`. Post trace and native gels hook it.
    TEMPLATE_PASS @ 0x0001_6610: [fn(shader: stack 0, layer: stack 1, implementation: stack 2, selector: stack 3, target: stack 4, flags: stack 5) -> al ret 24], Optional; "docs/graphics/native-renderer.md";
    /// Full-target template quad; its gel caller returns to 0x1675a.
    TEMPLATE_QUAD @ 0x0001_5ec0: [fn(target: stack 0) ret 4], Optional; "docs/graphics/native-gels.md";
    /// A batch of screen quads begins (`0x00035b90`): stdcall `(record)`,
    /// `ret 4`. The record (`native_render::hud::Record`) names the
    /// bitmaps, address modes, filter, blend mode and colours; the begin
    /// sets that state and the combiner. Its callers, the string draw
    /// (`0x0001f070`, one quad per glyph) and the movie's draw
    /// (`0x00156960`), are Rust's, so the batch is `render_bridge`'s
    /// named panic.
    SCREEN_QUADS_BEGIN @ 0x0003_5b90: [fn(record: stack 0) ret 4]; "docs/graphics/native-renderer.md";
    /// One screen quad (`0x000363a0`): stdcall `(&corners)`, four corners
    /// of `0x14` bytes (x, y, u, v, colour), pushed as one inline
    /// `Begin(7)`/`End` with no D3D entry. `ret 4`.
    SCREEN_QUAD @ 0x0003_63a0: [fn(corners: stack 0) ret 4], Hot; "docs/graphics/native-renderer.md";
    /// The batch's end (`0x00036560`): no arguments, `ret`. With
    /// `[0x004c1a18]` set it calls [`SCREEN_STATE_RESET`] with 1.
    SCREEN_QUADS_END @ 0x0003_6560: [fn() ret 0]; "docs/graphics/native-renderer.md";
    /// The device state's reset after a screen-quad batch (`0x0001e930`):
    /// clears the combiner block at `0x00484f68` and sets the render
    /// states back. Stdcall `(flag)`, `ret 4`. Its only callers are
    /// [`SCREEN_QUADS_END`] and the movie's draw `0x00156960`.
    SCREEN_STATE_RESET @ 0x0001_e930: [fn(flag: stack 0) ret 4]; "docs/graphics/native-ui-producers.md";
    /// One HUD shader quad (`0x0022a664`): ECX the four corners, EAX the
    /// coordinate rectangle, stack `(bitmap datum, bitmap index, shader)`,
    /// `ret 0xc`. A shader draw (`0x0001bbf0`) and one
    /// `D3DDevice_Begin(7)`..`End`.
    HUD_SHADER_QUAD @ 0x0022_a664: [fn(corners: ecx, rect: eax, bitmap: stack 0, index: stack 1, shader: stack 2) ret 12], Hot; "docs/graphics/native-renderer.md";
    /// Draws the current layer (`[0x00467130]`) of the current list: layer
    /// setup, then `LAYER_ENTRIES` for the layers in its mask.
    LAYER_DRAW @ 0x0004_4550: [opaque]; "docs/graphics/layer-draw.md";
    /// Fill the ordered decorator grid cell table from the BSP and the game-view camera.
    DECORATOR_GRID @ 0x0003_f830: [fn(grid: stack 0, camera: stack 1) ret 8]; "docs/graphics/native-decorators.md";
    /// The layer's end: every `LAYER_DRAW` call in `RENDER_LIGHT_PASSES`
    /// is followed by one; it ends the layer's state and sets the current
    /// layer (`[0x00467130]`) to 0.
    LAYER_END @ 0x0004_45d0: [opaque]; "docs/graphics/native-renderer.md";
    /// Resolve one material for the record-to-scene-item walk. The main
    /// caller tests AL before advancing its part cursor in EDI.
    WORLD_PART_MATERIAL @ 0x0004_5ce0: [fn(record: ecx, scratch: eax, part: stack 0, region: stack 1) -> al ret 8], Hot; "docs/graphics/native-renderer.md";
    /// The standard geometry item's build callback. AL admits its sort slot.
    WORLD_GEOMETRY_BUILD @ 0x0004_cbf0: [fn(shader: stack 0, arg: stack 1, lod: stack 2, layer: stack 3, selector: stack 4, part: stack 5, entry: stack 6) -> al ret 28], Hot; "docs/graphics/layer-draw.md";
    /// BSP geometry emitter. AL selects masked subparts, CL selects strips.
    WORLD_PART_EMIT @ 0x0004_daa0: [fn(section: stack 0, part: stack 1, bitmap: stack 2) ret 12], Hot; "docs/graphics/native-renderer.md";
    /// LAYER_ENTRIES calls the item's geometry handler with seven stack words.
    /// Its continuation reloads EAX from the loop counter before using it.
    LIGHT_GEOMETRY_DRAW @ 0x0004_d0b0: [fn(shader: stack 0, arg: stack 1, variant: stack 2, layer: stack 3, selector: stack 4, part: stack 5, entry: stack 6) ret 28], Hot; "docs/graphics/layer-draw.md";
    /// The layer draw's setup call (ESI = `0x00485b48`).
    LAYER_SETUP @ 0x0001_6b10: [opaque]; "docs/graphics/layer-draw.md";
    /// Draws a list's entries for a layer: builds them, then calls each
    /// entry type's handler. EDI the list, EBX the layer, stack 0 an
    /// argument (`[0x00467134]`), `ret 4`.
    LAYER_ENTRIES @ 0x0003_bc30: [fn(list: edi, layer: ebx, arg: stack 0) ret 4]; "docs/graphics/layer-draw.md";
    /// Builds a layer's entries (0x18 bytes each) into the caller's buffer;
    /// the count in EAX. Stdcall `(list, arg, layer, buffer)`.
    LAYER_BUILD @ 0x0003_bab0: [fn(list: stack 0, arg: stack 1, layer: stack 2, buffer: stack 3) -> eax ret 16]; "docs/graphics/layer-draw.md";
    /// The engine's qsort: ECX the count, EAX the base, stack `(size,
    /// compare, context)`, `ret 0xc`. The layer build sorts its entries
    /// with it (comparators `0x0003bb80`, `0x0003bbd0` for layer 5).
    LAYER_SORT @ 0x0013_da70: [opaque]; "docs/graphics/layer-draw.md";
    /// The engine's index sort (u16 elements): EAX the count, ECX the base,
    /// stack `(unused, compare, context)`, `ret 0xc`. The transparents'
    /// draw order sorts with it (comparator `0x0001e700`).
    TRANSPARENT_SORT @ 0x0013_df80: [opaque]; "docs/graphics/render-loop.md";
    /// Build a potentially visible set's region records for a single frustum.
    VIEW_PVS_REGIONS @ 0x0017_33e0: [fn(start: eax, output: ebx, camera: stack 0) ret 4]; "docs/graphics/native-visibility.md";
    /// Materialize one merged tangent-space portal window.
    VIEW_WINDOW_BUILD @ 0x0016_3db0: [fn(rect: ecx, output: edi, frustum: stack 0, ordinal: stack 1) -> al ret 8]; "docs/graphics/native-visibility.md";
    /// The shader draw's render-state list: EBX the draw context, stack 0
    /// the list word's address (`count << 9 | first`), `ret 4`. Applies
    /// each entry through `SetRenderState_Simple`, the D3D shadow and
    /// dirty bits, or a per-state setter.
    RENDER_STATES @ 0x0001_a170: [fn(context: ebx, list: stack 0) ret 4]; "docs/graphics/render-loop.md";
    /// Flood a single frustum into the caller's portal-node and cluster scratch.
    PORTAL_WINDOW_WALK @ 0x0017_2840: [fn(view: stack 0, nodes: stack 1, clusters: stack 2, queue: stack 3, projection: stack 4) -> al ret 20]; "docs/graphics/native-visibility.md";
    /// Merge portal windows by cluster in the current flood's node pool.
    PORTAL_WINDOW_MERGE @ 0x0017_2c70: [fn(chains: esi, pool: stack 0) ret 4]; "docs/graphics/native-visibility.md";
    /// Materialize regions and frusta from the merged node pool. The
    /// single caller passes its lookup, camera and output tables, with a
    /// zero word it leaves unused.
    PORTAL_WINDOW_REGIONS @ 0x0017_30a0: [fn(lookup: stack 0, camera: stack 1, unused: stack 2, output: stack 3) ret 16]; "docs/graphics/native-visibility.md";
    /// Publish sphere-mode region records and update visited-cluster stamps.
    VIEW_SPHERE_REGIONS @ 0x0013_2d60: [fn(vis: eax) ret 0]; "docs/graphics/native-visibility.md";
    /// The gather's setup: the view's fields of the visibility struct,
    /// its frusta, start cluster and mode. Its one caller is the gather
    /// `0x001320f0` (`0x00132129`).
    VISIBILITY_SETUP @ 0x0013_2b80: [fn(flags: ecx, count: ebx, vis: stack 0, kind: stack 1, frusta: stack 2, start: stack 3, source: stack 4, centre: stack 5, radius: stack 6, clip: stack 7, clip_offset: stack 8) -> eax ret 36]; "docs/graphics/native-visibility.md";
    /// The frusta mode's producer: per frustum the walk, the merge and the
    /// region writer, then the ordinal sort. Called by the gather at
    /// `0x0013216c`.
    VIEW_FRUSTA_REGIONS @ 0x0017_3130: [fn(count: stack 0, view: stack 1, start: stack 2, output: stack 3) ret 16]; "docs/graphics/native-visibility.md";
    /// Build a view's projection terms from its render camera: ECX the
    /// camera, EDX the tangent extents or null, EAX the 0x40-byte output.
    /// Its one caller is `0x0002fd90`.
    VIEW_PROJECTION_BUILD @ 0x0002_fb70: [fn(camera: ecx, extents: edx, output: eax) ret 0]; "docs/graphics/native-visibility.md";
    /// Populate visibility lists from the current view's cluster references.
    SCENE_CLUSTER_GATHER @ 0x0013_2220: [fn(vis: stack 0) ret 4]; "docs/graphics/native-renderer.md";
    /// Publish clusters and their instanced geometry in visibility order.
    SCENE_GEOMETRY_GATHER @ 0x0013_4390: [fn(vis: stack 0) ret 4]; "docs/graphics/native-renderer.md";
    /// Move objects with the model-link property to the front of the list.
    SCENE_OBJECT_ORDER @ 0x0013_4240: [fn(vis: stack 0) ret 4]; "docs/graphics/native-renderer.md";
    /// Collect, rank and fade the current view's dynamic lights.
    VISIBLE_LIGHT_LIST @ 0x0003_1910: [fn(output: stack 0) -> eax ret 4]; "docs/graphics/native-renderer.md";
    /// Mark caster and receiver flags on a light's visibility lists.
    LIGHT_GEOMETRY_FLAGS @ 0x0003_1660: [fn(light: eax, definition: ecx, vis: esi, owner: stack 0, root: stack 1, point: stack 2, shadow: stack 3) ret 16]; "docs/graphics/native-renderer.md";
    /// Append the optional owner or owning unit as a dynamic light caster.
    LIGHT_EXTRA_CASTER @ 0x0003_1c20: [fn(shadow: al, owner: ecx, vis: stack 0) ret 4]; "docs/graphics/native-renderer.md";
    /// Select a model detail tier. The scaled camera distance is in the low
    /// float of XMM4. The return value and fraction drive the per-view hysteresis.
    MODEL_LOD @ 0x0004_c450: [fn(distances: ecx, fraction: ebx, object: stack 0) -> al ret 4]; "docs/graphics/occlusion-culling.md";
    /// Write one BSP or model render record, with its associated cache state.
    SCENE_RECORD_WRITER @ 0x0002_cbf0: [fn(section: eax, selector: ecx, node_count: edx, pixels: stack 9) -> eax ret 84]; "docs/graphics/native-renderer.md";
    /// The object render records for a visibility struct: EAX the struct
    /// (`0x00547f88` the main view's), stack 0 an argument passed on,
    /// `ret 4`. Reorders the struct's object list (`0x00134240`), then
    /// calls `OBJECT_RENDER_SETUP` per listed object.
    OBJECT_RENDER_RECORDS @ 0x0013_3720: [fn(vis: eax, arg: stack 0) ret 4]; "docs/graphics/occlusion-culling.md";
    /// One listed object's render setup (view depth, LOD, nodes, children,
    /// the `0x004ba138` records): ESI the visibility struct, stack
    /// `(list index, byte, byte*, -1, arg)`, AL a retry flag, `ret 0x14`.
    OBJECT_RENDER_SETUP @ 0x0013_3790: [fn(vis: esi, index: stack 0) -> al ret 20]; "docs/graphics/occlusion-culling.md";
    /// The sorted transparents of a scene pass (`render_scene_passes`
    /// calls it after the opaque layers and before the first-person
    /// chain).
    SCENE_TRANSPARENTS @ 0x0001_e370: [opaque]; "docs/graphics/occlusion-culling.md";
    /// Queue an effect callback with inline payload and view depth.
    TRANSPARENT_EFFECT_ENTRY @ 0x0001_e4e0: [fn(size: ecx, position: edx, handler: stack 0, payload: stack 1) -> al ret 8]; "docs/graphics/native-renderer.md";
    /// Queue the polygon effect callback with seven extra words.
    TRANSPARENT_POLYGON_ENTRY @ 0x0001_e5e0: [fn(size: ecx, position: edx) -> al ret 32]; "docs/graphics/native-renderer.md";
    /// A sorted transparent geometry entry's handler: the transparents call
    /// it through the entry's `+0x20` with the entry's `+0x24` (shader
    /// datum, then the render record argument), stdcall, `ret 4`. Runs the
    /// opaque path's build and draw over the template's layer-15 passes.
    TRANSPARENT_GEOMETRY @ 0x0004_1020: [fn(data: stack 0) ret 4]; "docs/graphics/native-renderer.md";
    /// Shader draw begin: per draw.
    SHADER_DRAW_BEGIN @ 0x0001_bbf0: [fn(shader: edi, mode: stack 0, pass: stack 1, flags: stack 2, index: stack 3, scale: stack 4) ret 20], Hot; "docs/formats/shader-instances.md";
    /// `submit_structure_or_model_geometry`: per draw.
    GEOMETRY_SUBMIT @ 0x0004_1040: [opaque], Hot; "docs/formats/structure-bsp.md";
    /// A shader parameter's value for this draw: its sources through
    /// `[0x00485a58]`, the CRT `fmod`, then `FUNCTION_EVALUATE`.
    SHADER_PARAMETER @ 0x0001_b230: [opaque], Hot; "docs/graphics/layer-draw.md";
    /// A texture binding's source by type (a jump table).
    TEXTURE_SOURCE @ 0x0001_9530: [opaque], Hot; "docs/graphics/layer-draw.md";
    /// An animated bitmap's frame for a texture binding.
    BITMAP_FRAME @ 0x0001_43c0: [opaque], Hot; "docs/graphics/layer-draw.md";
    /// A tag function's value (two inputs on the stack).
    FUNCTION_EVALUATE @ 0x0013_b390: [opaque], Hot; "docs/graphics/layer-draw.md";
    /// The CRT's x87 `fmod` (`_CIfmod`: ST1 mod ST0), through the CRT's
    /// transcendental dispatcher.
    CRT_FMOD @ 0x0037_3b6a: [opaque], Hot; "docs/graphics/layer-draw.md";
    /// The top-level Settings list's row text (thiscall on the list, the
    /// item and a word).
    SETTINGS_ROW_TEXT @ 0x002b_1d6e: [fn(list: ecx, item: stack 0, word: stack 1) ret 8]; "docs/game/settings-menu.md";
    /// The top-level Settings list's row chosen (thiscall on the list, the
    /// player chain and a pointer to the row).
    SETTINGS_ROW_CHOSEN @ 0x002b_1da9: [fn(list: ecx, chain: stack 0, row: stack 1) ret 8]; "docs/game/settings-menu.md";
    /// The campaign pause menu's row text (screen 18, `dialog_pause`):
    /// switches on the item's row, 0 to 4; any other row gets the null id.
    PAUSE_ROW_TEXT @ 0x0023_1f6c: [fn(list: ecx, item: stack 0, word: stack 1) ret 8]; "docs/game/settings-menu.md";
    /// The campaign pause menu's row chosen: rows 0 to 3 have their own
    /// press, and every other row runs row 4's.
    PAUSE_ROW_CHOSEN @ 0x0023_1fe3: [fn(list: ecx, chain: stack 0, row: stack 1) ret 8]; "docs/game/settings-menu.md";
    /// The in-game Controller Settings loader (screen 200).
    PAUSE_CONTROLS_LOADER @ 0x0023_1db5: [fn(request: stack 0) -> eax ret 4]; "docs/game/settings-menu.md";
    /// The multiplayer pause menu's list handler constructor (screen 195,
    /// `mp_menu`): ESI the handler, one stack argument. Makes the rows'
    /// datum array (room for six) and adds a datum per row it shows, the
    /// row's action word at datum `+2`.
    MP_MENU_LIST_CTOR @ 0x0023_2708: [fn(list: esi, a: stack 0) ret 4]; "docs/game/settings-menu.md";
    /// The multiplayer pause menu's row text: looks the row's action up in
    /// a table of six.
    MP_MENU_ROW_TEXT @ 0x0023_284e: [fn(list: ecx, item: stack 0, word: stack 1) ret 8]; "docs/game/settings-menu.md";
    /// The multiplayer pause menu's row chosen: actions 0 to 4 have their
    /// own press, and every other action runs action 5's.
    MP_MENU_ROW_CHOSEN @ 0x0023_28b5: [fn(list: ecx, chain: stack 0, row: stack 1) ret 8]; "docs/game/settings-menu.md";
    /// The Controls list's row text: the label and value of rows 0 to 5.
    CONTROLS_ROW_TEXT @ 0x002c_8548: [fn(list: ecx, item: stack 0, word: stack 1) ret 8]; "docs/game/settings-menu.md";
    /// The Controls list's row chosen (rows past 4 open the vibration
    /// dialog).
    CONTROLS_ROW_CHOSEN @ 0x002c_8719: [fn(list: ecx, chain: stack 0, row: stack 1) ret 8]; "docs/game/settings-menu.md";
    /// The campaign difficulty list's row text.
    DIFFICULTY_ROW_TEXT @ 0x002c_9ce7: [fn(list: ecx, item: stack 0, word: stack 1) ret 8]; "docs/game/skulls.md";
    /// The campaign difficulty list's row chosen: clamps the row to 0..3,
    /// stores the difficulty and starts the campaign.
    DIFFICULTY_ROW_CHOSEN @ 0x002c_9d38: [fn(list: ecx, chain: stack 0, row: stack 1) ret 8]; "docs/game/skulls.md";
    /// The variant editor's category list (screen 178,
    /// `variant_editing_options`) row text: looks the row's word up in a
    /// table of six; an unknown word leaves the text as it was.
    VARIANT_OPTIONS_ROW_TEXT @ 0x002b_7581: [fn(list: ecx, item: stack 0, word: stack 1) ret 8]; "docs/game/skulls.md";
    /// The variant editor's category list row chosen: words 0 to 5 open a
    /// category's screen; any other word returns having done nothing.
    VARIANT_OPTIONS_ROW_CHOSEN @ 0x002b_75e8: [fn(list: ecx, chain: stack 0, row: stack 1) ret 8]; "docs/game/skulls.md";
    /// The game type list's screen constructor (screens 207 and 60, "SELECT
    /// GAME"; vtable `0x45aff0`): EDI the screen, then the request's
    /// window, slot and player, the screen id and three mode flags it
    /// keeps at `+0xb38..+0xb3a` (settings, create, pick). Builds the list
    /// at `+0x614` (`0x2c55f8`): a datum array "game engine variant
    /// category list" of nine, one datum per game type, its word at `+2`
    /// (0 Slayer, 1 King of the Hill, 3 Oddball, 4 Juggernaut, 6 CTF, 7
    /// Assault, 8 Territories).
    VARIANT_CATEGORY_SCREEN_CTOR @ 0x002b_1ab3: [fn(screen: edi, window: stack 0, slot: stack 1, player: stack 2, id: stack 3, settings: stack 4, create: stack 5, pick: stack 6) -> eax ret 28]; "docs/game/community-gametypes.md";
    /// The game type list's row text: looks the row's word up in a table
    /// of seven; an unknown word leaves the text as it was.
    VARIANT_CATEGORY_ROW_TEXT @ 0x002c_5823: [fn(list: ecx, item: stack 0, word: stack 1) ret 8]; "docs/game/community-gametypes.md";
    /// The game type list's row chosen: by the row's word, opens that game
    /// type's variant list (the create flag: a new variant of it); any
    /// word past the table runs Territories'.
    VARIANT_CATEGORY_ROW_CHOSEN @ 0x002c_5897: [fn(list: ecx, chain: stack 0, row: stack 1) ret 8]; "docs/game/community-gametypes.md";
    /// The variant list's screen constructor (screens 208 and 15, vtable
    /// `0x45d6f8`, `0x4fe0` bytes): EDI the screen, then the screen id and
    /// the request's window, slot and player. Its 21 loaders (seven game
    /// types, three modes) then store the content category at `+0x4fd8`.
    VARIANT_LIST_SCREEN_CTOR @ 0x002c_9ddb: [fn(screen: edi, id: stack 0, window: stack 1, slot: stack 2, player: stack 3) -> eax ret 16]; "docs/game/community-gametypes.md";
    /// The variant list's per-frame update (screen vtable slot 3): sets
    /// its title (text 5) and description (text 2) and the picture
    /// (bitmap 4) by the category at `+0x4fd8`, then the base update
    /// (`0x22e391`, which updates the children).
    VARIANT_LIST_UPDATE @ 0x002c_ad45: [fn(screen: ecx) ret 0]; "docs/game/community-gametypes.md";
    /// The variant list's own loader (screen vtable slot 26), by the
    /// category at `+0x4fd8` and the mode bytes at `+0x4fdc`, `+0x4fdd`:
    /// what a screen stack reopens it with.
    VARIANT_LIST_LOADER @ 0x002c_ac1a: [fn(screen: ecx) -> eax ret 0]; "docs/game/community-gametypes.md";
    /// The variants of a content category for a player: up to `*count`
    /// (a `u16`, written back) content handles into `handles`, saved ones
    /// and, with `builtins`, the built-in ones. The variant list's fill
    /// (`0x2ca0d9`, the list handler in EBX) calls it from `0x2ca10e`.
    VARIANTS_ENUMERATE @ 0x0021_5900: [fn(player: stack 0, category: stack 1, count: stack 2, handles: stack 3, builtins: stack 4) ret 20]; "docs/game/community-gametypes.md";
    /// The Game Setup dialog's list handler constructor (screen 23,
    /// `squad_settings_dialog`): ESI the handler, one stack argument. Fills
    /// the "squad setting list" rows (action word at datum `+2`) for the
    /// lobby's kind.
    SQUAD_SETTINGS_LIST_CTOR @ 0x002b_b4d2: [fn(list: esi, a: stack 0) ret 4]; "docs/halo1/multiplayer.md";
    /// The Game Setup list's row text: looks the row's action up in a table
    /// of nine; an unknown action leaves the text as it was.
    SQUAD_SETTINGS_ROW_TEXT @ 0x002b_b74f: [fn(list: ecx, item: stack 0, word: stack 1) ret 8]; "docs/halo1/multiplayer.md";
    /// The Game Setup list's row chosen: jumps through a ten-entry table by
    /// the row's action with no bounds check.
    SQUAD_SETTINGS_ROW_CHOSEN @ 0x002b_b801: [fn(list: ecx, chain: stack 0, row: stack 1) ret 8]; "docs/halo1/multiplayer.md";
    /// The Game Setup dialog's update (vtable `0x45c524`): sets the
    /// description, the "current" label and value, and the picture for
    /// the focused row's action (a jump table of ten at `0x2bbea8`; any
    /// other action gets none of them and image 0), then the base update.
    SQUAD_SETTINGS_UPDATE @ 0x002b_bbf6: [fn(screen: ecx) ret 0]; "docs/game/ui-builder.md";
    /// Whether the OptiMatch playlist list shows hopper `index` (0..16):
    /// the range, two gates and the descriptor's validity (tail `jmp` to
    /// `0x001934f0`). Its one caller is the list's item builder
    /// `0x002b1ff4` (return address `0x002b2069`), which stores the index
    /// in each row's datum at `+4`, so a skipped hopper leaves no hole.
    HOPPER_LISTED @ 0x0019_2db0: [fn(index: ecx) -> al ret 0]; "docs/game/bots.md";
    /// Reads one hopper rotation into the caller's block. The weighted map
    /// picker calls it separately from the validator and playlist UI.
    HOPPER_BLOCK_READ @ 0x0019_43d0: [fn(index: eax, block: stack 0) -> al ret 4]; "docs/live/live-matchmaking.md";
    /// The hopper Quickmatch searches: local player `player`'s saved
    /// playlist (`[0x54e9dc + player*0xc70]`), or -1 when the player has
    /// none (flag bit 4 of `[0x54e8e0 + player*0xc70]` clear). Its one
    /// caller is the Live menu's Quickmatch press (`0x00231056`), which
    /// starts nothing on -1.
    QUICKMATCH_HOPPER @ 0x0019_02de: [fn(player: ecx) -> eax ret 0]; "docs/game/bots.md";
    /// Saves `hopper` as local player `player`'s playlist for Quickmatch.
    /// Its one caller is the OptiMatch playlist list's press
    /// (`0x002b21e3`).
    PLAYLIST_REMEMBER @ 0x0019_02fc: [fn(player: stack 0, hopper: stack 1) ret 8]; "docs/game/bots.md";
    /// The Xbox Live menu's "build" method (vtable `0x00458b48`, slot
    /// `0x00458bec`): resets the search globals, fills the list, then puts
    /// the cursor on the row in `[0x00510a18]` when it is 0 to 3 (`0x0024c591`,
    /// the list at screen `+0x818`).
    LIVE_MENU_CURSOR @ 0x0023_1170: [fn(screen: ecx) ret 0]; "docs/game/bots.md";
    /// The multiplayer level list's data array (`[0x4ee4e4]`, 8-byte
    /// elements, the map id at `+4`), or 0 without UI level data. The
    /// Select Map list's constructor (`0x2c9928`, list handler in ESI)
    /// calls it at `0x2c9980` and copies every element into its own list.
    MP_LEVEL_ARRAY @ 0x0019_c6a0: [fn() -> eax ret 0]; "docs/game/map-select.md";
    /// Refreshes the UI level lists (no direct caller: the function table at
    /// `0x43ca94`, at each map load): the arrays cleared, the content
    /// packages' `patch.lvl` (`0x19c120`, main menu only), the level select's
    /// list (`0x19c6d0`), then the multiplayer array (`0x19c7e0`).
    MP_LEVEL_REFRESH @ 0x0019_c190: [fn() ret 0]; "docs/game/map-select.md";
    /// A multiplayer list slot's name in the current language (stdcall:
    /// `slot + 0xc + 0x40 * language`). The Select Map row text calls it at
    /// `0x2c9afc`.
    MP_LEVEL_NAME @ 0x002a_fd70: [fn(slot: stack 0) -> eax ret 4]; "docs/game/map-select.md";
    /// The current language's index (cached at `0x47ff38`). The Select Map's
    /// focus handler (`0x2c9b7c`, its screen in EDI and the focused row's
    /// slot in EBP) calls it at `0x2c9be6` and shows
    /// `slot + 0x24c + 0x100 * language`, the slot's description.
    UI_LANGUAGE @ 0x0011_cae0: [fn() -> eax ret 0]; "docs/game/map-select.md";
    /// The Select Map list's row text (list handler in ECX): the row's map
    /// id, its slot, then [`MP_LEVEL_NAME`] for the text.
    MAP_LIST_ROW_TEXT @ 0x002c_9ac4: [fn(list: ecx, item: stack 0, word: stack 1) ret 8]; "docs/game/map-select.md";
    /// The Select Map list's row chosen (list handler in ECX, at its
    /// screen's `+0x614`): the map, then message 3 to its own screen.
    MAP_LIST_ROW_CHOSEN @ 0x002c_9b17: [fn(list: ecx, chain: stack 0, row: stack 1) ret 8]; "docs/game/map-select.md";
    /// Open the lobby's Select Map dialog from a screen request.
    SELECT_MAP_LOAD @ 0x002b_b2db: [fn(request: stack 0) -> eax ret 4]; "docs/game/map-select.md";
    /// Open the lobby's Game Setup dialog from a screen request.
    GAME_SETUP_LOAD @ 0x002b_bacb: [fn(request: stack 0) -> eax ret 4]; "docs/game/ui-screens.md";
    LOBBY_VARIANT_LIST_LOAD @ 0x002b_1b85: [fn(request: stack 0) -> eax ret 4]; "docs/game/ui-screens.md";
    LOBBY_LEVEL_LIST_LOAD @ 0x002b_136d: [fn(request: stack 0) -> eax ret 4]; "docs/game/ui-screens.md";
    LOBBY_DIFFICULTY_LOAD @ 0x002b_b432: [fn(request: stack 0) -> eax ret 4]; "docs/game/ui-screens.md";
    /// The Select Map screen's destructor (screen vtable `0x45c650` slot 0).
    MAP_SCREEN_FREE @ 0x002b_b384: [fn(screen: ecx, flags: stack 0) -> eax ret 4]; "docs/game/map-select.md";
    /// Whether the HUD draws (AL); 0 in a campaign with Blind on.
    HUD_SHOWN @ 0x0013_939b: [fn() -> al ret 0]; "docs/game/skulls.md";
    /// The game engine's black overlay over the view, from the HUD draw
    /// (`0x001396a0`): the view's local user's spawn fade
    /// (`[GAME_ENGINE] + 0xe4 + 4 * user`); a view with no local user
    /// (`0x004b9ed8` none) draws it at 1.0 (`0x0045dbc0`), opaque.
    HUD_BLACK_OVERLAY @ 0x0015_9880: [fn() ret 0]; "docs/game/forge-design.md";
    HUD_TEXT_SETUP @ 0x0013_edb0: [fn(font: esi, colour: edi, shadow: stack 3) ret 16], Optional; "docs/halo1/text.md";
    HUD_TEXT_MEASURE @ 0x0013_ee20: [fn(text: ecx, font: stack 0) -> al ret 4], Optional; "docs/halo1/text.md";
    HUD_TEXT_BOUNDS @ 0x0024_ce9d: [fn(text: stack 0) ret 4], Optional; "docs/halo1/text.md";
    HUD_MESSAGE_LOOKUP @ 0x0013_925f: [fn(key: edi, output: ecx) ret 0], Optional; "docs/halo1/text.md";
    HUD_GLYPH_FIND @ 0x0014_0b20: [fn(font: eax, unit: ecx) -> eax ret 0], Optional; "docs/halo1/text.md";
    HUD_GLYPH_CREATE @ 0x0014_0b80: [fn(font: stack 0, unit: stack 1) -> eax ret 8], Optional; "docs/halo1/text.md";
    HUD_GLYPH_LOAD @ 0x0014_1020: [fn(glyph: eax, flags: edx) -> al ret 0], Optional; "docs/halo1/text.md";
    HUD_GLYPH_PIXELS_READY @ 0x0014_0870: [fn(glyph: eax, flags: stack 0) -> al ret 4], Optional; "docs/halo1/text.md";
    HUD_GLYPH_PIXELS @ 0x0014_0960: [fn(glyph: eax, output: stack 0) -> al ret 4], Optional; "docs/halo1/text.md";
    HUD_INTERACTION_VISIBLE @ 0x0024_c98c: [fn(user: eax, visible: stack 0) ret 4], Optional; "docs/halo1/text.md";
    PLAYER_PICKUP_SCAN @ 0x0015_08e0: [fn(player: stack 0) ret 4], Optional; "docs/halo1/text.md";
    PLAYER_PICKUP_CANDIDATE @ 0x0015_0e30: [fn(player: stack 0, target: stack 1) ret 8], Optional; "docs/halo1/text.md";
    PLAYER_PRESS_INTERACT @ 0x0015_1c90: [fn(player: eax, action: stack 0) -> al ret 4], Optional; "docs/halo1/text.md";
    HUD_MATERIAL_CONSTANTS @ 0x0001_8ee0: [fn(material: stack 0) ret 4], Optional; "docs/halo1/text.md";
    HUD_MATERIAL_PIXEL_SHADER @ 0x0001_b050: [fn(material: esi) ret 0], Optional; "docs/halo1/text.md";
    HUD_MATERIAL_READY @ 0x0001_cd90: [fn() ret 0], Optional; "docs/halo1/text.md";
    HUD_WIDGET_INPUTS @ 0x0013_ad48: [fn(weapon: ecx, hud: edx, user: stack 0, inputs: stack 1, output: stack 2) ret 12], Optional; "docs/halo1/hud.md";
    HUD_WIDGET_STRING @ 0x0022_a871: [fn(widget: ebx, hud: stack 0, values: stack 1, output: stack 2, font: stack 3) ret 16], Optional; "docs/halo1/hud.md";
    HUD_WIDGET_EFFECTS @ 0x0022_bcaf: [fn(effects: eax, y_basis: ebx, origin: stack 0, x_basis: stack 1) ret 8], Optional; "docs/halo1/hud.md";
    HUD_WIDGET_SPRITE @ 0x0022_ae21: [fn(widget: ecx, bounds: eax, user: stack 0, image: stack 1, width: stack 2, height: stack 3) ret 16], Optional; "docs/halo1/hud.md";
    HUD_WIDGET_GATE @ 0x0013_ac87: [fn(state: eax, weapon: ecx, word_c: edx, word_a: stack 0, word_b: stack 1, word_d: stack 2) -> al ret 12], Optional; "docs/editor/hud-editor.md";
    /// The HUD's walks of the definitions its record names (`nhdt`): the
    /// record's words 5, 2, 3, 4, 5, 6, 0 and 1, each through `HUD_WALK`
    /// with its slot. It writes the record to `[0x00510c4c] + 0x1bc` first.
    HUD_DEFINITIONS @ 0x0013_94b9: [fn(record: esi, user: edi) ret 0], Optional; "docs/graphics/native-hud-overlay.md";
    /// The HUD walk of one definition (`nhdt`): its state words
    /// (`0x0013a050`), then each shown screen-effect, bitmap and text
    /// widget's draw, in the definition's order.
    HUD_WALK @ 0x0013_a532: [fn(definition: eax, player: stack 0, record: stack 1, slot: stack 2) ret 12], Optional; "docs/graphics/native-hud-overlay.md";
    /// A screen-effect widget's draw: one call of `SCREEN_EFFECT`.
    HUD_SCREEN_WIDGET @ 0x0022_b8e2: [fn(widget: eax, player: stack 0, inputs: stack 1) ret 8], Optional; "docs/graphics/native-hud-overlay.md";
    /// A bitmap widget's draw: its sprite, its basis, then one
    /// `HUD_SHADER_QUAD` a quad.
    HUD_BITMAP_WIDGET @ 0x0022_aff1: [fn(player: stack 0, record: stack 1, widget: stack 2, inputs: stack 3) ret 16], Optional; "docs/graphics/native-hud-overlay.md";
    /// A text widget's draw: its string, laid out and drawn.
    HUD_TEXT_WIDGET @ 0x0022_aa16: [fn(player: stack 0, record: stack 1, widget: stack 2, inputs: stack 3) ret 16], Optional; "docs/graphics/native-hud-overlay.md";
    /// The string draw (`0x0001f070`, from `0x0001fa50` and the HUD's
    /// `0x0001fb10`): EAX a clip rectangle or 0, ECX the rectangle or 0
    /// (the frame), stack `(&end, leading, &units, scale, record +0x58,
    /// record +0x5c, keep state)`, `ret 0x1c`. One screen quad a glyph
    /// through the layout `0x0013f0e0`.
    STRING_DRAW @ 0x0001_f070: [fn(clip: eax, rect: ecx, end: stack 0, leading: stack 1, units: stack 2, scale: stack 3, extra_a: stack 4, extra_b: stack 5, keep: stack 6) ret 28], Optional; "docs/graphics/native-hud-overlay.md";
    HUD_SENSOR_DRAW @ 0x001a_2823: [fn(screen: eax, user: ecx) ret 0], Optional; "docs/halo1/hud.md";
    /// One motion sensor blip's draw into the sensor's target, called by
    /// `0x001a2197` (return `0x001a2219`): its position (two `f32`), shade,
    /// size, colour (three `f32`) and whether it pulses (low byte).
    HUD_SENSOR_BLIP @ 0x0025_4490: [fn(position: stack 0, shade: stack 1, size: stack 2, colour: stack 3, pulse: stack 4) ret 20], Optional; "docs/graphics/native-hud-overlay.md";
    /// The motion sensor's wash, tint and composite fans around its centre
    /// (two `f32`), called by `HUD_SENSOR_DRAW` at `0x001a295b`.
    HUD_SENSOR_COMPOSITE @ 0x0025_48f0: [fn(centre: stack 0, value: stack 1) ret 8], Optional; "docs/graphics/native-hud-overlay.md";
    HUD_TEXT_TAIL @ 0x0015_f120: [fn(player: eax, out: esi, capacity: stack 0, mode: stack 1) -> al ret 8], Optional; "docs/halo1/text.md";
    HUD_TEXT_DRAW @ 0x0001_fa30: [fn(rect: eax, text: ecx) ret 0], Optional; "docs/halo1/text.md";
    HUD_TEXT_SUBSTITUTE @ 0x0022_d2ee: [fn(text: stack 0, capacity: stack 1) ret 8], Optional; "docs/halo1/text.md";
    /// Draws the first-person weapons of one local user (stdcall, four
    /// arguments, EAX the result); skipped whole in a campaign with Blind on.
    FIRST_PERSON_RENDER @ 0x0016_5f16: [fn(user: stack 0, a: stack 1, b: stack 2, c: stack 3) -> eax ret 16]; "docs/game/skulls.md";
    /// A weapon object made from its scenario placement (stdcall: the
    /// object datum, the placement); its rounds are the placement's
    /// `+0x4c`, halved in a campaign with Famine on.
    WEAPON_FROM_PLACEMENT @ 0x000f_da00: [fn(object: stack 0, placement: stack 1) ret 8]; "docs/game/skulls.md";
    /// Open a screen: build a 0x20-byte request for the player mask and
    /// call `loader` with it (stdcall).
    OPEN_SCREEN @ 0x0014_9f1e: [fn(mask: stack 0, loader: stack 1) ret 8]; "docs/formats/ui.md";
    /// Allocate a UI object of `size` bytes (stdcall); 0 when out of memory.
    UI_ALLOC @ 0x001a_47fd: [fn(size: stack 0) -> eax ret 4]; "docs/formats/ui.md";
    /// The Controls screen loader (stdcall, the request).
    CONTROLS_LOADER @ 0x002b_7714: [fn(request: stack 0) -> eax ret 4]; "docs/formats/ui.md";
    /// The Controls screen constructor: the screen in EDI, the request's
    /// `+4`, `+8` and `+2` (word) on the stack; the screen in EAX.
    CONTROLS_SCREEN_CTOR @ 0x002b_7752: [fn(screen: edi, a: stack 0, b: stack 1, player: stack 2) -> eax ret 12]; "docs/formats/ui.md";
    /// Queue a constructed screen on the channel named by the request.
    SCREEN_PUSH @ 0x0014_7f6d: [fn(screen: edi, request: ebx) ret 0]; "docs/formats/ui.md";
    /// The generic screen event handler; B and Back close by channel.
    SCREEN_BACK_EVENT @ 0x0023_00ea: [fn(screen: ecx, event: stack 0) -> al ret 4]; "docs/game/ui-screens.md";
    /// Game Setup handles X and delegates other events to SCREEN_BACK_EVENT.
    GAME_SETUP_EVENT @ 0x002b_bed0: [fn(screen: ecx, event: stack 0) -> al ret 4]; "docs/game/ui-screens.md";
    /// Unlink and free a screen on a UI channel.
    UI_CHANNEL_FREE @ 0x0023_51ea: [fn(channel: ecx, screen: stack 0) ret 4]; "docs/game/ui-screens.md";
    /// Play the UI sound by index (Back is 4).
    UI_SOUND @ 0x0023_6299: [fn(sound: eax) ret 0]; "docs/game/ui-screens.md";
    /// Screen vtable slot 18 of the Controls class: set up the current pane
    /// (header, then the pane's widgets).
    SCREEN_SETUP_PANE @ 0x0023_047e: [fn(screen: ecx, arg: stack 0) ret 4]; "docs/formats/ui.md";
    /// Set a text widget's string id (the widget in EDI).
    TEXT_SET @ 0x0025_3b1a: [fn(text: edi, id: stack 0) ret 4]; "docs/formats/ui.md";
    /// The pane builder's text step: make a text widget from a text block
    /// definition (`0x2c` bytes), a child of the screen in EBX, and give it
    /// the block's string; the widget in EAX, or 0.
    PANE_TEXT_ADD @ 0x0022_f9bf: [fn(screen: ebx, index: stack 0, block: stack 1) -> eax ret 8]; "docs/formats/ui.md";
    /// The pane builder's bitmap step: make a bitmap widget from a bitmap
    /// block definition (`0x38` bytes), a child of the screen in ESI; the
    /// widget in EAX, or 0.
    PANE_BITMAP_ADD @ 0x0022_fa30: [fn(screen: esi, block: stack 0) -> eax ret 4]; "docs/formats/ui.md";
    /// Builds the wide-name history rows for Recent Players and returns the list.
    RECENT_PLAYERS_BUILD @ 0x002b_393c: [fn(list: ebx, request: stack 0) -> eax ret 4]; "docs/game/bots.md";
    /// Retail's lossy wide-to-ASCII conversion, used when packing a Live selection.
    PLAYER_NAME_TO_ASCII @ 0x0013_fb90: [fn(source: eax, dest: ecx, capacity: edx) ret 0]; "docs/game/bots.md";
    /// The two retail player-options constructors share the current selected-player record.
    PLAYER_OPTIONS_CHECK_FRIEND @ 0x002b_70a3: [fn(request: eax, source: esi) -> eax ret 0]; "docs/game/bots.md";
    PLAYER_OPTIONS_OPEN @ 0x002b_71a6: [fn(request: eax, source: esi) -> eax ret 0]; "docs/game/bots.md";
    /// List skin by index in the current interface, or null.
    UI_LIST_SKIN @ 0x0014_837a: [fn(index: stack 0) -> eax ret 4]; "docs/formats/ui.md";
    PANE_SHADER_ADD @ 0x002b_af38: [fn(screen: esi, block: ebx) -> eax ret 0]; "docs/formats/ui.md";
    EMBLEM_WIDGET_DRAW @ 0x002b_11c9: [fn(widget: ecx, context: stack 0) ret 4]; "docs/game/bots.md";
    EMBLEM_WIDGET_SET @ 0x002b_12ca: [fn(widget: eax, team: edx, slot: ecx, profile: stack 0) ret 4]; "docs/game/bots.md";
    /// The current interface's screen tag for a screen id (stdcall): the
    /// `wgit` tag's datum handle, or -1.
    SCREEN_TAG_BY_ID @ 0x0014_8098: [fn(id: stack 0) -> eax ret 4]; "docs/formats/ui.md";
    /// The focused item's row handle of the list in EAX, or -1.
    LIST_FOCUSED_ROW @ 0x0024_c447: [fn(list: eax) -> eax ret 0]; "docs/formats/ui.md";
    /// Transform a UI quad in place from UI coordinates to screen coordinates.
    /// The list's arrow draw at `0x24bda2` calls it with its arrow quad in ESI.
    UI_QUAD_TO_SCREEN @ 0x0023_618e: [fn(quad: esi, scale: stack 0, transform: stack 1) -> eax ret 8]; "docs/formats/ui.md";
    /// An item widget's `index`-th text widget of kind 6 (thiscall on the
    /// item; the game pushes 0, the index, 6), or 0.
    ITEM_TEXT @ 0x0022_edb8: [fn(item: ecx, kind: stack 0, index: stack 1, zero: stack 2) -> eax ret 12]; "docs/audio/playstation-audio.md";
    /// The lifecycle-6 matchmaking update. ECX is its state object.
    MATCHMAKING_UPDATE @ 0x0007_06f0: [fn(state: ecx) -> al ret 0]; "docs/runtime/agent-api.md";
    /// Stops QoS and unregisters the session's key. The key id is in the
    /// session at +0x1c while +0x24 is set.
    SESSION_RELEASE_KEY @ 0x0005_fb60: [fn(session: esi) ret 0]; "docs/runtime/agent-api.md";
    /// The network session's game start: builds game options from the
    /// session in EDX and starts the game (`0x18e8b0`); AL 1 when it
    /// started. Refuses a campaign session with more than one peer
    /// (`0x6eea5`).
    SESSION_GAME_START @ 0x0006_ec80: [fn(session: edx, flag: stack 0) -> al ret 4]; "docs/live/live-coop.md";
    /// The scoreboard badge's visibility (AL) and connection bar count
    /// (written to the out pointer).
    SCOREBOARD_CONNECTION_BARS @ 0x0006_8a90: [fn(stats: ebx, bars: stack 0) -> al ret 4], Optional; "docs/game/bots.md";
    /// Whether a machine (its 6-byte id at EAX) is in the game, AL: 1 with
    /// no simulation or for this console's machine, else its simulation
    /// view's `+0x78` (0 with no view). The player update (`0x14d2a0`, its
    /// only caller) copies it into each player's flag bit 0, and only
    /// players with it spawn (`0x14d3a0`).
    MACHINE_IN_GAME @ 0x0006_86e0: [fn(machine: eax) -> al ret 0]; "docs/game/bots.md";
    /// Gather one stored remote action into this tick's simulation update.
    SIMULATION_SLOT_ACTION @ 0x0008_4990: [fn(slot: stack 0, action: stack 1) -> al ret 8]; "docs/game/bots.md";
    /// Clear and default a game-options block (ESI). The session start
    /// calls it right after its peer and player checks.
    OPTIONS_NEW @ 0x0013_8110: [fn(options: esi) ret 0]; "docs/runtime/level-boot.md";
    /// Whether the squad session is joinable (Live presence, AL); false
    /// for a campaign session (`0x63e7a`).
    SESSION_JOINABLE @ 0x0006_3e00: [fn() -> al ret 0]; "docs/live/live-coop.md";
    /// The host's game-start session parameters (session in EBX, state 5
    /// only); for a campaign session it sets the peer limit to 1.
    SESSION_GAME_LIMITS @ 0x0006_e720: [fn(session: ebx) -> al ret 0]; "docs/live/live-coop.md";
    /// Lobby: set the squad session's map (host only): stdcall `(campaign
    /// id)`, the map id in ESI, AL 1 when set.
    LOBBY_SET_MAP @ 0x0019_a6f2: [fn(map: esi, campaign: stack 0) -> al ret 4]; "docs/live/live-coop.md";
    /// Lobby: set the squad session's game variant to the `0x130`-byte
    /// record on the stack, AL nonzero when taken: the dedicated host's
    /// call, the pause menu's Game Setup's and Quick Options' for a
    /// script variant (docs/game/rules-variants.md "Quick Options in the
    /// lobby").
    LOBBY_SET_VARIANT @ 0x0019_a728: [fn(variant: stack 0) -> al ret 4]; "docs/game/rules-variants.md";
    /// Lobby: set the squad session's campaign difficulty (AX; `0xffff`
    /// none) and, for a difficulty, clear the variant (synchronous
    /// protocol). AL 1 when set.
    LOBBY_SET_DIFFICULTY @ 0x0019_a76d: [fn(difficulty: eax) -> al ret 0]; "docs/live/live-coop.md";
    /// Lobby: put the squad lobby in a mode (stdcall; 0 co-op campaign,
    /// 2 custom game): the Xbox Live menu's Custom Game row calls it with 2
    /// once its session is made (`0x23101b`), the lobby's "Switch To: Co-Op
    /// Game" with 0 (`0x2bba30`).
    LOBBY_MODE_SET @ 0x0019_9a03: [fn(mode: stack 0) ret 4]; "docs/live/live-coop.md";
    /// Available Games' Create row (local and Live flags both zero for System Link).
    BROWSER_CREATE_SQUAD @ 0x0019_9df9: [fn(local: stack 0, live: stack 1) -> al ret 8]; "docs/live/dedicated-host.md";
    /// Per-frame reconciliation of the local UI screen and network life cycle.
    UI_SYNC_NETWORK_SCREEN @ 0x0019_9b08: [fn() ret 0]; "docs/live/dedicated-host.md";
    /// The menu leaves the squad and target, immediately or gracefully.
    UI_LEAVE_SESSIONS @ 0x0019_9e2e: [fn(immediate: stack 0) ret 4]; "docs/live/dedicated-host.md";
    /// Lobby: why the game cannot start yet (stdcall, an optional float
    /// out); 9 ready, 7 the teams or co-op check (`0x19a4be`: co-op needs
    /// exactly two players, both on this console), 8 and 10 the load and
    /// countdown states. Its status line picks the text (`0x250008`).
    LOBBY_START_STATE @ 0x0019_a2ce: [fn(progress: stack 0) -> eax ret 4]; "docs/live/live-coop.md";
    /// The start state's tail after a co-op check that passed: a countdown
    /// running (`>= 0`, state 10), else `LOBBY_ALL_READY` (AL: 9, else 8).
    LOBBY_COUNTDOWN @ 0x0006_4a60: [fn() -> eax ret 0]; "docs/live/live-coop.md";
    /// Despite the name, not a readiness count: AL 1 when this console
    /// leads the party (session `+0x72d8` equals `+0x50`, in states 3 to
    /// 8). Start state 8's text is "Waiting for ... to start the game"
    /// (docs/game/bots.md §4.2, "The start check").
    LOBBY_ALL_READY @ 0x0005_92f0: [fn() -> al ret 0]; "docs/live/live-coop.md";
    /// Refresh the pregame lobby's game type picture and option lines
    /// (EAX the screen, stack 0 the current game variant or null).
    LOBBY_VARIANT_PANEL @ 0x0025_01c0: [fn(screen: eax, variant: stack 0) ret 4]; "docs/game/ui-builder.md";
    /// The pregame lobby's input handler (ECX the screen, the event on the
    /// stack, AL handled, `ret 4`): event `+0` the kind (5 a button), `+4`
    /// the controller, `+8` the button (0 A, `0xc` Start). A or Start from
    /// a controller with a profile reaches Start Game (`0x250c30`,
    /// `0x25106c`); from one without, the profile chooser (`0x250cc3`).
    PREGAME_LOBBY_INPUT @ 0x0025_0b1e: [fn(screen: ecx, event: stack 0) -> al ret 4]; "docs/game/bots.md";
    /// The squad membership's peer count. Only leave callers treat a
    /// leader with exclusively phantom opponents as alone.
    PARTY_PEER_COUNT @ 0x0019_9f34: [fn() -> eax ret 0]; "docs/game/bots.md";
    /// Whether the player slot in AX is occupied in the squad membership.
    PARTY_PLAYER_VALID @ 0x0019_a951: [fn(player: eax) -> al ret 0]; "docs/game/bots.md";
    /// The lobby Back or pause Leave Game has committed to leaving. The
    /// entry follows the tail jump of `UI_LEAVE_SESSIONS` at `0x199e37`.
    PARTY_LEAVE @ 0x0019_9e3c: [fn(controller: stack 0) ret 4]; "docs/game/bots.md";
    /// The player accepted the matchmaking lobby's Back confirmation.
    MATCHMAKING_CANCEL_CONFIRMED @ 0x0025_2150: [fn(controller: stack 0) -> al ret 4]; "docs/game/bots.md";
    /// The pregame lobby's list item handler (ECX the screen, `ret 8`),
    /// set for its rows at `0x24fb40`: stack 0 a record whose `+0` is the
    /// input event (`+4` the controller), stack 1 the row (a word: 0 Start
    /// Game, which asks the start state at `0x251051` and starts the
    /// countdown on 9). The list takes A before the screen does, so A on
    /// Start Game never reaches `PREGAME_LOBBY_INPUT` (the run
    /// 20261119-005001 started its game with no `bots: start state` line).
    LOBBY_ROW_PRESS @ 0x0025_0fb3: [fn(screen: ecx, item: stack 0, row: stack 1) -> al ret 8]; "docs/game/bots.md";
    /// The per-tick player update (from `game_tick`, `0x13801f`): every
    /// player; a first spawn pending (flags byte `+2` bit 3) goes to
    /// `PLAYER_SPAWN`; the co-op respawn (`0x14cdd0`) runs inside it.
    PLAYER_UPDATE @ 0x0014_d3a0: [fn(update: stack 0) ret 4]; "docs/live/live-coop.md";
    /// Spawn one player (handle): unit type, a starting location from
    /// `PLAYER_SPAWN_LOCATION`, the unit; AL 0 when no location was free.
    /// Clears the first-spawn bit (`0x14f6c1`) when it spawned.
    PLAYER_SPAWN @ 0x0014_f2e0: [fn(player: stack 0) -> al ret 4]; "docs/live/live-coop.md";
    /// The tag a starting weapon value names, for the spawn's weapon pass
    /// (`0x19ef40`): 0 or over 20 the profile's own weapon, 1 none (-1),
    /// 2 a random slot of the multiplayer globals' weapon list that is not
    /// `other` (it calls itself, returning to `0x19ee18`), 3 to 20 slot
    /// `value - 3`. The pass calls it for the secondary from `0x19f03d` and
    /// `0x19f076` and for the primary from `0x19f051` and `0x19f05e`.
    STARTING_WEAPON_PICK @ 0x0019_eda0: [fn(default: stack 0, value: stack 1, other: stack 2) -> eax ret 12]; "docs/game/pogo-stick.md";
    /// The best free starting location for a player (handle in ECX): AX the
    /// index into `scnr+0x100`, `0xffff` when every spot is blocked.
    PLAYER_SPAWN_LOCATION @ 0x0014_ef00: [fn(player: ecx, unit: stack 0, a: stack 1, b: stack 2) -> eax ret 12]; "docs/live/live-coop.md";
    /// The spawn chooser's free-spot test, called only from
    /// `PLAYER_SPAWN_LOCATION` (`0x14efce`), and only for a location that
    /// weighs more than the best so far: ECX the starting location
    /// (`scnr+0x104` + `0x34` × index), EAX the unit type the player spawns
    /// as (-1 passes). AL 1 when the type's capsule fits there, 0 when
    /// something stands in it, and the chooser then keeps its best. EDI
    /// holds the chooser's player handle across the call.
    SPAWN_LOCATION_FREE @ 0x0023_bb70: [fn(location: ecx, unit: eax) -> al ret 0]; "docs/game/notes/2026-11-24-siege-spawns-and-bodies.md";
    /// Retail co-op placement: move a player (EBX) next to a position (ESI,
    /// three floats) beside an anchor unit (stack). The co-op respawn's
    /// own call is at `0x14cffd`. AL 1 when placed.
    PLAYER_PLACE_NEAR @ 0x0014_e970: [fn(player: ebx, position: esi, anchor: stack 0) -> al ret 4]; "docs/live/live-coop.md";
    /// Teleport an object to a cutscene flag: the body of HaloScript
    /// `object_teleport` (`0x2a2517`) and of
    /// `volume_teleport_players_not_inside` (`0x29f641`).
    SCRIPT_TELEPORT @ 0x0029_ffb0: [fn(object: stack 0, flag: stack 1, a: stack 2, b: stack 3) ret 16], Optional; "docs/live/live-coop.md";
    /// Whether the game may save a checkpoint now: the body of
    /// HaloScript `game_safe_to_save` (`0x2a9bb8`). A run of checks, among
    /// them every player's unit (`0x14df40`, `0x14deb0`) and "some player
    /// dead" (`players_globals+5`); AL 1 when safe.
    SAVE_SAFE @ 0x0022_6190: [fn() -> al ret 0]; "docs/live/live-coop.md";
    /// The Live join lifecycle. Its update promotes the target squad or records
    /// the title's reason for declining the join.
    LIVE_JOIN_UPDATE @ 0x0006_fd10: [fn(context: ecx) -> al ret 0], Optional; "docs/live/live-custom-games.md";
    /// The host's answer to a join request (session in EAX, request in
    /// EDI): 0 accepts, 3 refuses unless the session state (`S+0x741c`)
    /// is 5 (the lobby), 2 for an invite-only session, 4 when full.
    SESSION_JOIN_CHECK @ 0x0006_2fe0: [fn(session: eax, request: edi) -> eax ret 0]; "docs/live/live-coop.md";
    /// Build the shared game description sent in LAN discovery and Live QoS.
    SESSION_DESCRIBE @ 0x0006_4d70: [fn(description: stack 0) -> al ret 4]; "docs/game/notes/creature-entity.md";
    /// Validate a host description before connecting. The first failure reason
    /// lives at context +0x104. Both the browser and accepted invites call it.
    SESSION_CHECK_DESCRIPTION @ 0x0006_f2b0: [fn(description: esi, context: edi, players: stack 0) ret 4]; "docs/game/notes/creature-entity.md";
    /// Encode message 8, including its 16-bit version word.
    SESSION_WRITE_JOIN_REQUEST @ 0x000a_cc20: [fn(stream: stack 0, size: stack 1, message: stack 2) ret 12]; "docs/game/notes/creature-entity.md";
    /// Dispatch decoded message 8 to its session before checking or queueing
    /// the inner request.
    SESSION_RECEIVE_JOIN_REQUEST @ 0x0009_4100: [fn(network: ecx, message: esi, peer: stack 0) ret 4]; "docs/game/notes/creature-entity.md";
    /// Sync-server: start sending the game state to a joining client's
    /// view (EAX): compress the block and queue the join and state pieces
    /// ahead of its updates. AL 1 when queued.
    SIM_JOIN_SEND_STATE @ 0x0008_62e0: [fn(view: eax) -> al ret 0], Optional; "docs/live/live-coop.md";
    /// Sync-client: the synchronous-join message (view in EDI, the
    /// server's update number on the stack). AL 1 when taken.
    SIM_JOIN_RECEIVE @ 0x0008_5dc0: [fn(view: edi, update: stack 0) -> al ret 4], Optional; "docs/live/live-coop.md";
    /// Sync-client: the last (empty) game-state piece: decompress what
    /// arrived (`length` bytes) into the game-state block. AL 1 when it
    /// came to `0x3fe000` bytes.
    SIM_STATE_RECEIVED @ 0x0006_aa20: [fn(world: edi, length: stack 0) -> al ret 4], Optional; "docs/live/live-coop.md";
    /// Apply one update record (stack): player entries, the player-set
    /// change (`+0xDD0`, mask `+0xDD4`), events, the save flag (`+0x403C`).
    SIM_UPDATE_APPLY @ 0x0006_83a0: [fn(update: stack 0) ret 4], Optional; "docs/live/live-coop.md";
    /// Synchronous: send this console's player actions for the tick to
    /// the machine that builds the updates (message `0x28`, `0x180`
    /// bytes, up to four local players).
    SIM_ACTIONS_SEND @ 0x0008_5fc0: [opaque], Optional; "docs/live/live-coop.md";
    /// Encode a synchronous 0x28 after the sender has admitted it to the channel.
    SIM_ACTIONS_ENCODE @ 0x000b_2790: [fn(stream: stack 0, size: stack 1, actions: stack 2) ret 12], Optional; "docs/live/live-coop.md";
    /// Synchronous: a peer's actions arrived (from the `0x28` handler
    /// `0x949d0`).
    SIM_ACTIONS_RECEIVE @ 0x0008_5f50: [opaque], Optional; "docs/live/live-coop.md";
    /// Synchronous: send one tick's update record to one peer's view
    /// (message `0x27`; queued instead while that view is joining).
    SIM_UPDATE_SEND @ 0x0008_60b0: [opaque], Optional; "docs/live/live-coop.md";
    /// Synchronous: an update arrived; run the ticks it allows (from the
    /// `0x27` handler `0x94990`).
    SIM_CATCH_UP @ 0x0006_98e0: [opaque], Optional; "docs/live/live-coop.md";
    /// Save the game state (checkpoint): the save callbacks, then the
    /// writer `0x2151f0`.
    GAME_STATE_SAVE @ 0x0012_3e20: [opaque], Optional; "docs/runtime/checkpoints.md";
    /// Revert the game state to a saved slot.
    GAME_STATE_REVERT @ 0x0012_3f60: [opaque], Optional; "docs/runtime/checkpoints.md";
    /// Restore the whole game state from a saved image (header checked).
    GAME_STATE_RESTORE @ 0x0012_4380: [opaque], Optional; "docs/runtime/checkpoints.md";
    /// Switch the structure BSP (prepare, dispose, load, publish): stdcall
    /// `(bsp, unload)`, the index an `i16` in the first word, `unload`
    /// (a byte) whether the old BSP's tag data is released. Returns at
    /// once, AL 1, when `bsp` is both the loaded BSP (`0x4686c4`) and the
    /// active one (game globals `+2`), so it cannot rebuild the active BSP
    /// in place; [`STRUCTURE_BSP_DEACTIVATE`] then
    /// [`STRUCTURE_BSP_ACTIVATE`] do (docs/editor/realtime-edits.md,
    /// "Collision").
    BSP_SWITCH @ 0x0011_c1b0: [fn(bsp: stack 0, unload: stack 1) -> al ret 8], Optional; "docs/runtime/level-boot.md";
    /// Each game system's dispose-from-old-structure-BSP callback (the
    /// lifecycle table's column 5, `0x440dec + 0x24 n`, last to first),
    /// then the active BSP (game globals `+2`) set to -1. `BSP_SWITCH`
    /// calls it when the active BSP changes (`0x11c23a`).
    STRUCTURE_BSP_DEACTIVATE @ 0x0013_7da0: [fn() ret 0], Optional; "docs/editor/realtime-edits.md";
    /// Clears three `0x40`-byte tables of the game globals (`+0x1138`,
    /// `+0x1178`, `+0x11b8`), sets the active BSP (`+2`) to the loaded one
    /// (`0x4686c4`) and runs each game system's
    /// initialize-for-new-structure-BSP callback (column 4, `0x440de8 +
    /// 0x24 n`): physics builds its Havok world over the structure
    /// (`0x1c2690`), objects find their locations again (`0xb6bb0`).
    /// `BSP_SWITCH` calls it last (`0x11c345`).
    STRUCTURE_BSP_ACTIVATE @ 0x0013_7d40: [fn() ret 0], Optional; "docs/editor/realtime-edits.md";
    /// HaloScript `cinematic_start`.
    CINEMATIC_START @ 0x002a_9740: [opaque], Optional; "docs/runtime/level-boot.md";
    /// HaloScript `cinematic_stop`.
    CINEMATIC_STOP @ 0x002a_9760: [opaque], Optional; "docs/runtime/level-boot.md";
    /// The level select's list: one campaign's map ids from the start-level
    /// list, sorted, into the data array (stdcall `(array, campaign id)`).
    /// 1.0 asks for campaign 1 only (`0x19c190`).
    CAMPAIGN_LEVEL_LIST @ 0x0019_c6d0: [fn(array: stack 0, campaign: stack 1) ret 8]; "docs/halo1/campaign-picker.md";
    CAMPAIGN_OPTIONS_LIST @ 0x002b_14dc: [fn(list: ebx, player: stack 0) -> eax ret 4]; "docs/halo1/campaign-picker.md";
    /// Campaign Options list slot 19: the number of item widgets to link.
    /// Retail returns the active row count from the list's datum array.
    CAMPAIGN_OPTION_ITEM_COUNT @ 0x002b_160c: [fn(list: ecx) -> eax ret 0]; "docs/halo1/campaign-picker.md";
    CAMPAIGN_OPTION_TEXT @ 0x002b_1613: [fn(list: ecx, item: stack 0, arg: stack 1) ret 8]; "docs/halo1/campaign-picker.md";
    CAMPAIGN_OPTION_CHOSEN @ 0x002b_166d: [fn(list: ecx, chain: stack 0, row: stack 1) ret 8]; "docs/halo1/campaign-picker.md";
    PROFILE_LAST_MAP @ 0x0019_10d9: [fn() -> eax ret 0]; "docs/halo1/campaign-picker.md";
    PROFILE_CHECKPOINT_SET @ 0x0019_048c: [fn(slot: stack 0) ret 4]; "docs/halo1/campaign-picker.md";
    CAMPAIGN_CHECKPOINT_POLL @ 0x002b_178e: [fn(list: ecx) ret 0]; "docs/halo1/campaign-picker.md";
    CAMPAIGN_OPTION_HELP @ 0x002b_18b7: [fn(list: eax) -> eax ret 0]; "docs/halo1/campaign-picker.md";
    CHECKPOINT_QUEUE_READ @ 0x002a_da70: [fn(path: eax) -> eax ret 20]; "docs/halo1/campaign-picker.md";
    CHECKPOINT_QUEUE_WRITE @ 0x002a_dec0: [fn(path: eax) -> eax ret 20]; "docs/halo1/campaign-picker.md";
    /// The menu's campaign start: options from the selection globals
    /// `0x54e7c0`-`0x54e7c8`, the level looked up by `(campaign, map)`.
    CAMPAIGN_START @ 0x0023_643f: [fn() ret 0]; "docs/halo1/campaign-picker.md";
    /// The next level of a campaign: the smallest map id above `map` with
    /// that campaign id in the start-level list (stdcall, EAX, -1 none).
    /// The game-won path (`0x18e960`) and `0x163610` ask it.
    NEXT_LEVEL @ 0x0019_c440: [fn(campaign: stack 0, map: stack 1) -> eax ret 8]; "docs/halo1/campaign-picker.md";
    /// The start-level record of `(campaign, map)`: the first with both ids
    /// in the loaded globals' start-level list (stdcall, EAX, 0 none). Some
    /// callers pass campaign 1 whatever the map: the level select's press
    /// (`0x2c5118`), which faults on 0, and Continue (`0x199ab7`).
    CAMPAIGN_LEVEL_LOOKUP @ 0x0019_c270: [fn(campaign: stack 0, map: stack 1) -> eax ret 8]; "docs/halo1/campaign-picker.md";
    /// A finished level raises one slot's profile progress for a difficulty
    /// to map id ESI (stdcall `(slot, difficulty)`).
    PROGRESS_RAISE @ 0x0019_043f: [fn(map: esi, slot: stack 0, difficulty: stack 1) ret 8]; "docs/halo1/campaign-picker.md";
    /// The last level started (map id EAX) into every signed-in profile and
    /// `0x510840`.
    LAST_LEVEL_SET @ 0x0019_040d: [fn(map: eax) ret 0]; "docs/halo1/campaign-picker.md";
    /// The level select screen's constructor (thiscall; the lobby's passes
    /// 1 in its second argument). Sets `0x54e7c0` to 1.
    LEVEL_SELECT_NEW @ 0x002c_4ea0: [fn(screen: ecx, a: stack 0, lobby: stack 1) -> eax ret 8]; "docs/halo1/campaign-picker.md";
    /// The highest map id the signed-in profiles finished, any difficulty
    /// (EAX, -1 for none).
    PROGRESS_MAX @ 0x0019_0565: [fn() -> eax ret 0]; "docs/halo1/campaign-picker.md";
    /// Fill a screen request (ECX): the player mask, two words and the
    /// loader; `OPEN_SCREEN` and the menus' own opens call it.
    SCREEN_REQUEST @ 0x0014_9f49: [fn(request: ecx, mask: stack 0, a: stack 1, b: stack 2, loader: stack 3) ret 16]; "docs/halo1/campaign-picker.md";
    /// The action assembled for a local or remote slot becomes the unit's control.
    PLAYER_ACTION_TO_UNIT_CONTROL @ 0x0015_05f0: [fn(player: eax, unit: stack 0, action: stack 1) ret 8]; "docs/engine/sim-rewrite.md";
    PLAYER_NEUTRAL_UNIT_CONTROL @ 0x0015_0790: [fn(unit: edi) ret 0]; "docs/engine/sim-rewrite.md";
    PLAYER_LOOK_CURVE @ 0x0018_5b30: [fn(samples: esi, count: eax, axis: stack 0) ret 4]; "docs/engine/sim-rewrite.md";
    HUD_SEATS_UPDATE @ 0x0013_91ed: [fn() ret 0]; "docs/engine/sim-rewrite.md";
    HUD_SEAT_UPDATE @ 0x0013_a838: [fn(user: stack 0) ret 4]; "docs/engine/sim-rewrite.md";
    AIM_TARGET_UPDATE @ 0x001a_4900: [fn(user: eax, state: ebx, follow: stack 0) ret 4]; "docs/game/aim-assist.md";
    AIM_TARGET_SEARCH @ 0x001a_5370: [fn(camera: ecx, params: stack 0, direction: stack 1, unit: stack 2, team: stack 3, output: stack 4) ret 20]; "docs/game/aim-assist.md";
    AIM_TARGET_COMPARE @ 0x001a_66f0: [fn(lhs: stack 0, rhs: stack 1) -> eax ret 0]; "docs/game/aim-assist.md";
    SEAT_SHOT_AIM @ 0x001a_4c00: [fn(direction: eax, origin: ebx, player: stack 0, aim: stack 1, hand: stack 2) -> al ret 12]; "docs/game/aim-assist.md";
    /// One local player's control update (look input to the player's
    /// action): stdcall, the local user index first of five stack words,
    /// the player's local controller index (player `+0x24`) in EDX. Its
    /// only caller is `0x186ab0` (`0x186b16`), which `0x1870a0` runs for
    /// each bound local user. Reads slowdown and follow at
    /// `0x18622b` and calls the aim-assist search `0x1a4900` at `0x1861a1`.
    PLAYER_CONTROL_UPDATE @ 0x0018_5be0: [fn(user: stack 0, controller: edx) ret 0x14]; "docs/game/aim-assist.md";
    /// One bound local user's action (stdcall): the control update, the
    /// input-block, weapon, grenade and zoom state, the interaction
    /// search and the 0x5c-byte action at `out`. `0x1870a0` calls it for
    /// each bound local user, and the phantom action build for each
    /// phantom.
    PLAYER_ACTION_BUILD @ 0x0018_6ab0: [fn(player: stack 0, dt: stack 1, dt2: stack 2, out: stack 3) ret 0x10]; "docs/perf/sim-phantom-build.md";
    /// The weapons, vehicles, bipeds and controls in reach of the
    /// player's unit, tested into the 0xc-byte record at `out`; under
    /// `0x186ab0` (`0x186f0a`).
    PLAYER_INTERACTION_SEARCH @ 0x0015_1110: [fn(player: stack 0, out: stack 1) ret 8]; "docs/perf/sim-phantom-build.md";
    /// Gather script action-test flags and consume the controls claimed by a script.
    PLAYER_ACTION_TEST_UPDATE @ 0x0018_7b30: [fn(packet: ecx, actions: eax) ret 0]; "docs/halo1/script-builtins.md";
    /// The aim-assist parameters of a unit's held weapons, from the weapon
    /// tags (aim cone and pull angles and ranges, zoom applied):
    /// unit in ECX, the 0x24-byte result at ESI (kept to the return), zoom
    /// level on the stack, AL whether there is one. Callers: the local
    /// aim-assist search `0x1a4900` (`0x1a49a1`) and the shot aim
    /// `0x1a4c00` (`0x1a4cbf`, from the barrel fire `0x104150`).
    AIM_ASSIST_PARAMS @ 0x001a_50a0: [fn(unit: ecx, out: esi, zoom: stack 0) -> al ret 4]; "docs/game/aim-assist.md";
    /// The players screen's (`0x1e`) button event for a controller with no
    /// profile: the screen pushed, the event record in ESI (`+4` the
    /// controller, `+8` the button: 0 A, `0xc` Start). A or Start starts
    /// the profile pick (`0x1905bf`). Called from the screen's handler
    /// `0x252ad7`.
    PLAYERS_SCREEN_UNJOINED @ 0x0025_2a67: [fn(screen: stack 0, event: esi) -> al ret 4]; "docs/game/bots.md";
    /// The players screen's (`0x1e`) per-frame update (vtable `0x45a490`
    /// slot 3, thiscall, plain `ret`; from the window update `0x2358b9`):
    /// for each port it lays out the pane. A port is "joined" (name, no
    /// prompt) when its controller handle (`0x4e61cc`, `0x18f8b8`) is
    /// non-zero and its slot has a profile (`+0` bit `0x10`); with a
    /// controller and no profile it says "PRESS A TO JOIN" (`0x0f00028a`),
    /// with none "CONNECT CONTROLLER" (`0x1100028b`). Ends in the children's
    /// update `0x22e391`.
    PLAYERS_SCREEN_UPDATE @ 0x0025_2554: [fn(screen: ecx) ret 0]; "docs/game/bots.md";
    /// Signs a profile into a local player slot: the slot index, the
    /// 0x1e0-byte profile block it copies to the slot's `+0x18`, and the
    /// profile file (-1 for none, so nothing is saved). Sets the slot's
    /// `0x10` flag and builds the controls table (`0x190c34`).
    PROFILE_SIGN_IN @ 0x0018_fb34: [fn(slot: stack 0, block: stack 1, file: stack 2) ret 12]; "docs/game/bots.md";
    /// Signs the profile out of local player slot EAX: closes its saved
    /// file when it has one (`+0x1f8` not -1, `0x2172a0`), zeroes the
    /// 0x1e0-byte block at `+0x18`, clears the slot's `0x10` flag and sets
    /// `+0x1f8` to -1. Does nothing when the flag is clear. Callers
    /// `0x190d76`, `0x24b7c8`.
    PROFILE_SIGN_OUT @ 0x0018_fc08: [fn(slot: eax) ret 0]; "docs/game/bots.md";
    /// Signs local player slot EAX out altogether, as the profile UI's
    /// sign-out (`0x24b807`) and a failed host logon's (`0x190186`, at
    /// `0x1901e7`) do: leaves Xbox Live (`0x190da5`: clears the `0x20` flag
    /// and the account record at `+0x470`), frees the slot's local user
    /// (`+4`, `0x64690` zeroes its `0xd0`-byte entry at `0x4cd904`), signs
    /// the profile out (`PROFILE_SIGN_OUT`), then zeroes the flags, the
    /// player id at `+8` and the name at `+0xc1c` and sets `+4` and `+0x14`
    /// to -1. With its local user gone the squad session's local-player
    /// pass (`0x65770`, status 0) takes the slot's player out of the
    /// session. Callers `0x18f5c6`, `0x1901e7`, `0x1906be`, `0x24b807`,
    /// `0x24ba16`, `0x24ba5a`.
    SLOT_SIGN_OUT @ 0x0019_0d4b: [fn(slot: eax) ret 0]; "docs/game/bots.md";
    /// Allocates a one-shot sound after native playback admission.
    SOUND_IMPULSE_ALLOCATE @ 0x0012_6000: [fn(tag: stack 0, channel: stack 1, source: stack 2, rule: stack 3) -> eax ret 16]; "docs/halo1/sounds.md";
    /// Stops the sound instance in EBX: it runs the owner's callback and starts the fade
    /// (`+0xb0`..`+0xb8`). Called by the script stop, the retrigger of a playing line
    /// (`0x189f1f`) and the object code. A handle that is not in the pool is ignored.
    SOUND_STOP_INSTANCE @ 0x0012_6360: [fn(instance: ebx) ret 0]; "docs/engine/sound-sim.md";
    /// Chooses the channel for a due instance. The low word of EAX is -1 when no channel is available.
    SOUND_CHOOSE_CHANNEL @ 0x0012_8700: [fn(instance: stack 0, reason: stack 1) -> eax ret 8]; "docs/audio/dsound.md";
    /// Unbinds the channel and notifies the owner before freeing the instance.
    SOUND_FINISH_INSTANCE @ 0x0012_7320: [fn(instance: eax, reason: stack 0) ret 4]; "docs/audio/dsound.md";
    /// Drops the instance's companion and returns its salted slot to the sound pool.
    SOUND_FREE_INSTANCE @ 0x0012_7390: [fn(instance: edi) ret 0]; "docs/audio/dsound.md";
    /// Walks a block of predicted resources in the requested prediction mode.
    PREDICT_RESOURCES @ 0x0016_e5e0: [fn(mode: edi, resources: stack 0) -> al ret 4], Optional; "docs/perf/sim-budget.md";
    /// Smooth a mix-in's position, obstruction and occlusion.
    SOUND_SUBMIX_UPDATE @ 0x0022_0e00: [fn(source: edi, slot: stack 0) ret 4], Optional; "docs/audio/dsound.md";
    /// Sets source reverb and filter parameters on a DirectSound submix buffer.
    SOUND_SUBMIX_FX @ 0x0022_11e0: [fn(submix: eax, room: stack 0) ret 4], Optional; "docs/perf/sim-budget.md";
    /// Convert the submix filter cutoff to its coefficient word.
    SOUND_SUBMIX_CUTOFF @ 0x0021_e270: [fn(frequency: stack 0) ret 4], Optional; "docs/audio/dsound.md";
    /// Convert the submix filter resonance to its coefficient word.
    SOUND_SUBMIX_RESONANCE @ 0x0021_e2d0: [fn(resonance: stack 0) ret 4], Optional; "docs/audio/dsound.md";
    /// Construct the spatial descriptor consumed by the channel mixer.
    SOUND_CHANNEL_SPATIAL @ 0x0012_9aa0: [fn(channel: eax, gain: stack 0, output: stack 1) ret 8], Optional; "docs/audio/dsound.md";
    /// Gather the sound instance parameters and submit the channel update.
    SOUND_CHANNEL_UPDATE @ 0x0012_8ca0: [fn(channel: stack 0, gain: stack 1, listener: stack 2) ret 12], Optional; "docs/engine/sim-rewrite.md";
    /// Collect a listener's cluster ambience candidates.
    SOUND_CLUSTER_AMBIENCE @ 0x0018_d730: [fn(bsp: stack 0, cluster: stack 1, position: stack 2, fallback_radius: stack 3, palettes: stack 4, gains: stack 5, portals: stack 6, clusters: stack 7, count: stack 8) ret 36], Optional; "docs/formats/structure-bsp.md";
    /// Compare two candidate distances during ambience selection.
    SOUND_AMBIENCE_FARTHER @ 0x0018_ae50: [fn(a: stack 0, b: stack 1, distances: stack 2) -> eax ret 12], Optional; "docs/formats/structure-bsp.md";
    /// The sound frame service, once a frame after the render: among
    /// other things, one listener per mapped local user (`0x125c93`).
    SOUND_UPDATE @ 0x0012_5b40: [fn() ret 0]; "docs/engine/engine-map.md";
    /// The announcer queue's update: counts the head line's delay down,
    /// plays it when the delay runs out, and takes the next line once the
    /// one playing has finished (`announcer_queue`). Skipping it holds
    /// every queued line where it is.
    ANNOUNCER_QUEUE_UPDATE @ 0x0023_f000: [fn() ret 0]; "docs/audio/audio.md";
    /// The sound listeners' update (stdcall, one argument, `ret 4`): for
    /// each local user mapped (`[0x4e8c20] + 0xc`) it sets the user's
    /// listener from its camera and marks it active (the byte at
    /// `[SOUND_GLOBALS] + 0x8e + 0x48 * user`). An unmapped user's active
    /// byte is left alone.
    /// Called from `SOUND_UPDATE` and from the per-frame effect update.
    SOUND_LISTENERS_UPDATE @ 0x0012_7400: [fn(arg: stack 0) ret 4]; "docs/game/bots.md";
    /// Whose own sound is this (stdcall, `ret 12`): when `*kind` is 1 (a
    /// positioned sound), walks the four local users (`[0x4e8c20] + 0xc`)
    /// and, for the first mapped one in first person (`0x155760`) whose
    /// player's unit is `object` or carries it (`0xbafb0`), sets `*kind`
    /// to 0 (a plain sound, no position: heard in that user's ears) and
    /// `*user` (when not null) to the user. Called when a sound starts on
    /// an object: `0x1890e5`, `0x18935f`, `0x189441`, `0x18cf76`,
    /// `0x18d4dc`, on the game thread (the object and effect updates and the
    /// per-frame update `0x18c060`).
    SOUND_OWNER_USER @ 0x0018_d4f0: [fn(object: stack 0, kind: stack 1, user: stack 2) ret 12]; "docs/game/bots.md";
    /// Start a spatial sound at an object marker. Returns the sound instance.
    SOUND_START_AT_MARKER @ 0x0018_91d0: [fn(marker: eax, object: esi, parameters: stack 0) -> eax ret 4]; "docs/game/ai-networking.md";
    // The sound manager's outside entries: each has a Rust body under
    // `impl.sim.sound-events.table` (docs/engine/sound-instances.md, "Entries").
    /// Make the sound manager: globals, the instance pool, channels, loops, the start layer.
    SOUND_MANAGER_CREATE @ 0x0012_53d0: [fn() ret 0]; "docs/engine/sound-instances.md";
    /// Drop the sound manager: every global back to 0.
    SOUND_MANAGER_DISPOSE @ 0x0012_5600: [fn() ret 0]; "docs/engine/sound-instances.md";
    /// Ask the sound cache for a tag's first permutation (`0x218850`, mode 2).
    SOUND_PREFETCH @ 0x0012_5f10: [fn(tag: edx) ret 0]; "docs/engine/sound-instances.md";
    /// Start a sound from a request record: the gate, the promotion rule, then the admission.
    SOUND_START_RECORD @ 0x0021_d110: [fn(record: eax, tag: stack 0) -> eax ret 4]; "docs/engine/sound-instances.md";
    /// Walk the start layer (`[0x51ebe0]`): free finished starts, play delayed ones.
    SOUND_STARTS_UPDATE @ 0x0021_d390: [fn() ret 0]; "docs/engine/sound-instances.md";
    /// Make a start layer record for a tag.
    SOUND_START_LAYER_NEW @ 0x0021_d2c0: [fn(tag: ecx, scale: xmm0, arg: stack 0) -> eax ret 4]; "docs/engine/sound-instances.md";
    /// Stop a start layer record: stopping, its stop components run (mode 2), then done.
    SOUND_START_LAYER_PLAY @ 0x0021_d360: [fn(start: ecx) ret 0]; "docs/engine/sound-instances.md";
    /// Run a start layer record: the gate, the admission or the looping sound.
    SOUND_START_LAYER_RUN @ 0x0021_d630: [fn(start: stack 0, mode: stack 1) -> eax ret 8]; "docs/engine/sound-instances.md";
    /// Finish every sound instance with reason `0xA`.
    SOUND_STOP_ALL @ 0x0012_6400: [fn() ret 0]; "docs/engine/sound-instances.md";
    /// Choose the background ambience sounds from a candidate list (sound globals `+0x1ac`).
    SOUND_AMBIENCE_SELECT @ 0x0012_6660: [fn(candidates: eax) ret 0]; "docs/engine/sound-instances.md";
    /// Set the sound environment: current, previous and blend at sound globals `+0x1f8`.
    SOUND_ENVIRONMENT_SET @ 0x0012_72c0: [fn(environment: ecx) ret 0]; "docs/engine/sound-instances.md";
    /// The first mapped local user's listener, or 0 when it is inactive.
    SOUND_FIRST_LISTENER @ 0x0012_5360: [fn() -> eax ret 0]; "docs/engine/sound-instances.md";
    /// Pause (1) or resume (0) the sound channels.
    SOUND_SET_PAUSED @ 0x0012_5a90: [fn(paused: eax) ret 0]; "docs/engine/sound-instances.md";
    /// The sound pump during a long load: mixes the channels when the game has waited.
    SOUND_LOAD_PUMP @ 0x0012_5d60: [fn() ret 0]; "docs/engine/sound-instances.md";
    /// Start or update an object's looping sound.
    SOUND_LOOP_UPDATE @ 0x0021_a250: [fn(tag: stack 0, owner: stack 1, a: stack 2, b: stack 3, source: stack 4, mode: stack 5, c: stack 6, scale: stack 7) -> al ret 32]; "docs/engine/sound-instances.md";
    /// Move a looping sound's source.
    SOUND_LOOP_SOURCE @ 0x0021_9c30: [fn(looping: eax, source: stack 0) ret 4]; "docs/engine/sound-instances.md";
    /// Choose a permutation of a tag's pitch range (`Chooser::choose`).
    SOUND_CHOOSE @ 0x0021_9070: [fn(tag: edi, range: stack 0) -> eax ret 4]; "docs/engine/sound-instances.md";
    /// The chooser's try loop (`chooser::pick`).
    SOUND_PICK @ 0x0021_9110: [fn(tag: eax, range: ecx, used: esi, last: stack 0, seed: stack 1, exhausted: stack 2, invert: stack 3) -> eax ret 16]; "docs/engine/sound-instances.md";
    /// A pitch range's history word, when the tag's class keeps one.
    SOUND_HISTORY_LAST @ 0x0021_9430: [fn(range: stack 0, tag: stack 1) -> eax ret 8]; "docs/engine/sound-instances.md";
    /// A tag's promotion bank, or 0 when it has none.
    SOUND_PROMOTION_BANK @ 0x0021_94a0: [fn(tag: eax) -> eax ret 0]; "docs/engine/sound-instances.md";
    /// Bind the sound instance to the speaking biped's mouth animation.
    BIPED_DIALOGUE_LIP_SYNC @ 0x0010_9220: [fn(unit: ebx, sound: stack 0, instance: stack 1) ret 8]; "docs/game/ai-networking.md";
    /// Per-type file bound called by the cache header validator (0x1228b8).
    CACHE_TYPE_LIMIT @ 0x0021_4ed0: [fn(kind: ecx) -> eax ret 0]; "docs/halo1/sounds.md";
    /// Apply the unit control packet (EAX unit datum, ECX control). The
    /// recording player calls this after its event decoder, at 0x1fb630.
    UNIT_APPLY_CONTROL @ 0x000c_6de0: [fn(unit: eax, control: ecx) ret 0]; "docs/halo1/campaign.md";
    /// The map cache copy machine: one state step per call (submit a
    /// 1 MiB read or write job, take its result, checksum, publish), AL
    /// returned and ignored by its callers. They step it only while the
    /// job-done byte `0x55bd0c` is set: the loading-screen loop `0x163890`
    /// (once a frame, at `0x1639bd`), the main loop `0x12b450` and the
    /// flush loop `0x213fa0` (back to back, nothing drawn).
    CACHE_COPY_STEP @ 0x0021_41f0: [fn() -> al ret 0]; "docs/runtime/cache-slots.md";
    /// A map's cache status by path (EAX, a C string): 3 resident in a
    /// slot, 4 a copy request done, 2 queued, 1 the other queue, 0 none.
    /// Reads only the slot paths, the copy requests, the two queued paths
    /// and the CRT locale; writes only its own stack. The frame poll
    /// `0x163680` asks it three to five times a frame.
    MAP_STATUS @ 0x0021_3f10: [fn(path: eax) -> eax ret 0], Optional; "docs/runtime/cache-slots.md";
    // Slice 11 of docs/engine/sim-rewrite-objects.md: the small type updates.
    /// Item update: slot `+40` of `item` (weapons, equipment and garbage inherit it).
    ITEM_UPDATE @ 0x0010_b4d0: [fn(object: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    /// Scenery update: slot `+40` of `scen`.
    SCENERY_UPDATE @ 0x0010_a2f0: [fn(object: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    /// Device update: slot `+40` of `devi` (machines, controls and light fixtures inherit it).
    DEVICE_UPDATE @ 0x0010_6790: [fn(object: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    /// Machine update: slot `+40` of `mach`.
    MACHINE_UPDATE @ 0x0011_52e0: [fn(object: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    /// Machine slot `+48`: whether any of the machine's entries has its bit set in `bits`; 1 to `[out]` on a hit.
    MACHINE_SLOT_48 @ 0x0011_5a40: [fn(object: stack 0, bits: stack 1, out: stack 2) -> al ret 12], Hot; "docs/engine/object-update.md";
    /// Device pose: slot `+6c` of `devi`; evaluates the channels at body `+18c` and `+1ac`.
    DEVICE_POSE @ 0x0010_7000: [fn(object: stack 0, a: stack 1, b: stack 2, c: stack 3) ret 16], Hot; "docs/engine/object-update.md";
    /// Projectile update: slot `+40` of `proj`; calls the projectile step `0xf8eb0`.
    PROJECTILE_UPDATE @ 0x000f_8de0: [fn(object: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    /// Crate update: slot `+40` of `bloc`.
    CRATE_UPDATE @ 0x0011_bce0: [fn(object: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    /// Creature update: slot `+40` of `crea`.
    CREATURE_UPDATE @ 0x0011_8700: [fn(object: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    /// Creature post-physics update: slot `+44` of `crea`.
    CREATURE_POST_UPDATE @ 0x0011_8b80: [fn(object: stack 0) ret 4], Hot; "docs/engine/object-update.md";
    // Slice 11: the projectile step.
    /// Projectile step: one tick of a projectile's flight (homing, drag, gravity, collision, up to ten hits), called by `PROJECTILE_UPDATE`.
    PROJECTILE_STEP @ 0x000f_8eb0: [fn(object: stack 0, velocity: stack 1) ret 8], Hot; "docs/engine/object-update.md";
    // Slice 4 of docs/engine/sim-rewrite-objects.md: the post-update shell.
    /// The bounding spheres from the root node matrix, and the spatial partition relink; `al` 1 when they were stored.
    OBJECT_BOUNDS @ 0x000b_def0: [fn(object: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    /// The attachment refresh: light lists, the object's lights, then children and (with `recurse`) siblings.
    OBJECT_ATTACHMENTS @ 0x000b_ef30: [fn(object: stack 0, lights: stack 1, lists: stack 2, recurse: stack 3, keep: stack 4) ret 20], Hot; "docs/engine/object-update.md";
    /// Hands the node matrices to the slot `+74` dispatcher when the model has them.
    OBJECT_NODES_NOTIFY @ 0x000b_e1d0: [fn(object: eax) ret 0], Hot; "docs/engine/object-update.md";
    /// The physics component's transform and velocities into the object.
    OBJECT_COMPONENT_TRANSFORM @ 0x000b_c5e0: [fn(object: stack 0) ret 4], Hot; "docs/engine/object-update.md";
    /// Set position, forward and up (any may be null); `al` 0 when a position was outside the world.
    OBJECT_SET_TRANSFORM @ 0x000b_7430: [fn(object: stack 0, position: stack 1, forward: stack 2, up: stack 3, connect: stack 4, refresh: stack 5, notify: stack 6, value: stack 7, keep: stack 8) -> al ret 36], Hot; "docs/engine/object-update.md";
    /// Set linear and angular velocity (either may be null); `quiet` skips the replication mark.
    OBJECT_SET_VELOCITY @ 0x000b_7740: [fn(object: eax, linear: ecx, angular: edi, quiet: stack 0) ret 4], Hot; "docs/engine/object-update.md";
    /// The object's position in world space, through its parent's node matrix; returns `out`.
    OBJECT_WORLD_POSITION @ 0x000b_9dd0: [fn(object: eax, out: esi) -> eax ret 0], Hot; "docs/engine/object-update.md";
    /// The object's forward and up in world space (either output may be null).
    OBJECT_WORLD_BASIS @ 0x000b_9fc0: [fn(object: eax, forward: esi, up: edi) ret 0], Hot; "docs/engine/object-update.md";
    /// The root ancestor's linear and angular velocity (either output may be null).
    OBJECT_ROOT_VELOCITY @ 0x000b_a1d0: [fn(object: eax, linear: ecx, angular: stack 0) ret 4], Hot; "docs/engine/object-update.md";
    // Slice 5 of docs/engine/sim-rewrite-objects.md: lights.
    /// The tick's light walk: every live light's record update, relink and expiry.
    LIGHTS_UPDATE @ 0x000c_1400: [fn() ret 0], Hot; "docs/engine/object-update.md";
    /// A light record's modulation step (brightness, colour and the two outputs at `[out]`), called by `LIGHT_RECORD_UPDATE`.
    LIGHT_RECORD_PHASE @ 0x000c_0b00: [fn(colour: eax, block: ecx, light: stack 0, input: stack 1, brightness: stack 2, out: stack 3) ret 16], Hot; "docs/engine/object-update.md";
    /// Place an active light and add it to the light lists.
    LIGHT_LINK @ 0x000c_2d00: [fn(light: stack 0) ret 4], Hot; "docs/engine/object-update.md";
    /// Remove an active light from the light lists.
    LIGHT_UNLINK @ 0x000c_3220: [fn(light: eax) ret 0], Hot; "docs/engine/object-update.md";
    /// Expire a light: unlink it, update its marker parent's flag when `notify`, free the datum.
    LIGHT_EXPIRE @ 0x000c_3260: [fn(light: stack 0, notify: stack 1) ret 8], Hot; "docs/engine/object-update.md";
    // Slice 6a of docs/engine/sim-rewrite-objects.md: the unit update's shell and movement.
    /// Unit movement: the aim and look directions turned toward the desired ones (`UNIT_UPDATE`'s third call).
    UNIT_MOVEMENT @ 0x000c_60c0: [fn(unit: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    /// Clamp a unit's direction (`look` 0: aim, 1: look) to its yaw and pitch limits; whether it clamped.
    UNIT_CLAMP_DIRECTION @ 0x000c_ba50: [fn(unit: eax, direction: stack 0, look: stack 1) -> al ret 8], Hot; "docs/engine/object-update.md";
    /// Whether the unit is free of seven states and of its held weapons' test (`0x100880`).
    UNIT_IDLE_TEST @ 0x000c_85c0: [fn(unit: eax) -> al ret 0], Hot; "docs/engine/object-update.md";
    // Slice 9 of docs/engine/sim-rewrite-objects.md: the weapon update.
    /// Weapon update: slot `+40` of `weap`, after the item update.
    WEAPON_UPDATE @ 0x000f_dfb0: [fn(object: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    // Slice 10 of docs/engine/sim-rewrite-objects.md: the vehicle update.
    /// Vehicle update: slot `+40` of `vehi`.
    VEHICLE_UPDATE @ 0x000e_fde0: [fn(object: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    /// The vehicle update's gate: `al` 1 when damage flags `+10a` bit 2 is
    /// set and the definition's vehicle type (`+1f0`) is 5, or 4 with
    /// definition `+218` equal to 1; the update then skips its physics.
    VEHICLE_UPDATE_GATE @ 0x000f_1320: [fn(object: eax) -> al ret 0], Hot; "docs/engine/object-update.md";
    /// Sets the first word of the vehicle update's 16 contact records
    /// (`d8` bytes each, from `buffer + bc0`) to none.
    VEHICLE_CONTACTS_RESET @ 0x000f_06c0: [fn(buffer: eax) ret 0], Hot; "docs/engine/object-update.md";
    /// Vehicle pose: slot `+6c` of `vehi`; packs its arguments for `0xf0760`.
    VEHICLE_POSE @ 0x000f_0f50: [fn(object: stack 0, a: stack 1, b: stack 2, c: stack 3) ret 16], Hot; "docs/engine/object-update.md";
    /// Vehicle pose channels: refreshes the vehicle's channel records at body `+400` when its animation changed, then calls `callback` per live channel.
    VEHICLE_POSE_CHANNELS @ 0x000f_0760: [fn(object: stack 0, callback: stack 1, frame: stack 2) ret 12], Hot; "docs/engine/object-update.md";
    /// The vehicle pose's per-channel callback: samples the channel record's animation (`0x1c7100`, or `0x1c7200` for an overlay).
    VEHICLE_POSE_CALLBACK @ 0x000f_06e0: [fn(object: stack 0, graph: stack 1, record: stack 2, weight: stack 3, a: stack 4, scale: stack 5, frame: stack 6) ret 28], Hot; "docs/engine/object-update.md";
    // Slice 6b of docs/engine/sim-rewrite-objects.md: the unit update's state
    // children (`UNIT_CONTROL_REFRESH`, `ce0c0`, is named above).
    /// Unit control: desired aim and look, timed bits, the two linked objects (`UNIT_UPDATE`'s first call).
    UNIT_UPDATE_CONTROL @ 0x000c_49b0: [fn(unit: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    /// Unit state: timers, unit states, held weapons (`UNIT_UPDATE`'s second call).
    UNIT_UPDATE_STATE @ 0x000c_58f0: [fn(unit: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    /// Powered seats: calls `UNIT_CONTROL_REFRESH`, then moves each seat's power (`UNIT_UPDATE`'s fourth call).
    UNIT_POWERED_SEATS @ 0x000c_5eb0: [fn(unit: eax) -> al ret 0], Hot; "docs/engine/object-update.md";
    // Slice 6c of docs/engine/sim-rewrite-objects.md: the rest of the unit
    // update (`UNIT_DIALOGUE_UPDATE` and `UNIT_LIGHT_UPDATE` are named above).
    /// The unit's timers (`+2e8`, `+2ca` with the deferred injury notice,
    /// `+1f4`); `al` whether any was running (`UNIT_UPDATE`'s fifth call).
    UNIT_TIMERS @ 0x000c_6740: [fn(unit: eax) -> al ret 0], Hot; "docs/engine/object-update.md";
    /// Active camouflage: the levels at `+2b0` and `+2b4` moved toward body
    /// `+134` bits 3 and 4, and the Assassins skull's AI camouflage.
    UNIT_CAMO_UPDATE @ 0x000c_6810: [fn(unit: eax) ret 0], Hot; "docs/engine/object-update.md";
    /// The unit's team `+138`: the player's, the actor's, or for a vehicle
    /// its seated units'.
    UNIT_TEAM_UPDATE @ 0x000c_6990: [fn(unit: eax) ret 0], Hot; "docs/engine/object-update.md";
    /// Start a seat iterator (`0x18` bytes at `iterator`) at the unit's top
    /// biped or vehicle ancestor.
    UNIT_SEATS_BEGIN @ 0x000d_0620: [fn(unit: eax, iterator: ebx) ret 0], Hot; "docs/engine/object-update.md";
    /// The iterator's next biped or vehicle (its handle at `+10`, seat at
    /// `+14`, parent at `+c`); `al` whether there was one.
    UNIT_SEATS_NEXT @ 0x000d_0690: [fn(iterator: edx) -> al ret 0], Hot; "docs/engine/object-update.md";
    /// The unit's animation state (`0xa0e` bytes; the vehicle update calls
    /// it): picks and starts the unit's two animation names and steps the
    /// limits block's three channels; `al` from its last two callees and
    /// the channels.
    UNIT_ANIMATION_STATE @ 0x0011_1650: [fn(unit: stack 0, record: stack 1) -> al ret 8], Hot; "docs/engine/object-update.md";
    // Slice 7a of docs/engine/sim-rewrite-objects.md: the biped update.
    /// Biped update: slot `+40` of `bipd`.
    BIPED_UPDATE @ 0x000d_d360: [fn(object: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    /// The first object up the parent chain from `object` (itself
    /// included) whose body `+4` bit 26 is clear; none when the chain ends
    /// first. The biped update's first call, on its parent.
    OBJECT_CARRIER @ 0x000b_af40: [fn(object: eax) -> eax ret 0], Hot; "docs/engine/object-update.md";
    /// The biped's counter at body `+39d`, counted down; at 1 in state 3 it
    /// starts an animation (`10f430`). `al` 1 when it counted.
    BIPED_TIMER_UPDATE @ 0x000e_2d80: [fn(object: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    /// A seconds value times the tick rate, rounded by `fistp` (`ecx`
    /// returns the same integer).
    GAME_SECONDS_TO_TICKS @ 0x0014_69f0: [fn(seconds: stack 0) -> eax ret 4], Hot; "docs/engine/object-update.md";
    /// Visit active unit actions in order. A callback returning zero clears
    /// its action. Returns one if any callback returned nonzero.
    UNIT_ACTIONS_UPDATE @ 0x000e_6830: [fn(object: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    /// An action request for the unit: eight words on the stack, the action
    /// first, handed to `0xe6900`.
    BIPED_ACTION @ 0x000e_68c0: [fn(action: ecx, object: stack 0) ret 4], Hot; "docs/engine/object-update.md";
    /// The animation kind (0, 1, or 2 for any other) of an animation name
    /// (string ID).
    BIPED_ANIMATION_KIND @ 0x000d_d300: [fn(name: ecx) -> al ret 0], Hot; "docs/engine/object-update.md";
    /// `eax` 1 when an unparented biped in state 1 or 3 has been in the air
    /// (body `+348` bit 0) long enough to land, else 0.
    BIPED_AIRBORNE_TEST @ 0x000e_4050: [fn(object: eax) -> eax ret 0], Hot; "docs/engine/object-update.md";
    /// After the fall counter's time, the root velocity against a speed;
    /// above it, `0xe4d90` with kind 2.
    BIPED_FALL_UPDATE @ 0x000e_45e0: [fn(object: esi) ret 0], Hot; "docs/engine/object-update.md";
    /// The animation kind's counter at body `+39b` and, for two animation
    /// names, `0xe4d90` with kind 3 and a weight.
    BIPED_ANIMATION_STATE_UPDATE @ 0x000e_4c50: [fn(object: edi) ret 0], Hot; "docs/engine/object-update.md";
    /// The look toward the target (body `+13c`): a head marker, the aim
    /// clamped to the look limits, and body `+3cc..3d4` stepped toward the
    /// result.
    BIPED_LOOK_UPDATE @ 0x000d_e190: [fn(object: eax) ret 0], Hot; "docs/engine/object-update.md";
    // Slice 7b of docs/engine/sim-rewrite-objects.md: the biped
    // post-physics update.
    /// Biped post-physics update: slot `+44` of `bipd`.
    BIPED_POST_UPDATE @ 0x000d_d990: [fn(object: stack 0) ret 4], Hot; "docs/engine/object-update.md";
    /// The step of the crouch fraction (body `+2c4`) this tick: the
    /// definition's rate (an actor may scale it) times the tick length,
    /// toward 1 or 0 by the control bit (body `+134` bit 23).
    BIPED_CROUCH_STEP @ 0x000d_d7d0: [fn(object: eax) -> xmm0 ret 0], Hot; "docs/engine/object-update.md";
    /// The height the crouch step moves the biped this tick into `out`,
    /// `al` 1 when there is one (landing or forced to crouch, the step not
    /// zero).
    BIPED_CROUCH_HEIGHT @ 0x000d_d8e0: [fn(object: edi, out: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    /// `al` 1 in state 6 with body `+3f8` set and `+348` bit 13.
    BIPED_CROUCH_FORCED @ 0x000e_0dc0: [fn(object: eax) -> al ret 0], Hot; "docs/engine/object-update.md";
    /// A collision query below an airborne biped and the stuck counter at
    /// body `+3ab`.
    BIPED_GROUND_PROBE @ 0x000e_30c0: [fn(object: ebx) ret 0], Hot; "docs/engine/object-update.md";
    /// `al` the flag bit of the surface the biped stands on (body `+354`),
    /// 0 with a contact object.
    BIPED_GROUND_FLAG @ 0x000d_df60: [fn(object: eax) -> al ret 0], Hot; "docs/engine/object-update.md";
    /// The ground and jump counters at body `+39c` and `+39e`, and the
    /// jump `0xe4770` when the control asks for it; `al` 1 when it jumped.
    BIPED_JUMP_UPDATE @ 0x000e_4680: [fn(object: edi) -> al ret 0], Hot; "docs/engine/object-update.md";
    /// Fills the physics step's input record (`0x1e68a0`) and, by the
    /// state, its fields `+44`, `+48`, `+4c`.
    BIPED_PHYSICS_INPUT @ 0x000d_d0d0: [fn(object: stack 0, record: stack 1, flag: stack 2) ret 12], Hot; "docs/engine/object-update.md";
    /// The landing's kind and ticks at body `+34c..34f`, from a speed in
    /// `xmm2` against the definition's bands.
    BIPED_LANDING_UPDATE @ 0x000e_4330: [fn(object: eax) ret 0], Hot; "docs/engine/object-update.md";
    /// The contact object at body `+394` and its counter `+398`.
    BIPED_CONTACT_UPDATE @ 0x000e_2fa0: [fn(object: eax, contact: ebx, unused: stack 0) ret 4], Hot; "docs/engine/object-update.md";
}

/// The point at `base`, if the table names one.
pub fn by_base(base: u32) -> Option<&'static Point> {
    ALL.iter().copied().find(|p| p.base == base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_address_is_named_once() {
        let mut seen = std::collections::BTreeMap::new();
        for p in ALL {
            assert!(seen.insert(p.base, p.name).is_none(), "{} twice", p.name);
            assert!(p.doc.starts_with("docs/"), "{}: doc {}", p.name, p.doc);
        }
        for g in GLOBALS {
            assert!(seen.insert(g.base, g.name).is_none(), "{} twice", g.name);
        }
    }

    #[test]
    fn the_macro_reads_signatures_and_flags() {
        assert_eq!(
            GAME_TICK.sig,
            Sig::Fn {
                args: &[],
                ret: None,
                pop: 0
            }
        );
        assert_eq!(GAME_ACTIVATE.sig.arg("options"), Some(Loc::Eax));
        assert_eq!(WEAPON_FIRE.sig.arg("barrel"), Some(Loc::Stack(1)));
        assert_eq!(WEAPON_FIRE.class, Class::Optional);
        assert!(LOCALE_FIND.hot && !GAME_TICK.hot);
        assert!(PLAYER_KILLED.sig.is_opaque());
        assert_eq!(GAME_MEDAL.sig.arg("medal"), Some(Loc::Eax));
        assert_eq!(GAME_MEDAL.sig.arg("player"), Some(Loc::Ecx));
        assert_eq!(GAME_MEDAL.sig.arg("count"), Some(Loc::Ebx));
        assert!(matches!(
            GAME_MEDAL.sig,
            Sig::Fn {
                ret: None,
                pop: 0,
                ..
            }
        ));
        assert_eq!(GAME_MEDAL.class, Class::Optional);
        // The engine vtable slots are thiscall: the engine object in ECX.
        assert_eq!(SLAYER_KILL.sig.arg("engine"), Some(Loc::Ecx));
        assert_eq!(SLAYER_KILL.sig.arg("victim"), Some(Loc::Stack(1)));
        assert!(matches!(
            SLAYER_KILL.sig,
            Sig::Fn {
                ret: None,
                pop: 16,
                ..
            }
        ));
        assert!(matches!(
            ENGINE_TRAIT_QUERY.sig,
            Sig::Fn {
                ret: Some(Loc::Al),
                pop: 8,
                ..
            }
        ));
        for query in [
            &CTF_TRAIT_QUERY, &ODDBALL_TRAIT_QUERY, &KING_TRAIT_QUERY,
            &JUGGERNAUT_TRAIT_QUERY, &TERRITORIES_TRAIT_QUERY,
        ] {
            assert_eq!(query.sig, ENGINE_TRAIT_QUERY.sig, "{}", query.name);
        }
        // The routines a script's calls reach: register arguments, no pop.
        assert_eq!(JUGGERNAUT_ROLE_SET.sig.arg("player"), Some(Loc::Eax));
        assert_eq!(JUGGERNAUT_ROLE_SET.sig.arg("on"), Some(Loc::Ecx));
        assert!(matches!(
            JUGGERNAUT_ROLE_SET.sig,
            Sig::Fn {
                ret: None,
                pop: 0,
                ..
            }
        ));
        assert_eq!(ODDBALL_BALL_RETURN.sig.arg("ball"), Some(Loc::Eax));
        assert!(matches!(
            ODDBALL_BALL_RETURN.sig,
            Sig::Fn {
                ret: None,
                pop: 0,
                ..
            }
        ));
        assert_eq!(OBJECTIVE_RETURN.sig.arg("object"), Some(Loc::Esi));
        assert!(matches!(
            OBJECTIVE_RETURN.sig,
            Sig::Fn {
                ret: None,
                pop: 4,
                ..
            }
        ));
        assert_eq!(by_base(0x0013_7fe0).map(|p| p.name), Some("GAME_TICK"));
    }
}
