use super::*;
use std::{
    hint::black_box,
    time::{Duration, Instant},
};

#[path = "ans.rs"]
mod ans;
#[path = "../pack/v8/projection.rs"]
mod projection;

trait Codec: Default {
    fn send(&mut self, f: &Frame, key: bool) -> Vec<u8>;
    fn receive(&mut self, p: &[u8]) -> Frame;
    fn save(&self) -> Vec<u8>;
    fn load(&mut self, p: &[u8]);
}
macro_rules! codec {
    ($t:ty) => {
        impl Codec for $t {
            fn send(&mut self, f: &Frame, k: bool) -> Vec<u8> {
                self.encode(f, k)
            }
            fn receive(&mut self, p: &[u8]) -> Frame {
                self.decode(p)
            }
            fn save(&self) -> Vec<u8> {
                self.snapshot()
            }
            fn load(&mut self, p: &[u8]) {
                self.resume(p)
            }
        }
    };
}
codec!(Connection);
codec!(ans::Ans<false,false>);
codec!(ans::Ans<true,false>);
codec!(ans::Ans<true,true>);
type ByteSymbols = ans::Ans<true, true, true, false, false>;
type ByteBits = ans::Ans<true, true, true, true, false>;
type PackedKey = ans::Ans<true, true, true, false, true>;
codec!(ByteSymbols);
codec!(ByteBits);
codec!(PackedKey);
type HotSymbols = ans::Ans<true, true, true, false, true, true>;
type HotBits = ans::Ans<true, true, true, true, true, true>;
codec!(HotSymbols);
codec!(HotBits);
fn check<C: Codec>(frames: &[Frame]) {
    let mut tx = C::default();
    let mut rx = C::default();
    for (i, f) in frames.iter().enumerate() {
        let key = i % 17 == 0 || f.cut.is_some();
        if key {
            rx = C::default();
        }
        if i % 11 == 5 {
            rx = C::default();
            rx.load(&tx.save());
        }
        let packet = tx.send(f, key);
        assert_eq!(rx.receive(&packet), quantize(f).unwrap(), "sample {i}");
    }
}
pub fn verify() {
    for scene in [
        crate::samples::multiplayer(200),
        crate::samples::campaign(200),
    ] {
        check::<Connection>(&scene);
        check::<ans::Ans<false, false>>(&scene);
        check::<ans::Ans<true, false>>(&scene);
        check::<ans::Ans<true, true>>(&scene);
        check::<ByteSymbols>(&scene);
        check::<ByteBits>(&scene);
        check::<PackedKey>(&scene);
        check::<HotSymbols>(&scene);
        check::<HotBits>(&scene);
    }
}
fn timed<T>(d: &mut Duration, f: impl FnOnce() -> T) -> T {
    let t = Instant::now();
    let result = f();
    *d += t.elapsed();
    result
}
fn measure<C: Codec>(frames: &[Frame], name: &str) {
    let mut enc = C::default();
    let mut dec = C::default();
    let mut v7 = Encoder::new(0, 150);
    let mut buf = Vec::new();
    let mut bytes = 0;
    let mut base_bytes = 0;
    let mut join = 0;
    let mut t = Duration::ZERO;
    let mut c = Duration::ZERO;
    for (i, f) in frames.iter().enumerate() {
        let mut control = || {
            base_bytes += timed(&mut c, || {
                let (rec, _) = v7.encode(f).unwrap();
                buf.clear();
                push_record(&mut buf, &rec);
                black_box(&buf);
                buf.len()
            });
        };
        if i % 2 == 0 {
            control();
        }
        if i == 4771 {
            let saved = enc.save();
            join = saved.len();
            dec = C::default();
            dec.load(&saved);
        }
        let p = timed(&mut t, || enc.send(f, i % 150 == 0));
        if i % 2 == 1 {
            control();
        }
        if i % 150 == 0 {
            dec = C::default();
        }
        assert_eq!(dec.receive(&p), quantize(f).unwrap());
        bytes += p.len();
    }
    assert_eq!(base_bytes, 1_394_368);
    println!(
        "{name:24} {bytes:9} {:8.3} {:9.3} {:9.3} {:7.3} {join:9}",
        bytes as f64 / 9600.,
        us(t, frames.len()),
        us(c, frames.len()),
        t.as_secs_f64() / c.as_secs_f64()
    );
}
fn us(t: Duration, n: usize) -> f64 {
    t.as_secs_f64() * 1e6 / n as f64
}
pub fn compare() {
    let frames = crate::samples::multiplayer(9000);
    println!("10 s keys; 9,000 complete framed packets. CPU microseconds per 16-player frame.");
    println!("variant                   bytes       B/p/s  candidate    v7-prod   ratio    join_B");
    measure::<Connection>(&frames, "binary arithmetic");
    measure::<ans::Ans<false, false>>(&frames, "symbol rANS");
    measure::<ans::Ans<true, false>>(&frames, "rANS + projection cache");
    measure::<ans::Ans<true, true>>(&frames, "rANS + sparse groups");
    measure::<ByteSymbols>(&frames, "byte range symbols");
    measure::<ByteBits>(&frames, "byte range bits");
    measure::<PackedKey>(&frames, "byte symbols + v7 key");
    measure::<HotSymbols>(&frames, "hot symbols + v7 key");
    measure::<HotBits>(&frames, "hot bits + v7 key");
}

