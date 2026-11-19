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
    pub flags: u32,
    pub items: Vec<Item>,
    /// Hardware constant slots 40..49, including the word kept from the
    /// previous setup. Never reconstruct from the main view camera.
    pub projection: [u32; shadow_constants::BLOCK_WORDS],
}

pub trait Host: Send + Sync {
    fn begin(&self, mem: &mut dyn GuestMemory, layer: Layer);
    fn end(&self, mem: &mut dyn GuestMemory, site: Site, layer: u32);
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
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
    let items = if layer == 14 {
        Vec::new()
    } else {
        shadows::items(&Mem(mem), list, layer)?
    };
    Some(Layer {
        site,
        layer,
        target,
        flags,
        items,
        projection,
    })
}

pub struct NativeShadows;

impl Feature for NativeShadows {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Native shadow-map production and comparison")
            .doc("docs/graphics/native-shadows.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        if !on(b.switches()) || !comparing() {
            return Ok(());
        }
        b.raw().observe_before(&points::LAYER_DRAW, "shadow layer", |o| {
            let mut mem = o.guest();
            let Ok(layer) = mem.u32(skip::CURRENT_LAYER) else { return };
            let Some(site) = mem.u32(o.cpu.esp).ok().and_then(|ret| shadows::site(layer, ret)) else { return };
            let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if let Some(host) = host {
                if let Some(data) = capture(&mut *mem, layer, site) {
                    host.begin(&mut *mem, data);
                } else {
                    eprintln!("native-shadows {site:?} layer={layer} refused: list, target or projection");
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
