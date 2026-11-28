//! Per-island collision pairs and their narrow-phase agents. See
//! docs/engine/havok-runtime.md for the retail addresses behind this walk.

mod cache;
pub mod digest;

pub use cache::{Cache, Child, Convex, Tree};

use std::collections::{HashMap, HashSet};

use guest_layout::view::objects;
use guest_layout::{DatumHandle, GuestMemory, MemFault};

use crate::havok::{self, bad, span};
use crate::layout;

const ISLAND_PAIRS: u32 = 0x68;
const HANDLE_IN_ENTITY: u32 = 0x1c;
const MAX_PAIRS: u32 = 32768;
const MAX_CACHE_BYTES: u32 = 256 * 1024;
/// The pair word's offset in a 16-byte pair record
/// (docs/engine/havok-runtime.md, "Collision agents").
pub const PAIR_WORD: u32 = 12;

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

/// Entities in the object's current physics component, in component order.
pub fn object_entities(m: &dyn GuestMemory, datum: u32) -> Result<Vec<u32>, MemFault> {
    let mut out = Vec::new();
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
            out.push(entity);
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
        pair_word: m.u32(entry + PAIR_WORD)?,
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
    let handles: HashSet<_> = object_entities(m, datum)?
        .into_iter()
        .map(|entity| entity + HANDLE_IN_ENTITY)
        .collect();
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

/// The entry of the object's one pair against a fixed body, if it has
/// exactly one.
pub fn fixed_pair(m: &dyn GuestMemory, datum: u32) -> Result<Option<u32>, MemFault> {
    let fixed: Vec<_> = agents(m, datum)?
        .into_iter()
        .filter(|p| p.other_fixed)
        .map(|p| p.entry)
        .collect();
    Ok(match fixed[..] {
        [entry] => Some(entry),
        _ => None,
    })
}

/// The word of the object's one pair against a fixed body (the level), or
/// `None` without exactly one. The same pairs as [`fixed_pair`] finds, by a
/// minimal walk: it checks each pair's handles and the other body's island
/// and decodes no collision-agent cache or tree, so it is cheap enough to
/// call every tick.
pub fn fixed_pair_word(m: &dyn GuestMemory, datum: u32) -> Result<Option<u32>, MemFault> {
    let world = m.u32(layout::HAVOK_WORLD_PTR)?;
    if world == 0 {
        return Ok(None);
    }
    let handles: HashSet<_> = object_entities(m, datum)?
        .into_iter()
        .map(|entity| entity + HANDLE_IN_ENTITY)
        .collect();
    if handles.is_empty() {
        return Ok(None);
    }
    span(world, layout::WORLD_FIXED_ISLAND + 4)?;
    let fixed = m.u32(world + layout::WORLD_FIXED_ISLAND)?;
    if fixed == 0 {
        return Ok(None);
    }
    let mut remaining_pairs = MAX_PAIRS;
    let mut found = None;
    for (island, _) in havok::islands(m, world)? {
        span(island, ISLAND_PAIRS + 12)?;
        let (ptr, count) = array(m, island + ISLAND_PAIRS, remaining_pairs, 16)?;
        remaining_pairs -= count;
        for i in 0..count {
            let entry = ptr + 16 * i;
            let sides = [m.u32(entry)?, m.u32(entry + 4)?];
            let Some(side) = sides.iter().position(|h| handles.contains(h)) else {
                continue;
            };
            if m.u32(entry + 8)? == 0 {
                continue;
            }
            let other = sides[1 - side].checked_sub(0x10).ok_or_else(|| bad(sides[1 - side]))?;
            span(other, 0x24)?;
            if m.u32(other + 0x18)? != 1 {
                continue;
            }
            let owner = m.u32(other + 0x20)?;
            span(owner, 0x48)?;
            if m.u32(owner + 0x44)? != fixed {
                continue;
            }
            if found.replace(m.u32(entry + PAIR_WORD)?).is_some() {
                return Ok(None);
            }
        }
    }
    Ok(found)
}

/// One of an object's pairs against a fixed body (the level), as
/// [`level_pairs`] finds it: the pair's entry, its agent (a rebuilt pair
/// gets a new one) and its word.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LevelPair {
    pub entry: u32,
    pub agent: u32,
    pub word: u32,
}

/// An object's pairs against fixed bodies, and whether it sleeps.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LevelPairs {
    pub pairs: Vec<LevelPair>,
    /// One of its bodies, or a pair of its, is in an inactive island.
    pub asleep: bool,
}