#[derive(Clone, Copy)]
enum Op {
    Bool(usize, bool),
    Uint(usize, u64),
    Byte(u8),
}
impl Symbols for Vec<Op> {
    const WRITING: bool = true;
    fn bit(&mut self, c: usize, b: bool) -> bool {
        self.push(Op::Bool(c, b));
        b
    }
    fn uint(&mut self, c: usize, v: u64) -> u64 {
        self.push(Op::Uint(c, v));
        v
    }
    fn literal(&mut self, b: u8) -> u8 {
        self.push(Op::Byte(b));
        b
    }
}
fn expand(ops: &[Op], bits: &mut Vec<(u16, bool)>) {
    for op in ops {
        match *op {
            Op::Bool(c, b) => bits.push((c as u16, b)),
            Op::Byte(v) => {
                for i in (0..8).rev() {
                    bits.push((u16::MAX, v >> i & 1 != 0));
                }
            }
            Op::Uint(c, v) => {
                let x = v + 1;
                let n = x.ilog2();
                for i in 0..=n {
                    bits.push(((c + i.min(15) as usize) as u16, i < n));
                }
                for i in (0..n).rev() {
                    bits.push(((c + 16 + i.min(15) as usize) as u16, x >> i & 1 != 0));
                }
            }
        }
    }
}
// Allocation callback is enabled for projection/traversal only, in a separate
// executable. It has zero overhead in the acceptance timing binary.
pub fn profile(mut allocations: impl FnMut(bool) -> (u64, u64)) {
    let frames = crate::samples::multiplayer(9000);
    let mut h = History::default();
    let mut probs = vec![2048u16; META + 256];
    let mut ops = Vec::new();
    let mut bits = Vec::new();
    let mut decisions = Vec::new();
    let mut stages = [Duration::ZERO; 5];
    let mut allocs = [(0u64, 0u64); 2];
    let mut symbols = 0;
    let mut choices = 0;
    let mut bytes = 0;
    for (i, f) in frames.iter().enumerate() {
        if i % 150 == 0 {
            h = History::default();
            probs.fill(2048);
        }
        let dt = f.tick - h.tick;
        allocations(true);
        let src = timed(&mut stages[0], || Lists::of(f).unwrap());
        let (n, b) = allocations(false);
        allocs[0].0 += n;
        allocs[0].1 += b;
        ops.clear();
        bits.clear();
        decisions.clear();
        allocations(true);
        timed(&mut stages[1], || h.write_frame(&mut ops, f, &src, dt));
        let (n, b) = allocations(false);
        allocs[1].0 += n;
        allocs[1].1 += b;
        h.tick = f.tick;
        timed(&mut stages[2], || expand(&ops, &mut bits));
        timed(&mut stages[3], || {
            for &(ctx, b) in &bits {
                let p = if ctx == u16::MAX {
                    2048
                } else {
                    let slot = &mut probs[usize::from(ctx)];
                    let p = *slot;
                    *slot = if b {
                        (p - p.div_ceil(32)).max(1)
                    } else {
                        (p + (4096 - p).div_ceil(32)).min(4095)
                    };
                    p
                };
                decisions.push((p, b));
            }
        });
        bytes += timed(&mut stages[4], || {
            let mut out = Coder::new(Output::default(), &mut []);
            for &(p, b) in &decisions {
                out.profiled_split(p, b);
            }
            out.finish().len()
        });
        symbols += ops.len();
        choices += bits.len();
    }
    for (i, (name, t)) in [
        "quantize/allocate lists",
        "columns/dictionary -> symbols",
        "expand integer codes -> bits",
        "probability updates -> intervals",
        "range/output only",
    ]
    .into_iter()
    .zip(stages)
    .enumerate()
    {
        println!("{name:34} {:8.3} us/frame", us(t, frames.len()));
        if let Some((n, b)) = allocs.get(i) {
            println!(
                "  {:.2} allocations, {:.1} allocated B/frame",
                *n as f64 / 9000.,
                *b as f64 / 9000.
            );
        }
    }
    println!(
        "{:.1} symbols / {:.1} binary choices per sample; {bytes} arithmetic payload bytes. Isolated stages materialize intermediate buffers; use paired timings for the total.",
        symbols as f64 / 9000.,
        choices as f64 / 9000.
    );
}
