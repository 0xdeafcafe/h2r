//! fnv64 (FNV-1a), the simulation hash. Shared here because the copies in
//! the cli are private.

const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0000_0100_0000_01b3;

#[derive(Clone, Copy, Debug)]
pub struct Fnv64(u64);

impl Default for Fnv64 {
    fn default() -> Self {
        Self::new()
    }
}

impl Fnv64 {
    pub fn new() -> Self {
        Self(OFFSET)
    }

    pub fn of(bytes: &[u8]) -> u64 {
        let mut h = Self::new();
        h.bytes(bytes);
        h.finish()
    }

    pub fn bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(PRIME);
        }
    }

    /// Integers hash as little-endian bytes, as they are saved.
    pub fn u16(&mut self, v: u16) {
        self.bytes(&v.to_le_bytes());
    }
    pub fn u32(&mut self, v: u32) {
        self.bytes(&v.to_le_bytes());
    }
    pub fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }

    pub fn finish(&self) -> u64 {
        self.0
    }
}
