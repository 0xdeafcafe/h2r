//! Mixed-mode fallback: when generated code stops on an instruction the
//! lifter does not implement, interpret from there (the IR interpreter for
//! everything the lifter lowers, [`crate::x87`] for x87) until control
//! reaches an address the generated module can enter again, then hand
//! back an ordinary transfer.
//!
//! Every generated entry is registered with the runtime as itself; the
//! runtime hands each STOP a generated handler returns to [`recover`]
//! (`Dispatch::set_stop_recovery`), which runs the fallback on an
//! unsupported-instruction STOP. The kernel and Machine see nothing
//! different. (Until 2026-11 every entry went through one [`trampoline`]
//! instead, which also kept the runtime's direct calls from ever matching;
//! `H2_BOOT_TRAMPOLINE=1` still registers that way, for comparison.)
//!
//! The lifter lowers x87 itself now (docs/lifter/FLOAT.md), so this is a safety
//! net, not the design: it keeps a boot or trace running through any
//! instruction the lifter still rejects instead of stopping dead. It is
//! slow, and interpreted transcendentals are not bit-exact.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use iced_x86::{Decoder, DecoderOptions};
use ir::interp::{self, Flow};
use runtime::{CpuAbi, Event, ExecCtx, ExecCtxOpaque, GuestFn, TransferAbi, abi};

struct Tables {
    /// Selector to the real generated handler.
    handlers: rustc_hash::FxHashMap<u32, GuestFn>,
    entries: HashSet<u32>,
    resumes: HashSet<u32>,
}

static TABLES: OnceLock<Tables> = OnceLock::new();
static STATE: OnceLock<Mutex<State>> = OnceLock::new();
static XBE: OnceLock<&'static [u8]> = OnceLock::new();

/// Counters for the report.
#[derive(Default, Debug, Clone)]
pub struct Stats {
    pub fallbacks: u64,
    pub interpreted: u64,
    pub x87: u64,
    pub x87_mnemonics: std::collections::BTreeMap<String, u64>,
    pub first: Vec<(u32, String)>,
}

pub static STATS: Mutex<Option<Stats>> = Mutex::new(None);


struct State {
    lifter: lift::Lifter<'static>,
    functions: Vec<u32>,
    cache: HashMap<u32, ir::IrInsn>,
}

/// Whether an unsupported-instruction STOP from generated code is
/// interpreted (a boot stopgap). Off until [`set_recovery`]: `--no-stopgaps`
/// leaves the STOP as the module reported it.
static RECOVERY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Turn the interpreter's recovery from generated-code STOPs on or off.
pub fn set_recovery(on: bool) {
    RECOVERY.store(on, std::sync::atomic::Ordering::SeqCst);
}

/// Set by the first [`init`]; never cleared.
static CLAIMED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Remember the image for lazy lifting; build the selector tables.
///
/// The tables, image and interpreter are process-wide and hold the first
/// module's handler pointers, so one process loads one generated module:
/// a second call is refused before anything is registered, and the caller
/// must keep the first module's library loaded for as long as its runtime
/// can reach the trampoline. Returns the count of resume selectors that
/// map to more than one handler.
pub fn init(xbe_bytes: &[u8], entries: &[(u32, GuestFn)], resumes: &[(u32, u32, u32)]) -> Result<usize, String> {
    if CLAIMED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return Err("the boot fallback serves one generated module per process; a second load was refused".into());
    }
    let mut handlers: rustc_hash::FxHashMap<u32, GuestFn> = entries.iter().copied().collect();
    let owners: HashMap<u32, GuestFn> = entries.iter().copied().collect();
    let mut conflicts = 0;
    for &(_, owner, start) in resumes {
        let f = *owners.get(&owner).ok_or_else(|| format!("resume owner {owner:#x} is not an entry"))?;
        if let Some(old) = handlers.insert(start, f)
            && old as usize != f as usize
        {
            conflicts += 1;
        }
    }
    let tables = Tables {
        handlers,
        entries: entries.iter().map(|e| e.0).collect(),
        resumes: resumes.iter().map(|r| r.0).collect(),
    };
    // CLAIMED guarantees these are unset.
    let set = XBE.set(Box::leak(xbe_bytes.to_vec().into_boxed_slice())).is_ok() && TABLES.set(tables).is_ok();
    assert!(set, "fallback tables set outside init");
    *STATS.lock().unwrap() = Some(Stats::default());
    Ok(conflicts)
}

