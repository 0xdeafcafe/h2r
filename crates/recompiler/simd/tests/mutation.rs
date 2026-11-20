//! Mutation sanity: a deliberately broken C side (round-up treated as
//! round-down, `-DH2SIMD_TEST_MUTATION`) must be caught by both the
//! differential check and the recorded hardware results, and only in records
//! that round up. A suite that passed this would be checking nothing.

mod common;
use common::*;
use std::process::Command;

fn mutant() -> Command {
    Command::new(soft_harness(&["-DH2SIMD_TEST_MUTATION"], "soft-mutant"))
}

fn rounds_up(i: &In) -> bool {
    (i.mxcsr >> 13) & 3 == 2
}

#[test]
fn broken_rounding_is_caught_by_the_differential() {
    let mut rng = Rng(0xBAD);
    let mut ins = edge_records(false);
    ins.extend((0..200_000).map(|_| record(&mut rng, false)));
    let got = run_harness(&mut mutant(), &ins);
    let want: Vec<Out> = ins.iter().map(run_rust).collect();
    let bad: Vec<usize> = (0..ins.len()).filter(|&k| got[k] != want[k]).collect();
    eprintln!("mutant differs on {} of {} records", bad.len(), ins.len());
    assert!(
        bad.len() > 1000,
        "the differential barely notices a broken rounding mode"
    );
    assert!(
        bad.iter().all(|&k| rounds_up(&ins[k])),
        "a difference outside round-up mode"
    );
}

#[test]
fn broken_rounding_is_caught_by_recorded_hardware() {
    let bytes = std::fs::read(crate_dir().join("tests/fixtures/sse-rosetta.bin")).unwrap();
    let ins: Vec<In> = bytes[12..]
        .chunks(IN_SIZE + OUT_SIZE)
        .map(|c| In::decode(&c[..IN_SIZE]))
        .collect();
    let hw: Vec<Out> = bytes[12..]
        .chunks(IN_SIZE + OUT_SIZE)
        .map(|c| Out::decode(&c[IN_SIZE..]))
        .collect();
    let got = run_harness(&mut mutant(), &ins);
    let honest: Vec<Out> = ins.iter().map(run_rust).collect();
    // Count only records where the honest side matches the hardware.
    let caught = (0..ins.len())
        .filter(|&k| honest[k] == hw[k] && got[k] != hw[k])
        .count();
    eprintln!(
        "mutant contradicts recorded hardware on {caught} of {} records",
        ins.len()
    );
    assert!(
        caught > 50,
        "the hardware fixture barely notices a broken rounding mode"
    );
}
