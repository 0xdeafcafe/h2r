//! The two module tiers (docs/lifter/cranelift-tier.md): clang's optimised
//! build in `<out>/libh2game.dylib` ([`crate::build`]), and the Cranelift
//! fast tier in `<out>/fast/libh2game.dylib` ([`build_fast`]), built in
//! seconds so a first start doesn't wait for clang. Each carries its own
//! stamp; a start uses clang's module when it is current, else the fast
//! one, and the caller starts clang's build in the background for the next
//! start ([`background_build`]). Nothing swaps while the game runs.

use std::path::{Path, PathBuf};
#[cfg(feature = "desktop-runtime")]
use std::time::Instant;

#[cfg(feature = "desktop-runtime")]
use anyhow::Context;
use anyhow::{Result, bail};

/// Which module a start runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// clang's optimised build.
    Clang,
    /// The Cranelift fast tier.
    Fast,
    /// Clang with coverage-selected owners, for light host runs.
    Host,
}

impl Tier {
    pub fn name(self) -> &'static str {
        match self {
            Tier::Clang => "clang",
            Tier::Fast => "fast",
            Tier::Host => "slim host",
        }
    }
}

/// What `--tier` (or `H2_TIER`) asks a start for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Choice {
    /// clang's module when current, else the fast tier (built if need be)
    /// with clang's build started in the background.
    #[default]
    Auto,
    /// The fast tier only: built if need be, no clang build.
    Fast,
    /// clang's module only, built in the foreground if need be (the old
    /// behaviour).
    Clang,
}

impl Choice {
    pub fn parse(s: &str) -> Result<Choice> {
        Ok(match s {
            "auto" => Choice::Auto,
            "fast" => Choice::Fast,
            "clang" => Choice::Clang,
            _ => bail!("tier {s:?}: expected auto, fast or clang"),
        })
    }

    /// `H2_TIER`, else [`Choice::Auto`].
    pub fn from_env() -> Result<Choice> {
        match std::env::var("H2_TIER") {
            Ok(s) if !s.is_empty() => Choice::parse(&s),
            _ => Ok(Choice::Auto),
        }
    }
}

/// The fast tier's module under `out`.
pub fn fast_dylib(out: &Path) -> PathBuf {
    out.join("fast").join(crate::DYLIB)
}

/// clang's module under `out`.
pub fn clang_dylib(out: &Path) -> PathBuf {
    out.join(crate::DYLIB)
}

/// The fast tier's stamp: the image, everything that turns it into the
/// module (the generator's sources plus crates/clif and Cargo.lock, for the
/// Cranelift version), the IR passes, Cranelift's optimisation level and
/// the module ABI. It never equals a clang stamp (it starts `tier fast`),
/// so neither tier's module passes for the other.
pub fn fast_stamp(xbe_bytes: &[u8]) -> String {
    fast_stamp_with(
        xbe_bytes,
        &emit::opt::Passes::from_env().map_or_else(|e| e, |p| p.spec()),
        &emit::compile::clif_opt_level().map_or_else(|e| e, str::to_owned),
    )
}

fn fast_stamp_with(xbe_bytes: &[u8], passes: &str, opt: &str) -> String {
    format!(
        "tier fast xbe {:016x} generator {} passes {passes} cranelift opt {opt} abi {}\n",
        crate::fallback::fnv64(xbe_bytes),
        env!("FAST_TIER_SOURCE_HASH"),
        runtime::cpu::ABI_VERSION
    )
}

/// Whether `dylib` exists and its stamp is the fast tier's for this image.
pub fn fast_current(xbe_bytes: &[u8], dylib: &Path) -> bool {
    crate::host_audit::module_from_env().is_none()
        && dylib.exists()
        && std::fs::read_to_string(crate::stamp_path(dylib)).is_ok_and(|s| s == fast_stamp(xbe_bytes))
}

/// Whether `dylib` is a current module of either tier, and which.
pub fn module_tier(xbe_bytes: &[u8], dylib: &Path) -> Option<Tier> {
    if crate::dylib_current(xbe_bytes, dylib) {
        Some(Tier::Clang)
    } else if fast_current(xbe_bytes, dylib) {
        Some(Tier::Fast)
    } else {
        None
    }
}

