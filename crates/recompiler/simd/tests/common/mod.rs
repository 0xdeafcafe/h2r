//! Shared test plumbing: the record protocol of `c/test/protocol.h`, the
//! Rust side's dispatch, input generators, and building and driving the C
//! harnesses.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const IN_SIZE: usize = 40;
pub const OUT_SIZE: usize = 24;

/// Op numbers, in `protocol.h` order.
pub const OPS: [&str; 28] = [
    "addss",
    "addps",
    "subss",
    "subps",
    "mulss",
    "mulps",
    "divss",
    "divps",
    "sqrtss",
    "sqrtps",
    "minss",
    "minps",
    "maxss",
    "maxps",
    "cmpss",
    "cmpps",
    "comiss",
    "ucomiss",
    "cvtsi2ss",
    "cvtss2si",
    "cvttss2si",
    "cvtpi2ps",
    "cvtps2pi",
    "cvttps2pi",
    "rcpss",
    "rcpps",
    "rsqrtss",
    "rsqrtps",
];
pub fn op(name: &str) -> u8 {
    OPS.iter().position(|&n| n == name).unwrap() as u8
}
pub fn is_approx(op: u8) -> bool {
    OPS[op as usize].starts_with("rcp") || OPS[op as usize].starts_with("rsqrt")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct In {
    pub op: u8,
    pub pred: u8,
    pub mxcsr: u32,
    pub a: u128,
    pub b: u128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Out {
    pub xm: bool,
    pub mxcsr: u32,
    pub r: u128,
}

impl In {
    pub fn encode(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&[self.op, self.pred, 0, 0]);
        buf.extend_from_slice(&self.mxcsr.to_le_bytes());
        buf.extend_from_slice(&self.a.to_le_bytes());
        buf.extend_from_slice(&self.b.to_le_bytes());
    }
    pub fn decode(b: &[u8]) -> In {
        In {
            op: b[0],
            pred: b[1],
            mxcsr: u32::from_le_bytes(b[4..8].try_into().unwrap()),
            a: u128::from_le_bytes(b[8..24].try_into().unwrap()),
            b: u128::from_le_bytes(b[24..40].try_into().unwrap()),
        }
    }
}

impl Out {
    pub fn encode(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&[self.xm as u8, 0, 0, 0]);
        buf.extend_from_slice(&self.mxcsr.to_le_bytes());
        buf.extend_from_slice(&self.r.to_le_bytes());
    }
    pub fn decode(b: &[u8]) -> Out {
        assert!(b[0] <= 1, "bad status {}", b[0]);
        Out {
            xm: b[0] == 1,
            mxcsr: u32::from_le_bytes(b[4..8].try_into().unwrap()),
            r: u128::from_le_bytes(b[8..24].try_into().unwrap()),
        }
    }
}

/// Runs one record through the Rust side.
pub fn run_rust(i: &In) -> Out {
    let mut m = i.mxcsr;
    let (a, b) = (i.a, i.b);
    let (a32, b32) = (a as u32, b as u32);
    let r: Result<u128, simd::Xm> = match OPS[i.op as usize] {
        "addss" => simd::addss(&mut m, a, b32),
        "addps" => simd::addps(&mut m, a, b),
        "subss" => simd::subss(&mut m, a, b32),
        "subps" => simd::subps(&mut m, a, b),
        "mulss" => simd::mulss(&mut m, a, b32),
        "mulps" => simd::mulps(&mut m, a, b),
        "divss" => simd::divss(&mut m, a, b32),
        "divps" => simd::divps(&mut m, a, b),
        "sqrtss" => simd::sqrtss(&mut m, a, b32),
        "sqrtps" => simd::sqrtps(&mut m, b),
        "minss" => simd::minss(&mut m, a, b32),
        "minps" => simd::minps(&mut m, a, b),
        "maxss" => simd::maxss(&mut m, a, b32),
        "maxps" => simd::maxps(&mut m, a, b),
        "cmpss" => simd::cmpss(&mut m, a, b32, i.pred),
        "cmpps" => simd::cmpps(&mut m, a, b, i.pred),
        "comiss" => simd::comiss(&mut m, a32, b32).map(u128::from),
        "ucomiss" => simd::ucomiss(&mut m, a32, b32).map(u128::from),
        "cvtsi2ss" => simd::cvtsi2ss(&mut m, a, b32),
        "cvtss2si" => simd::cvtss2si(&mut m, b32).map(u128::from),
        "cvttss2si" => simd::cvttss2si(&mut m, b32).map(u128::from),
        "cvtpi2ps" => simd::cvtpi2ps(&mut m, a, b as u64),
        "cvtps2pi" => simd::cvtps2pi(&mut m, b as u64).map(u128::from),
        "cvttps2pi" => simd::cvttps2pi(&mut m, b as u64).map(u128::from),
        "rcpss" => Ok(simd::rcpss(a, b32)),
        "rcpps" => Ok(simd::rcpps(b)),
        "rsqrtss" => Ok(simd::rsqrtss(a, b32)),
        "rsqrtps" => Ok(simd::rsqrtps(b)),
        o => panic!("unknown op {o}"),
    };
    match r {
        Ok(r) => Out {
            xm: false,
            mxcsr: m,
            r,
        },
        Err(_) => Out {
            xm: true,
            mxcsr: m,
            r: 0,
        },
    }
}

