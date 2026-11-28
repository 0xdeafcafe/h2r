//! `native-water`: the water edge blend, layer 6, drawn by the native
//! renderer instead of the game's items (docs/graphics/native-water-fog.md,
//! "Stage 4b: water and the rest"). On with the native renderer
//! (`impl.render.native`, on by default).
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
//! draw. The setup wrapper `0x0003ba90` before is never skipped: the
//! native draws land after it. The water end `0x0003c3c0` after is Rust
//! ([`crate::water_end`]): its gates return, its ocean is
//! [`crate::water_end::ocean`].
//!
//! Every item the guest's layer draw still draws after the decision keeps
//! its guest draw and is counted `scene_kept_6_<kind>_<cause>`, as the
//! fog's are (`crate::native_fog::count_late_kept`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use engine::points;
use native_render::camera::{GuestRead, GuestWrite};
use native_render::skip::{self, Cleared, CURRENT_LAYER};

use frame_scene::transparent::{DisplayTransparentPart, DisplayTransparentQueue};
use hooks::{BuildError, Builder, Feature, Manifest};

pub const ID: &str = "native-water";
/// Whether the native water is on.
pub fn on(switches: &hooks::Switches) -> bool {
    hooks::switches::native_render_on(switches)
}

/// Bit 4 of `0x004b9fe0`: set, `0x0003c3c0(0, 0)` runs instead of the
/// water setup and layer 6 does not draw (the lifted port's `0x0002c0ee`).
pub const WATER_GATE: u32 = 0x004b_9fe0;
/// The late passes' layer-6 call: the layer draw's return address
/// (`[esp]` at its entry, the lifted port's `0x0002c11b` call). Other
/// callers of the layer draw with layer 6 current are not the scene's
/// water and stay on the D3D path.
pub use scene_extract::late::SCENE_WATER_RETURN;
/// The layer the skip draws.
pub const WATER_LAYER: u32 = 6;

/// What draws the water edge blend: boot's glue over the GPU world and
/// the device. As the fog's ([`crate::native_fog::Host`]): plans the
/// skip over a preview, reserves the replacement (recording on every
/// copy before anything is surrendered), applies the mask edits and
/// commits; the setup wrapper `0x0003ba90` is never skipped, so the
/// replacement lands after it and before the water end.
pub trait Host: Send + Sync {
    /// `left` takes why the items it does not clear keep their guest draw.
    fn ask(&self, mem: &mut dyn GuestMemory, display_clusters: &[u16], left: &mut crate::native_layers::Left) -> Option<Cleared>;

