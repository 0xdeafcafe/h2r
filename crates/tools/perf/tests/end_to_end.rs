//! The scenarios run for real, and the profiler attributes generated code
//! to the guest functions and instructions it came from.

use std::path::{Path, PathBuf};

use perf::bench::{BenchCtx, Prepared, Scenario};
use perf::hotloop::{self, HotLoop};
// The profiling tests that use these run on macOS only.
#[cfg(target_os = "macos")]
use perf::profile::{self, Render};
#[cfg(target_os = "macos")]
use perf::sample::{Sampler, ThreadTarget};
#[cfg(target_os = "macos")]
use perf::symbolize::{Category, Frame, Symbolizer};

fn work() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join("perf-e2e")
}

fn ctx() -> BenchCtx {
    BenchCtx {
        work: work(),
        quick: true,
        frames: 1,
    }
}

fn have_clang() -> bool {
    let ok = emit::compile::compiler().is_ok();
    if !ok {
        eprintln!("SKIPPED: no clang");
    }
    ok
}

#[test]
fn hot_loops_run_through_generated_code() {
    if !have_clang() {
        return;
    }
    let m = hotloop::shared(&work()).unwrap();
    let cpu = m.call(hotloop::ALU, 10).unwrap();
    assert_eq!(cpu.ecx, 0);
    m.call(hotloop::MEMORY, 7).unwrap();
    let counter = m.read_u32(hotloop::DATA).unwrap();
    assert!(counter >= 7, "{counter}");
    // LEAF adds one to EAX per call.
    let cpu = m.call(hotloop::CALL, 5).unwrap();
    assert_eq!(cpu.ecx, 0);
    let cpu = m.call(hotloop::X87, 3).unwrap();
    assert_eq!(cpu.x87.tag, 0xffff);
    assert_eq!(
        m.read_u32(hotloop::DATA + 32).unwrap(),
        (((1.25f64 * 2.5 + 0.75).sqrt() / 3.0) as f32).to_bits()
    );
    for mut s in HotLoop::all() {
        assert!(matches!(s.prepare(&ctx()).unwrap(), Prepared::Ready));
        let v = s.measure().unwrap();
        #[cfg(unix)]
        assert!(v.extra["cpu_ns_per_trip"] > 0.0);
        assert!(
            v.value > 0.0 && v.value.is_finite(),
            "{}: {}",
            s.name(),
            v.value
        );
    }
}

/// Profile the ALU loop: most on-CPU samples land in generated code, are
/// charged to the loop's guest function, and name instructions inside it.
#[cfg(target_os = "macos")]
#[test]
fn profiles_attribute_samples_to_guest_functions_and_instructions() {
    if !have_clang() {
        return;
    }
    let m = hotloop::shared(&work()).unwrap();
    let sampler = Sampler::start(vec![ThreadTarget::current("game")], 2000).unwrap();
    let t = std::time::Instant::now();
    while t.elapsed() < std::time::Duration::from_millis(400) {
        m.call(hotloop::ALU, 200_000).unwrap();
    }
    let raw = sampler.finish();
    let p = profile::resolve(raw, &mut Symbolizer::new());
    let r = Render {
        on_cpu_only: true,
        ..Render::default()
    };
    let s = p.summary(&r);
    // A loaded host can starve the sampler; only a handful are needed.
    assert!(s.samples >= 10, "{} samples", s.samples);
    let guest = s
        .categories
        .iter()
        .find(|c| c.category == Some(Category::Guest))
        .map_or(0, |c| c.samples);
    assert!(
        guest * 2 > s.samples,
        "guest {guest} of {}:\n{}",
        s.samples,
        p.table(&r)
    );
    assert_eq!(
        s.guest_self[0].name,
        format!("guest {:#010x}", hotloop::ALU),
        "{}",
        p.table(&r)
    );
    // Line tables (emit's default H2_DEBUG=lines) give the instruction.
    let insns: Vec<u32> = p
        .stacks
        .iter()
        .filter_map(|st| match st.frames.first() {
            Some(Frame::Guest {
                function,
                insn: Some(i),
            }) if *function == hotloop::ALU => Some(*i),
            _ => None,
        })
        .collect();
    assert!(
        !insns.is_empty(),
        "no instruction-level samples:\n{}",
        p.table(&r)
    );
    assert!(
        insns
            .iter()
            .all(|&i| (hotloop::ALU..hotloop::ALU + 0x40).contains(&i)),
        "{insns:x?}"
    );
    let folded = p.folded(&r);
    assert!(
        folded.contains(&format!("guest {:#010x};@0x", hotloop::ALU)),
        "{folded}"
    );
}

