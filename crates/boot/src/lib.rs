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

pub mod d3d_glue;
pub mod dsound_glue;
pub mod fallback;
pub mod game_glue;
pub mod sha;
pub mod shim;
pub mod sse;
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

/// The bridge source linked into the generated dylib.
pub const BRIDGE: &str = include_str!("../c/boot_bridge.c");
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

/// Lift, emit and compile the image into `out/libh2game.dylib`.
pub fn build(xbe_bytes: &[u8], out: &Path, jobs: usize) -> Result<PathBuf> {
    let x = xbe::Xbe::parse(xbe_bytes)?;
    let t = Instant::now();
    let a = analyze::analyze(&x)?;
    eprintln!("analyze: {:.1}s, {} functions", t.elapsed().as_secs_f64(), a.functions.len());
    let t = Instant::now();
    let lifter = lift::Lifter::new(&a, &x)?;
    let mut funcs = Vec::new();
    let mut unlifted = Vec::new();
    for &e in a.functions.keys() {
        match lifter.lift(e) {
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
    let owners = emit::plan(funcs);
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
    emit::compile::install_header(&src, Path::new(H2ABI_H)).map_err(anyhow::Error::msg)?;
    let arch = emit::compile::Arch::host();
    let b = emit::compile::build(&src, out, arch, jobs).map_err(anyhow::Error::msg)?;
    eprintln!(
        "clang ({}): {} compiled, {} reused, {:.1}s, {:.1} MB of objects",
        arch.name(),
        b.compiled,
        b.reused,
        b.compile_secs,
        b.object_bytes as f64 / 1e6
    );
    let lib = out.join(DYLIB);
    let t = Instant::now();
    emit::compile::host_dylib(&b, &src, &[("h2boot_bridge.c", BRIDGE)], &lib).map_err(anyhow::Error::msg)?;
    eprintln!("link: {:.1}s -> {}", t.elapsed().as_secs_f64(), lib.display());
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
        let install: libloading::Symbol<unsafe extern "C" fn(*const shim::Bridge)> = lib.get(b"h2boot_install")?;
        install(&shim::bridge());
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
        if conflicts != 0 {
            eprintln!("warning: {conflicts} selectors map to more than one handler");
        }
        // Every row goes through the fallback trampoline (fallback.rs).
        rt.edit_dispatch(|d| -> Result<()> {
            for &(addr, _) in &rows {
                d.register_generated(addr, runtime::cpu::ABI_VERSION, fallback::trampoline)
                    .with_context(|| format!("entry {addr:#x}"))?;
            }
            for r in resumes {
                d.register_resume(r.pc, r.owner, r.start).with_context(|| format!("resume {:#x}", r.pc))?;
            }
            Ok(())
        })?;
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

/// Map the image, attach the kernel, register the generated module and run
/// the entry thread.
pub fn run(xbe_bytes: &[u8], dylib: &Path, opts: &RunOpts) -> Result<RunReport> {
    let x = xbe::Xbe::parse(xbe_bytes)?;
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
    // SAFETY: the dylib is ours and stays loaded until the end of this fn.
    let module = unsafe { load_module(dylib, &loaded.runtime, xbe_bytes)? };
    let unread = fallback::install_unread_targets(&loaded.runtime);
    crate::log::note(&format!("fallback: {unread} unread jump-table targets registered"));
    eprintln!("registered {} entries, {} resumes in {:.1}s", module.entries, module.resumes, t.elapsed().as_secs_f64());
    let k = kernel::Kernel::for_image(&mut loaded).context("attaching the kernel")?;
    std::fs::create_dir_all(&opts.data_root)?;
    let cfg = vfs::Config::new(&opts.data_root, x.certificate.title_id);
    let v = match vfs::Vfs::with_disc(cfg.clone(), &opts.disc) {
        Ok(v) => v,
        Err(e) => {
            // A raw XBE (or a file that is not an XISO) still boots, with no
            // DVD mounted: the first map open then fails in the guest.
            eprintln!("warning: {} is not a mountable XISO ({e:?}); running with no DVD", opts.disc.display());
            vfs::Vfs::with_storage(cfg).map_err(|e| anyhow::anyhow!("vfs: {e:?}"))?
        }
    };
    k.set_vfs(v).context("attaching the VFS")?;
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
        game_glue::install_stream_purge(&loaded.runtime, u);
    }
    let d3d_calls = if opts.d3d { Some(d3d_glue::install(&loaded.runtime)?) } else { None };
    if let (Some(_), Some(u)) = (&d3d_calls, &uses) {
        d3d_glue::install_stopgaps(&loaded.runtime, u);
        d3d_glue::install_render_target_fast(&loaded.runtime, u);
        d3d_glue::install_gpu_registers(&loaded.runtime)?;
    }
    let ds = if opts.dsound { Some(dsound_glue::install(&loaded.runtime, &k)?) } else { None };
    let xa = if opts.xapi { Some(xapi_glue::register(&loaded.runtime, &k)?) } else { None };
    if opts.bink {
        let hle = bink_hle::BinkHle::new(None);
        loaded.runtime.edit_dispatch(|d| bink_hle::register(d, &hle));
    }
    let tr = trace::install(&loaded.runtime, opts.trace.as_deref())?;
    let mut m = kernel::Machine::new(
        Arc::clone(&loaded.runtime),
        Arc::clone(&k),
        loaded.cpu,
        loaded.stack_bottom,
        loaded.stack_top,
    )
    .context("creating the entry thread")?;
    m.max_transfers_per_slice = opts.transfers_per_slice;
    eprintln!("entry {:#010x}, running", loaded.cpu.eip);
    let t = Instant::now();
    // One slice at a time so XAPI's input pump runs between slices
    // (docs/xapi.md "Input pump"); no guard is held here.
    let mut outcome = kernel::RunOutcome::SliceLimit;
    let mut input_errors = 0u64;
    for _ in 0..opts.max_slices {
        outcome = m.run(1).context("machine run")?;
        if !matches!(outcome, kernel::RunOutcome::SliceLimit) {
            break;
        }
        if let Some(x) = &xa
            && xapi::service_input(&loaded.runtime, x, &mut xapi_glue::KernelEvents(Arc::clone(&k))).is_err()
        {
            input_errors += 1;
        }
    }
    let secs = t.elapsed().as_secs_f64();
    let (stop, eip) = match &outcome {
        kernel::RunOutcome::Stopped { thread, transfer } => {
            let th = m.thread(*thread).context("stopped thread")?;
            (format!("thread {thread}: {}", describe(th.ctx.event(transfer.event_id), *thread)), th.cpu.eip)
        }
        o => (format!("{o:?}"), m.threads().next().map_or(0, |t| t.cpu.eip)),
    };
    eprintln!("ran {secs:.2}s, {} slices", m.schedule().len());
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
    }
    if xa.is_some() {
        eprintln!("xapi: registered; input pump errors {input_errors}");
    }
    if let Some(i) = &d3d_calls {
        eprintln!("{}", d3d_glue::describe(i));
    }
    if let Some(s) = fallback::STATS.lock().unwrap().as_ref() {
        eprintln!(
            "fallback: {} entries, {} instructions interpreted, {} x87 emulated; x87 mix {:?}",
            s.fallbacks, s.interpreted, s.x87, s.x87_mnemonics
        );
    }
    {
        let t = tr.lock().unwrap();
        eprintln!("{} kernel calls; last 40:", t.calls.len());
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
    let stops: BTreeMap<u32, &str> = k.stop_reasons().into_iter().collect();
    for (o, r) in &stops {
        eprintln!("kernel stop reason #{o}: {r}");
    }
    let report = RunReport { outcome: format!("{outcome:x?}"), stop, eip, kernel_log: k.log() };
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
