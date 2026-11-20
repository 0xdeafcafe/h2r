//! Build and load: the XBE lifted, emitted and compiled into one host
//! module, its build stamp, the module loaded with the boot bridge, and
//! the interpreter for what the module leaves out (`fallback`, `x87`,
//! `sse`).
//!
//! `boot` is the composition root above this crate and re-exports it,
//! so `boot::build`, `boot::tier` and `boot::load_module` resolve as
//! before (docs/runtime/BOOT.md, docs/dev/crate-structure.md).

pub mod fallback;
pub mod host_coverage;
pub mod host_report;
pub mod shim;
pub mod sse;
pub mod tier;
pub mod x87;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
#[cfg(feature = "desktop-runtime")]
use std::time::Instant;

use anyhow::{Context, Result, bail};

/// The bridge source linked into the generated dylib.
pub const BRIDGE: &str = include_str!("../c/boot_bridge.c");
/// The `h2x87_v1_*` forwarders generated code imports since x87 lowering:
/// emit's own x87 bridge, installed with `h2x87bridge_install`.
pub const BRIDGE_X87: &str = concat!(
    "#include \"h2gen.h\"\n",
    include_str!("../../../recompiler/emit/c/bridge_x87.c")
);
/// The runtime ABI header, copied next to the generated sources.
pub const H2ABI_H: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../runtime/runtime/include/h2abi.h");
/// File name of the linked module inside the build directory.
pub const DYLIB: &str = emit::compile::Arch::host().module_file_name();

/// Read `default.xbe` from an XISO, a raw XBE, or a directory holding the
/// disc's files (the tvOS bundle, docs/runtime/tvos.md).
pub fn read_xbe(path: &Path) -> Result<Vec<u8>> {
    use std::io::Read;
    if path.is_dir() {
        let xbe = std::fs::read_dir(path)
            .with_context(|| format!("listing {}", path.display()))?
            .flatten()
            .find(|e| e.file_name().eq_ignore_ascii_case("default.xbe"))
            .with_context(|| format!("no default.xbe in {}", path.display()))?;
        return Ok(std::fs::read(xbe.path())?);
    }
    let mut f = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut magic = [0u8; 4];
    f.read_exact(&mut magic)?;
    if &magic == b"XBEH" {
        return Ok(std::fs::read(path)?);
    }
    let mut iso = xiso::Xiso::open(std::io::BufReader::new(std::fs::File::open(path)?))?;
    let entry = iso.find("default.xbe")?;
    Ok(iso.read(&entry)?)
}

/// What a module is built from: the image's hash, the generator sources'
/// hash (build.rs), the IR passes `H2_PASSES` selects, the code generation
/// choices (`H2_ACCESS_COUNT`), the module ABI and the compiler flags,
/// with the optimisation tiers when they are on (`emit::compile::Tiers`).
/// [`build`] writes it next to the dylib
/// (`<dylib>.stamp`) once linked; [`run`] refuses a dylib whose stamp
/// differs, and `h2 run` rebuilds instead.
pub fn build_stamp(xbe_bytes: &[u8]) -> String {
    build_stamp_for(xbe_bytes, stamp_target())
}

fn build_stamp_for(xbe_bytes: &[u8], arch: emit::compile::Arch) -> String {
    let mut stamp = full_build_stamp(xbe_bytes, arch);
    if let Some(path) = host_report::module_from_env() {
        let fingerprint = std::fs::read(&path)
            .map(|bytes| format!("{:016x}", fallback::fnv64(&bytes)))
            .unwrap_or_else(|_| format!("unreadable {}", path.display()));
        stamp.push_str(&format!("host module {fingerprint}\n"));
    }
    stamp
}

fn full_build_stamp(xbe_bytes: &[u8], arch: emit::compile::Arch) -> String {
    match stamp_flags(arch) {
        Ok(flags) => stamp_for(
            xbe_bytes,
            &emit::opt::Passes::from_env().map_or_else(|e| e, |p| p.spec()),
            &emit::Codegen::from_env().map_or_else(|e| e, |c| format!("{c:?}")),
            &flags,
            arch,
        ),
        Err(e) => format!("invalid compiler flags: {e}"),
    }
}

#[cfg(test)]
fn stamp_with(xbe_bytes: &[u8], passes: &str, codegen: &str, flags: &[String]) -> String {
    stamp_for(xbe_bytes, passes, codegen, flags, stamp_target())
}

/// The compiler flags a stamp for `target` records: the target's own flags
/// and default level (`emit::compile::cflags_for`), so a Windows module
/// cross-built on a Mac and the app on the console agree, and the tiers.
fn stamp_flags(target: emit::compile::Arch) -> Result<Vec<String>, emit::compile::Error> {
    let mut flags = emit::compile::cflags_for(target)?;
    if let Some(t) = emit::compile::Tiers::from_env()? {
        flags.push(format!("tiers {}", t.spec()));
    }
    Ok(flags)
}

