//! Direct calls between generated handlers (RUNTIME-CONTRACT.md §4, "Direct
//! calls") against the runner: one synthetic module, never retail bytes,
//! built with `H2_DIRECT_CALLS` off and on, and run on the real runtime. A
//! direct call may only skip the runner where the runner would have done
//! nothing, so every run must end identically in every mode: transfer,
//! CPU, memory, events, backtrace, the preempted transfer, transfers spent.
mod common;
use iced_x86::{Decoder, DecoderOptions, code_asm::*};
use ir::{IrBlock, IrFunction};
use runtime::guest::Permissions;
use runtime::{
    CallConvention, CpuAbi, Dispatch, ExecCtx, Guest, GuestInterrupt, Raise, Runner, Runtime,
    TransferAbi, abi,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

const CODE: u32 = 0x400000;
const DATA: u32 = 0x108000;
/// `[DATA]` points at the next free word of the log every function writes.
const LOG: u32 = DATA + 0x100;
const STACK: u32 = 0x109000;
const UNMAPPED: u32 = 0x10;

// Entries (all in one page of code).
const NEST: u32 = CODE;
const NEST_B: u32 = CODE + 0x100;
const NEST_C: u32 = CODE + 0x200;
const REC: u32 = CODE + 0x300;
const FAULTS: u32 = CODE + 0x400;
const BAD: u32 = CODE + 0x480;
const TAIL: u32 = CODE + 0x500;
const T1: u32 = CODE + 0x580;
const T2: u32 = CODE + 0x600;
const REWRITE: u32 = CODE + 0x800;
const RW: u32 = CODE + 0x880;
const Z: u32 = CODE + 0x900;
const OVR: u32 = CODE + 0xa00;
const O: u32 = CODE + 0xa80;
const ISR: u32 = CODE + 0xb00;
const LOW: u32 = CODE + 0xc00;
const DUP: u32 = CODE + 0xc80;
const STOPS_CALLER: u32 = CODE + 0xd00;
const STOPS: u32 = CODE + 0xd80;
const HOME: u32 = CODE + 0xe00;
const HOME_B: u32 = CODE + 0xe80;
const LOOP: u32 = CODE + 0xf00;
const LOOP_B: u32 = CODE + 0xf80;
// The second page.
const IND: u32 = CODE + 0x1000;
const CL: u32 = CODE + 0x1100;
const IND_JUMP: u32 = CODE + 0x1300;
/// Shares NEST_B's slot in a table of 256, not in one of 4096.
const COLL: u32 = CODE + 0x1213;
const VEC_CALLER: u32 = CODE + 0x1800;
const VEC_CALLEE: u32 = CODE + 0x1900;
const VEC_SCRATCH: u32 = DATA + 0x80;

/// One function at `base`, lifted as the image's are (perf.rs does the same).
fn function(
    base: u32,
    body: impl FnOnce(&mut CodeAssembler) -> Result<(), IcedError>,
) -> IrFunction {
    let mut a = CodeAssembler::new(32).unwrap();
    body(&mut a).unwrap();
    let code = a.assemble(base as u64).unwrap();
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
            } => {
                starts.extend([taken.pc, fallthrough.pc]);
            }
            ir::Control::Jump {
                target: ir::Target::Direct(t),
                ..
            } => {
                starts.extend([*t, i.next_pc()]);
            }
            _ => {
                starts.insert(i.next_pc());
            }
        }
    }
    let end = base as u64 + code.len() as u64;
    let mut blocks: BTreeMap<u32, IrBlock> = BTreeMap::new();
    let mut current = base;
    for mut i in insns {
        if let ir::Control::Jump {
            target: ir::Target::Direct(target),
            tail,
            ..
        } = &mut i.control
        {
            *tail = (*target as u64) < base as u64 || *target as u64 >= end;
        }
        if starts.contains(&i.pc) {
            current = i.pc;
        }
        blocks
            .entry(current)
            .or_insert_with(|| IrBlock {
                start: current,
                insns: vec![],
            })
            .insns
            .push(i);
    }
    let f = IrFunction {
        entry: base,
        blocks,
        interior_entries: vec![],
        errors: vec![],
    };
    f.validate().unwrap_or_else(|e| panic!("{e}\n{f}"));
    f
}

/// Append `marker` to the log.
fn log(a: &mut CodeAssembler, marker: u32) -> Result<(), IcedError> {
    a.mov(edx, dword_ptr(DATA))?;
    a.mov(dword_ptr(edx), marker)?;
    a.add(dword_ptr(DATA), 4)
}

