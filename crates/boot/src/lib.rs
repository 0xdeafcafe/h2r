//! Boot glue: the whole XBE through lift, emit and clang into one host
//! dylib, then that dylib registered with the runtime, the kernel HLE
//! attached, and the entry thread run until something stops it.
//!
//! This is a tracer bullet, not the final shape of the engine: the
//! generated code is loaded as a dylib (with `c/boot_bridge.c` forwarding
//! its ABI imports to the runtime) so the host binary needs no link-time
//! knowledge of the generated symbols. See docs/runtime/BOOT.md.
//!
//! Nothing derived from the disc is written outside the caller's output
//! directory, which should be under `$TMPDIR` or `target/`.

pub mod account_screen;
pub mod agent_glue;
pub mod aim_assist;
pub mod campaign_cards;
pub mod campaign_info;
pub mod campaign_menu;
pub mod campaigns;
pub mod cheats;
pub mod community_variants;
pub mod controls_text;
pub mod coop_jip;
pub mod coop_latency;
pub mod coop_live;
pub mod coop_players;
pub mod coop_probe;
pub mod custom_games;
pub mod d3d_diag;
pub mod d3d_glue;
pub mod debug_menu_glue;
pub mod dsound_glue;
pub mod effect_census;
pub mod fallback;
pub mod fov_row;
pub mod game_glue;
pub mod game_setup_help;
pub mod h1_options;
pub mod h1_options_row;
pub mod h1_variants;
pub mod health;
pub mod host_perf;
pub mod hot_glue;
pub mod hud;
pub mod hud_swap;
pub mod icons;
pub mod keyboard_glue;
pub mod layout_glue;
pub mod level_boot;
pub mod list_class;
pub mod live_glue;
pub mod live_notice;
pub mod loaded_map;
pub mod lockstep_glue;
pub mod map_bases;
pub mod map_copy;
pub mod map_override;
pub mod map_select;
pub mod map_slots;
pub mod mod_slots;
pub mod mods_screen;
pub mod movies_glue;
pub mod native_lights_glue;
pub mod native_objects_glue;
pub mod native_post_glue;
pub mod native_post_load;
pub mod native_transparents_glue;
pub mod native_world_glue;
pub mod new_maps;
pub mod pad_speaker;
pub mod precision;
pub mod registration;
pub mod render_probe;
pub mod render_thread;
pub mod retail_rows;
pub mod sampling;
pub use live_telemetry as telemetry;
pub mod bot_glue;
pub mod bot_mark;
pub mod bots_nav_auto;
mod clanker_card;
pub mod clankermatch;
pub mod clankers;
pub mod pause_menu;
pub mod setting_defs;
pub mod sha;
pub mod shim;
pub mod signin_keyboard;
pub mod signin_screen;
pub mod sim_bots;
pub mod sim_glue;
pub mod skulls;
pub mod skulls_custom;
pub mod skulls_menu;
pub mod skulls_mp;
pub mod skulls_mp_row;
pub mod skulls_screen;
pub mod snapshot_glue;
pub mod snapshot_probe;
pub mod speaker_row;
pub mod speaker_setting;
pub mod sse;
pub mod steam_speaker;
pub mod stopgaps;
pub mod sweep_diag;
pub mod tier;
pub mod touch_glue;
pub mod trace;
pub mod ui;
pub mod ui_kit;
pub mod ui_watch;
pub mod variant_ext;
pub mod variant_options;
pub mod variant_page;
pub mod video_screen;
pub mod voice_glue;
pub mod voice_settings;
pub mod wrappers;
pub mod x87;
pub mod xapi_glue;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result, bail};
pub use live_hle::LiveConfig;
use runtime::{Event, GuestFn};

/// The bridge source linked into the generated dylib.
pub const BRIDGE: &str = include_str!("../c/boot_bridge.c");
/// The `h2x87_v1_*` forwarders generated code imports since x87 lowering:
/// emit's own x87 bridge, installed with `h2x87bridge_install`.
pub const BRIDGE_X87: &str = concat!(
    "#include \"h2gen.h\"\n",
    include_str!("../../emit/c/bridge_x87.c")
);
/// The runtime ABI header, copied next to the generated sources.
pub const H2ABI_H: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../runtime/include/h2abi.h");
/// File name of the linked module inside the build directory.
pub const DYLIB: &str = if cfg!(windows) {
    "h2game.dll"
} else {
    "libh2game.dylib"
};

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
/// with the optimisation tiers when `H2_OPT_COLD` asks for them.
/// [`build`] writes it next to the dylib
/// (`<dylib>.stamp`) once linked; [`run`] refuses a dylib whose stamp
/// differs, and `h2 run` rebuilds instead.
pub fn build_stamp(xbe_bytes: &[u8]) -> String {
    match stamp_flags(stamp_target()) {
        Ok(flags) => stamp_with(
            xbe_bytes,
            &emit::opt::Passes::from_env().map_or_else(|e| e, |p| p.spec()),
            &emit::Codegen::from_env().map_or_else(|e| e, |c| format!("{c:?}")),
            &flags,
        ),
        Err(e) => format!("invalid compiler flags: {e}"),
    }
}

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

fn stamp_path(dylib: &Path) -> PathBuf {
    let mut p = dylib.as_os_str().to_owned();
    p.push(".stamp");
    PathBuf::from(p)
}

