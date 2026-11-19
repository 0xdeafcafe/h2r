//! `native-water`: the census of the water, camouflage and hologram
//! layers — 6 (the water edge blend), 18 and 19 (active camouflage) and
//! 21 (the hologram) — read-only, drawing and skipping nothing
//! (docs/graphics/native-renderer.md, "Stage 4b: water and the rest").
//! Off by default; `H2_FEATURES=impl.render.native_water=on` places the
//! observers, and `H2_NATIVE_WATER_CENSUS=<layers>` picks the layers
//! (default `6`; `6,18,19,21` or `all`).
//!
//! What it prints, at each layer draw of a census layer (the first view
//! and then every [`EVERY`]th), by caller and material:
//!
//! - the items in the layer, split by what built them: a structure item
//!   (build `0x0004cbf0`, render record `+4` `-1`) naming its cluster or
//!   placement and part, an object's item (the record's `+4` its render
//!   model datum's low word), or another build;
//! - the shaders by tag index (an item's `+8` low word) with their item
//!   counts;
//! - the water wrappers' gates when the layer is 6: bit 4 of
//!   `0x004b9fe0` (set: `0x0003c3c0(0, 0)` runs instead of the water
//!   setup and the layer does not draw, the lifted port's `0x0002c0ee`)
//!   and the cleared byte `0x00509430`.
//!
//! The load-side census (which materials have passes in a census layer,
//! and the engine textures their stages would sample — the scene-copy
//! and reflection inputs) is boot's glue (`native_water_glue`), printed
//! when the world loads with the feature on. What draws the water edge
//! blend natively is a later slice: the wrappers `0x0003ba90` and
//! `0x0003c3c0` are not traced, and the camo and hologram passes are not
//! resolved.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use engine::memory::GuestMemory;
use engine::points;
use native_render::camera::GuestRead;
use native_render::skip::{
    self, CURRENT_LAYER, CURRENT_LIST, GEOMETRY_BUILD, ITEM_SIZE, MAX_ITEMS, RENDER_RECORDS,
    RENDER_RECORD_SIZE,
};

use crate::{BuildError, Builder, Feature, Manifest};

pub const ID: &str = "native-water";
/// The switch, looked up exactly.
pub const SWITCH: &str = "impl.render.native_water";
/// The switch's value when `H2_FEATURES` does not name it.
pub const DEFAULT: bool = false;

/// The census layers `H2_NATIVE_WATER_CENSUS` can name: water (6), active
/// camouflage (18, 19) and the hologram (21).
pub const CENSUS_LAYERS: [u32; 4] = [6, 18, 19, 21];
/// Layers seen between census lines.
pub const EVERY: u64 = 1800;

/// Whether the native water census is on.
pub fn on(switches: &crate::Switches) -> bool {
    switches.feature(SWITCH, DEFAULT)
}

