//! The late layers per item, in the guest's order: fog (layer 16) and
//! water (layer 6) draw every item their stage can, and only the items it
//! refuses keep their guest draw, each in its place
//! (docs/graphics/retire-d3d-r5.md, "Fog and water per item").
//!
//! The walk before the layer's build (`native_render::fog::clear_fog`)
//! cannot place an accepted item beside a kept one: the layer draw sorts
//! its entries and the passes blend, so the order is the sort's. It names
//! each accepted item `order` and clears nothing, and [`defer`] holds the
//! layer. Then, over the sorted entries:
//!
//! - at the dispatch boundary `0x0003bc51` (`LAYER_BUILT`, [`at_built`])
//!   the stage draws the run from the first entry
//!   (`native_render::fog::sorted_run`), up to the first entry it keeps;
//! - after each entry the guest dispatched (`0x0003bc90`,
//!   `LAYER_ENTRY_DONE`, [`after_entry`]) it draws the run that follows.
//!
//! Each run's entries are passed over by the loop
//! (`sorted_entries::Sorted::pass_over`) once its pass commits, so the
//! guest's draws and the stage's interleave in the sort's order. Skipping
//! a run's draws must leave the next kept entry the device drawing them
//! would: the stage checks each item's implementations against the device
//! as the next switch leaves it (`sorted_entries::cached_restore` over the
//! device, `layer_pass::skip_holds`), else the item is kept (`state`).
//!
//! The items the guest still draws are counted
//! `scene_kept_<layer>_<kind>_<cause>` once, when the layer is done (its
//! last entry, or the next layer draw: [`stale`]); the runs' items are
//! `layer_plan_<layer>_sorted_taken`, the layers held
//! `layer_plan_<layer>_sorted_views`.

use std::collections::HashSet;
use std::sync::Mutex;

use guest_layout::memory::GuestMemory;
use frame_scene::LatePart;
use native_render::camera::GuestRead;
use native_render::skip::{CURRENT_LAYER, CURRENT_LIST};
use native_render::sorted_entries::Sorted;

use crate::native_fog::Mem;
use crate::native_layers::{Left, count, count_kept_as};

/// The cause the walk before the build gives an accepted item beside a
/// kept one.
pub const ORDER: &str = "order";

/// Where a run starts: the layer's sorted entries, the first entry, and
/// the parts the layer's earlier runs drew (the scene's late layer holds
/// them all).
pub struct RunAt<'a> {
    pub sorted: Sorted,
    pub from: u32,
    pub prior: &'a [LatePart],
}

/// What a stage made of a run.
#[derive(Debug, PartialEq, Eq)]
pub enum Run {
    /// Drawn and committed: the list slots, the entries to pass over and
    /// the parts.
    Drawn { slots: Vec<u32>, entries: Vec<u32>, parts: Vec<LatePart> },
    /// Its first entry is one the guest keeps (named in `left`): the next
    /// run starts after it.
    Stopped,
    /// The view is refused (named in `left`): no later run is asked.
    Refused,
}

