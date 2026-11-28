//! Preflight the memory the original overwrites, including its stack. A
//! lifted caller can point into that stack through another virtual mapping.
use super::layout;
use crate::sim_native::Memory;
use runtime::{
    Guest,
    guest::Access,
};
use sim_collision::{
    Bail, Bsp, TopologyKey,
    mem::overlaps,
    structure::{LeafTable, TableVersion},
};

pub trait CollisionMemory: Memory {
    /// Reads must remain attributable to the guest instruction under a trap.
    fn trapped(&self) -> bool {
        false
    }

    /// Establish a version for this BSP and its decode constants. The
    /// default is for immutable bytes and a world dedicated to one fixture.
    fn bsp_revision(&mut self, _addr: u32) -> Result<(u64, u64), Bail> {
        Ok((0, 0))
    }

    /// A version of the bytes under the MOPP program at `code` (header and
    /// bytecode): it moves whenever any of them may have been written. The
    /// default is for immutable bytes.
    fn mopp_revision(&mut self, _code: u32) -> Result<u64, Bail> {
        Ok(0)
    }

    /// A version for all bytes of this leaf table, when the memory provider
    /// can detect every change. Other providers read the selected word live.
    fn leaf_table(&mut self, _count_addr: u32, _leaves: u32, _leaf: u32) -> Option<LeafTable> {
        None
    }

    /// A version for every byte a portal graph was decoded from, when the
    /// provider can detect every change. With `sources` (after a decode)
    /// it arms a new version over those spans; with `None` it returns the
    /// armed version for `key` while no source has changed. Other
    /// providers decode the graph on every call.
    fn topology_version(
        &mut self,
        _key: TopologyKey,
        _sources: Option<&[(u32, u32)]>,
    ) -> Option<TableVersion> {
        None
    }

    /// Physical spans behind the possibly mapped portion of a recursive
    /// frame below the entry. Flat providers require it to be contiguous.
    fn frame_spans(&self, lo: u32, hi: u32) -> Result<Vec<(u32, u32)>, Bail> {
        let len = hi.checked_sub(lo).ok_or(Bail::Alias)?;
        Ok(vec![(
            self.physical_range(lo, len).ok_or(Bail::Alias)?,
            len,
        )])
    }

    /// Physical address of the first byte of a contiguous, readable range.
    fn physical_range(&self, addr: u32, len: u32) -> Option<u32>;
    /// Every byte in this range can be written during this call.
    fn writable(&self, addr: u32, len: u32) -> bool;
}

impl CollisionMemory for Guest {
    fn frame_spans(&self, lo: u32, hi: u32) -> Result<Vec<(u32, u32)>, Bail> {
        let mut spans = Vec::new();
        for m in self.mappings() {
            let start = lo.max(m.virtual_start);
            let end = u64::from(hi).min(u64::from(m.virtual_start) + u64::from(m.len));
            if u64::from(start) < end {
                spans.push((
                    m.physical_start + (start - m.virtual_start),
                    (end - u64::from(start)) as u32,
                ));
            }
        }
        Ok(spans)
    }

    fn trapped(&self) -> bool {
        self.has_trap()
    }

    fn bsp_revision(&mut self, addr: u32) -> Result<(u64, u64), Bail> {
        super::sources::revision(self, addr)
    }

    fn leaf_table(&mut self, count_addr: u32, leaves: u32, leaf: u32) -> Option<LeafTable> {
        super::cluster::table(self, count_addr, leaves, leaf)
    }

    fn topology_version(
        &mut self,
        key: TopologyKey,
        sources: Option<&[(u32, u32)]>,
    ) -> Option<TableVersion> {
        super::topology::version(self, key, sources)
    }

    fn mopp_revision(&mut self, code: u32) -> Result<u64, Bail> {
        super::sources::mopp_revision(self, code)
    }

    fn physical_range(&self, addr: u32, len: u32) -> Option<u32> {
        // Readable, and physically contiguous across any page mappings it
        // crosses (the loader and the kernel map page by page).
        self.check(addr, len.max(1) as usize, Access::Read, 1).ok()?;
        crate::sim_native::plan::contiguous(self, addr, len as usize)
    }

    fn writable(&self, addr: u32, len: u32) -> bool {
        self.check(addr, len as usize, Access::Write, 1).is_ok()
    }
}

/// The physical frame below ESP and every other physical output span.
/// The gathered return target lies outside all of them.
pub(super) struct Outputs<const N: usize> {
    frame: (u32, u32),
    ranges: [(u32, u32); N],
    pub(super) target: u32,
}

