//! How far the f64 transcendentals (`set_f64_transcendentals`, boot's
//! `H2_X87_FAST_TRANSCENDENTALS=1`) land from the correctly rounded series,
//! over a sweep of synthetic operands: the distribution of the difference
//! in units of the 64-bit result's last place. Its own test binary, since
//! the switch is process-wide.
//!
//! `cargo test -p x87 --release --test f64_trans -- --nocapture`

use x87::{F80, Flags};

/// A value as a signed integer in units of `unit_exp`'s last place.
fn scaled(v: F80, unit_exp: i32) -> i128 {
    let exp = (v.se & 0x7fff) as i32;
    let sig = v.sig as i128;
    let shift = exp - unit_exp;
    let mag = if shift >= 0 { sig << shift.min(60) } else { sig >> (-shift).min(127) };
    if v.se & 0x8000 != 0 { -mag } else { mag }
}

/// |a - b| in units in the last place of `a` (the reference).
fn ulps(a: F80, b: F80) -> u128 {
    let e = (a.se & 0x7fff) as i32;
    (scaled(a, e) - scaled(b, e)).unsigned_abs()
}

/// A tiny deterministic generator (no test data from anywhere).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    /// Uniform in [lo, hi).
    fn uniform(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * ((self.next() >> 11) as f64 / (1u64 << 53) as f64)
    }
    /// A full 64-bit significand near `v`: the f64 operand's low 11 bits
    /// filled in, as an x87 value usually has them.
    fn widen(&mut self, v: f64) -> F80 {
        let f = F80::from_f64_bits(v.to_bits());
        F80 { sig: f.sig | (self.next() & 0x7ff), se: f.se }
    }
}

const BUCKETS: [(u128, &str); 8] = [
    (0, "0"),
    (1, "1"),
    (15, "2-15"),
    (255, "16-255"),
    (1023, "256-1023"),
    (2047, "1024-2047"),
    (4095, "2048-4095"),
    (u128::MAX, "4096+"),
];

struct Sweep {
    name: &'static str,
    diffs: Vec<u128>,
    flags_differ: usize,
    f64_path: usize,
}

impl Sweep {
    fn report(&mut self) -> u128 {
        self.diffs.sort_unstable();
        let n = self.diffs.len();
        let mut counts = [0usize; BUCKETS.len()];
        for d in &self.diffs {
            counts[BUCKETS.iter().position(|(top, _)| d <= top).unwrap()] += 1;
        }
        let hist: Vec<String> = BUCKETS
            .iter()
            .zip(counts)
            .filter(|(_, c)| *c != 0)
            .map(|((_, name), c)| format!("{name}: {:.2}%", 100.0 * c as f64 / n as f64))
            .collect();
        let max = *self.diffs.last().unwrap();
        println!(
            "{}: {n} operands ({} on the f64 path), ulps of the 64-bit result: median {}, p99 {}, max {} (~2^{:.1}); {}; flags other than PE/C1 differ {}",
            self.name,
            self.f64_path,
            self.diffs[n / 2],
            self.diffs[n * 99 / 100],
            max,
            (max.max(1) as f64).log2(),
            hist.join(", "),
            self.flags_differ
        );
        max
    }
}

fn sweep(name: &'static str, n: usize, mut operand: impl FnMut(&mut Rng) -> (F80, F80), f: impl Fn(F80, F80) -> (F80, Flags)) -> Sweep {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ name.len() as u64);
    let mut s = Sweep { name, diffs: Vec::with_capacity(n), flags_differ: 0, f64_path: 0 };
    let keep = Flags(!(Flags::PE.0 | Flags::C1.0));
    for _ in 0..n {
        let (a, b) = operand(&mut rng);
        x87::set_f64_transcendentals(false);
        let want = f(a, b);
        x87::set_f64_transcendentals(true);
        let got = f(a, b);
        x87::set_f64_transcendentals(false);
        if got != want {
            s.f64_path += 1;
        }
        s.diffs.push(ulps(want.0, got.0));
        if Flags(want.1.0 & keep.0) != Flags(got.1.0 & keep.0) {
            s.flags_differ += 1;
        }
    }
    s
}

#[test]
fn f64_transcendentals_against_the_series() {
    let n: usize = std::env::var("H2_X87_SWEEP").ok().and_then(|v| v.parse().ok()).unwrap_or(20_000);
    // Angles as a game uses them (|x| < 8, sin and cos of headings and
    // phases), and large ones, where the operand's rounding to 53 bits is
    // itself an angle error of up to |x| * 2^-53.
    let small = |r: &mut Rng| {
        let v = r.uniform(-8.0, 8.0);
        (r.widen(v), F80::ONE)
    };
    let large = |r: &mut Rng| {
        let v = r.uniform(-1.0e6, 1.0e6);
        (r.widen(v), F80::ONE)
    };
    let sin = |a: F80, _| {
        let t = x87::sin(a);
        (t.value, t.flags)
    };
    let cos = |a: F80, _| {
        let t = x87::cos(a);
        (t.value, t.flags)
    };
    let mut sweeps = vec![
        sweep("fsin |x|<8", n, small, sin),
        sweep("fsin |x|<1e6", n, large, sin),
        sweep("fcos |x|<8", n, small, cos),
        sweep("fcos |x|<1e6", n, large, cos),
        sweep(
            "fyl2x",
            n,
            |r| {
                let x = libm_exp2(r.uniform(-20.0, 20.0));
                let y = if r.next() % 2 == 0 { 1.0 } else { r.uniform(-4.0, 4.0) };
                (r.widen(y), r.widen(x))
            },
            |y, x| x87::yl2x(y, x),
        ),
        sweep(
            "f2xm1",
            n,
            |r| {
                let v = r.uniform(-1.0, 1.0);
                (r.widen(v), F80::ONE)
            },
            |a, _| x87::x2m1(a),
        ),
    ];
    for s in &mut sweeps {
        // Reported, not bounded: near a zero of sin or cos the relative
        // error is unbounded (the operand alone moves the result), which is
        // what the distribution is for.
        s.report();
        assert_eq!(s.flags_differ, 0, "{}: exceptions other than PE differ", s.name);
        assert!(s.f64_path * 2 > s.diffs.len(), "{}: the f64 path was rarely taken", s.name);
    }
}

/// 2^v without libm in the test (std's).
fn libm_exp2(v: f64) -> f64 {
    v.exp2()
}