fn state() -> &'static Mutex<State> {
    if let Some(s) = STATE.get() {
        return s;
    }
    let t = std::time::Instant::now();
    let s = build_state();
    let waited = t.elapsed().as_secs_f64();
    if std::thread::current().name() != Some(ANALYSIS_THREAD) && waited > 0.1 {
        eprintln!("fallback: waited {waited:.1}s for the interpreter's analysis");
    }
    s
}

const ANALYSIS_THREAD: &str = "fallback-analysis";

fn build_state() -> &'static Mutex<State> {
    STATE.get_or_init(|| {
        let bytes = *XBE.get().unwrap();
        let t = std::time::Instant::now();
        let x: &'static xbe::Xbe<'static> = Box::leak(Box::new(xbe::Xbe::parse(bytes).unwrap()));
        // The build left this image's analysis in the cache, if it ran
        // since the analyzer last changed.
        let (mut analysis, _) = crate::analyze_cached(x, crate::analysis_cache(bytes).as_deref()).unwrap();
        // The interpreter's own copy: with the stopgap, unread jump-table
        // targets become entries so the lazy lifter can reach them.
        let unread = UNREAD_ENABLED
            .load(std::sync::atomic::Ordering::SeqCst)
            .then(|| UNREAD.get_or_init(|| unread_targets(bytes, &analysis)));
        for &addr in unread.map_or(&[][..], |u| u.all.as_slice()) {
            analysis.functions.entry(addr).or_insert(analyze::Function {
                start: addr,
                source: analyze::Source::CodeRef,
                section: ".text".into(),
                returns: true,
            });
        }
        let a: &'static analyze::Analysis = Box::leak(Box::new(analysis));
        let lifter = lift::Lifter::new(a, x).unwrap();
        let functions = a.functions.keys().copied().collect();
        eprintln!("fallback: analyzed the image for the interpreter in {:.1}s", t.elapsed().as_secs_f64());
        Mutex::new(State { lifter, functions, cache: HashMap::new() })
    })
}

impl State {
    fn insn(&mut self, pc: u32) -> Option<&ir::IrInsn> {
        if !self.cache.contains_key(&pc) {
            // Lift the nearest functions at or below pc until one holds it.
            let i = self.functions.partition_point(|&e| e <= pc);
            for &e in self.functions[..i].iter().rev().take(64) {
                let Some(f) = self.lifter.lift(e) else { continue };
                let has = f.blocks.values().any(|b| b.insns.iter().any(|x| x.pc == pc));
                for insn in f.blocks.into_values().flat_map(|b| b.insns) {
                    self.cache.entry(insn.pc).or_insert(insn);
                }
                if has {
                    break;
                }
            }
        }
        self.cache.get(&pc)
    }
}

fn to_interp(c: &CpuAbi) -> interp::Cpu {
    interp::Cpu {
        gpr: [c.eax, c.ecx, c.edx, c.ebx, c.esp, c.ebp, c.esi, c.edi],
        eip: c.eip,
        eflags: c.eflags,
        flags_valid: c.eflags_defined,
        fs_base: c.fs_base,
        xmm: c.xmm.map(u128::from_le_bytes),
        x87: interp::X87 {
            st: c.x87.st,
            control: c.x87.control,
            status: c.x87.status,
            tag: c.x87.tag,
            opcode: c.x87.opcode,
            instruction_pointer: c.x87.instruction_pointer,
            data_pointer: c.x87.data_pointer,
            instruction_selector: c.x87.instruction_selector,
            data_selector: c.x87.data_selector,
        },
        mxcsr: c.mxcsr,
        tsc: 0,
    }
}

