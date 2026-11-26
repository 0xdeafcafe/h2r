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
];

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
        b.raw()
            .implement(point(&cut), cut.name, move |cpu, ctx, _| {
                let mut guest = ctx.memory();
                frame_step::before(&mut *guest, cut.name).map_err(|f| GuestFault {
                    addr: f.addr,
                    len: f.len as usize,
                    access: if f.write { Access::Write } else { Access::Read },
                    reason: FaultReason::Unmapped,
                })?;
                cpu.eax = cut.abi_result;
                guest.ret(cpu, u32::from(cut.pop))
            })
            .context_only();
    }
}