fn funcs() -> Vec<IrFunction> {
    vec![
        // Calls of two entries that share a slot in the smallest table.
        function(CL, |a| {
            let mut l = a.create_label();
            a.mov(ecx, 3)?;
            a.set_label(&mut l)?;
            a.call(NEST_B as u64)?;
            a.call(COLL as u64)?;
            a.dec(ecx)?;
            a.jnz(l)?;
            a.ret()
        }),
        function(COLL, |a| {
            log(a, 32)?;
            a.ret()
        }),
        function(IND_JUMP, |a| {
            a.mov(ebx, dword_ptr(DATA + 0x40))?;
            a.add(ebx, 1)?;
            a.mov(eax, O)?;
            a.jmp(eax)
        }),
        // Calls through a register, to a caller of its own and to a leaf
        // (O, which the override scenario overrides), in a loop.
        function(IND, |a| {
            let mut l = a.create_label();
            a.mov(ecx, 3)?;
            a.set_label(&mut l)?;
            log(a, 30)?;
            a.mov(eax, NEST_B)?;
            a.call(eax)?;
            log(a, 31)?;
            a.mov(eax, O)?;
            a.call(eax)?;
            a.dec(ecx)?;
            a.jnz(l)?;
            a.ret()
        }),
        // Nested calls, a stdcall callee among them, in a loop.
        function(NEST, |a| {
            let mut l = a.create_label();
            a.mov(ecx, 3)?;
            a.set_label(&mut l)?;
            log(a, 1)?;
            a.call(NEST_B as u64)?;
            log(a, 2)?;
            a.dec(ecx)?;
            a.jnz(l)?;
            a.ret()
        }),
        function(NEST_B, |a| {
            log(a, 3)?;
            a.push(ecx)?;
            a.call(NEST_C as u64)?;
            a.add(ebx, eax)?;
            log(a, 4)?;
            a.ret()
        }),
        function(NEST_C, |a| {
            log(a, 5)?;
            a.mov(eax, dword_ptr(esp + 4))?;
            a.ret_1(4)
        }),
        // Recursion ECX deep.
        function(REC, |a| {
            let mut done = a.create_label();
            a.test(ecx, ecx)?;
            a.jz(done)?;
            a.dec(ecx)?;
            log(a, 6)?;
            a.call(REC as u64)?;
            a.inc(ebx)?;
            a.set_label(&mut done)?;
            a.ret()
        }),
        // A callee that faults.
        function(FAULTS, |a| {
            log(a, 7)?;
            a.call(BAD as u64)?;
            log(a, 8)?;
            a.ret()
        }),
        function(BAD, |a| {
            log(a, 9)?;
            a.mov(eax, dword_ptr(UNMAPPED))?;
            a.ret()
        }),
        // A callee that tail-jumps to another function, which returns.
        function(TAIL, |a| {
            let mut l = a.create_label();
            a.mov(ecx, 3)?;
            a.set_label(&mut l)?;
            a.call(T1 as u64)?;
            log(a, 10)?;
            a.dec(ecx)?;
            a.jnz(l)?;
            a.ret()
        }),
        function(T1, |a| {
            log(a, 11)?;
            a.jmp(T2 as u64)
        }),
        function(T2, |a| {
            log(a, 12)?;
            a.ret()
        }),
        // A callee that rewrites its return address.
        function(REWRITE, |a| {
            log(a, 13)?;
            a.call(RW as u64)?;
            log(a, 14)?;
            a.ret()
        }),
        function(RW, |a| {
            a.mov(dword_ptr(esp), Z)?;
            a.ret()
        }),
        function(Z, |a| {
            log(a, 15)?;
            a.ret()
        }),
        // A callee that gets an override between runs.
        function(OVR, |a| {
            let mut l = a.create_label();
            a.mov(ecx, 4)?;
            a.set_label(&mut l)?;
            a.call(O as u64)?;
            log(a, 16)?;
            a.dec(ecx)?;
            a.jnz(l)?;
            a.ret()
        }),
        function(O, |a| {
            log(a, 17)?;
            a.add(ebx, 1)?;
            a.ret()
        }),
        // A callee that returns to the right place with ESP still at its
        // return slot (it pushed a copy of the return address): the runner
        // keeps the call's shadow record, so the halt's backtrace shows it.
        function(LOW, |a| {
            log(a, 19)?;
            a.call(DUP as u64)?;
            a.hlt()
        }),
        function(DUP, |a| {
            a.push(dword_ptr(esp))?;
            a.ret()
        }),
        // Register homes (emit::c::homes): the caller writes only EDX and
        // ESP, so it reads EBX and ESI from CpuAbi and never spills,
        // reloads or stores them, EDI or EFLAGS. The callee writes all of
        // those; the caller must see its values after each return, and
        // they must survive to the caller's own return. No flag access in
        // the caller (its log does not `add`).
        function(HOME, |a| {
            for _ in 0..2 {
                a.call(HOME_B as u64)?;
                a.mov(edx, dword_ptr(DATA))?;
                a.mov(dword_ptr(edx), ebx)?;
                a.mov(dword_ptr(edx + 4), esi)?;
                a.lea(edx, ptr(edx + 8))?;
                a.mov(dword_ptr(DATA), edx)?;
            }
            a.ret()
        }),
        function(HOME_B, |a| {
            a.add(ebx, 0x1111)?;
            a.lea(esi, ptr(ebx + 0x10))?;
            a.mov(edi, esi)?;
            a.stc()?;
            a.ret()
        }),
        // A direct call spills only the locals changed since the last
        // sync: ESI changes after the call, on the way back round the
        // loop, so the next call must spill it for the callee to see.
        function(LOOP, |a| {
            let mut top = a.create_label();
            a.mov(ecx, 3)?;
            a.set_label(&mut top)?;
            a.call(LOOP_B as u64)?;
            a.lea(esi, ptr(ecx + 0x100))?;
            a.dec(ecx)?;
            a.jnz(top)?;
            a.ret()
        }),
        function(LOOP_B, |a| {
            a.mov(edx, dword_ptr(DATA))?;
            a.mov(dword_ptr(edx), esi)?;
            a.lea(edx, ptr(edx + 4))?;
            a.mov(dword_ptr(DATA), edx)?;
            a.ret()
        }),
        // A callee that stops on an instruction the lifter does not lower
        // (`fnstenv` with a 16-bit operand size), as the boot's fallback
        // interpreter recovers from.
        function(STOPS_CALLER, |a| {
            log(a, 30)?;
            a.call(STOPS as u64)?;
            log(a, 31)?;
            a.ret()
        }),
        function(STOPS, |a| {
            log(a, 32)?;
            a.mov(ebx, 0x5150)?;
            a.mov(eax, DATA + 0x80)?;
            a.db(&[0x66, 0xD9, 0x30])?;
            a.ret()
        }),
        function(VEC_CALLER, |a| {
            a.mov(dword_ptr(VEC_SCRATCH), 0x42)?;
            a.movss(xmm7, dword_ptr(VEC_SCRATCH))?;
            a.call(VEC_CALLEE as u64)?;
            a.movss(dword_ptr(VEC_SCRATCH), xmm7)?;
            a.mov(eax, dword_ptr(VEC_SCRATCH))?;
            a.movss(dword_ptr(VEC_SCRATCH), xmm2)?;
            a.mov(edx, dword_ptr(VEC_SCRATCH))?;
            a.ret()
        }),
        function(VEC_CALLEE, |a| {
            a.movaps(xmm2, xmm7)?;
            a.mov(dword_ptr(VEC_SCRATCH), 0x99)?;
            a.movss(xmm7, dword_ptr(VEC_SCRATCH))?;
            a.ret()
        }),
        // An interrupt callback.
        function(ISR, |a| {
            log(a, 18)?;
            a.ret()
        }),
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// Built with H2_DIRECT_CALLS=0: today's code.
    Off,
    /// Built with direct calls, the context's switch off.
    Disabled,
    /// Built with direct calls and using them.
    On,
    /// [`Mode::Off`] and [`Mode::On`] built with the handler entries and
    /// exits from before `h2abi_v1_exit` and lent views (`H2_EXIT_CALL=0`:
    /// a charge, then a release; `H2_DIRECT_VIEW=0`: every handler acquires
    /// and releases its own) and installed as such a module is, through the
    /// v2 bridge table.
    OldOff,
    OldOn,
}
const MODES: [Mode; 5] = [Mode::Off, Mode::Disabled, Mode::On, Mode::OldOff, Mode::OldOn];

impl Mode {
    fn direct(self) -> bool {
        !matches!(self, Mode::Off | Mode::OldOff)
    }
    fn old(self) -> bool {
        matches!(self, Mode::OldOff | Mode::OldOn)
    }
}

struct Module {
    _lib: libloading::Library,
    table: *const runtime::generated::H2GenModule,
    version: runtime::ffi::GeneratedAbiVersion,
}

fn build(direct: bool, old: bool) -> Module {
    let owners = emit::plan_with(funcs(), emit::opt::Passes::from_env().unwrap());
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("direct-{direct}-{old}"));
    let src = dir.join("src");
    emit::write_module_with(&owners, &src, emit::Codegen { accounting: true }).unwrap();
    // The flag as `h2 emit` passes it, whatever the environment says.
    let header = src.join("h2gen.h");
    let content = std::fs::read_to_string(&header).unwrap();
    let mut flag = format!(
        "#undef H2_DIRECT_CALLS\n#define H2_DIRECT_CALLS {}\n",
        u8::from(direct)
    );
    if old {
        flag += "#define H2_EXIT_CALL 0\n#define H2_DIRECT_VIEW 0\n";
    }
    std::fs::write(&header, format!("{flag}{content}")).unwrap();
    emit::compile::install_header(&src, &common::abi_header().unwrap()).unwrap();
    let b = emit::compile::build(&src, &dir, emit::compile::Arch::host(), 2).unwrap();
    let dylib = dir.join("direct.dylib");
    emit::compile::host_dylib(&b, &src, &[("bridge.c", emit::host::BRIDGE)], &dylib).unwrap();
    let _x87 = unsafe { emit::host::Generated::load(&dylib) }.unwrap();
    let lib = unsafe { libloading::Library::new(&dylib) }.unwrap();
    unsafe {
        let install: libloading::Symbol<unsafe extern "C" fn(*const *const (), usize) -> u32> =
            lib.get(if old { b"h2bridge_install_v2".as_slice() } else { b"h2bridge_install_v3" }).unwrap();
        let callbacks = [
            runtime::ffi::h2abi_v1_read as *const (),
            runtime::ffi::h2abi_v1_write as *const (),
            runtime::generated::h2shim_v1_event as *const (),
            runtime::generated::h2shim_v1_unsupported as *const (),
            runtime::generated::h2shim_v1_rdtsc as *const (),
            runtime::ffi::h2abi_v1_atomic_rmw as *const (),
            runtime::ffi::h2abi_v1_xchg as *const (),
            runtime::ffi::h2abi_v1_cmpxchg as *const (),
            runtime::ffi::h2abi_v1_profile as *const (),
            acquire as *const (),
            release as *const (),
            runtime::ffi::h2abi_v1_read_counted as *const (),
            runtime::ffi::h2abi_v1_write_counted as *const (),
            charge as *const (),
            runtime::ffi::h2abi_v1_budget as *const (),
            exit as *const (),
        ];
        // An old module's table ends before `exit`.
        let callbacks = &callbacks[..callbacks.len() - usize::from(old)];
        assert_eq!(install(callbacks.as_ptr(), size_of_val(callbacks)), 1);
        let table: libloading::Symbol<*const runtime::generated::H2GenModule> =
            lib.get(b"h2gen_module").unwrap();
        let version = *lib
            .get::<runtime::ffi::GeneratedAbiVersion>(b"h2gen_module_abi_version")
            .unwrap();
        let features = (**table).features;
        assert_eq!(
            features & runtime::generated::FEATURE_DIRECT_CALLS != 0,
            direct,
            "{features:#x}"
        );
        Module {
            table: *table,
            version,
            _lib: lib,
        }
    }
}