/// The current module under `out` a start should run, clang's first.
pub fn current(xbe_bytes: &[u8], out: &Path) -> Option<(PathBuf, Tier)> {
    let clang = clang_dylib(out);
    if crate::dylib_current(xbe_bytes, &clang) {
        return Some((clang, Tier::Clang));
    }
    let fast = fast_dylib(out);
    fast_current(xbe_bytes, &fast).then_some((fast, Tier::Fast))
}

/// What a start does about modules under `out`, given `choice`: the module
/// to run (once built), whether to build it now with the fast tier or
/// clang, and whether to start clang's build in the background.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub dylib: PathBuf,
    pub tier: Tier,
    /// Build `dylib` (in `tier`) before running.
    pub build: bool,
    /// Start clang's build in the background for the next start.
    pub background_clang: bool,
}

/// [`Plan`] from which modules under `out` are current.
pub fn plan(xbe_bytes: &[u8], out: &Path, choice: Choice) -> Plan {
    plan_from(current(xbe_bytes, out).map(|(_, t)| t), fast_current(xbe_bytes, &fast_dylib(out)), out, choice)
}

/// Select a prebuilt module for a light host or sim. Normal desktop
/// runs never treat a coverage-pruned image as their current module.
pub fn plan_light(xbe_bytes: &[u8], out: &Path, choice: Choice) -> Plan {
    let dylib = clang_dylib(out);
    if choice != Choice::Fast && crate::current_slim_stamp(xbe_bytes, &dylib).is_some() {
        return Plan { dylib, tier: Tier::Host, build: false, background_clang: false };
    }
    plan(xbe_bytes, out, choice)
}

fn plan_from(best: Option<Tier>, fast_ok: bool, out: &Path, choice: Choice) -> Plan {
    let fast = |build, background_clang| Plan { dylib: fast_dylib(out), tier: Tier::Fast, build, background_clang };
    let clang = |build| Plan { dylib: clang_dylib(out), tier: Tier::Clang, build, background_clang: false };
    match choice {
        Choice::Clang => clang(best != Some(Tier::Clang)),
        Choice::Fast => fast(!fast_ok, false),
        Choice::Auto => match best {
            Some(Tier::Clang) => clang(false),
            Some(Tier::Fast) => fast(false, true),
            Some(Tier::Host) => unreachable!("light host planning handles slim modules"),
            None => fast(true, true),
        },
    }
}

/// Make ready the module a start of `disc` runs, by [`plan`]: build it if
/// need be, and start clang's build in the background when the plan says
/// so and `background` allows (a failure to start it only warns). Returns
/// the module to run.
#[cfg(feature = "desktop-runtime")]
pub fn prepare(xbe_bytes: &[u8], disc: &Path, out: &Path, choice: Choice, jobs: usize, background: bool) -> Result<Plan> {
    let p = plan(xbe_bytes, out, choice);
    if p.build {
        if p.dylib.exists() {
            eprintln!("tier: {} was built from another image or generator; rebuilding", p.dylib.display());
        }
        match p.tier {
            Tier::Fast => build_fast(xbe_bytes, out, jobs)?,
            Tier::Clang => crate::build(xbe_bytes, out, jobs)?,
            Tier::Host => unreachable!("slim host modules are built explicitly"),
        };
    }
    if p.background_clang
        && background
        && let Err(e) = background_build(disc, out)
    {
        // The fast tier still runs; the next start tries again.
        eprintln!("warning: clang's background build didn't start: {e:#}");
    }
    Ok(p)
}

/// Lift the image and compile it with the Cranelift fast tier into
/// `out/fast/libh2game.dylib`, stamped with [`fast_stamp`].
#[cfg(feature = "desktop-runtime")]
pub fn build_fast(xbe_bytes: &[u8], out: &Path, jobs: usize) -> Result<PathBuf> {
    build_fast_for(xbe_bytes, out, jobs, emit::compile::Arch::target()?)
}

/// The Cranelift ISA is the running host's. Check this for both builds and
/// stamp-only checks so cross builds cannot accept a host fast-tier module.
#[cfg(feature = "desktop-runtime")]
pub fn check_fast_target(arch: emit::compile::Arch) -> Result<()> {
    anyhow::ensure!(
        crate::host_audit::module_from_env().is_none(),
        "H2_HOST_MODULE requires the clang tier"
    );
    if arch != emit::compile::Arch::host() {
        bail!("the fast tier builds for this host only; use --tier clang for {}", arch.name());
    }
    Ok(())
}

