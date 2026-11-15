//! Runs everywhere (arm64, no Rosetta, no clang): reads only the committed fixtures.
use sse_oracle::fixtures as fx;
use sse_oracle::fixtures::Fixtures;
use std::collections::BTreeMap;

const FORMS: usize = 228;
const MNEMONICS: usize = 85;
const PER_FORM: usize = 22_472; // (52*52 + 64 + 14 + 20 + 7) * 8
const TOTAL: usize = 5_123_616;

#[test]
fn counts_per_form_and_category() {
    // (code, instruction) -> category -> count
    let mut forms: BTreeMap<(String, Vec<u8>), BTreeMap<&'static str, usize>> = BTreeMap::new();
    let mut mnemonics: BTreeMap<String, usize> = BTreeMap::new();
    let mut total = 0;
    let mut modes: BTreeMap<(String, Vec<u8>), [usize; 8]> = BTreeMap::new();
    let mut exceptions = 0;
    for f in sse_oracle::fixtures() {
        assert_eq!((f.schema, f.backend.as_str()), (1, "rosetta2-x86_64"));
        let category = match f.case.split('-').next().unwrap() {
            "edge" => "edge",
            "random" => "random",
            "shift" => "shift",
            "integer" => "integer",
            "sticky" => "sticky",
            other => panic!("unknown case {other}"),
        };
        let key = (f.code.clone(), f.instruction.clone());
        *forms
            .entry(key.clone())
            .or_default()
            .entry(category)
            .or_default() += 1;
        let m = ((f.input.mxcsr >> 13) & 3) * 2 + ((f.input.mxcsr >> 15) & 1);
        modes.entry(key).or_default()[m as usize] += 1;
        *mnemonics.entry(f.mnemonic.clone()).or_default() += 1;
        // Invariants the generator guarantees for every row.
        assert_eq!(f.input.mxcsr & 0x1fc0, 0x1f80);
        match &f.output.state {
            Some(s) => {
                assert!(f.output.exception.is_none());
                if f.mnemonic != "ldmxcsr" {
                    assert_eq!(s.mxcsr & !63, f.input.mxcsr & !63);
                }
            }
            None => exceptions += 1,
        }
        total += 1;
    }
    assert_eq!(total, TOTAL);
    assert_eq!(exceptions, 0);
    assert_eq!(forms.len(), FORMS);
    assert_eq!(mnemonics.len(), MNEMONICS);
    for (form, categories) in &forms {
        let expected: BTreeMap<&str, usize> = [
            ("edge", 52 * 52 * 8),
            ("random", 64 * 8),
            ("shift", 14 * 8),
            ("integer", 20 * 8),
            ("sticky", 7 * 8),
        ]
        .into();
        assert_eq!(categories, &expected, "{form:?}");
        assert_eq!(categories.values().sum::<usize>(), PER_FORM);
        assert_eq!(modes[form], [PER_FORM / 8; 8], "{form:?}");
    }
    // Spot-check multi-form mnemonics against docs/sse-oracle.md.
    for (m, n) in [
        ("addps", 44_944),
        ("shufps", 1_191_016),
        ("sfence", 22_472),
        ("psllw", 224_720),
    ] {
        assert_eq!(mnemonics[m], n, "{m}");
    }
}

#[test]
fn known_values() {
    // addss 1.0 + 1.0 with default MXCSR: the pair(1.0, 1.0) edge case, round-to-nearest, FTZ off.
    let f = sse_oracle::fixtures()
        .find(|f| f.mnemonic == "addss" && f.case == "edge-24-24" && f.input.mxcsr == 0x1f80)
        .expect("fixture present");
    assert_eq!(f.output.state.unwrap().xmm[0][0], 0x40000000);
    // Documented default NaN: +inf + -inf is 0xffc00000 with IE set.
    let f = sse_oracle::fixtures()
        .find(|f| f.mnemonic == "addss" && f.case == "edge-2-3" && f.input.mxcsr == 0x1f80)
        .unwrap();
    let s = f.output.state.unwrap();
    assert_eq!(s.xmm[0][0], 0xffc00000);
    assert_eq!(s.mxcsr & 63, 1);
}

