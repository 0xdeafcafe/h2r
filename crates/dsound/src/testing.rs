//! A synthetic guest for tests: flat memory, a bump allocator, and recorded
//! events. Also usable by the runtime-side glue tests.

use crate::host::{EventSink, GuestMemory, MemFault, StreamCallback};

/// Flat memory starting at `base`, with a bump allocator in the upper half.
pub struct TestHost {
    base: u32,
    mem: Vec<u8>,
    next_alloc: u32,
    pub signalled: Vec<u32>,
    pub callbacks: Vec<StreamCallback>,
    pub freed: Vec<u32>,
    /// When `Some(n)`, only the next `n` allocations succeed.
    pub allocs_left: Option<usize>,
    /// Every call to `alloc`, successful or not.
    pub alloc_calls: usize,
    /// Readable window at the very top of the address space
    /// (`HIGH_BASE..=0xffff_ffff`), for overflow tests.
    high: Vec<u8>,
    low: Vec<u8>,
}

/// End of the readable page at address 0 (see `HIGH_BASE`).
pub const LOW_END: u32 = 0x1000;

/// Start of [`TestHost`]'s top-of-address-space window.
pub const HIGH_BASE: u32 = 0xffff_f000;

impl TestHost {
    pub fn new() -> Self {
        let base = 0x0001_0000;
        let size = 0x40_0000;
        Self {
            base,
            mem: vec![0; size],
            next_alloc: base + size as u32 / 2,
            signalled: Vec::new(),
            callbacks: Vec::new(),
            freed: Vec::new(),
            allocs_left: None,
            alloc_calls: 0,
            high: vec![0; 0x1000],
            low: vec![0; LOW_END as usize],
        }
    }

    /// Start of the lower half, free for tests to lay data out in by hand.
    pub fn scratch(&self) -> u32 {
        self.base
    }

    fn range(&self, addr: u32, len: usize) -> Result<std::ops::Range<usize>, MemFault> {
        let fault = MemFault { addr, len: len as u32 };
        let off = addr.checked_sub(self.base).ok_or(fault)? as usize;
        let end = off.checked_add(len).ok_or(fault)?;
        if end > self.mem.len() {
            return Err(fault);
        }
        Ok(off..end)
    }
}

impl Default for TestHost {
    fn default() -> Self {
        Self::new()
    }
}

impl GuestMemory for TestHost {
    fn read(&self, addr: u32, out: &mut [u8]) -> Result<(), MemFault> {
        if addr >= HIGH_BASE {
            let off = (addr - HIGH_BASE) as usize;
            let fault = MemFault { addr, len: out.len() as u32 };
            let src = self.high.get(off..off + out.len()).ok_or(fault)?;
            out.copy_from_slice(src);
            return Ok(());
        }
        if addr < LOW_END {
            // Readable page at address 0, so an address that wrapped past
            // 2^32 would be read successfully instead of faulting.
            let fault = MemFault { addr, len: out.len() as u32 };
            let src = self.low.get(addr as usize..addr as usize + out.len()).ok_or(fault)?;
            out.copy_from_slice(src);
            return Ok(());
        }
        out.copy_from_slice(&self.mem[self.range(addr, out.len())?]);
        Ok(())
    }
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
        if addr >= HIGH_BASE {
            let off = (addr - HIGH_BASE) as usize;
            let fault = MemFault { addr, len: data.len() as u32 };
            let dst = self.high.get_mut(off..off + data.len()).ok_or(fault)?;
            dst.copy_from_slice(data);
            return Ok(());
        }
        if addr < LOW_END {
            let fault = MemFault { addr, len: data.len() as u32 };
            let dst = self.low.get_mut(addr as usize..addr as usize + data.len()).ok_or(fault)?;
            dst.copy_from_slice(data);
            return Ok(());
        }
        let r = self.range(addr, data.len())?;
        self.mem[r].copy_from_slice(data);
        Ok(())
    }
    fn alloc(&mut self, size: u32, align: u32) -> Option<u32> {
        self.alloc_calls += 1;
        if let Some(n) = self.allocs_left.as_mut() {
            if *n == 0 {
                return None;
            }
            *n -= 1;
        }
        let p = self.next_alloc.checked_add(align - 1)? & !(align - 1);
        let end = p.checked_add(size)?;
        self.range(p, size as usize).ok()?;
        self.next_alloc = end;
        Some(p)
    }
    fn free(&mut self, addr: u32) {
        self.freed.push(addr);
    }
}

impl EventSink for TestHost {
    fn signal_event(&mut self, handle: u32) {
        self.signalled.push(handle);
    }
    fn stream_callback(&mut self, cb: StreamCallback) {
        self.callbacks.push(cb);
    }
}
