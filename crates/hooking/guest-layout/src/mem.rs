//! The memory a view reads: two byte-level methods, generic so a field
//! read through a concrete memory compiles to a bounds check and a load.

/// A failed guest access. `addr` is where the access started.
///
/// `guest_layout::memory::MemFault` is this type, so `?` moves faults between views and
/// engine code without a conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fault {
    pub addr: u32,
    pub len: u32,
    pub write: bool,
}

/// Byte-addressed guest memory a view reads. All or nothing: a read that
/// faults leaves `buf` unspecified.
///
/// `engine` implements it for `dyn guest_layout::memory::GuestMemory`, its fake guest
/// and its flat memory, so views take what engine code already holds.
pub trait Mem {
    fn read_bytes(&self, addr: u32, buf: &mut [u8]) -> Result<(), Fault>;
}

/// Guest memory a view writes.
pub trait MemMut: Mem {
    fn write_bytes(&mut self, addr: u32, data: &[u8]) -> Result<(), Fault>;
}

impl<M: Mem + ?Sized> Mem for &M {
    #[inline]
    fn read_bytes(&self, addr: u32, buf: &mut [u8]) -> Result<(), Fault> {
        (**self).read_bytes(addr, buf)
    }
}

impl<M: Mem + ?Sized> Mem for &mut M {
    #[inline]
    fn read_bytes(&self, addr: u32, buf: &mut [u8]) -> Result<(), Fault> {
        (**self).read_bytes(addr, buf)
    }
}

impl<M: MemMut + ?Sized> MemMut for &mut M {
    #[inline]
    fn write_bytes(&mut self, addr: u32, data: &[u8]) -> Result<(), Fault> {
        (**self).write_bytes(addr, data)
    }
}

/// One contiguous run of guest memory starting at `base`, for tests and
/// for consumers that copied a region out (a snapshot, a film keyframe).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FlatRam {
    base: u32,
    bytes: Vec<u8>,
}

impl FlatRam {
    /// `len` zero bytes at `base`.
    pub fn new(base: u32, len: usize) -> Self {
        FlatRam {
            base,
            bytes: vec![0; len],
        }
    }

    /// `bytes` at `base`.
    pub fn from_bytes(base: u32, bytes: Vec<u8>) -> Self {
        FlatRam { base, bytes }
    }

    pub fn base(&self) -> u32 {
        self.base
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    fn span(&self, addr: u32, len: usize) -> Option<std::ops::Range<usize>> {
        let start = addr.checked_sub(self.base)? as usize;
        let end = start.checked_add(len)?;
        (end <= self.bytes.len()).then_some(start..end)
    }
}

impl Mem for FlatRam {
    #[inline]
    fn read_bytes(&self, addr: u32, buf: &mut [u8]) -> Result<(), Fault> {
        let r = self.span(addr, buf.len()).ok_or(Fault {
            addr,
            len: buf.len() as u32,
            write: false,
        })?;
        buf.copy_from_slice(&self.bytes[r]);
        Ok(())
    }
}

impl MemMut for FlatRam {
    #[inline]
    fn write_bytes(&mut self, addr: u32, data: &[u8]) -> Result<(), Fault> {
        let r = self.span(addr, data.len()).ok_or(Fault {
            addr,
            len: data.len() as u32,
            write: true,
        })?;
        self.bytes[r].copy_from_slice(data);
        Ok(())
    }
}
