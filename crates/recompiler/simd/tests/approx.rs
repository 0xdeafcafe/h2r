//! rcp/rsqrt against measured Intel hardware.
//!
//! The reference is camoufox PR #35 (github.com/JWriter20/camoufox/pull/35,
//! browser/tests/cpu-estimates): RCPPS/RSQRTPS recorded over all 2^32 inputs
//! on an Intel Core i7-12700H and an i9-11900H, which agree bit for bit. It
//! publishes the 12,288-entry table (rcp of 1.m, rsqrt of 1.m and of 2.m, for
//! the 4096 values of the top 12 fraction bits) and FNV-1a hashes of the
//! native instructions over every input. Only those hashes are used here,
//! not the table or code. See docs/lifter/simd.md for why the Pentium III is
//! expected to share these tables.

use std::process::Command;

mod common;

// The recording uses this offset basis (the standard one without its last
// digit); kept so the hashes compare.
const FNV0: u64 = 1_469_598_103_934_665_603;
fn fnv(h: u64, x: u32) -> u64 {
    (h ^ x as u64).wrapping_mul(0x0000_0100_0000_01B3)
}

/// FNV-1a of the measured table, in the recording's order.
const INTEL_TABLE_FNV: u64 = 0x6318_46b7_0b29_a383;
/// FNV-1a of the native instructions' outputs for inputs 0..2^32.
const INTEL_RCP_FNV: u64 = 0xbfa9_847d_5afd_0383;
const INTEL_RSQRT_FNV: u64 = 0x7cdf_6c7d_3f01_0383;

#[test]
fn table_matches_measured_intel_table() {
    let mut h = FNV0;
    for i in 0..4096u32 {
        h = fnv(h, simd::rcp_approx(0x3F80_0000 | i << 11));
    }
    for p in 0..2u32 {
        for i in 0..4096u32 {
            h = fnv(h, simd::rsqrt_approx((127 + p) << 23 | i << 11));
        }
    }
    assert_eq!(h, INTEL_TABLE_FNV);
}

/// Every input, special values and flush thresholds included: Rust and C
/// against the hashes of the native Intel instructions. Several seconds in
/// release mode, so opt-in: `cargo test --release -p simd --test approx --
/// --ignored`.
#[test]
#[ignore]
fn every_input_matches_intel() {
    let (mut r, mut s) = (FNV0, FNV0);
    for x in 0..=u32::MAX {
        r = fnv(r, simd::rcp_approx(x));
        s = fnv(s, simd::rsqrt_approx(x));
    }
    assert_eq!((r, s), (INTEL_RCP_FNV, INTEL_RSQRT_FNV), "Rust");

    let exe = common::build("approx_hash.c", "approx-hash", &[]);
    let out = Command::new(exe).output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(
        text.trim(),
        format!("{INTEL_RCP_FNV:016x} {INTEL_RSQRT_FNV:016x}"),
        "C"
    );
}
