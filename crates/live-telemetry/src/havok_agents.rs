//! Per-island collision pairs and their narrow-phase agents. See
//! docs/engine/havok-runtime.md for the retail addresses behind this walk.

mod cache;

pub use cache::{Cache, Child, Convex, Tree};

use std::collections::HashSet;

use engine::view::objects;
use engine::{DatumHandle, GuestMemory, MemFault};

use crate::havok::{self, bad, span};
use crate::layout;

const ISLAND_PAIRS: u32 = 0x68;
const HANDLE_IN_ENTITY: u32 = 0x1c;
const MAX_PAIRS: u32 = 32768;
const MAX_CACHE_BYTES: u32 = 256 * 1024;

#[derive(Debug)]
pub struct Pair {
    pub island: u32,
    pub entry: u32,
    /// In native dispatch order, including when the requested object is B.
    pub collidables: [u32; 2],
    pub object_side: usize,
    pub entity: u32,
    pub other_entity: Option<u32>,
    pub other_datum: Option<u32>,
    pub other_fixed: bool,
    pub pair_word: u32,
    pub cache: Cache,
    pub tree: Option<Tree>,
}

fn array(
    m: &dyn GuestMemory,
    header: u32,
    limit: u32,
    stride: u32,
) -> Result<(u32, u32), MemFault> {
    span(header, 12)?;
    let (ptr, count) = havok::array(m, header, limit)?;
    let capacity = m.u32(header + 8)? & 0x7fff_ffff;
    if count > capacity {
        return Err(bad(header + 4));
    }
    span(ptr, count * stride)?;
    Ok((ptr, count))
}

/// Collect all the component's rigid-body handles, including multi-body objects.
fn handles(m: &dyn GuestMemory, datum: u32) -> Result<HashSet<u32>, MemFault> {
    let mut out = HashSet::new();
    let Some(handle) = DatumHandle::from_raw(datum) else {
        return Ok(out);
    };
    let Some((_, body)) = objects::get(m, handle)? else {
        return Ok(out);
    };
    span(body.addr, layout::BODY_PHYSICS + 4)?;
    let index = m.u32(body.addr + layout::BODY_PHYSICS)?;
    if index == u32::MAX {
        return Ok(out);
    }
    let table = m.u32(layout::PHYSICS_COMPONENTS_PTR)?;
    span(table, layout::DATUM_DATA + 4)?;
    let data = m.u32(table + layout::DATUM_DATA)?;
    let offset = (index & 0xffff) * layout::PHYSICS_COMPONENT_SIZE;
    span(data, offset + layout::PHYSICS_COMPONENT_SIZE)?;
    let component = data + offset;
    if m.u32(component + layout::COMPONENT_OBJECT)? != datum {
        return Err(bad(component));
    }
    let (ptr, count) = array(
        m,
        component + layout::COMPONENT_BODIES,
        256,
        layout::COMPONENT_BODY_SIZE,
    )?;
    for i in 0..count {
        let entry = ptr + i * layout::COMPONENT_BODY_SIZE;
        let entity = m.u32(entry + layout::COMPONENT_BODY_RIGID)?;
        if entity != 0 {
            span(entity, HANDLE_IN_ENTITY + 4)?;
            out.insert(entity + HANDLE_IN_ENTITY);
        }
    }
    Ok(out)
}

fn pair(
    m: &dyn GuestMemory,
    island: u32,
    fixed: u32,
    entry: u32,
    handles: &HashSet<u32>,
    remaining: &mut u32,
) -> Result<Option<Pair>, MemFault> {
    let sides = [m.u32(entry)?, m.u32(entry + 4)?];
    let Some(side) = sides.iter().position(|h| handles.contains(h)) else {
        return Ok(None);
    };
    let agent = m.u32(entry + 8)?;
    if agent == 0 {
        return Ok(None);
    }
    let collidable = |handle: u32| {
        let addr = handle.checked_sub(0x10).ok_or_else(|| bad(handle))?;
        span(addr, 0x24)?;
        Ok::<_, MemFault>(addr)
    };
    let collidables = [collidable(sides[0])?, collidable(sides[1])?];
    let other = collidables[1 - side];
    let other_entity = if m.u32(other + 0x18)? == 1 {
        let owner = m.u32(other + 0x20)?;
        span(owner, 0x48)?;
        Some(owner)
    } else {
        None
    };
    let other_datum = other_entity
        .map(|entity| havok::entity_object(m, entity))
        .transpose()?
        .flatten();
    let other_island = other_entity.map(|e| m.u32(e + 0x44)).transpose()?;
    let cache = cache::cache(m, agent, remaining)?;
    let tree = cache::tree(m, &cache, collidables, remaining)?;
    Ok(Some(Pair {
        island,
        entry,
        collidables,
        object_side: side,
        entity: sides[side] - HANDLE_IN_ENTITY,
        other_entity,
        other_datum,
        other_fixed: fixed != 0 && other_island == Some(fixed),
        pair_word: m.u32(entry + 12)?,
        cache,
        tree,
    }))
}

/// Visit the active, sleeping and fixed islands. A fixed body's pairs can be kept
/// in its moving partner's island. Filter on the handles before following any agent.
pub fn agents(m: &dyn GuestMemory, datum: u32) -> Result<Vec<Pair>, MemFault> {
    let mut out = Vec::new();
    let world = m.u32(layout::HAVOK_WORLD_PTR)?;
    if world == 0 {
        return Ok(out);
    }
    let handles = handles(m, datum)?;
    if handles.is_empty() {
        return Ok(out);
    }
    span(world, layout::WORLD_FIXED_ISLAND + 4)?;
    let fixed = m.u32(world + layout::WORLD_FIXED_ISLAND)?;
    let mut remaining_pairs = MAX_PAIRS;
    let mut remaining_bytes = MAX_CACHE_BYTES;
    for (island, _) in havok::islands(m, world)? {
        span(island, ISLAND_PAIRS + 12)?;
        let (ptr, count) = array(m, island + ISLAND_PAIRS, remaining_pairs, 16)?;
        remaining_pairs -= count;
        for i in 0..count {
            let entry = ptr + 16 * i;
            if let Some(pair) = pair(m, island, fixed, entry, &handles, &mut remaining_bytes)? {
                out.push(pair);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