/// The target a stamp is for: `H2_EMIT_TARGET`'s, else the host.
fn stamp_target() -> emit::compile::Arch {
    emit::compile::Arch::target().unwrap_or(emit::compile::Arch::host())
}

/// The stamp for a module compiled for `target`. A macOS stamp is the same
/// as before targets existed; a tvOS, iOS or Windows one names its triple,
/// so a module cross-built on a Mac with `H2_EMIT_TARGET=tvos` (or `ios`,
/// `windows`) matches what the app computes on the device (where that is
/// the host) and nothing else.
fn stamp_for(
    xbe_bytes: &[u8],
    passes: &str,
    codegen: &str,
    flags: &[String],
    target: emit::compile::Arch,
) -> String {
    let target = match target {
        emit::compile::Arch::Arm64Tvos
        | emit::compile::Arch::Arm64Ios
        | emit::compile::Arch::X86_64Linux
        | emit::compile::Arch::X86_64Windows => {
            format!(" target {}", target.triple())
        }
        emit::compile::Arch::Arm64 | emit::compile::Arch::X86_64 => String::new(),
    };
    format!(
        "xbe {:016x} generator {} passes {passes} codegen {codegen} abi {} flags {flags:?}{target}\n",
        fallback::fnv64(xbe_bytes),
        env!("GENERATOR_SOURCE_HASH"),
        runtime::cpu::ABI_VERSION
    )
}

pub fn stamp_path(dylib: &Path) -> PathBuf {
    let mut p = dylib.as_os_str().to_owned();
    p.push(".stamp");
    PathBuf::from(p)
}

/// Whether `dylib` exists and was built from this image by this generator.
pub fn dylib_current(xbe_bytes: &[u8], dylib: &Path) -> bool {
    dylib_current_for(xbe_bytes, dylib, stamp_target())
}

/// Check a cross-built module using the same target that produced its stamp.
pub fn dylib_current_for(xbe_bytes: &[u8], dylib: &Path, arch: emit::compile::Arch) -> bool {
    dylib.exists()
        && std::fs::read_to_string(stamp_path(dylib)).is_ok_and(|s| s == build_stamp_for(xbe_bytes, arch))
}

const SLIM_STAMP: &str = "host coverage ";

fn slim_stamp(xbe: &[u8], arch: emit::compile::Arch, coverage: &host_coverage::Coverage) -> String {
    format!("{}{SLIM_STAMP}{:016x}\n", full_build_stamp(xbe, arch), coverage.fingerprint())
}

/// Exact stamp check for a build requested with a particular coverage list.
pub fn slim_current_for(
    xbe: &[u8],
    module: &Path,
    arch: emit::compile::Arch,
    coverage: &host_coverage::Coverage,
) -> bool {
    module.is_file()
        && std::fs::read_to_string(stamp_path(module))
            .is_ok_and(|stamp| stamp == slim_stamp(xbe, arch, coverage))
}

/// A light host starts a staged slim module using its own coverage stamp.
/// The image, generator, target and settings still have to match.
pub fn current_slim_stamp(xbe: &[u8], module: &Path) -> Option<String> {
    if !module.is_file() || host_report::module_from_env().is_some() {
        return None;
    }
    let stamp = std::fs::read_to_string(stamp_path(module)).ok()?;
    let full = full_build_stamp(xbe, stamp_target());
    let fingerprint = stamp
        .strip_prefix(full.as_str())?
        .strip_prefix(SLIM_STAMP)?
        .strip_suffix('\n')?;
    (fingerprint.len() == 16 && fingerprint.bytes().all(|b| b.is_ascii_hexdigit()))
        .then_some(stamp)
}

/// Where [`build`] (and the fallback interpreter at run time) keep
/// discovery's result for `xbe_bytes`: in the boot cache directory
/// (`H2_BOOT_CACHE`, default `$TMPDIR/h2boot-cache`; `off` disables it),
/// shared by every output directory, named by the image's hash and size
/// and by everything discovery's code is made of (`ANALYSIS_SOURCE_HASH`:
/// the xbe and analyze sources, Cargo.lock and the rustc version). `None`
/// when caching is off, or `H2_TRACE` asks discovery for diagnostics.
pub fn analysis_cache(xbe_bytes: &[u8]) -> Option<PathBuf> {
    if std::env::var_os("H2_TRACE").is_some_and(|v| !v.is_empty()) {
        return None;
    }
    Some(analysis_cache_in(
        xbe_bytes,
        &fallback::boot_cache_dir()?,
        env!("ANALYSIS_SOURCE_HASH"),
    ))
}

fn analysis_cache_in(xbe_bytes: &[u8], dir: &Path, analyzer: &str) -> PathBuf {
    dir.join(format!(
        "analysis-{:016x}-{:x}-{analyzer}.h2a",
        fallback::fnv64(xbe_bytes),
        xbe_bytes.len()
    ))
}