fn from_interp(i: &interp::Cpu, c: &mut CpuAbi) {
    [c.eax, c.ecx, c.edx, c.ebx, c.esp, c.ebp, c.esi, c.edi] = i.gpr;
    c.eip = i.eip;
    c.eflags = i.eflags | 2;
    c.eflags_defined = i.flags_valid;
    c.fs_base = i.fs_base;
    c.xmm = i.xmm.map(|x| x.to_le_bytes());
    let x = &i.x87;
    c.x87.st = x.st;
    c.x87.control = x.control;
    c.x87.status = x.status;
    c.x87.tag = x.tag;
    c.x87.opcode = x.opcode;
    c.x87.instruction_pointer = x.instruction_pointer;
    c.x87.data_pointer = x.data_pointer;
    c.x87.instruction_selector = x.instruction_selector;
    c.x87.data_selector = x.data_selector;
    c.mxcsr = i.mxcsr;
}

struct GuestMem<'a>(&'a mut runtime::Guest, Option<runtime::GuestFault>);

impl interp::Memory for GuestMem<'_> {
    fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), interp::MemFault> {
        self.0.read_bytes(addr, buf, 1).map_err(|f| {
            self.1 = Some(f);
            interp::MemFault { addr, write: false }
        })
    }
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), interp::MemFault> {
        self.0.write_bytes(addr, data, 1).map_err(|f| {
            self.1 = Some(f);
            interp::MemFault { addr, write: true }
        })
    }
}

impl crate::x87::Mem for GuestMem<'_> {
    fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), u32> {
        self.0.read_bytes(addr, buf, 1).map_err(|f| {
            self.1 = Some(f);
            addr
        })
    }
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), u32> {
        self.0.write_bytes(addr, data, 1).map_err(|f| {
            self.1 = Some(f);
            addr
        })
    }
}

fn resume_jump(target: u32) -> TransferAbi {
    let mut t = TransferAbi::jump(target);
    t.route = abi::RESUME;
    t
}

const MAX_STEPS: u64 = 50_000_000;

/// The interpreter counted `reads` rdtsc values up from a seed it took
/// from the runtime; read the runtime counter until it is past all of
/// them. Each runtime read moves at least `TSC_MIN_STEP`, so this is
/// about `reads / 64` reads, and none when there were no interpreted reads
/// past the seed's own step.
fn publish_tsc(reads: u64, mut read: impl FnMut() -> u64) {
    if reads < runtime::tsc::TSC_MIN_STEP {
        return;
    }
    let target = read().wrapping_add(reads);
    while read() < target {}
}

