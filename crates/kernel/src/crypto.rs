//! XcSHAInit / XcSHAUpdate / XcSHAFinal (335-337): SHA-1 as specified by
//! FIPS 180-4, written from the specification (no kernel code or tables).
//!
//! The context is the caller's opaque XC_SERVICE_SHA_CONTEXT_SIZE (116-byte)
//! buffer. Its layout here is private to this kernel; callers only pass it
//! back, or copy it whole. Bytes used, little-endian:
//!
//! | Offset | Size | Field |
//! | ---: | ---: | --- |
//! | 0 | 20 | h0..h4 |
//! | 20 | 8 | message length in bytes |
//! | 28 | 64 | partial block |
//! | 92 | 4 | partial block length (0..63) |
use crate::{
    Action, Core,
    abi::{Args, Value},
    ord,
};
use runtime::{Guest, GuestFault, guest::Access};

/// XC_SERVICE_SHA_CONTEXT_SIZE. The state uses its first `STATE_SIZE` bytes.
pub const SHA_CONTEXT_SIZE: u32 = 116;
const STATE_SIZE: u32 = 96;
const INITIAL: [u32; 5] = [
    0x6745_2301,
    0xefcd_ab89,
    0x98ba_dcfe,
    0x1032_5476,
    0xc3d2_e1f0,
];

/// Incremental SHA-1, usable on the host as well (tests, tools).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sha1 {
    h: [u32; 5],
    length: u64,
    block: [u8; 64],
    filled: usize,
}
impl Default for Sha1 {
    fn default() -> Self {
        Self {
            h: INITIAL,
            length: 0,
            block: [0; 64],
            filled: 0,
        }
    }
}
impl Sha1 {
    pub fn new() -> Self {
        Self::default()
    }
    fn compress(h: &mut [u32; 5], block: &[u8; 64]) {
        let mut w = [0u32; 80];
        for (t, word) in block.as_chunks::<4>().0.iter().enumerate() {
            w[t] = u32::from_be_bytes(*word);
        }
        for t in 16..80 {
            w[t] = (w[t - 3] ^ w[t - 8] ^ w[t - 14] ^ w[t - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = *h;
        for (t, &wt) in w.iter().enumerate() {
            let (f, k) = match t {
                0..=19 => ((b & c) | (!b & d), 0x5a82_7999),
                20..=39 => (b ^ c ^ d, 0x6ed9_eba1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1b_bcdc),
                _ => (b ^ c ^ d, 0xca62_c1d6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wt);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    pub fn update(&mut self, mut data: &[u8]) {
        self.length = self.length.wrapping_add(data.len() as u64);
        while !data.is_empty() {
            let n = (64 - self.filled).min(data.len());
            self.block[self.filled..self.filled + n].copy_from_slice(&data[..n]);
            self.filled += n;
            data = &data[n..];
            if self.filled == 64 {
                Self::compress(&mut self.h, &self.block);
                self.filled = 0;
            }
        }
    }
    pub fn finish(mut self) -> [u8; 20] {
        let bits = self.length.wrapping_mul(8);
        self.update(&[0x80]);
        while self.filled != 56 {
            self.update(&[0]);
        }
        self.update(&bits.to_be_bytes());
        debug_assert_eq!(self.filled, 0);
        let mut digest = [0; 20];
        for (out, word) in digest.as_chunks_mut::<4>().0.iter_mut().zip(self.h) {
            out.copy_from_slice(&word.to_be_bytes());
        }
        digest
    }
    fn load(memory: &Guest, context: u32) -> Result<Option<Self>, GuestFault> {
        let mut bytes = [0u8; STATE_SIZE as usize];
        memory.read_bytes(context, &mut bytes, 1)?;
        let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let filled = word(92) as usize;
        if filled >= 64 {
            return Ok(None);
        }
        Ok(Some(Self {
            h: [word(0), word(4), word(8), word(12), word(16)],
            length: u64::from_le_bytes(bytes[20..28].try_into().unwrap()),
            block: bytes[28..92].try_into().unwrap(),
            filled,
        }))
    }
    fn store(&self, memory: &mut Guest, context: u32) -> Result<(), GuestFault> {
        let mut bytes = [0u8; STATE_SIZE as usize];
        for (i, word) in self.h.iter().enumerate() {
            bytes[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }
        bytes[20..28].copy_from_slice(&self.length.to_le_bytes());
        bytes[28..92].copy_from_slice(&self.block);
        bytes[92..96].copy_from_slice(&(self.filled as u32).to_le_bytes());
        memory.write_bytes(context, &bytes, 1)
    }
}

impl Core {
    pub(crate) fn sha_call(&mut self, memory: &mut Guest, a: &Args) -> Result<Action, GuestFault> {
        let context = a.get(0);
        match a.export.ordinal {
            ord::XC_SHA_INIT => {
                memory.check(context, STATE_SIZE as usize, Access::Write, 1)?;
                Sha1::new().store(memory, context)?;
            }
            ord::XC_SHA_UPDATE => {
                let (input, length) = (a.get(1), a.get(2));
                memory.check(context, STATE_SIZE as usize, Access::Write, 1)?;
                memory.check(input, length as usize, Access::Read, 1)?;
                let Some(mut sha) = Sha1::load(memory, context)? else {
                    return Ok(Action::Stop("SHA context was not initialized by XcSHAInit"));
                };
                // Bounded host scratch, whatever the guest length.
                let mut chunk = [0u8; 4096];
                let mut done = 0u32;
                while done < length {
                    let n = (length - done).min(chunk.len() as u32);
                    memory.read_bytes(input + done, &mut chunk[..n as usize], 1)?;
                    sha.update(&chunk[..n as usize]);
                    done += n;
                }
                sha.store(memory, context)?;
            }
            ord::XC_SHA_FINAL => {
                let digest = a.get(1);
                memory.check(context, STATE_SIZE as usize, Access::Write, 1)?;
                memory.check(digest, 20, Access::Write, 1)?;
                let Some(sha) = Sha1::load(memory, context)? else {
                    return Ok(Action::Stop("SHA context was not initialized by XcSHAInit"));
                };
                memory.write_bytes(digest, &sha.finish(), 1)?;
            }
            _ => return Ok(Action::Stop("declared export has no handler")),
        }
        Ok(Action::Complete(Value::Void))
    }
}

#[cfg(test)]
mod tests {
    use super::Sha1;
    fn hex(digest: [u8; 20]) -> String {
        digest.iter().map(|b| format!("{b:02x}")).collect()
    }
    fn sha(data: &[u8]) -> String {
        let mut s = Sha1::new();
        s.update(data);
        hex(s.finish())
    }
    #[test]
    fn fips_180_vectors() {
        assert_eq!(sha(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(sha(b""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(
            sha(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
        let mut s = Sha1::new();
        for _ in 0..1000 {
            s.update(&[b'a'; 1000]);
        }
        assert_eq!(hex(s.finish()), "34aa973cd4c4daa4f61eeb2bdbad27316534016f");
    }
    #[test]
    fn split_updates_match_one_update() {
        let data: Vec<u8> = (0..300u32).map(|i| (i * 7) as u8).collect();
        for split in [0, 1, 55, 56, 63, 64, 65, 200] {
            let mut s = Sha1::new();
            s.update(&data[..split]);
            s.update(&data[split..]);
            assert_eq!(hex(s.finish()), sha(&data), "split {split}");
        }
    }
}