impl<const N: usize> Outputs<N> {
    pub(super) fn new(
        g: &impl CollisionMemory,
        sp: u32,
        depth: u32,
        ranges: [(u32, u32); N],
    ) -> Result<Self, Bail> {
        let writable = |at, len| {
            if !g.writable(at, len) {
                return Err(Bail::Fault);
            }
            Ok((g.physical_range(at, len).ok_or(Bail::Alias)?, len))
        };
        let mut writes = Self {
            frame: writable(sp.checked_sub(depth).ok_or(Bail::Fault)?, depth)?,
            ranges: [(0, 0); N],
            target: g.u32(sp).map_err(|_| Bail::Fault)?,
        };
        for (i, (addr, len)) in ranges.into_iter().enumerate() {
            let span = writable(addr, len)?;
            writes.check_span(span)?;
            writes.ranges[i] = span;
        }
        writes.separate(g, sp, 4)?;
        Ok(writes)
    }

    fn check_span(&self, (p, n): (u32, u32)) -> Result<(), Bail> {
        if overlaps(p, n, self.frame.0, self.frame.1)
            || self.ranges.iter().any(|&(q, m)| overlaps(p, n, q, m))
        {
            return Err(Bail::Alias);
        }
        Ok(())
    }

    pub(super) fn separate(
        &self,
        g: &impl CollisionMemory,
        addr: u32,
        len: u32,
    ) -> Result<(), Bail> {
        if len != 0 {
            self.check_span((g.physical_range(addr, len).ok_or(Bail::Alias)?, len))?;
        }
        Ok(())
    }

    pub(super) fn check_bsp(&self, g: &impl CollisionMemory, bsp: &Bsp) -> Result<(), Bail> {
        for (addr, len) in bsp.sources() {
            self.separate(g, addr, len)?;
        }
        self.separate(g, layout::ZERO, 4)?;
        self.separate(g, layout::PROJECTION, 36)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim_collision::{point_to_leaf, vector_context, vector_test};
    use runtime::CpuAbi;
    use runtime::guest::Permissions;
    use sim_collision::{
        Bail, CollisionWorld, Entry,
        synth::{Builder, SOLID},
    };
    use sim_core::Epoch;

    #[test]
    fn inputs_in_guest_stack_frames_decline_before_a_write() {
        const BASE: u32 = 0x10_000;
        const BSP: u32 = BASE;
        const SP: u32 = BASE + 0x8000;
        const RECORD: u32 = BASE + 0x1000;
        const CONTEXT: u32 = BASE + 0x2000;
        const DELTA: u32 = BASE + 0x2100;
        let mut g = Guest::new();
        g.map(BASE, 0x9000, 0, Permissions::RW).unwrap();
        g.map(layout::PROJECTION, 36, 0x9000, Permissions::RW)
            .unwrap();
        g.map(layout::ONE, 0x40, 0xa000, Permissions::RW).unwrap();
        g.set_u32(layout::ONE, 1.0f32.to_bits()).unwrap();
        let mut bsp = Builder::default();
        bsp.planes.push([1.0, 0.0, 0.0, 0.0]);
        bsp.nodes.push((0, SOLID, SOLID));
        bsp.write(BSP, |at, bytes| g.write_bytes(at, bytes, 1).unwrap());
        static EPOCH: Epoch = Epoch::new();
        let world = CollisionWorld::new(&EPOCH);
        for entry in [Entry::PointToLeaf, Entry::VectorTest, Entry::VectorContext] {
            let start = SP - 8;
            let mut cpu = CpuAbi {
                esp: SP,
                mxcsr: 0x1fa0,
                ..CpuAbi::default()
            };
            let call: fn(&mut CpuAbi, &mut Guest, &CollisionWorld) -> _ = match entry {
                Entry::PointToLeaf => {
                    cpu.ecx = BSP;
                    cpu.edx = start;
                    point_to_leaf
                }
                Entry::VectorTest => {
                    cpu.eax = RECORD;
                    cpu.edx = BSP;
                    g.set_u32(SP + 12, start).unwrap();
                    g.set_u32(SP + 16, DELTA).unwrap();
                    vector_test
                }
                Entry::VectorContext => {
                    g.set_u32(SP + 4, CONTEXT).unwrap();
                    g.set_u32(CONTEXT + layout::context::BSP, BSP).unwrap();
                    g.set_u32(CONTEXT + layout::context::RECORD, RECORD)
                        .unwrap();
                    g.set_u32(CONTEXT + layout::context::START, start).unwrap();
                    g.set_u32(CONTEXT + layout::context::DELTA, DELTA).unwrap();
                    vector_context
                }
                _ => unreachable!(),
            };
            let before = cpu;
            let mut bytes = vec![0; 0x9000];
            g.read_bytes(BASE, &mut bytes, 1).unwrap();
            let result = call(&mut cpu, &mut g, &world);
            assert!(result.unwrap().is_none(), "{entry:?}");
            assert_eq!(before, cpu, "{entry:?}");
            let mut after = vec![0; bytes.len()];
            g.read_bytes(BASE, &mut after, 1).unwrap();
            assert!(after == bytes, "{entry:?} changed guest RAM");
            assert_eq!(
                world.stats().entries[entry as usize].bails[Bail::Alias as usize],
                1
            );
        }
    }
}
