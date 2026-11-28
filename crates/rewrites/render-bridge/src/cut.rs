//! Cut producers: a guest function whose body never runs
//! (docs/graphics/framescene.md, 8.2; docs/graphics/native-renderer.md).
//!
//! A cut producer is replaced whole. Its hook runs [`crate::frame_step::before`]
//! for the producer's name, then returns the entry's ABI result to the
//! caller without calling the body. There is no switch that brings the body
//! back: the renderer draws what the scene holds, and what it cannot draw is
//! a `scene_dropped_*` count, never a D3D draw. The body survives only as a
//! test oracle.
//!
//! Cuts are keyed by [`crate::residual_census::REGISTRY`] row name, so the
//! table and the census cannot drift: [`check`] refuses a cut whose name is
//! not a row or whose entry is not that row's.

use std::collections::HashMap;
use std::sync::Mutex;

use hooks::points::{self, Point, Sig};
use runtime::guest::{Access, FaultReason};
use runtime::GuestFault;

use crate::{frame_step, residual_census};

const DOC: &str = "docs/graphics/framescene.md";

/// One cut producer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cut {
    /// The `residual_census::REGISTRY` row it cuts.
    pub name: &'static str,
    /// The guest function; the row's entry.
    pub entry: u32,
    /// What the caller reads back in `eax`.
    pub abi_result: u32,
    /// Argument bytes the function pops (`ret n`).
    pub pop: u16,
}

/// The cut producers. A slice adds its row here with its test.
pub const CUTS: &[Cut] = &[
    // The light frame: gather, per-light passes and first-person lights in
    // one producer. Its gather is `scene_extract::lights::gather`
    // (docs/graphics/native-lights.md); it ends in `ret` with EAX 0
    // (`0x00031f4f`).
    Cut {
        name: "dynamic_lights",
        entry: 0x0003_1c80,
        abi_result: 0,
        pop: 0,
    },
    // The object shadows (docs/graphics/native-shadows.md): the Rust list
    // answers every call, and a shadow it cannot draw panics
    // (`lights: object_shadows`). Its layers draw in slot 3, before
    // layer 7. `eax` is the list's own count of casters.
    Cut {
        name: "object_shadows",
        entry: 0x0003_35e0,
        abi_result: 0,
        pop: 4,
    },
    // Both calls, argument 0 and 1; no caller reads its answer.
    FIRST_PERSON,
];

/// The first person's cut: `0x0003fdf0` takes the view in `eax` and the call
/// (0 or 1) on the stack, `ret 4`; its answer is `al = 1`, which no caller
/// reads. `native_first_person` is its producer.
pub const FIRST_PERSON: Cut = Cut {
    name: "first_person",
    entry: 0x0003_fdf0,
    abi_result: 1,
    pop: 4,
};

/// Cuts whose producer is a feature that builds the call's whole output
/// from the call's registers (the first person: `native_first_person`,
/// which draws the scene's parts), so
/// [`install`] leaves their points to it. Each still has its row here, so
/// the census and the table cannot drift. The object shadows are the same:
/// `native_lights::object_shadows` answers the list, and its drop is a count.
///
/// `dynamic_lights` is one too: its point also holds the light frame body
/// ([`crate::native_lights`]), and a point takes one implementation, so
/// that body answers the cut itself ([`answer`]) when the scene's own
/// light path runs ([`crate::native_lights::scene_path_ready`]) and
/// otherwise gathers as before.
pub const OWN_PRODUCER: &[&str] = &["dynamic_lights", "first_person", "object_shadows"];

/// A world layer draw (`LAYER_DRAW`, `0x00044550`) cut to the scene. The
/// layer draw serves every layer, so these are cut per layer: its producer
/// is [`crate::native_layers`] (layers 1, 2, 5, 7, 8, 17 and 20) and
/// [`crate::native_world`] (layer 3), which clear the layer's bit on every
/// item the scene draws. An item the scene cannot draw yet keeps its guest
/// draw and is counted `scene_kept_<layer>_<kind>_<cause>`.
#[cfg(feature = "native-render")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayerCut {
    /// The layer number.
    pub layer: u8,
    /// The slot it draws in (`native_render::layer_pass::world_slot`).
    pub slot: frame_scene::ViewSlot,
}