/// Interpret from `cpu.eip`.
fn run(cpu: &mut CpuAbi, ctx: &mut ExecCtx) -> TransferAbi {
    let tables = TABLES.get().unwrap();
    let mut st = state().lock().unwrap();
    let rt = Arc::clone(ctx.runtime());
    let mut icpu = to_interp(cpu);
    // rdtsc in the interpreter counts up from here; seed it from the
    // runtime's guest TSC (kernel clock) before taking the memory lock.
    let seed = rt.read_tsc();
    icpu.tsc = seed;
    let mut steps = 0u64;
    let (mut x87n, mut mn): (u64, HashMap<String, u64>) = (0, HashMap::new());
    let result = {
        let mut guard = rt.memory();
        let mut mem = GuestMem(&mut guard, None);
        loop {
            let pc = icpu.eip;
            if steps > 0 {
                if tables.resumes.contains(&pc) {
                    break Ok(resume_jump(pc));
                }
                if tables.entries.contains(&pc) {
                    break Ok(TransferAbi::jump(pc));
                }
            }
            if steps >= MAX_STEPS {
                break Err(format!("fallback step limit at {pc:#x}"));
            }
            steps += 1;
            let Some(insn) = st.insn(pc).cloned() else {
                break Err(format!("fallback: no lifted instruction at {pc:#x}"));
            };
            if insn.is_supported() {
                match interp::step(&mut icpu, &mut mem, &insn) {
                    Ok(Flow::Call { target, continuation }) => break Ok(TransferAbi::call(target, continuation)),
                    Ok(Flow::Return { target }) => break Ok(TransferAbi::ret(target)),
                    Ok(Flow::Jump { target, tail: true }) => break Ok(TransferAbi::jump(target)),
                    Ok(_) => continue,
                    Err(interp::Exit::Memory { pc, .. }) => {
                        let f = mem.1.take();
                        break Err(format!("memory fault at {pc:#x}: {f:x?}"));
                    }
                    Err(interp::Exit::UnknownTarget { pc, target }) if unread_allowed(pc, target) => {
                        icpu.eip = target;
                        continue;
                    }
                    Err(e) => break Err(format!("fallback stop: {e:x?}")),
                }
            }
            // Not lowered: decode it ourselves.
            let mut code = [0u8; 15];
            let n = (0..15).take_while(|&i| mem.0.read_bytes(pc + i, &mut code[i as usize..i as usize + 1], 1).is_ok()).count();
            let mut d = Decoder::with_ip(32, &code[..n], pc as u64, DecoderOptions::NONE);
            let ins = d.decode();
            let (m, detail) = insn.unsupported().unwrap_or(("?", "?"));
            let r = if detail.starts_with("x87") {
                crate::x87::exec(&mut icpu, &mut mem, &ins)
            } else if detail.starts_with("sse") {
                crate::sse::exec(&mut icpu, &mut mem, &ins)
            } else {
                break Err(format!("unsupported instruction at {pc:#x}: {m} ({detail}) in the fallback"));
            };
            match r {
                Ok(next) => {
                    x87n += 1;
                    *mn.entry(format!("{:?}", ins.mnemonic())).or_default() += 1;
                    icpu.eip = next;
                }
                Err(crate::x87::Error::Memory(a)) => {
                    let f = mem.1.take();
                    break Err(format!("x87 memory fault at {pc:#x} addr {a:#x}: {f:x?}"));
                }
                Err(crate::x87::Error::Unsupported(s)) => break Err(format!("at {pc:#x}: {s}")),
            }
        }
    };
    drop(st);
    // Hand the interpreted reads back (the memory lock is released, so the
    // clock may be read): the next generated rdtsc must not repeat or go
    // back on a value the interpreter returned.
    publish_tsc(icpu.tsc.wrapping_sub(seed), || rt.read_tsc());
    {
        let mut s = STATS.lock().unwrap();
        let s = s.get_or_insert_default();
        s.fallbacks += 1;
        s.interpreted += steps;
        s.x87 += x87n;
        for (k, v) in mn {
            *s.x87_mnemonics.entry(k).or_default() += v;
        }
        if s.first.len() < 40 && s.first.iter().all(|f| f.0 != cpu.eip) {
            s.first.push((cpu.eip, String::new()));
        }
    }
    from_interp(&icpu, cpu);
    match result {
        Ok(t) => t,
        Err(msg) => {
            let id = crate::shim::record_ctx(ctx, cpu.eip, msg);
            TransferAbi::stop(id)
        }
    }
}

/// What the boot does with a STOP a generated handler returns: with
/// recovery on, an unknown switch target its site's table holds is taken
/// and a lifter-unsupported instruction is interpreted, both by [`run`];
/// any other STOP (or recovery off) is returned as it is. The runtime calls
/// this for every generated STOP ([`runtime::Dispatch::set_stop_recovery`],
/// installed by [`install_stop_recovery`]), so the handlers themselves are
/// registered directly and run with no wrapper.
pub fn recover(cpu: &mut CpuAbi, ctx: &mut ExecCtx, t: TransferAbi) -> TransferAbi {
    recover_if(RECOVERY.load(std::sync::atomic::Ordering::SeqCst), cpu, ctx, t)
}

fn recover_if(on: bool, cpu: &mut CpuAbi, ctx: &mut ExecCtx, t: TransferAbi) -> TransferAbi {
    if t.kind != abi::STOP || !on {
        return t;
    }
    if let Some(target) = crate::shim::take_unknown_switch(ctx.thread_id, cpu.eip)
        && unread_allowed(cpu.eip, target)
    {
        // A switch generated with too few cases: take the jump.
        cpu.eip = target;
        return run(cpu, ctx);
    }
    if !crate::shim::is_unsupported(ctx.thread_id, cpu.eip) {
        return t;
    }
    run(cpu, ctx)
}

/// Have the runtime pass every generated STOP to [`recover`].
pub fn install_stop_recovery(rt: &runtime::Runtime) {
    rt.edit_dispatch(|d| d.set_stop_recovery(recover));
}

