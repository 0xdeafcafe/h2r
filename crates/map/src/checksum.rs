//! Retail `0x123110`: XOR complete little-endian qwords with a parity seed,
//! then fold the high dword into the low dword. A final 0..7 bytes is ignored.
//! The copy loop at `0x2141f0` starts at file +0x800 and uses physical EOF,
//! comparing the accumulated result with header +0x2d0 at `0x214616`.
use crate::{Error, Map, Result};
use std::io::{Read, Seek};

/// Incremental equivalent of the engine routine over one contiguous byte stream.
/// Chunk boundaries supplied to `update` do not affect the result. Engine copy
/// chunks are multiples of eight, except possibly the final chunk.
#[derive(Default, Debug, Clone)]
pub struct XorChecksum {
    value: u32,
    tail: [u8; 8],
    used: usize,
}
impl XorChecksum {
    /// Add bytes. Incomplete qwords are retained until the next call.
    pub fn update(&mut self, mut bytes: &[u8]) {
        if self.used != 0 {
            let n = (8 - self.used).min(bytes.len());
            self.tail[self.used..self.used + n].copy_from_slice(&bytes[..n]);
            self.used += n;
            bytes = &bytes[n..];
            if self.used == 8 {
                self.value ^= folded(self.tail);
                self.used = 0;
            }
        }
        let (words, tail) = bytes.as_chunks::<8>();
        for word in words {
            self.value ^= folded(*word);
        }
        if !tail.is_empty() {
            self.tail[..tail.len()].copy_from_slice(tail);
            self.used = tail.len();
        }
    }
    /// Finish, ignoring the final incomplete qword just as `size >> 3` does.
    pub fn finish(self) -> u32 {
        self.value
    }
}
fn folded(word: [u8; 8]) -> u32 {
    let word = u64::from_le_bytes(word);
    // Stack seed bytes at 0x123117..0x123133: 01 02 04 08 10 20 40 80.
    word as u32 ^ (word >> 32) as u32 ^ 0x8844_2211
}
/// Apply the engine routine to a buffer (excluding the map header).
pub fn engine_xor(bytes: &[u8]) -> u32 {
    let mut checksum = XorChecksum::default();
    checksum.update(bytes);
    checksum.finish()
}
impl<R: Read + Seek> Map<R> {
    /// Compute the copy-time checksum over the bounded physical file after +0x800.
    pub fn computed_checksum(&mut self) -> Result<u32> {
        let mut checksum = XorChecksum::default();
        let mut at = 0x800;
        while at < self.storage_length() {
            let n = (self.storage_length() - at).min(1024 * 1024);
            checksum.update(&self.read_storage_range(at, n)?);
            at += n;
        }
        Ok(checksum.finish())
    }
    /// Reject a checksum mismatch. Parsing alone remains useful for damaged maps.
    pub fn validate_checksum(&mut self) -> Result<()> {
        if self.computed_checksum()? != self.header().checksum {
            return Err(Error::Invalid("copy-time XOR checksum mismatch"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn engine_qword_seed_and_ignored_tail() {
        assert_eq!(engine_xor(&[]), 0);
        assert_eq!(engine_xor(&[0; 7]), 0);
        assert_eq!(engine_xor(&[0; 8]), 0x8844_2211);
        assert_eq!(engine_xor(&[0; 16]), 0);
        assert_eq!(engine_xor(&[1, 2, 4, 8, 16, 32, 64, 128]), 0);
        assert_eq!(engine_xor(&[1, 0, 0, 0, 2, 0, 0, 0, 99]), 0x8844_2212);
    }
    #[test]
    fn arbitrary_stream_boundaries_match_independent_qword_model() {
        let bytes: Vec<u8> = (0..113).collect();
        let expected = bytes.as_chunks::<8>().0.iter().fold(0, |v, c| {
            v ^ u32::from_le_bytes(c[..4].try_into().unwrap())
                ^ u32::from_le_bytes(c[4..].try_into().unwrap())
                ^ 0x8844_2211
        });
        for size in 1..=113 {
            let mut state = XorChecksum::default();
            for chunk in bytes.chunks(size) {
                state.update(chunk);
            }
            assert_eq!(state.finish(), expected, "chunk {size}");
        }
    }
}