thread_local! {
    /// This thread's calls of the runtime's view and settling exports:
    /// `fast_acquire`, `fast_release`, `charge`, `exit`.
    static ABI_CALLS: std::cell::Cell<[u64; 4]> = const { std::cell::Cell::new([0; 4]) };
}

fn abi_call(i: usize) {
    ABI_CALLS.with(|c| {
        let mut v = c.get();
        v[i] += 1;
        c.set(v);
    });
}

/// The bridge's callbacks: the runtime's own, counted.
unsafe extern "C" fn acquire(c: *mut runtime::ExecCtxOpaque) -> *mut runtime::ffi::FastAccess {
    abi_call(0);
    unsafe { runtime::ffi::h2abi_v1_fast_acquire(c) }
}
unsafe extern "C" fn release(c: *mut runtime::ExecCtxOpaque) {
    abi_call(1);
    unsafe { runtime::ffi::h2abi_v1_fast_release(c) }
}
unsafe extern "C" fn charge(c: *mut runtime::ExecCtxOpaque, n: u64) {
    abi_call(2);
    unsafe { runtime::ffi::h2abi_v1_charge(c, n) }
}
unsafe extern "C" fn exit(c: *mut runtime::ExecCtxOpaque, n: u64) {
    abi_call(3);
    unsafe { runtime::ffi::h2abi_v1_exit(c, n) }
}

