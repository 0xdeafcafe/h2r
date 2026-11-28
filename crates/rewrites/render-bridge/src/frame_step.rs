//! The frame step's tick-visible writes (docs/graphics/framescene.md, 5.2).
//!
//! A cut producer's guest body never runs ([`crate::cut`]), so whatever the
//! tick reads from what that body wrote is made here, in the order the
//! guest made it. Each [`Row`] is one row of the 5.2 table: the writes it
//! makes, who owns it, and the test in `tests/cut.rs` that proves it. A row
//! whose writer is a later slice's body (the seed, the effects word, the
//! stamps, residency) is a no-op until that slice fills it; the row and its
//! place in the order exist now so the slice only adds the body.

use guest_layout::memory::{GuestMemory, MemFault};

/// The guest words the frame step writes: the frame's tick writes, one
/// source for this frame step and the headless one (`sim_frame`).
pub use engine::render_loop::tick_writes::Addrs;
use engine::render_loop::tick_writes;

/// Who runs a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    /// The frame owner, once per frame ([`Step::Frame`]).
    Frame,
    /// Before each of these cut producers' bodies would have run, named by
    /// `producers::REGISTRY` row ([`Step::Producer`]).
    Producers(&'static [&'static str]),
}

/// One step: the frame, or one cut producer's turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step<'a> {
    Frame,
    Producer(&'a str),
}

/// One row of framescene.md 5.2.
pub struct Row {
    pub name: &'static str,
    pub owner: Owner,
    /// The test in `tests/cut.rs` that proves the row.
    pub test: &'static str,
    make: fn(&Addrs, &mut dyn GuestMemory) -> Result<(), MemFault>,
}

fn nothing(_: &Addrs, _: &mut dyn GuestMemory) -> Result<(), MemFault> {
    Ok(())
}

fn frame_counter(a: &Addrs, g: &mut dyn GuestMemory) -> Result<(), MemFault> {
    tick_writes::frame_counter(a, g)
}

fn one_shot(a: &Addrs, g: &mut dyn GuestMemory) -> Result<(), MemFault> {
    tick_writes::one_shot(a, g)
}

fn present_counter(a: &Addrs, g: &mut dyn GuestMemory) -> Result<(), MemFault> {
    tick_writes::present_counter(a, g)
}

/// The residency requests are the object setup's: its own producer
/// (`native_scene_lists`' seam at `OBJECT_RENDER_SETUP`) stages them with the
/// records it writes, in the guest's visit order, and publishes both in one
/// turn, so the row's `make` is empty.
///
/// The rows, in the order the guest's frame makes them: object code
/// (seed, stamps, residency) and the particle draws inside the views, then
/// `render_views`' counter and one-shot byte, then the present's counter.
/// The rows 5.2 marks "guest code keeps running" and the frame wait, the
/// game-time bracket and frame timing are the frame owner's own calls and
/// have no row.
pub const ROWS: &[Row] = &[
    Row {
        name: "shared_seed",
        owner: Owner::Producers(&[]),
        test: "the_frame_step_makes_the_ticks_writes_in_guest_order",
        make: nothing,
    },
    Row {
        name: "effects_word",
        owner: Owner::Producers(&[]),
        test: "the_frame_step_makes_the_ticks_writes_in_guest_order",
        make: nothing,
    },
    Row {
        name: "particle_stamps",
        owner: Owner::Producers(&[]),
        test: "the_frame_step_makes_the_ticks_writes_in_guest_order",
        make: nothing,
    },
    Row {
        name: "animation_stamps",
        owner: Owner::Producers(&[]),
        test: "the_frame_step_makes_the_ticks_writes_in_guest_order",
        make: nothing,
    },
    Row {
        name: "residency",
        owner: Owner::Producers(&["object_setup"]),
        test: "object_setup_makes_the_guests_residency_requests",
        make: nothing,
    },
    Row {
        name: "render_frame_counter",
        owner: Owner::Frame,
        test: "the_frame_step_makes_the_ticks_writes_in_guest_order",
        make: frame_counter,
    },
    Row {
        name: "one_shot_byte",
        owner: Owner::Frame,
        test: "the_frame_step_makes_the_ticks_writes_in_guest_order",
        make: one_shot,
    },
    Row {
        name: "present_counter",
        owner: Owner::Frame,
        test: "the_frame_step_makes_the_ticks_writes_in_guest_order",
        make: present_counter,
    },
];

