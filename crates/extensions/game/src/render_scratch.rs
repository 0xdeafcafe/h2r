//! The render's scratch off the arena (docs/engine/sim-render-split.md,
//! "6. Render scratch and stamps", slice 8). The frame step writes three
//! kinds of render-only bytes into the game-state arena: the
//! render-state cache at `[0x00509434]` (the object setup's lighting
//! records, `0x0003ddd0` and its native port
//! `engine::render::object_lighting`), each object's handle into it
//! at body `+0xcc`, and the light records' render fields (the re-pose's
//! `+0x84..+0x9c` and `+0xac..+0xc4`, and `+0xf8..+0x110`, which
//! `0x000c12d0` and `0x00031660` write).
//!
//! While a world frame renders, each of those fields holds the render's
//! own copy, and the tick's bytes are put back when the frame ends: every
//! writer inside the frame step, guest body or Rust port, runs against the
//! render's copy, and the arena leaves the frame step as it entered it.
//! The copies are the render-private store's (host memory: no checkpoint,
//! film, keyframe or hash holds them).
//!
//! The tick still owns the fields between frames: the effect update calls
//! `0x0003ddd0` from the tick (`engine::effect_update::alive`, step 5), an
//! object's creation sets its `+0xcc`, a light's creation its record. A
//! field whose tick bytes are not the ones the last frame put back was
//! written by the tick since, and the frame starts that field from the
//! tick's bytes. So a datum reused, a map loaded or a revert drops the
//! render's copy of what it changed, and nothing else.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use engine::datum::layout;
use engine::{GuestMemory, points};
use hooks::{Builder, Feature, Flow, Manifest};

pub const ID: &str = "render-scratch";

/// The render-state cache's datum array pointer (`scene_extract::layout::
/// objects::RENDER_STATE_CACHE`) and its record size.
pub const RENDER_STATES: u32 = 0x0050_9434;
const RENDER_STATE_SIZE: u32 = 0x100;
/// The lights array's pointer (`sim_census`, `0xbfffb` stores it) and its
/// record size.
pub const LIGHTS: u32 = 0x004e_030c;
const LIGHT_SIZE: u32 = 0x110;
/// A light record's render fields: `(offset, len)`.
pub const LIGHT_FIELDS: [(u32, u32); 3] = [(0x84, 0x18), (0xac, 0x18), (0xf8, 0x18)];
/// The objects array's pointer, its 12-byte headers (salt `+0`, body `+8`)
/// and the body's render-state handle.
pub const OBJECTS: u32 = 0x004e_0300;
const OBJECT_HEADER_SIZE: u32 = 0xc;
const OBJECT_BODY: u32 = 8;
pub const OBJECT_RENDER_STATE: u32 = 0xcc;
/// No real array is larger.
const MAX_COUNT: u32 = 0x1_0000;

/// One datum array's header, when it reads as an array of `size`-byte
/// elements.
struct Array {
    header: u32,
    count: u32,
    high_water: u32,
    data: u32,
    bitmap: u32,
}

fn array(m: &dyn GuestMemory, pointer: u32, size: u32) -> Option<Array> {
    let header = m.u32(pointer).ok().filter(|&h| h != 0)?;
    let field = |off| m.u32(header.wrapping_add(off)).ok();
    if field(layout::SIGNATURE)? != engine::datum::SIGNATURE
        || field(layout::ELEMENT_SIZE)? != size
    {
        return None;
    }
    let count = field(layout::MAXIMUM_COUNT)?;
    let high_water = field(layout::HIGH_WATER)?;
    let data = field(layout::DATA)?;
    if count > MAX_COUNT || high_water > count || data == 0 {
        return None;
    }
    Some(Array { header, count, high_water, data, bitmap: field(layout::BITMAP)? })
}

/// The render-private fields of the arena this frame, `(address, len)`:
/// the render-state cache whole (header, bitmap, every record), each
/// light record's render fields, and each live object's `+0xcc`.
pub fn fields(m: &dyn GuestMemory) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    if let Some(a) = array(m, RENDER_STATES, RENDER_STATE_SIZE) {
        out.push((a.header, layout::HEADER_SIZE));
        if a.bitmap != 0 {
            out.push((a.bitmap, a.count.div_ceil(32) * 4));
        }
        out.extend((0..a.count).map(|i| (a.data + i * RENDER_STATE_SIZE, RENDER_STATE_SIZE)));
    }
    if let Some(a) = array(m, LIGHTS, LIGHT_SIZE) {
        for i in 0..a.count {
            let record = a.data + i * LIGHT_SIZE;
            out.extend(LIGHT_FIELDS.iter().map(|&(off, len)| (record + off, len)));
        }
    }
    if let Some(a) = array(m, OBJECTS, OBJECT_HEADER_SIZE) {
        for i in 0..a.high_water {
            let entry = a.data + i * OBJECT_HEADER_SIZE;
            if m.u16(entry).ok().is_some_and(|salt| salt != 0)
                && let Ok(body) = m.u32(entry + OBJECT_BODY)
                && body != 0
            {
                out.push((body + OBJECT_RENDER_STATE, 4));
            }
        }
    }
    out
}

