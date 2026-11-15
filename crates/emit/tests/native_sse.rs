//! Synthetic native-vs-authority sweeps and the independent SSE oracle.
#[path = "../../simd/tests/common/mod.rs"]
mod simd_common;
use simd_common::*;
use std::path::PathBuf;
use std::process::Command;

fn harness(native: bool) -> PathBuf {
    harness_mode(native, 0)
}

fn harness_mode(native: bool, mode: u32) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "native-sse-{native}-{mode}-{}",
        std::env::var("H2SSE_TEST_ARCH").unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let mut source = std::fs::read_to_string(root.join("../simd/c/test/soft_harness.c")).unwrap();
    source = source.replace("#include \"h2simd.h\"", "#include \"h2sse_native.h\"");
    for name in &OPS[..10] {
        source = source.replace(&format!("h2s_{name}("), &format!("h2sn_{name}("));
    }
    source = source.replace(
        "    while (fread",
        r#"
#if defined(__aarch64__)
    uint64_t original_control, test_control;
    __asm__ volatile("mrs %0, fpcr" : "=r"(original_control));
    test_control = original_control;
    if (H2SSE_HOST_MODE == 1) test_control |= UINT64_C(1) << 22;
    if (H2SSE_HOST_MODE == 2) test_control |= UINT64_C(1) << 24;
    if (H2SSE_HOST_MODE == 3) test_control |= UINT64_C(1) << 8;
    __asm__ volatile("msr fpcr, %0" :: "r"(test_control));
#elif defined(__x86_64__)
    uint32_t original_control, test_control;
    __asm__ volatile("stmxcsr %0" : "=m"(original_control));
    test_control = original_control;
    if (H2SSE_HOST_MODE == 1) test_control |= 1u << 13;
    if (H2SSE_HOST_MODE == 2) test_control |= 0x8040u;
    if (H2SSE_HOST_MODE == 3) test_control &= ~0x80u;
    __asm__ volatile("ldmxcsr %0" :: "m"(test_control));
    /* Rosetta forces exception masks on. Test preservation of the controls
     * actually installed; it cannot validate unmasked host exceptions. */
    __asm__ volatile("stmxcsr %0" : "=m"(test_control));
#endif
    while (fread"#,
    );
    source = source.replace(
        "    return 0;\n}",
        r#"
#if defined(__aarch64__)
    uint64_t after;
    __asm__ volatile("mrs %0, fpcr" : "=r"(after));
    __asm__ volatile("msr fpcr, %0" :: "r"(original_control));
    if (after != test_control) return 4;
#elif defined(__x86_64__)
    uint32_t after;
    __asm__ volatile("stmxcsr %0" : "=m"(after));
    __asm__ volatile("ldmxcsr %0" :: "m"(original_control));
    if ((after & ~63u) != (test_control & ~63u)) return 4;
#endif
    return 0;
}"#,
    );
    let src = dir.join("harness.c");
    std::fs::write(&src, source).unwrap();
    let exe = dir.join("harness");
    let mut command = Command::new("clang");
    if let Ok(arch) = std::env::var("H2SSE_TEST_ARCH") {
        command.args(["-arch", &arch]);
    }
    let out = command
        .args([
            "-std=c11",
            "-O1",
            "-fno-fast-math",
            "-ffp-contract=off",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-fsanitize=undefined",
            "-fno-sanitize-recover=all",
        ])
        .arg(format!("-DH2_SSE_NATIVE={}", u8::from(native)))
        .arg(format!("-DH2SSE_HOST_MODE={mode}"))
        .arg("-I")
        .arg(root.join("c"))
        .arg("-I")
        .arg(root.join("../simd/c"))
        .arg("-I")
        .arg(root.join("../simd/c/test"))
        .arg(src)
        .arg("-o")
        .arg(&exe)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    exe
}

fn check(exe: &std::path::Path, input: &[In]) {
    let want: Vec<_> = input.iter().map(run_rust).collect();
    let got = run_harness(&mut Command::new(exe), input);
    compare(input, &want, &got, "native SSE vs integer authority").unwrap();
}

#[test]
fn synthetic_modes_edges_and_random_states() {
    let exe = harness(true);
    check(&exe, &edge_records(false));
    let mut rng = Rng(0x3193a9b17);
    for batch in 0..20 {
        let input: Vec<_> = (0..50_000)
            .map(|_| {
                let mut i = record(&mut rng, false);
                // Half the batches force the native operand and guest-mode guards.
                if batch % 2 == 0 {
                    i.op %= 10;
                    i.mxcsr = 0x1f80 | (rng.next() as u32 & 0x803f);
                    let value = |r: &mut Rng| {
                        (r.next() as u32 & 0x807f_ffff) | ((67 + r.below(121) as u32) << 23)
                    };
                    i.a = vector(&mut rng, value);
                    i.b = vector(&mut rng, value);
                    if i.op >= 8 {
                        i.b &= !0x8000_0000_8000_0000_8000_0000_8000_0000;
                    }
                }
                // The authority ignores DAZ even for direct calls with this bit.
                if rng.below(3) == 0 {
                    i.mxcsr |= 0x40;
                }
                i
            })
            .collect();
        check(&exe, &input);
    }
    let off = harness(false);
    check(&off, &edge_records(false));
}

