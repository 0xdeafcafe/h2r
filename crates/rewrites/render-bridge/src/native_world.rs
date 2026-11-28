//! `native-world`: the static world drawn by the native renderer instead
//! of the game's layer-3 entries (docs/graphics/native-world.md, stage
//! 1). On with the native renderer (`impl.render.native`, on by default;
//! `=off` returns the world to the D3D path with every other native
//! stage).
//!
//! One observer before the layer draw `LAYER_DRAW` (`0x00044550`), an
//! opaque point, so nothing here replaces guest code:
//!
//! - first it sets back the bits it cleared at the last layer 3 (the
//!   layers after 3 on the scene list, 5 and up, come next in the same
//!   view, so the bits are back before anything reads them again), each
//!   only if the item still holds the mask it left ([`restore_pending`]);
//! - then, when the current layer is 3 and its list holds a BSP item
//!   ([`native_render::skip::has_structure_items`]: not the first-person
//!   render's layer 3, models only): read how its items switch the
//!   stencil ([`native_render::skip::stencil_plan`]: the native pass
//!   draws a marked section's parts with the reference the game's mode 2
//!   writes), the render camera and the active BSP, ask the [`Host`]
//!   (boot's glue: the GPU world and the D3D device) for the native pass,
//!   and when it accepts, clear layer 3's bit
//!   on the BSP items whose part it draws, by cluster or placement and
//!   part index, with the part's shader ([`native_render::skip::clear_parts`]):
//!   the layer then builds no entry for them. A structure item the native
//!   pass does not draw keeps its guest draw and is counted by cause
//!   (`scene_kept_3_world_<cause>`) until the scene draws it: no wall is
//!   left undrawn.
//!
//! Nothing runs after the body, so the chain needs no Rust continuation,
//! and the observer is context-only: it reads and writes the memory its
//! context sees, and the host's pass request goes to the D3D device in
//! render order, as the render's own D3D calls do.
//!
//! It sits inside `render-timing`'s observers at the point
//! (`after_feature`), so its layer stage holds it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use guest_layout::memory::GuestMemory;
use guest_layout::points;
use frame_scene::View;
use native_render::camera::{GuestRead, GuestWrite};
use native_render::skip::{self, Cleared, LIGHTMAP_LAYER, StencilPlan};
use native_render::world::Owner;

use crate::native_map_work::{self, BSP_LOAD_PENDING, Stage};
use hooks::{BuildError, Builder, Feature, Manifest};

pub const ID: &str = "native-world";
/// Whether the native world is on: the native renderer's master switch.
pub fn on(_: &hooks::Switches) -> bool {
    true
}

mod layer_build;
pub(crate) use layer_build::counters as geometry_list_counters;
pub(crate) mod scene_build;
mod scene_walk;
mod section;
pub(crate) use section::display_cluster_transparents;
pub use section::{DISPLAY_LAYERS, display_cluster_complete, display_late_shader};

fn claimed_entries(layer: u32) -> bool {
    match layer {
        3 => has_claim(),
        6 => crate::native_water::has_claim(),
        16 => crate::native_fog::has_claim(),
        _ => false,
    }
}

/// The active structure BSP's tag data (docs/formats/structure-bsp.md).
pub const ACTIVE_BSP: u32 = 0x004e_0348;

/// The world parts the native world draws, by owner (cluster or
/// placement) and part index in its section, each with its material's
/// shader by tag index (the low word of the datum an item's `+8` holds).
pub type PartSet = HashMap<(Owner, u8), u32>;

/// What draws the world: boot's glue over the GPU world and the device.
pub trait Host: Send + Sync {
    /// Release the displaced BSP before any stage asks for new resources.
    fn retire_bsp(&self) {}

    /// Publish completed worker resources before the frame draws its views.
    fn begin_frame(&self) {}

    /// Starts the shared BSP load once the scene binds its world palettes.
    fn load_bsp(&self, _mem: &dyn GuestMemory, _bsp: u32) -> bool {
        false
    }

    /// Before layer 3 of a view (`mem` the guest, read only): `bsp` the active BSP's address, `view`
    /// the frame step's open view (its camera; [`crate::scene_step`]), `stencil` how the layer's items switch
    /// the stencil. Asks for the native pass when a world for `bsp` is
    /// ready and the device can draw it. When `claim_guest` is false
    /// only the extra display clusters can draw, with no guest masks.
    /// Returns the guest parts it draws, or why it draws none: the
    /// structure items then keep their guest draw, counted
    /// `scene_kept_3_world_<cause>` ([`crate::native_layers::count_kept`]).
    fn ask(&self, mem: &mut dyn GuestMemory, bsp: u32, view: &View, stencil: &StencilPlan, claim_guest: bool) -> Result<Cleared, &'static str>;
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
/// Bits cleared at the last layer 3, with each item's mask as it was left,
/// until the next layer draw sets them back. One render runs at a time
/// (on the render thread, or on the game thread serially or after a
/// fallback, which carries on the same render against live RAM), so the
/// next layer draw that reads this is the same render's.
static CLEARED: Mutex<Option<Pending>> = Mutex::new(None);

/// What [`before_layer`] cleared and the masks it left.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pending {
    pub cleared: Cleared,
    /// The mask each item of `cleared` was left with.
    pub left: Vec<u32>,
}
static VIEWS: AtomicU64 = AtomicU64::new(0);
static DRAWN: AtomicU64 = AtomicU64::new(0);
static ITEMS: AtomicU64 = AtomicU64::new(0);
static STENCIL: AtomicU64 = AtomicU64::new(0);
static WITHOUT_WORLD: AtomicU64 = AtomicU64::new(0);