/// Discovery's result for the image: read from `cache` when an earlier
/// build by the same analyzer left it there, otherwise computed and saved
/// there. With no `cache`, always computed. Returns whether the cache
/// served it.
pub fn analyze_cached(
    x: &xbe::Xbe,
    cache: Option<&Path>,
) -> Result<(analyze::Analysis, bool)> {
    if let Some(cache) = cache
        && let Ok(bytes) = std::fs::read(cache)
        && let Some(a) = analyze::Analysis::decode(&bytes)
    {
        // Pruning by modification time drops the least recently used.
        let _ = std::fs::File::options()
            .append(true)
            .open(cache)
            .and_then(|f| f.set_modified(std::time::SystemTime::now()));
        return Ok((a, true));
    }
    let a = analyze::analyze(x)?;
    // Best effort: a failed write only costs the next build the analysis.
    if let Some(cache) = cache
        && let Some(dir) = cache.parent()
        && let Err(e) =
            std::fs::create_dir_all(dir).and_then(|()| emit::output::replace(cache, &a.encode()))
    {
        eprintln!("analyze: not cached ({}: {e})", cache.display());
    }
    Ok((a, false))
}

/// Lift, emit and compile the image for the target selected by `H2_EMIT_TARGET`.
#[cfg(feature = "desktop-runtime")]
pub fn build(xbe_bytes: &[u8], out: &Path, jobs: usize) -> Result<PathBuf> {
    build_for(xbe_bytes, out, jobs, emit::compile::Arch::target()?)
}

/// Compile and stamp the image for the explicitly selected target.
#[cfg(feature = "desktop-runtime")]
pub fn build_for(xbe_bytes: &[u8], out: &Path, jobs: usize, arch: emit::compile::Arch) -> Result<PathBuf> {
    build_selected(xbe_bytes, out, jobs, arch, None)
}

#[cfg(feature = "desktop-runtime")]
pub fn build_slim(
    xbe_bytes: &[u8],
    out: &Path,
    jobs: usize,
    arch: emit::compile::Arch,
    coverage: &host_coverage::Coverage,
) -> Result<PathBuf> {
    anyhow::ensure!(host_report::module_from_env().is_none(), "--profile host conflicts with H2_HOST_MODULE");
    build_selected(xbe_bytes, out, jobs, arch, Some(coverage))
}

#[cfg(feature = "desktop-runtime")]
fn build_selected(
    xbe_bytes: &[u8],
    out: &Path,
    jobs: usize,
    arch: emit::compile::Arch,
    coverage: Option<&host_coverage::Coverage>,
) -> Result<PathBuf> {
    let total = Instant::now();
    let x = xbe::Xbe::parse(xbe_bytes)?;
    let lib = out.join(arch.module_file_name());
    // A failed build must not leave an old stamp vouching for a new dylib.
    let _ = std::fs::remove_file(stamp_path(&lib));
    let t = Instant::now();
    let (a, cached) = analyze_cached(&x, analysis_cache(xbe_bytes).as_deref())?;
    eprintln!(
        "analyze: {:.1}s, {} functions{}",
        t.elapsed().as_secs_f64(),
        a.functions.len(),
        if cached { " (cached)" } else { "" }
    );
    let t = Instant::now();
    let lifter = lift::Lifter::new(&a, &x)?;
    let entries: Vec<u32> = a.functions.keys().copied().collect();
    let lifted = emit::par::map(&entries, |&e| lifter.lift(e));
    let mut funcs = Vec::new();
    let mut unlifted = Vec::new();
    for (&e, f) in entries.iter().zip(lifted) {
        match f {
            Some(f) => funcs.push(f),
            None => unlifted.push(e),
        }
    }
    eprintln!(
        "lift: {:.1}s, {} lifted, {} not lifted{}",
        t.elapsed().as_secs_f64(),
        funcs.len(),
        unlifted.len(),
        if unlifted.is_empty() {
            String::new()
        } else {
            format!(" (first: {:x?})", &unlifted[..unlifted.len().min(8)])
        }
    );
    let t = Instant::now();
    let owners = emit::plan(funcs);
    eprintln!(
        "plan: {:.1}s (IR passes and owner shapes)",
        t.elapsed().as_secs_f64()
    );
    let src = out.join("src");
    let t = Instant::now();
    let codegen = emit::Codegen::from_env().map_err(anyhow::Error::msg)?;
    let w = if let Some(coverage) = coverage {
        let native = coverage.owners(&owners)?;
        eprintln!(
            "host coverage: {} of {} native owners, {} interpreted",
            native.len(),
            owners.len(),
            owners.len() - native.len()
        );
        emit::write_slim_module(&owners, &src, codegen, &native)?
    } else {
        let hot = host_report::module_from_env()
            .map(|path| host_report::module_owners(&path, &full_build_stamp(xbe_bytes, arch)))
            .transpose()?;
        emit::write_module_with_hot(&owners, &src, codegen, hot.as_ref())?
    };
    eprintln!(
        "emit: {:.1}s, {} owners ({} cut into parts), {} files ({} rewritten), {:.1} MB of C, {} entries, {} resumes",
        t.elapsed().as_secs_f64(),
        w.owners,
        w.split,
        w.files.len(),
        w.written,
        w.bytes as f64 / 1e6,
        w.entries,
        w.resumes
    );
    drop(owners);
    emit::compile::install_header(&src, Path::new(H2ABI_H)).map_err(anyhow::Error::msg)?;
    let b = emit::compile::build(&src, out, arch, jobs).map_err(anyhow::Error::msg)?;
    eprintln!(
        "clang ({}): {} compiled, {} reused{}, {:.1}s keying (dependency scans, hashing, pins), {:.1}s compiling, {:.1}s archive, {:.1} MB of objects",
        arch.name(),
        b.compiled,
        b.reused,
        if b.cold > 0 {
            format!(", {} sources in the cold tier", b.cold)
        } else {
            String::new()
        },
        b.cache_secs,
        b.compile_secs,
        b.archive_secs,
        b.object_bytes as f64 / 1e6
    );
    let t = Instant::now();
    emit::compile::host_dylib(
        &b,
        &src,
        &[
            ("h2boot_bridge.c", BRIDGE),
            ("h2boot_bridge_x87.c", BRIDGE_X87),
        ],
        &lib,
    )
    .map_err(anyhow::Error::msg)?;
    eprintln!(
        "link: {:.1}s -> {}",
        t.elapsed().as_secs_f64(),
        lib.display()
    );
    let stamp = match coverage {
        Some(coverage) => slim_stamp(xbe_bytes, arch, coverage),
        None => build_stamp_for(xbe_bytes, arch),
    };
    std::fs::write(stamp_path(&lib), stamp)
        .with_context(|| format!("stamping {}", lib.display()))?;
    eprintln!("build: {:.1}s", total.elapsed().as_secs_f64());
    Ok(lib)
}

