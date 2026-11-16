//! Boot glue: the whole XBE through lift, emit and clang into one host
//! dylib, then that dylib registered with the runtime, the kernel HLE
//! attached, and the entry thread run until something stops it.
//!
//! This is a tracer bullet, not the final shape of the engine: the
//! generated code is loaded as a dylib (with `c/boot_bridge.c` forwarding
//! its ABI imports to the runtime) so the host binary needs no link-time
//! knowledge of the generated symbols. See docs/BOOT.md.
//!
//! Nothing derived from the disc is written outside the caller's output
//! directory, which should be under `$TMPDIR` or `target/`.

pub mod community_variants;
pub mod custom_games;
pub mod d3d_diag;
pub mod d3d_glue;
pub mod dsound_glue;
pub mod fallback;
pub mod fov_row;
pub mod game_glue;
pub mod icons;
pub mod layout_glue;
pub mod level_boot;
pub mod keyboard_glue;
pub mod live_glue;
pub mod live_notice;
pub mod lockstep_glue;
pub mod map_override;
pub mod pad_speaker;
pub use live_telemetry as telemetry;
pub mod sha;
pub mod shim;
pub mod speaker_row;
pub mod setting_defs;
pub mod skulls;
pub mod skulls_menu;
pub mod speaker_setting;
pub mod ui;
pub mod video_screen;
pub mod sse;
pub mod sweep_diag;
pub mod stopgaps;
pub mod trace;
pub mod wrappers;
pub mod x87;
pub mod xapi_glue;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use runtime::{Event, GuestFn};
pub use live_hle::LiveConfig;

/// The bridge source linked into the generated dylib.
pub const BRIDGE: &str = include_str!("../c/boot_bridge.c");
/// The `h2x87_v1_*` forwarders generated code imports since x87 lowering:
/// emit's own x87 bridge, installed with `h2x87bridge_install`.
pub const BRIDGE_X87: &str = concat!("#include \"h2gen.h\"\n", include_str!("../../emit/c/bridge_x87.c"));
/// The runtime ABI header, copied next to the generated sources.
pub const H2ABI_H: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../runtime/include/h2abi.h");
/// File name of the linked module inside the build directory.
pub const DYLIB: &str = "libh2game.dylib";

