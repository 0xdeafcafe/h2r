//! Ordered verification walk over the live Havok heap (sim-rewrite-havok.md §9.2;
//! what it covers and leaves out: docs/engine/havok-step.md, "The digest").
//! Hash little-endian guest bytes, including pointers, without converting floats.
//! Scratch buffers, unused array capacity and freed payloads are outside the walk.

use super::{array, cache, ISLAND_PAIRS, MAX_PAIRS};
use crate::havok::{bad, span};
use crate::layout as l;
use guest_layout::{GuestMemory, MemFault};

// Pool constructor 0x22c3e0, usage 0x22cb90, pressure and overflow 0x146fd0.
const POOL: u32 = 0x0047_9898;
const STACKS: [u32; 3] = [0x0047_9894, 0x0047_989c, 0x0047_98a0];
const SOLVER_BUFFER: u32 = 0x0048_4b38;
const CHARGED: u32 = 0xf0344;
const FREE: u32 = 0xf0388;
const CLASSES: u32 = 17;
const CLASS_SIZE: u32 = 0x68;
const MAX_WORK_BYTES: u32 = 16 << 20;
// The phantom add 0x30e4b0 appends to the world's phantom array and gives
// the phantom the id at +0x20 from the counter at world +0x130
// (docs/engine/havok-step.md, "Censuses").
const WORLD_PHANTOMS: u32 = 0x120;
const WORLD_OBJECT_COUNTER: u32 = 0x130;
const PHANTOM_HEADER: u32 = 0x24;

#[derive(Debug, PartialEq, Eq)]
pub struct Digest {
    pub hash: u64,
    pub bodies: u32,
    pub constraints: u32,
    pub pairs: u32,
    /// Bytes in occupied pool slots, the integer numerator of 0x22cb90.
    pub heap_used: u32,
}

struct Walk<'a> {
    m: &'a dyn GuestMemory,
    result: Digest,
    remaining: u32,
}