/// Makes `step`'s rows' writes in [`ROWS`] order and returns their names.
pub fn writes_at(
    a: &Addrs,
    mem: &mut dyn GuestMemory,
    step: Step,
) -> Result<Vec<&'static str>, MemFault> {
    let mut made = Vec::new();
    for row in ROWS {
        let mine = match (row.owner, step) {
            (Owner::Frame, Step::Frame) => true,
            (Owner::Producers(names), Step::Producer(p)) => names.contains(&p),
            _ => false,
        };
        if mine {
            (row.make)(a, mem)?;
            made.push(row.name);
        }
    }
    Ok(made)
}

/// [`writes_at`] the retail image's words.
pub fn writes(mem: &mut dyn GuestMemory, step: Step) -> Result<Vec<&'static str>, MemFault> {
    writes_at(&Addrs::HALO2, mem, step)
}

/// What a cut producer's hook runs before it answers: its turn's rows.
pub fn before(mem: &mut dyn GuestMemory, name: &str) -> Result<(), MemFault> {
    writes(mem, Step::Producer(name)).map(drop)
}

/// The layer draw `0x00044550` returning for the opaque block's layers
/// and the light passes' and shadow layers' (docs/graphics/framescene.md,
/// slice 4; docs/graphics/layer-draw.md).
///
/// The native stages clear the layer's bit on every item they draw before
/// the call (`native_layers`, `native_world`, `native_objects`,
/// `native_decorators`). When no item of the current list still holds the
/// bit, the guest body builds no entry and runs no handler, so it makes
/// none of 5.2's rows; what it leaves is its own words, which [`writes`]
/// makes: the drawn layer `[0x004858b0]` back to -1, the draw byte
/// `[0x004670bc]` set, and the shader-draw cache `0x00485b48` cleared as
/// `0x00016b10` clears it. When the cache holds an open shader
/// (`+0x24` non-zero), `0x00016b10` restores its render states through
/// `0x00018900`, a D3D write, so the guest body runs ([`Keep::OpenShader`]).
pub mod layer {
    use super::Step;
    use guest_layout::memory::{GuestMemory, MemFault};