#[test]
fn case_ids_roundtrip() {
    for id in fx::selected_cases() {
        assert_eq!(fx::case_id(&fx::case_name(id)), Some(id));
    }
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}
fn words(s: &sse_oracle::State) -> Vec<u8> {
    let mut w: Vec<u32> = s
        .xmm
        .iter()
        .flatten()
        .chain(s.mm.iter().flatten())
        .copied()
        .collect();
    w.extend([s.mxcsr, s.eflags, s.eax, s.ecx]);
    w.extend(s.memory);
    w.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// Digest over every canonical record (form, case, full input, full output),
/// frozen from an independent computation over the original JSONL corpus
/// (Python: per-record CRC32, folded as acc = acc*0x100000001b3 ^ crc).
#[test]
fn frozen_digest_of_all_records() {
    let (mut acc, mut n) = (0u64, 0usize);
    for f in sse_oracle::fixtures() {
        let mut b = f.code.into_bytes();
        b.push(0);
        b.extend(&f.instruction);
        b.push(0xff);
        b.extend(f.case.as_bytes());
        b.push(0);
        b.extend(words(&f.input));
        match (&f.output.state, f.output.exception) {
            (Some(s), None) => {
                b.push(1);
                b.extend(words(s));
            }
            (None, Some(sig)) => {
                b.push(2);
                b.extend(sig.to_le_bytes());
            }
            _ => panic!("bad observation"),
        }
        acc = acc.wrapping_mul(0x100000001b3) ^ crc32(&b) as u64;
        n += 1;
    }
    assert_eq!((n, acc), (TOTAL, DIGEST));
}
const DIGEST: u64 = 0x69f2_9ec9_65a2_23f0;

fn find(m: &str, case: &str, mxcsr: u32) -> Vec<sse_oracle::Fixture> {
    let v: Vec<_> = sse_oracle::fixtures()
        .filter(|f| f.mnemonic == m && f.case == case && f.input.mxcsr == mxcsr)
        .collect();
    assert!(!v.is_empty(), "{m} {case}");
    v
}

#[test]
fn representative_outputs() {
    // MOVNTPS stores xmm0 (= a) to memory; the memory delta must survive.
    for f in find("movntps", "edge-24-0", 0x1f80) {
        assert_eq!(f.output.state.unwrap().memory, [0x3f800000; 4]);
    }
    for f in find("movups", "edge-24-0", 0x1f80) {
        if f.code.ends_with("_xmmm128_xmm") {
            assert_eq!(f.output.state.unwrap().memory, [0x3f800000; 4]);
        }
    }
    // CVTTSS2SI truncates 1.5 to 1; 2^31 is the integer indefinite with IE.
    for f in find("cvttss2si", "edge-30-30", 0x1f80) {
        assert_eq!(f.output.state.unwrap().eax, 1);
    }
    for f in find("cvttss2si", "edge-41-41", 0x1f80) {
        let s = f.output.state.unwrap();
        assert_eq!((s.eax, s.mxcsr & 1), (0x80000000, 1));
    }
    // MOVMSKPS gathers four sign bits.
    for f in find("movmskps", "edge-1-1", 0x1f80) {
        assert_eq!(f.output.state.unwrap().eax, 0xf);
    }
    // COMISS on QNaN vs 1.0: unordered => ZF=PF=CF=1, IE set.
    for f in find("comiss", "edge-4-24", 0x1f80) {
        let s = f.output.state.unwrap();
        assert_eq!((s.eflags & 0x45, s.mxcsr & 1), (0x45, 1));
    }
    // MAXSS/MINSS return the second (source) operand for +0 vs -0.
    for m in ["maxss", "minss"] {
        for f in find(m, "edge-0-1", 0x1f80) {
            assert_eq!(f.output.state.unwrap().xmm[0][0], 0x80000000, "{m}");
        }
    }
    // Rounding mode and FTZ round-trip through LDMXCSR (rc=3, ftz=1).
    for f in find("ldmxcsr", "edge-24-24", 0xff80) {
        assert_eq!(f.output.state.unwrap().mxcsr, 0xff80);
    }
    // Round toward +inf (rc=2): 1.0 + 2^-24 rounds up to 1.0000001. Forms that
    // alias both operands compute 1.0 + 1.0 instead.
    let r: Vec<u32> = find("addss", "edge-24-28", 0x1f80 | 2 << 13)
        .into_iter()
        .map(|f| f.output.state.unwrap().xmm[0][0])
        .collect();
    assert!(r.contains(&0x3f800001) && r.iter().all(|x| [0x3f800001, 0x40000000].contains(x)));
    let r: Vec<u32> = find("addss", "edge-24-28", 0x1f80)
        .into_iter()
        .map(|f| f.output.state.unwrap().xmm[0][0])
        .collect();
    assert!(r.contains(&0x3f800000));
    // Shift by an out-of-range count clears the MMX register (PSLLQ by 64).
    for f in find("psllq", "shift-count-64", 0x1f80) {
        let s = f.output.state.unwrap();
        if f.code.ends_with("_mm_mmm64") {
            assert_eq!(s.mm[0], [0, 0]);
        }
    }
}

// ---- malformed input ----

fn header(forms: u32) -> Vec<u8> {
    let mut v = fx::MAGIC.to_vec();
    v.extend(0x243f6a8885a308d3u64.to_le_bytes());
    v.extend(forms.to_le_bytes());
    v
}
fn form(cases: u32) -> Vec<u8> {
    let mut v = vec![1, b'x', 1, b'y', 0];
    v.extend(cases.to_le_bytes());
    v
}
fn drain(raw: Vec<u8>) -> anyhow::Result<usize> {
    let mut n = 0;
    for f in Fixtures::from_raw(raw)? {
        f?;
        n += 1;
    }
    Ok(n)
}
fn one_case(id: u16, masks: [u64; 8]) -> Vec<u8> {
    let mut v = header(1);
    v.extend(form(1));
    v.extend(id.to_le_bytes());
    for m in masks {
        v.extend(m.to_le_bytes());
    }
    v
}

#[test]
fn rejects_truncation_everywhere() {
    let raw = fx::raw().unwrap();
    for cut in [1, 3, 8, 100, 5000] {
        assert!(drain(raw[..raw.len() - cut].to_vec()).is_err(), "cut {cut}");
    }
    assert!(Fixtures::from_raw(raw[..12].to_vec()).is_err());
    assert!(drain(one_case(0, [0; 8])[..40].to_vec()).is_err());
}
#[test]
fn rejects_trailing_bytes() {
    let mut v = one_case(0, [0; 8]);
    assert_eq!(drain(v.clone()).unwrap(), 8);
    v.push(0);
    assert!(drain(v).is_err());
}
#[test]
fn rejects_invalid_case_ids() {
    for id in [2704 + 64, 2999, 3014, 3099, 3120, 3207, 4000, u16::MAX] {
        assert!(drain(one_case(id, [0; 8])).is_err(), "{id}");
    }
    // Duplicate or descending ids within a form.
    let mut v = header(1);
    v.extend(form(2));
    for id in [5u16, 5] {
        v.extend(id.to_le_bytes());
        v.extend([0u8; 64]);
    }
    assert!(drain(v).is_err());
}
#[test]
fn rejects_reserved_mask_bits_and_bad_exceptions() {
    assert!(drain(one_case(0, [1 << 56, 0, 0, 0, 0, 0, 0, 0])).is_err());
    assert!(drain(one_case(0, [1 << 62, 0, 0, 0, 0, 0, 0, 0])).is_err());
    let exc = 1u64 << 63;
    assert!(
        drain(one_case(0, [exc, 0, 0, 0, 0, 0, 0, 0])).is_err(),
        "zero signal"
    );
    assert!(drain(one_case(0, [exc | 1 << 8 | 8, 0, 0, 0, 0, 0, 0, 0])).is_err());
    assert_eq!(drain(one_case(0, [exc | 8; 8])).unwrap(), 8);
}
#[test]
fn many_empty_forms_do_not_overflow_the_stack() {
    let mut v = header(100_000);
    for _ in 0..100_000 {
        v.extend(form(0));
    }
    assert_eq!(drain(v.clone()).unwrap(), 0);
    // Declared forms that are missing are an error, not a hang or panic.
    let mut v = header(u32::MAX);
    v.extend(form(0));
    assert!(drain(v).is_err());
}
#[test]
fn decoded_size_is_limited_while_streaming() {
    let mut enc = zstd::stream::Encoder::new(Vec::new(), 19).unwrap();
    let chunk = vec![0u8; 1 << 20];
    for _ in 0..128 {
        std::io::Write::write_all(&mut enc, &chunk).unwrap();
    }
    let packed = enc.finish().unwrap();
    assert!(packed.len() < 64 << 10, "{}", packed.len());
    let err = fx::decompress(&packed, fx::MAX_DECODED).unwrap_err();
    assert!(err.to_string().contains("exceed"), "{err}");
    assert_eq!(fx::decompress(&packed, 200 << 20).unwrap().len(), 128 << 20);
}