/// Every build, once per test binary: direct calls off and on, then the
/// same two with the old exits.
fn modules() -> &'static [Module; 4] {
    struct Shared([Module; 4]);
    // SAFETY: the tables are immutable statics of libraries kept loaded.
    unsafe impl Sync for Shared {}
    unsafe impl Send for Shared {}
    static M: std::sync::OnceLock<Shared> = std::sync::OnceLock::new();
    &M.get_or_init(|| Shared([build(false, false), build(true, false), build(false, true), build(true, true)])).0
}

fn dispatch(mode: Mode) -> Dispatch {
    let m = &modules()[usize::from(mode.direct()) + 2 * usize::from(mode.old())];
    let mut d = Dispatch::default();
    unsafe { d.register_generated_module(&*m.table, m.version) }.unwrap();
    d
}

/// Everything a run can be told apart by.
#[derive(Debug, PartialEq)]
struct Outcome {
    transfer: TransferAbi,
    cpu: CpuAbi,
    memory: Vec<u8>,
    events: Vec<String>,
    backtrace: Vec<runtime::GuestFrame>,
    preempted: Option<TransferAbi>,
    transfers: u64,
    /// The context's access accounting after the run: accesses charged
    /// (`Profile::memory_accessors`), those the inline RAM view made, the
    /// slice spent, the budget left (`h2abi_v1_budget`), the preempt word.
    accounting: [u64; 5],
}

struct Machine {
    ctx: ExecCtx,
    cpu: CpuAbi,
}

impl Machine {
    fn new(mode: Mode) -> Machine {
        let mut mem = Guest::new();
        mem.map_zeroed(CODE, 8192, Permissions::RX).unwrap();
        mem.map_zeroed(DATA, 8192, Permissions::RW).unwrap();
        mem.set_u32(DATA, LOG).unwrap();
        mem.set_u32(STACK, runtime::exec::HARNESS_EXIT).unwrap();
        let mut ctx = ExecCtx::new(Runtime::new(mem, dispatch(mode)), 1);
        ctx.enable_direct_calls(matches!(mode, Mode::On | Mode::OldOn));
        ctx.enable_profiling(true);
        Machine {
            ctx,
            cpu: CpuAbi {
                esp: STACK,
                ..CpuAbi::default()
            },
        }
    }
    fn reset(&mut self) {
        self.cpu = CpuAbi {
            esp: STACK,
            ..CpuAbi::default()
        };
        let mut m = self.ctx.memory();
        m.set_u32(STACK, runtime::exec::HARNESS_EXIT).unwrap();
    }
    fn run(&mut self, runner: &Runner, transfer: TransferAbi) -> Outcome {
        let t = runner.run_from(&mut self.cpu, &mut self.ctx, transfer);
        self.outcome(t)
    }
    fn outcome(&mut self, transfer: TransferAbi) -> Outcome {
        let mut memory = vec![0; 8192];
        self.ctx.memory().read_bytes(DATA, &mut memory, 1).unwrap();
        let pending = u64::from(self.ctx.preempt_pending());
        // SAFETY: the context is live and nothing else is using it.
        let budget = unsafe { runtime::ffi::h2abi_v1_budget((&mut self.ctx as *mut ExecCtx).cast()) };
        let p = self.ctx.profile().unwrap();
        let accounting = [
            p.memory_accessors,
            p.fast_memory_accesses,
            self.ctx.slice_spent(),
            budget,
            pending,
        ];
        Outcome {
            accounting,
            transfer,
            cpu: self.cpu,
            memory,
            events: self
                .ctx
                .take_events()
                .into_iter()
                .map(|e| format!("{e:?}"))
                .collect(),
            backtrace: self.ctx.backtrace(self.cpu.eip),
            preempted: self.ctx.take_preempted(),
            transfers: self.ctx.take_profile().unwrap().transfers,
        }
    }
    fn direct_transfers(&self) -> u64 {
        self.ctx.profile().unwrap().direct_transfers
    }
}

/// Run `scenario` in every mode; all must agree.
fn agree(name: &str, scenario: impl Fn(&mut Machine) -> Vec<Outcome>) {
    let mut results = Vec::new();
    for mode in MODES {
        let mut m = Machine::new(mode);
        results.push((mode, scenario(&mut m)));
    }
    for (mode, r) in &results[1..] {
        assert_eq!(r.len(), results[0].1.len(), "{name}: {mode:?}");
        for (i, (a, b)) in results[0].1.iter().zip(r).enumerate() {
            assert_eq!(a, b, "{name}: run {i}, {mode:?} differs from Off");
        }
    }
}

fn run_twice(entry: u32) -> impl Fn(&mut Machine) -> Vec<Outcome> {
    move |m| {
        // The first run fills the context's slots; the second calls directly.
        let a = m.run(&Runner::default(), TransferAbi::jump(entry));
        m.reset();
        let b = m.run(&Runner::default(), TransferAbi::jump(entry));
        vec![a, b]
    }
}

