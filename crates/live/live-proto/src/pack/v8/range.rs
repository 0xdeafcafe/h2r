//! Twelve-bit adaptive symbol frequencies and forward byte-normalized range I/O.
use super::*;
const SCALE: u32 = 4096;
const K: usize = 16;

#[derive(Clone)]
struct Distribution {
    count: [u16; K],
    end: [u16; K],
    seen: u32,
    total: u16,
}
impl Default for Distribution {
    fn default() -> Self {
        let mut d = Self {
            count: [16, 3, 3, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1],
            end: [0; K],
            seen: 0,
            total: 37,
        };
        d.total = d.count.iter().sum();
        d.rebuild();
        d
    }
}
impl Distribution {
    fn rebuild(&mut self) {
        let mut mass = 0;
        let mut peak = 0;
        for (i, &count) in self.count.iter().enumerate() {
            let f = 1 + u32::from(count) * (SCALE - K as u32) / u32::from(self.total);
            mass += f;
            self.end[i] = mass as u16;
            if count > self.count[peak] {
                peak = i;
            }
        }
        for end in &mut self.end[peak..] {
            *end += (SCALE - mass) as u16;
        }
    }
    #[inline]
    fn observe(&mut self, s: usize) {
        self.count[s] += 1;
        self.total += 1;
        self.seen = self.seen.wrapping_add(1);
        if self.seen & 127 == 0 || (self.seen < 128 && self.seen.is_power_of_two()) {
            if self.total >= 2048 {
                self.total = 0;
                for n in &mut self.count {
                    *n = n.div_ceil(2);
                    self.total += *n;
                }
            }
            self.rebuild();
        }
    }
    #[inline]
    fn interval(&self, s: usize) -> Span {
        let start = if s == 0 { 0 } else { self.end[s - 1] };
        Span::new(start, self.end[s] - start)
    }
}
#[derive(Clone, Copy, Default)]
struct Span {
    start: u16,
    freq: u16,
}
impl Span {
    #[inline]
    fn new(start: u16, freq: u16) -> Self {
        Self { start, freq }
    }
}
pub(super) struct Tables {
    binary: Vec<u16>,
    ints: Vec<Distribution>,
}
impl Default for Tables {
    fn default() -> Self {
        Self {
            binary: vec![2048; META + 256],
            ints: vec![Distribution::default(); (META + 256) / 32],
        }
    }
}

trait Engine {
    const WRITE: bool;
    fn slot(&mut self) -> u16;
    fn invalid(&mut self);
    fn advance(&mut self, span: Span);
}
pub(super) struct Tape<'a, E> {
    pub(super) engine: E,
    pub(super) t: &'a mut Tables,
}
impl<E: Engine> Tape<'_, E> {
    #[inline]
    fn uniform(&mut self, bits: u32, want: u64) -> u64 {
        let shift = 12 - bits;
        let sym = if E::WRITE {
            want as u16
        } else {
            self.engine.slot() >> shift
        };
        self.engine.advance(Span::new(sym << shift, 1 << shift));
        u64::from(sym)
    }
}
impl<E: Engine> Symbols for Tape<'_, E> {
    const WRITING: bool = E::WRITE;
    #[inline]
    fn bit(&mut self, ctx: usize, want: bool) -> bool {
        let p = self.t.binary[ctx];
        let one = if E::WRITE {
            want
        } else {
            self.engine.slot() >= p
        };
        let span = if one {
            Span::new(p, SCALE as u16 - p)
        } else {
            Span::new(0, p)
        };
        self.engine.advance(span);
        self.t.binary[ctx] = if one {
            (p - p.div_ceil(32)).max(1)
        } else {
            (p + (4096 - p).div_ceil(32)).min(4095)
        };
        one
    }
    #[inline]
    fn uint(&mut self, ctx: usize, want: u64) -> u64 {
        let d = &mut self.t.ints[ctx >> 5];
        let s = if E::WRITE {
            if want < 8 {
                want as usize
            } else {
                (5 + want.ilog2() as usize).min(15)
            }
        } else {
            let slot = self.engine.slot();
            d.end.partition_point(|&end| end <= slot)
        };
        self.engine.advance(d.interval(s));
        d.observe(s);
        match s {
            0..=7 => s as u64,
            8..=14 => {
                let k = s as u32 - 5;
                let high = 1u64 << k;
                high + self.uniform(k, want & (high - 1))
            }
            _ => {
                let mut v = want.saturating_sub(1024);
                let mut n = 0;
                for shift in (0..63).step_by(7) {
                    let b = self.uniform(8, (v & 127) + 128 * u64::from(v >= 128));
                    n |= (b & 127) << shift;
                    if b < 128 {
                        if n > 1 << 52 {
                            self.engine.invalid();
                            return 0;
                        }
                        return n + 1024;
                    }
                    v >>= 7;
                }
                self.engine.invalid();
                0
            }
        }
    }
    #[inline]
    fn literal(&mut self, v: u8) -> u8 {
        self.uniform(8, u64::from(v)) as u8
    }
}

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
    fn invalid(&mut self) {
        unreachable!("encoder uses bounded integers")
    }
    #[inline]
    fn slot(&mut self) -> u16 {
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
    valid: bool,
}
impl<'a> ByteReader<'a> {
    pub(super) fn new(bytes: &'a [u8]) -> Result<Self> {
        if bytes.len() < 5 || bytes[0] != 0 {
            return Err(PackError::Bad("short range header"));
        }
        Ok(Self {
            valid: true,
            range: u32::MAX,
            code: u32::from_be_bytes(bytes[1..5].try_into().unwrap()),
            bytes: &bytes[5..],
        })
    }
    pub(super) fn finished(&self) -> Result<()> {
        if self.valid && self.bytes.is_empty() && self.code == 0 {
            Ok(())
        } else {
            Err(PackError::Bad("invalid range payload"))
        }
    }
}
impl Engine for ByteReader<'_> {
    const WRITE: bool = false;
    fn invalid(&mut self) {
        self.valid = false;
    }
    #[inline]
    fn slot(&mut self) -> u16 {
        let slot = self.code / (self.range >> 12);
        if slot >= SCALE {
            self.valid = false;
            return 0;
        }
        slot as u16
    }
    #[inline]
    fn advance(&mut self, span: Span) {
        let unit = self.range >> 12;
        self.code -= unit * u32::from(span.start);
        self.range = unit * u32::from(span.freq);
        while self.range < BOTTOM {
            self.range <<= 8;
            let (&b, tail) = self.bytes.split_first().unwrap_or_else(|| {
                self.valid = false;
                (&0, &[])
            });
            self.code = self.code << 8 | u32::from(b);
            self.bytes = tail;
        }
    }
}