/// The entity's island (`0x0030d2b9`).
const ENTITY_ISLAND: u32 = 0x44;

/// Every pair against a fixed body of each of `datums`, in their order,
/// by one minimal walk of the islands (the checks [`fixed_pair_word`]
/// makes, no cache or tree decoded), so many objects can be read every
/// tick. All of an object's level pairs, not only a single one: near
/// instanced geometry a body has more than one. An object with no
/// physics gets none.
pub fn level_pairs(m: &dyn GuestMemory, datums: &[u32]) -> Result<Vec<LevelPairs>, MemFault> {
    let mut out = vec![LevelPairs::default(); datums.len()];
    let world = m.u32(layout::HAVOK_WORLD_PTR)?;
    if world == 0 {
        return Ok(out);
    }
    let mut owner: HashMap<u32, usize> = HashMap::new();
    let mut entities = Vec::new();
    for (i, &d) in datums.iter().enumerate() {
        let Ok(found) = object_entities(m, d) else {
            continue;
        };
        for e in found {
            owner.insert(e + HANDLE_IN_ENTITY, i);
            entities.push((e, i));
        }
    }
    if owner.is_empty() {
        return Ok(out);
    }
    span(world, layout::WORLD_FIXED_ISLAND + 4)?;
    let fixed = m.u32(world + layout::WORLD_FIXED_ISLAND)?;
    let islands = havok::islands(m, world)?;
    let mut remaining_pairs = MAX_PAIRS;
    for &(island, state) in &islands {
        span(island, ISLAND_PAIRS + 12)?;
        let (ptr, count) = array(m, island + ISLAND_PAIRS, remaining_pairs, 16)?;
        remaining_pairs -= count;
        for i in 0..count {
            let entry = ptr + 16 * i;
            let sides = [m.u32(entry)?, m.u32(entry + 4)?];
            let Some((side, &who)) = sides
                .iter()
                .enumerate()
                .find_map(|(k, h)| owner.get(h).map(|w| (k, w)))
            else {
                continue;
            };
            let agent = m.u32(entry + 8)?;
            if agent == 0 || fixed == 0 {
                continue;
            }
            let other = sides[1 - side]
                .checked_sub(0x10)
                .ok_or_else(|| bad(sides[1 - side]))?;
            span(other, 0x24)?;
            if m.u32(other + 0x18)? != 1 {
                continue;
            }
            let body = m.u32(other + 0x20)?;
            span(body, 0x48)?;
            if m.u32(body + ENTITY_ISLAND)? != fixed {
                continue;
            }
            out[who].pairs.push(LevelPair {
                entry,
                agent,
                word: m.u32(entry + PAIR_WORD)?,
            });
            if state == havok::IslandState::Inactive {
                out[who].asleep = true;
            }
        }
    }
    let inactive: HashSet<u32> = islands
        .iter()
        .filter(|(_, s)| *s == havok::IslandState::Inactive)
        .map(|(i, _)| *i)
        .collect();
    for (e, i) in entities {
        if m.u32(e + ENTITY_ISLAND).is_ok_and(|isl| inactive.contains(&isl)) {
            out[i].asleep = true;
        }
    }
    Ok(out)
}

/// Writes `word` over each of `pairs` whose word differs, as
/// [`level_pairs`] found them this tick (nothing has run since, so the
/// entries are the pairs still). How many it wrote.
pub fn set_level_words(
    m: &mut dyn GuestMemory,
    pairs: &[LevelPair],
    word: u32,
) -> Result<u32, MemFault> {
    let mut n = 0;
    for p in pairs.iter().filter(|p| p.word != word) {
        m.write(p.entry + PAIR_WORD, &word.to_le_bytes())?;
        n += 1;
    }
    Ok(n)
}

/// Writes `word` over the pair word of `entry`, which must be one of the
/// object's pairs as [`agents`] finds them. The word before, or `None` when
/// `entry` isn't one of them.
pub fn set_pair_word(
    m: &mut dyn GuestMemory,
    datum: u32,
    entry: u32,
    word: u32,
) -> Result<Option<u32>, MemFault> {
    if !agents(&*m, datum)?.iter().any(|p| p.entry == entry) {
        return Ok(None);
    }
    let before = m.u32(entry + PAIR_WORD)?;
    m.write(entry + PAIR_WORD, &word.to_le_bytes())?;
    Ok(Some(before))
}

#[cfg(test)]
mod tests;