/// How many transfers of a run were direct: the On machine runs it once
/// more to look.
fn direct_count(entry: u32, cap: Option<u32>) -> u64 {
    let mut m = Machine::new(Mode::On);
    if let Some(c) = cap {
        m.ctx.set_direct_call_depth(c);
    }
    m.run(&Runner::default(), TransferAbi::jump(entry));
    m.reset();
    m.run_counting(entry)
}

impl Machine {
    fn run_counting(&mut self, entry: u32) -> u64 {
        Runner::default().run(&mut self.cpu, &mut self.ctx, entry);
        self.direct_transfers()
    }
}

#[test]
fn nested_calls_and_a_stdcall_callee() {
    agree("nested", |m| {
        let out = run_twice(NEST)(m);
        // The accounting compared is real: accesses were charged.
        assert!(out.iter().all(|o| o.accounting[0] > 0), "{:?}", out[1].accounting);
        out
    });
    // 3 trips: A->B, B->C and both returns, all direct on the second run.
    assert_eq!(direct_count(NEST, None), 12);
}

/// Calls through a register (`H2_ICALL`): the warm run finds both targets in their slots and calls them directly, and
/// every mode still ends the same; an override on one target between runs
/// stops its calls going direct, and disabling it lets them again.
#[test]
fn indirect_calls() {
    let calls = Arc::new(AtomicU32::new(0));
    agree("indirect", |m| {
        let mut out = run_twice(IND)(m);
        let seen = calls.clone();
        m.ctx.runtime().edit_dispatch(|d| {
            d.register_override(O, "test override", move |cpu, ctx, _| {
                seen.fetch_add(1, Ordering::Relaxed);
                cpu.ebx = cpu.ebx.wrapping_add(0x100);
                ctx.memory().ret(cpu, 0)
            })
        });
        m.reset();
        out.push(m.run(&Runner::default(), TransferAbi::jump(IND)));
        m.ctx.runtime().edit_dispatch(|d| d.set_enabled(O, false));
        m.reset();
        out.push(m.run(&Runner::default(), TransferAbi::jump(IND)));
        out
    });
    assert_eq!(calls.load(Ordering::Relaxed), 3 * MODES.len() as u32);
    // 3 trips: NEST_B's call of NEST_C and its return, and IND's two
    // calls and their returns.
    assert_eq!(direct_count(IND, None), 18);
}

#[test]
fn an_indirect_jump_respects_dispatch_and_execute_permissions() {
    agree("indirect jump", |m| {
        let mut out = run_twice(IND_JUMP)(m);
        assert_eq!(out[1].cpu.ebx, out[0].cpu.ebx);
        m.reset();
        m.ctx.arm_preemption(1);
        let stopped = m.run(&Runner::default(), TransferAbi::jump(IND_JUMP));
        assert_eq!(stopped.preempted, Some(TransferAbi::jump(O)));
        assert_eq!(stopped.accounting[2], 1);
        out.push(stopped);
        m.ctx.disarm_preemption();

        m.ctx.runtime().edit_dispatch(|d| {
            d.register_override(O, "test jump override", |cpu, ctx, _| {
                cpu.ebx += 0x100;
                ctx.memory().ret(cpu, 0)
            });
        });
        m.reset();
        out.push(m.run(&Runner::default(), TransferAbi::jump(IND_JUMP)));
        assert_ne!(out[1].cpu.ebx, out[3].cpu.ebx);
        m.ctx.runtime().edit_dispatch(|d| {
            d.set_enabled(O, false);
        });
        m.reset();
        out.push(m.run(&Runner::default(), TransferAbi::jump(IND_JUMP)));

        m.ctx.memory().protect(CODE, Permissions::READ).unwrap();
        m.reset();
        out.push(m.run(&Runner::default(), TransferAbi::jump(IND_JUMP)));
        assert_eq!(out[5].transfer.kind, abi::FAULT);
        out
    });

    assert_eq!(direct_count(IND_JUMP, None), 1);
}

#[test]
fn a_native_override_is_never_learned_as_an_indirect_target() {
    unsafe extern "C" fn redirect(
        _: *mut CpuAbi,
        _: *mut runtime::ExecCtxOpaque,
        _: u32,
    ) -> TransferAbi {
        TransferAbi::jump(O)
    }

    let mut m = Machine::new(Mode::On);
    m.run_counting(IND_JUMP);
    m.ctx.runtime().edit_dispatch(|d| {
        // Keep the generated original, and replace its entry with a native
        // override that loops until the runner's transfer budget is spent.
        unsafe {
            d.register_native_override(O, "native override", CpuAbi::default().abi_version, redirect)
        }
        .unwrap();
    });
    m.reset();
    let t = Runner { max_transfers: 8 }.run(&mut m.cpu, &mut m.ctx, IND_JUMP);
    assert!(matches!(m.ctx.event(t.event_id), Some(runtime::Event::TransferLimit)));

    m.reset();
    m.ctx.enable_profiling(true);
    let t = Runner { max_transfers: 8 }.run(&mut m.cpu, &mut m.ctx, IND_JUMP);
    assert!(matches!(m.ctx.event(t.event_id), Some(runtime::Event::TransferLimit)));
    assert_eq!(m.direct_transfers(), 0);
}

