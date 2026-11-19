//! Layer boundaries for the shadow-map differential. No observer mutates
//! guest state, including on a malformed or partially readable list.

use crate::{BuildError, Builder, Feature, Manifest, native_world::Mem};
use engine::{memory::GuestMemory, points, shadow_constants};
use native_render::{
    shadows::{self, Item, Site},
    skip,
};
use std::sync::{Arc, Mutex};

pub const SWITCH: &str = "impl.render.native_shadows";
pub const ID: &str = "native-shadows";

pub fn on(switches: &crate::Switches) -> bool {
    switches.feature(SWITCH, false)
}

pub fn comparing() -> bool {
    std::env::var("H2_NATIVE_SHADOWS_COMPARE").is_ok_and(|v| v == "1")
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
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

pub fn clear_host() {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = None;
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
const fn point(name: &'static str, addr: u32) -> points::Point {
    points::Point::new(
        name,
        addr,
        points::Sig::Opaque,
        "docs/graphics/native-shadows.md",
    )
}
static ADDITIONAL: points::Point = point("ADDITIONAL_SHADOW_CASTERS", 0x0003_0ed0);
static OBJECTS: points::Point = point("SCENE_CALL_000335e0", 0x0003_35e0);
static GATHER: points::Point = point("GATHER_OBJECT_SHADOWS", 0x0003_3400);
static OBJECT: points::Point = point("OBJECT_SHADOW", 0x0003_2350);
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
            .observe_before(&ADDITIONAL, "additional shadow caster owner", |_| {
                if let Some(h) = host() {
                    h.parent(Some(Site::AdditionalCaster), None);
                }
            })
            .context_only();
        b.raw()
            .observe_before(&OBJECTS, "object shadow list", |_| {
                OBJECT_LIST.set(None);
                if let Some(h) = host() {
                    h.parent(None, None);
                }
            })
            .context_only();
        b.raw()
            .observe_before(&GATHER, "object shadow candidates", |o| {
                let m = o.guest();
                let esp = o.cpu.esp;
                OBJECT_LIST.set(
                    (m.u32(esp).ok() == Some(0x0003_3632))
                        .then(|| m.u32(esp.wrapping_add(4)).ok())
                        .flatten(),
                );
            })
            .context_only();
        b.raw().observe_before(&OBJECT, "object shadow owner", |o| {
            let m = o.guest();
            let esp = o.cpu.esp;
            let address = o.cpu.esi;
            let args = [m.u32(esp.wrapping_add(4)).ok(), m.u32(esp.wrapping_add(8)).ok()];
            let index = OBJECT_LIST.get().and_then(|base| shadows::object_candidate(base, address, o.cpu.edi));
            let matched = index.is_some() && m.u32(esp).ok() == Some(0x0003_364c)
                && m.u8(0x0050_9413).ok() == Some(1) && m.u8(0x0048_5a46).ok() == Some(1)
                && args.iter().all(Option::is_some) && args == [m.u32(address).ok(), m.u32(address.wrapping_add(4)).ok()];
            if comparing() { eprintln!("native-shadows object candidate={index:?} remaining={} args={args:x?} chain_match={matched}", o.cpu.edi); }
            if let Some(h) = host() { h.parent(matched.then_some(Site::Object), args[0].zip(args[1]).map(|(a,b)| [a,b])); }
        }).context_only();
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
        b.raw().observe_before(&points::LAYER_DRAW, "shadow layer", |o| {
            let mut mem = o.guest();
            let Ok(layer) = mem.u32(skip::CURRENT_LAYER) else { return };
            let Some(site) = mem.u32(o.cpu.esp).ok().and_then(|ret| shadows::site(layer, ret)) else { return };
            let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if let Some(host) = host {
                if let Some(data) = capture(&mut *mem, layer, site) {
                    host.begin(&mut *mem, data);
                } else {
                    host.parent(None, None);
                    if comparing() { eprintln!("native-shadows {site:?} layer={layer} refused: list, target or projection"); }
                }
            }
        }).context_only();
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
                let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
                if let Some(host) = host {
                    host.end(&mut *mem, site, layer);
                }
            })
            .context_only();
        b.snapshot(crate::SnapshotPolicy::Stateless);
        Ok(())
    }
}
