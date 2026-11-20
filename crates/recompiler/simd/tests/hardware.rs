//! Both sides against results from real SSE hardware.
//!
//! The normal tests run natively on any host against a frozen fixture,
//! `tests/fixtures/sse-rosetta.bin`, recorded once on x86_64 SSE under
//! Rosetta 2. Nothing here needs Rosetta unless asked for:
//!
//! * `H2SIMD_ROSETTA=1` runs a fresh random set live through the host's SSE
//!   unit (natively on x86_64; through Rosetta on Apple silicon). It skips,
//!   passing, when there is no way to run x86_64 code.
//! * `H2SIMD_RECORD_FIXTURE=1` (with the above) rewrites the fixture.
//!
//! Rosetta reports every exception as masked whatever MXCSR says, so the
//! hardware records use fully masked MXCSRs; unmasked behaviour rests on
//! the SDM and the Rust/C differential (docs/lifter/simd.md). Rosetta's rcp/rsqrt
//! tables are not the Pentium III's, so those ops are not recorded here;
//! tests/approx.rs checks them against measured Intel tables.

mod common;
use common::*;
use std::process::Command;

const FIXTURE: &str = "tests/fixtures/sse-rosetta.bin";
const MAGIC: &[u8; 8] = b"H2SIMDF1";

fn load_fixture() -> (Vec<In>, Vec<Out>) {
    let bytes = std::fs::read(crate_dir().join(FIXTURE)).expect("read the fixture");
    assert_eq!(&bytes[..8], MAGIC, "fixture magic");
    let n = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let body = &bytes[12..];
    assert_eq!(body.len(), n * (IN_SIZE + OUT_SIZE), "fixture length");
    body.chunks(IN_SIZE + OUT_SIZE)
        .map(|c| (In::decode(&c[..IN_SIZE]), Out::decode(&c[IN_SIZE..])))
        .unzip()
}

/// Rosetta sets DE for `cmp{ss,ps}` with the unord (3) and ord (7)
/// predicates when one operand of a lane is a NaN and the other a denormal;
/// for the other six predicates, and for every other op, it lets the NaN
/// suppress DE, as the SDM's exception priority says. We keep the uniform
/// rule (docs/lifter/simd.md) and accept exactly that difference.
fn rosetta_cmp_de(i: &In, sse: &Out, ours: &Out) -> bool {
    let name = OPS[i.op as usize];
    if !(name == "cmpss" || name == "cmpps") || !matches!(i.pred & 7, 3 | 7) {
        return false;
    }
    let lanes = if name == "cmpss" { 1 } else { 4 };
    let nan_denormal = (0..lanes).any(|l| {
        let (x, y) = ((i.a >> (32 * l)) as u32, (i.b >> (32 * l)) as u32);
        let nan = |v: u32| v & 0x7FFF_FFFF > 0x7F80_0000;
        let den = |v: u32| v & 0x7F80_0000 == 0 && v & 0x007F_FFFF != 0;
        (nan(x) && den(y)) || (den(x) && nan(y))
    });
    nan_denormal && sse.xm == ours.xm && sse.r == ours.r && sse.mxcsr == ours.mxcsr | simd::DE
}

/// Hardware results with the known Rosetta deviation replaced by ours.
fn accept_known(ins: &[In], sse: &[Out], ours: &[Out]) -> (Vec<Out>, usize) {
    let mut n = 0;
    let v = (0..ins.len())
        .map(|k| {
            if sse[k] != ours[k] && rosetta_cmp_de(&ins[k], &sse[k], &ours[k]) {
                n += 1;
                ours[k]
            } else {
                sse[k]
            }
        })
        .collect();
    (v, n)
}

#[test]
fn rust_matches_recorded_hardware() {
    let (ins, sse) = load_fixture();
    let got: Vec<Out> = ins.iter().map(run_rust).collect();
    let (want, known) = accept_known(&ins, &sse, &got);
    eprintln!("{known} records show the known Rosetta cmp DE deviation");
    if let Err(e) = compare(&ins, &want, &got, "Rust vs recorded SSE") {
        panic!("{e}");
    }
    eprintln!(
        "{} recorded hardware results match the Rust side",
        ins.len()
    );
}