/// Installs the host (boot). Without one nothing is skipped.
pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

/// Removes the host: layer 3 is left alone.
pub fn clear_host() {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Layer-3 views seen, views drawn natively, items skipped.
pub fn counts() -> (u64, u64, u64) {
    (
        VIEWS.load(Ordering::Relaxed),
        DRAWN.load(Ordering::Relaxed),
        ITEMS.load(Ordering::Relaxed),
    )
}

/// Layer-3 views left to the guest because the items' stencil switches do not read.
pub fn stencil_views() -> u64 {
    STENCIL.load(Ordering::Relaxed)
}

/// Layer-3 views whose list holds no BSP item (the first-person render's):
/// the host is not asked.
pub fn views_without_world() -> u64 {
    WITHOUT_WORLD.load(Ordering::Relaxed)
}

/// The guest's memory as `native_render` reads and writes it.
pub struct Mem<'a>(pub &'a mut dyn GuestMemory);

impl GuestRead for Mem<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

impl GuestWrite for Mem<'_> {
    fn write_u32(&mut self, addr: u32, value: u32) -> Option<()> {
        self.0.set_u32(addr, value).ok()
    }
}

/// The exact part sequence corresponding to the reversible mask edits, including duplicates.
pub fn plan_parts(mem: &dyn GuestRead, parts: &PartSet, keep: impl Fn(Owner, u8) -> bool)
    -> Option<(skip::MaskPlan, Cleared, Vec<(Owner, u8)>)>
{
    let mut preview = skip::MaskPreview::new(mem)?;
    let selected = std::cell::RefCell::new(Vec::new());
    let cleared = skip::clear_parts(&mut preview, LIGHTMAP_LAYER, &|owner, part, shader| {
        use timeline::native::{Metric, Stage};
        let accepted = keep(owner, part) && parts.get(&(owner, part)) == Some(&shader);
        if accepted { selected.borrow_mut().push((owner, part)); }
        else { Stage::World.count(Metric::Examined, 1); Stage::World.count(Metric::RefusedOwnership, 1); }
        accepted
    })?;
    Some((preview.finish()?, cleared, selected.into_inner()))
}

/// The observer before the layer draw, on any guest memory: the last layer
/// 3's bits back, then this layer's skip. A structure item the native pass
/// does not draw keeps its guest draw and is counted
/// `scene_kept_3_world_<cause>` ([`crate::native_layers::count_kept`]):
/// the host's cause, `part` for a part outside the drawn set, under the
/// site's prefix (`alt` for the alternate view).
pub fn before_layer(mem: &mut dyn GuestMemory, ret: u32) {
    restore_pending(mem);
    let m = Mem(mem);
    if m.read_u32(skip::CURRENT_LAYER) != Some(LIGHTMAP_LAYER) {
        return;
    }
    VIEWS.fetch_add(1, Ordering::Relaxed);
    let site = match native_render::world_layers::layer_site(LIGHTMAP_LAYER, ret) {
        native_render::world_layers::Site::AlternateView => "alt",
        _ => "",
    };
    if let Some(left) = claim_layer(mem, ret) {
        crate::native_layers::count_kept(&Mem(mem), LIGHTMAP_LAYER, site, false, &left, None);
    }
}

