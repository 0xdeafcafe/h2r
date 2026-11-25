//! The guest commits J's result and fills its recursive stack while
//! computing. Planning from the entry image only works when none of these
//! bytes feeds a later input, including a cached BSP or MOPP source.
use super::*;
use crate::sim_native::Memory;
use runtime::{
    GuestFault,
    guest::{Access, FaultReason},
};
use sim_collision::{
    bsp::{HEADER_BYTES, block},
    mem::overlaps,
    mopp,
};
use std::cell::Cell;

pub(in super::super) struct Guard<'a, M> {
    base: &'a mut M,
    protected: Vec<(u32, u32)>,
    alias: Cell<bool>,
}

impl<'a, M: CollisionMemory> Guard<'a, M> {
    pub(super) fn new(base: &'a mut M, sp: u32, out: u32, flags: u32) -> Result<Self, Bail> {
        // C inside E's terminal, or the deepest child 169510, its model
        // iterator and C. 169510 pushes its nine arguments and keeps 0x454
        // bytes per child. The extra page covers nonrecursive helper frames.
        let inner = if flags & 8 != 0 {
            0x480 * (sim_collision::bsp::MAX_DEPTH + 1) + 0x1000
        } else if flags & 4 != 0 {
            0x1c0 * sim_collision::mopp::MAX_FRAMES + 0x1000
        } else {
            0x40
        };
        let depth = 0x454 + inner + layout::VECTOR_DEAD_BELOW;
        Self::with_frame(
            base,
            sp.saturating_sub(depth),
            sp,
            &[(sp, 28), (out, RECORD_SIZE as u32)],
        )
    }

    pub(in super::super) fn with_frame(
        base: &'a mut M,
        lo: u32,
        hi: u32,
        regions: &[(u32, u32)],
    ) -> Result<Self, Bail> {
        let protected = base.frame_spans(lo, hi)?;
        let mut guard = Self {
            base,
            protected,
            alias: Cell::new(false),
        };
        for &(at, len) in regions {
            guard.separate(at, len)?;
            let p = guard.base.physical_range(at, len).ok_or(Bail::Alias)?;
            guard.protected.push((p, len));
        }
        Ok(guard)
    }

    pub(in super::super) fn separate(&self, at: u32, len: u32) -> Result<(), Bail> {
        if len == 0 {
            return Ok(());
        }
        let Some(p) = self.base.physical_range(at, len) else {
            self.alias.set(true);
            return Err(Bail::Alias);
        };
        if self.protected.iter().any(|&(a, n)| overlaps(a, n, p, len)) {
            self.alias.set(true);
            return Err(Bail::Alias);
        }
        Ok(())
    }

    pub(in super::super) fn alias(&self) -> bool {
        self.alias.get()
    }
}

impl<M: CollisionMemory> Memory for Guard<'_, M> {
    fn read_bytes(&self, addr: u32, dst: &mut [u8], alignment: u32) -> Result<(), GuestFault> {
        self.separate(addr, dst.len() as u32)
            .map_err(|_| GuestFault {
                addr,
                len: dst.len(),
                access: Access::Read,
                reason: FaultReason::InvalidBuffer,
            })?;
        self.base.read_bytes(addr, dst, alignment)
    }

    fn write_bytes(&mut self, addr: u32, src: &[u8], _: u32) -> Result<(), GuestFault> {
        Err(GuestFault {
            addr,
            len: src.len(),
            access: Access::Write,
            reason: FaultReason::Protection,
        })
    }
}

impl<M: CollisionMemory> CollisionMemory for Guard<'_, M> {
    fn physical_range(&self, at: u32, len: u32) -> Option<u32> {
        self.base.physical_range(at, len)
    }
    fn writable(&self, at: u32, len: u32) -> bool {
        self.base.writable(at, len)
    }
    fn bsp_revision(&mut self, addr: u32) -> Result<(u64, u64), Bail> {
        self.separate(addr, HEADER_BYTES)?;
        self.separate(layout::ZERO, 4)?;
        self.separate(layout::PROJECTION, 36)?;
        let h = Bsp::read_header(&Read(&*self.base, (0, 0)), addr)?;
        for (i, stride) in block::STRIDE.into_iter().enumerate() {
            self.separate(
                h[2 * i + 1],
                h[2 * i].checked_mul(stride).ok_or(Bail::Data)?,
            )?;
        }
        self.base.bsp_revision(addr)
    }
    fn mopp_revision(&mut self, code: u32) -> Result<u64, Bail> {
        self.separate(code, mopp::HEADER_BYTES)?;
        let h = mopp::Header::read(&Read(&*self.base, (0, 0)), code)?;
        self.separate(code, h.size)?;
        self.base.mopp_revision(code)
    }
}
