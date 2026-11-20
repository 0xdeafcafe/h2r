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
        "native-sse-{native}-{mode}-{}-{:?}",
        std::env::var("H2SSE_TEST_ARCH").unwrap_or_default(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let mut source = std::fs::read_to_string(root.join("../simd/c/test/soft_harness.c")).unwrap();
    source = source.replace("#include \"h2simd.h\"", "#include \"h2sse_native.h\"");
    for name in &OPS[..10] {
        source = source.replace(&format!("h2s_{name}("), &format!("h2sn_{name}_mode(H2_FP_SAMPLE(), "));
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
        .arg("-DH2_SSE_HOST_RUN=1")
        .arg("-DH2_SSE_STICKY_PE=1")
        .arg("-DH2_SSE_PACKED_X86=1")
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
    // FastTwoSum must detect a tiny lost addend even when binary64
    // addition would have lost it too. Alternate signed zeros, exact
    // cancellation and ties, and put the inexact lane last.
    let mut sums = Vec::new();
    for op in 0..4 {
        for pe in [0, 0x20] {
            for (a, b) in [(1.0f32, -1.0f32), (-0.0, 0.0), (2.0f32.powi(60), 2.0f32.powi(-60)),
                (1.0, 2.0f32.powi(-24)), (-1.0, -2.0f32.powi(-24))] {
                let lane = if op & 1 != 0 { 96 } else { 0 };
                sums.push(In {
                    op, pred: 0, mxcsr: 0x1f80 | pe,
                    a: u128::from(a.to_bits()) << lane,
                    b: u128::from(b.to_bits()) << lane,
                });
            }
        }
    }
    // One rejected lane must delegate the whole packed op, even when PE
    // was already set. Include zero divisors, negative sqrt and sNaN.
    for op in [1, 3, 5, 7, 9] {
        let ones = 0x3f800000_3f800000_3f800000_3f800000u128;
        for high in [0u32, 0xbf800000, 1, 0x7f800001] {
            for mxcsr in [0x1fa0, 0x1f20] {
                sums.push(In {
                    op,
                    pred: 0,
                    mxcsr,
                    a: ones,
                    b: (ones & (u128::MAX >> 32)) | (u128::from(high) << 96),
                });
            }
        }
    }
    check(&exe, &sums);
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

/// Strict FP is scoped to the helpers. The header is included before the
/// generated handlers (and before h2x87_native.h, which is empty when
/// `H2_X87_NATIVE=0`), so `FENV_ACCESS ON` must not stay in effect for code
/// that follows it.
#[test]
fn strict_fp_access_ends_with_the_header() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("native-sse-fenv-scope");
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("after.c");
    std::fs::write(
        &src,
        "#include <stdint.h>\ntypedef unsigned __int128 h2u128;\n#include \"h2sse_native.h\"\n\
         float h2_after_header(float a, float b) { return a + b; }\n",
    )
    .unwrap();
    let ir = dir.join("after.ll");
    let out = Command::new("clang")
        .args([
            "-std=c11",
            "-O1",
            "-fno-fast-math",
            "-ffp-contract=off",
            "-S",
            "-emit-llvm",
        ])
        .arg("-DH2_SSE_NATIVE=1")
        .arg("-I")
        .arg(root.join("c"))
        .arg("-I")
        .arg(root.join("../simd/c"))
        .arg(&src)
        .arg("-o")
        .arg(&ir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ir = std::fs::read_to_string(&ir).unwrap();
    let body = ir
        .split("@h2_after_header(")
        .nth(1)
        .and_then(|rest| rest.split("\n}").next())
        .expect("h2_after_header in the IR");
    assert!(body.contains("fadd"), "{body}");
    assert!(
        !body.contains("constrained"),
        "FENV_ACCESS leaked past h2sse_native.h:\n{body}"
    );
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

/// Build `tests/c/sse_fast_equiv.c` at `opt` and run `count` random cases.
fn fast_equiv(opt: &str, sanitize: bool, count: u64, seed: u64) -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let arch = std::env::var("H2SSE_TEST_ARCH").unwrap_or_default();
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("native-sse-equiv-{opt}-{sanitize}-{arch}"));
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("equiv");
    let mut command = Command::new("clang");
    if !arch.is_empty() {
        command.args(["-arch", &arch]);
    }
    command.args([
        "-std=c11",
        opt,
        "-fno-fast-math",
        "-ffp-contract=off",
        "-Wall",
        "-Wextra",
        "-Werror",
        "-DH2_SSE_NATIVE=1",
        "-DH2_SSE_STICKY_PE=1",
        "-DH2_SSE_PACKED_X86=1",
    ]);
    if sanitize {
        command.args(["-fsanitize=undefined", "-fno-sanitize-recover=all"]);
    }
    let out = command
        .arg("-I")
        .arg(root.join("c"))
        .arg("-I")
        .arg(root.join("../simd/c"))
        .arg(root.join("tests/c/sse_fast_equiv.c"))
        .arg("-o")
        .arg(&exe)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&exe)
        .arg(count.to_string())
        .arg(seed.to_string())
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    String::from_utf8(run.stdout).unwrap()
}

