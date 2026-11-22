//! Packet-local binary arithmetic intervals, with connection-scoped adaptive
//! probabilities. E1/E2/E3 renormalization, 32 bits of precision.
use super::super::BitWriter;
const HALF: u64 = 1 << 31;
const Q: u64 = 1 << 30;
pub trait Bits {
    const WRITING: bool;
    fn choose(&mut self, cut: u64, want: bool) -> bool;
    fn shift(&mut self, offset: u64);
}
#[derive(Default)]
pub struct Output {
    writer: BitWriter,
    pending: usize,
}
impl Output {
    fn emit(&mut self, one: bool) {
        self.writer.bit(one);
        for _ in 0..self.pending {
            self.writer.bit(!one);
        }
        self.pending = 0;
    }
}
impl Bits for Output {
    const WRITING: bool = true;
    #[inline]
    fn choose(&mut self, _: u64, want: bool) -> bool {
        want
    }
    #[inline]
    fn shift(&mut self, offset: u64) {
        if offset == Q {
            self.pending += 1;
        } else {
            self.emit(offset == HALF);
        }
    }
}
pub struct Input<'a> {
    bytes: &'a [u8],
    pos: usize,
    code: u64,
}
impl<'a> Input<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        let mut s = Self {
            bytes,
            pos: 0,
            code: 0,
        };
        for _ in 0..32 {
            s.shift(0);
        }
        s
    }
}
impl Bits for Input<'_> {
    const WRITING: bool = false;
    #[inline]
    fn choose(&mut self, cut: u64, _: bool) -> bool {
        self.code >= cut
    }
    #[inline]
    fn shift(&mut self, offset: u64) {
        let bit = (self.bytes.get(self.pos / 8).copied().unwrap_or(0) >> (self.pos % 8)) & 1;
        self.code = 2 * (self.code - offset) + u64::from(bit);
        self.pos += 1;
    }
}
pub struct Coder<'a, T> {
    lo: u64,
    hi: u64,
    io: T,
    probs: &'a mut [u16],
}
impl<'a, T: Bits> Coder<'a, T> {
    pub fn new(io: T, probs: &'a mut [u16]) -> Self {
        Self {
            lo: 0,
            hi: u64::from(u32::MAX),
            io,
            probs,
        }
    }
    #[inline]
    fn split(&mut self, p: u64, want: bool) -> bool {
        let cut = self.lo + (((self.hi - self.lo + 1) * p) >> 12);
        let one = self.io.choose(cut, want);
        if one {
            self.lo = cut;
        } else {
            self.hi = cut - 1;
        }
        loop {
            let off = if self.hi < HALF {
                0
            } else if self.lo >= HALF {
                HALF
            } else if self.lo >= Q && self.hi < 3 * Q {
                Q
            } else {
                break;
            };
            self.io.shift(off);
            self.lo = 2 * (self.lo - off);
            self.hi = 2 * (self.hi - off) + 1;
        }
        one
    }
    #[inline]
    pub fn bit(&mut self, ctx: usize, want: bool) -> bool {
        let p = self.probs[ctx];
        let one = self.split(u64::from(p), want);
        self.probs[ctx] = if one {
            (p - p.div_ceil(32)).max(1)
        } else {
            (p + (4096 - p).div_ceil(32)).min(4095)
        };
        one
    }
    pub fn uint(&mut self, ctx: usize, want: u64) -> u64 {
        let x = want + 1;
        let bits = x.ilog2();
        let mut k = 0;
        while self.bit(ctx + k.min(15) as usize, k < bits) {
            k += 1;
            assert!(k <= 62, "bad prototype integer");
        }
        let mut n = 1;
        for j in (0..k).rev() {
            n = 2 * n + u64::from(self.bit(ctx + 16 + j.min(15) as usize, (x >> j) & 1 != 0));
        }
        n - 1
    }
    pub fn literal(&mut self, want: u8) -> u8 {
        (0..8).rev().fold(0, |n, i| {
            2 * n + u8::from(self.split(2048, want >> i & 1 != 0))
        })
    }
}
impl Coder<'_, Output> {
    pub fn finish(mut self) -> Vec<u8> {
        self.io.pending += 1;
        self.io.emit(self.lo >= Q);
        self.io.writer.finish()
    }
}

#[cfg(feature = "live-floor-bench")]
impl Coder<'_, Output> {
    /// Replay captured, already-modelled decisions for the isolated entropy
    /// backend cost. This path does not update or read a probability table.
    pub(super) fn profiled_split(&mut self, p: u16, b: bool) {
        self.split(u64::from(p), b);
    }
}
