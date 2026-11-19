//! Deterministic edge-pair and random bit-pattern inputs, across eight MXCSR modes.
use crate::{Fixture, Form, Oracle, State};
use anyhow::Result;
use std::{collections::BTreeMap, io::Write};

pub const SEED: u64 = 0x243f6a8885a308d3;

// Both signs, distinct payloads, conversion boundaries, rounding ties, underflow.
pub const EDGES: &[u32] = &[
    0, 0x80000000, 0x7f800000, 0xff800000, 0x7fc00000, 0xffc00000, 0x7fc12345, 0xffc54321,
    0x7f800001, 0xff800001, 0x7fa12345, 0xffa54321, 1, 0x80000001, 0x007fffff, 0x807fffff,
    0x00800000, 0x80800000, 0x00800001, 0x80800001, 0x7f7fffff, 0xff7fffff, 0x3f000000, 0xbf000000,
    0x3f800000, 0xbf800000, 0x3f800001, 0x3f7fffff, 0x33800000, 0x33000000, 0x3fc00000, 0xbfc00000,
    0x40200000, 0xc0200000, 0x4affffff, 0x4b000000, 0x4b000001, 0x4b7fffff, 0x4b800000, 0x4b800001,
    0x4effffff, 0x4f000000, 0x4f000001, 0xceffffff, 0xcf000000, 0xcf000001, 0x7fffffff, 0x80000000,
    0xffffffff, 0x55555555, 0xaaaaaaaa, 0x0000ffff,
];
pub(crate) fn random(seed: &mut u64) -> u32 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    (*seed >> 16) as u32
}
pub(crate) fn pair(a: u32, b: u32) -> State {
    let mut s = State {
        eax: a,
        ecx: b,
        eflags: 0xad7,
        memory: [b; 4],
        ..State::default()
    };
    for (i, r) in s.xmm.iter_mut().enumerate() {
        *r = [if i == 0 { a } else { b }; 4];
    }
    for (i, r) in s.mm.iter_mut().enumerate() {
        *r = [if i == 0 { a } else { b }; 2];
    }
    s
}
/// Next random-case state; consumes exactly 55 PRNG draws.
pub(crate) fn random_state(rng: &mut u64) -> State {
    let mut s = State::default();
    for w in s
        .xmm
        .iter_mut()
        .flatten()
        .chain(s.mm.iter_mut().flatten())
        .chain(s.memory.iter_mut())
    {
        *w = random(rng);
    }
    s.eax = random(rng);
    s.ecx = random(rng);
    s.eflags |= random(rng) & 0x8d5;
    s
}
pub(crate) const SHIFT_COUNTS: [u32; 14] =
    [0, 1, 7, 8, 15, 16, 31, 32, 63, 64, 65, 255, 256, 0x80000000];
pub(crate) const INTEGER_A: [u32; 5] = [0x7f7f7f7f, 0x80808080, 0x7fff7fff, 0x80008000, 0xffff0001];
pub(crate) const INTEGER_B: [u32; 4] = [1, 0x01010101, 0x00010001, 0xffffffff];
pub(crate) const STICKY: [u32; 7] = [1, 2, 4, 8, 16, 32, 63];
pub(crate) fn shift_state(count: u32) -> State {
    let mut s = pair(0x87654321, count);
    s.mm[1] = [count, 0];
    s.memory = [count, 0, 0, 0];
    s
}
pub(crate) fn sticky_state(flags: u32) -> State {
    let mut s = pair(0x3f800000, 0x40000000);
    s.mxcsr |= flags;
    s
}
/// Apply one RC/FTZ mode to a case input, as the generator does.
pub(crate) fn with_mode(mut input: State, mnemonic: &str, rc: u32, ftz: u32) -> State {
    input.mxcsr |= rc << 13 | ftz << 15;
    if mnemonic == "ldmxcsr" {
        input.memory[0] = input.mxcsr;
    }
    input
}
/// Streams instead of keeping the corpus in memory. Count map is per mnemonic.
pub fn generate(
    oracle: &mut Oracle,
    forms: &[Form],
    output: &mut impl Write,
    random_cases: usize,
    seed: u64,
) -> Result<BTreeMap<String, usize>> {
    generate_filtered(
        oracle,
        forms,
        random_cases,
        seed,
        |_, _| true,
        |f| {
            serde_json::to_writer(&mut *output, &f)?;
            output.write_all(b"\n")?;
            Ok(())
        },
    )
}
/// Like [`generate`], but runs only cases accepted by `keep(form, case_name)`
/// and passes each fixture to `sink` instead of writing JSONL.
pub fn generate_filtered(
    oracle: &mut Oracle,
    forms: &[Form],
    random_cases: usize,
    seed: u64,
    keep: impl Fn(&Form, &str) -> bool,
    mut sink: impl FnMut(Fixture) -> Result<()>,
) -> Result<BTreeMap<String, usize>> {
    let backend = if crate::NATIVE_LINUX {
        "linux-x86_64"
    } else {
        "rosetta2-x86_64"
    };
    let mut counts = BTreeMap::new();
    for form in forms {
        let mut cases = Vec::new();
        for (i, &a) in EDGES.iter().enumerate() {
            for (j, &b) in EDGES.iter().enumerate() {
                cases.push((format!("edge-{i}-{j}"), pair(a, b)));
            }
        }
        let mut rng = seed;
        for i in 0..random_cases {
            cases.push((format!("random-{i}"), random_state(&mut rng)));
        }
        for count in SHIFT_COUNTS {
            cases.push((format!("shift-count-{count}"), shift_state(count)));
        }
        for a in INTEGER_A {
            for b in INTEGER_B {
                cases.push((format!("integer-{a:08x}-{b:08x}"), pair(a, b)));
            }
        }
        // Independently exercise sticky preservation, including movement/control instructions.
        for flags in STICKY {
            cases.push((format!("sticky-{flags}"), sticky_state(flags)));
        }
        for (case, input) in cases {
            if !keep(form, &case) {
                continue;
            }
            for rc in 0..4 {
                for ftz in 0..2 {
                    let input = with_mode(input.clone(), &form.mnemonic, rc, ftz);
                    let observation = oracle.query(&form.bytes, &input)?;
                    sink(Fixture {
                        schema: 1,
                        backend: backend.into(),
                        mnemonic: form.mnemonic.clone(),
                        code: form.code.clone(),
                        instruction: form.bytes.clone(),
                        case: case.clone(),
                        input,
                        output: observation,
                    })?;
                    *counts.entry(form.mnemonic.clone()).or_default() += 1;
                }
            }
        }
    }
    Ok(counts)
}

