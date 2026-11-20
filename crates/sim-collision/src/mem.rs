//! Reads of guest memory. Kernels see only this: bulk and scalar reads,
//! little-endian, through the runtime's checked bus in the game and a
//! plain buffer in tests.

/// A read the guest bus refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fault {
    pub addr: u32,
}

pub trait GuestRead {
    fn read(&self, addr: u32, dst: &mut [u8]) -> Result<(), Fault>;

    fn u8(&self, a: u32) -> Result<u8, Fault> {
        let mut b = [0; 1];
        self.read(a, &mut b)?;
        Ok(b[0])
    }

    fn u16(&self, a: u32) -> Result<u16, Fault> {
        let mut b = [0; 2];
        self.read(a, &mut b)?;
        Ok(u16::from_le_bytes(b))
    }

    fn u32(&self, a: u32) -> Result<u32, Fault> {
        let mut b = [0; 4];
        self.read(a, &mut b)?;
        Ok(u32::from_le_bytes(b))
    }

    /// Three consecutive binary32 bit patterns.
    fn vec3(&self, a: u32) -> Result<[u32; 3], Fault> {
        let mut b = [0; 12];
        self.read(a, &mut b)?;
        Ok(std::array::from_fn(|i| {
            u32::from_le_bytes(b[4 * i..4 * i + 4].try_into().unwrap())
        }))
    }

    /// `len` bytes at `a`, or a fault when the range wraps.
    fn bytes(&self, a: u32, len: usize) -> Result<Vec<u8>, Fault> {
        if u64::from(a) + len as u64 > 1 << 32 {
            return Err(Fault { addr: a });
        }
        let mut out = vec![0; len];
        self.read(a, &mut out)?;
        Ok(out)
    }
}

/// Whether `[a, a + a_len)` and `[b, b + b_len)` share a byte, on the
/// 32-bit address space (a range that wraps counts as overlapping
/// everything past its start).
pub fn overlaps(a: u32, a_len: u32, b: u32, b_len: u32) -> bool {
    let (a, a_end) = (u64::from(a), u64::from(a) + u64::from(a_len));
    let (b, b_end) = (u64::from(b), u64::from(b) + u64::from(b_len));
    a_len != 0 && b_len != 0 && a < b_end && b < a_end
}