/// Two callees in one slot (`H2_DIRECT_SLOTS`, runtime environment): with
/// 256 slots each call finds the other's entry there and goes through the
/// runner, which takes the slot back; with 4096 or more both stay. Every
/// mode ends the same either way.
#[test]
fn callees_sharing_a_slot() {
    let n = (CL ^ (CL >> 8), NEST_B ^ (NEST_B >> 8), COLL ^ (COLL >> 8));
    assert_eq!(n.1 & 255, n.2 & 255);
    assert_ne!(n.1 & 4095, n.2 & 4095);
    agree("shared slot", run_twice(CL));
    let slots = std::env::var("H2_DIRECT_SLOTS")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|n| n.is_power_of_two() && *n <= 8192)
        .unwrap_or(4096);
    // 3 trips: NEST_B's call of NEST_C and its return always; with room,
    // CL's two calls and their returns too.
    assert_eq!(direct_count(CL, None), if slots >= 4096 { 18 } else { 6 });
}

#[test]
fn recursion_past_the_depth_cap() {
    for depth in [40u32, 100] {
        for cap in [None, Some(0), Some(1), Some(4)] {
            agree(&format!("recursion {depth} cap {cap:?}"), |m| {
                if let Some(c) = cap {
                    m.ctx.set_direct_call_depth(c);
                }
                let mut out = Vec::new();
                for _ in 0..2 {
                    m.reset();
                    m.cpu.ecx = depth;
                    out.push(m.run(&Runner::default(), TransferAbi::jump(REC)));
                }
                out
            });
        }
    }
    let mut m = Machine::new(Mode::On);
    m.ctx.set_direct_call_depth(4);
    for _ in 0..2 {
        m.reset();
        m.cpu.ecx = 10;
        m.ctx.enable_profiling(true);
        Runner::default().run(&mut m.cpu, &mut m.ctx, REC);
    }
    let direct = m.direct_transfers();
    assert!(direct > 0, "no direct calls under the cap");
    // Ten calls, ten returns: at most the levels under the cap are direct
    // at once, the rest went through the runner.
    assert!(direct < 20, "{direct}");
}

#[test]
fn a_callee_that_faults() {
    agree("fault", |m| {
        let a = m.run(&Runner::default(), TransferAbi::jump(FAULTS));
        assert_eq!(a.transfer.kind, abi::FAULT);
        m.reset();
        let b = m.run(&Runner::default(), TransferAbi::jump(FAULTS));
        vec![a, b]
    });
}

#[test]
fn a_callee_that_tail_jumps() {
    agree("tail", run_twice(TAIL));
    // Second run: A->T1 call, T1->T2 jump, T2's return to A, three trips.
    assert_eq!(direct_count(TAIL, None), 9);
}

#[test]
fn a_callee_that_rewrites_its_return_address() {
    agree("rewrite", |m| {
        let out = run_twice(REWRITE)(m);
        let log: Vec<u32> = out[1].memory[0x100..0x100 + 12]
            .chunks(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        // 13, then Z's 15 and never A's 14: the return went where the
        // stack said.
        assert_eq!(log, [13, 15, 13], "{log:?}");
        out
    });
}

#[test]
fn a_callee_that_returns_with_esp_at_its_slot() {
    agree("low esp", |m| {
        let out = run_twice(LOW)(m);
        assert_eq!(out[1].transfer.kind, abi::STOP);
        assert_eq!(out[1].backtrace.len(), 2, "{:?}", out[1].backtrace);
        out
    });
}

#[test]
fn an_override_registered_on_the_callee_between_runs() {
    let calls = Arc::new(AtomicU32::new(0));
    agree("override", |m| {
        let first = run_twice(OVR)(m);
        let seen = calls.clone();
        m.ctx.runtime().edit_dispatch(|d| {
            d.register_override(O, "test override", move |cpu, ctx, _| {
                seen.fetch_add(1, Ordering::Relaxed);
                cpu.ebx = cpu.ebx.wrapping_add(0x100);
                ctx.memory().ret(cpu, 0)
            })
        });
        m.reset();
        let mut out = first;
        out.push(m.run(&Runner::default(), TransferAbi::jump(OVR)));
        // Disabled, the generated callee runs again.
        m.ctx.runtime().edit_dispatch(|d| d.set_enabled(O, false));
        m.reset();
        out.push(m.run(&Runner::default(), TransferAbi::jump(OVR)));
        out
    });
    // Four calls in each mode's override run, none bypassing it.
    assert_eq!(calls.load(Ordering::Relaxed), 4 * MODES.len() as u32);
}

#[test]
fn a_callee_that_stops_reaches_the_stop_recovery_hook() {
    // The boot's fallback recovers a generated STOP from a runtime hook
    // instead of wrapping every handler: a STOP in a directly invoked
    // callee is handed up to the runner, so the hook sees it with the
    // callee's CPU whether the call was direct or not.
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    agree("stop recovery", |m| {
        let s = seen.clone();
        m.ctx.runtime().edit_dispatch(|d| {
            d.set_stop_recovery(move |cpu: &mut CpuAbi, ctx: &mut ExecCtx, t: TransferAbi| {
                s.lock().unwrap().push((cpu.eip, cpu.ebx, t.kind));
                // What interpreting the rest of the callee does: return.
                ctx.memory().ret(cpu, 0).unwrap()
            })
        });
        let out = run_twice(STOPS_CALLER)(m);
        for o in &out {
            assert_eq!(
                o.events.last().map(|e| e.contains("HarnessExit")),
                Some(true),
                "{:?}",
                o.events
            );
            let log: Vec<u32> = o.memory[0x100..0x100 + 12]
                .chunks(4)
                .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
                .collect();
            assert_eq!(log, [30, 32, 31]);
        }
        out
    });
    let stop_pc = STOPS + 29;
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2 * MODES.len(), "{seen:x?}");
    assert!(
        seen.iter().all(|&s| s == (stop_pc, 0x5150, abi::STOP)),
        "{seen:x?}"
    );
    // On the second run the call into the stopping callee was direct.
    assert!(direct_count(STOPS_CALLER, None) > 0);
}