// ---- inputs ---------------------------------------------------------------------

/// SplitMix64: small, fast and reproducible across hosts.
pub struct Rng(pub u64);
impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    pub fn pick<T: Copy>(&mut self, xs: &[T]) -> T {
        xs[self.below(xs.len() as u64) as usize]
    }
}

/// Values every operation should meet: signed zeros, the denormal and normal
/// boundaries, infinities, quiet and signalling NaNs with payloads, values
/// round 1, and the int32 conversion boundaries.
pub const EDGES: &[u32] = &[
    0x0000_0000,
    0x8000_0000,
    0x0000_0001,
    0x8000_0001,
    0x0000_0002,
    0x007F_FFFF,
    0x807F_FFFF,
    0x0040_0000,
    0x0080_0000,
    0x8080_0000,
    0x0080_0001,
    0x00FF_FFFF,
    0x0100_0000,
    0x3F80_0000,
    0xBF80_0000,
    0x3F80_0001,
    0x3F7F_FFFF,
    0x3F7F_FFFE,
    0x3F00_0000,
    0xBF00_0000,
    0x3FC0_0000,
    0x4000_0000,
    0x4040_0000,
    0x3400_0000,
    0x3380_0000,
    0x3380_0001,
    0x337F_FFFF,
    0x7F7F_FFFF,
    0xFF7F_FFFF,
    0x7F00_0000,
    0x7E80_0000,
    0x7F80_0000,
    0xFF80_0000,
    0x7FC0_0000,
    0xFFC0_0000,
    0x7FC0_0001,
    0x7FA0_0000,
    0xFFA0_0000,
    0x7F80_0001,
    0x7FBF_FFFF,
    0xFFFF_FFFF,
    0x4F00_0000,
    0xCF00_0000,
    0x4EFF_FFFF,
    0xCEFF_FFFF,
    0xCF00_0001,
    0x4F80_0000,
    0x4B00_0000,
    0x4B80_0000,
    0x4AFF_FFFF,
    0x3EFF_FFFF,
    0x3F40_0000,
    0xBF40_0000,
    0x3FE0_0000,
    0x0C80_0000,
    0x1F80_0000,
    0x5F80_0000,
    0x6000_0000,
    0x2000_0000,
    0x1FFF_FFFF,
    0x7E7F_FFFF,
    0x7EFF_FFFF,
    0x7F7F_FFFE,
];

/// A float operand: edges, near-edges, random bit patterns, and values in
/// exponent bands where results overflow, underflow or cancel.
pub fn float(rng: &mut Rng) -> u32 {
    let sign = (rng.next() as u32) & 0x8000_0000;
    match rng.below(10) {
        0 | 1 => rng.pick(EDGES),
        2 => rng
            .pick(EDGES)
            .wrapping_add((rng.below(5) as u32).wrapping_sub(2)),
        3 => sign | (rng.next() as u32 & 0x007F_FFFF), // denormal
        4 => sign | ((rng.below(8) as u32 + 1) << 23) | (rng.next() as u32 & 0x007F_FFFF), // near 2^-126
        5 => sign | ((rng.below(10) as u32 + 0xF5) << 23) | (rng.next() as u32 & 0x007F_FFFF), // near max
        6 => sign | ((rng.below(16) as u32 + 120) << 23) | (rng.next() as u32 & 0x007F_FFFF), // round 1
        7 => sign | ((rng.below(12) as u32 + 150) << 23) | (rng.next() as u32 & 0x007F_FFFF), // round 2^31
        8 => {
            // few significant bits, so sums and products land exactly
            let m = (rng.next() as u32 & 0x007F_FFFF) & !((1 << rng.below(23)) - 1);
            sign | ((rng.below(254) as u32 + 1) << 23) | m
        }
        _ => rng.next() as u32,
    }
}