/// Read `default.xbe` from an XISO, or a raw XBE.
pub fn read_xbe(path: &Path) -> Result<Vec<u8>> {
    use std::io::Read;
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
/// choices (`H2_ACCESS_COUNT`), the module ABI and the compiler flags.
/// [`build`] writes it next to the dylib
/// (`<dylib>.stamp`) once linked; [`run`] refuses a dylib whose stamp
/// differs, and `h2 run` rebuilds instead.
pub fn build_stamp(xbe_bytes: &[u8]) -> String {
    match emit::compile::cflags() {
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
    format!(
        "xbe {:016x} generator {} passes {passes} codegen {codegen} abi {} flags {flags:?}\n",
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
    dylib.exists() && std::fs::read_to_string(stamp_path(dylib)).is_ok_and(|s| s == build_stamp(xbe_bytes))
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
    Some(analysis_cache_in(xbe_bytes, &fallback::boot_cache_dir()?, env!("ANALYSIS_SOURCE_HASH")))
}

fn analysis_cache_in(xbe_bytes: &[u8], dir: &Path, analyzer: &str) -> PathBuf {
    dir.join(format!("analysis-{:016x}-{:x}-{analyzer}.h2a", fallback::fnv64(xbe_bytes), xbe_bytes.len()))
}

/// Discovery's result for the image: read from `cache` when an earlier
/// build by the same analyzer left it there, otherwise computed and saved
/// there. With no `cache`, always computed. Returns whether the cache
/// served it.
pub(crate) fn analyze_cached(x: &xbe::Xbe, cache: Option<&Path>) -> Result<(analyze::Analysis, bool)> {
    if let Some(cache) = cache
        && let Ok(bytes) = std::fs::read(cache)
        && let Some(a) = analyze::Analysis::decode(&bytes)
    {
        // Pruning by modification time drops the least recently used.
        let _ = std::fs::File::options().append(true).open(cache).and_then(|f| f.set_modified(std::time::SystemTime::now()));
        return Ok((a, true));
    }
    let a = analyze::analyze(x)?;
    // Best effort: a failed write only costs the next build the analysis.
    if let Some(cache) = cache
        && let Some(dir) = cache.parent()
        && let Err(e) = std::fs::create_dir_all(dir).and_then(|()| emit::output::replace(cache, &a.encode()))
    {
        eprintln!("analyze: not cached ({}: {e})", cache.display());
    }
    Ok((a, false))
}

/// Lift, emit and compile the image into `out/libh2game.dylib`.
pub fn build(xbe_bytes: &[u8], out: &Path, jobs: usize) -> Result<PathBuf> {
    let total = Instant::now();
    let x = xbe::Xbe::parse(xbe_bytes)?;
    // A failed build must not leave an old stamp vouching for a new dylib.
    let _ = std::fs::remove_file(stamp_path(&out.join(DYLIB)));
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
    eprintln!("plan: {:.1}s (IR passes and owner shapes)", t.elapsed().as_secs_f64());
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
    let arch = emit::compile::Arch::host();
    let b = emit::compile::build(&src, out, arch, jobs).map_err(anyhow::Error::msg)?;
    eprintln!(
        "clang ({}): {} compiled, {} reused, {:.1}s keying (dependency scans, hashing, pins), {:.1}s compiling, {:.1}s archive, {:.1} MB of objects",
        arch.name(),
        b.compiled,
        b.reused,
        b.cache_secs,
        b.compile_secs,
        b.archive_secs,
        b.object_bytes as f64 / 1e6
    );
    let lib = out.join(DYLIB);
    let t = Instant::now();
    emit::compile::host_dylib(&b, &src, &[("h2boot_bridge.c", BRIDGE), ("h2boot_bridge_x87.c", BRIDGE_X87)], &lib).map_err(anyhow::Error::msg)?;
    eprintln!("link: {:.1}s -> {}", t.elapsed().as_secs_f64(), lib.display());
    std::fs::write(stamp_path(&lib), build_stamp(xbe_bytes)).with_context(|| format!("stamping {}", lib.display()))?;
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

/// Load the generated dylib, install the bridge, and register every entry
/// and resume with the runtime's dispatch (RUNTIME-CONTRACT.md §3).
///
/// # Safety
/// `path` must be a dylib produced by [`build`] for this host.
pub unsafe fn load_module(path: &Path, rt: &runtime::Runtime, xbe_bytes: &[u8]) -> Result<Module> {
    let lib = unsafe { libloading::Library::new(path) }.with_context(|| format!("loading {}", path.display()))?;
    unsafe {
        let version: libloading::Symbol<unsafe extern "C" fn() -> u32> = lib.get(b"h2gen_module_abi_version")?;
        if version() != runtime::cpu::ABI_VERSION {
            bail!("module ABI {} != runtime {}", version(), runtime::cpu::ABI_VERSION);
        }
        let install: libloading::Symbol<unsafe extern "C" fn(*const shim::Bridge, usize) -> u32> =
            lib.get(b"h2boot_install_v2").context("boot bridge predates optional callbacks; rebuild the generated module")?;
        if install(&shim::bridge(), std::mem::size_of::<shim::Bridge>()) != 1 {
            bail!("boot bridge layout mismatch; rebuild the generated module");
        }
        let install_x87: libloading::Symbol<unsafe extern "C" fn(*const *const std::ffi::c_void, u32)> =
            lib.get(b"h2x87bridge_install")?;
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
            let (Some(f), Some(v)) = (e.f, e.abi_version) else { bail!("entry {:#x}: null row", e.addr) };
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
                let f = if trampoline { fallback::trampoline as runtime::GuestFn } else { f };
                d.register_generated(addr, runtime::cpu::ABI_VERSION, f).with_context(|| format!("entry {addr:#x}"))?;
            }
            for r in resumes {
                d.register_resume(r.pc, r.owner, r.start).with_context(|| format!("resume {:#x}", r.pc))?;
            }
            Ok(())
        })?;
        if !trampoline {
            fallback::install_stop_recovery(rt);
        }
        Ok(Module { _lib: lib, entries: entries.len(), resumes: resumes.len() })
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
    /// Register crates/bink-hle in skip mode.
    pub bink: bool,
    /// Register crates/dsound's overrides (headless mixer).
    pub dsound: bool,
}

fn describe(ev: Option<&Event>, thread: u32) -> String {
    match ev {
        Some(Event::KernelCall(k)) => {
            let args: Vec<String> =
                k.stack_args.iter().map(|a| a.as_ref().map_or("?".into(), |v| format!("{v:#x}"))).collect();
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
/// fills these in; see docs/BOOT.md "Hosted runs".
#[derive(Default)]
pub struct Host {
    /// D3D renders here (present hook attached) instead of the headless
    /// backend; `H2_BOOT_GFX` is then ignored.
    pub gfx: Option<d3d::GfxBackend>,
    /// With `gfx`: a second backend on the same device that renders the
    /// frame buffers at this scale and presents, while `gfx` stays at the
    /// guest's size and answers the guest (`d3d::ScaledBackend`,
    /// `H2_RENDER_SCALE`, docs/high-resolution.md). `gfx` then has no
    /// present hook; the view has it. At 1 the view sleeps; with
    /// `settings`, the scale follows the Resolution setting during the run.
    pub gfx_view: Option<(d3d::GfxBackend, u32)>,
    /// Run `gfx`'s work on a render thread (`d3d::ThreadedBackend`,
    /// docs/render-thread.md); off in `Host::default()`. Hosts set it from
    /// [`d3d_glue::render_thread_requested`].
    pub render_thread: bool,
    /// Where the logical-time audio output plays (an audio device), instead
    /// of a `NullSink` (`dsound_glue::install_with_sink`).
    pub audio_sink: Option<dsound_glue::SinkOpener>,
    /// The controller speaker (docs/playstation-audio.md): a second sink
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
    /// Button icons for the device in use (`icons`, docs/button-icons.md).
    /// `None` takes `H2_ICONS`; with that unset too, the fonts and the glyph
    /// lookups are left alone.
    pub icons: Option<icons::IconOptions>,
    /// Keep only this many kernel calls in memory (`trace::install_bounded`).
    pub trace_limit: Option<usize>,
    /// Start this campaign level in place of the main menu (`level_boot`,
    /// docs/level-boot.md). `None` reads `H2_LEVEL` and `H2_DIFFICULTY`.
    pub level: Option<level_boot::Request>,
    /// Called after every slice, input pump and audio advance, with the
    /// kernel (for its logical clock). Returning false ends the run
    /// (outcome `HostStop`).
    pub between_slices: Option<BetweenSlices>,
}

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
pub fn run_with_live(xbe_bytes: &[u8], dylib: &Path, opts: &RunOpts, live: &LiveConfig) -> Result<RunReport> {
    run_hosted_with_live(xbe_bytes, dylib, opts, Host::default(), live)
}

/// [`run_hosted`] with explicit Live client settings. The host/CLI supplies
/// the account path and server base URL; no implicit environment settings.
pub fn run_hosted_with_live(
    xbe_bytes: &[u8], dylib: &Path, opts: &RunOpts, mut host: Host, live: &LiveConfig,
) -> Result<RunReport> {
    let x = xbe::Xbe::parse(xbe_bytes)?;
    if !dylib_current(xbe_bytes, dylib) {
        bail!(
            "{} was not built from this image by this generator (stamp {} wants {:?}); rebuild it with `h2 build`",
            dylib.display(),
            stamp_path(dylib).display(),
            build_stamp(xbe_bytes).trim()
        );
    }
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
        crate::log::note(&format!("fallback: {unread} unread jump-table targets registered"));
    }
    eprintln!("registered {} entries, {} resumes in {:.1}s", module.entries, module.resumes, t.elapsed().as_secs_f64());
    let k = kernel::Kernel::for_image(&mut loaded).context("attaching the kernel")?;
    let _live = live_glue::install(&loaded.runtime, &k, live)?;
    // Features declare their hooks here; the plan is built and installed
    // once, below, after the last of them (docs/feature-engine.md).
    let mut features = feature::Registry::new(feature::Switches::from_env().map_err(anyhow::Error::msg)?);
    let _telemetry = telemetry::TelemetryConfig::from_live(live.enabled, live.accounts_path.as_deref(), live.server_url.as_deref()).and_then(|config| telemetry::install(&mut features, &config)).unwrap_or_else(|error| { eprintln!("live telemetry disabled: {error}"); None });
    // Our text on the cannot-connect dialog when Live is off or its server
    // is unreachable (docs/live.md, "Client HLE: Live not set up").
    live_notice::install(&loaded.runtime, live).context("installing the Live notice")?;
    std::fs::create_dir_all(&opts.data_root)?;
    let cfg = vfs::Config::new(&opts.data_root, x.certificate.title_id);
    if opts.xapi && host.input.is_none() {
        let (input, keyboard) = input::InputManager::with_keyboard_pad(Default::default())?;
        host.input = Some(input);
        host.keyboard = Some(keyboard);
    }
    let icon_options = match host.icons {
        Some(o) => Some(o),
        None => icons::choice_from_env()?.map(|choice| icons::IconOptions { choice, gamepads: false }),
    };
    let icons = icon_options
        .map(|o| Arc::new(icons::Icons::new(o, host.keyboard.as_ref())))
        .filter(|i| i.enabled());
    // Edited maps from H2_MAP_DIR in place of the disc's (docs/storage.md
    // "Map overrides"); unset, the disc is mounted as it always was.
    let maps = map_override::dir();
    let mounted = map_override::mount(cfg.clone(), &opts.disc, icons.as_ref().map(|i| i.font_rewrite()), maps.as_deref());
    if let Some(i) = &icons {
        icons::install(&loaded.runtime, Arc::clone(i))?;
    }
    let v = match mounted {
        Ok(v) => v,
        Err(e) => {
            // A raw XBE (or a file that is not an XISO) still boots, with no
            // DVD mounted: the first map open then fails in the guest.
            eprintln!("warning: {} is not a mountable XISO ({e:?}); running with no DVD", opts.disc.display());
            vfs::Vfs::with_storage(cfg).map_err(|e| anyhow::anyhow!("vfs: {e:?}"))?
        }
    };
    k.set_vfs(v).context("attaching the VFS")?;
    // Frame species and weapon names come from the loaded map's tag names,
    // which stay on disc: let telemetry find the map file through the VFS.
    if let (Some(t), Some(vfs)) = (&_telemetry, k.vfs()) {
        t.set_map_locator(Box::new(move |map| {
            let mut v = vfs.lock().unwrap_or_else(|e| e.into_inner());
            let file = v.host_file(&format!("\\Device\\CdRom0\\maps\\{map}.map"))?;
            Some(telemetry::MapFile { path: file.path, offset: file.offset, len: file.len })
        }));
    }
    let uses = opts.stopgaps.then(|| stopgaps::install(&k, &loaded.runtime));
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
        // (docs/BOOT.md "Retired"); H2_BOOT_OLD_GLUE=1 brings them back.
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
    // Community classics in the built-in variant list (docs/community-gametypes.md).
    community_variants::install(&loaded.runtime).context("installing the community game variants")?;
    // Extra button layouts and the Controller Speaker row on the Controls
    // screen (docs/controller-layouts.md, docs/playstation-audio.md).
    // The Video row on the Settings list opens our Video screen
    // (docs/settings-menu.md).
    let model = match host.settings.clone() {
        Some(store) => Some(Arc::new(
            video_screen::model(store, video_screen::Switches::from_env()).map_err(anyhow::Error::msg)?,
        )),
        None => None,
    };
    layout_glue::install(&loaded.runtime, &mut features, model).context("installing the button layouts")?;
    // The party privacy as the Live session's visibility, and the Live
    // menu's Custom Games row (registered with the layouts above;
    // docs/live-custom-games.md).
    custom_games::install_privacy(&loaded.runtime, _live.as_ref(), host.settings.as_ref())
        .context("installing the Live game visibility")?;
    // A campaign level in place of the main menu (`h2 play --level`,
    // H2_LEVEL) and the level-loaded mark H2_PERF_FROM=level waits on
    // (docs/level-boot.md).
    let level = match host.level.take() {
        Some(l) => Some(l),
        None => level_boot::from_env(&opts.disc).context("H2_LEVEL")?,
    };
    let perf_from_var = std::env::var("H2_PERF_FROM").ok();
    let perf_from = perf_from_var.as_deref().and_then(level_boot::PerfFrom::parse);
    if let Some(v) = perf_from_var.as_deref().filter(|_| perf_from.is_none()) {
        eprintln!("perf: H2_PERF_FROM={v:?} is neither a present number nor `level`; ignored");
    }
    let level_name = level.as_ref().map(|l| l.name.clone());
    level_boot::install(&loaded.runtime, &mut features, level, perf_from == Some(level_boot::PerfFrom::Level))
        .context("installing the level boot")?;
    // Campaign skulls kept per profile: a pickup is remembered and a
    // campaign start turns on the profile's chosen skulls (docs/skulls.md).
    // Headless runs (no settings store) leave skulls as retail. Hooks on
    // the feature plan; the line after the plan installs says where.
    let skulls_on = skulls::install(&mut features, host.settings.clone()).context("installing the skulls")?;
    // Sample the render thread too when profiling.
    #[cfg(feature = "perf")]
    let _ = d3d::RENDER_THREAD_START.set(|| perf::register_current_thread("render"));
    let mut d3d_calls = match (opts.d3d, host.gfx.take()) {
        (false, _) => None,
        (true, Some(b)) => Some(match host.gfx_view.take() {
            Some((view, scale)) => {
                let mut b = d3d::ScaledBackend::new(b, view, scale).map_err(anyhow::Error::msg)?;
                eprintln!("d3d: rendering at {scale}x; the guest sees a 1x reference");
                // The Video screen's Resolution row changes the scale at the
                // next frame boundary (docs/high-resolution.md).
                if let Some(store) = &host.settings {
                    let wanted = store.handle::<u32>(setting_defs::RENDER_SCALE.key);
                    b.follow_scale(move || wanted.get());
                }
                let report = Some(d3d_glue::scale_report(&mut b));
                if host.render_thread {
                    d3d_glue::install_with_report(&loaded.runtime, d3d::ThreadedBackend::new(b)?, report)?
                } else {
                    d3d_glue::install_with_report(&loaded.runtime, b, report)?
                }
            }
            None if host.render_thread => d3d_glue::install_with(&loaded.runtime, d3d::ThreadedBackend::new(b)?)?,
            None => d3d_glue::install_with(&loaded.runtime, b)?,
        }),
        (true, None) => Some(d3d_glue::install(&loaded.runtime)?),
    };
    // Vblanks on the 60 Hz logical clock (docs/d3d.md "Vertical blank"),
    // delivered as DPCs; the frame wait above is the clocked one.
    if let Some(handler) = d3d_calls.as_mut().and_then(|i| i.vblank.take()) {
        k.add_periodic(kernel::periodic::Source { name: "vblank", rate: kernel::periodic::Rate::HZ_60, max_catch_up: 4, handler })
            .map_err(|e| anyhow::anyhow!("vblank source: {e:?}"))?;
    }
    if d3d_calls.is_some() && uses.is_some() {
        d3d_glue::install_back_surfaces(&loaded.runtime)?;
    }
    let pad = host.pad_speaker.take();
    let ds = match (opts.dsound, host.audio_sink.take()) {
        (false, _) => None,
        (true, Some(open)) => Some(dsound_glue::install_with_sinks(&loaded.runtime, &k, xbe_bytes, open, pad)?),
        (true, None) => Some(dsound_glue::install(&loaded.runtime, &k, xbe_bytes)?),
    };
    let (xa, pad) = match (opts.xapi, host.input.take()) {
        (false, _) => (None, None),
        (true, Some(input)) => (Some(xapi_glue::register_with(&loaded.runtime, &k, input)?), host.keyboard.take()),
        (true, None) => {
            let (x, pad) = xapi_glue::register(&loaded.runtime, &k)?;
            (Some(x), Some(pad))
        }
    };
    if let Some(keyboard) = pad.as_ref() {
        keyboard_glue::install(&loaded.runtime, keyboard.clone()).context("installing host keyboard entry")?;
    }
    let script = xapi_glue::InputScript::from_env();
    // H2_LOCKSTEP / H2_INPUT_RECORD / H2_INPUT_REPLAY (lockstep_glue.rs).
    let lockstep_config = lockstep_glue::Config::from_env();
    let mut harness = if lockstep_config.enabled() {
        let h = lockstep_glue::Lockstep::install(&mut features, lockstep_config, lockstep::TickHook::HALO2, xa.as_deref())?;
        if let Some(n) = &h.tick_note {
            eprintln!("lockstep: hashing presents only: {n}");
        }
        Some(h)
    } else {
        None
    };
    // The Rust datum, tag and locale functions, when H2_FEATURES has
    // impl.engine=on (off by default).
    features.add(&feature::leaves::EngineLeaves).map_err(|e| anyhow::anyhow!("engine leaves: {e}"))?;
    let plan = features.build(&feature::Base).map_err(|conflicts| {
        let c: Vec<String> = conflicts.iter().map(ToString::to_string).collect();
        anyhow::anyhow!("feature plan refused: {}", c.join("; "))
    })?;
    for line in plan.text().lines() {
        eprintln!("feature plan: {line}");
    }
    let installed = plan.install(&loaded.runtime).context("installing the feature plan")?;
    for n in &installed.notes {
        eprintln!("feature plan: {n}");
    }
    if let Some(store) = host.settings.as_ref().filter(|_| skulls_on) {
        eprintln!("{}", skulls::report(&plan, &installed, store));
    }
    if let (Some(h), Some(why)) = (harness.as_mut(), installed.off_reason("lockstep")) {
        eprintln!("lockstep: hashing presents only: {why}");
        h.set_tick_note(why.to_owned());
    }
    if opts.bink {
        let hle = bink_hle::BinkHle::new(None);
        loaded.runtime.edit_dispatch(|d| bink_hle::register(d, &hle));
    }
    let tr = trace::install_bounded(&loaded.runtime, opts.trace.as_deref(), host.trace_limit.unwrap_or(usize::MAX))?;
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
    eprintln!("entry {:#010x}, running", loaded.cpu.eip);
    let t = Instant::now();
    // One slice at a time so XAPI's input pump runs between slices
    // (docs/xapi.md "Input pump"); no guard is held here.
    let mut outcome = kernel::RunOutcome::SliceLimit;
    let mut host_stop = false;
    let mut input_errors = 0u64;
    let mut audio_errors: Vec<String> = Vec::new();
    // Logical time added for pending audio alone since a thread last ran.
    let mut audio_idle = 0u64;
    // H2_BOOT_STOP_AT_PRESENT=N: end the run once N presents are out.
    let stop_at: Option<u64> = std::env::var("H2_BOOT_STOP_AT_PRESENT").ok().and_then(|v| v.parse().ok());
    // H2_FRAME_STATS=1: frame times split by phase (docs/frame-budget.md).
    // `h2 play --frame-stats` may have started them already.
    let frame_stats = timeline::phase::enabled() || timeline::phase::start_from_env();
    // H2_PERF_FROM=N (cli built with `--features perf`): open
    // `perf::Session::from_env()` (H2_PROFILE, H2_TIMELINE) at present N;
    // H2_PERF_FROM=level at the first present after the level loaded
    // (`level_boot::loaded_at`). It is finished below, before the module is
    // dropped.
    #[cfg(feature = "perf")]
    let mut perf_from = perf_from;
    #[cfg(not(feature = "perf"))]
    {
        if perf_from.is_some() {
            eprintln!("perf: H2_PERF_FROM needs an h2 built with --features perf; no session");
        }
        let _ = &level_name;
    }
    #[cfg(feature = "perf")]
    let mut perf_session: Option<perf::Session> = None;
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
        }
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
        if dsound_glue::guest_slices(&m) != ran_before {
            audio_idle = 0;
        }
        if let Some(x) = &xa
            && xapi::service_input(&loaded.runtime, x, &mut xapi_glue::KernelEvents(Arc::clone(&k))).is_err()
        {
            input_errors += 1;
        }
        if let Some(h) = &harness {
            h.after_slice(&loaded.runtime, d3d_diag::presents());
        }
        if let Some(a) = &ds
            && let Err(e) = dsound_glue::advance(a, &k)
            && audio_errors.len() < 8
        {
            audio_errors.push(format!("slice {slice}: {e}"));
        }
        if let Some(f) = &mut host.between_slices
            && !f(&k)
        {
            host_stop = true;
            break;
        }
    }
    if let Some(a) = &ds {
        a.output.halt();
    }
    if let Some(h) = &harness {
        match h.finish() {
            Ok(line) => eprintln!("{line}"),
            Err(e) => eprintln!("lockstep: {e:#}"),
        }
    }
    let secs = t.elapsed().as_secs_f64();
    #[cfg(feature = "perf")]
    drop(perf_session.take());
    if frame_stats && let Some(report) = timeline::phase::stop() {
        eprint!("{report}");
    }
    let (stop, eip) = match &outcome {
        kernel::RunOutcome::Stopped { thread, transfer } => {
            let th = m.thread(*thread).context("stopped thread")?;
            (format!("thread {thread}: {}", describe(th.ctx.event(transfer.event_id), *thread)), th.cpu.eip)
        }
        o => (format!("{o:?}"), m.threads().next().map_or(0, |t| t.cpu.eip)),
    };
    eprintln!("ran {secs:.2}s, {} slices, {} preemptions", m.schedule().len(), m.preemptions());
    let fps_from = std::env::var("H2_BOOT_FPS_FROM").ok().and_then(|v| v.parse().ok());
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
                let (Ok(next), Ok(ret)) = (mem.u32(bp), mem.u32(bp.wrapping_add(4))) else { break };
                chain.push(format!("{ret:#x}"));
                if next <= bp {
                    break;
                }
                bp = next;
            }
            eprintln!("  ebp chain (return addresses): {}", chain.join(" <- "));
            let words: Vec<String> =
                (0..24).filter_map(|i| mem.u32(c.esp.wrapping_add(4 * i)).ok()).map(|v| format!("{v:x}")).collect();
            eprintln!("  stack: {}", words.join(" "));
        }
        let tail: Vec<String> =
            th.ctx.events().iter().rev().take(6).map(|e| format!("{e:x?}").chars().take(300).collect()).collect();
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
        for c in t.calls.iter().rev().take(40).collect::<Vec<_>>().into_iter().rev() {
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
            let Some((a, n)) = item.split_once(':') else { continue };
            let parse = |v: &str| u32::from_str_radix(v.trim().trim_start_matches("0x"), 16).ok();
            let (Some(a), Some(n)) = (parse(a), parse(n)) else { continue };
            let words: Vec<String> = (0..n.div_ceil(4))
                .map(|i| mem.u32(a + 4 * i).map_or("????????".into(), |w| format!("{w:08x}")))
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
            let parse = |v: Option<&str>| v.and_then(|v| u32::from_str_radix(v.trim().trim_start_matches("0x"), 16).ok());
            let (Some(a), Some(n), Some(path)) = (parse(it.next()), parse(it.next()), it.next()) else { continue };
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
    let outcome = if host_stop { "HostStop".to_string() } else { format!("{outcome:x?}") };
    let report = RunReport { outcome, stop, eip, kernel_log: k.log() };
    drop(m);
    drop(loaded);
    drop(module);
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
        for (o, flags, va, raw_at, name_at, name) in
            [(0x200, 0x06, 0x11000, 0x1000, 0x280, &b".text\0"[..]), (0x238, 0x02, 0x12000, 0x1100, 0x290, b".rdata\0")]
        {
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
        let dir = std::env::temp_dir().join(format!("h2boot-analysis-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // call 0x11010; ret / 0x11010: ret
        let one = synthetic_xbe(&[0xE8, 0x0B, 0, 0, 0, 0xC3, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xC3]);
        let two = synthetic_xbe(&[0xC3]);
        let x1 = xbe::Xbe::parse(&one).unwrap();
        let x2 = xbe::Xbe::parse(&two).unwrap();
        let fresh = analyze::analyze(&x1).unwrap().encode();
        assert_ne!(fresh, analyze::analyze(&x2).unwrap().encode(), "the two images analyze differently");

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
        let (b, hit) = analyze_cached(&x2, Some(&analysis_cache_in(&two, &dir, "analyzer-a"))).unwrap();
        assert!(!hit);
        assert_eq!(b.encode(), analyze::analyze(&x2).unwrap().encode());
        assert!(!analyze_cached(&x1, Some(&analysis_cache_in(&one, &dir, "analyzer-b"))).unwrap().1);

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
        let p = analysis_cache_in(b"XBEH image", Path::new("/cache"), env!("ANALYSIS_SOURCE_HASH"));
        assert!(p.to_string_lossy().contains(env!("ANALYSIS_SOURCE_HASH")), "{}", p.display());
        if std::env::var_os("H2_TRACE").is_none() && std::env::var_os("H2_BOOT_CACHE").is_none() {
            assert_eq!(analysis_cache(b"XBEH image").unwrap().parent(), Some(&*std::env::temp_dir().join("h2boot-cache")));
        }
    }

    #[test]
    fn generated_flags_are_part_of_the_dylib_stamp() {
        let stamp = |flags: &[String]| stamp_with(b"synthetic", "all", "Codegen { accounting: false }", flags);
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
        for name in ["H2_FASTMEM", "H2_X87_NATIVE", "H2_SSE_NATIVE", "H2_DIRECT_CALLS"] {
            assert_eq!(stamp.contains(&format!("\"-D{name}=1\"")), on(name), "{name}: {stamp}");
        }
        let accounting = format!("{:?}", emit::Codegen { accounting: on("H2_ACCESS_COUNT"), reg_homes: on("H2_REG_HOMES") });
        assert!(stamp.contains(&format!("codegen {accounting} ")), "{stamp}");
    }

    #[test]
    fn passes_and_codegen_are_part_of_the_dylib_stamp() {
        let flags = vec!["-O1".to_owned()];
        let off = format!("{:?}", emit::Codegen::default());
        let on = format!("{:?}", emit::Codegen { accounting: true, ..emit::Codegen::default() });
        let all_locals = format!("{:?}", emit::Codegen { reg_homes: false, ..emit::Codegen::default() });
        let base = stamp_with(b"synthetic", "all", &off, &flags);
        assert_ne!(base, stamp_with(b"synthetic", "all", &on, &flags), "H2_ACCESS_COUNT");
        assert_ne!(base, stamp_with(b"synthetic", "all", &all_locals, &flags), "H2_REG_HOMES");
        assert_ne!(base, stamp_with(b"synthetic", "none", &off, &flags), "H2_PASSES");
        assert_eq!(base, stamp_with(b"synthetic", "all", &off, &flags));
    }

    #[test]
    fn a_dylib_is_current_only_for_its_image_and_generator() {
        let dir = std::env::temp_dir().join(format!("h2boot-stamp-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let lib = dir.join(DYLIB);
        let (a, b) = (b"XBEH image a".as_slice(), b"XBEH image b".as_slice());
        assert!(!dylib_current(a, &lib), "missing");
        std::fs::write(&lib, b"").unwrap();
        assert!(!dylib_current(a, &lib), "no stamp: built before stamps, or the build failed");
        std::fs::write(stamp_path(&lib), build_stamp(a)).unwrap();
        assert!(dylib_current(a, &lib));
        assert!(!dylib_current(b, &lib), "another image");
        std::fs::write(stamp_path(&lib), build_stamp(a).replace(env!("GENERATOR_SOURCE_HASH"), "0000000000000000")).unwrap();
        assert!(!dylib_current(a, &lib), "another generator");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
