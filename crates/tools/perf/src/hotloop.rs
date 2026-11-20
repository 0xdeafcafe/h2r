//! Synthetic guest loops through the whole pipeline: iced-x86 assembles
//! them, `lift` lowers them, `emit` writes C, clang builds a host dylib,
//! and the real runtime runs it. No retail bytes are involved.
//!
//! - `hot-loop/alu`: register arithmetic; the generated code's locals and
//!   the back-edge preemption poll.
//! - `hot-loop/memory`: loads, stores and read-modify-writes through the
//!   checked `h2abi_v1_read`/`write` accessors.
//! - `hot-loop/call`: a call to a separate function per trip, so two
//!   runner transfers (CALL, RETURN) and two dispatches per iteration.
//!
//! Each reports wall nanoseconds per loop trip and, on Unix, thread CPU
//! nanoseconds per trip (excluding time descheduled on a busy host).

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::c_char;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use anyhow::{Context, Result, anyhow, bail};
use iced_x86::code_asm::*;
use iced_x86::{Decoder, DecoderOptions};
use runtime::{
    CpuAbi, Dispatch, Event, ExecCtx, ExecCtxOpaque, H2GenModule, Runner, Runtime,
    guest::Permissions,
};

use crate::bench::{BenchCtx, Better, Measurement, Prepared, Scenario};

pub const CODE: u32 = 0x0040_0000;
pub const DATA: u32 = 0x0001_0000;
pub const STACK_BOTTOM: u32 = 0x6fff_0000;
pub const STACK_TOP: u32 = 0x7000_0000;

/// `alu(n)` (stdcall).
pub const ALU: u32 = CODE + 0x1000;
/// `memory(n)` (stdcall).
pub const MEMORY: u32 = CODE + 0x2000;
/// `call(n)` (stdcall), calling `LEAF` each trip.
pub const CALL: u32 = CODE + 0x3000;
pub const LEAF: u32 = CODE + 0x3800;
/// Independent x87 arithmetic and transport per trip.
pub const X87: u32 = CODE + 0x4000;

/// The runtime's ABI header, written next to the generated sources.
const H2ABI_H: &str = include_str!("../../../runtime/runtime/include/h2abi.h");

/// Assemble at `base`, lower, and split into blocks at every branch target
/// and after every transfer.
pub fn function(
    base: u32,
    body: impl FnOnce(&mut CodeAssembler) -> Result<(), IcedError>,
) -> Result<ir::IrFunction> {
    let mut a = CodeAssembler::new(32)?;
    body(&mut a)?;
    let code = a.assemble(base as u64)?;
    let env = lift::Env::default();
    let insns: Vec<_> = Decoder::with_ip(32, &code, base as u64, DecoderOptions::NONE)
        .into_iter()
        .map(|i| lift::lower(&i, &env, lift::Site::Plain))
        .collect();
    let mut starts = BTreeSet::from([base]);
    for i in &insns {
        match &i.control {
            ir::Control::Next => {}
            ir::Control::Branch {
                taken, fallthrough, ..
            } => starts.extend([taken.pc, fallthrough.pc]),
            _ => {
                starts.insert(i.next_pc());
            }
        }
    }
    let mut blocks: BTreeMap<u32, ir::IrBlock> = BTreeMap::new();
    let mut current = base;
    for i in insns {
        if starts.contains(&i.pc) {
            current = i.pc;
        }
        blocks
            .entry(current)
            .or_insert_with(|| ir::IrBlock {
                start: current,
                insns: vec![],
            })
            .insns
            .push(i);
    }
    let f = ir::IrFunction {
        entry: base,
        blocks,
        interior_entries: vec![],
        errors: vec![],
    };
    f.validate().map_err(|e| anyhow!("{e}\n{f}"))?;
    Ok(f)
}