/// The layers `H2_NATIVE_WATER_CENSUS` names, or `None` when the variable
/// names none (`all` is every census layer; the default is water, 6).
pub fn layers(env: Option<&str>) -> Option<Vec<u32>> {
    let v = env?.trim();
    if v.is_empty() || v == "0" {
        return None;
    }
    if v.eq_ignore_ascii_case("all") {
        return Some(CENSUS_LAYERS.to_vec());
    }
    let mut out = Vec::new();
    for part in v.split(',') {
        let Ok(n) = part.trim().parse::<u32>() else {
            return None;
        };
        if !CENSUS_LAYERS.contains(&n) {
            return None;
        }
        if !out.contains(&n) {
            out.push(n);
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

/// The layers to census now ([`layers`], the default water only).
pub fn census_layers() -> Vec<u32> {
    layers(std::env::var("H2_NATIVE_WATER_CENSUS").ok().as_deref()).unwrap_or_else(|| vec![6])
}

/// Bit 4 of `0x004b9fe0`: set, `0x0003c3c0(0, 0)` runs instead of the
/// water setup and layer 6 does not draw (the lifted port's `0x0002c0ee`).
pub const WATER_GATE: u32 = 0x004b_9fe0;
/// The byte `render_scene_passes` clears before the water branch
/// (`0x0002c0ee`).
pub const WATER_CLEARED: u32 = 0x0050_9430;

/// What built one item of a census layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Caller {
    /// A structure item of cluster `n` (part `p`).
    Cluster(u32, u8),
    /// A structure item of placement `n` (part `p`).
    Placement(u32, u8),
    /// An object's item, its render model datum's low word.
    Object(u16),
    /// Another build function (address).
    Other(u32),
}

/// One census layer's tally.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tally {
    pub items: usize,
    /// Item count by caller kind.
    pub callers: BTreeMap<Caller, usize>,
    /// Item count by shader tag index.
    pub shaders: BTreeMap<u32, usize>,
}

/// Adds every item of the current list in `layer` to `tally`. `None`
/// when the list does not read (an unreadable item ends the walk with
/// what it had).
pub fn tally(mem: &dyn GuestRead, layer: u32) -> Option<Tally> {
    let list = mem.read_u32(CURRENT_LIST)?;
    if list == 0 {
        return Some(Tally::default());
    }
    let count = mem.read_u32(list + 4)?;
    let first = mem.read_u32(list + 0xc)?;
    let items = mem.read_u32(list + 0x14)?;
    let bit = 1u32 << layer;
    let mut t = Tally::default();
    for i in first..count.min(first.saturating_add(MAX_ITEMS)) {
        let item = items + ITEM_SIZE * i;
        let mask = mem.read_u32(item + 0x10)?;
        if mask & bit == 0 {
            continue;
        }
        t.items += 1;
        let build = mem.read_u32(item)?;
        let arg = mem.read_u32(item + 0xc)?;
        let caller = if build == GEOMETRY_BUILD {
            let record = (arg >> 8) as u16 as i16;
            if record < 0 {
                Caller::Other(build)
            } else {
                let base = RENDER_RECORDS + RENDER_RECORD_SIZE * record as u32;
                let model = mem.read_u32(base + 4)?;
                if model == u32::MAX {
                    match skip::structure_part(mem, item) {
                        Some((native_render::world::Owner::Cluster(c), part)) => Caller::Cluster(c, part),
                        Some((native_render::world::Owner::Instance(p), part)) => Caller::Placement(p, part),
                        None => Caller::Other(build),
                    }
                } else {
                    Caller::Object(model as u16)
                }
            }
        } else {
            Caller::Other(build)
        };
        *t.callers.entry(caller).or_default() += 1;
        *t.shaders.entry(mem.read_u32(item + 8)?).or_default() += 1;
    }
    Some(t)
}

/// Views seen per census layer, for [`EVERY`].
static VIEWS: Mutex<BTreeMap<u32, u64>> = Mutex::new(BTreeMap::new);
/// Lines printed per census layer (the first view's is the census).
static LINES: AtomicU64 = AtomicU64::new(0);

/// The guest's memory as `native_render` reads it.
struct G<'a>(&'a dyn GuestMemory);

impl GuestRead for G<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

/// The observer before the layer draw: the census of the current layer,
/// when it is one of the named ones.
pub fn before_layer(mem: &dyn GuestMemory) {
    let layer = match mem.u32(CURRENT_LAYER) {
        Ok(l) => l,
        Err(_) => return,
    };
    if !census_layers().contains(&layer) {
        return;
    }
    let n = {
        let mut views = VIEWS.lock().unwrap_or_else(|e| e.into_inner());
        let n = views.entry(layer).or_default();
        *n += 1;
        *n
    };
    if n != 1 && (n - 1) % EVERY != 0 {
        return;
    }
    let Some(t) = tally(&G(mem), layer) else { return };
    let gate = (layer == 6).then(|| {
        let skip = mem.u32(WATER_GATE).map(|w| w & 0x10 != 0).unwrap_or(false);
        let cleared = mem.u32(WATER_CLEARED).unwrap_or(0);
        format!(", water gate {} (bit 4 of {WATER_GATE:#x}), cleared byte {cleared}", if skip { "skipped" } else { "drawing" })
    });
    let mut callers = String::new();
    for (k, v) in t.callers.iter().take(8) {
        let c = match k {
            Caller::Cluster(c, p) => format!("cluster {c} part {p}"),
            Caller::Placement(p, part) => format!("placement {p} part {part}"),
            Caller::Object(m) => format!("object model {m:#06x}"),
            Caller::Other(b) => format!("build {b:#010x}"),
        };
        callers.push_str(&format!("\n  native-water:   {v:>4}  {c}"));
    }
    if t.callers.len() > 8 {
        callers.push_str(&format!("\n  native-water:   ... and {} more caller kinds", t.callers.len() - 8));
    }
    let mut shaders = String::new();
    for (s, v) in t.shaders.iter().take(8) {
        shaders.push_str(&format!(" {s:#010x} x{v}"));
    }
    if t.shaders.len() > 8 {
        shaders.push_str(&format!(" ... and {} more", t.shaders.len() - 8));
    }
    LINES.fetch_add(1, Ordering::Relaxed);
    eprintln!(
        "native-water: layer {layer}, view {n}: {} items ({} caller kinds, {} shaders){gate}\n  native-water:   shaders:{shaders}{callers}",
        t.items,
        t.callers.len(),
        t.shaders.len()
    );
}

