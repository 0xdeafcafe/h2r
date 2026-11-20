//! The guest-memory boundary every engine function goes through.
//!
//! Engine code never holds host pointers into guest RAM. It reads and writes
//! through [`GuestMemory`], so the same function runs against the runtime's
//! checked `Guest`, the IR interpreter's memory in a differential harness, or
//! the small [`FlatMemory`] that tests build by hand.
//!
//! Every access is little-endian and fallible. An access that the original
//! code would fault on must fault here too, so callers propagate [`MemFault`]
//! with `?` rather than substituting a default.

/// A failed guest access. `addr` is where the access started. The same
/// type as `guest_layout::Fault`, so a typed view's fault needs no
/// conversion.
pub use guest_layout::Fault as MemFault;

/// Byte-addressed guest memory (linear addresses, no segment base).
///
/// Only [`read`](GuestMemory::read) and [`write`](GuestMemory::write) are
/// required; the scalar helpers are built on them. The trait is object-safe
/// so override adapters can take `&mut dyn GuestMemory`.
pub trait GuestMemory {
    /// Fill `buf` from `addr..addr + buf.len()`. All or nothing.
    fn read(&self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault>;
    /// Store `data` at `addr`. All or nothing.
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault>;

    /// Consume a runtime write subscription on this specific memory view.
    /// Render hosts which enter D3D directly share its subscription with
    /// the ABI handlers. A synthetic memory has no pending runtime writes.
    #[cfg(feature = "runtime-glue")]
    fn take_write_watch(&mut self, _id: runtime::WatchId) -> Result<Vec<runtime::DirtyRange>, runtime::WatchError> {
        Ok(Vec::new())
    }

    fn u8(&self, addr: u32) -> Result<u8, MemFault> {
        let mut b = [0; 1];
        self.read(addr, &mut b)?;
        Ok(b[0])
    }
    fn u16(&self, addr: u32) -> Result<u16, MemFault> {
        let mut b = [0; 2];
        self.read(addr, &mut b)?;
        Ok(u16::from_le_bytes(b))
    }
    fn u32(&self, addr: u32) -> Result<u32, MemFault> {
        let mut b = [0; 4];
        self.read(addr, &mut b)?;
        Ok(u32::from_le_bytes(b))
    }
    fn i32(&self, addr: u32) -> Result<i32, MemFault> {
        self.u32(addr).map(|v| v as i32)
    }
    fn f32(&self, addr: u32) -> Result<f32, MemFault> {
        self.u32(addr).map(f32::from_bits)
    }
    /// `len` bytes from `addr`.
    fn bytes(&self, addr: u32, len: usize) -> Result<Vec<u8>, MemFault> {
        let mut b = vec![0; len];
        self.read(addr, &mut b)?;
        Ok(b)
    }
    /// A NUL-terminated byte string of at most `max` bytes (without the
    /// NUL). Faults if no NUL is found in `max + 1` readable bytes.
    fn cstr(&self, addr: u32, max: usize) -> Result<Vec<u8>, MemFault> {
        let mut out = Vec::new();
        for i in 0..=max as u32 {
            let b = self.u8(addr.wrapping_add(i))?;
            if b == 0 {
                return Ok(out);
            }
            out.push(b);
        }
        Err(MemFault {
            addr,
            len: max as u32 + 1,
            write: false,
        })
    }
    /// A NUL-terminated UTF-16 string of at most `max` code units, lossily
    /// decoded. Faults as [`cstr`](GuestMemory::cstr).
    fn utf16(&self, addr: u32, max: usize) -> Result<String, MemFault> {
        let mut units = Vec::new();
        for i in 0..=max as u32 {
            let u = self.u16(addr.wrapping_add(2 * i))?;
            if u == 0 {
                return Ok(String::from_utf16_lossy(&units));
            }
            units.push(u);
        }
        Err(MemFault {
            addr,
            len: 2 * (max as u32 + 1),
            write: false,
        })
    }
    /// Three consecutive `f32`s.
    fn vec3(&self, addr: u32) -> Result<[f32; 3], MemFault> {
        Ok([
            self.f32(addr)?,
            self.f32(addr.wrapping_add(4))?,
            self.f32(addr.wrapping_add(8))?,
        ])
    }
    fn set_f32(&mut self, addr: u32, v: f32) -> Result<(), MemFault> {
        self.set_u32(addr, v.to_bits())
    }
    fn set_u8(&mut self, addr: u32, v: u8) -> Result<(), MemFault> {
        self.write(addr, &[v])
    }
    fn set_u16(&mut self, addr: u32, v: u16) -> Result<(), MemFault> {
        self.write(addr, &v.to_le_bytes())
    }
    fn set_u32(&mut self, addr: u32, v: u32) -> Result<(), MemFault> {
        self.write(addr, &v.to_le_bytes())
    }
    fn set_i32(&mut self, addr: u32, v: i32) -> Result<(), MemFault> {
        self.set_u32(addr, v as u32)
    }

    /// `rep stosd` of `len >> 2` dwords then `rep stosb` of `len & 3`
    /// bytes, all of `byte`, from `addr` upward: the engine's every
    /// element/header/bitmap fill compiles to exactly this pair.
    ///
    /// Fault behaviour matches the instructions: every store before the
    /// faulting one has happened, and the faulting dword (or byte) writes
    /// nothing. Large runs are written in chunks and only split into
    /// single stores when a chunk faults, which relies on
    /// [`write`](GuestMemory::write) being all-or-nothing.
    fn fill(&mut self, addr: u32, len: u32, byte: u8) -> Result<(), MemFault> {
        const CHUNK: u32 = 4096;
        let buf = [byte; CHUNK as usize];
        let dwords = len & !3;
        let mut done = 0u32;
        while done < dwords {
            let n = (dwords - done).min(CHUNK);
            let at = addr.wrapping_add(done);
            if self.write(at, &buf[..n as usize]).is_err() {
                // Find the first dword store that faults.
                for k in (0..n).step_by(4) {
                    self.write(at.wrapping_add(k), &buf[..4])?;
                }
            }
            done += n;
        }
        for k in dwords..len {
            self.write(addr.wrapping_add(k), &buf[..1])?;
        }
        Ok(())
    }
}

/// One contiguous mapped range `[base, base + bytes.len())`. Anything outside
/// it faults. Meant for tests and harnesses, not the runtime.
#[derive(Debug, Clone)]
pub struct FlatMemory {
    pub base: u32,
    pub bytes: Vec<u8>,
}

impl FlatMemory {
    pub fn new(base: u32, len: usize) -> Self {
        FlatMemory {
            base,
            bytes: vec![0; len],
        }
    }