/// Every transfer limit, so the run stops at each direct call and return.
#[test]
fn every_transfer_limit() {
    for entry in [NEST, TAIL, OVR] {
        for limit in 0..40 {
            agree(&format!("{entry:#x} limit {limit}"), |m| {
                let warm = m.run(&Runner::default(), TransferAbi::jump(entry));
                m.reset();
                let r = m.run(
                    &Runner {
                        max_transfers: limit,
                    },
                    TransferAbi::jump(entry),
                );
                vec![warm, r]
            });
        }
    }
}

/// An armed slice that runs out after every possible number of accesses:
/// the preempt word is set inside a callee, at a call or at a return, and
/// the run must give up its thread at the same transfer, then resume to the
/// same end.
#[test]
fn a_preempt_request_at_every_access() {
    for entry in [NEST, TAIL, REC, IND] {
        for slice in 1..60u64 {
            agree(&format!("{entry:#x} slice {slice}"), |m| {
                let warm = {
                    m.cpu.ecx = 6;
                    m.run(&Runner::default(), TransferAbi::jump(entry))
                };
                m.reset();
                m.cpu.ecx = 6;
                m.ctx.arm_preemption(slice);
                let mut out = vec![warm];
                let mut t = TransferAbi::jump(entry);
                for _ in 0..200 {
                    let r = Runner::default().run_from(&mut m.cpu, &mut m.ctx, t);
                    let o = m.outcome(r);
                    let next = o.preempted;
                    out.push(o);
                    m.ctx.enable_profiling(true);
                    match next {
                        Some(p) => {
                            m.ctx.arm_preemption(slice);
                            t = p;
                        }
                        None => break,
                    }
                }
                out
            });
        }
    }
}

/// A host request (`request_preempt`) raised before a warm run: the first
/// safe point takes it, and no direct call may run while the word is set.
#[test]
fn a_host_request_while_armed() {
    agree("request", |m| {
        let warm = m.run(&Runner::default(), TransferAbi::jump(NEST));
        m.reset();
        m.ctx.arm_preemption(0);
        m.ctx.runtime().request_preempt(runtime::preempt::REQUEST);
        let r = m.run(&Runner::default(), TransferAbi::jump(NEST));
        let p = r.preempted.expect("preempted");
        let end = m.run(&Runner::default(), p);
        vec![warm, r, end]
    });
}

fn interrupt(thread: Option<u32>) -> GuestInterrupt {
    GuestInterrupt {
        target: ISR,
        args: vec![],
        convention: CallConvention::Cdecl,
        thread,
        coalesce: false,
        name: "test interrupt",
    }
}

/// An interrupt queued for another thread stays queued; while it is, no
/// call is direct (the runner would look at the queue at each one).
#[test]
fn a_pending_interrupt_at_a_direct_call() {
    agree("pending elsewhere", |m| {
        let warm = m.run(&Runner::default(), TransferAbi::jump(NEST));
        assert_eq!(
            m.ctx.runtime().raise_interrupt(interrupt(Some(2))),
            Raise::Queued
        );
        m.reset();
        let r = m.run(&Runner::default(), TransferAbi::jump(NEST));
        vec![warm, r]
    });
    let mut m = Machine::new(Mode::On);
    m.run(&Runner::default(), TransferAbi::jump(NEST));
    m.ctx.runtime().raise_interrupt(interrupt(Some(2)));
    m.reset();
    assert_eq!(m.run_counting(NEST), 0);
}

/// An interrupt for this thread queued at a direct call is delivered there:
/// raised from the callee's override, which hands back to the runner, the
/// next call of the warm loop must not run directly past it.
#[test]
fn an_interrupt_raised_inside_the_loop_is_delivered_at_the_next_safe_point() {
    agree("deliver", |m| {
        let warm = run_twice(OVR)(m);
        let rt = Arc::downgrade(m.ctx.runtime());
        let raised = AtomicU32::new(0);
        m.ctx.runtime().edit_dispatch(|d| {
            d.register_override(O, "raises", move |cpu, ctx, _| {
                // The second call raises; the ISR runs at the next safe
                // point, which the inline return must not skip.
                if raised.fetch_add(1, Ordering::Relaxed) == 1 {
                    rt.upgrade().unwrap().raise_interrupt(interrupt(None));
                }
                ctx.memory().ret(cpu, 0)
            })
        });
        m.reset();
        let mut out = warm;
        out.push(m.run(&Runner::default(), TransferAbi::jump(OVR)));
        out
    });
}

/// Registers a caller only reads, or never touches, live in CpuAbi: a
/// directly invoked callee's writes to them reach the caller and its exit
/// in every mode.
#[test]
fn a_callee_writes_registers_its_caller_only_reads() {
    let caller = funcs().into_iter().find(|f| f.entry == HOME).unwrap();
    // EDX and ESP only: EBX and ESI are read from CpuAbi, EDI and EFLAGS
    // are not touched at all.
    assert_eq!(emit::c::homes(&caller), 1 << 2 | 1 << 4);
    agree("homes", |m| {
        let out = run_twice(HOME)(m);
        for o in &out {
            assert_eq!(o.cpu.eip, runtime::exec::HARNESS_EXIT, "{:x?}", o.transfer);
            assert_eq!((o.cpu.ebx, o.cpu.esi, o.cpu.edi), (0x2222, 0x2232, 0x2232));
            assert_eq!(o.cpu.eflags & 1, 1, "CF from the callee: {:#x}", o.cpu.eflags);
            let log = |i: usize| u32::from_le_bytes(o.memory[0x100 + 4 * i..][..4].try_into().unwrap());
            assert_eq!([log(0), log(1), log(2), log(3)], [0x1111, 0x1121, 0x2222, 0x2232]);
        }
        out
    });
    // Both calls and both returns are direct on the second run.
    assert_eq!(direct_count(HOME, None), 4);
}

