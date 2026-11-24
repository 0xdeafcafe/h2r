//! Batch J planning. The next query reads the previous query's epoch and
//! mark stores through physical RAM, including through aliases. The caller
//! receives an answer per input to scatter after every query has succeeded.
use super::*;
use crate::sim_native::Memory;
use runtime::{
    GuestFault,
    guest::{Access, FaultReason},
};
use sim_collision::{
    batch::MarkBytes,
    bsp::{HEADER_BYTES, block},
    mopp,
};

#[derive(Clone, Copy, Debug)]
pub struct Trace {
    pub start: [u32; 3],
    pub delta: [u32; 3],
    pub flags: u32,
    pub ignore: [u32; 2],
}

/// Query outputs live in Rust. As with serial `trace`, all coordinates and
/// the returned fraction are binary32 bit patterns. No query in the batch
/// observes an intervening guest tick or a caller's result buffer.
pub fn trace_batch(
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
    queries: &[Trace],
) -> Result<Vec<QueryResult>, Bail> {
    let cpu = CpuAbi {
        mxcsr: 0x1fa0,
        ..CpuAbi::xbox_thread()
    };
    let mut ram = QueryMemory {
        base: g,
        marks: MarkBytes::default(),
    };
    let mut out = Vec::with_capacity(queries.len());
    for &Trace {
        start,
        delta,
        flags,
        ignore,
    } in queries
    {
        let p = plan(
            &cpu,
            &mut ram,
            world,
            Ray {
                start,
                delta,
                flags,
                ignore,
            },
            Record([0; RECORD_SIZE]),
        )?;
        p.marks.expected(|addr, bytes| {
            let len = bytes.len() as u32;
            if !ram.base.writable(addr, len) {
                return Err(Bail::Fault);
            }
            let physical = ram.base.physical_range(addr, len).ok_or(Bail::Alias)?;
            ram.marks.store(physical, bytes)
        })?;
        out.push(p.into_result());
    }
    Ok(out)
}

struct QueryMemory<'a, M> {
    base: &'a mut M,
    marks: MarkBytes,
}

impl<M: CollisionMemory> QueryMemory<'_, M> {
    // The world's caches are built from the backing RAM. A staged mark
    // overlapping a cached input through any mapping invalidates this batch.
    fn unstaged(&self, addr: u32, len: u32) -> Result<(), Bail> {
        if len > 0 {
            let start = self.base.physical_range(addr, len).ok_or(Bail::Alias)?;
            if self.marks.overlaps(start, len) {
                return Err(Bail::Alias);
            }
        }
        Ok(())
    }
}

impl<M: CollisionMemory> Memory for QueryMemory<'_, M> {
    fn read_bytes(&self, addr: u32, dst: &mut [u8], alignment: u32) -> Result<(), GuestFault> {
        self.base.read_bytes(addr, dst, alignment)?;
        if !self.marks.is_empty() && !dst.is_empty() {
            let fail = || GuestFault {
                addr,
                len: dst.len(),
                access: Access::Read,
                reason: FaultReason::InvalidBuffer,
            };
            let len = u32::try_from(dst.len()).map_err(|_| fail())?;
            let offset = self.base.physical_range(addr, len).ok_or_else(fail)?;
            self.marks.forward(offset, dst);
        }
        Ok(())
    }

    fn write_bytes(&mut self, addr: u32, src: &[u8], _: u32) -> Result<(), GuestFault> {
        Err(GuestFault {
            addr,
            len: src.len(),
            access: Access::Write,
            reason: FaultReason::InvalidBuffer,
        })
    }

    fn physical_read(&self, addr: u32, len: usize) -> Option<u32> {
        self.base.physical_range(addr, u32::try_from(len).ok()?)
    }
}

impl<M: CollisionMemory> CollisionMemory for QueryMemory<'_, M> {
    fn physical_range(&self, addr: u32, len: u32) -> Option<u32> {
        self.base.physical_range(addr, len)
    }

    fn writable(&self, addr: u32, len: u32) -> bool {
        self.base.writable(addr, len)
    }

    fn bsp_revision(&mut self, addr: u32) -> Result<(u64, u64), Bail> {
        if !self.marks.is_empty() {
            self.unstaged(addr, HEADER_BYTES)?;
            self.unstaged(layout::ZERO, 4)?;
            self.unstaged(layout::PROJECTION, 36)?;
            let header = Bsp::read_header(&Read(&*self.base, (0, 0)), addr)?;
            for (i, stride) in block::STRIDE.into_iter().enumerate() {
                self.unstaged(
                    header[2 * i + 1],
                    header[2 * i].checked_mul(stride).ok_or(Bail::Data)?,
                )?;
            }
        }
        self.base.bsp_revision(addr)
    }

    fn mopp_revision(&mut self, code: u32) -> Result<u64, Bail> {
        if !self.marks.is_empty() {
            self.unstaged(code, mopp::HEADER_BYTES)?;
            let header = mopp::Header::read(&Read(&*self.base, (0, 0)), code)?;
            self.unstaged(code, header.size)?;
        }
        self.base.mopp_revision(code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::{Guest, guest::Permissions};

    #[test]
    fn forwarded_marks_follow_physical_aliases_without_poisoning_decode_caches() {
        let mut base = Guest::new();
        const GLOBAL: u32 = 0x10000;
        const ALIAS: u32 = 0x20000;
        for addr in [GLOBAL, ALIAS] {
            base.map(addr, 0x100, 0x8000, Permissions::RW).unwrap();
        }
        base.set_u32(GLOBAL + 4, 0x1234_5678).unwrap();
        let mut g = QueryMemory {
            base: &mut base,
            marks: MarkBytes::default(),
        };
        let phys = g.physical_range(GLOBAL + 5, 1).unwrap();
        g.marks.store(phys, &[0xab]).unwrap();
        assert_eq!(g.u32(ALIAS + 4).unwrap(), 0x1234_ab78);
        assert_eq!(g.bsp_revision(ALIAS), Err(Bail::Alias));
        assert_eq!(g.mopp_revision(ALIAS), Err(Bail::Alias));
        assert_eq!(g.base.u32(ALIAS + 4).unwrap(), 0x1234_5678);
    }
}