impl Walk<'_> {
    fn bytes(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.result.hash ^= u64::from(byte);
            self.result.hash = self.result.hash.wrapping_mul(0x100000001b3);
        }
    }

    fn range(&mut self, base: u32, len: u32) -> Result<(), MemFault> {
        span(base, len)?;
        self.remaining = self.remaining.checked_sub(len).ok_or_else(|| bad(base))?;
        let mut buf = [0; 512];
        let mut at = 0;
        while at < len {
            let n = (len - at).min(buf.len() as u32);
            self.m.read(base + at, &mut buf[..n as usize])?;
            self.bytes(&buf[..n as usize]);
            at += n;
        }
        Ok(())
    }

    fn list(&mut self, header: u32, limit: u32, stride: u32) -> Result<(u32, u32), MemFault> {
        let (ptr, count) = array(self.m, header, limit, stride)?;
        self.range(header, 12)?;
        self.range(ptr, count * stride)?;
        Ok((ptr, count))
    }

    fn entity(&mut self, e: u32) -> Result<(), MemFault> {
        self.range(e, l::ENTITY_RESTITUTION + 4)?;
        let shape = self.m.u32(e + l::ENTITY_SHAPE)?;
        if shape != 0 {
            self.range(shape, 4)?; // shape identity and vtable
        }
        self.list(e + l::ENTITY_PROPERTIES, 256, 8)?;
        let motion = self.m.u32(e + l::RIGID_MOTION)?;
        span(motion, 6)?;
        // 0x311940 sets the size of the allocation at +4 before construction.
        let size = u32::from(self.m.u16(motion + 4)?);
        if !(0xc0..=4096).contains(&size) {
            return Err(bad(motion + 4));
        }
        self.range(motion, size)?;
        self.result.bodies += 1;
        Ok(())
    }

    fn constraint(&mut self, c: u32) -> Result<(), MemFault> {
        self.range(c, 0x18)?;
        if self.m.u32(c)? == l::CONTACT_CONSTRAINT_VTABLE {
            self.list(c + 0x20, 256, 1)?;
            self.list(c + l::CONTACT_POINTS, 256, l::CONTACT_POINT_SIZE)?;
            self.list(c + l::CONTACT_PROPERTIES, 256, l::CONTACT_PROPERTY_SIZE)?;
        }
        self.result.constraints += 1;
        Ok(())
    }

    fn agent(&mut self, base: u32, depth: u8) -> Result<(), MemFault> {
        if base == 0 {
            return Ok(());
        }
        if depth > 16 {
            return Err(bad(base));
        }
        let (_, kind, size) = cache::allocation(self.m, base)?;
        self.range(base, size)?;
        if matches!(kind, "bv_tree" | "welded_bv_tree") {
            let (entries, count) = self.list(base + 0x0c, MAX_PAIRS, 12)?;
            for i in 0..count {
                self.agent(self.m.u32(entries + 12 * i + 8)?, depth + 1)?;
            }
        }
        Ok(())
    }

    fn island(&mut self, island: u32) -> Result<(), MemFault> {
        // Includes solver sizes, deactivation/overflow flags and queued pair headers.
        self.range(island, 0xb0)?;
        let (ptr, n) = self.list(island + l::ISLAND_ENTITIES, 8192, 4)?;
        for i in 0..n {
            self.entity(self.m.u32(ptr + i * 4)?)?;
        }
        let (ptr, n) = self.list(island + l::ISLAND_CONSTRAINTS, 16384, 4)?;
        for i in 0..n {
            self.constraint(self.m.u32(ptr + i * 4)?)?;
        }
        let (ptr, n) = self.list(island + ISLAND_PAIRS, MAX_PAIRS, 16)?;
        for i in 0..n {
            self.agent(self.m.u32(ptr + i * 16 + 8)?, 0)?;
        }
        self.result.pairs += n;
        self.list(island + 0x4c, 8192, 4)?;
        Ok(())
    }

    fn pool(&mut self, pool: u32) -> Result<(), MemFault> {
        span(pool, FREE + CLASSES * 4)?;
        self.range(pool, CLASS_SIZE)?;
        self.range(pool + 0xf0340, (1 + 2 * CLASSES) * 4)?;
        for i in 1..CLASSES {
            let class = 4 * i;
            let size = self.m.u32(pool + CLASS_SIZE + class)?;
            let total = self.m.u32(pool + CHARGED + class)?;
            let free = self.m.u32(pool + FREE + class)?;
            let used = total.checked_sub(free).and_then(|n| n.checked_mul(size))
                .ok_or_else(|| bad(pool + FREE + class))?;
            self.result.heap_used = self.result.heap_used.checked_add(used)
                .ok_or_else(|| bad(pool + CHARGED + class))?;
            // Include the list order. Only the link word is live in a freed slot.
            let mut node = self.m.u32(pool + 0x24 + class)?;
            for _ in 0..free {
                self.range(node, 4)?;
                node = self.m.u32(node)?;
            }
            if node != 0 {
                return Err(bad(node));
            }
        }
        Ok(())
    }
}