    /// The layers whose draw can return: the opaque block's.
    pub const LAYERS: [u32; 6] = [1, 3, 5, 7, 8, 20];
    /// The light passes' and the shadow layers' (`native_lights`,
    /// `native_shadows`): the same body runs for them.
    pub const LIGHT_LAYERS: [u32; 5] = [10, 11, 12, 13, 14];
    /// R5's layers taken from the scene (`native_layers`): the scene's,
    /// the alternate view's and the first person's layer 2, the scene's
    /// layer 17, and the sorted handlers' camouflage 18 and 19 and
    /// hologram 21. The same body runs for them: each is in the draw mask
    /// `0x00bffdee` with no condition of its own.
    pub const LATE_LAYERS: [u32; 5] = [2, 17, 18, 19, 21];
    /// The late passes' fog (16) and water (6) at the scene's calls
    /// (`native_fog`, `native_water`), which count every item they leave.
    pub const FOG_WATER_LAYERS: [u32; 2] = [16, 6];
    /// Layer 16's own gate in the body (`0x00044582`): it builds entries
    /// only when the fog mode is 3 or byte 1 or 2 of the word at
    /// [`FOG_BYTES`] is set (`native_render::fog::layer_draws`).
    pub const FOG_LAYER: u32 = 16;
    pub const FOG_MODE: u32 = 0x0048_574c;
    pub const FOG_BYTES: u32 = 0x0048_5a74;
    /// The current layer the draw reads.
    pub const CURRENT_LAYER: u32 = 0x0046_7130;
    /// The current list the draw reads.
    pub const CURRENT_LIST: u32 = 0x0046_7138;
    /// The drawn layer: the current layer during the call, -1 after it.
    pub const DRAWN_LAYER: u32 = 0x0048_58b0;
    /// The byte the draw sets to 1.
    pub const DRAW_FLAG: u32 = engine::render_loop::layer::DRAW_FLAG;
    /// The shader-draw cache `0x00016b10` clears (ESI of the setup).
    pub const SETUP: u32 = engine::render_loop::layer::SETUP_CONTEXT;
    /// The cache's open shader: non-zero, `0x00016b10` calls `0x00018900`.
    pub const SETUP_OPEN: u32 = 0x24;
    /// The float `0x00016b10` stores in the cache's four slots.
    pub const SETUP_FLOAT: u32 = 0x0045_dbd4;
    /// The render records an item's argument indexes.
    pub const RENDER_RECORDS: u32 = 0x004b_a138;
    /// The structure and model geometry build (an item's `+0`).
    pub const GEOMETRY_BUILD: u32 = 0x0004_cbf0;
    /// Items walked at most.
    pub const MAX_ITEMS: u32 = 1 << 16;

    /// The cache words `0x00016b10` zeroes (`+0xc` to `+0x24`).
    const ZEROED: [u32; 7] = [0x0c, 0x10, 0x14, 0x18, 0x1c, 0x20, 0x24];
    /// The words it sets to -1.
    const MINUS_ONE: [u32; 6] = [0, 0x40, 0x1320, 0x1360, 0x13a0, 0x13e0];
    /// The slots it stores [`SETUP_FLOAT`] in.
    const FLOATS: [u32; 4] = [0x1330, 0x1370, 0x13b0, 0x13f0];