/// One native handler for every generated entry and resume, which looks the
/// real handler up by selector, calls it, and [`recover`]s its STOP. Only
/// with `H2_BOOT_TRAMPOLINE=1` (see `load_module`), to compare against the
/// old registration: the runtime's direct calls never match it, so every
/// call between generated functions goes through the runner.
///
/// # Safety
/// Called by the runtime with the live, exclusive CPU/context pair.
pub unsafe extern "C" fn trampoline(cpu: *mut CpuAbi, ctx: *mut ExecCtxOpaque, sel: u32) -> TransferAbi {
    let tables = TABLES.get().unwrap();
    let Some(&f) = tables.handlers.get(&sel) else {
        // SAFETY: as above.
        let ctx = unsafe { &mut *ctx.cast::<ExecCtx>() };
        return TransferAbi::fault(ctx.emit(Event::UnrecordedFault { pc: sel }));
    };
    // SAFETY: the real handler has the same contract.
    let t = unsafe { f(cpu, ctx, sel) };
    if t.kind != abi::STOP || !RECOVERY.load(std::sync::atomic::Ordering::SeqCst) {
        return t;
    }
    // SAFETY: the handler has returned; the pair is ours again.
    let (cpu, ctx) = unsafe { (&mut *cpu, &mut *ctx.cast::<ExecCtx>()) };
    recover(cpu, ctx, t)
}

/// Jump-table targets the analyzer did not read; see [`unread_targets`].
pub struct Unread {
    /// `jmp` site to the targets its table holds.
    pub by_site: HashMap<u32, Vec<u32>>,
    /// Every target, sorted.
    pub all: Vec<u32>,
}

static UNREAD: OnceLock<Unread> = OnceLock::new();
/// Set by [`install_unread_targets`] (the stopgap) before the interpreter's
/// analysis is built.
static UNREAD_ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The table of a `jmp [index*4+disp32]` (FF /4, SIB scale 4, no base).
fn table_of(code: &[u8]) -> Option<u32> {
    (code.len() >= 7 && code[0] == 0xff && code[1] == 0x24 && code[2] & 0xc7 == 0x85)
        .then(|| u32::from_le_bytes(code[3..7].try_into().unwrap()))
}

/// Jump-table targets the analyzer emitted no case for: every switch it
/// found unguarded (`SwitchKind::Unbounded`) has its table read from the
/// image until an entry is neither zero nor code in the site's section,
/// skipping zeros. Up to 256 entries. Analyze now reads every table this
/// finds on the retail image (the CRT copy's masked and negative-index
/// tables since bceed11; the long null runs of `0x336f0`, `0x33980` and
/// `0x22dcd9` since it skips up to 16 nulls in a table after its own
/// code), pinned by `analyze`'s `xbe_pins`, so this is a net for a table
/// read short, not a fix in use.
///
/// Owner: analyze (`jump_table`).
pub fn unread_targets(xbe_bytes: &[u8], analysis: &analyze::Analysis) -> Unread {
    let mut by_site = HashMap::new();
    if let Ok(x) = xbe::Xbe::parse(xbe_bytes) {
        for (&site, kind) in &analysis.switches {
            if *kind != analyze::SwitchKind::Unbounded {
                continue;
            }
            let (Some(sec), Ok(code)) = (x.section_containing(site), x.bytes_at(site, 7)) else { continue };
            let Some(table) = table_of(code) else { continue };
            let mut targets = Vec::new();
            for k in 0..256u32 {
                let Ok(v) = x.read_u32(table.wrapping_add(4 * k)) else { break };
                if v == 0 {
                    continue;
                }
                if !sec.contains(v) {
                    break;
                }
                targets.push(v);
            }
            by_site.insert(site, targets);
        }
    }
    let all = by_site.values().flatten().copied().collect::<std::collections::BTreeSet<_>>().into_iter().collect();
    Unread { by_site, all }
}

/// Whether the switch at `site` may continue at `target` (a target its
/// table holds; never an arbitrary address).
fn unread_allowed(site: u32, target: u32) -> bool {
    UNREAD.get().and_then(|u| u.by_site.get(&site)).is_some_and(|t| t.contains(&target))
}