    fn span(&self, addr: u32, len: usize) -> Option<std::ops::Range<usize>> {
        // An access may not wrap past the top of the 32-bit address space.
        if addr as u64 + len as u64 > 1 << 32 {
            return None;
        }
        let off = addr.checked_sub(self.base)? as usize;
        let end = off.checked_add(len)?;
        (end <= self.bytes.len()).then_some(off..end)
    }
}

/// Typed views (`guest_layout`) read whatever engine code holds: a
/// `dyn GuestMemory`, the fake guest, or flat memory.
impl guest_layout::Mem for dyn GuestMemory + '_ {
    #[inline]
    fn read_bytes(&self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault> {
        self.read(addr, buf)
    }
}

impl guest_layout::MemMut for dyn GuestMemory + '_ {
    #[inline]
    fn write_bytes(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
        self.write(addr, data)
    }
}

macro_rules! layout_mem {
    ($($t:ty),*) => {$(
        impl guest_layout::Mem for $t {
            #[inline]
            fn read_bytes(&self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault> {
                GuestMemory::read(self, addr, buf)
            }
        }
        impl guest_layout::MemMut for $t {
            #[inline]
            fn write_bytes(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
                GuestMemory::write(self, addr, data)
            }
        }
    )*};
}
layout_mem!(FlatMemory, crate::testing::FakeGuest);

impl GuestMemory for FlatMemory {
    fn read(&self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault> {
        let r = self.span(addr, buf.len()).ok_or(MemFault {
            addr,
            len: buf.len() as u32,
            write: false,
        })?;
        buf.copy_from_slice(&self.bytes[r]);
        Ok(())
    }

    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
        let r = self.span(addr, data.len()).ok_or(MemFault {
            addr,
            len: data.len() as u32,
            write: true,
        })?;
        self.bytes[r].copy_from_slice(data);
        Ok(())
    }
}

/// The CRT `strncpy` at `0x3203e0`, ported store for store.
///
/// Semantically: copy up to `n` bytes from `src`, stop after the first NUL,
/// zero-pad to `n`. The access pattern matters when something faults, so it
/// follows the MSVC routine: bytes until `src` is dword-aligned, then aligned
/// dword reads with (possibly unaligned) dword stores, the dword holding the
/// NUL stored with its later bytes zeroed; padding is bytes until `dst` is
/// aligned, then dwords, then bytes. Dword reads can touch up to three bytes
/// past the NUL, as the original's do.
pub(crate) fn strncpy(
    m: &mut dyn GuestMemory,
    mut dst: u32,
    mut src: u32,
    n: u32,
) -> Result<(), MemFault> {
    if n == 0 {
        return Ok(());
    }
    let mut left = n;
    // Leading bytes until the source is aligned.
    while src & 3 != 0 {
        let b = m.u8(src)?;
        src = src.wrapping_add(1);
        m.set_u8(dst, b)?;
        dst = dst.wrapping_add(1);
        left -= 1;
        if left == 0 {
            return Ok(());
        }
        if b == 0 {
            return pad(m, dst, left);
        }
    }
    // Aligned dwords.
    let mut dwords = left >> 2;
    let tail = left & 3;
    while dwords != 0 {
        let v = m.u32(src)?;
        src = src.wrapping_add(4);
        let nul = v.to_le_bytes().iter().position(|&b| b == 0);
        match nul {
            None => {
                m.set_u32(dst, v)?;
                dst = dst.wrapping_add(4);
                dwords -= 1;
            }
            Some(k) => {
                let keep = if k == 0 {
                    0
                } else {
                    v & (u32::MAX >> (32 - 8 * k))
                };
                m.set_u32(dst, keep)?;
                dst = dst.wrapping_add(4);
                dwords -= 1;
                for _ in 0..dwords {
                    m.set_u32(dst, 0)?;
                    dst = dst.wrapping_add(4);
                }
                for _ in 0..tail {
                    m.set_u8(dst, 0)?;
                    dst = dst.wrapping_add(1);
                }
                return Ok(());
            }
        }
    }
    // Trailing bytes.
    let mut rest = tail;
    while rest != 0 {
        let b = m.u8(src)?;
        src = src.wrapping_add(1);
        m.set_u8(dst, b)?;
        dst = dst.wrapping_add(1);
        rest -= 1;
        if b == 0 {
            for _ in 0..rest {
                m.set_u8(dst, 0)?;
                dst = dst.wrapping_add(1);
            }
            return Ok(());
        }
    }
    Ok(())
}

/// strncpy's padding after a NUL copied in the unaligned prologue: bytes
/// until `dst` is aligned, then dwords, then bytes. `left >= 1`.
fn pad(m: &mut dyn GuestMemory, mut dst: u32, mut left: u32) -> Result<(), MemFault> {
    while dst & 3 != 0 {
        m.set_u8(dst, 0)?;
        dst = dst.wrapping_add(1);
        left -= 1;
        if left == 0 {
            return Ok(());
        }
    }
    for _ in 0..left >> 2 {
        m.set_u32(dst, 0)?;
        dst = dst.wrapping_add(4);
    }
    for _ in 0..left & 3 {
        m.set_u8(dst, 0)?;
        dst = dst.wrapping_add(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_memory_faults_outside_its_range() {
        let mut m = FlatMemory::new(0x1000, 0x10);
        assert_eq!(
            m.u32(0x100e),
            Err(MemFault {
                addr: 0x100e,
                len: 4,
                write: false
            })
        );
        assert!(m.set_u32(0x100c, 0xdead_beef).is_ok());
        assert_eq!(m.u16(0x100e).unwrap(), 0xdead);
        assert!(m.write(0x0fff, &[1]).is_err());
    }

    #[test]
    fn fill_is_dwords_then_bytes_and_keeps_the_prefix() {
        // 9002 bytes = 2250 dwords + 2 bytes; memory ends at 5002, so the
        // dword at 5000 straddles the end and faults without writing.
        let mut m = FlatMemory::new(0, 5002);
        let err = m.fill(0, 9002, 0xab).unwrap_err();
        assert_eq!(
            err,
            MemFault {
                addr: 5000,
                len: 4,
                write: true
            }
        );
        assert!(m.bytes[..5000].iter().all(|&b| b == 0xab));
        assert_eq!(&m.bytes[5000..], &[0, 0]);
        // The byte tail is stored byte by byte.
        let mut m = FlatMemory::new(0, 6);
        let err = m.fill(0, 7, 0xab).unwrap_err();
        assert_eq!(
            err,
            MemFault {
                addr: 6,
                len: 1,
                write: true
            }
        );
        assert_eq!(m.bytes, [0xab; 6]);
    }

    #[test]
    fn flat_memory_rejects_accesses_past_4gib() {
        // Backing that (nominally) extends past the top of the address space
        // must still not serve a wrapping access.
        let m = FlatMemory::new(0xffff_fff0, 0x20);
        assert!(m.u32(0xffff_fffc).is_ok());
        assert_eq!(
            m.u32(0xffff_fffe),
            Err(MemFault {
                addr: 0xffff_fffe,
                len: 4,
                write: false
            })
        );
        assert!(m.u8(0xffff_ffff).is_ok());
    }

    #[test]
    fn strncpy_follows_the_crt_store_pattern() {
        // Unaligned source: bytes until aligned, then a dword holding the
        // NUL stored with its later bytes zeroed, then zero padding.
        let mut m = FlatMemory::new(0, 64);
        m.write(1, b"abc\0zzzz").unwrap();
        m.write(32, &[0xff; 16]).unwrap();
        strncpy(&mut m, 32, 1, 11).unwrap();
        assert_eq!(&m.bytes[32..48], b"abc\0\0\0\0\0\0\0\0\xff\xff\xff\xff\xff");
        // A dword store that crosses the end of memory faults with the
        // earlier stores done: aligned source, unaligned destination.
        let mut m = FlatMemory::new(0, 32);
        m.write(0, b"abcdefgh\0").unwrap();
        let err = strncpy(&mut m, 26, 0, 8).unwrap_err();
        assert_eq!(
            err,
            MemFault {
                addr: 30,
                len: 4,
                write: true
            }
        );
        assert_eq!(&m.bytes[26..30], b"abcd");
        assert_eq!(&m.bytes[30..32], &[0, 0]);
    }

    #[test]
    fn strncpy_pads_and_stops_at_nul() {
        let mut m = FlatMemory::new(0, 64);
        m.write(0, b"ab\0zz").unwrap();
        m.write(16, &[0xff; 8]).unwrap();
        strncpy(&mut m, 16, 0, 6).unwrap();
        assert_eq!(&m.bytes[16..24], b"ab\0\0\0\0\xff\xff");
    }
}
