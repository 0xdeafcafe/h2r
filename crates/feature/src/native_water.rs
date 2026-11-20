//! `native-water`: the water edge blend, layer 6, drawn by the native
//! renderer instead of the game's items (docs/graphics/native-renderer.md,
//! "Stage 4b: water and the rest"), and the census of the water,
//! camouflage and hologram layers (6, 18, 19, 21). On by default
//! ([`DEFAULT`], since 2026-11-20); `H2_FEATURES=impl.render.native_water=off`
//! turns both off for the comparison against the D3D path.
//!
//! **The skip** (layer 6 only): one context-only observer before the
//! layer draw `LAYER_DRAW` (`0x00044550`), after the fog's. It restores
//! the last skip's bits, then when the current layer is 6, the caller is
//! the late passes' call (`[esp] == [`SCENE_WATER_RETURN`]`), the
//! water gate holds (bit 4 of `0x004b9fe0` clear: the wrapper
//! `0x0003ba90` ran and the layer draws) and the list holds a structure
//! item in the layer, asks the [`Host`] (boot's glue) for the native
//! replacement. The host plans the skip over a preview, reserves the
//! replacement through the ordered handoff (recording on every copy
//! before anything is surrendered), applies the mask edits and commits;
//! the hook stores what was cleared for the restore at the next layer
//! draw. The wrappers themselves — `0x0003ba90` before, `0x0003c3c0`
//! with its alpha-only clear `0x3c515` after — are never skipped: the
//! native draws land inside their window, and the clear keeps its guest
//! order after them.
//!
//! **The census** (`H2_NATIVE_WATER_CENSUS=<layers>`, default `6`;
//! `6,18,19,21` or `all`): at each layer draw of a named layer, the
//! first view and then every [`EVERY`]th, it prints by caller and
//! material:
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
//! and reflection inputs) prints with the world's load
//! (`boot::native_water_glue`). Layers 18, 19 and 21 (active camouflage
//! and the hologram) are census-only: their caller is not established
//! and their state does not resolve yet.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use engine::memory::GuestMemory;
use engine::points;
use native_render::camera::{GuestRead, GuestWrite};
use native_render::skip::{
    self, Cleared, CURRENT_LAYER, CURRENT_LIST, GEOMETRY_BUILD, ITEM_SIZE, MAX_ITEMS, RENDER_RECORDS,
    RENDER_RECORD_SIZE,
};

use crate::{BuildError, Builder, Feature, Manifest};

pub const ID: &str = "native-water";
/// The switch, looked up exactly.
pub const SWITCH: &str = "impl.render.native_water";
/// The switch's value when `H2_FEATURES` does not name it.
pub const DEFAULT: bool = true;

/// The census layers `H2_NATIVE_WATER_CENSUS` can name: water (6), active
/// camouflage (18, 19) and the hologram (21).
pub const CENSUS_LAYERS: [u32; 4] = [6, 18, 19, 21];
/// Layers seen between census lines.
pub const EVERY: u64 = 1800;

/// Whether the native water census is on.
pub fn on(switches: &crate::Switches) -> bool {
    crate::switches::native_stage_on(switches, SWITCH)
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
pub fn census_layers() -> &'static [u32] {
    static LAYERS: OnceLock<Vec<u32>> = OnceLock::new();
    LAYERS.get_or_init(|| {
        layers(std::env::var("H2_NATIVE_WATER_CENSUS").ok().as_deref()).unwrap_or_else(|| vec![6])
    })
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
static VIEWS: Mutex<BTreeMap<u32, u64>> = Mutex::new(BTreeMap::new());
/// Lines printed per census layer (the first view's is the census).
static LINES: AtomicU64 = AtomicU64::new(0);

/// The guest's memory as `native_render` reads it.
struct G<'a>(&'a dyn GuestMemory);

impl GuestRead for G<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

/// The late passes' layer-6 call: the layer draw's return address
/// (`[esp]` at its entry, the lifted port's `0x0002c117` call). Other
/// callers of the layer draw with layer 6 current are not the scene's
/// water and stay on the D3D path.
pub const SCENE_WATER_RETURN: u32 = 0x0002_c120;
/// The layer the skip draws.
pub const WATER_LAYER: u32 = 6;

/// What draws the water edge blend: boot's glue over the GPU world and
/// the device. As the fog's ([`crate::native_fog::Host`]): plans the
/// skip over a preview, reserves the replacement (recording on every
/// copy before anything is surrendered), applies the mask edits and
/// commits; the wrappers `0x0003ba90`/`0x0003c3c0` are never skipped,
/// so the replacement lands inside their window and the alpha-only
/// clear `0x3c515` keeps its guest order after it.
pub trait Host: Send + Sync {
    fn ask(&self, mem: &mut dyn GuestMemory) -> Option<Cleared>;
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
/// Bits cleared at the last layer 6, with each item's mask as it was
/// left, until the next layer draw sets them back.
static CLEARED: Mutex<Option<Pending>> = Mutex::new(None);

/// What the last skip cleared and the masks it left.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pending {
    pub cleared: Cleared,
    /// The mask each item of `cleared` was left with.
    pub left: Vec<u32>,
}

static WATER_VIEWS: AtomicU64 = AtomicU64::new(0);
static WATER_DRAWN: AtomicU64 = AtomicU64::new(0);
static WATER_ITEMS: AtomicU64 = AtomicU64::new(0);
static WATER_GATED: AtomicU64 = AtomicU64::new(0);
static WATER_WRONG_CALLER: AtomicU64 = AtomicU64::new(0);

