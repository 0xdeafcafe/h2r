//! The Rust side against the C side (`c/h2simd.h`, built with UBSan): the
//! systematic edge cases plus a large random set, any MXCSR (unmasked
//! exceptions included). `H2SIMD_DIFF_N` sets the random count (default
//! 2,000,000); `H2SIMD_DIFF_SEED` the seed.

mod common;
use common::*;

fn check(ins: &[In], what: &str) {
    let exe = soft_harness(&[], "soft");
    let got = run_harness(&mut std::process::Command::new(exe), ins);
    let want: Vec<Out> = ins.iter().map(run_rust).collect();
    if let Err(e) = compare(ins, &want, &got, what) {
        panic!("{e}");
    }
}

#[test]
fn rust_and_c_agree_on_edge_cases() {
    let ins = edge_records(false);
    check(&ins, "edge cases");
    eprintln!("{} edge records agree", ins.len());
}

#[test]
fn rust_and_c_agree_on_random_inputs() {
    let n: usize = std::env::var("H2SIMD_DIFF_N")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2_000_000);
    let seed: u64 = std::env::var("H2SIMD_DIFF_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0x5EED);
    let mut rng = Rng(seed);
    let ins: Vec<In> = (0..n).map(|_| record(&mut rng, false)).collect();
    check(&ins, "random");
    eprintln!("{n} random records agree (seed {seed:#x})");
}