/// A loaded generated module. Keep it alive while the runtime may call it.
pub struct Module {
    _lib: libloading::Library,
    pub entries: usize,
    pub resumes: usize,
    pub slim: bool,
}

/// `module` relative to the package root `root`, for `LoadPackagedLibrary`,
/// which takes only a path inside the package with no `..`: `None` when
/// the module is elsewhere.
pub fn package_relative(module: &Path, root: &Path) -> Option<PathBuf> {
    let rel = module.strip_prefix(root).ok()?;
    let normal = rel
        .components()
        .all(|c| matches!(c, std::path::Component::Normal(_)));
    (normal && !rel.as_os_str().is_empty()).then(|| rel.to_path_buf())
}

/// Opens the generated module. In a packaged (UWP) process on Windows a
/// module inside the package is opened with `LoadPackagedLibrary`, the
/// app container's loader (the Xbox app, docs/runtime/xbox.md); everything
/// else, and every other platform, uses `dlopen` or `LoadLibraryExW`.
unsafe fn open_module(path: &Path) -> Result<libloading::Library> {
    #[cfg(windows)]
    if let Some(root) = packaged::package_root()
        && let Some(rel) = package_relative(path, &root)
    {
        return unsafe { packaged::load(&rel) };
    }
    Ok(unsafe { emit::host::open_library(path) }?)
}

#[cfg(windows)]
mod packaged {
    use std::ffi::c_void;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::{Path, PathBuf};

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentPackagePath(length: *mut u32, path: *mut u16) -> i32;
        fn LoadPackagedLibrary(name: *const u16, reserved: u32) -> *mut c_void;
    }

    /// The installed package's root, when this process has package identity.
    pub fn package_root() -> Option<PathBuf> {
        let mut len = 0u32;
        // ERROR_INSUFFICIENT_BUFFER with the length when packaged,
        // APPMODEL_ERROR_NO_PACKAGE otherwise.
        // SAFETY: a null buffer with a zero length only asks for the length.
        if unsafe { GetCurrentPackagePath(&mut len, std::ptr::null_mut()) } != 122 || len == 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize];
        // SAFETY: `buf` holds `len` units.
        if unsafe { GetCurrentPackagePath(&mut len, buf.as_mut_ptr()) } != 0 {
            return None;
        }
        buf.truncate(buf.iter().position(|&c| c == 0).unwrap_or(buf.len()));
        Some(PathBuf::from(std::ffi::OsString::from_wide(&buf)))
    }

    /// # Safety
    /// As `libloading::Library::new`: the module's initialisers run.
    pub unsafe fn load(rel: &Path) -> anyhow::Result<libloading::Library> {
        let wide: Vec<u16> = rel
            .as_os_str()
            .encode_wide()
            .map(|c| {
                if c == u16::from(b'/') {
                    u16::from(b'\\')
                } else {
                    c
                }
            })
            .chain([0])
            .collect();
        // SAFETY: NUL-terminated; the caller's contract for the rest.
        let h = unsafe { LoadPackagedLibrary(wide.as_ptr(), 0) };
        if h.is_null() {
            anyhow::bail!(
                "LoadPackagedLibrary({}): {}",
                rel.display(),
                std::io::Error::last_os_error()
            );
        }
        // SAFETY: a module handle we own from here on.
        Ok(unsafe { libloading::os::windows::Library::from_raw(h as isize) }.into())
    }
}