/// Installs the host (boot). Without one nothing is skipped.
pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

/// Layer-6 calls seen, views replaced natively, items skipped, views the
/// water gate left to the D3D path, calls from another caller.
pub fn counts() -> (u64, u64, u64, u64, u64) {
    (
        WATER_VIEWS.load(Ordering::Relaxed),
        WATER_DRAWN.load(Ordering::Relaxed),
        WATER_ITEMS.load(Ordering::Relaxed),
        WATER_GATED.load(Ordering::Relaxed),
        WATER_WRONG_CALLER.load(Ordering::Relaxed),
    )
}

/// Sets back the bits the last layer 6 cleared, on the items that still
/// hold the mask it left.
pub fn restore_pending(mem: &mut dyn GuestMemory) {
    let Some(p) = CLEARED.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return;
    };
    let mut m = crate::native_fog::Mem(mem);
    let bit = 1u32 << p.cleared.layer;
    for (&item, &left) in p.cleared.items.iter().zip(&p.left) {
        if m.read_u32(item + 0x10) == Some(left) {
            let _ = m.write_u32(item + 0x10, left | bit);
        }
    }
}

/// The census of the current layer, when it is one of the named ones.
/// `ret` is the layer draw's return address (`[esp]` at entry): for the
/// layers whose caller no lifted port names (18, 19 and 21) it is the
/// evidence the next slice needs.
fn census_at(mem: &dyn GuestMemory, ret: u32) {
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
    }).unwrap_or_default();
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
        "native-water: layer {layer}, view {n} (layer draw returns to {ret:#010x}): {} items ({} caller kinds, {} shaders){gate}\n  native-water:   shaders:{shaders}{callers}",
        t.items,
        t.callers.len(),
        t.shaders.len()
    );
}

/// The observer before the layer draw, on any guest memory: the last
/// layer 6's bits back, the census, then this layer's skip. `ret` is the
/// layer draw's return address (`[esp]` at entry).
pub fn before_layer(mem: &mut dyn GuestMemory, ret: u32) {
    restore_pending(mem);
    census_at(mem, ret);
    let mut m = crate::native_fog::Mem(mem);
    if m.read_u32(CURRENT_LAYER) != Some(WATER_LAYER) {
        return;
    }
    WATER_VIEWS.fetch_add(1, Ordering::Relaxed);
    if ret != SCENE_WATER_RETURN {
        let n = WATER_WRONG_CALLER.fetch_add(1, Ordering::Relaxed);
        if n % 1800 == 0 {
            eprintln!(
                "native-water: {} layer-6 calls from {ret:#010x}, not the late passes' ({:#010x}): left to the D3D path",
                n + 1,
                SCENE_WATER_RETURN
            );
        }
        return;
    }
    // The water gate (bit 4 of `0x004b9fe0`, the lifted port's
    // `0x0002c0ee`): set, the wrapper does not draw and neither does the
    // native pass. Unreadable, the D3D path.
    match m.read_u32(WATER_GATE) {
        Some(word) if word & 0x10 == 0 => {}
        Some(_) => {
            WATER_GATED.fetch_add(1, Ordering::Relaxed);
            return;
        }
        None => return,
    }
    // A layer 6 without a structure item draws no world water.
    if skip::has_structure_items(&m, WATER_LAYER) != Some(true) {
        return;
    }
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return;
    };
    let Some(cleared) = host.ask(&mut *m.0) else {
        return;
    };
    if cleared.items.is_empty() {
        return;
    }
    WATER_DRAWN.fetch_add(1, Ordering::Relaxed);
    WATER_ITEMS.fetch_add(cleared.items.len() as u64, Ordering::Relaxed);
    let views = WATER_VIEWS.load(Ordering::Relaxed);
    if views % 1800 == 1 {
        let (v, d, i, g, w) = counts();
        eprintln!("native-water: {d} of {v} layer-6 views drawn natively ({g} gated off, {w} other callers), {i} items skipped");
    }
    let left = cleared.items.iter().map(|&item| m.read_u32(item + 0x10).unwrap_or(0)).collect();
    *CLEARED.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pending { cleared, left });
}

pub(crate) fn forget_pending() {
    CLEARED.lock().unwrap_or_else(|e| e.into_inner()).take();
}

pub struct NativeWater;

impl Feature for NativeWater {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Native water edge blend and a census of water, camouflage and hologram draws")
            .doc("docs/graphics/native-renderer.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        if !on(b.switches()) {
            return Ok(());
        }
        b.raw()
            .observe_before(&points::LAYER_DRAW, "native_water_skip", |o| {
                if crate::native_map_work::Stage::Water.standing_down() {
                    return;
                }
                let mut g = o.guest();
                let ret = g.u32(o.cpu.esp).unwrap_or(0);
                before_layer(&mut *g, ret);
            })
            .after_feature(crate::render_timing::ID)
            .after_feature(crate::native_world::ID)
            .after_feature(crate::native_fog::ID)
            .context_only();
        b.snapshot(crate::SnapshotPolicy::Stateless);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Sparse guest words for the census tests.
    #[derive(Default)]
    struct Mem(HashMap<u32, u32>);

    impl Mem {
        fn w(&mut self, addr: u32, v: u32) {
            self.0.insert(addr, v);
        }
    }

    impl GuestRead for Mem {
        fn read_u32(&self, addr: u32) -> Option<u32> {
            self.0.get(&addr).copied()
        }
    }

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
        m.w(RENDER_RECORDS + 0x20 * 2, 0);
        m.w(RENDER_RECORDS + 0x20 * 2 + 0xc, 37 << 9);
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
