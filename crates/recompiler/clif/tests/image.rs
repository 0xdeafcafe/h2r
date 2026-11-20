//! Time the fast tier over the user's retail XBE: analyse (or reuse boot's
//! cached analysis), lift, plan, compile every owner with Cranelift and
//! link the module, then delete it. Ignored by default; needs `H2_XBE`
//! (or `disc/default.xbe`). Nothing derived from the disc is kept.
//!
//! ```sh
//! H2_XBE=/path/to/default.xbe cargo test --release -p clif --test image -- --ignored --nocapture
//! ```
//!
//! `H2_CLIF_JOBS` sets the compile threads (default: every core).

#[path = "../../emit/tests/common/mod.rs"]
mod common;

use std::time::Instant;

/// The newest analysis in boot's cache (`H2_BOOT_CACHE`, default
/// `$TMPDIR/h2boot-cache`) that decodes, if any.
fn cached_analysis() -> Option<analyze::Analysis> {
    let dir = std::env::var_os("H2_BOOT_CACHE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("h2boot-cache"));
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("analysis-"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort();
    files.into_iter().rev().find_map(|(_, p)| analyze::Analysis::decode(&std::fs::read(p).ok()?))
}

/// User plus system CPU time of this process so far.
fn cpu_secs() -> f64 {
    // SAFETY: getrusage fills the struct it is given.
    let mut u: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) };
    let s = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
    s(u.ru_utime) + s(u.ru_stime)
}

#[test]
#[ignore = "times the whole retail image; needs H2_XBE"]
fn fast_tier_compiles_the_whole_image() {
    let Some(xbe_path) = common::xbe_path() else {
        common::skip("fast_tier_compiles_the_whole_image", "needs H2_XBE");
        return;
    };
    let Some(header) = common::abi_header() else {
        common::skip("fast_tier_compiles_the_whole_image", "needs h2abi.h");
        return;
    };
    let bytes = std::fs::read(&xbe_path).unwrap();
    let x = xbe::Xbe::parse(&bytes).unwrap();
    let t = Instant::now();
    let a = match cached_analysis() {
        Some(a) => a,
        None => analyze::analyze(&x).unwrap(),
    };
    eprintln!("analyze: {:.1}s, {} functions", t.elapsed().as_secs_f64(), a.functions.len());
    let t = Instant::now();
    let lifter = lift::Lifter::new(&a, &x).unwrap();
    let entries: Vec<u32> = a.functions.keys().copied().collect();
    let funcs: Vec<_> = emit::par::map(&entries, |&e| lifter.lift(e)).into_iter().flatten().collect();
    eprintln!("lift: {:.1}s, {} functions", t.elapsed().as_secs_f64(), funcs.len());
    let t = Instant::now();
    let owners = emit::plan(funcs);
    eprintln!("plan: {:.1}s, {} owners", t.elapsed().as_secs_f64(), owners.len());

    let jobs = std::env::var("H2_CLIF_JOBS")
        .ok()
        .and_then(|j| j.parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4));
    let dir = std::env::temp_dir().join(format!("h2clif-image-{}", std::process::id()));
    let dylib = dir.join("libh2game.dylib");
    let cpu0 = cpu_secs();
    let t = Instant::now();
    let r = clif::build_dylib(&owners, &dir, &header, &[("bridge.c", emit::host::BRIDGE)], &dylib, jobs);
    let total = t.elapsed().as_secs_f64();
    // Wall time depends on the machine's load; this process's CPU time
    // (Cranelift on every thread, not clang's children) does not.
    let cpu = cpu_secs() - cpu0;
    let size = std::fs::metadata(&dylib).map(|m| m.len()).unwrap_or(0);
    let _ = std::fs::remove_dir_all(&dir);
    let r = r.unwrap_or_else(|e| panic!("{e}"));
    eprintln!(
        "clif: {} owners in {} shards on {jobs} threads: compile {:.1}s, write {:.1}s, C parts and link {:.1}s, total {total:.1}s ({cpu:.1}s of CPU in this process); {:.1} MB of objects, dylib {:.1} MB",
        r.owners,
        r.shards,
        r.compile_secs,
        r.write_secs,
        r.link_secs,
        r.object_bytes as f64 / 1e6,
        size as f64 / 1e6
    );
}