/// One field's two copies: the tick's bytes the last frame put back, and
/// the render's as that frame left them.
struct Copies {
    tick: Vec<u8>,
    render: Vec<u8>,
}

/// The render-private store of the arena's render scratch.
#[derive(Default)]
pub struct Store {
    copies: HashMap<u32, Copies>,
    /// Inside a frame: each field and the tick's bytes to put back.
    entered: Option<Vec<(u32, Vec<u8>)>>,
}

impl Store {
    /// Put the render's copy in every field the tick has not written since
    /// the last frame, keeping the tick's bytes for [`Store::leave`].
    /// Entered already: nothing.
    pub fn enter(&mut self, m: &mut dyn GuestMemory) {
        if self.entered.is_some() {
            return;
        }
        let mut entered = Vec::new();
        for (addr, len) in fields(m) {
            let mut tick = vec![0; len as usize];
            if m.read(addr, &mut tick).is_err() {
                continue;
            }
            if let Some(c) = self.copies.get(&addr)
                && c.tick == tick
            {
                let _ = m.write(addr, &c.render);
            }
            entered.push((addr, tick));
        }
        // A field gone from the arena (its object deleted) is dropped.
        let live: std::collections::HashSet<u32> = entered.iter().map(|(a, _)| *a).collect();
        self.copies.retain(|a, _| live.contains(a));
        self.entered = Some(entered);
    }

    /// Keep the render's bytes and put the tick's back. Not entered:
    /// nothing.
    pub fn leave(&mut self, m: &mut dyn GuestMemory) {
        let Some(entered) = self.entered.take() else {
            return;
        };
        for (addr, tick) in entered {
            let mut render = vec![0; tick.len()];
            if m.read(addr, &mut render).is_err() {
                continue;
            }
            let _ = m.write(addr, &tick);
            self.copies.insert(addr, Copies { tick, render });
        }
    }

    /// Fields the store holds a render copy of.
    pub fn len(&self) -> usize {
        self.copies.len()
    }

    pub fn is_empty(&self) -> bool {
        self.copies.is_empty()
    }

    /// Drop every copy (a game's end).
    pub fn clear(&mut self) {
        self.copies.clear();
        self.entered = None;
    }
}

/// Every world frame renders on the store's copies. Registered by boot for
/// every session that renders, beside the render's random stream.
#[derive(Default)]
pub struct RenderScratch(Arc<Mutex<Store>>);

impl Feature for RenderScratch {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "The render's scratch in a store of its own, off the arena")
            .doc("docs/engine/sim-render-split.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), hooks::BuildError> {
        // Between frames the arena holds the tick's bytes; the copies only
        // ever reach drawing, and a field the tick changed starts over.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        let store = Arc::clone(&self.0);
        b.raw()
            .before(&points::RENDER_FRAME, "enter the render's scratch", move |call| {
                let mut guest = call.ctx.memory();
                if crate::render_rng::private_frame(&*guest) {
                    store.lock().unwrap_or_else(|e| e.into_inner()).enter(&mut *guest);
                }
                Ok(Flow::Continue)
            })
            .first();
        let store = Arc::clone(&self.0);
        b.raw()
            .after(&points::RENDER_FRAME, "put the tick's scratch back", move |call| {
                store.lock().unwrap_or_else(|e| e.into_inner()).leave(&mut *call.ctx.memory());
                Ok(())
            })
            .last();
        let store = Arc::clone(&self.0);
        b.game()
            .on_end("render scratch: drop the copies", move |_| {
                store.lock().unwrap_or_else(|e| e.into_inner()).clear();
            })
            .optional();
        Ok(())
    }
}

pub fn install(registry: &mut hooks::Registry) -> anyhow::Result<()> {
    registry
        .add(&RenderScratch::default())
        .map_err(|e| anyhow::anyhow!("render scratch: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests;
