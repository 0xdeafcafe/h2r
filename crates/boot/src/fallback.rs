//! Mixed-mode fallback: when generated code stops on an instruction the
//! lifter does not implement, interpret from there (the IR interpreter for
//! everything the lifter lowers, [`crate::x87`] for x87) until control
//! reaches an address the generated module can enter again, then hand
//! back an ordinary transfer.
//!
//! Every generated entry and resume is registered with the runtime through
//! one native trampoline, which looks up the real handler by selector,
//! calls it, and runs the fallback on an unsupported-instruction STOP. The
//! runtime, kernel and Machine see nothing different.
//!
//! This exists so the boot tracer can see past the x87 gap; it is not the
//! design (docs/FLOAT.md is). It is slow, and transcendentals are not
//! bit-exact.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use iced_x86::{Decoder, DecoderOptions};
use ir::interp::{self, Flow};
use runtime::{CpuAbi, Event, ExecCtx, ExecCtxOpaque, GuestFn, TransferAbi, abi};

struct Tables {
    /// Selector to the real generated handler.
    handlers: HashMap<u32, GuestFn>,
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
    let mut handlers: HashMap<u32, GuestFn> = entries.iter().copied().collect();
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
    STATE.get_or_init(|| {
        let bytes = *XBE.get().unwrap();
        let t = std::time::Instant::now();
        let x: &'static xbe::Xbe<'static> = Box::leak(Box::new(xbe::Xbe::parse(bytes).unwrap()));
        let mut analysis = analyze::analyze(x).unwrap();
        // The interpreter's own copy: unread jump-table targets become
        // entries so the lazy lifter can reach them.
        for &addr in COPY_TARGETS.get().map_or(&[][..], Vec::as_slice) {
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
                    Err(interp::Exit::UnknownTarget { target, .. }) if is_copy_target(target) => {
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

/// The one native handler every generated entry and resume is registered with.
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
    if t.kind != abi::STOP {
        return t;
    }
    // SAFETY: the handler has returned; the pair is ours again.
    let (cpu, ctx) = unsafe { (&mut *cpu, &mut *ctx.cast::<ExecCtx>()) };
    if let Some(target) = crate::shim::take_unknown_switch(ctx.thread_id, cpu.eip)
        && is_copy_target(target)
    {
        // A CRT copy switch generated with too few cases: take the jump.
        cpu.eip = target;
        return run(cpu, ctx);
    }
    if !crate::shim::is_unsupported(ctx.thread_id, cpu.eip) {
        return t;
    }
    run(cpu, ctx)
}

/// The CRT copy routine (memcpy and memmove share one body), whose
/// dispatch tables the analyzer cannot read; see [`copy_targets`].
pub const CRT_COPY: u32 = 0x0032_0890;
/// How far past [`CRT_COPY`] its code and tables reach.
const CRT_COPY_LEN: u32 = 0x400;

/// The CRT copy's jump-table targets, set by [`install_unread_targets`]
/// (a boot stopgap, so absent with `--no-stopgaps`).
static COPY_TARGETS: OnceLock<Vec<u32>> = OnceLock::new();

/// Every jump-table target of the CRT copy routine, read from the image.
///
/// Its dispatches are `jmp [r*4+table]` with the index masked by `and r,3`
/// or negated (the backward dword tail indexes below its base), not
/// bounded by a cmp/ja guard. The analyzer reads such tables as unbounded,
/// stops at slot 0 (which overlaps code) or misses the negative slots, and
/// emits the switch with too few cases (`crates/analyze` `jump_table`;
/// owner: analyze). Here every `jmp [r*4+disp32]` in the routine names a
/// table, and the 8 dwords on either side of it that point back into the
/// routine are its targets.
pub fn copy_targets(xbe_bytes: &[u8]) -> Vec<u32> {
    let Ok(x) = xbe::Xbe::parse(xbe_bytes) else { return Vec::new() };
    let read = |va: u32, n: u32| -> Option<Vec<u8>> { x.bytes_at(va, n as usize).ok().map(<[u8]>::to_vec) };
    let (lo, hi) = (CRT_COPY, CRT_COPY + CRT_COPY_LEN);
    let Some(code) = read(lo, CRT_COPY_LEN) else { return Vec::new() };
    let inside = |v: u32| (lo..hi).contains(&v);
    let mut out = std::collections::BTreeSet::new();
    for i in 0..code.len().saturating_sub(7) {
        // FF /4 with a SIB of scale 4 and no base: jmp [index*4+disp32].
        if code[i] != 0xff || code[i + 1] != 0x24 || code[i + 2] & 0xc7 != 0x85 {
            continue;
        }
        let table = u32::from_le_bytes(code[i + 3..i + 7].try_into().unwrap());
        if !inside(table) {
            continue;
        }
        for k in -8i32..8 {
            let at = table.wrapping_add_signed(4 * k);
            if let Some(b) = read(at, 4) {
                let v = u32::from_le_bytes(b.try_into().unwrap());
                if inside(v) && v != lo {
                    out.insert(v);
                }
            }
        }
    }
    out.into_iter().collect()
}

fn is_copy_target(target: u32) -> bool {
    COPY_TARGETS.get().is_some_and(|t| t.binary_search(&target).is_ok())
}

/// Boot stopgap: register every CRT copy table target that has no
/// generated entry or resume as a fallback entry, and let the interpreter
/// lift from them. Generated code and the interpreter both stop on such a
/// switch with an unknown target; [`trampoline`] and [`run`] continue at the
/// target when it is one of these (and only then). Returns how many were
/// registered. Retire when analyze sizes these tables.
pub fn install_unread_targets(rt: &runtime::Runtime) -> usize {
    let targets = COPY_TARGETS.get_or_init(|| copy_targets(XBE.get().copied().unwrap_or(&[])));
    let tables = TABLES.get().unwrap();
    let mut n = 0;
    rt.edit_dispatch(|d| {
        for &addr in targets {
            if tables.entries.contains(&addr) || tables.resumes.contains(&addr) {
                continue;
            }
            d.register_override(addr, "boot:fallback from a CRT copy table target", move |cpu, ctx, _| {
                cpu.eip = addr;
                Ok(run(cpu, ctx))
            });
            n += 1;
        }
    });
    n
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
        assert_eq!(init(&[], &[(0x1000, one as GuestFn)], &[]), Ok(0));
        assert!(init(&[], &[(0x2000, one as GuestFn)], &[]).is_err());
        let t = TABLES.get().unwrap();
        assert!(t.entries.contains(&0x1000) && !t.entries.contains(&0x2000));
    }
}
