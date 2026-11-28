//! Layer boundaries for the native shadow maps, on with the native
//! renderer (`impl.render.native`, on by default). No observer mutates
//! guest state, including on a malformed or partially readable list.
//!
//! The object shadows (`0x000335e0`) are a cut (`crate::cut`): their layers
//! draw from the scene in [`SLOT`], before layer 7, at any render scale. A
//! group the host cannot draw is not handed to the guest's handlers; it is
//! `scene_dropped_shadow_layers_<cause>`.

use crate::native_world::Mem;
use engine::render::light_gather::{object_shadows, passes};
use guest_layout::{memory::GuestMemory, points};
use engine::shadow_constants;
use hooks::{BuildError, Builder, Feature, Manifest};
use native_render::{
    shadows::{self, Item, Site},
    skip,
};
use std::sync::{Arc, Mutex};

pub const ID: &str = "native-shadows";

/// The view slot the object shadows draw in: slot 3, then layer 7.
pub const SLOT: frame_scene::ViewSlot = frame_scene::ViewSlot::ShadowsAndLayer7;

pub fn on(_: &hooks::Switches) -> bool {
    true
}

#[derive(Debug)]
pub struct Layer {
    pub site: Site,
    pub layer: u32,
    pub target: u32,
    pub view: u32,
    pub flags: u32,
    pub items: Vec<Item>,
    /// Hardware constant slots 40..49, including the word kept from the
    /// previous setup. Never reconstruct from the main view camera.
    pub projection: [u32; shadow_constants::BLOCK_WORDS],
}

#[derive(Debug, Clone, Copy)]
pub struct Entry {
    pub index: u16,
    pub implementation: u8,
    pub shader: u32,
    pub argument: u32,
    pub definition_bit5: Option<bool>,
    pub int4: Option<bool>,
}

pub trait Host: Send + Sync {
    fn parent(&self, site: Option<Site>, identity: Option<[u32; 2]>);
    fn entry(&self, mem: &mut dyn GuestMemory, entry: Option<Entry>);
    fn begin(&self, mem: &mut dyn GuestMemory, layer: Layer);
    fn end(&self, mem: &mut dyn GuestMemory, site: Site, layer: u32);
    /// Replace the object filter's guest quads at its return.
    fn filter(
        &self,
        _mem: &mut dyn GuestMemory,
        _esp: u32,
        _filter: frame_scene::ShadowFilter,
    ) -> Result<(), &'static str> {
        Err("device")
    }
    /// Finish the filter's swallow at the call's return.
    fn finish_filter(&self, _mem: &mut dyn GuestMemory, _esp: u32) -> Result<(), &'static str> {
        Ok(())
    }
    /// The filter was refused before the host saw it: its guest quads run.
    /// A private chain hands its casters back to target 10 first.
    fn filter_kept(&self, _mem: &mut dyn GuestMemory) {}
    /// Draw a whole layer-10 group from the scene before its handlers.
    /// Refusal issues nothing and names the counter's category.
    fn draw_alone(
        &self,
        _mem: &mut dyn GuestMemory,
        _view: u32,
        _group: &crate::native_lights::SceneGroup,
    ) -> Result<(), &'static str> {
        Err("device")
    }
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