/// The inlined fast paths (NEON for packed ops on arm64) against the
/// integer authority: millions of random lanes, edge exponents, signed
/// zeros, denormals, infinities, NaNs, cancellation, every MXCSR mode.
/// Each form must actually take the fast path for a real share of cases.
#[test]
fn inlined_fast_paths_match_the_authority() {
    let count = std::env::var("H2SSE_FAST_COUNT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4_000_000u64);
    for (opt, sanitize, seed) in [("-O1", true, 0x5eed_0001u64), ("-O2", false, 0x5eed_0002)] {
        let report = fast_equiv(opt, sanitize, count, seed);
        let mut forms = 0;
        for line in report.lines() {
            let f: Vec<&str> = line.split(' ').collect();
            let (hit, total): (u64, u64) = (f[1].parse().unwrap(), f[2].parse().unwrap());
            assert!(
                hit * 10 > total,
                "{opt} {}: only {hit} of {total} took the fast path",
                f[0]
            );
            forms += 1;
        }
        assert_eq!(forms, 10, "{report}");
        eprintln!("{opt}:\n{report}");
    }
}

/// The guarded path must inline into a non-strict caller (strict-FP
/// helpers cannot), leaving only the cold authority call out of line.
#[test]
fn fast_paths_inline_into_generated_handlers() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("native-sse-inline");
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("caller.c");
    std::fs::write(
        &src,
        "#include <stdint.h>\ntypedef unsigned __int128 h2u128;\n#include \"h2sse_native.h\"\n\
         uint32_t h2_caller(uint32_t *mx, h2u128 a, h2u128 b, h2u128 *o) {\n\
         uint32_t s = h2sn_mulps(mx, a, b, o); s |= h2sn_addps(mx, *o, b, o);\n\
         s |= h2sn_divss(mx, *o, (uint32_t)b, o); return s | h2sn_sqrtps(mx, *o, o); }\n",
    )
    .unwrap();
    let ir = dir.join("caller.ll");
    let out = Command::new("clang")
        .args([
            "-std=c11",
            "-O1",
            "-fno-fast-math",
            "-ffp-contract=off",
            "-S",
            "-emit-llvm",
        ])
        .arg("-DH2_SSE_NATIVE=1")
        .arg("-I")
        .arg(root.join("c"))
        .arg("-I")
        .arg(root.join("../simd/c"))
        .arg(&src)
        .arg("-o")
        .arg(&ir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ir = std::fs::read_to_string(&ir).unwrap();
    let body = ir
        .split("@h2_caller(")
        .nth(1)
        .and_then(|rest| rest.split("\n}").next())
        .expect("h2_caller in the IR");
    for line in body.lines().filter(|l| l.contains(" call ")) {
        assert!(
            line.contains("_slow(") || line.contains("asm") || line.contains("@llvm."),
            "unexpected call in the fast path: {line}"
        );
    }
    assert!(body.contains("fmul") && body.contains("fadd"), "{body}");
    assert!(!body.contains("constrained"), "{body}");
}

/// Per-op timing (`tests/c/sse_bench.c`): the native entry points against
/// the integer authority (`H2_SSE_NATIVE=0`), at the production `-O1`.
/// To compare with an older header, build the same file against it.
#[test]
#[ignore = "timing experiment; run serially"]
fn benchmark_sse_ops() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("native-sse-bench");
    std::fs::create_dir_all(&dir).unwrap();
    for native in [0, 1] {
        let exe = dir.join(format!("bench-{native}"));
        let out = Command::new("clang")
            .args(["-std=c11", "-O1", "-fno-fast-math", "-ffp-contract=off"])
            .arg(format!("-DH2_SSE_NATIVE={native}"))
            .arg("-I")
            .arg(root.join("c"))
            .arg("-I")
            .arg(root.join("../simd/c"))
            .arg(root.join("tests/c/sse_bench.c"))
            .arg("-o")
            .arg(&exe)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let run = Command::new(&exe).output().unwrap();
        assert!(run.status.success());
        println!(
            "H2_SSE_NATIVE={native}\n{}",
            String::from_utf8_lossy(&run.stdout)
        );
    }
}