    /// Why a layer draw of the opaque block runs the guest body.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Keep {
        /// The map's stages stand down (`native_map_work`).
        StandingDown,
        /// The list or the cache does not read.
        Unread,
        /// The shader-draw cache holds an open shader.
        OpenShader,
        /// A structure item (render record `+4` is -1) holds the bit.
        Structure,
        /// An object's geometry item holds the bit.
        Object,
        /// An item with another build function holds the bit.
        Other,
    }

    impl Keep {
        pub fn name(self) -> &'static str {
            match self {
                Keep::StandingDown => "standing_down",
                Keep::Unread => "unread",
                Keep::OpenShader => "open_shader",
                Keep::Structure => "structure",
                Keep::Object => "object",
                Keep::Other => "other",
            }
        }
    }

    fn unread(_: MemFault) -> Keep {
        Keep::Unread
    }

    /// What the first item still holding `layer`'s bit is.
    fn kind(mem: &dyn GuestMemory, item: u32) -> Result<Keep, MemFault> {
        if mem.u32(item)? != GEOMETRY_BUILD {
            return Ok(Keep::Other);
        }
        // `sar 8`, then the low word sign-extended (`0x0004cc02`).
        let index = i32::from((mem.i32(item + 0xc)? >> 8) as i16);
        let record = RENDER_RECORDS.wrapping_add((index as u32).wrapping_mul(0x20));
        Ok(if mem.u32(record.wrapping_add(4))? == u32::MAX { Keep::Structure } else { Keep::Object })
    }

    /// The items of the current list still holding `layer`'s bit, in list
    /// order, each with its kind ([`Keep::Structure`], [`Keep::Object`] or
    /// [`Keep::Other`]; [`Keep::Unread`] for one whose record does not
    /// read): what the guest body draws. `Err` when the list does not
    /// read. Reads only.
    pub fn held(mem: &dyn GuestMemory, layer: u32) -> Result<Vec<(u32, Keep)>, Keep> {
        if !builds(mem, layer)? {
            return Ok(Vec::new());
        }
        let list = mem.u32(CURRENT_LIST).map_err(unread)?;
        if list == 0 {
            return Err(Keep::Unread);
        }
        let count = mem.u32(list.wrapping_add(4)).map_err(unread)?;
        let first = mem.u32(list.wrapping_add(0xc)).map_err(unread)?;
        let items = mem.u32(list.wrapping_add(0x14)).map_err(unread)?;
        // The build walks `first..count` (signed, `jge`).
        if (first as i32) < (count as i32) && count.wrapping_sub(first) > MAX_ITEMS {
            return Err(Keep::Unread);
        }
        let bit = 1u32 << layer;
        let mut out = Vec::new();
        let mut at = first;
        while (at as i32) < (count as i32) {
            let item = items.wrapping_add(at.wrapping_mul(0x18));
            if mem.u32(item + 0x10).map_err(unread)? & bit != 0 {
                out.push((at, kind(mem, item).unwrap_or(Keep::Unread)));
            }
            at = at.wrapping_add(1);
        }
        Ok(out)
    }

    /// Whether the body builds `layer`'s entries at all: false only for
    /// layer 16 with its gate off ([`FOG_LAYER`]), which then draws no
    /// item whatever its bits.
    pub fn builds(mem: &dyn GuestMemory, layer: u32) -> Result<bool, Keep> {
        if layer != FOG_LAYER || mem.u32(FOG_MODE).map_err(unread)? == 3 {
            return Ok(true);
        }
        Ok(mem.u32(FOG_BYTES).map_err(unread)? & 0x00ff_ff00 != 0)
    }

    /// Whether `layer`'s draw over the current list must run the guest
    /// body, and why: the cache's open shader, or the first item still
    /// holding the bit ([`held`]); `Ok` when it can return. Reads only.
    pub fn keep(mem: &dyn GuestMemory, layer: u32) -> Result<(), Keep> {
        mem.u32(SETUP_FLOAT).map_err(unread)?;
        if mem.u32(SETUP + SETUP_OPEN).map_err(unread)? != 0 {
            return Err(Keep::OpenShader);
        }
        match held(mem, layer)?.first() {
            Some(&(_, kind)) => Err(kind),
            None => Ok(()),
        }
    }

    /// `0x00016b10`'s writes to the shader-draw cache after its restore
    /// list: the words it zeroes, the ones it sets to -1 and the slots it
    /// stores [`SETUP_FLOAT`] in.
    pub fn clear_cache(mem: &mut dyn GuestMemory) -> Result<(), MemFault> {
        let float = mem.u32(SETUP_FLOAT)?;
        for at in ZEROED {
            mem.set_u32(SETUP + at, 0)?;
        }
        for at in MINUS_ONE {
            mem.set_u32(SETUP + at, u32::MAX)?;
        }
        for at in FLOATS {
            mem.set_u32(SETUP + at, float)?;
        }
        Ok(())
    }

    /// The guest body's lasting writes for a layer draw that builds no
    /// entry, in its order, then the frame step's 5.2 rows for the turn
    /// (none: no handler runs). Call only after [`keep`] said `Ok`.
    pub fn writes(mem: &mut dyn GuestMemory) -> Result<(), MemFault> {
        mem.set_u8(DRAW_FLAG, 1)?;
        clear_cache(mem)?;
        mem.set_u32(DRAWN_LAYER, u32::MAX)?;
        super::writes(mem, Step::Producer(PRODUCER)).map(drop)
    }

    /// The producer name the frame step's rows know a layer draw by.
    pub const PRODUCER: &str = "layer_draw";

    /// [`keep`], then [`writes`] when it can return.
    pub fn step(mem: &mut dyn GuestMemory, layer: u32) -> Result<(), Keep> {
        keep(&*mem, layer)?;
        writes(mem).map_err(unread)
    }
}