/// A module loaded after a symbolizer has already resolved something is
/// still seen (by that symbolizer and a new one), and a module can be
/// dropped and rebuilt in the same process.
#[cfg(target_os = "macos")]
#[test]
fn modules_loaded_later_resolve_and_rebuild_safely() {
    if !have_clang() {
        return;
    }
    let mut early = Symbolizer::new();
    #[inline(never)]
    fn host_marker() -> u64 {
        std::hint::black_box(3)
    }
    assert!(matches!(
        early.resolve(host_marker as *const () as u64),
        Frame::Host { .. }
    ));
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("perf-e2e-late");
    let first = hotloop::build(&dir).unwrap();
    first.call(hotloop::ALU, 3).unwrap();
    drop(first);
    let m = hotloop::build(&dir).unwrap();
    m.call(hotloop::ALU, 3).unwrap();
    let pc = m.handler_address(hotloop::ALU).expect("h2fn symbol");
    for s in [&mut early, &mut Symbolizer::new()] {
        match s.resolve(pc) {
            Frame::Guest { function, .. } => assert_eq!(function, hotloop::ALU),
            f => panic!("{f:?}"),
        }
    }
}

/// Objects from a shared cache still give instructions after the work
/// directory that first compiled them is gone: line tables name sources
/// relative to their directory, and the symbolizer reads this build's.
#[cfg(target_os = "macos")]
#[test]
fn instructions_resolve_from_objects_another_output_compiled() {
    if !have_clang() {
        return;
    }
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("perf-e2e-shared-cache");
    let _ = std::fs::remove_dir_all(&root);
    let cache = root.join("cache");
    let first = root.join("first");
    drop(hotloop::build_in(&first, &cache).unwrap());
    std::fs::remove_dir_all(&first).unwrap();
    let m = hotloop::build_in(&root.join("second"), &cache).unwrap();
    m.call(hotloop::ALU, 3).unwrap();
    let base = m.handler_address(hotloop::ALU).unwrap();
    let mut s = Symbolizer::new();
    // The prologue (register homes, the entry switch) has no instruction of its
    // own, and it has grown past the first 256 bytes: look through the body.
    let insns: Vec<u32> = (0..2048)
        .step_by(4)
        .filter_map(|off| match s.resolve(base + off) {
            Frame::Guest {
                function,
                insn: Some(i),
            } if function == hotloop::ALU => Some(i),
            _ => None,
        })
        .collect();
    assert!(!insns.is_empty(), "no instructions in {:#x}", hotloop::ALU);
    assert!(
        insns
            .iter()
            .all(|&i| (hotloop::ALU..hotloop::ALU + 0x40).contains(&i)),
        "{insns:x?}"
    );
    drop(m);
    let _ = std::fs::remove_dir_all(&root);
}

/// The first bytes of a handler (prologue, selector switch) never take the
/// previous function's last instruction from the line tables.
#[cfg(target_os = "macos")]
#[test]
fn instructions_stay_inside_their_generated_function() {
    if !have_clang() {
        return;
    }
    let m = hotloop::shared(&work()).unwrap();
    let mut s = Symbolizer::new();
    for (entry, len) in [
        (hotloop::LEAF, 0x10),
        (hotloop::CALL, 0x20),
        (hotloop::MEMORY, 0x20),
    ] {
        let base = m.handler_address(entry).unwrap();
        for off in (0..64).step_by(4) {
            match s.resolve(base + off) {
                Frame::Guest { function, insn } => {
                    assert_eq!(function, entry);
                    if let Some(i) = insn {
                        assert!(
                            (entry..entry + len).contains(&i),
                            "{entry:#x}+{off}: insn {i:#x}"
                        );
                    }
                }
                f => panic!("{entry:#x}+{off}: {f:?}"),
            }
        }
    }
}
