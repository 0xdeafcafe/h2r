//! The compiler switches through the real build (synthetic code, no game
//! data): `H2_PGO=gen` builds a module that writes a raw profile,
//! `llvm-profdata merge` reads it, `H2_PGO=use:<profdata>` builds with it,
//! and `H2_OPT=3` and `H2_CPU` compile. Every mode keys its objects apart
//! and computes the same result.
//!
//! One test: the switches are environment variables `cflags` reads, and
//! tests in one binary share the environment.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::function;
use emit::compile::{self, Arch};
use emit::host::{BRIDGE, CpuAbi, Ctx, ENTRY, Generated, HARNESS_EXIT, JUMP, RETURN, TransferAbi};
use iced_x86::code_asm::*;
use ir::interp::FlatMemory;

const A: u32 = 0x0040_0000;
const STACK: u32 = 0x0010_8000;

/// `sum(n)`: eax = n + (n - 1) + ... + 1.
fn sum() -> Vec<emit::Owner> {
    emit::plan(vec![function(A, |a| {
        let mut l = a.create_label();
        a.xor(eax, eax)?;
        a.mov(ecx, dword_ptr(esp + 4))?;
        a.set_label(&mut l)?;
        a.add(eax, ecx)?;
        a.dec(ecx)?;
        a.jnz(l)?;
        a.ret()
    })])
}

fn run_sum(g: &Generated, n: u32) -> u32 {
    let mut mem = FlatMemory::new(STACK - 0x1000, 0x1000);
    mem.load(STACK - 8, &HARNESS_EXIT.to_le_bytes());
    mem.load(STACK - 4, &n.to_le_bytes());
    let mut cpu = CpuAbi::default();
    cpu.gpr[4] = STACK - 8;
    let mut ctx = Ctx::new(mem);
    let (t, _) = g.run(&mut cpu, &mut ctx, TransferAbi::new(JUMP, ENTRY, A), 16);
    assert_eq!((t.kind, t.target_pc), (RETURN, HARNESS_EXIT), "{t:x?}");
    cpu.gpr[0]
}

/// Emit, compile and load `sum` under `name` with the environment as it
/// is: the module and its object names (the cache keys).
fn build(header: &Path, name: &str) -> (Generated, Vec<String>) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("pgo")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    emit::write_module(&sum(), &src).unwrap();
    compile::install_header(&src, header).unwrap();
    let b = compile::build(&src, &dir, Arch::host(), 2).unwrap_or_else(|e| panic!("{name}: {e}"));
    let dylib = dir.join(format!("lib{name}.dylib"));
    compile::host_dylib(&b, &src, &[("bridge.c", BRIDGE)], &dylib)
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    let names = b
        .objects
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    // SAFETY: built just now from this crate's output for this host.
    (unsafe { Generated::load(&dylib) }.unwrap(), names)
}

/// `llvm-profdata` from the active developer directory, the one beside
/// the `clang` the build runs.
fn profdata_tool() -> Option<PathBuf> {
    let o = Command::new("xcrun")
        .args(["--find", "llvm-profdata"])
        .output()
        .ok()?;
    let p = PathBuf::from(String::from_utf8_lossy(&o.stdout).trim());
    (o.status.success() && p.is_file()).then_some(p)
}

/// The `N.N.N` after "version" in a `--version` banner.
fn version(text: &str) -> Option<String> {
    let mut words = text.split_whitespace();
    words.find(|w| *w == "version")?;
    words.next().map(str::to_owned)
}

fn set(k: &str, v: impl AsRef<std::ffi::OsStr>) {
    // SAFETY: this binary's only test; nothing else reads the environment
    // concurrently.
    unsafe { std::env::set_var(k, v) }
}

