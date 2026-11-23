//! Layer boundaries for the native shadow maps, on with the native
//! renderer (`impl.render.native`, on by default). No observer mutates
//! guest state, including on a malformed or partially readable list.

use crate::native_world::Mem;
use engine::{memory::GuestMemory, points, shadow_constants};
use hooks::{BuildError, Builder, Feature, Manifest};
use native_render::{
    shadows::{self, Item, Site},
    skip,
};
use std::sync::{Arc, Mutex};

pub const ID: &str = "native-shadows";

pub fn on(switches: &hooks::Switches) -> bool {
    hooks::switches::native_render_on(switches)
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

const REFUSALS: [&str; 8] = [
    "remainder",
    "stream",
    "pose",
    "program",
    "state",
    "target",
    "callback",
    "device",
];
static ALONE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static REFUSED: [std::sync::atomic::AtomicU64; REFUSALS.len()] =
    [const { std::sync::atomic::AtomicU64::new(0) }; REFUSALS.len()];

/// Draw a caster group from the scene and empty its guest entry loop on
/// success. Every refusal leaves the gathered/resolved-draw path intact.
pub fn draw_alone(
    mem: &mut dyn GuestMemory,
    layer: u32,
    group: &crate::native_lights::SceneGroup,
) -> bool {
    use std::sync::atomic::Ordering;
    if layer != 10 {
        return false;
    }
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
            ALONE.fetch_add(1, Ordering::Relaxed);
            true
        }
        Err(why) => {
            let n = REFUSALS
                .iter()
                .position(|&name| name == why)
                .unwrap_or(REFUSALS.len() - 1);
            REFUSED[n].fetch_add(1, Ordering::Relaxed);
            false
        }
    }
}

/// Caster layers committed from the scene, no geometry handler run.
pub fn drawn_alone() -> u64 {
    ALONE.load(std::sync::atomic::Ordering::Relaxed)
}

/// `render.scene` counters for scene-caster refusals by category.
pub fn alone_counters() -> Vec<(&'static str, u64)> {
    std::iter::once(("scene_shadow_layers_alone_10", drawn_alone()))
        .chain(REFUSALS.iter().zip(&REFUSED).map(|(name, count)| {
            let name = match *name {
                "remainder" => "scene_shadow_alone_refused_remainder",
                "stream" => "scene_shadow_alone_refused_stream",
                "pose" => "scene_shadow_alone_refused_pose",
                "program" => "scene_shadow_alone_refused_program",
                "state" => "scene_shadow_alone_refused_state",
                "target" => "scene_shadow_alone_refused_target",
                "callback" => "scene_shadow_alone_refused_callback",
                _ => "scene_shadow_alone_refused_device",
            };
            (name, count.load(std::sync::atomic::Ordering::Relaxed))
        }))
        .chain(std::iter::once(("scene_dropped_shadow_layers", 0)))
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
thread_local! { static OBJECT_LIST: std::cell::Cell<Option<u32>> = const { std::cell::Cell::new(None) }; }

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
                OBJECT_LIST.set(None);
                if let Some(h) = host() {
                    h.parent(None, None);
                }
            })
            .context_only();
        b.raw()
            .observe_before(
                &points::GATHER_OBJECT_SHADOWS,
                "object shadow candidates",
                |o| {
                    let m = o.guest();
                    let esp = o.cpu.esp;
                    OBJECT_LIST.set(
                        (m.u32(esp).ok() == Some(0x0003_3632))
                            .then(|| m.u32(esp.wrapping_add(4)).ok())
                            .flatten(),
                    );
                },
            )
            .context_only();
        b.raw()
            .observe_before(&points::OBJECT_SHADOW, "object shadow owner", |o| {
                let m = o.guest();
                let esp = o.cpu.esp;
                let address = o.cpu.esi;
                let args = [
                    m.u32(esp.wrapping_add(4)).ok(),
                    m.u32(esp.wrapping_add(8)).ok(),
                ];
                let index = OBJECT_LIST
                    .get()
                    .and_then(|base| shadows::object_candidate(base, address, o.cpu.edi));
                let matched = index.is_some()
                    && m.u32(esp).ok() == Some(0x0003_364c)
                    && m.u8(0x0050_9413).ok() == Some(1)
                    && m.u8(0x0048_5a46).ok() == Some(1)
                    && args.iter().all(Option::is_some)
                    && args == [m.u32(address).ok(), m.u32(address.wrapping_add(4)).ok()];
                let identity = args[0].zip(args[1]).map(|(a, b)| [a, b]);
                crate::scene_step::object_shadow(&*m, identity.filter(|_| matched));
                if let Some(h) = host() {
                    h.parent(matched.then_some(Site::Object), identity);
                }
            })
            .context_only();
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
                let Some(site) = mem
                    .u32(o.cpu.esp)
                    .ok()
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
                let Some(site) = mem
                    .u32(o.cpu.esp)
                    .ok()
                    .and_then(|ret| shadows::site(layer, ret.checked_sub(5)?))
                else {
                    return;
                };
                if site == Site::Object && layer == 11 {
                    crate::scene_step::object_shadow_end();
                }
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

    #[test]
    fn a_whole_caster_group_commits_and_a_refusal_keeps_its_handlers_and_counts() {
        let mut mem = engine::FlatMemory::new(0, 0x1000);
        let group = group();
        set_host(Arc::new(Stub(Ok(()))));
        let before = drawn_alone();
        assert!(draw_alone(&mut mem, 10, &group));
        assert_eq!(drawn_alone(), before + 1);
        assert!(!draw_alone(&mut mem, 11, &group), "only caster layer 10");
        assert_eq!(drawn_alone(), before + 1);
        set_host(Arc::new(Stub(Err("stream"))));
        assert!(!draw_alone(&mut mem, 10, &group));
        assert_eq!(drawn_alone(), before + 1);
        let counters = alone_counters();
        assert!(
            counters
                .iter()
                .any(|(name, count)| *name == "scene_shadow_alone_refused_stream" && *count > 0)
        );
        clear_host();
        assert!(!draw_alone(&mut mem, 10, &group));
    }
}