/// Whether `dylib` exists and was built from this image by this generator.
pub fn dylib_current(xbe_bytes: &[u8], dylib: &Path) -> bool {
    dylib.exists()
        && std::fs::read_to_string(stamp_path(dylib)).is_ok_and(|s| s == build_stamp(xbe_bytes))
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
pub(crate) fn analyze_cached(
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

/// Lift, emit and compile the image into `out/libh2game.dylib`.
pub fn build(xbe_bytes: &[u8], out: &Path, jobs: usize) -> Result<PathBuf> {
    let total = Instant::now();
    let x = xbe::Xbe::parse(xbe_bytes)?;
    // The host, tvOS with H2_EMIT_TARGET=tvos (docs/runtime/tvos.md), or Windows
    // with H2_EMIT_TARGET=windows (docs/runtime/windows.md).
    let arch = emit::compile::Arch::target().map_err(anyhow::Error::msg)?;
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
    let w = emit::write_module(&owners, &src)?;
    eprintln!(
        "emit: {:.1}s, {} owners, {} files ({} rewritten), {:.1} MB of C, {} entries, {} resumes",
        t.elapsed().as_secs_f64(),
        w.owners,
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
    std::fs::write(stamp_path(&lib), build_stamp(xbe_bytes))
        .with_context(|| format!("stamping {}", lib.display()))?;
    eprintln!("build: {:.1}s", total.elapsed().as_secs_f64());
    Ok(lib)
}

#[repr(C)]
struct GenEntry {
    addr: u32,
    kind: u32,
    f: Option<GuestFn>,
    abi_version: Option<unsafe extern "C" fn() -> u32>,
}

#[repr(C)]
struct GenResume {
    pc: u32,
    owner: u32,
    start: u32,
}

#[repr(C)]
struct GenModule {
    abi_version: u32,
    entry_count: u32,
    resume_count: u32,
    reserved: u32,
    entries: *const GenEntry,
    resumes: *const GenResume,
}

/// A loaded generated module. Keep it alive while the runtime may call it.
pub struct Module {
    _lib: libloading::Library,
    pub entries: usize,
    pub resumes: usize,
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
    Ok(unsafe { libloading::Library::new(path) }?)
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
        let module: libloading::Symbol<*const GenModule> = lib.get(b"h2gen_module")?;
        let m = &**module;
        if m.abi_version != runtime::cpu::ABI_VERSION {
            bail!("module table ABI {}", m.abi_version);
        }
        let entries = std::slice::from_raw_parts(m.entries, m.entry_count as usize);
        let resumes = std::slice::from_raw_parts(m.resumes, m.resume_count as usize);
        let mut rows = Vec::with_capacity(entries.len());
        for e in entries {
            let (Some(f), Some(v)) = (e.f, e.abi_version) else {
                bail!("entry {:#x}: null row", e.addr)
            };
            if v() != runtime::cpu::ABI_VERSION {
                bail!("entry {:#x}: object ABI {}", e.addr, v());
            }
            rows.push((e.addr, f));
        }
        let res: Vec<(u32, u32, u32)> = resumes.iter().map(|r| (r.pc, r.owner, r.start)).collect();
        let conflicts = fallback::init(xbe_bytes, &rows, &res).map_err(anyhow::Error::msg)?;
        // Each row is registered as itself, and the runtime hands its STOPs
        // to the fallback (fallback.rs). H2_BOOT_TRAMPOLINE=1 routes every
        // row through the old trampoline instead, for comparison: that also
        // turns the module's direct calls off in effect, since a direct call
        // needs the registered handler to be the callee's own.
        let trampoline = std::env::var("H2_BOOT_TRAMPOLINE").as_deref() == Ok("1");
        if trampoline {
            eprintln!("boot: H2_BOOT_TRAMPOLINE=1: every entry through the fallback trampoline");
            // The trampoline finds a resume's handler by selector alone.
            if conflicts != 0 {
                eprintln!("warning: {conflicts} selectors map to more than one handler");
            }
        }
        rt.edit_dispatch(|d| -> Result<()> {
            for &(addr, f) in &rows {
                let f = if trampoline {
                    fallback::trampoline as runtime::GuestFn
                } else {
                    f
                };
                d.register_generated(addr, runtime::cpu::ABI_VERSION, f)
                    .with_context(|| format!("entry {addr:#x}"))?;
            }
            for r in resumes {
                d.register_resume(r.pc, r.owner, r.start)
                    .with_context(|| format!("resume {:#x}", r.pc))?;
            }
            Ok(())
        })?;
        if !trampoline {
            fallback::install_stop_recovery(rt);
        }
        Ok(Module {
            _lib: lib,
            entries: entries.len(),
            resumes: resumes.len(),
        })
    }
}

/// Options for [`run`].
#[derive(Debug, Clone)]
pub struct RunOpts {
    /// The XISO, mounted as the DVD drive.
    pub disc: PathBuf,
    /// Host root for the hard-disk partitions (E:, cache, utility).
    pub data_root: PathBuf,
    pub max_slices: u64,
    /// Register crates/xapi (`xapi::register`) and pump its input.
    pub xapi: bool,
    pub transfers_per_slice: u64,
    /// Install crates/boot's kernel stopgaps (see stopgaps.rs).
    pub stopgaps: bool,
    /// Kernel-call trace output (TSV), see trace.rs.
    pub trace: Option<PathBuf>,
    /// Register crates/d3d's adapters (headless).
    pub d3d: bool,
    /// Register crates/bink-hle: playback of the movies installed in
    /// `movies`, skip mode for the rest (movies_glue.rs).
    pub bink: bool,
    /// Register crates/dsound's overrides (headless mixer).
    pub dsound: bool,
    /// Which movies may play: those `h2 movies` installed and current for
    /// the disc (`movies_glue::Movies`; level boots and headless runs skip
    /// by default, `--no-movies` always).
    pub movies: movies_glue::Movies,
}

fn describe(ev: Option<&Event>, thread: u32) -> String {
    match ev {
        Some(Event::KernelCall(k)) => {
            let args: Vec<String> = k
                .stack_args
                .iter()
                .map(|a| a.as_ref().map_or("?".into(), |v| format!("{v:#x}")))
                .collect();
            format!(
                "kernel call #{} {} (ret {:x?}, args [{}], ecx {:#x}, edx {:#x})",
                k.ordinal,
                k.name,
                k.return_pc,
                args.join(", "),
                k.cpu.ecx,
                k.cpu.edx
            )
        }
        Some(Event::UnrecordedFault { pc }) => match shim::lookup(thread, *pc) {
            Some(s) => s,
            None => format!("unrecorded fault at {pc:#x}"),
        },
        Some(e) => format!("{e:x?}"),
        None => "no event".into(),
    }
}

/// Why `H2_PERF_FROM`, `H2_TIMELINE` or `H2_PROFILE` will do nothing in
/// this run, if they won't: the session needs an h2 built with
/// `--features perf`, and opens only at `H2_PERF_FROM` (a present number
/// or `level`), so `H2_TIMELINE`/`H2_PROFILE` alone write nothing.
pub fn perf_env_note(
    perf_build: bool,
    perf_from: bool,
    session_vars: bool,
) -> Option<&'static str> {
    match (perf_build, perf_from, session_vars) {
        (false, true, _) | (false, _, true) => Some(
            "perf: H2_PERF_FROM, H2_TIMELINE and H2_PROFILE need an h2 built with --features perf; no session, nothing written",
        ),
        (true, false, true) => Some(
            "perf: H2_TIMELINE/H2_PROFILE open their session at H2_PERF_FROM (a present or `level`), which is unset; nothing written",
        ),
        _ => None,
    }
}

/// What a run ended with, for callers and docs.
pub struct RunReport {
    pub outcome: String,
    pub stop: String,
    pub eip: u32,
    pub kernel_log: Vec<String>,
}

/// What the run is hosted in. The default is headless: a `RecordingBackend`
/// (or `H2_BOOT_GFX`), logical-time audio into a `NullSink`, a keyboard pad
/// only `H2_BOOT_INPUT` presses, every kernel call kept in memory, and
/// `max_slices` as the only stop. A windowed host (`h2 play`, crates/app)
/// fills these in; see docs/runtime/BOOT.md "Hosted runs".
#[derive(Default)]
pub struct Host {
    /// D3D renders here (present hook attached) instead of the headless
    /// backend; `H2_BOOT_GFX` is then ignored.
    pub gfx: Option<d3d::GfxBackend>,
    /// With `gfx`: a second backend on the same device that renders the
    /// frame buffers at this scale and presents, while `gfx` stays at the
    /// guest's size and answers the guest (`d3d::ScaledBackend`,
    /// `H2_RENDER_SCALE`, docs/graphics/high-resolution.md). `gfx` then has no
    /// present hook; the view has it. At 1 the view sleeps; with
    /// `settings`, the scale follows the Resolution setting during the run.
    pub gfx_view: Option<(d3d::GfxBackend, u32)>,
    /// Run `gfx`'s work on a render thread (`d3d::ThreadedBackend`,
    /// docs/graphics/render-thread.md); off in `Host::default()`. Hosts set it from
    /// [`d3d_glue::render_thread_requested`].
    pub render_thread: bool,
    /// Where the logical-time audio output plays (an audio device), instead
    /// of a `NullSink` (`dsound_glue::install_with_sink`).
    pub audio_sink: Option<dsound_glue::SinkOpener>,
    /// Opens the host's microphone and voice output for voice chat
    /// (`voice_glue`, docs/live/live-voice.md). `None`: the communicator
    /// still works when a microphone is chosen, with a silent microphone.
    pub voice_io: Option<voice::io::OpenIo>,
    /// The controller speaker (docs/audio/playstation-audio.md): a second sink
    /// for the sounds its routes pick, and the sound index built from each
    /// map the game opens. Needs `audio_sink`'s logical output; ignored
    /// without DirectSound.
    pub pad_speaker: Option<dsound_glue::PadSpeaker>,
    /// Our settings (`setting_defs`), behind the Controller Speaker row on
    /// Controls and the Video screen's rows (`video_screen::model`), and
    /// read by the camera hook (`fov_row`). `None` (headless runs) leaves
    /// the Controls screen and the camera as retail.
    pub settings: Option<Arc<settings::Store>>,
    /// XAPI's input manager (`xapi_glue::register_with`).
    pub input: Option<input::InputManager>,
    /// The keyboard pad `H2_BOOT_INPUT` presses, when `input` is given.
    pub keyboard: Option<input::backend::keyboard::KeyboardMouse>,
    /// Button icons for the device in use (`icons`, docs/input/button-icons.md).
    /// `None` takes `H2_ICONS`; with that unset too, the fonts and the glyph
    /// lookups are left alone.
    pub icons: Option<icons::IconOptions>,
    /// Keep only this many kernel calls in memory (`trace::install_bounded`).
    pub trace_limit: Option<usize>,
    /// Start this campaign level in place of the main menu (`level_boot`,
    /// docs/runtime/level-boot.md). `None` reads `H2_LEVEL` and `H2_DIFFICULTY`.
    pub level: Option<level_boot::Request>,
    /// Called after every slice, input pump and audio advance, with the
    /// kernel (for its logical clock). Returning false ends the run
    /// (outcome `HostStop`).
    pub between_slices: Option<BetweenSlices>,
    /// Our debug menu (docs/runtime/debug-menu.md): its calls go to the
    /// agent (`H2_AGENT`'s, else a local one with no socket), and it gets
    /// the menu's host methods (`settings.*`, `perf.overlay`, `cheat.*`).
    pub debug_menu: Option<debug_menu::Shared>,
    /// The perf overlay's on/off switch, for `perf.overlay`.
    pub perf_overlay: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// Called once with the kernel's VFS when it is attached, before the
    /// game runs: `h2 play` watches U: through it for cloud saves
    /// (`vfs::Vfs::set_change_observer`, docs/live/live-cloud-saves.md).
    pub on_vfs: Option<VfsReady>,
}

/// [`Host::on_vfs`].
pub type VfsReady = Box<dyn FnOnce(&Arc<std::sync::Mutex<vfs::Vfs>>) + Send>;

/// [`Host::between_slices`]: true to keep running.
pub type BetweenSlices = Box<dyn FnMut(&kernel::Kernel) -> bool>;

/// Map the image, attach the kernel, register the generated module and run
/// the entry thread.
pub fn run(xbe_bytes: &[u8], dylib: &Path, opts: &RunOpts) -> Result<RunReport> {
    run_hosted(xbe_bytes, dylib, opts, Host::default())
}

/// [`run`] inside a host: its D3D backend, audio sink and input, and a
/// callback between slices that paces the run and can end it.
pub fn run_hosted(xbe_bytes: &[u8], dylib: &Path, opts: &RunOpts, host: Host) -> Result<RunReport> {
    run_hosted_with_live(xbe_bytes, dylib, opts, host, &LiveConfig::default())
}

/// [`run`] with the opt-in client configuration. Offline callers keep [`run`].
pub fn run_with_live(
    xbe_bytes: &[u8],
    dylib: &Path,
    opts: &RunOpts,
    live: &LiveConfig,
) -> Result<RunReport> {
    run_hosted_with_live(xbe_bytes, dylib, opts, Host::default(), live)
}

/// [`run_hosted`] with explicit Live client settings. The host/CLI supplies
/// the account path and server base URL; no implicit environment settings.
pub fn run_hosted_with_live(
    xbe_bytes: &[u8],
    dylib: &Path,
    opts: &RunOpts,
    host: Host,
    live: &LiveConfig,
) -> Result<RunReport> {
    run_configured(xbe_bytes, dylib, opts, host, live, None)
}

/// Runs a prepared headless match, optionally capturing it in Live.
pub fn run_sim(
    xbe_bytes: &[u8],
    dylib: &Path,
    opts: &RunOpts,
    sim: sim_glue::Sim,
    upload: Option<telemetry::TelemetryConfig>,
) -> Result<RunReport> {
    run_configured(
        xbe_bytes,
        dylib,
        opts,
        Host::default(),
        &LiveConfig::default(),
        Some((sim, upload)),
    )
}

fn run_configured(
    xbe_bytes: &[u8],
    dylib: &Path,
    opts: &RunOpts,
    mut host: Host,
    live: &LiveConfig,
    sim: Option<(sim_glue::Sim, Option<telemetry::TelemetryConfig>)>,
) -> Result<RunReport> {
    let x = xbe::Xbe::parse(xbe_bytes)?;
    // Either tier's module, by its own stamp (tier.rs).
    let Some(tier) = tier::module_tier(xbe_bytes, dylib) else {
        bail!(
            "{} was not built from this image by this generator (stamp {} wants {:?}, or the fast tier's {:?}); rebuild it with `h2 build`",
            dylib.display(),
            stamp_path(dylib).display(),
            build_stamp(xbe_bytes).trim(),
            tier::fast_stamp(xbe_bytes).trim()
        );
    };
    eprintln!(
        "tier: running the {} module {}",
        tier.name(),
        dylib.display()
    );
    // Keep Halo's fixed arena, physical [0x61000, 0x3145000), free, with its
    // contiguous window at 0x80061000 and (boot addition) its write-combined
    // window at 0xf0061000, so MmAllocateContiguousMemoryEx can return
    // exactly what the game assumes. The write-combined window is a boot
    // stopgap: --no-stopgaps leaves it out.
    let mut options = runtime::loader::LoadOptions::halo2();
    if opts.stopgaps {
        for r in &mut options.physical_reservations {
            r.aliases.push(d3d_glue::WC_BASE | r.physical.start);
        }
    }
    let mut loaded = runtime::load_xbe_with(xbe_bytes, &options).context("mapping the XBE")?;
    if opts.stopgaps {
        d3d_glue::map_reserved_write_combined(&loaded.runtime)?;
    }
    for n in &loaded.permission_notes {
        log::note(&format!("page {:#x} widened to {:?}", n.page, n.effective));
    }
    let t = Instant::now();
    // Shadow pages: trace 0xa0ff0000, wrappers 0xa0ff1000, d3d 0xa0ff2000,
    // barrier 0xa0ff4000, live notice 0xa0ff6000. Hooks placed by the
    // feature plan (telemetry, lockstep, the level-loaded mark, our strings)
    // take theirs from the feature arena (`feature::arena`).
    // SAFETY: the dylib is ours and stays loaded until the end of this fn.
    let module = unsafe { load_module(dylib, &loaded.runtime, xbe_bytes)? };
    // Interpreting past unsupported instructions is a stopgap too.
    fallback::set_recovery(opts.stopgaps);
    if opts.stopgaps {
        let unread = fallback::install_unread_targets(&loaded.runtime);
        crate::log::note(&format!(
            "fallback: {unread} unread jump-table targets registered"
        ));
    }
    eprintln!(
        "registered {} entries, {} resumes in {:.1}s",
        module.entries,
        module.resumes,
        t.elapsed().as_secs_f64()
    );
    let k = kernel::Kernel::for_image(&mut loaded).context("attaching the kernel")?;
    // The disc's map hashes, shared by the package resolver and the Live
    // map gate's bases (docs/game/maps.md §5): each disc map is read at most
    // once a session, or not at all when the stamp under <data>/maps/cache
    // already has it.
    let disc_hashes = content::DiscHashes::default();
    let map_gate = live
        .enabled
        .then(|| map_bases::gate(&opts.disc, &opts.data_root, disc_hashes.clone()));
    let _live = live_glue::install(&loaded.runtime, &k, live, map_gate.clone())?;
    // Features declare their hooks here; the plan is built and installed
    // once, below, after the last of them (docs/engine/feature-engine.md).
    let mut features =
        feature::Registry::new(feature::Switches::from_env().map_err(anyhow::Error::msg)?);
    // Our text on the cannot-connect dialog when Live is off or its server
    // is unreachable (docs/live/live.md, "Client HLE: Live not set up").
    live_notice::install(&loaded.runtime, live).context("installing the Live notice")?;
    std::fs::create_dir_all(&opts.data_root)?;
    let mut cfg = vfs::Config::new(&opts.data_root, x.certificate.title_id);
    // Converted Halo 1 maps follow the sandbox chosen per game (h1_variants).
    cfg.variants = h1_variants::mp_variants();
    cfg.content_maps = new_maps::mounted_packages();
    // H2_SIM=<match file>: sim mode (sim_glue.rs, docs/runtime/sim.md).
    let (sim_mode, upload) = match sim {
        Some((sim, upload)) => (Some(sim), upload),
        None => (sim_glue::from_env()?, None),
    };
    let uploading_sim = upload.is_some();
    if sim_mode.is_some() && host.gfx.is_some() {
        anyhow::bail!("{}: sim mode is headless; this host has a window", sim::ENV);
    }
    if opts.xapi && host.input.is_none() {
        if sim_mode.is_some() {
            // No keyboard pad: all four ports are the bots'.
            host.input = Some(input::InputManager::new(Default::default())?);
        } else {
            let (input, keyboard) = input::InputManager::with_keyboard_pad(Default::default())?;
            host.input = Some(input);
            host.keyboard = Some(keyboard);
        }
    }
    let icon_options = match host.icons {
        Some(o) => Some(o),
        None => icons::choice_from_env()?.map(|choice| icons::IconOptions {
            choice,
            gamepads: false,
        }),
    };
    let icons = icon_options
        .map(|o| Arc::new(icons::Icons::new(o, host.keyboard.as_ref())))
        .filter(|i| i.enabled());
    // Packages for maps the disc doesn't have become content packages
    // under T:\$c, which the main menu reads for its map list
    // (docs/game/maps.md §4); before the VFS lists that folder.
    let mods_not_shown = new_maps::install_at_boot(&opts.data_root, &opts.disc);
    // The map select by category (docs/game/map-select.md), from the list
    // the main menu will build: the disc's maps and those packages.
    let _ = map_select::prepare_at_boot(&opts.data_root, &opts.disc, mods_not_shown);
    // The Halo 1 maps' sandbox and the campaign's assets start as the
    // settings say; the pregame screen changes them per game, and the VFS
    // and the health model follow (h1_variants, docs/halo1/multiplayer.md).
    h1_variants::init_from_store(&setting_defs::load(&opts.data_root, &[]).0);
    // A second campaign (docs/halo1/campaign-picker.md): read before the
    // VFS mounts, which serves its converted maps.
    let campaigns = campaigns::boot_state(&opts.data_root, &opts.disc);
    let campaign_maps = campaigns
        .as_ref()
        .map(|s| campaigns::host_maps(&opts.data_root, s))
        .unwrap_or_default();
    // Explicit editor maps still use H2_MAP_DIR. Installed packages use
    // their independent content offers (docs/game/maps.md).
    let maps = map_override::dir();
    let packages = map_override::packages_dir(&opts.data_root);
    // Our campaigns' level pictures, added to mainmenu as the VFS serves it
    // (docs/halo1/campaign-picker.md, "Pictures").
    let cards = campaigns.as_ref().and_then(|s| {
        let levels: Vec<(String, String)> = s
            .built
            .iter()
            .flat_map(|b| b.levels.iter().map(|l| (b.key.clone(), l.map.clone())))
            .collect();
        campaign_cards::CardRewrite::new(
            campaign_cards::read_cards(&opts.data_root, &levels),
            s.cards.clone(),
        )
    });
    // Every font gets the bot mark's gear (docs/game/bots.md §8.2.2), after
    // the icon sets.
    let fonts = vfs::Chain(
        icons.as_ref().map(|i| i.font_rewrite()),
        bot_mark::GearRewrite,
    );
    let rewrites = Some(vfs::Chain(fonts, cards));
    let mounted = map_override::mount(
        cfg.clone(),
        &opts.disc,
        rewrites,
        maps.as_deref(),
        &campaign_maps,
    );
    if let Some(i) = &icons {
        icons::install(&loaded.runtime, Arc::clone(i))?;
    }
    let v = match mounted {
        Ok(v) => v,
        Err(e) => {
            // A raw XBE (or a file that is not an XISO) still boots, with no
            // DVD mounted: the first map open then fails in the guest.
            eprintln!(
                "warning: {} is not a mountable XISO ({e:?}); running with no DVD",
                opts.disc.display()
            );
            vfs::Vfs::with_storage(cfg).map_err(|e| anyhow::anyhow!("vfs: {e:?}"))?
        }
    };
    k.set_vfs(v).context("attaching the VFS")?;
    if let (Some(ready), Some(vfs)) = (host.on_vfs.take(), k.vfs()) {
        ready(&vfs);
    }
    // Live downloads publish their offers before the join is handed to the
    // game. The next main-menu tick refreshes the in-memory map list.
    if let (Some(vfs), Some(gate)) = (k.vfs(), &map_gate) {
        let disc = opts.disc.clone();
        let maps = new_maps::mounted_packages();
        let variants = h1_variants::mp_variants();
        gate.on_prepare(Box::new(move |id| {
            if maps.has_id(id) || variants.has_id(id) {
                Ok(())
            } else {
                Err(format!("map {id} has no playable slot on this console"))
            }
        }));
        gate.on_installed(Box::new(move |_| {
            let mut vfs = vfs.lock().unwrap_or_else(|e| e.into_inner());
            map_select::packages_changed(&disc);
            vfs.rescan_maps();
        }));
    }
    let uses = opts
        .stopgaps
        .then(|| stopgaps::install(&k, &loaded.runtime));
    if let Some(u) = &uses {
        // Retired with the kernel round: the class-34 wrapper
        // (wrappers::install), XeLoad/UnloadSection (install_sections) and
        // the FILE_SYNCHRONOUS_IO_ALERT rewrite. H2_BOOT_OLD_GLUE=1 brings
        // them back.
        wrappers::map_shadow(&loaded.runtime)?;
        if std::env::var_os("H2_BOOT_OLD_GLUE").is_some() {
            wrappers::install(&loaded.runtime, u)?;
            stopgaps::install_sections(&loaded.runtime, xbe_bytes.to_vec(), u);
        }
        if !opts.xapi {
            xapi_glue::install_no_xapi_mount(&loaded.runtime, &k, 5, u);
        }
        // Retired with kernel round 3, XAPI round 2 and DSound round 2
        // (docs/runtime/BOOT.md "Retired"); H2_BOOT_OLD_GLUE=1 brings them back.
        if std::env::var_os("H2_BOOT_OLD_GLUE").is_some() {
            game_glue::install(&loaded.runtime, u);
            wrappers::install_alertable(&loaded.runtime, u);
            wrappers::install_eeprom_all(&loaded.runtime, u);
            wrappers::install_mutants(&loaded.runtime, u);
            wrappers::install_top_down(&loaded.runtime, u);
            wrappers::install_free_virtual(&loaded.runtime, u);
            wrappers::install_set_allocation(&loaded.runtime, u);
            wrappers::install_full_attributes(&loaded.runtime, u);
        }
    }
    // Community classics in the built-in variant list (docs/game/community-gametypes.md).
    community_variants::install(&loaded.runtime)
        .context("installing the community game variants")?;
    // Extra button layouts and the Controller Speaker row on the Controls
    // screen (docs/input/controller-layouts.md, docs/audio/playstation-audio.md).
    // The Video row on the Settings list opens our Video screen
    // (docs/game/settings-menu.md).
    let model = match host.settings.clone() {
        Some(store) => Some(Arc::new(
            video_screen::model(store, video_screen::Switches::from_env())
                .map_err(anyhow::Error::msg)?,
        )),
        None => None,
    };
    // More than one campaign (docs/halo1/campaign-picker.md): campaigns
    // installed under <data>/campaigns get their level lists and the
    // picker in front of the level select. None installed: nothing is
    // hooked and the disc is not read for it.
    let campaigns = campaigns.map(|c| campaign_menu::prepare(c, host.settings.clone()));
    let level = match (host.level.take(), &sim_mode) {
        (Some(l), _) => Some(l),
        (None, Some(s)) => Some(s.level_request(&opts.disc, &opts.data_root)?),
        (None, None) => level_boot::from_env(&opts.disc, Some(&opts.data_root)).context("H2_LEVEL")?,
    };
    let perf_from_var = std::env::var("H2_PERF_FROM").ok();
    let perf_from = perf_from_var
        .as_deref()
        .and_then(level_boot::PerfFrom::parse);
    if let Some(v) = perf_from_var.as_deref().filter(|_| perf_from.is_none()) {
        eprintln!("perf: H2_PERF_FROM={v:?} is neither a present number nor `level`; ignored");
    }
    let level_name = level.as_ref().map(|l| l.name.clone());
    // Every feature the plan gets up to the touch glue is registered in
    // `registration` (and its tests build the plan from the same code).
    let core = registration::register_core(
        &mut features,
        registration::CoreInputs {
            rt: &loaded.runtime,
            telemetry: match upload {
                Some(config) => Ok(config),
                None => telemetry::TelemetryConfig::from_live(
                    live.enabled,
                    live.accounts_path.as_deref(),
                    live.server_url.as_deref(),
                ),
            },
            vfs: k.vfs(),
            model,
            campaigns,
            settings: host.settings.clone(),
            live: _live.clone(),
            data_root: &opts.data_root,
            packages: &packages,
            level,
            perf_level: perf_from == Some(level_boot::PerfFrom::Level),
            xapi: opts.xapi,
        },
    )?;
    let (skulls_on, skulls_mp_on, skulls_custom_on, health_on) = (
        core.skulls_on,
        core.skulls_mp_on,
        core.skulls_custom_on,
        core.health_on,
    );
    // The bots' nav meshes, built in the background where missing or stale
    // (docs/game/bots.md §10 stage 2); H2_BOTS_NAV_AUTO=0 turns it off.
    // Sim mode never builds them: a build on a thread is at the wall
    // clock's mercy (docs/runtime/sim.md §8).
    if let (true, Some(vfs), None) = (core.bots_on, k.vfs(), &sim_mode) {
        bots_nav_auto::start(&opts.data_root, &opts.disc, vfs, level_name.clone());
    }
    if let Some(s) = &sim_mode {
        if !core.bots_on {
            anyhow::bail!("sim: the bots are off (H2_FEATURES or no XAPI); a match needs them");
        }
        s.add_bots();
    }
    let _telemetry = core.telemetry;
    if uploading_sim && _telemetry.is_none() {
        anyhow::bail!("sim upload requires the telemetry feature");
    }
    // Frame species and weapon names come from the loaded map's tag names,
    // which stay on disc: let telemetry find the map file through the VFS.
    // Which map exactly (docs/game/maps.md §5): a package's MapId, None for the
    // disc's own map. Games and advertisements carry it as `map_id`.
    if let (Some(t), Some(vfs)) = (&_telemetry, k.vfs()) {
        t.set_map_id_locator(Box::new(move |map| {
            let mut v = vfs.lock().unwrap_or_else(|e| e.into_inner());
            let id = map_override::map_id(&mut v, map)?;
            Some((id, v.content_base_name(map)))
        }));
    }
    // A matchmaking game's record names its playlist and Live session
    // (docs/live/live-matchmaking.md §8.3, §8.4): the server fills in the
    // hopper's title and finds the arbitration round by the session. A
    // clanker hopper's game is a Clankermatch (docs/game/bots.md §8.2).
    if let (Some(t), Some(hle)) = (&_telemetry, &_live) {
        let hle = Arc::clone(hle);
        t.set_playlist_locator(Box::new(move || {
            hle.matchmade().map(|(id, session)| telemetry::Playlist {
                name: None,
                id: id.to_string(),
                session_id: Some(session),
                clankers: hle.clanker_hoppers().contains(id),
            })
        }));
    }
    if let (Some(hle), Some(vfs)) = (&_live, k.vfs()) {
        hle.set_map_probe(Some(Arc::new(move |g: &runtime::Guest| {
            let mut v = vfs.lock().unwrap_or_else(|e| e.into_inner());
            map_override::lobby_map_id(&mut v, g)
        })));
    }
    if let (Some(t), Some(vfs)) = (&_telemetry, k.vfs()) {
        t.set_map_locator(Box::new(move |map| {
            let mut v = vfs.lock().unwrap_or_else(|e| e.into_inner());
            let file = map_override::map_file(&mut v, map)?;
            Some(telemetry::MapFile {
                path: file.path,
                offset: file.offset,
                len: file.len,
            })
        }));
    }
    // Sample the render thread too when profiling.
    #[cfg(feature = "perf")]
    let _ = d3d::RENDER_THREAD_START.set(|| perf::register_current_thread("render"));
    // In-between frames at the display rate (docs/game/high-framerate.md,
    // "Phase 2" and "E1"): `impl.render.interpolate` or
    // `impl.render.reproject`, render thread only; in a window, or on the
    // headless GPU backend (`H2_BOOT_GFX`, paced on the wall clock).
    let interp = if opts.d3d {
        d3d_glue::interp_render_thread(
            host.gfx.is_some(),
            host.render_thread,
            std::env::var_os("H2_BOOT_GFX").is_some(),
            d3d_glue::render_thread_requested(),
        )
        .and_then(|render_thread| d3d_glue::interpolation(host.settings.as_ref(), render_thread))
    } else {
        None
    };
    // No window: a headless run, which skips the movies by default.
    let headless = host.gfx.is_none();
    let mut d3d_calls = match (opts.d3d, host.gfx.take()) {
        (false, _) => None,
        (true, Some(b)) => Some(match host.gfx_view.take() {
            Some((view, scale)) => {
                let mut b = d3d::ScaledBackend::new(b, view, scale).map_err(anyhow::Error::msg)?;
                eprintln!("d3d: rendering at {scale}x; the guest sees a 1x reference");
                // The Video screen's Resolution row changes the scale at the
                // next frame boundary (docs/graphics/high-resolution.md).
                // The Lighting row (E3 lighting, docs/graphics/e3-lighting.md)
                // changes the view's shadows there too.
                if let Some(store) = &host.settings {
                    let wanted = store.handle::<u32>(setting_defs::RENDER_SCALE.key);
                    b.follow_scale(move || wanted.get());
                    let lighting = store.handle::<u32>(setting_defs::LIGHTING.key);
                    b.set_lighting(setting_defs::lighting_of(lighting.get()));
                    b.follow_lighting(move || setting_defs::lighting_of(lighting.get()));
                }
                let report = Some(d3d_glue::scale_report(&mut b));
                if host.render_thread {
                    d3d_glue::install_threaded(&loaded.runtime, b, report, interp)?
                } else {
                    d3d_glue::install_with_report(&loaded.runtime, b, report)?
                }
            }
            None if host.render_thread => {
                d3d_glue::install_threaded(&loaded.runtime, b, None, interp)?
            }
            None => d3d_glue::install_with(&loaded.runtime, b)?,
        }),
        (true, None) => Some(d3d_glue::install_headless(&loaded.runtime, interp)?),
    };
    // Vblanks on the 60 Hz logical clock (docs/graphics/d3d.md "Vertical blank"),
    // delivered as DPCs; the frame wait above is the clocked one.
    if let Some(handler) = d3d_calls.as_mut().and_then(|i| i.vblank.take()) {
        k.add_periodic(kernel::periodic::Source {
            name: "vblank",
            rate: kernel::periodic::Rate::HZ_60,
            max_catch_up: 4,
            handler,
        })
        .map_err(|e| anyhow::anyhow!("vblank source: {e:?}"))?;
    }
    if d3d_calls.is_some() && uses.is_some() {
        d3d_glue::install_back_surfaces(&loaded.runtime)?;
    }
    let pad = host.pad_speaker.take();
    let ds = match (opts.dsound, host.audio_sink.take()) {
        (false, _) => None,
        (true, Some(open)) => Some(dsound_glue::install_with_sinks(
            &loaded.runtime,
            &k,
            xbe_bytes,
            open,
            pad,
        )?),
        (true, None) => Some(dsound_glue::install(&loaded.runtime, &k, xbe_bytes)?),
    };
    if let Some(audio) = &ds
        && features.switches().audio_hrtf()
    {
        audio.mixer().lock().unwrap().set_hrtf(true);
    }
    if let Some(input) = host.input.as_mut() {
        // Local bots' pads (docs/game/bots.md): after the host's own.
        input.add_backend(Box::new(bots::local::global().backend()));
    }
    // `input.host` (`h2 agent hostpads`) reads the manager's report after
    // the manager moves into XAPI.
    let host_pads = host.input.as_ref().map(input::InputManager::host_report);
    let (xa, pad) = match (opts.xapi, host.input.take()) {
        (false, _) => (None, None),
        (true, Some(input)) => (
            Some(xapi_glue::register_with(&loaded.runtime, &k, input)?),
            host.keyboard.take(),
        ),
        (true, None) => {
            let (x, pad) = xapi_glue::register(&loaded.runtime, &k)?;
            (Some(x), Some(pad))
        }
    };
    // Voice chat's communicator (docs/live/live-voice.md): off until a
    // microphone is chosen on the Voice screen.
    let mut voice_link = xa.as_ref().and_then(|x| {
        let game_mix = ds.as_ref().map(dsound_glue::Audio::mixer);
        voice_glue::install(
            &loaded.runtime,
            x,
            host.settings.as_ref(),
            host.voice_io.take(),
            game_mix,
        )
    });
    if let Some(keyboard) = pad.as_ref() {
        keyboard_glue::install(&loaded.runtime, keyboard.clone())
            .context("installing host keyboard entry")?;
    }
    let script = xapi_glue::InputScript::from_env();
    // H2_LOCKSTEP / H2_INPUT_RECORD / H2_INPUT_REPLAY (lockstep_glue.rs).
    let lockstep_config = lockstep_glue::Config::from_env();
    let lockstep_on = lockstep_config.enabled();
    // H2_AGENT=<socket>: the agent API (agent_glue.rs, docs/runtime/agent-api.md);
    // the debug menu talks to a local one with no socket.
    let agent_source = match agent_glue::socket_from_env() {
        Some(path) => registration::AgentSource::Socket(path),
        None if host.debug_menu.is_some() => registration::AgentSource::Local,
        None => registration::AgentSource::None,
    };
    let watch_overrides = agent_glue::socket_from_env().is_some();
    let pipeline =
        registration::pipeline_for(d3d_glue::pipeline_render(d3d_calls.as_ref()), lockstep_on);
    // Sim mode skips the render it would pipeline (both implement the frame).
    let pipeline = if sim_mode.is_some() {
        pipeline.disabled("sim mode skips the world render (sim-frame)")
    } else {
        pipeline
    };
    render_probe::set_pipeline(pipeline.clone());
    let coop_probe = coop_probe::parse(|k| std::env::var(k).ok()).map_err(anyhow::Error::msg)?;
    // The native world's map file (native_world_glue.rs): read from the
    // loaded map's file, found through the VFS.
    if let Some(vfs) = k.vfs() {
        native_world_glue::set_locator(Box::new(move |map| {
            let mut v = vfs.lock().unwrap_or_else(|e| e.into_inner());
            let file = map_override::map_file(&mut v, map)?;
            Some(telemetry::MapFile {
                path: file.path,
                offset: file.offset,
                len: file.len,
            })
        }));
    }
    // The lockstep harness, the agent, our cheats, the render features and
    // the probes (registration.rs, the rest of the plan's features).
    let late = registration::register_late(
        &mut features,
        registration::LateInputs {
            rt: &loaded.runtime,
            lockstep: lockstep_config,
            agent: agent_source,
            xa: xa.as_ref(),
            pipeline,
            coop_probe,
            live: _live.clone(),
            settings: host.settings.clone(),
            map_gate: map_gate.clone(),
            disc: &opts.disc,
            data_root: &opts.data_root,
        },
    )?;
    let mut harness = late.harness;
    let agent = late.agent;
    let mut snapshot_probe = late.snapshot_probe;
    // edit.* and render.* on the agent (hot_glue.rs, docs/graphics/hot-iteration.md).
    if let Some(a) = &agent {
        let maps = || {
            k.vfs().map(|vfs| -> hot_glue::MapLocator {
                Box::new(move |map| {
                    let mut v = vfs.lock().unwrap_or_else(|e| e.into_inner());
                    let file = map_override::map_file(&mut v, map)?;
                    Some(telemetry::MapFile {
                        path: file.path,
                        offset: file.offset,
                        len: file.len,
                    })
                })
            })
        };
        hot_glue::install(&a.agent, &loaded.runtime, maps(), watch_overrides);
        // sim.give and sim.spawn find tags as edit.* does (agent_glue.rs).
        if let Some(sim) = &a.sim {
            sim.set_tags(Box::new(hot_glue::map_tags(&loaded.runtime, maps())));
        }
        // world.* results name their tags the same way.
        a.agent.set_tag_names(Box::new(hot_glue::map_tags(&loaded.runtime, maps())));
    }
    // The debug menu's host methods and connection (debug_menu_glue.rs).
    if let Some(a) = &agent {
        debug_menu_glue::install(
            host.debug_menu.as_ref(),
            host_pads,
            &a.agent,
            host.settings.clone(),
            host.perf_overlay.clone(),
        );
    }
    // Sim mode's render skip and metrics, and match.* on the agent.
    if let Some(s) = &sim_mode {
        s.install(&mut features)?;
        if let Some(a) = &agent {
            s.add_methods(&a.agent);
        }
    }
    // Part of a machine snapshot's build key (snapshot_glue.rs).
    let switch_spec = features.switches().spec();
    let plan = features.build(&feature::Base).map_err(|conflicts| {
        let c: Vec<String> = conflicts.iter().map(ToString::to_string).collect();
        anyhow::anyhow!("feature plan refused: {}", c.join("; "))
    })?;
    for line in plan.text().lines() {
        eprintln!("feature plan: {line}");
    }
    let installed = plan
        .install(&loaded.runtime)
        .context("installing the feature plan")?;
    for n in &installed.notes {
        eprintln!("feature plan: {n}");
    }
    if let Some(a) = &agent {
        a.set_plan(&plan, &installed);
        if let Some(why) = installed.off_reason("agent") {
            a.agent.set_no_tick_hook();
            eprintln!("agent: answering between slices only, no pausing: {why}");
        }
    }
    if let Some(store) = host.settings.as_ref().filter(|_| skulls_on) {
        eprintln!("{}", skulls::report(&plan, &installed, store));
    }
    if skulls_mp_on {
        eprintln!("{}", skulls_mp::report(&plan, &installed));
    }
    if skulls_custom_on {
        eprintln!("{}", skulls_custom::report(&plan, &installed));
    }
    if health_on {
        eprintln!("{}", health::report(&plan, &installed));
    }
    if let (Some(h), Some(why)) = (harness.as_mut(), installed.off_reason("lockstep")) {
        eprintln!("lockstep: hashing presents only: {why}");
        h.set_tick_note(why.to_owned());
    }
    let bink = if opts.bink {
        // A lockstep run compares schedules and movie pacing follows the
        // wall clock, so it skips them; so, by default, do level boots and
        // headless runs.
        let why_off =
            movies_glue::why_off(&opts.movies, lockstep_on, level_name.as_deref(), headless);
        let (_, hle) = movies_glue::install(
            &loaded.runtime,
            &opts.movies,
            &opts.disc,
            why_off.as_deref(),
            ds.as_ref().map(dsound_glue::Audio::mixer),
        );
        Some(hle)
    } else {
        eprintln!("movies: off (--no-bink: the original player runs)");
        None
    };
    let tr = trace::install_bounded(
        &loaded.runtime,
        // Sim mode writes no kernel trace file (docs/runtime/sim.md §2).
        if sim_mode.is_some() { None } else { opts.trace.as_deref() },
        host.trace_limit.unwrap_or(usize::MAX),
    )?;
    let mut m = kernel::Machine::new(
        Arc::clone(&loaded.runtime),
        Arc::clone(&k),
        loaded.cpu,
        loaded.stack_bottom,
        loaded.stack_top,
    )
    .context("creating the entry thread")?;
    m.max_transfers_per_slice = opts.transfers_per_slice;
    // H2_BOOT_REALTIME: hold the host back to real time (wall pacing); the
    // schedule is unchanged.
    if std::env::var_os("H2_BOOT_REALTIME").is_some() {
        m.pacer = Some(kernel::pacer::Pacer::real_time());
    }
    // Sim mode: the match's pace (unpaced by default).
    if let Some(s) = &sim_mode {
        m.pacer = s.pacer();
    }
    eprintln!("entry {:#010x}, running", loaded.cpu.eip);
    let t = Instant::now();
    // One slice at a time so XAPI's input pump runs between slices
    // (docs/runtime/xapi.md "Input pump"); no guard is held here.
    let mut outcome = kernel::RunOutcome::SliceLimit;
    let mut host_stop = false;
    let mut input_errors = 0u64;
    let mut audio_errors: Vec<String> = Vec::new();
    // Logical time added for pending audio alone since a thread last ran.
    let mut audio_idle = 0u64;
    // H2_AUDIO_STATS=1: a line of audio health every few seconds.
    let mut audio_report = ds
        .as_ref()
        .and_then(|_| dsound_glue::AudioReport::from_env());
    // Machine snapshots (snapshot_glue.rs, docs/runtime/snapshots.md):
    // what holds state outside guest RAM, for `state.*` and the probe.
    let snap_host = snapshot_glue::Host {
        rt: &loaded.runtime,
        d3d: d3d_calls.as_ref().map(|i| &*i.snapshot),
        audio: ds.as_ref(),
        xapi: xa.as_ref(),
        bink: bink.as_deref(),
        live: _live.as_deref(),
    };
    let mut rebuilders = kernel::snapshot::Rebuilders::new();
    rebuilders.add(trace::KERNEL_CALL_RETURN, trace::rebuilder(&tr));
    let shelf = agent.as_ref().and_then(|a| {
        // The running tier's own stamp: a fast-tier snapshot is not a clang one's.
        let stamp = match tier {
            tier::Tier::Clang => build_stamp(xbe_bytes),
            tier::Tier::Fast => tier::fast_stamp(xbe_bytes),
        };
        let key = snapshot_glue::build_key(stamp, &switch_spec, &plan, &snap_host.names());
        let shelf = snapshot_glue::Shelf::from_env(key)?;
        a.agent.set_snapshots(Box::new(shelf.clone()));
        Some(shelf)
    });
    // H2_BOOT_STOP_AT_PRESENT=N: end the run once N presents are out.
    let stop_at: Option<u64> = std::env::var("H2_BOOT_STOP_AT_PRESENT")
        .ok()
        .and_then(|v| v.parse().ok());
    // H2_HOST_PERF=1 (the Xbox app's default): a line every 100 frames or
    // 5 s of where frame time goes on this host (host_perf.rs); it starts
    // frame stats quietly if nothing else has.
    let host_perf =
        host_perf::start_from_env(host.render_thread || d3d_glue::render_thread_requested());
    // H2_FRAME_STATS=1: frame times split by phase (docs/perf/gameplay-budget.md).
    // `h2 play --frame-stats` may have started them already.
    let frame_stats = timeline::phase::enabled() || timeline::phase::start_from_env();
    let gpu = !headless || std::env::var_os("H2_BOOT_GFX").is_some();
    if let Some(note) = d3d_glue::frame_stats_gpu_note(frame_stats && d3d_calls.is_some(), gpu) {
        eprintln!("{note}");
    }
    // H2_EFFECT_CENSUS=1: a line per new timed light (EXPLOSION for a
    // grenade-sized one) and an exit summary (effect_census.rs).
    let mut effect_census =
        effect_census::requested_by(std::env::var(effect_census::ENV).ok().as_deref()).then(|| {
            eprintln!(
                "effects: census on ({}): a line per new timed light, a summary at exit",
                effect_census::ENV
            );
            (effect_census::Census::new(), u64::MAX)
        });
    // H2_PERF_FROM=N (cli built with `--features perf`): open
    // `perf::Session::from_env()` (H2_PROFILE, H2_TIMELINE) at present N;
    // H2_PERF_FROM=level at the first present after the level loaded
    // (`level_boot::loaded_at`). It is finished below, before the module is
    // dropped.
    if let Some(note) = perf_env_note(
        cfg!(feature = "perf"),
        perf_from.is_some(),
        ["H2_TIMELINE", "H2_PROFILE"]
            .iter()
            .any(|k| std::env::var_os(k).is_some_and(|v| !v.is_empty())),
    ) {
        eprintln!("{note}");
    }
    #[cfg(feature = "perf")]
    let mut perf_from = perf_from;
    #[cfg(not(feature = "perf"))]
    let _ = &level_name;
    #[cfg(feature = "perf")]
    let mut perf_session: Option<perf::Session> = None;
    // The sampler's main target (agent glue sets it too).
    #[cfg(feature = "perf")]
    if sampling::seconds_from_env().is_some() {
        sampling::set_game_thread();
    }
    for slice in 0..opts.max_slices {
        if stop_at.is_some_and(|n| d3d_diag::presents() >= n) {
            break;
        }
        #[cfg(feature = "perf")]
        if let Some(from) = perf_from
            && from.due(d3d_diag::presents(), level_boot::loaded_at())
        {
            perf_from = None;
            let n = d3d_diag::presents();
            match (from, &level_name) {
                (level_boot::PerfFrom::Level, Some(l)) => {
                    eprintln!("perf: session from present {n}, the first after {l} loaded")
                }
                (level_boot::PerfFrom::Level, None) => {
                    eprintln!("perf: session from present {n}, the first after the level loaded")
                }
                _ => eprintln!("perf: session from present {n}"),
            }
            perf_session = perf::Session::from_env();
            // H2_SAMPLE_SECONDS: the in-process sampler's dump instead
            // (sampling.rs; for the Xbox app, which can't symbolise).
            if let Some(s) = sampling::seconds_from_env() {
                sampling::start_timed(s);
            }
        }
        #[cfg(feature = "perf")]
        sampling::tick();
        if let Some(pad) = &pad {
            script.step(slice, pad);
        }
        let ran_before = dsound_glue::guest_slices(&m);
        outcome = m.run(1).context("machine run")?;
        // Every thread waits and nothing is timed: pending audio is a wake
        // source the scheduler does not know about (dsound_glue::wake_idle).
        if matches!(outcome, kernel::RunOutcome::Deadlock { .. })
            && let Some(a) = &ds
        {
            match dsound_glue::wake_idle(a, &k, &loaded.runtime, &mut audio_idle) {
                Ok(true) => continue,
                Ok(false) => {}
                Err(e) => audio_errors.push(format!("slice {slice}: {e}")),
            }
        }
        if !matches!(outcome, kernel::RunOutcome::SliceLimit) {
            break;
        }
        // Only a guest thread running refills the audio-only budget, not a
        // slice that just delivered vblank.
        if dsound_glue::ran_guest_thread(&m, ran_before) {
            audio_idle = 0;
        }
        if let Some(link) = voice_link.as_mut() {
            link.pump();
        }
        if let Some(x) = &xa
            && timeline::host::INPUT
                .time(|| {
                    xapi::service_input(
                        &loaded.runtime,
                        x,
                        &mut xapi_glue::KernelEvents(Arc::clone(&k)),
                    )
                })
                .is_err()
        {
            input_errors += 1;
        }
        if let Some((census, last)) = &mut effect_census {
            let present = d3d_diag::presents();
            if present != *last {
                *last = present;
                let mem = loaded.runtime.memory();
                for line in census.at_present(&*mem, present) {
                    eprintln!("{line}");
                }
            }
        }
        if let Some(h) = &harness {
            h.after_slice(&loaded.runtime, d3d_diag::presents());
        }
        if let Some(a) = &agent {
            a.after_slice(&loaded.runtime, d3d_diag::presents());
        }
        // A `state.save` or `state.load` is due: run it here, between
        // slices (the agent's tick observer ended the slice for it).
        if let (Some(a), Some(shelf)) = (&agent, &shelf)
            && a.agent.snapshot_pending()
        {
            let mut machine =
                snap_host.machine(&mut m, &plan, &rebuilders, shelf, d3d_diag::presents());
            a.agent.run_snapshots(&mut machine);
        }
        if let Some(p) = &mut snapshot_probe {
            p.after_slice(&m, &plan, || snap_host.blockers());
        }
        if let Some(a) = &ds
            && let Err(e) = dsound_glue::advance(a, &k)
            && audio_errors.len() < 8
        {
            audio_errors.push(format!("slice {slice}: {e}"));
        }
        if let (Some(a), Some(r)) = (&ds, &mut audio_report) {
            r.tick(a);
        }
        if let Some(f) = &mut host.between_slices
            && !f(&k)
        {
            host_stop = true;
            break;
        }
        // Sim mode: the match is over (its end, its limits, match.stop).
        if let Some(s) = &sim_mode
            && !s.keep_running()
        {
            host_stop = true;
            break;
        }
    }
    let sim_upload_result = match &sim_mode {
        Some(s) => match s.finish() {
            Ok(report) if uploading_sim => serde_json::to_value(report)
                .map_err(anyhow::Error::from)
                .and_then(|report| {
                    _telemetry
                        .as_ref()
                        .expect("upload telemetry installed")
                        .finish_sim(report)
                        .map_err(anyhow::Error::msg)
                }),
            Err(error) if uploading_sim => Err(error),
            Err(error) => {
                eprintln!("sim: {error:#}");
                Ok(())
            }
            Ok(_) => Ok(()),
        },
        None => Ok(()),
    };
    if let Some(a) = &ds {
        a.output.halt();
    }
    if let Some(p) = &snapshot_probe {
        p.finish();
    }
    // H2_PGO=gen: the run is over (a present count, the slice limit, the
    // host); the training profile is complete.
    module.write_profile();
    // Fail what's queued and close the socket before the module goes.
    drop(agent);
    if let Some(h) = &harness {
        match h.finish() {
            Ok(line) => eprintln!("{line}"),
            Err(e) => eprintln!("lockstep: {e:#}"),
        }
    }
    let secs = t.elapsed().as_secs_f64();
    #[cfg(feature = "perf")]
    drop(perf_session.take());
    if host_perf {
        host_perf::stop();
    }
    if frame_stats && let Some(report) = timeline::phase::stop() {
        eprint!("{report}");
    }
    let (stop, eip) = match &outcome {
        kernel::RunOutcome::Stopped { thread, transfer } => {
            let th = m.thread(*thread).context("stopped thread")?;
            (
                format!(
                    "thread {thread}: {}",
                    describe(th.ctx.event(transfer.event_id), *thread)
                ),
                th.cpu.eip,
            )
        }
        o => (
            format!("{o:?}"),
            m.threads().next().map_or(0, |t| t.cpu.eip),
        ),
    };
    eprintln!(
        "ran {secs:.2}s, {} slices, {} preemptions",
        m.schedule().len(),
        m.preemptions()
    );
    let fps_from = std::env::var("H2_BOOT_FPS_FROM")
        .ok()
        .and_then(|v| v.parse().ok());
    if let Some(fps) = d3d_diag::frame_rate(t, 100, fps_from) {
        eprintln!("{fps}");
    }
    for th in m.threads() {
        let c = &th.cpu;
        eprintln!(
            "thread {}: eip {:#010x} eax {:#x} ecx {:#x} edx {:#x} ebx {:#x} esp {:#x} ebp {:#x} esi {:#x} edi {:#x}",
            th.id, c.eip, c.eax, c.ecx, c.edx, c.ebx, c.esp, c.ebp, c.esi, c.edi
        );
        // EBP chain and raw stack words, for finding the caller path.
        {
            let mem = loaded.runtime.memory();
            let mut bp = c.ebp;
            let mut chain = Vec::new();
            for _ in 0..24 {
                let (Ok(next), Ok(ret)) = (mem.u32(bp), mem.u32(bp.wrapping_add(4))) else {
                    break;
                };
                chain.push(format!("{ret:#x}"));
                if next <= bp {
                    break;
                }
                bp = next;
            }
            eprintln!("  ebp chain (return addresses): {}", chain.join(" <- "));
            let words: Vec<String> = (0..24)
                .filter_map(|i| mem.u32(c.esp.wrapping_add(4 * i)).ok())
                .map(|v| format!("{v:x}"))
                .collect();
            eprintln!("  stack: {}", words.join(" "));
        }
        let tail: Vec<String> = th
            .ctx
            .events()
            .iter()
            .rev()
            .take(6)
            .map(|e| format!("{e:x?}").chars().take(300).collect())
            .collect();
        for e in tail.iter().rev() {
            eprintln!("  event: {e}");
        }
    }
    if let Some(u) = &uses {
        for (o, (name, n)) in u.lock().unwrap().iter() {
            eprintln!("stopgap used: #{o} {name} x{n}");
        }
    }
    trace::flush(&tr);
    if std::env::var_os("H2_BOOT_MAPS").is_some() {
        let mem = loaded.runtime.memory();
        let mut ms: Vec<_> = mem.mappings().to_vec();
        ms.sort_by_key(|m| m.physical_start);
        for m in ms {
            eprintln!(
                "map phys {:#010x}..{:#010x} va {:#010x} {:?}",
                m.physical_start,
                m.physical_start as u64 + m.len as u64,
                m.virtual_start,
                m.permissions
            );
        }
    }
    if let Some(h) = &ds {
        eprintln!("{}", dsound_glue::describe(h));
        if !audio_errors.is_empty() {
            eprintln!("dsound advance errors: {audio_errors:?}");
        }
    }
    if xa.is_some() {
        eprintln!("xapi: registered; input pump errors {input_errors}");
    }
    if let Some(i) = &d3d_calls {
        eprintln!("{}", d3d_glue::describe(i));
    }
    if let Some((census, _)) = &effect_census {
        eprintln!("{}", census.summary());
    }
    if let Some(s) = fallback::STATS.lock().unwrap().as_ref() {
        let first: Vec<String> = s.first.iter().map(|f| format!("{:#x}", f.0)).collect();
        eprintln!(
            "fallback: {} entries, {} instructions interpreted, {} x87 emulated; x87 mix {:?}; from [{}]",
            s.fallbacks,
            s.interpreted,
            s.x87,
            s.x87_mnemonics,
            first.join(" ")
        );
    }
    {
        let t = tr.lock().unwrap();
        eprintln!("{} kernel calls; last 40:", t.total);
        for c in t
            .calls
            .iter()
            .rev()
            .take(40)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            let args: Vec<String> = c.args.iter().map(|a| format!("{a:x}")).collect();
            eprintln!(
                "  #{:<5} t{} {:>3} {:<30} ret {:#08x} [{}] -> {} {}",
                c.seq,
                c.thread,
                c.ordinal,
                c.name,
                c.ret_pc,
                args.join(" "),
                c.result.map_or("(open)".into(), |r| format!("{r:#x}")),
                c.detail
            );
        }
    }
    // H2_BOOT_DUMP=addr:len[,addr:len...]: guest words at the end of the run.
    if let Ok(spec) = std::env::var("H2_BOOT_DUMP") {
        let mem = loaded.runtime.memory();
        for item in spec.split(',') {
            let Some((a, n)) = item.split_once(':') else {
                continue;
            };
            let parse = |v: &str| u32::from_str_radix(v.trim().trim_start_matches("0x"), 16).ok();
            let (Some(a), Some(n)) = (parse(a), parse(n)) else {
                continue;
            };
            let words: Vec<String> = (0..n.div_ceil(4))
                .map(|i| {
                    mem.u32(a + 4 * i)
                        .map_or("????????".into(), |w| format!("{w:08x}"))
                })
                .collect();
            for (i, row) in words.chunks(8).enumerate() {
                eprintln!("dump {:#010x}: {}", a + 32 * i as u32, row.join(" "));
            }
        }
    }
    // H2_BOOT_DUMP_RAW=addr:len:file[;...]: raw guest bytes to host files.
    if let Ok(spec) = std::env::var("H2_BOOT_DUMP_RAW") {
        let mem = loaded.runtime.memory();
        for item in spec.split(';') {
            let mut it = item.splitn(3, ':');
            let parse = |v: Option<&str>| {
                v.and_then(|v| u32::from_str_radix(v.trim().trim_start_matches("0x"), 16).ok())
            };
            let (Some(a), Some(n), Some(path)) = (parse(it.next()), parse(it.next()), it.next())
            else {
                continue;
            };
            let bytes: Vec<u8> = (0..n).map(|i| mem.u8(a + i).unwrap_or(0)).collect();
            if let Err(e) = std::fs::write(path, bytes) {
                eprintln!("dump {a:#x}: {e}");
            }
        }
    }
    let stops: BTreeMap<u32, &str> = k.stop_reasons().into_iter().collect();
    for (o, r) in &stops {
        eprintln!("kernel stop reason #{o}: {r}");
    }
    let outcome = if host_stop {
        "HostStop".to_string()
    } else {
        format!("{outcome:x?}")
    };
    let report = RunReport {
        outcome,
        stop,
        eip,
        kernel_log: k.log(),
    };
    drop(m);
    drop(loaded);
    drop(module);
    sim_upload_result.context("uploading the simulation to Live")?;
    Ok(report)
}