#[test]
fn c_matches_recorded_hardware() {
    let (ins, sse) = load_fixture();
    let got = run_harness(&mut Command::new(soft_harness(&[], "soft-hw")), &ins);
    let (want, _) = accept_known(&ins, &sse, &got);
    if let Err(e) = compare(&ins, &want, &got, "C vs recorded SSE") {
        panic!("{e}");
    }
}

/// A command running the real-SSE harness, or None when this host cannot
/// run x86_64 code.
fn sse_command() -> Option<Command> {
    if cfg!(target_arch = "x86_64") {
        return Some(Command::new(build("sse_harness.c", "sse", &[])));
    }
    if !cfg!(target_os = "macos") {
        return None;
    }
    let rosetta = Command::new("arch")
        .args(["-x86_64", "/usr/bin/true"])
        .status()
        .is_ok_and(|s| s.success());
    if !rosetta {
        return None;
    }
    let exe = build("sse_harness.c", "sse-x86_64", &["-arch", "x86_64"]);
    let mut c = Command::new("arch");
    c.arg("-x86_64").arg(exe);
    Some(c)
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
fn linux_native_sse_harness_adds_scalar_lanes() {
    let input = In {
        op: op("addss"),
        pred: 0,
        mxcsr: 0x1f80,
        a: (17u128 << 32) | u128::from(1.0f32.to_bits()),
        b: u128::from(2.0f32.to_bits()),
    };
    // Use a separate output name so the optional live sweep can compile in parallel.
    let mut cmd = Command::new(build("sse_harness.c", "sse-linux-add", &[]));
    assert_eq!(run_harness(&mut cmd, &[input]), vec![run_rust(&input)]);
}

/// Inputs for the hardware: fully masked MXCSRs, no rcp/rsqrt.
fn hardware_inputs(n: usize, seed: u64) -> Vec<In> {
    let mut ins: Vec<In> = edge_records(true)
        .into_iter()
        .filter(|i| !is_approx(i.op))
        .collect();
    let mut rng = Rng(seed);
    while ins.len() < n {
        let i = record(&mut rng, true);
        if !is_approx(i.op) {
            ins.push(i);
        }
    }
    ins
}

#[test]
fn live_hardware_spot_check() {
    if std::env::var_os("H2SIMD_ROSETTA").is_none() {
        eprintln!("skipped: set H2SIMD_ROSETTA=1 to run against the host's SSE unit");
        return;
    }
    let Some(mut cmd) = sse_command() else {
        eprintln!("skipped: this host cannot run x86_64 code");
        return;
    };
    let n: usize = std::env::var("H2SIMD_HW_N")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1_000_000);
    let seed: u64 = std::env::var("H2SIMD_HW_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0x4A4D);
    let ins = hardware_inputs(n, seed);
    let hw = run_harness(&mut cmd, &ins);
    let got: Vec<Out> = ins.iter().map(run_rust).collect();
    let (want, known) = accept_known(&ins, &hw, &got);
    eprintln!("{known} records show the known Rosetta cmp DE deviation");
    let res = compare(&ins, &want, &got, "Rust vs live SSE");
    if std::env::var_os("H2SIMD_RECORD_FIXTURE").is_some() {
        assert!(
            res.is_ok(),
            "not recording a fixture the Rust side disagrees with:\n{}",
            res.unwrap_err()
        );
        record_fixture(&ins, &hw);
    }
    if let Err(e) = res {
        panic!("{e}");
    }
    eprintln!("{} live hardware results match the Rust side", ins.len());
}

/// Keeps every edge record and a deterministic sample of the random ones.
fn record_fixture(ins: &[In], hw: &[Out]) {
    let edges = edge_records(true)
        .into_iter()
        .filter(|i| !is_approx(i.op))
        .count();
    let mut rng = Rng(0xF1C5);
    let mut keep: Vec<usize> = (0..edges).filter(|_| rng.below(40) == 0).collect();
    keep.extend((edges..ins.len()).filter(|_| rng.below(400) == 0));
    let mut buf = MAGIC.to_vec();
    buf.extend_from_slice(&(keep.len() as u32).to_le_bytes());
    for &k in &keep {
        ins[k].encode(&mut buf);
        hw[k].encode(&mut buf);
    }
    std::fs::write(crate_dir().join(FIXTURE), buf).unwrap();
    eprintln!("recorded {} results into {FIXTURE}", keep.len());
}