/// Boot stopgap: let the interpreter lift from every [`unread_targets`]
/// target, and register each that has no generated entry or resume as a
/// fallback entry. A switch that stops with an unknown target, in
/// generated code or the interpreter, continues at it when the site's
/// table holds it ([`trampoline`], [`run`]). Returns how many were
/// registered. Retire when analyze reads these tables. Builds the
/// interpreter's analysis now.
pub fn install_unread_targets(rt: &runtime::Runtime) -> usize {
    UNREAD_ENABLED.store(true, std::sync::atomic::Ordering::SeqCst);
    let bytes = *XBE.get().unwrap();
    let path = unread_cache_path(bytes);
    match path.as_deref().and_then(read_unread_cache) {
        // Cached: register now, and build the interpreter's analysis on a
        // thread; the first interpreted entry waits for it if it must.
        Some(u) => {
            let _ = UNREAD.set(u);
            eprintln!("fallback: unread jump-table targets from {}", path.as_deref().unwrap().display());
            let _ = std::thread::Builder::new().name(ANALYSIS_THREAD.into()).spawn(|| {
                let _ = state();
            });
        }
        None => {
            let _ = state();
            if let (Some(p), Some(u)) = (path.as_deref(), UNREAD.get()) {
                write_unread_cache(p, u);
            }
        }
    }
    let Some(unread) = UNREAD.get() else { return 0 };
    let tables = TABLES.get().unwrap();
    let mut n = 0;
    rt.edit_dispatch(|d| {
        for &addr in &unread.all {
            if tables.entries.contains(&addr) || tables.resumes.contains(&addr) {
                continue;
            }
            d.register_override(addr, "boot:fallback from an unread jump-table target", move |cpu, ctx, _| {
                cpu.eip = addr;
                Ok(run(cpu, ctx))
            });
            n += 1;
        }
    });
    n
}

/// The boot's cache directory (`H2_BOOT_CACHE`, or `$TMPDIR/h2boot-cache`),
/// or `None` for `H2_BOOT_CACHE=off`. Holds the unread-target sets and
/// the cached analyses ([`crate::analysis_cache`]).
pub(crate) fn boot_cache_dir() -> Option<std::path::PathBuf> {
    match std::env::var_os("H2_BOOT_CACHE") {
        Some(d) if d == "off" => None,
        Some(d) => Some(std::path::PathBuf::from(d)),
        None => Some(std::env::temp_dir().join("h2boot-cache")),
    }
}

/// Where the unread-target set for this image is cached: `H2_BOOT_CACHE`
/// or `$TMPDIR/h2boot-cache`, keyed on the XBE bytes and on the analyze
/// and fallback sources (`build.rs`). `H2_BOOT_CACHE=off` disables it.
fn unread_cache_path(xbe: &[u8]) -> Option<std::path::PathBuf> {
    Some(boot_cache_dir()?.join(format!("unread-{:016x}-{}.txt", fnv64(xbe), env!("UNREAD_SOURCE_HASH"))))
}

pub(crate) fn fnv64(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// One line per site: `site target target ...`, hex.
fn read_unread_cache(path: &std::path::Path) -> Option<Unread> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut by_site = HashMap::new();
    for line in text.lines() {
        let mut words = line.split_whitespace().map(|w| u32::from_str_radix(w, 16));
        let site = words.next()?.ok()?;
        let targets = words.collect::<Result<Vec<u32>, _>>().ok()?;
        by_site.insert(site, targets);
    }
    let all = by_site.values().flatten().copied().collect::<std::collections::BTreeSet<_>>().into_iter().collect();
    Some(Unread { by_site, all })
}

fn write_unread_cache(path: &std::path::Path, u: &Unread) {
    let mut sites: Vec<_> = u.by_site.iter().collect();
    sites.sort();
    let text: String = sites
        .into_iter()
        .map(|(site, t)| std::iter::once(*site).chain(t.iter().copied()).map(|v| format!("{v:x}")).collect::<Vec<_>>().join(" ") + "\n")
        .collect();
    let tmp = path.with_extension("tmp");
    let ok = path.parent().is_some_and(|d| std::fs::create_dir_all(d).is_ok())
        && std::fs::write(&tmp, text).is_ok()
        && std::fs::rename(&tmp, path).is_ok();
    if !ok {
        crate::log::note(&format!("fallback: could not cache unread targets at {}", path.display()));
    }
}