/// Shared note sink (stderr when `H2_BOOT_VERBOSE` is set).
pub mod log {
    pub fn note(s: &str) {
        if std::env::var_os("H2_BOOT_VERBOSE").is_some() {
            eprintln!("note: {s}");
        }
    }
}

/// Collected shim diagnostics.
pub(crate) static SHIM_EVENTS: Mutex<Vec<(u32, u32, String)>> = Mutex::new(Vec::new());

#[cfg(test)]
mod string_ids;
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perf_env_note_names_what_writes_nothing() {
        assert!(
            perf_env_note(false, false, true)
                .unwrap()
                .contains("--features perf")
        );
        assert!(
            perf_env_note(false, true, false)
                .unwrap()
                .contains("--features perf")
        );
        assert!(
            perf_env_note(true, false, true)
                .unwrap()
                .contains("H2_PERF_FROM")
        );
        assert_eq!(perf_env_note(true, true, true), None);
        assert_eq!(perf_env_note(false, false, false), None);
        assert_eq!(perf_env_note(true, false, false), None);
    }

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
        let accounting = format!(
            "{:?}",
            emit::Codegen {
                accounting: on("H2_ACCESS_COUNT"),
                reg_homes: on("H2_REG_HOMES")
            }
        );
        assert!(stamp.contains(&format!("codegen {accounting} ")), "{stamp}");
    }

    #[test]
    fn passes_and_codegen_are_part_of_the_dylib_stamp() {
        let flags = vec!["-O1".to_owned()];
        let off = format!("{:?}", emit::Codegen::default());
        let on = format!(
            "{:?}",
            emit::Codegen {
                accounting: true,
                ..emit::Codegen::default()
            }
        );
        let all_locals = format!(
            "{:?}",
            emit::Codegen {
                reg_homes: false,
                ..emit::Codegen::default()
            }
        );
        let base = stamp_with(b"synthetic", "all", &off, &flags);
        assert_ne!(
            base,
            stamp_with(b"synthetic", "all", &on, &flags),
            "H2_ACCESS_COUNT"
        );
        assert_ne!(
            base,
            stamp_with(b"synthetic", "all", &all_locals, &flags),
            "H2_REG_HOMES"
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