#[cfg(feature = "desktop-runtime")]
pub fn build_fast_for(xbe_bytes: &[u8], out: &Path, jobs: usize, arch: emit::compile::Arch) -> Result<PathBuf> {
    check_fast_target(arch)?;
    let total = Instant::now();
    let x = xbe::Xbe::parse(xbe_bytes)?;
    let lib = fast_dylib(out);
    let dir = lib.parent().expect("fast_dylib has a parent").to_path_buf();
    let _ = std::fs::remove_file(crate::stamp_path(&lib));
    let t = Instant::now();
    let (a, cached) = crate::analyze_cached(&x, crate::analysis_cache(xbe_bytes).as_deref())?;
    eprintln!(
        "analyze: {:.1}s, {} functions{}",
        t.elapsed().as_secs_f64(),
        a.functions.len(),
        if cached { " (cached)" } else { "" }
    );
    let t = Instant::now();
    let lifter = lift::Lifter::new(&a, &x)?;
    let entries: Vec<u32> = a.functions.keys().copied().collect();
    let funcs: Vec<_> = emit::par::map(&entries, |&e| lifter.lift(e)).into_iter().flatten().collect();
    eprintln!("lift: {:.1}s, {} of {} lifted", t.elapsed().as_secs_f64(), funcs.len(), entries.len());
    let t = Instant::now();
    let owners = emit::plan(funcs);
    eprintln!("plan: {:.1}s", t.elapsed().as_secs_f64());
    let r = clif::build_dylib(
        &owners,
        &dir,
        Path::new(crate::H2ABI_H),
        &[("h2boot_bridge.c", crate::BRIDGE), ("h2boot_bridge_x87.c", crate::BRIDGE_X87)],
        &lib,
        jobs,
    )
    .map_err(anyhow::Error::msg)?;
    eprintln!(
        "cranelift: {} owners in {} shards, {:.1}s compiling, {:.1}s writing, {:.1}s C parts and link, {:.1} MB of objects -> {}",
        r.owners,
        r.shards,
        r.compile_secs,
        r.write_secs,
        r.link_secs,
        r.object_bytes as f64 / 1e6,
        lib.display()
    );
    // The objects are only the link's input; the module stands alone.
    let _ = std::fs::remove_dir_all(dir.join("obj"));
    std::fs::write(crate::stamp_path(&lib), fast_stamp(xbe_bytes)).with_context(|| format!("stamping {}", lib.display()))?;
    eprintln!("build (fast tier): {:.1}s", total.elapsed().as_secs_f64());
    Ok(lib)
}

/// The log [`background_build`]'s build writes under `out`.
#[cfg(feature = "desktop-runtime")]
pub fn background_log(out: &Path) -> PathBuf {
    out.join("clang-build.log")
}

/// The command for clang's build of `disc` into `out` in the background:
/// this executable's `build <disc> --out <out> --background`, logging to
/// [`background_log`]. `build --background` lowers its own priority and
/// leaves at once when another background build holds `out`'s lock.
#[cfg(feature = "desktop-runtime")]
pub fn background_command(exe: &Path, disc: &Path, out: &Path) -> std::process::Command {
    let mut c = std::process::Command::new(exe);
    // Named, so an H2_TIER=fast in the environment doesn't make it fast.
    c.arg("build").arg(disc).arg("--out").arg(out).args(["--tier", "clang", "--background"]);
    c.stdin(std::process::Stdio::null());
    c
}