/// The loops' guest functions.
pub fn guest_functions() -> Result<Vec<ir::IrFunction>> {
    let alu = function(ALU, |a| {
        let mut top = a.create_label();
        a.mov(ecx, dword_ptr(esp + 4))?;
        a.xor(eax, eax)?;
        a.set_label(&mut top)?;
        a.add(eax, ecx)?;
        a.xor(eax, 0x5a5a)?;
        a.rol(eax, 3)?;
        a.dec(ecx)?;
        a.jnz(top)?;
        a.ret_1(4)
    })?;
    let memory = function(MEMORY, |a| {
        let mut top = a.create_label();
        a.mov(ecx, dword_ptr(esp + 4))?;
        a.set_label(&mut top)?;
        a.inc(dword_ptr(DATA))?;
        a.mov(eax, dword_ptr(DATA + 4))?;
        a.add(dword_ptr(DATA + 8), eax)?;
        a.dec(ecx)?;
        a.jnz(top)?;
        a.ret_1(4)
    })?;
    let call = function(CALL, |a| {
        let mut top = a.create_label();
        a.mov(ecx, dword_ptr(esp + 4))?;
        a.set_label(&mut top)?;
        a.push(ecx)?;
        a.call(LEAF as u64)?;
        a.pop(ecx)?;
        a.dec(ecx)?;
        a.jnz(top)?;
        a.ret_1(4)
    })?;
    let leaf = function(LEAF, |a| {
        a.add(eax, 1)?;
        a.ret()
    })?;
    let x87 = function(X87, |a| {
        let mut top = a.create_label();
        a.mov(ecx, dword_ptr(esp + 4))?;
        a.set_label(&mut top)?;
        a.fld(dword_ptr(DATA + 16))?;
        a.fmul(dword_ptr(DATA + 20))?;
        a.fadd(dword_ptr(DATA + 24))?;
        a.fsqrt()?;
        a.fdiv(dword_ptr(DATA + 28))?;
        a.fstp(dword_ptr(DATA + 32))?;
        a.dec(ecx)?;
        a.jnz(top)?;
        a.ret_1(4)
    })?;
    Ok(vec![alu, memory, call, leaf, x87])
}

/// emit's test bridge (`c/bridge.c`) forwards the ABI imports through this
/// table; it points at the runtime's exports.
#[repr(C)]
struct Bridge {
    read: unsafe extern "C" fn(*mut ExecCtxOpaque, u32, u32, u32, u32, *mut u8) -> u32,
    write: unsafe extern "C" fn(*mut ExecCtxOpaque, u32, u32, u32, u32, *const u8) -> u32,
    event: unsafe extern "C" fn(*mut ExecCtxOpaque, u32, u32, u32, u32) -> u32,
    unsupported: unsafe extern "C" fn(*mut ExecCtxOpaque, u32, *const c_char, *const c_char) -> u32,
    rdtsc: unsafe extern "C" fn(*mut ExecCtxOpaque, u32) -> u64,
    rmw: unsafe extern "C" fn(
        *mut CpuAbi,
        *mut ExecCtxOpaque,
        u32,
        u32,
        u32,
        u32,
        u32,
        *mut u32,
    ) -> u32,
    xchg:
        unsafe extern "C" fn(*mut CpuAbi, *mut ExecCtxOpaque, u32, u32, u32, u32, *mut u32) -> u32,
    cmpxchg:
        unsafe extern "C" fn(*mut CpuAbi, *mut ExecCtxOpaque, u32, u32, u32, u32, *mut u32) -> u32,
}

impl Bridge {
    const LAYOUT: [usize; 9] = [
        std::mem::offset_of!(Bridge, read),
        std::mem::offset_of!(Bridge, write),
        std::mem::offset_of!(Bridge, event),
        std::mem::offset_of!(Bridge, unsupported),
        std::mem::offset_of!(Bridge, rdtsc),
        std::mem::offset_of!(Bridge, rmw),
        std::mem::offset_of!(Bridge, xchg),
        std::mem::offset_of!(Bridge, cmpxchg),
        size_of::<Bridge>(),
    ];
}

/// The loops built and loaded, with a runtime they are registered in.
///
/// The runtime's dispatch table holds raw pointers into the dylib, so
/// the runtime never leaves this struct: callers get bounded operations
/// ([`Module::call`], [`Module::read_u32`]), not the `Arc<Runtime>`, and
/// fields drop in order (runtime, then library). Every `ExecCtx` made
/// from the runtime lives inside one `call`.
///
/// ```compile_fail
/// // The runtime field is private: it cannot be cloned out.
/// fn escape(m: &perf::hotloop::Module) {
///     let _ = &m.runtime;
/// }
/// ```
pub struct Module {
    pub dir: PathBuf,
    runtime: Arc<Runtime>,
    lib: libloading::Library,
}

/// Build (or reuse, through emit's object cache) the loops under `work`.
pub fn build(work: &Path) -> Result<Module> {
    build_in(work, &emit::compile::obj_dir(&work.join("hot-loop")))
}