/// A second operand related to the first, to hit cancellation, ties and
/// equal compares.
pub fn related(rng: &mut Rng, a: u32) -> u32 {
    match rng.below(6) {
        0 => a,
        1 => a ^ 0x8000_0000,
        2 => a.wrapping_add((rng.below(7) as u32).wrapping_sub(3)),
        3 => {
            (a & 0xFF80_0000).wrapping_sub((rng.below(30) as u32) << 23)
                | (rng.next() as u32 & 0x007F_FFFF)
        }
        _ => float(rng),
    }
}

pub fn int(rng: &mut Rng) -> u32 {
    match rng.below(4) {
        0 => rng.pick(&[
            0,
            1,
            u32::MAX,
            0x8000_0000,
            0x7FFF_FFFF,
            0x0100_0000,
            0x0100_0001,
            0x00FF_FFFF,
            0x8000_0001,
        ]),
        1 => (rng.next() as u32) >> rng.below(32),
        2 => ((rng.next() as u32) >> rng.below(32)).wrapping_neg(),
        _ => rng.next() as u32,
    }
}

pub fn vector(rng: &mut Rng, f: impl Fn(&mut Rng) -> u32) -> u128 {
    (0..4).fold(0, |v, i| v | (f(rng) as u128) << (32 * i))
}

/// A random MXCSR: any rounding mode, FTZ on or off, sticky flags already
/// set, and (unless `masked_only`) any subset of the masks cleared. DAZ is
/// never set: the Pentium III rejects it.
pub fn mxcsr(rng: &mut Rng, masked_only: bool) -> u32 {
    let rc = (rng.below(4) as u32) << 13;
    let ftz = if rng.below(3) == 0 { 0x8000 } else { 0 };
    let sticky = if rng.below(4) == 0 {
        rng.next() as u32 & 0x3F
    } else {
        0
    };
    let masks = if masked_only || rng.below(3) == 0 {
        0x1F80
    } else if rng.below(2) == 0 {
        0x1F80 & !(1 << (7 + rng.below(6)))
    } else {
        (rng.next() as u32) & 0x1F80
    };
    rc | ftz | sticky | masks
}

pub fn record(rng: &mut Rng, masked_only: bool) -> In {
    let op = rng.below(OPS.len() as u64) as u8;
    let name = OPS[op as usize];
    let a = vector(rng, float);
    let b = if name.starts_with("cvtsi") || name.starts_with("cvtpi") {
        vector(rng, int)
    } else {
        // pair each lane of b with the matching lane of a
        (0..4).fold(0u128, |v, i| {
            v | (related(rng, (a >> (32 * i)) as u32) as u128) << (32 * i)
        })
    };
    In {
        op,
        pred: rng.below(256) as u8,
        mxcsr: mxcsr(rng, masked_only),
        a,
        b,
    }
}

/// Systematic cases: every op on every pair of edge values (lane 0, with the
/// other lanes random), under each rounding mode with and without FTZ.
pub fn edge_records(masked_only: bool) -> Vec<In> {
    let mut rng = Rng(0xED6E);
    let mut v = Vec::new();
    for op in 0..OPS.len() as u8 {
        let name = OPS[op as usize];
        let bs: Vec<u32> = if name.starts_with("cvtsi") || name.starts_with("cvtpi") {
            vec![
                0,
                1,
                u32::MAX,
                0x8000_0000,
                0x7FFF_FFFF,
                0x0100_0001,
                0x00FF_FFFF,
                0xFEFF_FFFF,
                0x7FFF_FFC0,
            ]
        } else {
            EDGES.to_vec()
        };
        let preds: &[u8] = if name.starts_with("cmp") {
            &[0, 1, 2, 3, 4, 5, 6, 7]
        } else {
            &[0]
        };
        let modes: &[u32] = &[0x1F80, 0x3F80, 0x5F80, 0x7F80, 0x9F80, 0xFF80];
        for &x in EDGES {
            for &y in &bs {
                for &pred in preds {
                    let mx = rng.pick(modes);
                    let mx = if masked_only || rng.below(2) == 0 {
                        mx
                    } else {
                        mx & !((rng.next() as u32) & 0x1F80)
                    };
                    let a = vector(&mut rng, float) & !0xFFFF_FFFF | x as u128;
                    let b = vector(&mut rng, float) & !0xFFFF_FFFF | y as u128;
                    v.push(In {
                        op,
                        pred,
                        mxcsr: mx,
                        a,
                        b,
                    });
                }
            }
        }
    }
    v
}