/// Start clang's build for the next start; it outlives this process. The
/// log says where its output goes.
#[cfg(feature = "desktop-runtime")]
pub fn background_build(disc: &Path, out: &Path) -> Result<()> {
    let exe = std::env::current_exe().context("finding this executable")?;
    std::fs::create_dir_all(out)?;
    let log_path = background_log(out);
    let log = std::fs::File::create(&log_path).with_context(|| format!("creating {}", log_path.display()))?;
    let mut c = background_command(&exe, disc, out);
    c.stdout(log.try_clone()?).stderr(log);
    let child = c.spawn().with_context(|| format!("starting {}", exe.display()))?;
    eprintln!(
        "tier: clang's build started in the background (pid {}, low priority, log {}); the next start uses it",
        child.id(),
        log_path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_start_prefers_clang_then_fast_then_builds_fast() {
        let out = Path::new("/out");
        let p = plan_from(Some(Tier::Clang), true, out, Choice::Auto);
        assert_eq!(p, Plan { dylib: out.join(crate::DYLIB), tier: Tier::Clang, build: false, background_clang: false });
        let p = plan_from(Some(Tier::Fast), true, out, Choice::Auto);
        assert_eq!(p, Plan { dylib: fast_dylib(out), tier: Tier::Fast, build: false, background_clang: true });
        let p = plan_from(None, false, out, Choice::Auto);
        assert_eq!(p, Plan { dylib: fast_dylib(out), tier: Tier::Fast, build: true, background_clang: true });
    }

    #[test]
    fn a_forced_tier_builds_only_that_tier() {
        let out = Path::new("/out");
        assert_eq!(plan_from(Some(Tier::Fast), true, out, Choice::Clang), Plan {
            dylib: clang_dylib(out),
            tier: Tier::Clang,
            build: true,
            background_clang: false
        });
        assert!(!plan_from(Some(Tier::Clang), true, out, Choice::Clang).build);
        // The fast tier is used even when clang's is current, and never
        // starts clang.
        assert_eq!(plan_from(Some(Tier::Clang), true, out, Choice::Fast), Plan {
            dylib: fast_dylib(out),
            tier: Tier::Fast,
            build: false,
            background_clang: false
        });
        assert!(plan_from(Some(Tier::Clang), false, out, Choice::Fast).build);
    }

    #[test]
    fn tier_choices_parse() {
        assert_eq!(Choice::parse("auto").unwrap(), Choice::Auto);
        assert_eq!(Choice::parse("fast").unwrap(), Choice::Fast);
        assert_eq!(Choice::parse("clang").unwrap(), Choice::Clang);
        assert!(Choice::parse("jit").is_err());
    }

    #[test]
    fn fast_and_clang_stamps_never_vouch_for_each_other() {
        let dir = std::env::temp_dir().join(format!("h2boot-tier-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (a, b) = (b"XBEH image a".as_slice(), b"XBEH image b".as_slice());
        let fast = fast_dylib(&dir);
        let clang = clang_dylib(&dir);
        std::fs::create_dir_all(fast.parent().unwrap()).unwrap();
        std::fs::write(&fast, b"").unwrap();
        std::fs::write(&clang, b"").unwrap();
        assert_eq!(current(a, &dir), None, "no stamps");

        std::fs::write(crate::stamp_path(&fast), fast_stamp(a)).unwrap();
        assert_eq!(current(a, &dir), Some((fast.clone(), Tier::Fast)));
        assert_eq!(module_tier(a, &fast), Some(Tier::Fast));
        assert_eq!(current(b, &dir), None, "another image");
        // A fast stamp on clang's module is not clang's.
        std::fs::write(crate::stamp_path(&clang), fast_stamp(a)).unwrap();
        assert!(!crate::dylib_current(a, &clang));
        assert_eq!(current(a, &dir), Some((fast.clone(), Tier::Fast)));

        std::fs::write(crate::stamp_path(&clang), crate::build_stamp(a)).unwrap();
        assert_eq!(current(a, &dir), Some((clang.clone(), Tier::Clang)), "clang's wins");
        // A clang stamp on the fast module is not the fast tier's.
        std::fs::write(crate::stamp_path(&fast), crate::build_stamp(a)).unwrap();
        assert!(!fast_current(a, &fast));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_fast_stamp_names_passes_and_optimisation() {
        let s = fast_stamp_with(b"x", "all", "speed");
        assert!(s.starts_with("tier fast "), "{s}");
        assert_ne!(s, fast_stamp_with(b"x", "none", "speed"));
        assert_ne!(s, fast_stamp_with(b"x", "all", "none"));
        assert_ne!(s, fast_stamp_with(b"y", "all", "speed"));
    }

    #[cfg(feature = "desktop-runtime")]
    #[test]
    fn the_background_build_is_this_h2s_build_into_the_same_out() {
        let c = background_command(Path::new("/bin/h2"), Path::new("/d/halo2.iso"), Path::new("/o"));
        assert_eq!(c.get_program(), "/bin/h2");
        let args: Vec<_> = c.get_args().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(args, ["build", "/d/halo2.iso", "--out", "/o", "--tier", "clang", "--background"]);
    }
}