/// [`build`] with emit's objects kept in `objects` (a cache that may be
/// shared with other work directories, like `H2_OBJ_CACHE`).
pub fn build_in(work: &Path, objects: &Path) -> Result<Module> {
    let cc = emit::compile::compiler().map_err(|e| anyhow!("no C compiler: {e}"))?;
    let dir = work.join("hot-loop");
    let src = dir.join("src");
    let owners = emit::plan(guest_functions()?);
    emit::write_module(&owners, &src).with_context(|| format!("writing {}", src.display()))?;
    std::fs::create_dir_all(&dir)?;
    let header = dir.join("h2abi.h");
    std::fs::write(&header, H2ABI_H)?;
    emit::compile::install_header(&src, &header).map_err(|e| anyhow!("{e}"))?;
    let arch = emit::compile::Arch::host();
    let b = emit::compile::build_in(
        &src,
        &dir,
        objects,
        arch,
        emit::compile::default_jobs(),
        &cc,
    )
    .map_err(|e| anyhow!("{e}"))?;
    // A new name per build: rewriting a dylib another Module still has
    // mapped would corrupt that one's code pages.
    static BUILDS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = BUILDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dylib = dir.join(format!(
        "libh2perf_hotloop_{}_{n}.dylib",
        std::process::id()
    ));
    emit::compile::host_dylib(&b, &src, &[("bridge.c", emit::host::BRIDGE)], &dylib)
        .map_err(|e| anyhow!("{e}"))?;
    // SAFETY: built just now by emit for this host.
    let lib = unsafe { libloading::Library::new(&dylib) }?;
    let bridge = Bridge {
        read: runtime::ffi::h2abi_v1_read,
        write: runtime::ffi::h2abi_v1_write,
        event: runtime::generated::h2shim_v1_event,
        unsupported: runtime::generated::h2shim_v1_unsupported,
        rdtsc: runtime::generated::h2shim_v1_rdtsc,
        rmw: runtime::ffi::h2abi_v1_atomic_rmw,
        xchg: runtime::ffi::h2abi_v1_xchg,
        cmpxchg: runtime::ffi::h2abi_v1_cmpxchg,
    };
    let mut memory = runtime::Guest::new();
    memory.map_zeroed(DATA, 0x1_0000, Permissions::RW)?;
    for (i, v) in [1.25f32, 2.5, 0.75, 3.0].into_iter().enumerate() {
        memory.set_u32(DATA + 16 + 4 * i as u32, v.to_bits())?;
    }
    memory.map_zeroed(CODE, 0x1_0000, Permissions::RWX)?;
    // Lifted code, as the loader guards an XBE's: direct calls need it
    // (runtime direct.rs).
    memory.guard_code(CODE, 0x1_0000)?;
    memory.map_zeroed(STACK_BOTTOM, STACK_TOP - STACK_BOTTOM, Permissions::RW)?;
    let rt = Runtime::new(memory, Dispatch::default());
    // SAFETY: bridge.c copies the table after its layout is checked; the
    // signatures match h2abi.h. The module tables follow the current ABI
    // and live as long as `lib`, which `Module` keeps.
    unsafe {
        let layout: libloading::Symbol<unsafe extern "C" fn(u32) -> usize> =
            lib.get(b"h2bridge_layout")?;
        let c: Vec<usize> = (0..Bridge::LAYOUT.len() as u32)
            .map(|i| layout(i))
            .collect();
        if c != Bridge::LAYOUT {
            bail!(
                "bridge.c's H2Bridge layout {c:?} differs from {:?}",
                Bridge::LAYOUT
            );
        }
        let install: libloading::Symbol<unsafe extern "C" fn(*const Bridge)> =
            lib.get(b"h2bridge_install")?;
        install(&bridge);
        let install_x87: libloading::Symbol<
            unsafe extern "C" fn(*const *const std::ffi::c_void, u32),
        > = lib.get(b"h2x87bridge_install")?;
        let x87 = emit::host::x87_helpers();
        install_x87(x87.as_ptr(), x87.len() as u32);
        let module: libloading::Symbol<*const H2GenModule> = lib.get(b"h2gen_module")?;
        let version: libloading::Symbol<unsafe extern "C" fn() -> u32> =
            lib.get(b"h2gen_module_abi_version")?;
        let (module, version) = (&**module, *version);
        rt.edit_dispatch(|d| d.register_generated_module(module, version))
            .map_err(|e| anyhow!("registering the loops: {e:?}"))?;
    }
    Ok(Module {
        dir,
        runtime: rt,
        lib,
    })
}

/// One module per work directory per process: the scenarios share it.
pub fn shared(work: &Path) -> Result<Arc<Module>> {
    static M: OnceLock<Mutex<BTreeMap<PathBuf, Arc<Module>>>> = OnceLock::new();
    let mut m = M
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(x) = m.get(work) {
        return Ok(Arc::clone(x));
    }
    let x = Arc::new(build(work)?);
    m.insert(work.to_path_buf(), Arc::clone(&x));
    Ok(x)
}

impl Module {
    /// A guest word, for checking what a loop wrote.
    pub fn read_u32(&self, addr: u32) -> Result<u32> {
        Ok(self.runtime.memory().u32(addr)?)
    }