#[test]
fn pgo_cpu_and_o3_build_run_and_key_apart() {
    const NAME: &str = "pgo_cpu_and_o3_build_run_and_key_apart";
    let Some(header) = common::abi_header() else {
        common::skip(NAME, "needs h2abi.h (H2ABI_HEADER)");
        return;
    };
    if !Arch::host().is_macos() {
        common::skip(NAME, "the profile runtime link is macOS-only here");
        return;
    }
    let Some(tool) = profdata_tool() else {
        common::skip(NAME, "no llvm-profdata (xcrun --find llvm-profdata)");
        return;
    };
    let cc = compile::compiler().unwrap();
    let banner = |p: &Path| {
        String::from_utf8_lossy(&Command::new(p).arg("--version").output().unwrap().stdout)
            .into_owned()
    };
    let (cc_version, tool_version) = (version(&banner(&cc)), version(&banner(&tool)));
    assert!(
        cc_version.is_some() && cc_version == tool_version,
        "clang {cc_version:?} vs llvm-profdata {tool_version:?}"
    );

    let scratch = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("pgo")
        .join("profiles");
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();
    for k in ["H2_PGO", "H2_CPU", "H2_HOT"] {
        // SAFETY: as in `set`.
        unsafe { std::env::remove_var(k) };
    }
    set("H2_OPT", "2");
    // Every source at H2_OPT: tiers would compile this synthetic one cold.
    set("H2_OPT_COLD", "off");

    let (plain, plain_keys) = build(&header, "plain");
    assert_eq!(run_sum(&plain, 100), 5050);
    assert!(plain.write_profile().is_none(), "not instrumented");

    // Instrumented: the profile goes where H2_PGO_OUT says, on request.
    let raw = scratch.join("sum.profraw");
    set("H2_PGO_OUT", &raw);
    set("H2_PGO", "gen");
    let (generated, gen_keys) = build(&header, "gen");
    assert_ne!(gen_keys, plain_keys, "H2_PGO=gen keys its objects");
    for n in 1..=200 {
        assert_eq!(run_sum(&generated, n), n * (n + 1) / 2);
    }
    let written = generated
        .write_profile()
        .expect("an instrumented module")
        .unwrap();
    assert_eq!(written, raw);
    assert!(
        std::fs::metadata(&raw).unwrap().len() > 0,
        "empty raw profile"
    );

    let profdata = scratch.join("sum.profdata");
    let o = Command::new(&tool)
        .arg("merge")
        .arg("-o")
        .arg(&profdata)
        .arg(&raw)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "llvm-profdata merge: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    let o = Command::new(&tool)
        .args(["show", "--all-functions", "--counts"])
        .arg(&profdata)
        .output()
        .unwrap();
    let shown = String::from_utf8_lossy(&o.stdout);
    assert!(
        o.status.success() && shown.contains("h2fn_00400000"),
        "{shown}"
    );
    let max = shown
        .lines()
        .find_map(|l| l.trim().strip_prefix("Maximum function count:"))
        .and_then(|v| v.trim().parse::<u64>().ok());
    assert!(
        max.is_some_and(|m| m >= 200),
        "the handler ran 200 times: {shown}"
    );

    // Optimised with the profile: keyed apart, not instrumented, same answers.
    set("H2_PGO", format!("use:{}", profdata.display()));
    let (used, use_keys) = build(&header, "use");
    assert_ne!(use_keys, plain_keys);
    assert_ne!(use_keys, gen_keys);
    assert!(
        used.write_profile().is_none(),
        "a use build is not instrumented"
    );
    assert_eq!(run_sum(&used, 100), 5050);
    // A new profile at the same path is a new key.
    let o = Command::new(&tool)
        .arg("merge")
        .arg("-o")
        .arg(&profdata)
        .arg(&raw)
        .arg(&raw)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let (_, again_keys) = build(&header, "use-again");
    assert_ne!(again_keys, use_keys, "profile contents are in the key");

    // -O3 and the target CPU.
    // SAFETY: as in `set`.
    unsafe { std::env::remove_var("H2_PGO") };
    set("H2_OPT", "3");
    let (o3, o3_keys) = build(&header, "o3");
    assert_ne!(o3_keys, plain_keys);
    assert_eq!(run_sum(&o3, 100), 5050);
    if Arch::host() == Arch::Arm64 {
        set("H2_CPU", "apple-m1");
        let (m1, m1_keys) = build(&header, "o3-m1");
        assert_ne!(m1_keys, o3_keys);
        assert_eq!(run_sum(&m1, 100), 5050);
        assert!(
            compile::cflags()
                .unwrap()
                .contains(&"-mcpu=apple-m1".to_owned())
        );
        set("H2_CPU", "not-a-cpu");
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("pgo")
            .join("bad-cpu");
        let src = dir.join("src");
        emit::write_module(&sum(), &src).unwrap();
        compile::install_header(&src, &header).unwrap();
        assert!(
            compile::build(&src, &dir, Arch::host(), 2).is_err(),
            "clang rejects an unknown -mcpu"
        );
    }
}