impl Module {
    /// With a module built with `H2_PGO=gen`, write its raw profile now
    /// (`H2_PGO_OUT`, default `$TMPDIR/h2.profraw`) and say so on stderr.
    /// The profile runtime writes it again at exit; this one covers a run
    /// that ends here (`H2_BOOT_STOP_AT_PRESENT`) but not the process.
    pub fn write_profile(&self) {
        // SAFETY: load_module only accepts our modules.
        match unsafe { emit::compile::pgo::write_profile(&self._lib) } {
            None => {}
            Some(Ok(path)) => eprintln!("pgo: raw profile written to {}", path.display()),
            Some(Err(e)) => eprintln!("pgo: {e}"),
        }
    }
}

/// Install `bridge` into a generated dylib's boot bridge: the whole table
/// where it has `h2boot_install_v3`, else (a module built before
/// `h2abi_v1_exit`) the v2 prefix, whose `charge` and `fast_release` its
/// handlers still call. Returns the version installed.
///
/// # Safety
/// `lib` must be a dylib built with [`BRIDGE`] (any version) for this host.
pub unsafe fn install_bridge(lib: &libloading::Library, bridge: &shim::Bridge) -> Result<u32> {
    type Install = unsafe extern "C" fn(*const shim::Bridge, usize) -> u32;
    let (version, size, install) = unsafe {
        if let Ok(f) = lib.get::<Install>(b"h2boot_install_v3") {
            (3, std::mem::size_of::<shim::Bridge>(), *f)
        } else {
            let f = lib
                .get::<Install>(b"h2boot_install_v2")
                .context("boot bridge predates optional callbacks; rebuild the generated module")?;
            (2, shim::Bridge::V2_SIZE, *f)
        }
    };
    if unsafe { install(bridge, size) } != 1 {
        bail!("boot bridge layout mismatch; rebuild the generated module");
    }
    Ok(version)
}

/// Load the generated dylib, install the bridge, and register every entry
/// and resume with the runtime's dispatch (RUNTIME-CONTRACT.md §3).
///
/// # Safety
/// `path` must be a dylib produced by [`build`] for this host.
pub unsafe fn load_module(path: &Path, rt: &runtime::Runtime, xbe_bytes: &[u8]) -> Result<Module> {
    let lib =
        unsafe { open_module(path) }.with_context(|| format!("loading {}", path.display()))?;
    unsafe {
        let version: libloading::Symbol<unsafe extern "C" fn() -> u32> =
            lib.get(b"h2gen_module_abi_version")?;
        if version() != runtime::cpu::ABI_VERSION {
            bail!(
                "module ABI {} != runtime {}",
                version(),
                runtime::cpu::ABI_VERSION
            );
        }
        install_bridge(&lib, &shim::bridge())?;
        let install_x87: libloading::Symbol<
            unsafe extern "C" fn(*const *const std::ffi::c_void, u32),
        > = lib.get(b"h2x87bridge_install")?;
        let fns = shim::x87_helpers();
        install_x87(fns.as_ptr(), fns.len() as u32);
        let module: libloading::Symbol<*const runtime::H2GenModule> = lib.get(b"h2gen_module")?;
        let m = &**module;
        if m.abi_version != runtime::cpu::ABI_VERSION {
            bail!("module table ABI {}", m.abi_version);
        }
        let entries = std::slice::from_raw_parts(m.entries, m.entry_count as usize);
        let resumes = std::slice::from_raw_parts(m.resumes, m.resume_count as usize);
        let cold = match lib.get::<*const u32>(b"h2gen_cold_v1") {
            Ok(table) => {
                // The generated symbol starts with the two counts, followed
                // by those many [selector, owner] rows (emit's registry).
                let table = *table;
                let entries = *table as usize;
                let resumes = *table.add(1) as usize;
                let rows = table.add(2).cast::<[u32; 2]>();
                Some(fallback::ColdRoutes {
                    entries: std::slice::from_raw_parts(rows, entries),
                    resumes: std::slice::from_raw_parts(rows.add(entries), resumes),
                })
            }
            Err(_) => None,
        };
        if std::env::var_os(host_report::ENV).is_some_and(|p| !p.is_empty()) {
            anyhow::ensure!(cold.is_none(), "host audit requires a full generated module");
            rt.enable_guest_coverage(entries, resumes);
        }
        let mut rows = Vec::with_capacity(entries.len());
        for e in entries {
            let (Some(_), Some(v)) = (e.handler, e.abi_version) else {
                bail!("entry {:#x}: null row", e.addr)
            };
            if v() != runtime::cpu::ABI_VERSION {
                bail!("entry {:#x}: object ABI {}", e.addr, v());
            }
            rows.push(e.addr);
        }
        let res: Vec<(u32, u32, u32)> = resumes.iter().map(|r| (r.pc, r.owner, r.start)).collect();
        fallback::init_with_cold(xbe_bytes, &rows, &res, cold).map_err(anyhow::Error::msg)?;
        // Each row is registered as itself, a slim module's left-out owners
        // go to the interpreter, and the runtime hands every generated STOP
        // to the fallback (fallback.rs).
        rt.edit_dispatch(|d| -> Result<()> {
            d.register_generated_module(m, *version)
                .context("registering generated module")?;
            if let Some(cold) = cold {
                fallback::register_cold(d, cold, fallback::interpret_cold)
                    .context("registering slim host interpreter routes")?;
            }
            Ok(())
        })?;
        fallback::install_stop_recovery(rt);
        Ok(Module {
            _lib: lib,
            entries: entries.len(),
            resumes: resumes.len(),
            slim: cold.is_some(),
        })
    }
}