pub struct NativeWater;

impl Feature for NativeWater {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "The water, camouflage and hologram census (read-only)")
            .doc("docs/graphics/native-renderer.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        if !on(b.switches()) {
            return Ok(());
        }
        b.raw()
            .observe_before(&points::LAYER_DRAW, "native_water_census", |o| {
                let g = o.guest();
                before_layer(&*g);
            })
            .after_feature(crate::render_timing::ID)
            .context_only();
        b.snapshot(crate::SnapshotPolicy::Stateless);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_render::camera::tests::Mem;

    const LIST: u32 = 0x004c_1a48;
    const ITEMS: u32 = 0x0060_0000;

    fn scene() -> Mem {
        let mut m = Mem::default();
        m.w(CURRENT_LAYER, 6);
        m.w(CURRENT_LIST, LIST);
        m.w(LIST, 100);
        m.w(LIST + 4, 4);
        m.w(LIST + 0xc, 0);
        m.w(LIST + 0x14, ITEMS);
        let item = |m: &mut Mem, i: u32, build: u32, record: u32, mask: u32| {
            let a = ITEMS + ITEM_SIZE * i;
            m.w(a, build);
            m.w(a + 4, 0x0004_d0b0);
            m.w(a + 8, 0xe000_0010 + i);
            m.w(a + 0xc, record << 8 | (i + 1));
            m.w(a + 0x10, mask);
        };
        item(&mut m, 0, GEOMETRY_BUILD, 2, 1 << 6);
        item(&mut m, 1, GEOMETRY_BUILD, 5, 1 << 6);
        item(&mut m, 2, GEOMETRY_BUILD, 7, 1 << 6);
        item(&mut m, 3, 0x0005_0000, 2, 1 << 6);
        m.w(RENDER_RECORDS + 0x20 * 2 + 4, u32::MAX);
        m.w(RENDER_RECORDS + 0x20 * 2, 37 << 9);
        m.w(RENDER_RECORDS + 0x20 * 5 + 4, 0xe1a3_0004);
        m.w(RENDER_RECORDS + 0x20 * 7 + 4, u32::MAX);
        m.w(RENDER_RECORDS + 0x20 * 7, 1 << 12);
        m.w(RENDER_RECORDS + 0x20 * 7 + 0xc, 140 << 18);
        m
    }

    #[test]
    fn the_census_splits_by_caller_and_shader() {
        let m = scene();
        let t = tally(&m, 6).unwrap();
        assert_eq!(t.items, 4);
        assert_eq!(t.callers[&Caller::Cluster(37, 1)], 1);
        assert_eq!(t.callers[&Caller::Placement(140, 3)], 1);
        assert_eq!(t.callers[&Caller::Object(4)], 1);
        assert_eq!(t.callers[&Caller::Other(0x0005_0000)], 1);
        assert_eq!(t.shaders[&0xe000_0012], 1);
        // Only the layer's items.
        let mut m = scene();
        m.w(ITEMS + ITEM_SIZE + 0x10, 1 << 3);
        assert_eq!(tally(&m, 6).unwrap().items, 3);
        assert_eq!(tally(&m, 3).unwrap().items, 1);
    }

    #[test]
    fn the_layer_list_parses() {
        assert_eq!(layers(None), None);
        assert_eq!(layers(Some("")), None);
        assert_eq!(layers(Some("6")), Some(vec![6]));
        assert_eq!(layers(Some("6,21")), Some(vec![6, 21]));
        assert_eq!(layers(Some("all")), Some(vec![6, 18, 19, 21]));
        assert_eq!(layers(Some("3")), None, "not a census layer");
        assert_eq!(layers(Some("6,x")), None);
    }
}
