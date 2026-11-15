//! Both sides against the SSE oracle's JSONL corpus (crates/sse-oracle,
//! docs/sse-oracle.md, schema 1): every fixture whose instruction is one of
//! ours is turned into a protocol record and checked bit for bit, MXCSR
//! and the compare EFLAGS included.
//!
//! The corpus is read from `H2SIMD_ORACLE_CORPUS` (the full scratch corpus,
//! or the oracle's committed subset). Without it the test says so and
//! passes. Approximation ops (rcp/rsqrt) are reported, not asserted: the
//! corpus comes from Rosetta, whose tables are not the Pentium III's
//! (tests/approx.rs checks ours against measured Intel tables).

mod common;
use common::*;
use serde_json::Value;
use std::io::BufRead;

/// The fixture as a protocol record and the observed outcome, or None for
/// an instruction this crate does not model.
fn convert(v: &Value) -> Option<(In, Out)> {
    let mnemonic = v["mnemonic"].as_str()?;
    let op = OPS.iter().position(|&n| n == mnemonic)? as u8;
    let bytes: Vec<u8> = v["instruction"]
        .as_array()?
        .iter()
        .map(|b| b.as_u64().unwrap() as u8)
        .collect();
    let mut k = 0;
    if bytes[k] == 0xF3 {
        k += 1;
    }
    assert_eq!(bytes[k], 0x0F, "{mnemonic} {bytes:?}");
    let modrm = bytes[k + 2];
    let imm = bytes.get(k + 3).copied().unwrap_or(0);
    let (md, reg, rm) = (
        modrm >> 6,
        ((modrm >> 3) & 7) as usize,
        (modrm & 7) as usize,
    );
    let input = &v["input"];
    let state = &v["output"]["state"];
    assert!(
        v["output"]["exception"].is_null(),
        "masked corpus should not trap: {v}"
    );
    let words = |x: &Value| -> u128 {
        x.as_array()
            .unwrap()
            .iter()
            .enumerate()
            .fold(0, |acc, (i, w)| {
                acc | (w.as_u64().unwrap() as u128) << (32 * i)
            })
    };
    let xmm = |s: &Value, n: usize| words(&s["xmm"][n]);
    let mm = |s: &Value, n: usize| words(&s["mm"][n]);
    let gpr = |s: &Value, n: usize| s[["eax", "ecx"][n]].as_u64().unwrap() as u128;
    let mem = words(&input["memory"]);
    let from_mem = md == 0 && rm == 7;
    assert!(from_mem || md == 3, "{mnemonic} {bytes:?}");
    let src_xmm = if from_mem { mem } else { xmm(input, rm) };
    let mxcsr = input["mxcsr"].as_u64().unwrap() as u32;
    let mx_out = state["mxcsr"].as_u64().unwrap() as u32;
    let (a, b, r) = match mnemonic {
        "comiss" | "ucomiss" => {
            let fl = state["eflags"].as_u64().unwrap() as u32;
            assert_eq!(fl & 0x890, 0, "{mnemonic} must clear OF, SF and AF");
            (xmm(input, reg), src_xmm, (fl & 0x45) as u128)
        }
        "cvtsi2ss" => (
            xmm(input, reg),
            if from_mem { mem } else { gpr(input, rm) },
            xmm(state, reg),
        ),
        "cvtpi2ps" => (
            xmm(input, reg),
            if from_mem { mem } else { mm(input, rm) },
            xmm(state, reg),
        ),
        "cvtss2si" | "cvttss2si" => (0, src_xmm, gpr(state, reg)),
        "cvtps2pi" | "cvttps2pi" => (0, src_xmm, mm(state, reg)),
        _ => (xmm(input, reg), src_xmm, xmm(state, reg)),
    };
    // A scalar memory operand is 32 bits wide; cvt*2pi/cvtpi2ps 64.
    let b = if from_mem && mnemonic.ends_with("ss") || mnemonic == "comiss" || mnemonic == "ucomiss"
    {
        b & 0xFFFF_FFFF
    } else {
        b
    };
    let i = In {
        op,
        pred: imm,
        mxcsr,
        a,
        b,
    };
    Some((
        i,
        Out {
            xm: false,
            mxcsr: mx_out,
            r,
        },
    ))
}

#[test]
fn oracle_corpus() {
    let Some(path) = std::env::var_os("H2SIMD_ORACLE_CORPUS") else {
        eprintln!("skipped: set H2SIMD_ORACLE_CORPUS to an sse-oracle JSONL corpus");
        return;
    };
    let file = std::fs::File::open(&path).expect("open the corpus");
    let mut ins = Vec::new();
    let mut want = Vec::new();
    let (mut lines, mut skipped) = (0usize, 0usize);
    for line in std::io::BufReader::with_capacity(1 << 20, file).lines() {
        let line = line.unwrap();
        lines += 1;
        // Cheap prefilter on the mnemonic before parsing the JSON.
        let Some(at) = line.find("\"mnemonic\":\"") else {
            continue;
        };
        let m = &line[at + 12..];
        let m = &m[..m.find('"').unwrap()];
        if !OPS.contains(&m) {
            skipped += 1;
            continue;
        }
        let v: Value = serde_json::from_str(&line).unwrap();
        if let Some((i, o)) = convert(&v) {
            ins.push(i);
            want.push(o);
        }
    }
    eprintln!(
        "{lines} fixtures read, {} ours, {skipped} other instructions",
        ins.len()
    );

    let (approx, exact): (Vec<usize>, Vec<usize>) =
        (0..ins.len()).partition(|&k| is_approx(ins[k].op));
    let exact_in: Vec<In> = exact.iter().map(|&k| ins[k]).collect();
    let exact_want: Vec<Out> = exact.iter().map(|&k| want[k]).collect();

    let rust: Vec<Out> = exact_in.iter().map(run_rust).collect();
    let c = run_harness(
        &mut std::process::Command::new(soft_harness(&[], "soft-oracle")),
        &exact_in,
    );
    let mut per_op = std::collections::BTreeMap::new();
    for i in &exact_in {
        *per_op.entry(OPS[i.op as usize]).or_insert(0usize) += 1;
    }
    eprintln!("checked per op: {per_op:?}");
    let r1 = compare(&exact_in, &exact_want, &rust, "Rust vs oracle corpus");
    let r2 = compare(&exact_in, &rust, &c, "C vs Rust on the oracle corpus");

    let approx_diff = approx
        .iter()
        .filter(|&&k| run_rust(&ins[k]) != want[k])
        .count();
    eprintln!(
        "rcp/rsqrt: {approx_diff} of {} differ from Rosetta (expected: Rosetta's tables are not the Pentium III's)",
        approx.len()
    );
    if let Err(e) = r1 {
        panic!("{e}");
    }
    if let Err(e) = r2 {
        panic!("{e}");
    }
    eprintln!("{} oracle fixtures match on both sides", exact_in.len());
}