    /// Host address of the generated handler for guest function `entry`
    /// (`h2fn_<entry>`), for symbolizer tests.
    pub fn handler_address(&self, entry: u32) -> Option<u64> {
        let name = format!("{}\0", emit::c::symbol(entry));
        // SAFETY: only the symbol's address is taken; nothing is called.
        // (A `Symbol<*const ()>` derefs to the symbol's address itself.)
        unsafe {
            self.lib
                .get::<*const ()>(name.as_bytes())
                .ok()
                .map(|s| *s as u64)
        }
    }

    /// Run stdcall `entry(n)` to its return; the transfers it took.
    pub fn call(&self, entry: u32, n: u32) -> Result<CpuAbi> {
        let mut cpu = CpuAbi {
            eip: entry,
            esp: STACK_TOP - 0x100,
            ..CpuAbi::xbox_thread()
        };
        {
            let mut m = self.runtime.memory();
            m.set_u32(cpu.esp, runtime::exec::HARNESS_EXIT)?;
            m.set_u32(cpu.esp + 4, n)?;
        }
        let mut ctx = ExecCtx::new(Arc::clone(&self.runtime), 1);
        let t = Runner {
            max_transfers: u64::MAX,
        }
        .run(&mut cpu, &mut ctx, entry);
        match ctx.event(t.event_id) {
            Some(Event::HarnessExit) => Ok(cpu),
            e => bail!("{entry:#x}({n}) ended with {t:x?}: {e:x?}"),
        }
    }
}

/// One of the loops as a benchmark.
pub struct HotLoop {
    name: &'static str,
    description: &'static str,
    entry: u32,
    trips: u32,
    module: Option<Arc<Module>>,
}

impl HotLoop {
    pub fn all() -> Vec<HotLoop> {
        let mk = |name, description, entry, trips| HotLoop {
            name,
            description,
            entry,
            trips,
            module: None,
        };
        vec![
            mk(
                "hot-loop/x87",
                "x87 load/mul/add/sqrt/div/store through generated code",
                X87,
                100_000,
            ),
            mk(
                "hot-loop/alu",
                "register arithmetic loop in generated code",
                ALU,
                2_000_000,
            ),
            mk(
                "hot-loop/memory",
                "load/store/RMW loop through the checked memory accessors",
                MEMORY,
                1_000_000,
            ),
            mk(
                "hot-loop/call",
                "call/return per trip: runner transfers and dispatch",
                CALL,
                200_000,
            ),
        ]
    }
}

/// Thread CPU excludes time descheduled; unlike process CPU it also leaves
/// background workers out of these single-threaded synthetic workloads.
fn thread_cpu_ns() -> Option<f64> {
    #[cfg(unix)]
    {
        let mut time = std::mem::MaybeUninit::<libc::timespec>::uninit();
        // SAFETY: clock_gettime initialises the timespec on success.
        if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, time.as_mut_ptr()) } == 0 {
            let time = unsafe { time.assume_init() };
            return Some(time.tv_sec as f64 * 1e9 + time.tv_nsec as f64);
        }
    }
    None
}

impl Scenario for HotLoop {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        self.description
    }
    fn unit(&self) -> &str {
        "ns/trip"
    }
    fn better(&self) -> Better {
        Better::Lower
    }
    fn prepare(&mut self, ctx: &BenchCtx) -> Result<Prepared> {
        if emit::compile::compiler().is_err() {
            return Ok(Prepared::Skip("no clang".into()));
        }
        if ctx.quick {
            self.trips = (self.trips / 100).max(100);
        }
        self.module = Some(shared(&ctx.work)?);
        Ok(Prepared::Ready)
    }
    fn measure(&mut self) -> Result<Measurement> {
        let m = self.module.as_ref().context("not prepared")?;
        let cpu_start = thread_cpu_ns();
        let t = Instant::now();
        m.call(self.entry, self.trips)?;
        let ns = t.elapsed().as_nanos() as f64;
        let mut result = Measurement::new(ns / self.trips as f64).with("trips", self.trips as f64);
        if let (Some(start), Some(end)) = (cpu_start, thread_cpu_ns()) {
            result = result.with("cpu_ns_per_trip", (end - start) / self.trips as f64);
        }
        Ok(result)
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    #[test]
    fn named_worker_has_monotonic_wall_thread_and_process_clocks() {
        std::thread::Builder::new()
            .name("h2-perf-clock".into())
            .spawn(|| {
                let name = std::fs::read_to_string("/proc/thread-self/comm").unwrap();
                assert_eq!(name.trim_end(), "h2-perf-clock");
                let wall = timeline::now_ns();
                let thread = super::thread_cpu_ns().unwrap();
                let process = timeline::host::process_cpu_ns().unwrap();
                assert!(timeline::now_ns() >= wall);
                assert!(super::thread_cpu_ns().unwrap() >= thread);
                assert!(timeline::host::process_cpu_ns().unwrap() >= process);
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