#[derive(Debug, serde::Serialize)]
pub struct Audit {
    pub fixtures: usize,
    pub forms: usize,
    pub mnemonics: BTreeMap<String, usize>,
    pub exceptions: usize,
}
/// Validate the schema, mode coverage, sticky flags and unaffected EFLAGS while
/// reading the corpus once. This is an integrity check, not a semantics model.
pub fn audit(input: impl std::io::BufRead) -> Result<Audit> {
    use anyhow::{Context, ensure};
    let mut report = Audit {
        fixtures: 0,
        forms: 0,
        mnemonics: BTreeMap::new(),
        exceptions: 0,
    };
    let mut modes = BTreeMap::<Vec<u8>, u8>::new();
    let mut backend = None;
    for (line, json) in input.lines().enumerate() {
        let f: Fixture =
            serde_json::from_str(&json?).with_context(|| format!("line {}", line + 1))?;
        ensure!(
            f.schema == 1 && matches!(f.backend.as_str(), "rosetta2-x86_64" | "linux-x86_64"),
            "unknown schema/backend on line {}",
            line + 1
        );
        if let Some(first) = &backend {
            ensure!(first == &f.backend, "mixed backends on line {}", line + 1);
        } else {
            backend = Some(f.backend);
        }
        if !modes.contains_key(&f.instruction) {
            crate::validate_instruction(&f.instruction)?;
        }
        let mode = ((f.input.mxcsr >> 13) & 3) | ((f.input.mxcsr >> 13) & 4);
        *modes.entry(f.instruction).or_default() |= 1 << mode;
        ensure!(
            f.input.mxcsr & 0x1fc0 == 0x1f80,
            "unmasked exceptions or DAZ on line {}",
            line + 1
        );
        if let Some(s) = f.output.state {
            ensure!(
                f.output.exception.is_none(),
                "both state and exception on line {}",
                line + 1
            );
            if f.mnemonic != "ldmxcsr" {
                ensure!(
                    s.mxcsr & !63 == f.input.mxcsr & !63,
                    "MXCSR control changed on line {}",
                    line + 1
                );
                ensure!(
                    s.mxcsr & f.input.mxcsr & 63 == f.input.mxcsr & 63,
                    "sticky flag cleared on line {}",
                    line + 1
                );
            }
            if !matches!(f.mnemonic.as_str(), "comiss" | "ucomiss") {
                ensure!(
                    s.eflags & 0x8d5 == f.input.eflags & 0x8d5,
                    "EFLAGS changed on line {}",
                    line + 1
                );
            }
            // Also check completion of non-temporal stores in the capture.
            match f.mnemonic.as_str() {
                "movntps" => ensure!(
                    s.memory == f.input.xmm[0],
                    "MOVNTPS capture on line {}",
                    line + 1
                ),
                "movntq" => ensure!(
                    s.memory[..2] == f.input.mm[0],
                    "MOVNTQ capture on line {}",
                    line + 1
                ),
                _ => (),
            }
        } else {
            ensure!(
                f.output.exception.is_some(),
                "missing state and exception on line {}",
                line + 1
            );
            report.exceptions += 1;
        }
        report.fixtures += 1;
        *report.mnemonics.entry(f.mnemonic).or_default() += 1;
    }
    ensure!(
        modes.values().all(|m| *m == 255),
        "some forms are missing MXCSR modes"
    );
    report.forms = modes.len();
    Ok(report)
}

#[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]
#[test]
fn generated_native_rows_are_identified_and_audited() -> Result<()> {
    let dir = std::env::temp_dir().join(format!("sse-linux-corpus-{}", std::process::id()));
    std::fs::create_dir(&dir)?;
    let mut oracle = Oracle::spawn(&crate::build_runner(&dir)?)?;
    let form = Form {
        mnemonic: "movss".into(),
        code: "Movss_xmm_xmmm32".into(),
        bytes: vec![0xf3, 0x0f, 0x10, 0xc1],
        sites: 1,
        immediate: None,
    };
    let mut rows = Vec::new();
    generate_filtered(
        &mut oracle,
        &[form],
        0,
        SEED,
        |_, case| case == "edge-0-0",
        |f| {
            assert_eq!(f.backend, "linux-x86_64");
            serde_json::to_writer(&mut rows, &f)?;
            rows.push(b'\n');
            Ok(())
        },
    )?;
    assert_eq!(audit(rows.as_slice())?.fixtures, 8);
    drop(oracle);
    std::fs::remove_dir_all(dir)?;
    Ok(())
}