#[test]
fn arithmetic_matches_committed_sse_oracle() {
    let exe = harness(true);
    let mut input = Vec::new();
    let mut want = Vec::new();
    let mut checked = 0;
    for f in sse_oracle::fixtures() {
        let Some(op) = OPS[..10].iter().position(|n| *n == f.mnemonic) else {
            continue;
        };
        let k = usize::from(f.instruction[0] == 0xf3);
        let m = f.instruction[k + 2];
        let dst = ((m >> 3) & 7) as usize;
        let src = (m & 7) as usize;
        let words = |w: [u32; 4]| {
            w.iter()
                .enumerate()
                .fold(0, |a, (i, b)| a | (*b as u128) << (i * 32))
        };
        let output = f.output.state.unwrap();
        input.push(In {
            op: op as u8,
            pred: 0,
            mxcsr: f.input.mxcsr,
            a: words(f.input.xmm[dst]),
            b: words(if m >> 6 == 3 {
                f.input.xmm[src]
            } else {
                f.input.memory
            }),
        });
        want.push(Out {
            xm: false,
            mxcsr: output.mxcsr,
            r: words(output.xmm[dst]),
        });
        if input.len() == 8192 {
            let got = run_harness(&mut Command::new(&exe), &input);
            compare(&input, &want, &got, "native SSE vs oracle").unwrap();
            checked += input.len();
            input.clear();
            want.clear();
        }
    }
    let got = run_harness(&mut Command::new(&exe), &input);
    compare(&input, &want, &got, "native SSE vs oracle").unwrap();
    checked += input.len();
    assert!(
        checked > 300_000,
        "oracle sweep unexpectedly small: {checked}"
    );
    eprintln!("native SSE matches {checked} independent oracle records");
}

#[test]
fn host_rounding_flush_and_trap_controls_are_preserved() {
    let input = edge_records(false);
    for mode in [1, 2, 3] {
        check(&harness_mode(true, mode), &input);
    }
}

#[test]
#[ignore = "live synthetic SSE oracle; requires clang and Rosetta"]
fn live_synthetic_oracle_sweep() {
    use iced_x86::code_asm::*;
    assert!(
        sse_oracle::rosetta_available(),
        "live x86 oracle unavailable"
    );
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("native-sse-live-oracle");
    let binary = sse_oracle::build_runner(&dir).unwrap();
    let mut oracle = sse_oracle::Oracle::spawn(&binary).unwrap();
    let mut rng = Rng(0x9a731ec4);
    let mut input = Vec::new();
    let mut want = Vec::new();
    let words = |x: u128| std::array::from_fn(|i| (x >> (32 * i)) as u32);
    let bits = |x: [u32; 4]| {
        x.iter()
            .enumerate()
            .fold(0, |a, (i, w)| a | (*w as u128) << (32 * i))
    };
    for (op, name) in OPS[..10].iter().enumerate() {
        let mut assembler = CodeAssembler::new(64).unwrap();
        match *name {
            "addss" => assembler.addss(xmm0, xmm1),
            "addps" => assembler.addps(xmm0, xmm1),
            "subss" => assembler.subss(xmm0, xmm1),
            "subps" => assembler.subps(xmm0, xmm1),
            "mulss" => assembler.mulss(xmm0, xmm1),
            "mulps" => assembler.mulps(xmm0, xmm1),
            "divss" => assembler.divss(xmm0, xmm1),
            "divps" => assembler.divps(xmm0, xmm1),
            "sqrtss" => assembler.sqrtss(xmm0, xmm1),
            "sqrtps" => assembler.sqrtps(xmm0, xmm1),
            _ => unreachable!(),
        }
        .unwrap();
        let instruction = assembler.assemble(0).unwrap();
        for rc in 0..4 {
            for ftz in 0..2 {
                for case in 0..128 {
                    let value = |r: &mut Rng| {
                        (r.next() as u32 & 0x807f_ffff) | ((67 + r.below(121) as u32) << 23)
                    };
                    let mut a = vector(&mut rng, value);
                    let mut b = vector(&mut rng, value);
                    if case < EDGES.len() {
                        a = bits(std::array::from_fn(|l| EDGES[(case + l) % EDGES.len()]));
                        b = bits(std::array::from_fn(|l| {
                            EDGES[(case * 17 + l + 1) % EDGES.len()]
                        }));
                    }
                    let i = In {
                        op: op as u8,
                        pred: 0,
                        mxcsr: 0x1f80 | rc << 13 | ftz << 15 | (case as u32 & 63),
                        a,
                        b,
                    };
                    let mut state = sse_oracle::State {
                        mxcsr: i.mxcsr,
                        eflags: 0x202 | (rng.next() as u32 & 0x8d5),
                        ..Default::default()
                    };
                    state.xmm[0] = words(a);
                    state.xmm[1] = words(b);
                    let observed = oracle.query(&instruction, &state).unwrap();
                    assert!(
                        observed.exception.is_none(),
                        "{name} rc={rc} ftz={ftz} case={case}"
                    );
                    let observed = observed.state.unwrap();
                    assert_eq!(observed.eflags & 0x8d5, state.eflags & 0x8d5);
                    assert_eq!(&observed.xmm[1..], &state.xmm[1..]);
                    assert_eq!(observed.memory, state.memory);
                    input.push(i);
                    want.push(Out {
                        xm: false,
                        mxcsr: observed.mxcsr,
                        r: bits(observed.xmm[0]),
                    });
                }
            }
        }
    }
    let rust: Vec<_> = input.iter().map(run_rust).collect();
    compare(&input, &want, &rust, "Rust vs live synthetic oracle").unwrap();
    let got = run_harness(&mut Command::new(harness(true)), &input);
    compare(&input, &want, &got, "native SSE vs live synthetic oracle").unwrap();
    eprintln!(
        "native SSE matches {} live synthetic oracle records",
        input.len()
    );
}
