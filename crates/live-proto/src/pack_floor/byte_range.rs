//! Forward byte-normalized range coder; identical 12-bit frequency intervals
//! to rANS. Range multiplication only on the encoder, no token buffer.
use super::{Engine, Span};
const BOTTOM: u32 = 1 << 24;
pub(super) struct ByteWriter<'a> {
    out: &'a mut Vec<u8>,
    low: u64,
    range: u32,
    cache: u8,
    pending: usize,
}
impl<'a> ByteWriter<'a> {
    pub(super) fn new(out: &'a mut Vec<u8>) -> Self {
        Self {
            out,
            low: 0,
            range: u32::MAX,
            cache: 0,
            pending: 1,
        }
    }
    #[inline]
    fn shift(&mut self) {
        let l = self.low as u32;
        let carry = (self.low >> 32) as u8;
        if l < 0xff00_0000 || carry != 0 {
            self.out.push(self.cache.wrapping_add(carry));
            if self.pending > 1 {
                self.out.resize(
                    self.out.len() + self.pending - 1,
                    0xffu8.wrapping_add(carry),
                );
            }
            self.pending = 0;
            self.cache = (l >> 24) as u8;
        }
        self.pending += 1;
        self.low = u64::from(l << 8);
    }
    pub(super) fn finish(mut self) {
        for _ in 0..5 {
            self.shift();
        }
    }
}
impl Engine for ByteWriter<'_> {
    const WRITE: bool = true;
    #[inline]
    fn slot(&self) -> u16 {
        0
    }
    #[inline]
    fn advance(&mut self, span: Span) {
        let unit = self.range >> 12;
        self.low += u64::from(unit * u32::from(span.start));
        self.range = unit * u32::from(span.freq);
        while self.range < BOTTOM {
            self.range <<= 8;
            self.shift();
        }
    }
}
pub(super) struct ByteReader<'a> {
    range: u32,
    code: u32,
    bytes: &'a [u8],
}
impl<'a> ByteReader<'a> {
    pub(super) fn new(bytes: &'a [u8]) -> Self {
        assert_eq!(bytes[0], 0);
        Self {
            range: u32::MAX,
            code: u32::from_be_bytes(bytes[1..5].try_into().unwrap()),
            bytes: &bytes[5..],
        }
    }
    pub(super) fn finished(&self) {
        assert!(self.bytes.is_empty());
        assert_eq!(self.code, 0);
    }
}
impl Engine for ByteReader<'_> {
    const WRITE: bool = false;
    #[inline]
    fn slot(&self) -> u16 {
        (self.code / (self.range >> 12)) as u16
    }
    #[inline]
    fn advance(&mut self, span: Span) {
        let unit = self.range >> 12;
        self.code -= unit * u32::from(span.start);
        self.range = unit * u32::from(span.freq);
        while self.range < BOTTOM {
            self.range <<= 8;
            let (&b, tail) = self.bytes.split_first().expect("truncated range frame");
            self.code = self.code << 8 | u32::from(b);
            self.bytes = tail;
        }
    }
}
