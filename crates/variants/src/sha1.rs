//! SHA-1 (FIPS 180-4) and HMAC-SHA1 (RFC 2104), written from the
//! specifications. Saved variants carry an HMAC-SHA1 signature
//! ([`crate::save`]); this keeps the crate free of the kernel's HLE.

const INITIAL: [u32; 5] = [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476, 0xc3d2_e1f0];
const BLOCK: usize = 64;

/// Incremental SHA-1.
#[derive(Clone)]
pub struct Sha1 {
    h: [u32; 5],
    length: u64,
    block: [u8; BLOCK],
    filled: usize,
}

impl Default for Sha1 {
    fn default() -> Self {
        Self { h: INITIAL, length: 0, block: [0; BLOCK], filled: 0 }
    }
}

impl Sha1 {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.length += data.len() as u64;
        while !data.is_empty() {
            let take = (BLOCK - self.filled).min(data.len());
            self.block[self.filled..self.filled + take].copy_from_slice(&data[..take]);
            self.filled += take;
            data = &data[take..];
            if self.filled == BLOCK {
                let block = self.block;
                self.compress(&block);
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
        let mut out = [0; 20];
        for (o, h) in out.chunks_exact_mut(4).zip(self.h) {
            o.copy_from_slice(&h.to_be_bytes());
        }
        out
    }

    fn compress(&mut self, block: &[u8; BLOCK]) {
        let mut w = [0u32; 80];
        for (i, c) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes(c.try_into().unwrap());
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = self.h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..20 => ((b & c) | (!b & d), 0x5a82_7999),
                20..40 => (b ^ c ^ d, 0x6ed9_eba1),
                40..60 => ((b & c) | (b & d) | (c & d), 0x8f1b_bcdc),
                _ => (b ^ c ^ d, 0xca62_c1d6),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (h, v) in self.h.iter_mut().zip([a, b, c, d, e]) {
            *h = h.wrapping_add(v);
        }
    }
}

/// HMAC-SHA1 of `data` with a key of at most one block.
pub fn hmac_sha1(key: &[u8], data: &[u8]) -> [u8; 20] {
    assert!(key.len() <= BLOCK, "HMAC key longer than one block");
    let mut pad = [0u8; BLOCK];
    pad[..key.len()].copy_from_slice(key);
    let mut inner = Sha1::new();
    inner.update(&pad.map(|b| b ^ 0x36));
    inner.update(data);
    let digest = inner.finish();
    let mut outer = Sha1::new();
    outer.update(&pad.map(|b| b ^ 0x5c));
    outer.update(&digest);
    outer.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn sha1_matches_the_fips_examples() {
        let sha = |d: &[u8]| {
            let mut s = Sha1::new();
            s.update(d);
            hex(&s.finish())
        };
        assert_eq!(sha(b""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(sha(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(
            sha(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
        // Split updates give the same digest.
        let mut s = Sha1::new();
        for c in b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq".chunks(7) {
            s.update(c);
        }
        assert_eq!(hex(&s.finish()), "84983e441c3bd26ebaae4aa1f95129e5e54670f1");
    }

    #[test]
    fn hmac_matches_rfc_2202() {
        assert_eq!(
            hex(&hmac_sha1(&[0x0b; 20], b"Hi There")),
            "b617318655057264e28bc0b6fb378c8ef146be00"
        );
        assert_eq!(
            hex(&hmac_sha1(b"Jefe", b"what do ya want for nothing?")),
            "effcdf6ae5eb2fa2d27416d5f184df9c259a7c79"
        );
    }
}
