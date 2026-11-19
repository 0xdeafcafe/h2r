//! One byte-renormalized rANS state per packet. Collect intervals forward so
//! the causal encoder and decoder learn in the same order; emit in reverse.
use super::*;
const SCALE: u32 = 4096;
const LOW: u32 = 1 << 23;
const K: usize = 16;

#[derive(Clone, Serialize, Deserialize)]
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
        self.seen += 1;
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
#[derive(Serialize, Deserialize)]
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
    fn slot(&self) -> u16;
    fn advance(&mut self, span: Span);
}
struct Writer<'a>(&'a mut Vec<Span>);
impl Engine for Writer<'_> {
    const WRITE: bool = true;
    #[inline]
    fn slot(&self) -> u16 {
        0
    }
    #[inline]
    fn advance(&mut self, span: Span) {
        self.0.push(span);
    }
}
struct Reader<'a> {
    state: u32,
    bytes: &'a [u8],
}
impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        let (head, bytes) = bytes.split_at(4);
        Self {
            state: u32::from_le_bytes(head.try_into().unwrap()),
            bytes,
        }
    }
    fn finished(&self) {
        assert_eq!(self.state, LOW);
        assert!(self.bytes.is_empty());
    }
}
impl Engine for Reader<'_> {
    const WRITE: bool = false;
    #[inline]
    fn slot(&self) -> u16 {
        (self.state & (SCALE - 1)) as u16
    }
    #[inline]
    fn advance(&mut self, span: Span) {
        self.state =
            (self.state >> 12) * u32::from(span.freq) + u32::from(self.slot() - span.start);
        while self.state < LOW {
            let (&b, tail) = self.bytes.split_first().expect("truncated rANS frame");
            self.state = self.state << 8 | u32::from(b);
            self.bytes = tail;
        }
    }
}
fn finish(spans: &[Span], out: &mut Vec<u8>) {
    let at = out.len();
    let mut state = LOW;
    for &Span { start, freq } in spans.iter().rev() {
        let f = u32::from(freq);
        while state >= ((LOW >> 12) << 8) * f {
            out.push(state as u8);
            state >>= 8;
        }
        let div = state / f;
        state = (div << 12) + (state - div * f) + u32::from(start);
    }
    out[at..].reverse();
    out.splice(at..at, state.to_le_bytes());
}

struct Tape<'a, E> {
    engine: E,
    t: &'a mut Tables,
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
                        return n + 1024;
                    }
                    v >>= 7;
                }
                panic!("oversize research integer")
            }
        }
    }
    #[inline]
    fn literal(&mut self, v: u8) -> u8 {
        self.uniform(8, u64::from(v)) as u8
    }
}

#[derive(Default)]
pub(super) struct Ans {
    pub(super) h: History,
    tables: Tables,
    spans: Vec<Span>,
}
impl Ans {
    pub(super) fn encode(&mut self, f: &Frame, key: bool) -> Vec<u8> {
        let seq = self.h.seq;
        if key {
            self.h = History::default();
            self.tables = Tables::default();
        }
        let dt = f.tick - self.h.tick;
        let src = Lists::of(f).unwrap();
        self.spans.clear();
        let mut r = Tape {
            engine: Writer(&mut self.spans),
            t: &mut self.tables,
        };
        self.h.write_frame(&mut r, f, &src, dt);
        self.h.tick = f.tick;
        self.h.seq = seq + 1;
        let mut payload = Vec::new();
        var(&mut payload, seq * 2 + u64::from(key));
        var(&mut payload, dt);
        finish(&self.spans, &mut payload);
        let mut packet = Vec::new();
        push_record(&mut packet, &payload);
        packet
    }
    pub(super) fn decode(&mut self, packet: &[u8]) -> Frame {
        let mut records = records(packet);
        let mut body = records.next().unwrap().unwrap();
        assert!(records.next().is_none());
        let tag = number(&mut body);
        let seq = tag / 2;
        if tag & 1 != 0 {
            self.h = History::default();
            self.tables = Tables::default();
        } else {
            assert_eq!(self.h.seq, seq);
        }
        let dt = number(&mut body);
        let mut r = Tape {
            engine: Reader::new(body),
            t: &mut self.tables,
        };
        let f = self.h.read_frame(&mut r, dt, seq);
        r.engine.finished();
        f
    }
    pub(super) fn snapshot(&self) -> Vec<u8> {
        let mut z = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
        serde_json::to_writer(&mut z, &(&self.h, &self.tables)).unwrap();
        z.finish().unwrap()
    }
    pub(super) fn resume(&mut self, bytes: &[u8]) {
        (self.h, self.tables) =
            serde_json::from_reader(flate2::read::DeflateDecoder::new(bytes)).unwrap();
        self.h.ids = self
            .h
            .strings
            .iter()
            .cloned()
            .enumerate()
            .map(|(i, v)| (v, i))
            .collect();
    }
}