/// The world layers cut: 1, 3, 5, 8 and 20 in slot 1, 7 in slot 3 after the
/// object shadows, 2 in slot 6, 17 in slot 12.
#[cfg(feature = "native-render")]
pub const LAYER_CUTS: &[LayerCut] = &[
    LayerCut { layer: 1, slot: frame_scene::ViewSlot::OpaqueBase },
    LayerCut { layer: 3, slot: frame_scene::ViewSlot::OpaqueBase },
    LayerCut { layer: 5, slot: frame_scene::ViewSlot::OpaqueBase },
    LayerCut { layer: 8, slot: frame_scene::ViewSlot::OpaqueBase },
    LayerCut { layer: 20, slot: frame_scene::ViewSlot::OpaqueBase },
    LayerCut { layer: 7, slot: frame_scene::ViewSlot::ShadowsAndLayer7 },
    LayerCut { layer: 2, slot: frame_scene::ViewSlot::Layer2 },
    LayerCut { layer: 17, slot: frame_scene::ViewSlot::Layer17 },
];

/// The cut named `name`.
pub fn cut(name: &str) -> Option<&'static Cut> {
    CUTS.iter().find(|c| c.name == name)
}

/// A cut's answer: the frame step's writes for its turn, then the ABI
/// result and the return, the body never run.
pub fn answer(
    cut: &Cut,
    cpu: &mut runtime::CpuAbi,
    ctx: &mut runtime::ExecCtx,
) -> Result<runtime::TransferAbi, GuestFault> {
    let mut guest = ctx.memory();
    frame_step::before(&mut *guest, cut.name).map_err(|f| GuestFault {
        addr: f.addr,
        len: f.len as usize,
        access: if f.write { Access::Write } else { Access::Read },
        reason: FaultReason::Unmapped,
    })?;
    cpu.eax = cut.abi_result;
    guest.ret(cpu, u32::from(cut.pop))
}

/// Why a cut does not match the census.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    NotARow(&'static str),
    WrongEntry { name: &'static str, cut: u32, row: u32 },
}

/// Whether every cut in `cuts` names a registry row and its entry.
pub fn check(cuts: &[Cut]) -> Result<(), Refused> {
    for c in cuts {
        let i = residual_census::row(c.name).ok_or(Refused::NotARow(c.name))?;
        let row = &residual_census::REGISTRY[i];
        if row.entry != c.entry {
            return Err(Refused::WrongEntry {
                name: c.name,
                cut: c.entry,
                row: row.entry,
            });
        }
    }
    Ok(())
}

/// The point a cut is installed at: the points table's own where it names
/// the entry, else one made from the cut's name, the same for every call.
pub fn point(cut: &Cut) -> &'static Point {
    static MADE: Mutex<Option<HashMap<u32, &'static Point>>> = Mutex::new(None);
    if let Some(p) = points::by_base(cut.entry) {
        return p;
    }
    let mut made = MADE.lock().unwrap_or_else(|e| e.into_inner());
    made.get_or_insert_with(HashMap::new)
        .entry(cut.entry)
        .or_insert_with(|| Box::leak(Box::new(Point::new(cut.name, cut.entry, Sig::Opaque, DOC))))
}

/// Installs [`CUTS`].
#[cfg(feature = "native-render")]
pub(crate) fn build(b: &mut hooks::Builder) {
    install(b, CUTS);
}

/// Installs `cuts`: each entry returns its ABI result after the frame step's
/// writes for it, and its body never runs.
pub fn install(b: &mut hooks::Builder, cuts: &'static [Cut]) {
    if let Err(e) = check(cuts) {
        panic!("the cut table drifted from the census: {e:?}");
    }
    for cut in cuts {
        let cut = *cut;
        if OWN_PRODUCER.contains(&cut.name) {
            continue;
        }
        b.raw()
            .implement(point(&cut), cut.name, move |cpu, ctx, _| answer(&cut, cpu, ctx))
            .context_only();
    }
}