/// The world's layer 3 asks the host for the native pass. `Some` when the
/// view is the world's and its structure items are the scene's: why the
/// ones the host did not clear keep their guest draw.
fn claim_layer(mem: &mut dyn GuestMemory, ret: u32) -> Option<crate::native_layers::Left> {
    let m = Mem(mem);
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        crate::native_layers::leave(&m, LIGHTMAP_LAYER, crate::native_layers::Keep::Structure, "host");
        return None;
    };
    use native_render::world_layers::{Site, layer_site};
    let site = layer_site(LIGHTMAP_LAYER, ret);
    let bsp = m.read_u32(ACTIVE_BSP)?;
    let has_world = skip::has_structure_items(&m, LIGHTMAP_LAYER) == Some(true);
    if !has_world && site != Site::Scene {
        WITHOUT_WORLD.fetch_add(1, Ordering::Relaxed);
        return None;
    }
    let mut left = crate::native_layers::Left::default();
    let kind = if site == Site::AlternateView { frame_scene::ViewKind::Alternate } else { frame_scene::ViewKind::Game };
    let Some(mut view) = crate::scene_step::open_kind(&*m.0, kind) else {
        left.all("view");
        return has_world.then_some(left);
    };
    if site != Site::Scene { view.display_visibility = None; }
    let claim_guest = has_world;
    if !claim_guest && view.display_visibility.as_ref().is_none_or(|v| v.clusters.is_empty() && v.placements.is_empty()) {
        return None;
    }
    // How the layer's items switch the stencil (0x0004d640/0x0004d720):
    // unreadable, the items keep their guest draw.
    let Some(stencil) = skip::stencil_plan(&m, LIGHTMAP_LAYER) else {
        let n = STENCIL.fetch_add(1, Ordering::Relaxed);
        if n % 1800 == 0 {
            eprintln!("native-world: {} layer-3 views left to the guest: the items' stencil switches do not read", n + 1);
        }
        left.all("stencil_plan");
        return claim_guest.then_some(left);
    };
    match host.ask(m.0, bsp, &view, &stencil, claim_guest) {
        Ok(c) => {
            DRAWN.fetch_add(1, Ordering::Relaxed);
            ITEMS.fetch_add(c.items.len() as u64, Ordering::Relaxed);
            let views = VIEWS.load(Ordering::Relaxed);
            if views % 1800 == 1 {
                let (v, d, i) = counts();
                let w = views_without_world();
                eprintln!(
                    "native-world: {d} of {v} layer-3 views drawn natively ({w} without BSP items: first person), {i} world items skipped"
                );
            }
            let left_masks = c.items.iter().map(|&item| m.read_u32(item + 0x10).unwrap_or(0)).collect();
            *CLEARED.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pending { cleared: c, left: left_masks });
            // What the host did not clear: a part outside its drawn set.
            left.all("part");
        }
        Err(why) => left.all(why),
    }
    claim_guest.then_some(left)
}

pub(crate) fn has_claim() -> bool {
    CLEARED.lock().unwrap_or_else(|e| e.into_inner())
        .as_ref().is_some_and(|claim| !claim.cleared.items.is_empty())
}

/// Sets back the bits the last layer 3 cleared, on the items that still
/// hold the mask it left: an item list built again since (a new frame
/// whose render never reached another layer draw) is left alone.
pub fn restore_pending(mem: &mut dyn GuestMemory) {
    let Some(p) = CLEARED.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return;
    };
    let mut m = Mem(mem);
    let bit = 1u32 << p.cleared.layer;
    for (&item, &left) in p.cleared.items.iter().zip(&p.left) {
        if m.read_u32(item + 0x10) == Some(left) {
            let _ = m.write_u32(item + 0x10, left | bit);
        }
    }
}

/// Drops the pending restore without setting anything back: a snapshot
/// load, whose guest RAM is not the memory those bits were cleared in.
/// The loaded item lists are the loaded render's, built again each frame.
pub fn forget_pending() {
    CLEARED.lock().unwrap_or_else(|e| e.into_inner()).take();
}

pub(crate) fn retire_bsp() {
    let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(host) = host { host.retire_bsp(); }
}

pub(crate) fn begin_frame() {
    let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(host) = host { host.begin_frame(); }
}

pub(crate) fn load_bsp(mem: &dyn GuestMemory, esp: u32) {
    use native_render::world_layers::{Site, layer_site};
    if mem.u32(skip::CURRENT_LAYER).ok() != Some(LIGHTMAP_LAYER)
        || mem.u32(esp).ok().is_none_or(|ret| layer_site(LIGHTMAP_LAYER, ret) == Site::FirstPerson)
    {
        return;
    }
    load_active_bsp(mem);
}

/// Both a scene base layer and the dynamic light pass can be the first
/// consumer in a BSP. They share the activation's single pending load.
pub(crate) fn load_active_bsp(mem: &dyn GuestMemory) {
    if !BSP_LOAD_PENDING.load(Ordering::Acquire) {
        return;
    }
    let Ok(bsp) = mem.u32(ACTIVE_BSP) else {
        return;
    };
    if bsp == 0 || !BSP_LOAD_PENDING.swap(false, Ordering::AcqRel) {
        return;
    }
    let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let _dispatch = timeline::setup::dispatch();
    if !host.is_some_and(|host| host.load_bsp(mem, bsp)) {
        native_map_work::request_bsp_load();
    }
}

pub struct NativeWorld;

impl Feature for NativeWorld {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "The static world drawn by the native GPU renderer")
            .doc("docs/graphics/native-renderer.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        if !on(b.switches()) {
            b.snapshot(hooks::SnapshotPolicy::Stateless);
            return Ok(());
        }
        scene_walk::install(b);
        b.snapshot(hooks::SnapshotPolicy::section(Vec::new, |_, _| {
            forget_pending();
            Ok(())
        }));
        b.raw()
            .observe_before(&points::LAYER_DRAW, "native_world_skip", |o| {
                if Stage::World.standing_down() {
                    return;
                }
                let mut g = o.guest();
                let ret = g.u32(o.cpu.esp).unwrap_or(0);
                before_layer(&mut *g, ret);
            })
            .after_feature(crate::render_timing::ID)
            .after_feature(crate::scene_step::ID)
            .after_feature(native_map_work::ID)
            .context_only();
        Ok(())
    }
}