pub fn clear_host() {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

const DROP_CAUSES: [&str; 8] = [
    "remainder",
    "stream",
    "pose",
    "program",
    "state",
    "target",
    "callback",
    "device",
];
static ALONE: [std::sync::atomic::AtomicU64; 3] =
    [const { std::sync::atomic::AtomicU64::new(0) }; 3];
static DROPPED_BY: [std::sync::atomic::AtomicU64; DROP_CAUSES.len()] =
    [const { std::sync::atomic::AtomicU64::new(0) }; DROP_CAUSES.len()];

fn alone_index(layer: u32) -> Option<usize> {
    match layer {
        10 => Some(0),
        11 => Some(1),
        14 => Some(2),
        _ => None,
    }
}

/// Draw a shadow group from the scene and empty its guest entry loop. A
/// group the host cannot draw is dropped under its cause, never left to the
/// guest's handlers: the result is true for every shadow layer. False only
/// for a layer that is not a shadow layer.
pub fn draw_alone(
    mem: &mut dyn GuestMemory,
    layer: u32,
    group: &crate::native_lights::SceneGroup,
) -> bool {
    use std::sync::atomic::Ordering;
    let Some(index) = alone_index(layer) else {
        return false;
    };
    let view = mem
        .u32(engine::render_loop::occlusion::guest::VIEW_INDEX)
        .unwrap_or(u32::MAX);
    let result = HOST
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .ok_or("device")
        .and_then(|host| host.draw_alone(mem, view, group));
    match result {
        Ok(()) => {
            ALONE[index].fetch_add(1, Ordering::Relaxed);
            true
        }
        Err(why) => {
            let n = DROP_CAUSES
                .iter()
                .position(|&name| name == why)
                .unwrap_or(DROP_CAUSES.len() - 1);
            DROPPED_BY[n].fetch_add(1, Ordering::Relaxed);
            dropped_layer();
            true
        }
    }
}

/// Shadow layers committed from the scene, no geometry handler run.
pub fn drawn_alone(layer: u32) -> u64 {
    alone_index(layer).map_or(0, |index| {
        ALONE[index].load(std::sync::atomic::Ordering::Relaxed)
    })
}

/// `render.scene` counters for the shadow layers drawn and dropped, by cause.
pub fn alone_counters() -> Vec<(&'static str, u64)> {
    [
        ("scene_shadow_layers_alone_10", drawn_alone(10)),
        ("scene_shadow_layers_alone_11", drawn_alone(11)),
        ("scene_shadow_layers_alone_14", drawn_alone(14)),
    ]
    .into_iter()
    .chain(DROP_CAUSES.iter().zip(&DROPPED_BY).map(|(name, count)| {
        let name = match *name {
            "remainder" => "scene_dropped_shadow_layers_remainder",
            "stream" => "scene_dropped_shadow_layers_stream",
            "pose" => "scene_dropped_shadow_layers_pose",
            "program" => "scene_dropped_shadow_layers_program",
            "state" => "scene_dropped_shadow_layers_state",
            "target" => "scene_dropped_shadow_layers_target",
            "callback" => "scene_dropped_shadow_layers_callback",
            _ => "scene_dropped_shadow_layers_device",
        };
        (name, count.load(std::sync::atomic::Ordering::Relaxed))
    }))
    .chain(std::iter::once((
        "scene_dropped_shadow_layers",
        DROPPED.load(std::sync::atomic::Ordering::Relaxed),
    )))
    .collect()
}

static CHAINS_PRIVATE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static CHAINS_SPILLED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static DROPPED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// An object chain whose casters went into the stage's private pair
/// (native-shadow-layers.md, "A private chain").
pub fn chain_private() {
    CHAINS_PRIVATE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// A private chain whose filter stayed with the guest: its mask was copied
/// back into target 10 before the guest's quads.
pub fn chain_spilled() {
    CHAINS_SPILLED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// A shadow pass that could not be drawn, or that failed while recording on the render side.
/// Its generation draws no shadow. The caller logs the cause once.
pub fn dropped_layer() {
    DROPPED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// `render.scene` counters for private chains.
pub fn chain_counters() -> Vec<(&'static str, u64)> {
    use std::sync::atomic::Ordering;
    vec![
        (
            "scene_shadow_chains_private",
            CHAINS_PRIVATE.load(Ordering::Relaxed),
        ),
        (
            "scene_shadow_chains_spilled",
            CHAINS_SPILLED.load(Ordering::Relaxed),
        ),
    ]
}

const FILTER_REFUSALS: [&str; 5] = ["program", "state", "target", "callback", "device"];
static FILTERS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static FILTER_REFUSED: [std::sync::atomic::AtomicU64; FILTER_REFUSALS.len()] =
    [const { std::sync::atomic::AtomicU64::new(0) }; FILTER_REFUSALS.len()];

fn filter_refused(why: &'static str) {
    let n = FILTER_REFUSALS
        .iter()
        .position(|&name| name == why)
        .unwrap_or(FILTER_REFUSALS.len() - 1);
    FILTER_REFUSED[n].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// The filter wrapper pushes ten words and calls the blur at
/// `0x000222c2`. Its physical return site identifies that call even
/// after the pipelined renderer hands a native caster back to the game
/// thread. D3D scopes the swallow to the enclosing wrapper's after hook.
fn filter_entry(mem: &dyn GuestMemory, blur_esp: u32) -> Option<u32> {
    if mem.u32(blur_esp).ok()? != 0x0002_22c7 {
        return None;
    }
    blur_esp.checked_add(4 + 10 * 4)
}

/// Replace the current object shadow's filter with its scene's native
/// quads. Refusal leaves every guest quad in place.
pub fn filter(mem: &mut dyn GuestMemory, esp: u32, mxcsr: u32, x87_control: u16) -> bool {
    let Some(filter) = crate::scene_step::shadow_filter(mem, mxcsr, x87_control) else {
        filter_refused("program");
        if let Some(host) = host() {
            host.filter_kept(mem);
        }
        return false;
    };
    let result = HOST
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .ok_or("device")
        .and_then(|host| host.filter(mem, esp, filter));
    match result {
        Ok(()) => {
            FILTERS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            true
        }
        Err(why) => {
            filter_refused(why);
            false
        }
    }
}

/// Finish the filter's native swallow at the call's return.
pub fn finish_filter(mem: &mut dyn GuestMemory, esp: u32) {
    let result = HOST
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .ok_or("device")
        .and_then(|host| host.finish_filter(mem, esp));
    if let Err(why) = result {
        filter_refused(why);
    }
}

/// `render.scene` counters for native object filters and their refusals.
pub fn filter_counters() -> Vec<(&'static str, u64)> {
    std::iter::once((
        "scene_shadow_filters_native",
        FILTERS.load(std::sync::atomic::Ordering::Relaxed),
    ))
    .chain(
        FILTER_REFUSALS
            .iter()
            .zip(&FILTER_REFUSED)
            .map(|(name, count)| {
                let name = match *name {
                    "program" => "scene_dropped_shadow_filter_program",
                    "state" => "scene_dropped_shadow_filter_state",
                    "target" => "scene_dropped_shadow_filter_target",
                    "callback" => "scene_dropped_shadow_filter_callback",
                    _ => "scene_dropped_shadow_filter_device",
                };
                (name, count.load(std::sync::atomic::Ordering::Relaxed))
            }),
    )
    .collect()
}

fn capture(mem: &mut dyn GuestMemory, layer: u32, site: Site) -> Option<Layer> {
    let list = mem.u32(skip::CURRENT_LIST).ok()?;
    if list
        != if layer == 10 {
            shadows::CASTERS
        } else {
            shadows::RECEIVERS
        }
    {
        return None;
    }
    let target = mem.u32(shadows::CURRENT_TARGET).ok()?;
    let flags = mem.u32(shadow_constants::global::PASS_FLAGS).ok()?;
    if layer == 10 && target != if shadows::colour_mask(flags) { 10 } else { 9 } {
        return None;
    }
    let mut projection = [0; shadow_constants::BLOCK_WORDS];
    for (i, w) in projection.iter_mut().enumerate() {
        *w = mem
            .u32(shadow_constants::global::BLOCK + 4 * i as u32)
            .ok()?;
    }
    let items = shadows::items(&Mem(mem), list, layer)?;
    Some(Layer {
        site,
        layer,
        target,
        view: mem.u32(0x004b_a050).ok()?,
        flags,
        items,
        projection,
    })
}

fn read_entry(mem: &dyn GuestMemory, esp: u32) -> Option<Entry> {
    let sp = |off| esp.checked_add(off).and_then(|a| mem.u32(a).ok());
    let shader = sp(4)?;
    let p = sp(28)?;
    let def = mem.u32(0x0048_58c4).ok().filter(|&p| p != 0);
    let tag = mem
        .u32(0x004e_3b44)
        .ok()?
        .checked_add((shader & 0xffff) * 16)?;
    let shad = mem.u32(tag.checked_add(8)?).ok()?;
    Some(Entry {
        index: mem.u16(p).ok()?,
        implementation: mem.u8(p.checked_add(4)?).ok()?,
        argument: sp(24)?,
        shader,
        definition_bit5: def.and_then(|p| mem.u8(p).ok()).map(|v| v & 32 != 0),
        int4: shad
            .checked_add(0x3e)
            .and_then(|a| mem.u16(a).ok())
            .map(|v| v != 0),
    })
}
fn host() -> Option<Arc<dyn Host>> {
    HOST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}
/// The list calls the caster with two stack arguments below its three
/// saved registers. The ranked rows sit directly above those registers.
fn object_owner(mem: &dyn GuestMemory, esp: u32, cursor: u32, remaining: u32) -> Option<[u32; 2]> {
    if passes::caller(mem, esp)? != object_shadows::site::CASTER {
        return None;
    }
    let rows = esp.checked_add(4 + 8 + 12)?;
    shadows::object_candidate(rows, cursor, remaining)?;
    if mem.u8(0x0050_9413).ok()? != 1 || mem.u8(0x0048_5a46).ok()? != 1 {
        return None;
    }
    let identity = [mem.u32(cursor).ok()?, mem.u32(cursor.checked_add(4)?).ok()?];
    let args = [mem.u32(esp + 4).ok()?, mem.u32(esp + 8).ok()?];
    (identity == args).then_some(identity)
}

pub struct NativeShadows;

impl Feature for NativeShadows {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Native shadow-map production and comparison")
            .doc("docs/graphics/native-shadows.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        if !on(b.switches()) {
            return Ok(());
        }
        b.raw()
            .observe_before(&points::RENDER_LIGHT_PASSES, "shadow light owner", |o| {
                if let Some(h) = host() {
                    let m = o.guest();
                    let id = m
                        .u32(o.cpu.esp.wrapping_add(4))
                        .ok()
                        .map(|datum| [datum, o.cpu.eax]);
                    h.parent(Some(Site::Light), id);
                }
            })
            .context_only();
        b.raw()
            .observe_before(
                &points::ADDITIONAL_SHADOW_CASTERS,
                "additional shadow caster owner",
                |_| {
                    if let Some(h) = host() {
                        h.parent(Some(Site::AdditionalCaster), None);
                    }
                },
            )
            .context_only();
        b.raw()
            .observe_before(&points::OBJECT_SHADOW_LIST, "object shadow list", |_| {
                if let Some(h) = host() {
                    h.parent(None, None);
                }
            })
            .context_only();
        b.raw()
            .observe_before(&points::OBJECT_SHADOW, "object shadow owner", |o| {
                let m = o.guest();
                let identity = object_owner(&*m, o.cpu.esp, o.cpu.esi, o.cpu.edi);
                crate::scene_step::object_shadow(&*m, identity);
                if let Some(h) = host() {
                    h.parent(identity.map(|_| Site::Object), identity);
                }
            })
            .context_only();
        b.raw()
            .observe_before(&points::SCREEN_CONVOLUTION_BLUR, "object shadow filter", |o| {
                let mut mem = o.guest();
                if let Some(esp) = filter_entry(&*mem, o.cpu.esp) {
                    filter(&mut *mem, esp, o.cpu.mxcsr, o.cpu.x87.control);
                }
            })
            .context_only();
        b.raw().observe_after(
            &points::OBJECT_SHADOW_FILTER,
            "object shadow filter end",
            |o| {
                let mut mem = o.guest();
                // The wrapper's bare RET pops its return slot.
                finish_filter(&mut *mem, o.cpu.esp.wrapping_sub(4));
            },
        );
        b.raw()
            .observe_before(
                &points::LIGHT_GEOMETRY_DRAW,
                "sorted shadow geometry",
                |o| {
                    if !matches!(o.cpu.ebx, 10 | 11 | 14) {
                        return;
                    }
                    let mut m = o.guest();
                    let esp = o.cpu.esp;
                    if let Some(h) = host() {
                        let list = if o.cpu.ebx == 10 {
                            shadows::CASTERS
                        } else {
                            shadows::RECEIVERS
                        };
                        let sorted = m.u32(esp.wrapping_add(16)).ok() == Some(o.cpu.ebx)
                            && o.cpu.edi == list
                            && m.u32(skip::CURRENT_LIST).ok() == Some(list)
                            && m.u32(esp.wrapping_add(28)).ok() == Some(o.cpu.esi);
                        let entry = sorted.then(|| read_entry(&*m, esp)).flatten();
                        h.entry(&mut *m, entry);
                    }
                },
            )
            .context_only();
        b.raw()
            .observe_before(&points::LAYER_DRAW, "shadow layer", |o| {
                let mut mem = o.guest();
                let Ok(layer) = mem.u32(skip::CURRENT_LAYER) else {
                    return;
                };
                let Some(site) = passes::caller(&*mem, o.cpu.esp)
                    .and_then(|ret| shadows::site(layer, ret))
                else {
                    return;
                };
                let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
                if let Some(host) = host {
                    if let Some(data) = capture(&mut *mem, layer, site) {
                        host.begin(&mut *mem, data);
                    } else {
                        host.parent(None, None);
                    }
                }
            })
            .context_only();
        b.raw()
            .observe_before(&points::LAYER_END, "shadow layer end", |o| {
                let mut mem = o.guest();
                let Ok(layer) = mem.u32(skip::CURRENT_LAYER) else {
                    return;
                };
                // Both calls have a five-byte direct CALL instruction.
                let Some(site) = passes::caller(&*mem, o.cpu.esp)
                    .and_then(|ret| shadows::site(layer, ret.checked_sub(5)?))
                else {
                    return;
                };
                let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
                if let Some(host) = host {
                    host.end(&mut *mem, site, layer);
                }
            })
            .context_only();
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caster_owner_requires_a_live_row_in_its_own_frame() {
        let mut m = guest_layout::FlatMemory::new(0, 0x510000);
        let esp = 0x1000;
        let last = esp + 24 + 8 + 31 * 20;
        m.set_u32(esp, object_shadows::site::CASTER).unwrap();
        m.set_u8(0x0050_9413, 1).unwrap();
        m.set_u8(0x0048_5a46, 1).unwrap();
        for (offset, word) in [(0, 123), (4, 0.5f32.to_bits())] {
            m.set_u32(esp + 4 + offset, word).unwrap();
            m.set_u32(last + offset, word).unwrap();
        }
        assert_eq!(object_owner(&m, esp, last, 1), Some([123, 0.5f32.to_bits()]));
        assert_eq!(object_owner(&m, esp, last, 2), None);
        m.set_u32(last, 124).unwrap();
        assert_eq!(object_owner(&m, esp, last, 1), None);
    }

    #[test]
    fn only_the_wrappers_blur_scopes_a_filter_to_its_after_hook() {
        let mut mem = guest_layout::FlatMemory::new(0x1000, 0x100);
        let wrapper = 0x1080;
        let blur = wrapper - 44;
        mem.set_u32(wrapper, 0xbd00_0100).unwrap(); // synthetic after-hook token
        mem.set_u32(blur, 0x0002_22c7).unwrap();
        assert_eq!(filter_entry(&mem, blur), Some(wrapper));
        mem.set_u32(blur, 0x1004).unwrap();
        assert_eq!(filter_entry(&mem, blur), None);
    }

    struct Stub(Result<(), &'static str>);
    impl Host for Stub {
        fn parent(&self, _: Option<Site>, _: Option<[u32; 2]>) {}
        fn entry(&self, _: &mut dyn GuestMemory, _: Option<Entry>) {}
        fn begin(&self, _: &mut dyn GuestMemory, _: Layer) {}
        fn end(&self, _: &mut dyn GuestMemory, _: Site, _: u32) {}
        fn draw_alone(
            &self,
            _: &mut dyn GuestMemory,
            _: u32,
            _: &crate::native_lights::SceneGroup,
        ) -> Result<(), &'static str> {
            self.0
        }
    }

    fn group() -> crate::native_lights::SceneGroup {
        crate::native_lights::SceneGroup {
            group: frame_scene::LightGroup {
                light: 0,
                layer: 10,
                site: frame_scene::GroupSite::Light,
                projection: None,
                pass: None,
                entries: 0..0,
                remainder: 0,
            },
            entries: Vec::new(),
            rows: Vec::new(),
        }
    }

    /// The host is one global: tests that set it take turns.
    static HOST_TURN: Mutex<()> = Mutex::new(());

    #[test]
    fn a_whole_caster_group_commits_and_a_refusal_is_dropped_under_its_cause() {
        let _turn = HOST_TURN.lock().unwrap_or_else(|e| e.into_inner());
        let mut mem = guest_layout::FlatMemory::new(0, 0x1000);
        let group = group();
        set_host(Arc::new(Stub(Ok(()))));
        let before = drawn_alone(10);
        assert!(draw_alone(&mut mem, 10, &group));
        assert_eq!(drawn_alone(10), before + 1);
        assert!(draw_alone(&mut mem, 11, &group), "a receiver group");
        assert!(draw_alone(&mut mem, 14, &group), "an opacity group");
        assert!(!draw_alone(&mut mem, 12, &group), "not a shadow layer");
        assert_eq!(drawn_alone(10), before + 1);
        let stream = |rows: &[(&'static str, u64)]| counter(rows, "scene_dropped_shadow_layers_stream");
        let dropped = stream(&alone_counters());
        set_host(Arc::new(Stub(Err("stream"))));
        assert!(draw_alone(&mut mem, 10, &group), "the guest's handlers do not run");
        assert_eq!(drawn_alone(10), before + 1);
        assert!(stream(&alone_counters()) > dropped);
        assert!(
            alone_counters()
                .iter()
                .all(|(name, _)| !name.contains("shadow_alone_refused"))
        );
        clear_host();
        assert!(draw_alone(&mut mem, 10, &group), "no host is a `device` drop");
    }

    fn counter(rows: &[(&'static str, u64)], name: &str) -> u64 {
        rows.iter()
            .find(|(n, _)| *n == name)
            .map_or(u64::MAX, |r| r.1)
    }

    #[test]
    fn chain_and_drop_counters_are_rows_of_the_scene() {
        let private = counter(&chain_counters(), "scene_shadow_chains_private");
        let spilled = counter(&chain_counters(), "scene_shadow_chains_spilled");
        let dropped = counter(&alone_counters(), "scene_dropped_shadow_layers");
        chain_private();
        chain_private();
        chain_spilled();
        dropped_layer();
        // Other tests may count too: the rows rise by at least ours.
        assert!(counter(&chain_counters(), "scene_shadow_chains_private") >= private + 2);
        assert!(counter(&chain_counters(), "scene_shadow_chains_spilled") > spilled);
        assert!(counter(&alone_counters(), "scene_dropped_shadow_layers") > dropped);
    }

    struct Kept(std::sync::atomic::AtomicU32);
    impl Host for Kept {
        fn parent(&self, _: Option<Site>, _: Option<[u32; 2]>) {}
        fn entry(&self, _: &mut dyn GuestMemory, _: Option<Entry>) {}
        fn begin(&self, _: &mut dyn GuestMemory, _: Layer) {}
        fn end(&self, _: &mut dyn GuestMemory, _: Site, _: u32) {}
        fn filter_kept(&self, _: &mut dyn GuestMemory) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    #[test]
    fn a_filter_refused_before_the_host_tells_it_so_a_private_chain_can_spill() {
        let _turn = HOST_TURN.lock().unwrap_or_else(|e| e.into_inner());
        // No open frame step: the scene has no filter, so `program` refuses.
        let mut mem = guest_layout::FlatMemory::new(0, 0x1000);
        let host = Arc::new(Kept(std::sync::atomic::AtomicU32::new(0)));
        set_host(host.clone());
        assert!(!filter(&mut mem, 0x800, 0x1f80, 0x027f));
        assert_eq!(host.0.load(std::sync::atomic::Ordering::Relaxed), 1);
        clear_host();
    }
}