/// Interpret from `cpu.eip` (for overrides and tests).
pub fn interpret(cpu: &mut CpuAbi, ctx: &mut ExecCtx) -> TransferAbi {
    run(cpu, ctx)
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" fn one(_: *mut CpuAbi, _: *mut ExecCtxOpaque, _: u32) -> TransferAbi {
        TransferAbi::ret(0)
    }

    unsafe extern "C" fn stops(_: *mut CpuAbi, _: *mut ExecCtxOpaque, _: u32) -> TransferAbi {
        TransferAbi::stop(7)
    }

    /// The process-wide tables, once: 0x1000 returns, 0x3000 stops.
    fn tables() -> &'static Tables {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            init(&[], &[(0x1000, one as GuestFn), (0x3000, stops as GuestFn)], &[]).unwrap();
        });
        TABLES.get().unwrap()
    }

    #[test]
    fn no_stopgaps_keeps_the_generated_stop() {
        tables();
        set_recovery(false);
        let mut cpu = CpuAbi { eip: 0x3000, ..Default::default() };
        // With recovery off the trampoline returns before it touches ctx.
        let t = unsafe { trampoline(&mut cpu, std::ptr::null_mut(), 0x3000) };
        assert_eq!((t.kind, t.event_id), (abi::STOP, 7));
        assert_eq!(cpu.eip, 0x3000);
    }

    #[test]
    fn recovery_leaves_every_other_stop_as_it_is() {
        let rt = runtime::Runtime::new(runtime::Guest::new(), runtime::Dispatch::default());
        // A thread id no other test uses: the shim's tables are global.
        let mut ctx = ExecCtx::new(rt, 0x7e57);
        let mut cpu = CpuAbi { eip: 0x5000, ..Default::default() };
        // A STOP that is not the lifter's, and a FAULT.
        let id = crate::shim::record_ctx(&mut ctx, 0x5000, "guest exception/stop 8 hlt".into());
        for t in [TransferAbi::stop(id), TransferAbi::fault(id)] {
            let back = recover_if(true, &mut cpu, &mut ctx, t);
            assert_eq!((back.kind, back.event_id, cpu.eip), (t.kind, id, 0x5000));
        }
        // An unknown switch target no table allows is not taken.
        crate::shim::note_unknown_switch(0x7e57, 0x5000, 0x6000);
        let back = recover_if(true, &mut cpu, &mut ctx, TransferAbi::stop(id));
        assert_eq!((back.kind, cpu.eip), (abi::STOP, 0x5000));
        assert_eq!(crate::shim::take_unknown_switch(0x7e57, 0x5000), None, "consumed");
        // Recovery off: even the lifter's own STOP stays.
        let id = crate::shim::record_ctx(&mut ctx, 0x5000, "unsupported instruction at 0x5000: x (y)".into());
        let back = recover_if(false, &mut cpu, &mut ctx, TransferAbi::stop(id));
        assert_eq!((back.kind, back.event_id), (abi::STOP, id));
    }

    #[test]
    fn unread_cache_round_trips() {
        let dir = std::env::temp_dir().join(format!("h2boot-unread-test-{}", std::process::id()));
        let path = dir.join("u.txt");
        let u = Unread { by_site: HashMap::from([(0x320b00, vec![0x320b5c, 0x32092c]), (0x336f0, vec![])]), all: vec![] };
        write_unread_cache(&path, &u);
        let back = read_unread_cache(&path).unwrap();
        assert_eq!(back.by_site, u.by_site);
        assert_eq!(back.all, vec![0x32092c, 0x320b5c]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn interpreted_tsc_reads_are_published() {
        // Runtime counter at 1000, stepping by 64; the interpreter used 300.
        let mut last = 1000u64;
        let mut read = || {
            last += runtime::tsc::TSC_MIN_STEP;
            last
        };
        publish_tsc(300, &mut read);
        assert!(read() > 1000 + 300);
        let mut n = 0;
        publish_tsc(10, || {
            n += 1;
            0
        });
        assert_eq!(n, 0, "fewer reads than one step need nothing");
    }

    #[test]
    fn a_second_module_is_refused() {
        let t = tables();
        assert!(init(&[], &[(0x2000, one as GuestFn)], &[]).is_err());
        assert!(t.entries.contains(&0x1000) && !t.entries.contains(&0x2000));
    }
}
