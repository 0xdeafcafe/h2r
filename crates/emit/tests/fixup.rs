//! Contract change slice F1, the fixup spike
//! (docs/runtime/contract-thin-transfers-flat-ram.md, section 6): an inline
//! access is one host instruction, and a host fault on it resumes at the
//! site's slow label through `hostfault`'s fixup table. `tests/c/fixup.c`
//! has one `asm goto` load and store per width. Synthetic memory only.
//!
//! Needs `clang`. `fixture_builds_for_every_target` prints which row
//! encoding each target took (run with `--nocapture`).
use emit::compile::Arch;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Row encodings, the fixture's `H2_FIX_ABS` values.
const ENCODINGS: [(u32, &str); 2] = [(0, "relative32"), (1, "absolute64")];

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/c/fixup.c")
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn clang(args: &[&str], extra: &[&Path]) -> Result<(), String> {
    let out = Command::new("clang")
        .args([
            "-std=c11",
            "-O2",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-ffreestanding",
        ])
        .args(args)
        .args(extra)
        .output()
        .map_err(|e| format!("clang: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).into_owned())
    }
}

/// Every target emit compiles for: the fixture compiles to an object with
/// at least one row encoding. Self-relative rows are the design; a target
/// that needs the absolute fallback is named in the output, and the spec
/// records it.
#[test]
fn fixture_builds_for_every_target() {
    let dir = scratch("fixup-targets");
    let arches = [
        Arch::Arm64,
        Arch::X86_64,
        Arch::Arm64Tvos,
        Arch::Arm64Ios,
        Arch::X86_64Windows,
        Arch::X86_64Linux,
    ];
    let mut failures = Vec::new();
    for arch in arches {
        let mut took = None;
        let mut errors = Vec::new();
        for (abs, name) in ENCODINGS {
            let obj = dir.join(format!("fixup-{}-{name}.o", arch.name()));
            let define = format!("-DH2_FIX_ABS={abs}");
            match clang(
                &["-c", "-target", arch.triple(), &define, "-o"],
                &[&obj, &fixture()],
            ) {
                Ok(()) => {
                    took.get_or_insert(name);
                }
                Err(e) => errors.push(format!("{name}: {e}")),
            }
        }
        match took {
            Some(name) => println!("fixup rows on {}: {name}", arch.name()),
            None => failures.push(format!("{}:\n{}", arch.name(), errors.join("\n"))),
        }
        if took == Some("absolute64") {
            println!("  relative32 failed: {}", errors.join("\n"));
        }
    }
    assert!(
        failures.is_empty(),
        "no encoding builds:\n{}",
        failures.join("\n\n")
    );
}

#[cfg(unix)]
mod run {
    use super::*;
    use libloading::Library;
    use std::ptr;

    type Ld = unsafe extern "C" fn(*const u8, u32, *mut u32) -> u64;
    type St = unsafe extern "C" fn(*mut u8, u32, u64, *mut u32);
    type LdWide = unsafe extern "C" fn(*const u8, u32, *mut u8, *mut u32);
    type StWide = unsafe extern "C" fn(*mut u8, u32, *const u8, *mut u32);
    type Bounds = unsafe extern "C" fn(*mut *const u8, *mut *const u8);
    type FormatFn = unsafe extern "C" fn() -> u32;

    /// What the landing returns for a load (fixup.c `H2FIX_SLOW`).
    const SLOW: u64 = 0x5a5a_5a5a_5a5a_5a5a;
    /// Each half of the test window: covers a 16 KiB host page and more.
    const HALF: usize = 1 << 16;

    /// A host-built shared object of the fixture, or the clang error.
    fn shared(abs: u32, name: &str) -> Result<PathBuf, String> {
        let ext = if cfg!(target_vendor = "apple") {
            "dylib"
        } else {
            "so"
        };
        let lib = scratch("fixup-host").join(format!("libfixup-{name}.{ext}"));
        let define = format!("-DH2_FIX_ABS={abs}");
        clang(&["-shared", "-fPIC", &define, "-o"], &[&lib, &fixture()])?;
        Ok(lib)
    }

    /// `[base, base + HALF)` readable and writable, `[base + HALF, base +
    /// 2 * HALF)` a guard. Never unmapped: a region may outlive the test.
    fn window() -> *mut u8 {
        // SAFETY: a fresh anonymous mapping, then a protection change on its
        // second half.
        unsafe {
            let p = libc::mmap(
                ptr::null_mut(),
                2 * HALF,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            );
            assert_ne!(p, libc::MAP_FAILED);
            assert_eq!(
                libc::mprotect(p.cast::<u8>().add(HALF).cast(), HALF, libc::PROT_NONE),
                0
            );
            p.cast()
        }
    }

    fn pattern(base: *mut u8, addr: u32, len: usize, seed: u8) -> Vec<u8> {
        let bytes: Vec<u8> = (0..len)
            .map(|i| seed.wrapping_mul(31).wrapping_add(i as u8 * 7 + 1))
            .collect();
        // SAFETY: inside the window's writable half.
        unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), base.add(addr as usize), len) };
        bytes
    }

    fn read(base: *mut u8, addr: u32, len: usize) -> Vec<u8> {
        // SAFETY: inside the window's writable half.
        unsafe { std::slice::from_raw_parts(base.add(addr as usize), len).to_vec() }
    }

    /// One encoding's library: each width's load and store runs inline on
    /// a mapped address, and lands on the slow label on the guard and on a
    /// span that runs into it.
    fn check(lib: &Library, base: *mut u8, name: &str) {
        // SAFETY: the symbols have these signatures in fixup.c.
        unsafe {
            let format: libloading::Symbol<FormatFn> = lib.get(b"h2fix_format\0").unwrap();
            let bounds: libloading::Symbol<Bounds> = lib.get(b"h2fix_bounds\0").unwrap();
            let (mut start, mut end) = (ptr::null(), ptr::null());
            bounds(&mut start, &mut end);
            let format = hostfault::Format::from_raw(format()).unwrap();
            let table = hostfault::register_table(start, end, format)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            // Ten one-instruction accesses and two two-instruction ones;
            // more only if the compiler duplicated an asm statement.
            let rows = (end as usize - start as usize) / format.row_size();
            assert!(rows >= 14, "{name}: {rows} rows");

            let guard = HALF as u32 + 0x40;
            for (width, ld, st) in [
                (1usize, "h2fix_ld1", "h2fix_st1"),
                (2, "h2fix_ld2", "h2fix_st2"),
                (4, "h2fix_ld4", "h2fix_st4"),
                (8, "h2fix_ld8", "h2fix_st8"),
            ] {
                let ld: libloading::Symbol<Ld> = lib.get(format!("{ld}\0").as_bytes()).unwrap();
                let st: libloading::Symbol<St> = lib.get(format!("{st}\0").as_bytes()).unwrap();
                let at = 0x100 + width as u32 * 0x20 + 1; // unaligned too
                let bytes = pattern(base, at, width, width as u8);
                let mut want = [0u8; 8];
                want[..width].copy_from_slice(&bytes);
                let mut slow = 0;
                assert_eq!(
                    ld(base, at, &mut slow),
                    u64::from_le_bytes(want),
                    "{name}: load {width}"
                );
                assert_eq!(slow, 0, "{name}: load {width} inline");
                let value = 0x8877_6655_4433_2211u64;
                st(base, at, value, &mut slow);
                assert_eq!(slow, 0, "{name}: store {width} inline");
                assert_eq!(
                    read(base, at, width),
                    value.to_le_bytes()[..width],
                    "{name}: store {width}"
                );

                let mut slow = 0;
                assert_eq!(
                    ld(base, guard, &mut slow),
                    SLOW,
                    "{name}: load {width} on the guard"
                );
                assert_eq!(
                    slow, 1,
                    "fixup_lands_on_the_slow_label: {name} load {width}"
                );
                let mut slow = 0;
                st(base, guard, value, &mut slow);
                assert_eq!(
                    slow, 1,
                    "fixup_lands_on_the_slow_label: {name} store {width}"
                );
                if width > 1 {
                    // A span that starts mapped and ends in the guard.
                    let across = HALF as u32 - 1;
                    let mut slow = 0;
                    ld(base, across, &mut slow);
                    assert_eq!(slow, 1, "{name}: load {width} across into the guard");
                    let mut slow = 0;
                    st(base, across, value, &mut slow);
                    assert_eq!(slow, 1, "{name}: store {width} across into the guard");
                }
            }
            for (width, ld, st) in [
                (10usize, "h2fix_ld10", "h2fix_st10"),
                (16, "h2fix_ld16", "h2fix_st16"),
            ] {
                let ld: libloading::Symbol<LdWide> = lib.get(format!("{ld}\0").as_bytes()).unwrap();
                let st: libloading::Symbol<StWide> = lib.get(format!("{st}\0").as_bytes()).unwrap();
                let at = 0x400 + width as u32 * 0x40 + 3;
                let bytes = pattern(base, at, width, width as u8);
                let mut out = [0u8; 16];
                let mut slow = 0;
                ld(base, at, out.as_mut_ptr(), &mut slow);
                assert_eq!(slow, 0, "{name}: load {width} inline");
                assert_eq!(out[..width], bytes[..], "{name}: load {width}");
                let value: Vec<u8> = (0..16u8).map(|i| 0xa0 ^ i).collect();
                st(base, at, value.as_ptr(), &mut slow);
                assert_eq!(slow, 0, "{name}: store {width} inline");
                assert_eq!(
                    read(base, at, width),
                    value[..width],
                    "{name}: store {width}"
                );

                // Wholly in the guard, and across its start (the 10-byte
                // form's second instruction is the one that faults there).
                for at in [guard, HALF as u32 - 4] {
                    let mut slow = 0;
                    ld(base, at, out.as_mut_ptr(), &mut slow);
                    assert_eq!(
                        slow, 1,
                        "fixup_lands_on_the_slow_label: {name} load {width} at {at:#x}"
                    );
                    let mut slow = 0;
                    st(base, at, value.as_ptr(), &mut slow);
                    assert_eq!(
                        slow, 1,
                        "fixup_lands_on_the_slow_label: {name} store {width} at {at:#x}"
                    );
                }
            }
            drop(table);
        }
    }

    /// Per width, both row encodings where the host can build them: a fault
    /// on the access instruction resumes at the slow label, with no signal
    /// reaching any other handler.
    #[test]
    fn fixup_lands_on_the_slow_label() {
        let base = window();
        let region = hostfault::register_region(base as usize, 2 * HALF, hostfault::fixup, 0)
            .expect("hostfault region");
        let mut ran = Vec::new();
        for (abs, name) in ENCODINGS {
            let path = match shared(abs, name) {
                Ok(path) => path,
                // fixture_builds_for_every_target reports a missing encoding.
                Err(e) => {
                    println!("{name} does not build on the host: {e}");
                    continue;
                }
            };
            // SAFETY: the fixture has no initialisers and stays loaded until
            // its table is dropped in `check`.
            let lib = unsafe { Library::new(&path) }.unwrap();
            check(&lib, base, name);
            ran.push(name);
        }
        drop(region);
        assert!(!ran.is_empty(), "no encoding built for the host");
        println!("fixup ran with: {}", ran.join(", "));
    }
}