/// Shared note sink (stderr when `H2_BOOT_VERBOSE` is set).
pub mod log {
    pub fn verbose() -> bool {
        std::env::var_os("H2_BOOT_VERBOSE").is_some()
    }

    pub fn note(s: &str) {
        if verbose() {
            eprintln!("note: {s}");
        }
    }
}

/// Collected shim diagnostics.
pub static SHIM_EVENTS: Mutex<Vec<(u32, u32, String)>> = Mutex::new(Vec::new());

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal XBE: `.text` at 0x11000 holding `code` (int3-filled to
    /// 0x100 bytes), and `.rdata` at 0x12000 holding a kernel thunk table
    /// importing KeBugCheck. Synthetic bytes, like analyze's own tests.
    fn synthetic_xbe(code: &[u8]) -> Vec<u8> {
        const BASE: u32 = 0x10000;
        let mut d = vec![0u8; 0x1200];
        let put = |d: &mut Vec<u8>, o: usize, v: u32| d[o..o + 4].copy_from_slice(&v.to_le_bytes());
        d[..4].copy_from_slice(b"XBEH");
        put(&mut d, 0x104, BASE);
        put(&mut d, 0x108, 0x1000);
        put(&mut d, 0x10C, 0x3000);
        put(&mut d, 0x118, BASE + 0x400);
        put(&mut d, 0x11C, 2);
        put(&mut d, 0x120, BASE + 0x200);
        put(&mut d, 0x128, 0x11000 ^ 0xA8FC_57AB);
        put(&mut d, 0x158, 0x12000 ^ 0x5B6D_40B6);
        for (o, flags, va, raw_at, name_at, name) in [
            (0x200, 0x06, 0x11000, 0x1000, 0x280, &b".text\0"[..]),
            (0x238, 0x02, 0x12000, 0x1100, 0x290, b".rdata\0"),
        ] {
            put(&mut d, o, flags);
            put(&mut d, o + 4, va);
            put(&mut d, o + 8, 0x100);
            put(&mut d, o + 12, raw_at);
            put(&mut d, o + 16, 0x100);
            put(&mut d, o + 20, BASE + name_at as u32);
            d[name_at..name_at + name.len()].copy_from_slice(name);
        }
        d[0x1000..0x1100].fill(0xCC);
        d[0x1000..0x1000 + code.len()].copy_from_slice(code);
        put(&mut d, 0x1100, 0x8000_0000 | 95);
        d
    }

    /// The cached analysis is keyed by the image and the analyzer, served
    /// whole when both match, recomputed (and rewritten) when the file is
    /// damaged, and bypassed entirely when there is no cache.
    #[test]
    fn the_analysis_cache_serves_only_its_image_and_analyzer() {
        let dir =
            std::env::temp_dir().join(format!("h2boot-analysis-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // call 0x11010; ret / 0x11010: ret
        let one = synthetic_xbe(&[
            0xE8, 0x0B, 0, 0, 0, 0xC3, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC,
            0xC3,
        ]);
        let two = synthetic_xbe(&[0xC3]);
        let x1 = xbe::Xbe::parse(&one).unwrap();
        let x2 = xbe::Xbe::parse(&two).unwrap();
        let fresh = analyze::analyze(&x1).unwrap().encode();
        assert_ne!(
            fresh,
            analyze::analyze(&x2).unwrap().encode(),
            "the two images analyze differently"
        );

        let path = analysis_cache_in(&one, &dir, "analyzer-a");
        let (a, hit) = analyze_cached(&x1, Some(&path)).unwrap();
        assert!(!hit, "nothing cached yet");
        assert_eq!(a.encode(), fresh);
        let (a, hit) = analyze_cached(&x1, Some(&path)).unwrap();
        assert!(hit);
        assert_eq!(a.encode(), fresh, "the cache serves the same analysis");

        // Another image or another analyzer is another file.
        assert_ne!(path, analysis_cache_in(&two, &dir, "analyzer-a"));
        assert_ne!(path, analysis_cache_in(&one, &dir, "analyzer-b"));
        let (b, hit) =
            analyze_cached(&x2, Some(&analysis_cache_in(&two, &dir, "analyzer-a"))).unwrap();
        assert!(!hit);
        assert_eq!(b.encode(), analyze::analyze(&x2).unwrap().encode());
        assert!(
            !analyze_cached(&x1, Some(&analysis_cache_in(&one, &dir, "analyzer-b")))
                .unwrap()
                .1
        );

        // A damaged file is recomputed and replaced.
        std::fs::write(&path, b"not an analysis").unwrap();
        let (a, hit) = analyze_cached(&x1, Some(&path)).unwrap();
        assert!(!hit);
        assert_eq!(a.encode(), fresh);
        assert_eq!(std::fs::read(&path).unwrap(), fresh);

        // No cache (H2_TRACE): computed, and nothing read or written.
        let other = dir.join("unused.h2a");
        assert!(!analyze_cached(&x1, None).unwrap().1);
        assert!(!other.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The real cache name carries the analyzer's source hash, so editing
    /// the analyzer (or its dependencies or compiler) misses.
    #[test]
    fn the_analysis_cache_is_named_by_the_analyzer_source() {
        let p = analysis_cache_in(
            b"XBEH image",
            Path::new("/cache"),
            env!("ANALYSIS_SOURCE_HASH"),
        );
        assert!(
            p.to_string_lossy().contains(env!("ANALYSIS_SOURCE_HASH")),
            "{}",
            p.display()
        );
        if std::env::var_os("H2_TRACE").is_none() && std::env::var_os("H2_BOOT_CACHE").is_none() {
            assert_eq!(
                analysis_cache(b"XBEH image").unwrap().parent(),
                Some(&*std::env::temp_dir().join("h2boot-cache"))
            );
        }
    }

    #[test]
    fn generated_flags_are_part_of_the_dylib_stamp() {
        let stamp = |flags: &[String]| {
            stamp_with(b"synthetic", "all", "Codegen { accounting: false }", flags)
        };
        let base = vec!["-O1".to_owned()];
        let mut fast = base.clone();
        fast.push("-DH2_FASTMEM=1".to_owned());
        let mut profiled = base.clone();
        profiled.push("-DH2_PERF=1".to_owned());
        let mut direct = base.clone();
        direct.push("-DH2_DIRECT_CALLS=1".to_owned());
        assert_ne!(stamp(&base), stamp(&direct));
        assert_ne!(stamp(&base), stamp(&fast));
        assert_ne!(stamp(&base), stamp(&profiled));
        assert_eq!(stamp(&fast), stamp(&fast));
    }

    /// `h2 build`, `h2 run` and `h2 play` (and `h2 follow`, which runs
    /// `h2 play`) all stamp through here, so the stamp names the effective
    /// defaults: inline RAM, native x87/SSE, direct calls and accounting on unless
    /// set to `0`.
    #[test]
    fn the_stamp_records_the_effective_fast_path_defaults() {
        let stamp = build_stamp(b"synthetic");
        let on = |name: &str| std::env::var(name).as_deref() != Ok("0");
        for name in [
            "H2_FASTMEM",
            "H2_X87_NATIVE",
            "H2_SSE_NATIVE",
            "H2_DIRECT_CALLS",
        ] {
            assert_eq!(
                stamp.contains(&format!("\"-D{name}=1\"")),
                on(name),
                "{name}: {stamp}"
            );
        }
        let accounting = format!("{:?}", emit::Codegen::from_env().unwrap());
        assert!(stamp.contains(&format!("codegen {accounting} ")), "{stamp}");
    }

    #[test]
    fn passes_and_codegen_are_part_of_the_dylib_stamp() {
        let flags = vec!["-O1".to_owned()];
        let off = format!("{:?}", emit::Codegen::default());
        let on = format!("{:?}", emit::Codegen::accesses(true));
        let base = stamp_with(b"synthetic", "all", &off, &flags);
        assert_ne!(
            base,
            stamp_with(b"synthetic", "all", &on, &flags),
            "H2_ACCESS_COUNT"
        );
        assert_ne!(
            base,
            stamp_with(b"synthetic", "none", &off, &flags),
            "H2_PASSES"
        );
        assert_eq!(base, stamp_with(b"synthetic", "all", &off, &flags));
    }

    #[test]
    fn device_stamps_name_their_target_and_a_macos_one_is_unchanged() {
        use emit::compile::Arch;
        let flags = vec!["-O1".to_owned()];
        let mac = stamp_for(b"synthetic", "all", "c", &flags, Arch::Arm64);
        assert_eq!(
            mac,
            stamp_for(b"synthetic", "all", "c", &flags, Arch::X86_64)
        );
        assert!(
            !mac.contains("target"),
            "macOS stamps stay as they were: {mac}"
        );
        let tv = stamp_for(b"synthetic", "all", "c", &flags, Arch::Arm64Tvos);
        assert_ne!(mac, tv);
        assert!(
            tv.trim_end().ends_with(" target arm64-apple-tvos17.0"),
            "{tv}"
        );
        let ios = stamp_for(b"synthetic", "all", "c", &flags, Arch::Arm64Ios);
        assert_ne!(ios, mac);
        assert_ne!(ios, tv);
        assert!(
            ios.trim_end().ends_with(" target arm64-apple-ios17.0"),
            "{ios}"
        );
    }

    #[test]
    fn a_windows_stamp_names_its_target_and_its_module_is_a_dll() {
        use emit::compile::Arch;
        let flags = vec!["-O1".to_owned()];
        let mac = stamp_for(b"synthetic", "all", "c", &flags, Arch::Arm64);
        let win = stamp_for(b"synthetic", "all", "c", &flags, Arch::X86_64Windows);
        assert_ne!(mac, win);
        assert_ne!(
            win,
            stamp_for(b"synthetic", "all", "c", &flags, Arch::Arm64Tvos)
        );
        assert!(
            win.trim_end().ends_with(" target x86_64-pc-windows-msvc"),
            "{win}"
        );
        assert_eq!(
            win.strip_suffix(" target x86_64-pc-windows-msvc\n"),
            mac.strip_suffix('\n'),
            "only the target differs"
        );
        // What `h2 run`/`h2 play` look for is what `build` writes on this host.
        assert_eq!(DYLIB, Arch::host().module_file_name());
        assert_eq!(Arch::X86_64Windows.module_file_name(), "h2game.dll");
    }

    /// The console computes its stamp with `H2_OPT` unset (`pack` refuses
    /// it), so a Windows module's stamp records the target's own default
    /// level and flags, and the Mac's stays as it was.
    #[test]
    fn a_windows_stamp_records_its_default_level_and_cpu_flags() {
        use emit::compile::Arch;
        let win = stamp_flags(Arch::X86_64Windows).unwrap();
        let mac = stamp_flags(Arch::Arm64).unwrap();
        assert!(win.iter().any(|f| f == "-march=x86-64-v3"), "{win:?}");
        assert!(!mac.iter().any(|f| f.starts_with("-march")), "{mac:?}");
        if std::env::var_os("H2_OPT").is_none() {
            assert!(win.iter().any(|f| f == "-O2"), "{win:?}");
            assert!(mac.iter().any(|f| f == "-O1"), "{mac:?}");
        }
    }

    #[test]
    fn a_packaged_module_is_named_relative_to_the_package_root() {
        let root = Path::new("/pkg");
        assert_eq!(
            package_relative(Path::new("/pkg/h2game.dll"), root),
            Some(PathBuf::from("h2game.dll"))
        );
        assert_eq!(
            package_relative(Path::new("/pkg/lib/h2game.dll"), root),
            Some(PathBuf::from("lib/h2game.dll"))
        );
        // Outside the package, the package itself, or climbing out: the
        // ordinary loader.
        assert_eq!(package_relative(Path::new("/other/h2game.dll"), root), None);
        assert_eq!(package_relative(Path::new("/pkg"), root), None);
        assert_eq!(
            package_relative(Path::new("/pkg/../x/h2game.dll"), root),
            None
        );
    }

    #[test]
    fn read_xbe_takes_a_directory_holding_the_disc_files() {
        let dir = std::env::temp_dir().join(format!("h2boot-dir-disc-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("maps")).unwrap();
        std::fs::write(dir.join("Default.XBE"), b"XBEH synthetic").unwrap();
        assert_eq!(read_xbe(&dir).unwrap(), b"XBEH synthetic");
        std::fs::write(dir.join("raw.xbe"), b"XBEH raw").unwrap();
        assert_eq!(read_xbe(&dir.join("raw.xbe")).unwrap(), b"XBEH raw");
        std::fs::remove_file(dir.join("Default.XBE")).unwrap();
        assert!(read_xbe(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn slim_stamps_distinguish_coverage_even_when_the_owner_count_is_equal() {
        let a = host_coverage::Coverage { selectors: [(0x1000, 0x1000)].into() };
        let b = host_coverage::Coverage { selectors: [(0x1000, 0x1010)].into() };
        let arch = emit::compile::Arch::X86_64Linux;
        let image = b"image a";
        let stamp = slim_stamp(image, arch, &a);
        assert_ne!(stamp, full_build_stamp(image, arch));
        assert_ne!(stamp, slim_stamp(image, arch, &b));
        assert_ne!(stamp, slim_stamp(b"image b", arch, &a));
    }

    #[test]
    fn a_dylib_is_current_only_for_its_image_and_generator() {
        let dir = std::env::temp_dir().join(format!("h2boot-stamp-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let lib = dir.join(DYLIB);
        let (a, b) = (b"XBEH image a".as_slice(), b"XBEH image b".as_slice());
        assert!(!dylib_current(a, &lib), "missing");
        std::fs::write(&lib, b"").unwrap();
        assert!(
            !dylib_current(a, &lib),
            "no stamp: built before stamps, or the build failed"
        );
        std::fs::write(stamp_path(&lib), build_stamp(a)).unwrap();
        assert!(dylib_current(a, &lib));
        assert!(!dylib_current(b, &lib), "another image");
        std::fs::write(
            stamp_path(&lib),
            build_stamp(a).replace(env!("GENERATOR_SOURCE_HASH"), "0000000000000000"),
        )
        .unwrap();
        assert!(!dylib_current(a, &lib), "another generator");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
