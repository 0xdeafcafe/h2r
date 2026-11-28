//! Retail collision agent allocations and the BV-tree child list. Evidence in
//! docs/engine/havok-runtime.md, "Collision agents".

use guest_layout::{GuestMemory, MemFault};

use super::{array, bad, span};

pub(super) const TREE_B: u32 = 0x0041_2a98;
pub(super) const TREE_A: u32 = 0x0041_2adc;
const MOPP_B: u32 = 0x0041_2b08;
const MOPP_A: u32 = 0x0041_2b28;
const WELDED_B: u32 = 0x0041_2f5c;
const WELDED_A: u32 = 0x0041_2f7c;
const SPHERE_WELDED_B: u32 = 0x0041_2fb0;
const SPHERE_WELDED_A: u32 = 0x0041_2fd0;
const WELDED_SHAPE: u32 = 0x0045_5540;
const CONVEX: u32 = 0x0041_2c48;
const SWEPT_CONVEX: u32 = 0x0041_2290;
const STRUCTURE_SHAPE: u32 = 0x0045_6160;
const EMPTY_AGENT: u32 = 0x0048_05a4;
const MAX_AGENT_BYTES: u32 = 4096;
const MAX_CHILDREN: u32 = 512;

#[derive(Debug)]
pub struct Cache {
    pub base: u32,
    pub vtable: u32,
    pub manager: u32,
    pub kind: &'static str,
    pub bytes: Vec<u8>,
    pub convex: Option<Convex>,
}

#[derive(Debug)]
pub struct Convex {
    /// Active entries in the two shapes' portions of the shared support vertex ID array.
    pub vertices_a: Vec<u16>,
    pub vertices_b: Vec<u16>,
    pub contact_ids: Vec<u16>,
    pub separating_axis: Option<[f32; 3]>,
    pub separating_distance: Option<f32>,
    pub swept_sides: Option<u32>,
}

#[derive(Debug)]
pub struct Tree {
    pub min: Option<[f32; 3]>,
    pub max: Option<[f32; 3]>,
    pub children: Vec<Child>,
}

#[derive(Debug)]
pub struct Child {
    pub entry: u32,
    pub key: u32,
    pub word_04: u32,
    pub surface: Option<u16>,
    pub cache: Cache,
}

fn finite_vec(m: &dyn GuestMemory, addr: u32) -> Result<Option<[f32; 3]>, MemFault> {
    let v = m.vec3(addr)?;
    Ok(v.into_iter().all(f32::is_finite).then_some(v))
}

fn convex(m: &dyn GuestMemory, base: u32, swept: bool) -> Result<Convex, MemFault> {
    let a = u32::from(m.u8(base + 0x14)?);
    let b = u32::from(m.u8(base + 0x15)?);
    if a + b > 4 {
        return Err(bad(base + 0x14));
    }
    let vertex = |i| m.u16(base + 0x0c + 2 * i);
    let count = m.u32(base + 0x18)?;
    if count > 4 {
        return Err(bad(base + 0x18));
    }
    let distance = m.f32(base + 0x2c)?;
    Ok(Convex {
        vertices_a: (0..a).map(vertex).collect::<Result<_, _>>()?,
        vertices_b: (a..a + b).map(vertex).collect::<Result<_, _>>()?,
        contact_ids: (0..count)
            .map(|i| m.u16(base + 0x30 + 0x30 * i + 0x1c))
            .collect::<Result<_, _>>()?,
        separating_axis: finite_vec(m, base + 0x20)?,
        separating_distance: distance.is_finite().then_some(distance),
        swept_sides: swept.then(|| m.u32(base + 0xf0)).transpose()?,
    })
}

pub(super) fn allocation(m: &dyn GuestMemory, base: u32) -> Result<(u32, &'static str, u32), MemFault> {
    span(base, 12)?;
    let vtable = m.u32(base)?;
    let (kind, size) = if base == EMPTY_AGENT {
        if vtable != 0x0041_357c {
            return Err(bad(base));
        }
        ("empty", 12)
    } else {
        let size = u32::from(m.u16(base + 4)?);
        let (kind, expected) = match vtable {
            TREE_A | TREE_B | MOPP_A | MOPP_B => ("bv_tree", 0x40),
            WELDED_A | WELDED_B | SPHERE_WELDED_A | SPHERE_WELDED_B => ("welded_bv_tree", 0x40),
            CONVEX => ("convex", 0xf0),
            SWEPT_CONVEX => ("swept_convex", 0x100),
            _ => ("other", size),
        };
        if size != expected || !(12..=MAX_AGENT_BYTES).contains(&size) {
            return Err(bad(base + 4));
        }
        (kind, size)
    };
    Ok((vtable, kind, size))
}

pub(super) fn cache(
    m: &dyn GuestMemory,
    base: u32,
    remaining: &mut u32,
) -> Result<Cache, MemFault> {
    let (vtable, kind, size) = allocation(m, base)?;
    *remaining = remaining.checked_sub(size).ok_or_else(|| bad(base))?;
    span(base, size)?;
    Ok(Cache {
        base,
        vtable,
        manager: m.u32(base + 8)?,
        kind,
        bytes: m.bytes(base, size as usize)?,
        convex: matches!(vtable, CONVEX | SWEPT_CONVEX)
            .then(|| convex(m, base, vtable == SWEPT_CONVEX))
            .transpose()?,
    })
}

/// Decode the immediate children. Only the structure tree uses Halo's BSP surface key encoding.
pub(super) fn tree(
    m: &dyn GuestMemory,
    parent: &Cache,
    collidables: [u32; 2],
    remaining: &mut u32,
) -> Result<Option<Tree>, MemFault> {
    let (tree_side, wrapped) = match parent.vtable {
        TREE_A | MOPP_A => (0, false),
        TREE_B | MOPP_B => (1, false),
        WELDED_A | SPHERE_WELDED_A => (0, true),
        WELDED_B | SPHERE_WELDED_B => (1, true),
        _ => return Ok(None),
    };
    let mut shape = m.u32(collidables[tree_side])?;
    if wrapped && shape != 0 && m.u32(shape)? == WELDED_SHAPE {
        span(shape, 16)?;
        shape = m.u32(shape + 0x0c)?;
    }
    let structure = shape != 0 && m.u32(shape)? == STRUCTURE_SHAPE;
    let base = parent.base;
    let (ptr, count) = array(m, base + 0x0c, MAX_CHILDREN, 12)?;
    let mut children = Vec::with_capacity(count as usize);
    for i in 0..count {
        let entry = ptr + 12 * i;
        let key = m.u32(entry)?;
        children.push(Child {
            entry,
            key,
            word_04: m.u32(entry + 4)?,
            surface: (structure && key >> 29 == 1).then_some(key as u16),
            cache: cache(m, m.u32(entry + 8)?, remaining)?,
        });
    }
    Ok(Some(Tree {
        min: finite_vec(m, base + 0x20)?,
        max: finite_vec(m, base + 0x30)?,
        children,
    }))
}