    /// One run of the layer's sorted entries, as the fog's
    /// (`crate::native_fog::Host::ask_run`).
    fn ask_run(&self, mem: &mut dyn GuestMemory, at: &crate::late_sorted::RunAt<'_>, left: &mut crate::native_layers::Left) -> crate::late_sorted::Run;

    /// Whether the display pass draws this part of the sorted display
    /// queue: a water part the native set holds, whose live shader is the
    /// queue's. Default: none.
    fn draws_display_part(&self, _mem: &dyn GuestMemory, _part: &DisplayTransparentPart) -> bool {
        false
    }

    /// Draws `parts` (cluster, part) in order on the display copy, one
    /// display pass for the run, from inside the sorted dispatch. Returns
    /// how many it drew: 0 when it refused, with nothing drawn. The
    /// reference view is untouched. Default: refuses.
    fn draw_display_parts(&self, _mem: &mut dyn GuestMemory, _parts: &[PartKey]) -> usize {
        0
    }

    /// The parts the last [`ask`](Self::ask) drew on the display copy at
    /// layer 6, taken: the queue's runs do not draw them again.
    fn take_layer6_display(&self) -> Vec<PartKey> {
        Vec::new()
    }
}

mod display_runs;
pub use display_runs::{PartKey, Plan};

/// The queue's head run, at `TRANSPARENT_ORDER_READY` once the queue is
/// built (`esp` the dispatch's stack pointer). Without a host nothing is
/// planned.
pub(crate) fn begin_display_runs(mem: &mut dyn GuestMemory, queue: &DisplayTransparentQueue, esp: u32) {
    let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let Some(host) = host else { return };
    let already = host.take_layer6_display();
    display_runs::begin(mem, &*host, queue, esp, &already);
}

/// After the dispatch loop's entry `ordinal` returned or was skipped
/// (`TRANSPARENT_DISPATCH_NEXT`).
pub(crate) fn display_runs_after_entry(mem: &mut dyn GuestMemory, esp: u32, ordinal: u32) {
    let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let Some(host) = host else { return };
    display_runs::after_entry(mem, &*host, esp, ordinal);
}

/// The dispatch loop's exit (`SCENE_TRANSPARENTS_END`).
pub(crate) fn end_display_runs() {
    display_runs::end();
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
/// Layer 6 held for its sorted entries (`crate::late_sorted`).
static SORTED: Mutex<Option<crate::late_sorted::Deferred>> = Mutex::new(None);
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

pub(crate) fn has_claim() -> bool {
    CLEARED.lock().unwrap_or_else(|e| e.into_inner())
        .as_ref().is_some_and(|claim| !claim.cleared.items.is_empty())
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

/// The observer before the layer draw, on any guest memory: the last
/// layer 6's bits back, then this layer's skip. `ret` is the
/// layer draw's return address (`[esp]` at entry).
pub fn before_layer(mem: &mut dyn GuestMemory, ret: u32) {
    restore_pending(mem);
    crate::late_sorted::stale(&SORTED, &*mem);
    if crate::native_fog::Mem(&mut *mem).read_u32(CURRENT_LAYER) != Some(WATER_LAYER) {
        return;
    }
    WATER_VIEWS.fetch_add(1, Ordering::Relaxed);
    crate::native_layers::counts_own(&crate::native_fog::Mem(&mut *mem), WATER_LAYER);
    let mut left = crate::native_layers::Left::default();
    if claim(mem, ret, &mut left) {
        crate::native_fog::count_late_kept(&crate::native_fog::Mem(&mut *mem), WATER_LAYER, &left);
    }
}

/// The skip of one layer-6 call. `false` when the layer draws nothing (the
/// water gate set); else what it did not clear keeps its guest draw under
/// the cause named in `left`.
fn claim(mem: &mut dyn GuestMemory, ret: u32, left: &mut crate::native_layers::Left) -> bool {
    let m = crate::native_fog::Mem(mem);
    if ret != SCENE_WATER_RETURN {
        let n = WATER_WRONG_CALLER.fetch_add(1, Ordering::Relaxed);
        if n % 1800 == 0 {
            eprintln!(
                "native-water: {} layer-6 calls from {ret:#010x}, not the late passes' ({:#010x}): left to the D3D path",
                n + 1,
                SCENE_WATER_RETURN
            );
        }
        left.all(crate::native_layers::CALLER_CAUSE);
        return true;
    }
    // The water gate (bit 4 of `0x004b9fe0`, the lifted port's
    // `0x0002c0ee`): set, the wrapper does not draw and neither does the
    // native pass. Unreadable, the D3D path.
    match m.read_u32(WATER_GATE) {
        Some(word) if word & 0x10 == 0 => {}
        Some(_) => {
            WATER_GATED.fetch_add(1, Ordering::Relaxed);
            crate::scene_step::late_kept(&*m.0, WATER_LAYER as u8, false);
            return false;
        }
        None => {
            left.all("gate");
            return true;
        }
    }
    // The widened portal flood can reach water even when the retail list is empty.
    let display = crate::scene_step::late_display_visibility(&*m.0, WATER_LAYER as u8);
    let clusters = display.as_ref().map_or(&[][..], |extra| extra.whole_clusters.as_slice());
    if clusters.is_empty() && skip::has_structure_items(&m, WATER_LAYER) != Some(true) {
        crate::scene_step::late_kept(&*m.0, WATER_LAYER as u8, true);
        left.all("object");
        return true;
    }
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        left.all("host");
        return true;
    };
    let Some(cleared) = host.ask(&mut *m.0, clusters, left) else {
        // Items beside a kept one: drawn per item over the sorted entries.
        if crate::late_sorted::wants(left) && crate::late_sorted::defer(&SORTED, &m, WATER_LAYER, left) {
            return false;
        }
        crate::scene_step::late_kept(&*m.0, WATER_LAYER as u8, true);
        return true;
    };
    if cleared.items.is_empty() {
        return true;
    }
    WATER_DRAWN.fetch_add(1, Ordering::Relaxed);
    WATER_ITEMS.fetch_add(cleared.items.len() as u64, Ordering::Relaxed);
    let views = WATER_VIEWS.load(Ordering::Relaxed);
    if views % 1800 == 1 {
        let (v, d, i, g, w) = counts();
        eprintln!("native-water: {d} of {v} layer-6 views drawn natively ({g} gated off, {w} other callers), {i} items skipped");
    }
    let kept = cleared.items.iter().map(|&item| m.read_u32(item + 0x10).unwrap_or(0)).collect();
    *CLEARED.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pending { cleared, left: kept });
    true
}

pub(crate) fn forget_pending() {
    CLEARED.lock().unwrap_or_else(|e| e.into_inner()).take();
    crate::late_sorted::forget(&SORTED);
}

/// The host's ask for a run of layer 6's sorted entries.
fn ask_run(mem: &mut dyn GuestMemory, at: &crate::late_sorted::RunAt<'_>, left: &mut crate::native_layers::Left) -> crate::late_sorted::Run {
    match HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        Some(host) => host.ask_run(mem, at, left),
        None => {
            left.all("host");
            crate::late_sorted::Run::Refused
        }
    }
}

pub struct NativeWater;

impl Feature for NativeWater {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Native water edge blend")
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
        // Layer 6 per item, as the fog's (`crate::late_sorted`).
        b.raw()
            .observe_before(&points::scene_effects::LAYER_BUILT, "native_water_sorted", |o| {
                let mut g = o.guest();
                crate::late_sorted::at_built(&SORTED, &mut *g, o.cpu.esp, o.cpu.eax, &mut ask_run);
            })
            .after_feature(crate::native_world::ID)
            .context_only();
        b.raw()
            .observe_before(&points::scene_effects::LAYER_ENTRY_DONE, "native_water_entry", |o| {
                let mut g = o.guest();
                crate::late_sorted::after_entry(&SORTED, &mut *g, o.cpu.esi, &mut ask_run);
            })
            .context_only();
        // The teardown after it: its gates and its ocean's steps in Rust
        // (docs/graphics/retire-d3d-display.md, "The water end").
        crate::water_end::place(b);
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