/// A missing world is a distinct empty state. A bad read fails the whole walk.
pub fn read(m: &dyn GuestMemory) -> Result<Digest, MemFault> {
    let mut walk = Walk {
        m,
        result: Digest { hash: 0xcbf29ce484222325, bodies: 0, constraints: 0, pairs: 0, heap_used: 0 },
        remaining: MAX_WORK_BYTES,
    };
    walk.range(l::HAVOK_WORLD_PTR, 4)?;
    let world = m.u32(l::HAVOK_WORLD_PTR)?;
    if world == 0 {
        return Ok(walk.result);
    }
    span(world, 0x258)?;
    walk.range(world + 0x24, 12)?;
    walk.range(world + 0x10c, 12)?;
    walk.range(world + 0x12c, 2)?;
    walk.range(world + 0x150, 0x58)?;
    walk.range(world + 0x238, 0x20)?;
    for header in [l::WORLD_ACTIVE_ISLANDS, l::WORLD_INACTIVE_ISLANDS] {
        let (ptr, n) = walk.list(world + header, 4096, 4)?;
        for i in 0..n {
            walk.island(m.u32(ptr + 4 * i)?)?;
        }
    }
    let fixed = m.u32(world + l::WORLD_FIXED_ISLAND)?;
    if fixed != 0 {
        walk.island(fixed)?;
    }
    let (ptr, n) = walk.list(world + WORLD_PHANTOMS, 4096, 4)?;
    for i in 0..n {
        walk.range(m.u32(ptr + 4 * i)?, PHANTOM_HEADER)?;
    }
    walk.range(world + WORLD_OBJECT_COUNTER, 4)?;
    walk.range(POOL, 4)?;
    let pool = m.u32(POOL)?;
    if pool != 0 {
        walk.pool(pool)?;
    }
    for p in STACKS {
        walk.range(p, 4)?;
        let stack = m.u32(p)?;
        if stack != 0 {
            walk.range(stack, 0x24)?;
        }
    }
    walk.range(SOLVER_BUFFER, 4)?;
    let solver = m.u32(SOLVER_BUFFER)?;
    if solver != 0 {
        span(solver, 0x11)?;
        walk.range(solver + 8, 9)?;
    }
    Ok(walk.result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::{Guest, guest::Permissions};

    const WORLD: u32 = 0x10_0000;
    const ISLANDS: u32 = 0x10_1000;
    /// Island `i` at `ISLANDS + i × 0x100`; entity `i` at `ENTITIES + i ×
    /// 0x1000`, its motion at `+0x400`.
    const ENTITIES: u32 = 0x12_0000;
    const CONSTRAINT: u32 = 0x13_0000;

    fn island(i: u32) -> u32 {
        ISLANDS + i * 0x100
    }

    fn entity(i: u32) -> u32 {
        ENTITIES + i * 0x1000
    }

    /// A Havok array header: data, count, capacity (capacity ≥ count).
    fn array(g: &mut Guest, at: u32, data: u32, count: u32) {
        g.set_u32(at, data).unwrap();
        g.set_u32(at + 4, count).unwrap();
        g.set_u32(at + 8, count.max(1)).unwrap();
    }

    fn put_entity(g: &mut Guest, i: u32) {
        let e = entity(i);
        let motion = e + 0x400;
        g.set_u32(e + l::RIGID_MOTION, motion).unwrap();
        // The constructor at 0x311940 records the allocation size at +4.
        g.set_u16(motion + 4, 0x100).unwrap();
        g.set_f32(motion + 0x40, 1.0).unwrap();
        array(g, e + l::ENTITY_PROPERTIES, e + 0x800, 0);
    }

    fn put_island(g: &mut Guest, i: u32, entities: &[u32], constraints: &[u32]) {
        let isl = island(i);
        let list = isl + 0x80;
        array(g, isl + l::ISLAND_ENTITIES, list, entities.len() as u32);
        for (n, &e) in entities.iter().enumerate() {
            g.set_u32(list + 4 * n as u32, entity(e)).unwrap();
        }
        let list = isl + 0xa0;
        array(g, isl + l::ISLAND_CONSTRAINTS, list, constraints.len() as u32);
        for (n, &c) in constraints.iter().enumerate() {
            g.set_u32(list + 4 * n as u32, c).unwrap();
        }
        array(g, isl + 0x68, 0, 0);
        array(g, isl + 0x4c, 0, 0);
    }

    fn world() -> Guest {
        let mut g = Guest::new();
        g.map_zeroed(0x10_0000, 0x4_0000, Permissions::RW).unwrap();
        g.map_zeroed(0x47_9000, 0xd000, Permissions::RW).unwrap();
        g.map_zeroed(0x50_0000, 0x10_0000, Permissions::RW).unwrap();
        g.set_u32(l::HAVOK_WORLD_PTR, WORLD).unwrap();
        array(&mut g, WORLD + l::WORLD_ACTIVE_ISLANDS, ISLANDS, 1);
        g.set_u32(ISLANDS, island(0)).unwrap();
        array(&mut g, WORLD + l::WORLD_INACTIVE_ISLANDS, ISLANDS + 0x10, 1);
        g.set_u32(ISLANDS + 0x10, island(1)).unwrap();
        g.set_u32(WORLD + l::WORLD_FIXED_ISLAND, island(2)).unwrap();

        put_entity(&mut g, 0);
        put_entity(&mut g, 1);
        put_entity(&mut g, 2);
        // A contact constraint against the fixed body, with two points
        // and an id map; and a constraint of another class.
        let c = CONSTRAINT;
        g.set_u32(c, l::CONTACT_CONSTRAINT_VTABLE).unwrap();
        array(&mut g, c + 0x20, c + 0x200, 2);
        array(&mut g, c + l::CONTACT_POINTS, c + 0x300, 2);
        array(&mut g, c + l::CONTACT_PROPERTIES, c + 0x400, 2);
        let other = CONSTRAINT + 0x800;
        g.set_u32(other, 0x0041_0000).unwrap();
        put_island(&mut g, 0, &[0, 1], &[c, other]);
        put_island(&mut g, 1, &[2], &[]);
        put_island(&mut g, 2, &[], &[]);
        g
    }

    #[test]
    fn the_walk_counts_bodies_and_constraints_and_hashed_order() {
        let g = world();
        let d = read(&g).unwrap();
        assert_eq!((d.bodies, d.constraints, d.pairs, d.heap_used), (3, 2, 0, 0));
        // The same bytes hash the same; the order is part of the digest.
        assert_eq!(read(&g).unwrap().hash, d.hash);
        let mut nudged = world();
        nudged.set_f32(entity(1) + 0x408, 2.0).unwrap();
        let e = read(&nudged).unwrap();
        assert_ne!(e.hash, d.hash);
        assert_eq!((e.bodies, e.constraints), (3, 2));
    }

    #[test]
    fn phantoms_and_the_world_object_counter_are_in_the_walk() {
        const PHANTOM: u32 = 0x13_8000;
        const PHANTOM_LIST: u32 = 0x13_9000;
        let d = read(&world()).unwrap();
        let mut g = world();
        array(&mut g, WORLD + WORLD_PHANTOMS, PHANTOM_LIST, 1);
        g.set_u32(PHANTOM_LIST, PHANTOM).unwrap();
        let with = read(&g).unwrap();
        assert_ne!(with.hash, d.hash);
        // Phantoms are not bodies, constraints or pairs.
        assert_eq!(
            (with.bodies, with.constraints, with.pairs),
            (d.bodies, d.constraints, d.pairs)
        );
        g.set_u32(PHANTOM + 0x20, 7).unwrap();
        assert_ne!(read(&g).unwrap().hash, with.hash);
        let mut counted = world();
        counted.set_u32(WORLD + WORLD_OBJECT_COUNTER, 1).unwrap();
        assert_ne!(read(&counted).unwrap().hash, d.hash);
    }

    #[test]
    fn an_absent_world_is_a_distinct_empty_state_and_bad_sizes_fail() {
        let mut g = Guest::new();
        g.map_zeroed(0x50_0000, 0x10_0000, Permissions::RW).unwrap();
        let empty = read(&g).unwrap();
        assert_eq!((empty.bodies, empty.constraints, empty.pairs), (0, 0, 0));
        // The world pointer still hashed: not equal to a walk with a body.
        let w = world();
        assert_ne!(read(&w).unwrap().hash, empty.hash);
        let mut g = world();
        let motion = g.u32(entity(0) + l::RIGID_MOTION).unwrap();
        g.set_u16(motion + 4, 0xbf).unwrap();
        assert!(read(&g).is_err());
    }
}