// ---- C harnesses ------------------------------------------------------------------

pub fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub fn cc() -> String {
    std::env::var("CC").unwrap_or_else(|_| "cc".into())
}

/// Compiles a harness from `c/test/<src>` into the test temp dir.
pub fn build(src: &str, name: &str, extra: &[&str]) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("simd-harness");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join(name);
    let c = crate_dir().join("c");
    let out = Command::new(cc())
        .args([
            "-std=c11",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pedantic",
            "-O2",
        ])
        .args(extra)
        .arg(format!("-I{}", c.display()))
        .arg(c.join("test").join(src))
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run the C compiler (set CC to override)");
    assert!(
        out.status.success(),
        "building {src} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    exe
}

/// The soft-float harness, with UndefinedBehaviorSanitizer when the host
/// compiler has it.
pub fn soft_harness(extra: &[&str], name: &str) -> PathBuf {
    let mut flags = vec!["-fsanitize=undefined", "-fno-sanitize-recover=all"];
    flags.extend_from_slice(extra);
    build("soft_harness.c", name, &flags)
}

/// Pipes records through a harness and returns its outputs.
pub fn run_harness(cmd: &mut Command, ins: &[In]) -> Vec<Out> {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn harness");
    let mut stdin = child.stdin.take().unwrap();
    let mut buf = Vec::with_capacity(ins.len() * IN_SIZE);
    for i in ins {
        i.encode(&mut buf);
    }
    let writer = std::thread::spawn(move || {
        stdin.write_all(&buf).unwrap();
    });
    let mut out = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    writer.join().unwrap();
    let status = child.wait().unwrap();
    assert!(
        status.success(),
        "harness exited with {status} (3: wrote a destination on #XM)"
    );
    assert_eq!(out.len(), ins.len() * OUT_SIZE, "short harness output");
    out.chunks(OUT_SIZE).map(Out::decode).collect()
}

pub fn describe(i: &In) -> String {
    format!(
        "{} pred={} mxcsr={:#06x} a={:#034x} b={:#034x}",
        OPS[i.op as usize], i.pred, i.mxcsr, i.a, i.b
    )
}

/// Compares outputs, returning a report of the first few mismatches.
pub fn compare(ins: &[In], want: &[Out], got: &[Out], what: &str) -> Result<(), String> {
    let bad: Vec<usize> = (0..ins.len()).filter(|&k| want[k] != got[k]).collect();
    if bad.is_empty() {
        return Ok(());
    }
    let mut s = format!("{what}: {} of {} records differ\n", bad.len(), ins.len());
    let mut kinds = std::collections::BTreeMap::new();
    for &k in &bad {
        let pred = if OPS[ins[k].op as usize].starts_with("cmp") {
            ins[k].pred & 7
        } else {
            0
        };
        let key = (
            OPS[ins[k].op as usize],
            pred,
            want[k].mxcsr ^ got[k].mxcsr,
            want[k].r != got[k].r,
        );
        *kinds.entry(key).or_insert(0usize) += 1;
    }
    for ((op, pred, dm, dr), n) in kinds {
        s += &format!(
            "  {n:7} x {op} pred {pred}: mxcsr differs by {dm:#x}, result differs: {dr}\n"
        );
    }
    for &k in bad.iter().take(12) {
        s += &format!(
            "  {}\n    want xm={} mxcsr={:#06x} r={:#034x}\n    got  xm={} mxcsr={:#06x} r={:#034x}\n",
            describe(&ins[k]),
            want[k].xm,
            want[k].mxcsr,
            want[k].r,
            got[k].xm,
            got[k].mxcsr,
            got[k].r
        );
    }
    Err(s)
}