#[test]
fn xmm_homes_spill_and_reload_at_direct_calls() {
    let caller = funcs().into_iter().find(|f| f.entry == VEC_CALLER).unwrap();
    // XMM7 is written; XMM2 only read after the callee has changed it.
    let home = emit::c::homes(&caller);
    assert_eq!(home >> 9, 1 << 7);
    let call = caller.insns().find(|i| matches!(
        i.control,
        ir::Control::Call { target: ir::Target::Direct(VEC_CALLEE), .. }
    )).expect("fixture must call VEC_CALLEE directly");
    let spills = emit::c::dirty_at_exits(&caller, home);
    assert_eq!(spills[&call.pc] >> 9, 1 << 7, "spill XMM7 before the callee reads it");
    agree("xmm homes", |m| {
        let out = run_twice(VEC_CALLER)(m);
        for o in &out {
            assert_eq!((o.cpu.eax, o.cpu.edx), (0x99, 0x42));
            assert_eq!(o.cpu.xmm[7], 0x99u128.to_le_bytes());
            assert_eq!(o.cpu.xmm[2], 0x42u128.to_le_bytes());
        }
        out
    });
    assert_eq!(direct_count(VEC_CALLER, None), 2);
}

/// A local changed after a direct call returns, on the way back round a
/// loop, is spilled at the next call: the callee reads it from CpuAbi.
#[test]
fn a_register_changed_round_a_loop_reaches_the_next_direct_call() {
    agree("homes loop", |m| {
        let out = run_twice(LOOP)(m);
        for o in &out {
            let log = |i: usize| u32::from_le_bytes(o.memory[0x100 + 4 * i..][..4].try_into().unwrap());
            assert_eq!([log(0), log(1), log(2)], [0, 0x103, 0x102]);
        }
        out
    });
    assert_eq!(direct_count(LOOP, None), 6);
}

/// The calls a warm run of `entry` makes into the runtime: `fast_acquire`,
/// `fast_release`, `charge`, `exit`.
fn abi_calls(mode: Mode, entry: u32, cap: Option<u32>) -> [u64; 4] {
    let mut m = Machine::new(mode);
    if let Some(c) = cap {
        m.ctx.set_direct_call_depth(c);
    }
    m.run(&Runner::default(), TransferAbi::jump(entry));
    m.reset();
    ABI_CALLS.with(|c| c.set([0; 4]));
    m.run(&Runner::default(), TransferAbi::jump(entry));
    ABI_CALLS.with(|c| c.get())
}

/// A directly invoked handler runs on its caller's view: no acquire on
/// entry and, with nothing of its own to charge, no call on the way out.
/// Everything a run ends with is the same as with the old entries and exits
/// in every scenario above (`agree` runs `OldOff` and `OldOn` too); here,
/// the calls saved.
#[test]
fn a_direct_callee_borrows_its_callers_view() {
    if std::env::var("H2_FASTMEM").as_deref() == Ok("0") {
        common::skip("borrowed view", "H2_FASTMEM=0 builds no view");
        return;
    }
    let [off, on, old_off, old_on] = [Mode::Off, Mode::On, Mode::OldOff, Mode::OldOn].map(|m| abi_calls(m, NEST, None));
    // Through the runner every handler acquires a view and leaves with one
    // exit call where it used to release and charge.
    assert_eq!(off[0], old_off[0], "{off:?} {old_off:?}");
    assert_eq!((off[1], old_off[3]), (0, 0));
    if std::env::var("H2_VIEW_CHARGE").as_deref() != Ok("0") {
        // A counting module with H2_VIEW_CHARGE spends every count in the
        // view: with one, nothing is called to settle, on the way out or
        // before a call; only the entry's acquire is left.
        assert_eq!((off[2], off[3]), (0, 0), "{off:?}");
        assert_eq!(on, [1, 0, 0, 0], "{on:?}");
        assert_eq!((old_on[0], old_on[1], old_on[3]), (7, 7, 0), "{old_on:?}");
        assert_eq!(abi_calls(Mode::On, NEST, Some(0)), abi_calls(Mode::Disabled, NEST, None));
        return;
    }
    assert_eq!(off[3], old_off[1], "one exit per release: {off:?} {old_off:?}");
    // Direct: A calls B three times, B calls C each time, every return
    // inline. Each of the seven handlers used to acquire and release; now
    // only A acquires, and B and C borrow, each still with its own
    // accesses to charge on the way out (the exit call that is left).
    assert_eq!((old_on[0], old_on[1], old_on[3]), (7, 7, 0), "{old_on:?}");
    if std::env::var("H2_DIRECT_COUNT").as_deref() == Ok("1") {
        // The count goes with the view and comes back: A charges the lot
        // in its one exit, and no handler charges before a call.
        assert_eq!(on, [1, 0, 0, 1], "{on:?}");
    } else {
        assert_eq!((on[0], on[1], on[3]), (1, 0, 7), "{on:?}");
        // The charges before each direct call are the same.
        assert_eq!(on[2], old_on[2] - old_on[1], "{on:?} {old_on:?}");
    }
    // A call that falls back to the runner lends nothing: its callee
    // acquires, as through the runner. Past a cap of 0 that is every call,
    // as with direct calls disabled; past a cap of 1, B's calls of C.
    assert_eq!(abi_calls(Mode::On, NEST, Some(0)), abi_calls(Mode::Disabled, NEST, None));
    let [on, old_on] = [Mode::On, Mode::OldOn].map(|m| abi_calls(m, NEST, Some(1)));
    assert!(1 < on[0] && on[0] < old_on[0], "{on:?} {old_on:?}");
}