/// A late layer held for its sorted entries.
#[derive(Debug)]
pub struct Deferred {
    layer: u32,
    list: u32,
    /// The causes: the walk's before the build, `order` dropped as the
    /// runs decide, then the runs'.
    left: Left,
    /// What the guest's layer draw held before the build, by slot and
    /// kind, with whether the item's record read.
    held: Vec<(u32, &'static str, bool)>,
    sorted: Option<Sorted>,
    drawn: HashSet<u32>,
    parts: Vec<LatePart>,
    refused: bool,
    /// The kept counters' site (`""` the scene, `alt` the alternate view).
    site: &'static str,
    /// A late layer (6 or 16), which the scene records as the guest
    /// draws it when no run draws.
    late: bool,
}

/// Whether the walk before the build left accepted items to the sort.
pub fn wants(left: &Left) -> bool {
    left.items.values().any(|&c| c == ORDER)
}

/// Holds `layer` of the current list for its sorted entries, with the
/// walk's causes. `false` (nothing held) when the list does not read: the
/// caller counts what it keeps as before.
pub fn defer(slot: &Mutex<Option<Deferred>>, mem: &dyn GuestRead, layer: u32, left: &Left) -> bool {
    let (Some(list), Some(held)) = (mem.read_u32(CURRENT_LIST), native_render::fog::guest_items(mem, layer)) else {
        return false;
    };
    let held = held.into_iter().map(|h| (h.slot, h.kind(), h.item.is_some())).collect();
    hold(slot, Hold { layer, list, site: "", late: true, held }, left);
    true
}

/// A layer to hold: its list, the counters' site, whether it is a late
/// layer, and what the guest's layer draw holds of it, by slot and kind,
/// with whether the item's record read.
pub struct Hold {
    pub layer: u32,
    pub list: u32,
    pub site: &'static str,
    pub late: bool,
    pub held: Vec<(u32, &'static str, bool)>,
}

/// Holds a layer for its sorted entries with the walk's causes, its
/// `order` items to be decided by the runs.
pub fn hold(slot: &Mutex<Option<Deferred>>, h: Hold, left: &Left) {
    let mut left = left.clone();
    left.items.retain(|_, c| *c != ORDER);
    count(h.layer, "sorted_views", 1);
    *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(Deferred {
        layer: h.layer,
        list: h.list,
        left,
        held: h.held,
        sorted: None,
        drawn: HashSet::new(),
        parts: Vec::new(),
        refused: false,
        site: h.site,
        late: h.late,
    });
}

/// Counts what the guest drew of a held layer, once: every item it held
/// that no run drew, by its cause. A layer no run drew anything of is
/// written to the scene as the guest draws it.
fn finish(mem: &dyn GuestMemory, d: Deferred) {
    for &(slot, kind, record) in &d.held {
        if d.drawn.contains(&slot) {
            continue;
        }
        let cause = if record { d.left.cause(slot) } else { "record" };
        count_kept_as(d.site, d.layer, kind, cause, 1);
    }
    count(d.layer, "sorted_taken", d.drawn.len() as u64);
    if d.late && d.drawn.is_empty() {
        crate::scene_step::late_kept(mem, d.layer as u8, true);
    }
}

/// At a layer draw: a layer still held is done.
pub fn stale(slot: &Mutex<Option<Deferred>>, mem: &dyn GuestMemory) {
    let held = slot.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(d) = held {
        finish(mem, d);
    }
}

/// Drops what is held (a snapshot restore or a new map).
pub fn forget(slot: &Mutex<Option<Deferred>>) {
    slot.lock().unwrap_or_else(|e| e.into_inner()).take();
}

/// The stage's ask for one run.
pub type Ask<'a> = dyn FnMut(&mut dyn GuestMemory, &RunAt<'_>, &mut Left) -> Run + 'a;

/// Asks for the run from entry `from` and passes its entries over.
fn run(d: &mut Deferred, mem: &mut dyn GuestMemory, from: u32, ask: &mut Ask<'_>) {
    let Some(sorted) = d.sorted else { return };
    if d.refused || from >= sorted.count {
        return;
    }
    let at = RunAt { sorted, from, prior: &d.parts };
    match ask(&mut *mem, &at, &mut d.left) {
        Run::Drawn { slots, entries, parts } => {
            let mut m = Mem(&mut *mem);
            for &k in &entries {
                if sorted.pass_over(&mut m, k).is_none() {
                    // The pass has committed: the guest draws this entry too.
                    eprintln!("render-bridge: layer {} drew entry {k} natively but could not pass it over", d.layer);
                    count(d.layer, "sorted_pass_failed", 1);
                }
            }
            d.drawn.extend(slots);
            d.parts.extend(parts);
        }
        Run::Stopped => {}
        Run::Refused => d.refused = true,
    }
}

/// At the dispatch boundary (`esp`, `eax` the entry count): when the held
/// layer is the one being drawn, the run from the first entry. Another
/// layer's boundary leaves the held one alone.
pub fn at_built(slot: &Mutex<Option<Deferred>>, mem: &mut dyn GuestMemory, esp: u32, eax: u32, ask: &mut Ask<'_>) {
    let mut held = slot.lock().unwrap_or_else(|e| e.into_inner());
    let current = {
        let m = Mem(&mut *mem);
        (m.read_u32(CURRENT_LAYER), m.read_u32(CURRENT_LIST))
    };
    let Some(d) = held.as_mut().filter(|d| d.sorted.is_none() && current == (Some(d.layer), Some(d.list))) else {
        return;
    };
    let Some(sorted) = Sorted::at(esp, eax) else {
        // No entry: the loop does not run, and the guest draws nothing.
        let d = held.take();
        drop(held);
        if let Some(d) = d {
            finish(&*mem, d);
        }
        return;
    };
    d.sorted = Some(sorted);
    run(d, mem, 0, ask);
}

/// After the loop dispatched the entry at `esi`: the run that follows it,
/// or, after the last entry, the layer done. An entry the loop passed
/// over starts nothing (its run was asked already).
pub fn after_entry(slot: &Mutex<Option<Deferred>>, mem: &mut dyn GuestMemory, esi: u32, ask: &mut Ask<'_>) {
    let mut held = slot.lock().unwrap_or_else(|e| e.into_inner());
    let Some(d) = held.as_mut() else { return };
    let Some(sorted) = d.sorted else { return };
    let Some(k) = sorted.index_of(esi) else { return };
    let dispatched = sorted.entry(&Mem(&mut *mem), k).is_some_and(|e| !e.passed);
    if dispatched {
        run(d, mem, k + 1, ask);
    }
    if k + 1 >= sorted.count {
        let d = held.take();
        drop(held);
        if let Some(d) = d {
            finish(&*mem, d);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;
    use native_render::skip::{GEOMETRY_BUILD, GEOMETRY_DRAW, ITEM_SIZE, RENDER_RECORD_SIZE, RENDER_RECORDS};
    use native_render::world::Owner;

    const LIST: u32 = 0x004c_1a48;
    const ITEMS: u32 = 0x004d_0000;
    const ESP: u32 = 0x004e_0000;
    const TARGET: u32 = 0x004e_8000;

    /// The tests read the process-wide counters one at a time.
    static SERIAL: Mutex<()> = Mutex::new(());

    /// Layer 16 current with four structure items (clusters 1 to 4,
    /// records 1 to 4) and their entries sorted as `order`.
    fn layer(order: &[u32]) -> FlatMemory {
        let mut m = FlatMemory::new(0x0040_0000, 0x0010_0000);
        m.set_u32(CURRENT_LAYER, 16).unwrap();
        m.set_u32(CURRENT_LIST, LIST).unwrap();
        m.set_u32(LIST + 4, 4).unwrap();
        m.set_u32(LIST + 0xc, 0).unwrap();
        m.set_u32(LIST + 0x14, ITEMS).unwrap();
        m.set_u32(native_render::fog::PLANAR_READY, 1).unwrap();
        for i in 0..4 {
            let at = ITEMS + ITEM_SIZE * i;
            m.set_u32(at, GEOMETRY_BUILD).unwrap();
            m.set_u32(at + 4, GEOMETRY_DRAW).unwrap();
            m.set_u32(at + 8, 0xe000_0010).unwrap();
            m.set_u32(at + 0xc, (i + 1) << 8).unwrap();
            m.set_u32(at + 0x10, 1 << 16).unwrap();
            let record = RENDER_RECORDS + RENDER_RECORD_SIZE * (i + 1);
            m.set_u32(record, native_render::fog::RECORD_FOG).unwrap();
            m.set_u32(record + 4, u32::MAX).unwrap();
            m.set_u32(record + 0xc, (i + 1) << 9).unwrap();
        }
        for (k, &slot) in order.iter().enumerate() {
            let e = ESP + 8 + 0x18 * k as u32;
            m.set_u32(e, slot).unwrap();
            m.set_u32(e + 0x10, TARGET).unwrap();
        }
        m
    }

    fn counter(name: &str) -> u64 {
        crate::native_layers::counters().into_iter().find(|(k, _)| k == name).map_or(0, |(_, n)| n)
    }

    /// A stage that draws every cluster but 1, through the real run walk.
    fn stage(mem: &mut dyn GuestMemory, at: &RunAt<'_>, left: &mut Left) -> Run {
        let m = Mem(mem);
        let Some(run) = native_render::fog::sorted_run(&m, 16, &at.sorted, at.from, &|i| {
            if i.owner == Owner::Cluster(1) { Err("material") } else { Ok(()) }
        }) else {
            left.all("list");
            return Run::Refused;
        };
        if let Some((slot, why)) = run.stop {
            left.item(slot, why);
        }
        if run.drawn.is_empty() {
            return Run::Stopped;
        }
        let parts = run.drawn.iter().map(|i| LatePart { owner: i.owner, part: i.part }).collect();
        Run::Drawn { slots: run.slots, entries: run.entries, parts }
    }

    fn passed(m: &mut FlatMemory, k: u32) -> bool {
        Sorted { buffer: ESP + 8, count: 4 }.entry(&Mem(m), k).unwrap().passed
    }

    /// Sorted clusters 3, 1, 4, 2: the stage draws 3 before the loop, the
    /// guest draws 1, then the stage draws 4 and 2 after it. Only cluster
    /// 1 is counted kept, by its cause.
    #[test]
    fn the_stage_draws_around_the_entry_the_guest_keeps() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        static HELD: Mutex<Option<Deferred>> = Mutex::new(None);
        let mut m = layer(&[2, 0, 3, 1]);
        let mut left = Left::default();
        left.item(0, "material");
        for slot in 1..4 {
            left.item(slot, ORDER);
        }
        assert!(wants(&left));
        assert!(defer(&HELD, &Mem(&mut m), 16, &left));
        let (material, order, taken) =
            (counter("scene_kept_16_world_material"), counter("scene_kept_16_world_order"), counter("layer_plan_16_sorted_taken"));
        at_built(&HELD, &mut m, ESP, 4, &mut stage);
        assert_eq!((0..4).map(|k| passed(&mut m, k)).collect::<Vec<_>>(), vec![true, false, false, false]);
        after_entry(&HELD, &mut m, ESP + 8 + 0x18, &mut stage);
        assert_eq!((0..4).map(|k| passed(&mut m, k)).collect::<Vec<_>>(), vec![true, false, true, true]);
        // The loop passes over 2 and 3; the last ends the layer.
        after_entry(&HELD, &mut m, ESP + 8 + 0x18 * 3, &mut stage);
        assert!(HELD.lock().unwrap().is_none(), "the layer is done");
        assert_eq!(counter("scene_kept_16_world_material"), material + 1);
        assert_eq!(counter("scene_kept_16_world_order"), order, "no item waits on the order");
        assert_eq!(counter("layer_plan_16_sorted_taken"), taken + 3);
    }

    /// Another layer's boundary leaves the held layer; at the next layer
    /// draw it is done, every item kept.
    #[test]
    fn a_held_layer_whose_boundary_never_comes_keeps_every_draw() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        static HELD: Mutex<Option<Deferred>> = Mutex::new(None);
        let mut m = layer(&[0]);
        let mut left = Left::default();
        for slot in 0..4 {
            left.item(slot, ORDER);
        }
        assert!(defer(&HELD, &Mem(&mut m), 16, &left));
        m.set_u32(CURRENT_LAYER, 6).unwrap();
        at_built(&HELD, &mut m, ESP, 1, &mut |_, _, _| panic!("not this layer's"));
        let before = counter("scene_kept_16_world_unnamed");
        stale(&HELD, &m);
        assert_eq!(counter("scene_kept_16_world_unnamed"), before + 4, "the order causes went with the hold");
        assert!(HELD.lock().unwrap().is_none());
    }

    /// A refused view asks no later run and passes nothing over.
    #[test]
    fn a_refused_view_asks_no_later_run() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        static HELD: Mutex<Option<Deferred>> = Mutex::new(None);
        let mut m = layer(&[1, 0, 2, 3]);
        let mut left = Left::default();
        left.item(1, ORDER);
        assert!(defer(&HELD, &Mem(&mut m), 16, &left));
        let mut asked = 0;
        let mut refuse = |_: &mut dyn GuestMemory, _: &RunAt<'_>, left: &mut Left| {
            asked += 1;
            left.all("device");
            Run::Refused
        };
        at_built(&HELD, &mut m, ESP, 4, &mut refuse);
        after_entry(&HELD, &mut m, ESP + 8, &mut refuse);
        assert_eq!(asked, 1);
        assert!((0..4).all(|k| !passed(&mut m, k)));
    }
}
