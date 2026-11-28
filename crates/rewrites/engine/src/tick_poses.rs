//! Node matrices at tick end (docs/engine/sim-render-split.md, "2. Node
//! matrices", slice 4). After the tick's phases, every object whose body
//! still carries the dirty-pose bit (flags bit 30) without the hold bit
//! (bit 29) has its shared node matrices built by `0x000bd090`, the tick's
//! own build (`sim_objects/pose.rs` when the native walk holds it), which
//! clears bit 30. The object setup then finds no dirty pose, so its pose
//! request (`0x000bd610`) never evaluates at `0x000bd7b2`, and marker
//! lookup (`sim_native/marker.rs`) reads the same matrices on a host, in a
//! 640x480 window and in a 32:9 one.
//!
//! Order: object index order, an object's parent chain (body `+0x14`)
//! before the object, so an attached object composes with a parent built
//! this tick. Contract D (docs/engine/sim-rewrite.md,
//! section 3): retail has no camera-free order to match.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use guest_layout::datum::layout;
use guest_layout::entry::{Cx, EntryError, GuestCall};
use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::points;

/// Object body flags (`body + 4`).
pub const BODY_FLAGS: u32 = 4;
/// Set when the node matrices are stale; `0x000bd090` clears it.
pub const DIRTY: u32 = 1 << 30;
/// Set when the pose is not to be evaluated (`palette::object_pose`).
pub const HOLD: u32 = 1 << 29;
/// The parent's handle (`body + 0x14`), `u32::MAX` for none.
pub const PARENT: u32 = 0x14;
/// Bytes per object-pool slot: salt `+0`, type `+3`, body `+8`.
pub const SLOT: u32 = 12;
/// The most slots a pool walk reads.
const MAX_SLOTS: u32 = 0x1_0000;
/// The longest parent chain followed; a longer one is a loop.
const MAX_DEPTH: usize = 32;

static PHASES: AtomicU64 = AtomicU64::new(0);
static BUILT: AtomicU64 = AtomicU64::new(0);
static LEFT_DIRTY: AtomicU64 = AtomicU64::new(0);

/// The phase's counters since start.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Phases that found a loaded pool.
    pub phases: u64,
    /// `0x000bd090` calls made.
    pub built: u64,
    /// Of those, objects whose bit 30 was still set after the call.
    pub left_dirty: u64,
}

pub fn stats() -> Stats {
    Stats { phases: PHASES.load(Relaxed), built: BUILT.load(Relaxed), left_dirty: LEFT_DIRTY.load(Relaxed) }
}

/// A body's flags say its matrices are to be built.
pub fn pending(flags: u32) -> bool {
    flags & DIRTY != 0 && flags & HOLD == 0
}

/// The live body of `handle`, `None` for a dead slot or a stale salt.
fn body(m: &dyn GuestMemory, header: u32, handle: u32) -> Result<Option<u32>, MemFault> {
    let index = handle & 0xffff;
    if handle == u32::MAX || index >= m.u32(header.wrapping_add(layout::HIGH_WATER))?.min(MAX_SLOTS) {
        return Ok(None);
    }
    let slot = m.u32(header.wrapping_add(layout::DATA))?.wrapping_add(index * SLOT);
    let salt = m.u16(slot)?;
    if salt == 0 || u32::from(salt) != handle >> 16 {
        return Ok(None);
    }
    let body = m.u32(slot.wrapping_add(8))?;
    Ok((body != 0).then_some(body))
}

/// The handles to build, in build order: index order, each object's
/// pending ancestors first, each handle once. `None` before a map is
/// loaded (no pool).
pub fn plan(m: &dyn GuestMemory) -> Option<Vec<u32>> {
    let header = m.u32(points::OBJECTS.base).ok().filter(|&h| h != 0)?;
    let end = m.u32(header.wrapping_add(layout::HIGH_WATER)).ok()?.min(MAX_SLOTS);
    let data = m.u32(header.wrapping_add(layout::DATA)).ok().filter(|&d| d != 0)?;
    let mut order = Vec::new();
    let mut queued = vec![false; end as usize];
    let mut chain = Vec::new();
    for index in 0..end {
        let Ok(salt) = m.u16(data.wrapping_add(index * SLOT)) else { continue };
        if salt == 0 {
            continue;
        }
        // The object, then its parents, while each is live and pending.
        let mut at = u32::from(salt) << 16 | index;
        chain.clear();
        while chain.len() < MAX_DEPTH {
            let Ok(Some(b)) = body(m, header, at) else { break };
            let Ok(flags) = m.u32(b.wrapping_add(BODY_FLAGS)) else { break };
            if !pending(flags) || queued[(at & 0xffff) as usize] || chain.contains(&at) {
                break;
            }
            chain.push(at);
            let Ok(parent) = m.u32(b.wrapping_add(PARENT)) else { break };
            at = parent;
        }
        for &h in chain.iter().rev() {
            queued[(h & 0xffff) as usize] = true;
            order.push(h);
        }
    }
    Some(order)
}

/// The phase. `reserve` is the stack kept below the entry; each build goes
/// below it. Returns the objects built.
pub async fn run(cx: &Cx, reserve: u32) -> Result<usize, EntryError> {
    let Some(order) = cx.read(|m| plan(m)) else { return Ok(0) };
    PHASES.fetch_add(1, Relaxed);
    let mut built = 0;
    for handle in order {
        // An earlier build can have moved or cleared it.
        let flags_at = |m: &dyn GuestMemory| -> Option<u32> {
            let header = m.u32(points::OBJECTS.base).ok()?;
            let b = body(m, header, handle).ok()??;
            Some(b.wrapping_add(BODY_FLAGS))
        };
        let Some(at) = cx.read(|m| flags_at(m)) else { continue };
        if !pending(cx.read(|m| m.u32(at))?) {
            continue;
        }
        cx.call(GuestCall::new(points::OBJECT_NODE_MATRICES.base).reserve(reserve).args([handle])).await?;
        built += 1;
        BUILT.fetch_add(1, Relaxed);
        let still = cx.read(|m| flags_at(m).and_then(|at| m.u32(at).ok()));
        if still.is_some_and(|f| f & DIRTY != 0) {
            LEFT_DIRTY.fetch_add(1, Relaxed);
        }
    }
    Ok(built)
}

#[cfg(test)]
mod tests;
