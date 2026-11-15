//! A minimal host for generated code, for tests and the CLI's `--run-entry`.
//!
//! This is not the runtime. It loads a generated module built as a host
//! dylib (with [`BRIDGE`] supplying the ABI symbols), backs guest memory
//! with one [`FlatMemory`], and runs transfers in a loop: entry-routed
//! targets resolve through the module's entry table, continuations through
//! its resume table, anything else ends the run. The runtime's runner is
//! the real implementation of this contract; this one exists so the
//! emitter can be checked end to end without depending on runtime
//! internals that are still moving.
//!
//! `CpuAbi` and `TransferAbi` here mirror `h2abi.h` v1 field for field
//! (with layout assertions); the runtime's definitions are authoritative.
//!
//! The atomic accessors are emulated after the header's contract and the
//! runtime's `atomic.rs` (validate the span, compute on a copy, commit CPU,
//! memory and result together, change nothing on a fault), written
//! independently of the IR so the differential tests compare the lifter's
//! statements against the operation the runtime performs. One deliberate
//! difference: a fault on an unmapped operand is reported as the read the
//! interpreter reports, where the runtime reports a write-access check.

use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_void};
use std::path::Path;

use ir::interp::{FlatMemory, MemFault, Memory};

/// The C bridge source; see `c/bridge.c` and, for the `h2x87_v1_*`
/// symbols, `c/bridge_x87.c`.
pub const BRIDGE: &str = concat!(include_str!("../c/bridge.c"), include_str!("../c/bridge_x87.c"));

/// The x87 helpers the bridge forwards, in `h2x87bridge_install` index
/// order (`h2x87.h`'s declaration order): their names, and `crates/x87`'s
/// implementations, which [`Generated::load`] installs.
macro_rules! x87_helpers {
    ($($name:ident,)*) => {
        pub const X87_BRIDGE_SYMBOLS: [&str; 35] = [$(stringify!($name)),*];
        fn x87_helpers() -> [*const c_void; 35] {
            [$(x87::abi::ffi::$name as *const c_void),*]
        }
    };
}
x87_helpers! {
    h2x87_v1_ld_f32,
    h2x87_v1_ld_f64,
    h2x87_v1_ld_f80,
    h2x87_v1_ld_i16,
    h2x87_v1_ld_i32,
    h2x87_v1_ld_i64,
    h2x87_v1_ld_bcd,
    h2x87_v1_ld_const,
    h2x87_v1_ld_st,
    h2x87_v1_st_f32,
    h2x87_v1_st_f64,
    h2x87_v1_st_f80,
    h2x87_v1_st_i16,
    h2x87_v1_st_i32,
    h2x87_v1_st_i64,
    h2x87_v1_st_bcd,
    h2x87_v1_st_i32_trunc,
    h2x87_v1_store_commit,
    h2x87_v1_st_st,
    h2x87_v1_ftol2,
    h2x87_v1_arith,
    h2x87_v1_unary,
    h2x87_v1_cmp,
    h2x87_v1_xam,
    h2x87_v1_xch,
    h2x87_v1_free,
    h2x87_v1_adj_top,
    h2x87_v1_cmov,
    h2x87_v1_ldcw,
    h2x87_v1_wait,
    h2x87_v1_clex,
    h2x87_v1_init,
    h2x87_v1_save_image,
    h2x87_v1_save_commit,
    h2x87_v1_load_env,
}

pub const ABI_VERSION: u32 = 1;
pub const CALL: u32 = 1;
pub const JUMP: u32 = 2;
pub const RETURN: u32 = 3;
pub const FAULT: u32 = 4;
pub const STOP: u32 = 5;
pub const ENTRY: u32 = 0;
pub const RESUME: u32 = 1;
pub const RETURN_TARGET: u32 = 2;
/// `H2_HARNESS_EXIT`: a return here ends a run successfully.
pub const HARNESS_EXIT: u32 = 0xffff_fffc;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuAbi {
    pub abi_version: u32,
    pub struct_size: u32,
    pub gpr: [u32; 8],
    pub eip: u32,
    pub fs_base: u32,
    pub eflags: u32,
    pub eflags_defined: u32,
    pub x87: X87Abi,
    pub xmm: [[u8; 16]; 8],
    pub mxcsr: u32,
}

/// `X87Abi`, field for field.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct X87Abi {
    pub st: [[u8; 10]; 8],
    pub control: u16,
    pub status: u16,
    pub tag: u16,
    pub opcode: u16,
    pub instruction_pointer: u32,
    pub data_pointer: u32,
    pub instruction_selector: u16,
    pub data_selector: u16,
}

const _: () = assert!(size_of::<X87Abi>() == 100 && align_of::<X87Abi>() == 4);
const _: () = assert!(size_of::<CpuAbi>() == 288 && align_of::<CpuAbi>() == 4);
const _: () = assert!(std::mem::offset_of!(CpuAbi, x87) == 56);
const _: () = assert!(std::mem::offset_of!(CpuAbi, mxcsr) == 284);
const _: () = assert!(std::mem::offset_of!(CpuAbi, eip) == 40);
const _: () = assert!(std::mem::offset_of!(CpuAbi, xmm) == 156);

impl Default for CpuAbi {
    fn default() -> Self {
        CpuAbi {
            abi_version: ABI_VERSION,
            struct_size: size_of::<CpuAbi>() as u32,
            gpr: [0; 8],
            eip: 0,
            fs_base: 0,
            eflags: 2,
            eflags_defined: u32::MAX,
            x87: X87Abi::from_interp(&ir::interp::X87::default()),
            xmm: [[0; 16]; 8],
            mxcsr: ir::interp::MXCSR_DEFAULT,
        }
    }
}

impl X87Abi {
    pub fn from_interp(x: &ir::interp::X87) -> Self {
        X87Abi {
            st: x.st,
            control: x.control,
            status: x.status,
            tag: x.tag,
            opcode: x.opcode,
            instruction_pointer: x.instruction_pointer,
            data_pointer: x.data_pointer,
            instruction_selector: x.instruction_selector,
            data_selector: x.data_selector,
        }
    }

    pub fn to_interp(&self) -> ir::interp::X87 {
        ir::interp::X87 {
            st: self.st,
            control: self.control,
            status: self.status,
            tag: self.tag,
            opcode: self.opcode,
            instruction_pointer: self.instruction_pointer,
            data_pointer: self.data_pointer,
            instruction_selector: self.instruction_selector,
            data_selector: self.data_selector,
        }
    }
}

impl CpuAbi {
    /// From the interpreter's CPU model (the fields they share).
    pub fn from_interp(c: &ir::interp::Cpu) -> Self {
        let mut a = CpuAbi {
            gpr: c.gpr,
            eip: c.eip,
            fs_base: c.fs_base,
            eflags: c.eflags,
            eflags_defined: c.flags_valid,
            x87: X87Abi::from_interp(&c.x87),
            mxcsr: c.mxcsr,
            ..Default::default()
        };
        for (i, x) in c.xmm.iter().enumerate() {
            a.xmm[i] = x.to_le_bytes();
        }
        a
    }

    pub fn to_interp(&self) -> ir::interp::Cpu {
        let mut c = ir::interp::Cpu {
            gpr: self.gpr,
            eip: self.eip,
            eflags: self.eflags,
            flags_valid: self.eflags_defined,
            fs_base: self.fs_base,
            x87: self.x87.to_interp(),
            mxcsr: self.mxcsr,
            ..Default::default()
        };
        for (i, x) in self.xmm.iter().enumerate() {
            c.xmm[i] = u128::from_le_bytes(*x);
        }
        c
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferAbi {
    pub abi_version: u32,
    pub struct_size: u32,
    pub kind: u32,
    pub route: u32,
    pub target_pc: u32,
    pub resume_pc: u32,
    pub event_id: u32,
}

impl TransferAbi {
    pub fn new(kind: u32, route: u32, target_pc: u32) -> Self {
        TransferAbi {
            abi_version: ABI_VERSION,
            struct_size: size_of::<Self>() as u32,
            kind,
            route,
            target_pc,
            resume_pc: 0,
            event_id: 0,
        }
    }
}

pub type Handler = unsafe extern "C" fn(*mut CpuAbi, *mut c_void, u32) -> TransferAbi;

#[repr(C)]
struct GenEntry {
    addr: u32,
    kind: u32,
    f: Option<Handler>,
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

/// A diagnostic the generated code reported through a helper or shim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostEvent {
    Memory { pc: u32, fault: MemFault },
    /// `h2shim_v1_event`: code is an `H2SHIM_*` constant; `saved_eip` the
    /// EIP the exception reports (the next instruction for traps).
    Guest { pc: u32, saved_eip: u32, code: u32, arg: u32 },
    Unsupported { pc: u32, mnemonic: String, detail: String },
}

/// Execution context handed to generated code as `ExecCtxOpaque *`.
/// `preempt` must stay first: generated loops read the word it points to
/// (RUNTIME-CONTRACT.md, "Loop safe points"). Set it and the next loop
/// back-edge leaves with `JUMP`/`RESUME` to its header.
#[repr(C)]
pub struct Ctx {
    pub preempt: Box<std::sync::atomic::AtomicU32>,
    pub mem: FlatMemory,
    pub events: Vec<HostEvent>,
    /// Calls to the atomic accessors, by `H2_ATOMIC_*` tag (`xchg` is 15,
    /// `cmpxchg` 16), so tests can see the operation-wide path was taken.
    pub atomics: Vec<u32>,
    /// What `h2shim_v1_rdtsc` returns next; counts up like the
    /// interpreter's [`ir::interp::Cpu::tsc`].
    pub tsc: u64,
    /// The x87 state the CPU held when each #MF (event 14) was reported,
    /// as the callback saw it: FLOAT-ABI.md §2.4 has the raiser's
    /// FIP/FOP/FDP materialised before the event is raised.
    pub fp_error_x87: Vec<X87Abi>,
    /// The CPU of the running handler ([`Generated::run`]), for
    /// [`Ctx::fp_error_x87`].
    cpu: *const CpuAbi,
}

const _: () = assert!(std::mem::offset_of!(Ctx, preempt) == 0);

impl Ctx {
    pub fn new(mem: FlatMemory) -> Self {
        Ctx {
            preempt: Box::default(),
            mem,
            events: Vec::new(),
            atomics: Vec::new(),
            tsc: 0,
            fp_error_x87: Vec::new(),
            cpu: std::ptr::null(),
        }
    }

    fn emit(&mut self, e: HostEvent) -> u32 {
        self.events.push(e);
        self.events.len() as u32
    }

    pub fn event(&self, id: u32) -> Option<&HostEvent> {
        self.events.get((id as usize).checked_sub(1)?)
    }
}

#[repr(C)]
struct Bridge {
    read: unsafe extern "C" fn(*mut c_void, u32, u32, u32, u32, *mut u8) -> u32,
    write: unsafe extern "C" fn(*mut c_void, u32, u32, u32, u32, *const u8) -> u32,
    event: unsafe extern "C" fn(*mut c_void, u32, u32, u32, u32) -> u32,
    unsupported: unsafe extern "C" fn(*mut c_void, u32, *const c_char, *const c_char) -> u32,
    rdtsc: unsafe extern "C" fn(*mut c_void, u32) -> u64,
    rmw: unsafe extern "C" fn(*mut CpuAbi, *mut c_void, u32, u32, u32, u32, u32, *mut u32) -> u32,
    xchg: unsafe extern "C" fn(*mut CpuAbi, *mut c_void, u32, u32, u32, u32, *mut u32) -> u32,
    cmpxchg: unsafe extern "C" fn(*mut CpuAbi, *mut c_void, u32, u32, u32, u32, *mut u32) -> u32,
}

/// # Safety (all callbacks)
/// Generated code passes back the `Ctx` pointer it was given and buffers
/// valid for `width` bytes, per the ABI contract.
unsafe extern "C" fn cb_read(c: *mut c_void, pc: u32, addr: u32, width: u32, align: u32, out: *mut u8) -> u32 {
    let ctx = unsafe { &mut *c.cast::<Ctx>() };
    let mut buf = [0u8; 16];
    let buf = &mut buf[..width as usize];
    if align > 1 && !addr.is_multiple_of(align) {
        return ctx.emit(HostEvent::Guest { pc, saved_eip: pc, code: 7, arg: addr });
    }
    match ctx.mem.read(addr, buf) {
        Ok(()) => {
            unsafe { std::ptr::copy_nonoverlapping(buf.as_ptr(), out, buf.len()) };
            0
        }
        Err(fault) => ctx.emit(HostEvent::Memory { pc, fault }),
    }
}

unsafe extern "C" fn cb_write(c: *mut c_void, pc: u32, addr: u32, width: u32, align: u32, v: *const u8) -> u32 {
    let ctx = unsafe { &mut *c.cast::<Ctx>() };
    if align > 1 && !addr.is_multiple_of(align) {
        return ctx.emit(HostEvent::Guest { pc, saved_eip: pc, code: 7, arg: addr });
    }
    let data = unsafe { std::slice::from_raw_parts(v, width as usize) };
    match ctx.mem.write(addr, data) {
        Ok(()) => 0,
        Err(fault) => ctx.emit(HostEvent::Memory { pc, fault }),
    }
}

unsafe extern "C" fn cb_event(c: *mut c_void, pc: u32, saved_eip: u32, code: u32, arg: u32) -> u32 {
    let ctx = unsafe { &mut *c.cast::<Ctx>() };
    if code == 14 && !ctx.cpu.is_null() {
        // SAFETY: set by `Generated::run` to the CPU the handler is using;
        // only read, during the handler's own callback.
        ctx.fp_error_x87.push(unsafe { (*ctx.cpu).x87 });
    }
    ctx.emit(HostEvent::Guest { pc, saved_eip, code, arg })
}

unsafe extern "C" fn cb_unsupported(c: *mut c_void, pc: u32, m: *const c_char, d: *const c_char) -> u32 {
    let ctx = unsafe { &mut *c.cast::<Ctx>() };
    let s = |p: *const c_char| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
    ctx.emit(HostEvent::Unsupported { pc, mnemonic: s(m), detail: s(d) })
}

/// The interpreter's TSC stand-in ([`ir::interp::Cpu::tsc`]): each read
/// returns the counter and adds one.
unsafe extern "C" fn cb_rdtsc(c: *mut c_void, _pc: u32) -> u64 {
    let ctx = unsafe { &mut *c.cast::<Ctx>() };
    let t = ctx.tsc;
    ctx.tsc = t.wrapping_add(1);
    t
}

// ---- atomics: h2abi.h v1 contract, after the runtime's atomic.rs ----------

const CF: u32 = 1;
const PF: u32 = 4;
const AF: u32 = 0x10;
const ZF: u32 = 0x40;
const SF: u32 = 0x80;
const OF: u32 = 0x800;
const STATUS: u32 = CF | PF | AF | ZF | SF | OF;

fn status(v: u32, sign: u32) -> u32 {
    (u32::from(v == 0) * ZF) | (u32::from(v & sign != 0) * SF) | (u32::from((v as u8).count_ones().is_multiple_of(2)) * PF)
}

/// `a + b` or `a - b` at `bits`, setting all six status flags.
fn arith(cpu: &mut CpuAbi, a: u32, b: u32, bits: u32, sub: bool) -> u32 {
    let mask = u32::MAX >> (32 - bits);
    let (a, b) = (a & mask, b & mask);
    let r = if sub { a.wrapping_sub(b) } else { a.wrapping_add(b) } & mask;
    let cf = if sub { a < b } else { a as u64 + b as u64 > mask as u64 };
    let sign = 1 << (bits - 1);
    let of = (if sub { (a ^ b) & (a ^ r) } else { !(a ^ b) & (a ^ r) }) & sign != 0;
    let f = u32::from(cf) | (u32::from(of) * OF) | (u32::from((a ^ b ^ r) & 0x10 != 0) * AF) | status(r, sign);
    cpu.eflags = (cpu.eflags & !STATUS) | f;
    cpu.eflags_defined |= STATUS;
    r
}

/// Read the operand (a fault reports the first unmapped byte, as a read).
fn atomic_read(ctx: &mut Ctx, pc: u32, addr: u32, width: u32) -> Result<u32, u32> {
    let mut b = [0u8; 4];
    match ctx.mem.read(addr, &mut b[..width as usize]) {
        Ok(()) => Ok(u32::from_le_bytes(b)),
        Err(fault) => Err(ctx.emit(HostEvent::Memory { pc, fault })),
    }
}

fn atomic_write(ctx: &mut Ctx, pc: u32, addr: u32, width: u32, v: u32) -> Result<(), u32> {
    ctx.mem.write(addr, &v.to_le_bytes()[..width as usize]).map_err(|fault| ctx.emit(HostEvent::Memory { pc, fault }))
}

fn bad_width(ctx: &mut Ctx, pc: u32, addr: u32) -> u32 {
    ctx.emit(HostEvent::Guest { pc, saved_eip: pc, code: 7, arg: addr })
}

/// Run `op` on a copy of the CPU and commit it, the memory write and the
/// old value together, or nothing.
unsafe fn atomic_commit(
    cpu: *mut CpuAbi,
    c: *mut c_void,
    out: *mut u32,
    op: impl FnOnce(&mut Ctx, &mut CpuAbi) -> Result<u32, u32>,
) -> u32 {
    let ctx = unsafe { &mut *c.cast::<Ctx>() };
    let mut next = unsafe { *cpu };
    match op(ctx, &mut next) {
        Ok(old) => {
            unsafe {
                *cpu = next;
                *out = old;
            }
            0
        }
        Err(event) => event,
    }
}

unsafe extern "C" fn cb_rmw(
    cpu: *mut CpuAbi,
    c: *mut c_void,
    pc: u32,
    addr: u32,
    width: u32,
    op: u32,
    operand: u32,
    out: *mut u32,
) -> u32 {
    unsafe {
        atomic_commit(cpu, c, out, |ctx, cpu| {
            ctx.atomics.push(op);
            if !matches!(width, 1 | 2 | 4) || (matches!(op, 12..=14) && width == 1) {
                return Err(bad_width(ctx, pc, addr));
            }
            let bits = width * 8;
            let mask = u32::MAX >> (32 - bits);
            let old = atomic_read(ctx, pc, addr, width)?;
            let new = match op {
                1 => arith(cpu, old, operand, bits, false),
                3 => arith(cpu, old, operand, bits, true),
                5..=7 => {
                    let r = match op {
                        5 => old & operand,
                        6 => old | operand,
                        _ => old ^ operand,
                    } & mask;
                    cpu.eflags = (cpu.eflags & !STATUS) | status(r, 1 << (bits - 1));
                    cpu.eflags_defined = (cpu.eflags_defined | STATUS) & !AF;
                    r
                }
                8 | 9 => {
                    let (cf, cf_defined) = (cpu.eflags & CF, cpu.eflags_defined & CF);
                    let r = arith(cpu, old, 1, bits, op == 9);
                    cpu.eflags = (cpu.eflags & !CF) | cf;
                    cpu.eflags_defined = (cpu.eflags_defined & !CF) | cf_defined;
                    r
                }
                10 => arith(cpu, 0, old, bits, true),
                11 => !old & mask,
                12..=14 => {
                    let bit = 1 << (operand % bits);
                    cpu.eflags = (cpu.eflags & !(CF | OF | SF | AF | PF)) | u32::from(old & bit != 0);
                    cpu.eflags_defined = (cpu.eflags_defined | CF) & !(OF | SF | AF | PF);
                    match op {
                        12 => old | bit,
                        13 => old & !bit,
                        _ => old ^ bit,
                    }
                }
                _ => return Err(bad_width(ctx, pc, addr)),
            };
            atomic_write(ctx, pc, addr, width, new)?;
            Ok(old)
        })
    }
}

unsafe extern "C" fn cb_xchg(
    cpu: *mut CpuAbi,
    c: *mut c_void,
    pc: u32,
    addr: u32,
    width: u32,
    value: u32,
    out: *mut u32,
) -> u32 {
    unsafe {
        atomic_commit(cpu, c, out, |ctx, _| {
            ctx.atomics.push(15);
            if !matches!(width, 1 | 2 | 4) {
                return Err(bad_width(ctx, pc, addr));
            }
            let old = atomic_read(ctx, pc, addr, width)?;
            atomic_write(ctx, pc, addr, width, value)?;
            Ok(old)
        })
    }
}

unsafe extern "C" fn cb_cmpxchg(
    cpu: *mut CpuAbi,
    c: *mut c_void,
    pc: u32,
    addr: u32,
    width: u32,
    replacement: u32,
    out: *mut u32,
) -> u32 {
    unsafe {
        atomic_commit(cpu, c, out, |ctx, cpu| {
            ctx.atomics.push(16);
            if !matches!(width, 1 | 2 | 4) {
                return Err(bad_width(ctx, pc, addr));
            }
            let mask = u32::MAX >> (32 - width * 8);
            let old = atomic_read(ctx, pc, addr, width)?;
            let expected = cpu.gpr[0] & mask;
            arith(cpu, expected, old, width * 8, true);
            if expected == old {
                atomic_write(ctx, pc, addr, width, replacement)?;
            } else {
                // The runtime writes nothing back here. Its permission
                // check covers writing; FlatMemory is writable wherever it
                // is readable, so there is nothing more to check.
                cpu.gpr[0] = (cpu.gpr[0] & !mask) | old;
            }
            Ok(old)
        })
    }
}

/// A loaded generated module.
pub struct Generated {
    _lib: libloading::Library,
    pub entries: HashMap<u32, Handler>,
    /// Continuation PC to (owner handler, start selector).
    pub resumes: HashMap<u32, (Handler, u32)>,
}

impl Generated {
    /// Load a host dylib built with [`BRIDGE`] linked in.
    ///
    /// # Safety
    /// The library must be a module produced by this crate for this host.
    pub unsafe fn load(path: &Path) -> Result<Self, String> {
        let lib = unsafe { libloading::Library::new(path) }.map_err(|e| e.to_string())?;
        unsafe {
            let version: libloading::Symbol<unsafe extern "C" fn() -> u32> =
                lib.get(b"h2gen_module_abi_version").map_err(|e| e.to_string())?;
            if version() != ABI_VERSION {
                return Err(format!("module ABI {} != host {ABI_VERSION}", version()));
            }
            let install: libloading::Symbol<unsafe extern "C" fn(*const Bridge)> =
                lib.get(b"h2bridge_install").map_err(|e| e.to_string())?;
            let bridge = Bridge {
                read: cb_read,
                write: cb_write,
                event: cb_event,
                unsupported: cb_unsupported,
                rdtsc: cb_rdtsc,
                rmw: cb_rmw,
                xchg: cb_xchg,
                cmpxchg: cb_cmpxchg,
            };
            install(&bridge);
            let install_x87: libloading::Symbol<unsafe extern "C" fn(*const *const c_void, u32)> =
                lib.get(b"h2x87bridge_install").map_err(|e| e.to_string())?;
            let fns = x87_helpers();
            install_x87(fns.as_ptr(), fns.len() as u32);
            let module: libloading::Symbol<*const GenModule> =
                lib.get(b"h2gen_module").map_err(|e| e.to_string())?;
            let m = &**module;
            let table = std::slice::from_raw_parts(m.entries, m.entry_count as usize);
            // Every handler's own object must agree on the ABI, as the
            // runtime's register_generated_object checks.
            if let Some(e) = table.iter().find(|e| e.abi_version.is_none_or(|v| v() != ABI_VERSION)) {
                return Err(format!("entry {:#x}: object ABI version mismatch", e.addr));
            }
            let entries: HashMap<u32, Handler> = table.iter().filter_map(|e| Some((e.addr, e.f?))).collect();
            let by_owner: HashMap<u32, Handler> = entries.clone();
            let resumes = std::slice::from_raw_parts(m.resumes, m.resume_count as usize)
                .iter()
                .filter_map(|r| Some((r.pc, (*by_owner.get(&r.owner)?, r.start))))
                .collect();
            Ok(Generated { _lib: lib, entries, resumes })
        }
    }

    /// Run transfers from `start` until one this host can't resolve, a
    /// fault/stop, a return to [`HARNESS_EXIT`], or `max` handler calls.
    /// Returns the final transfer.
    pub fn run(&self, cpu: &mut CpuAbi, ctx: &mut Ctx, start: TransferAbi, max: usize) -> (TransferAbi, usize) {
        let mut t = start;
        for calls in 0..max {
            if matches!(t.kind, FAULT | STOP) || (t.kind == RETURN && t.target_pc == HARNESS_EXIT) {
                return (t, calls);
            }
            let target = t.target_pc;
            let resolved = match t.route {
                ENTRY => self.entries.get(&target).map(|h| (*h, target)),
                // A return only lands on a validated continuation, never on
                // entry dispatch (LIFTER.md §2): a function entry that is
                // also a continuation is registered as a resume.
                RESUME | RETURN_TARGET => self.resumes.get(&target).copied(),
                _ => None,
            };
            let Some((h, selector)) = resolved else { return (t, calls) };
            cpu.eip = target;
            ctx.cpu = cpu;
            // SAFETY: generated handlers follow the ABI; ctx outlives the call.
            t = unsafe { h(cpu, (ctx as *mut Ctx).cast(), selector) };
            ctx.cpu = std::ptr::null();
        }
        (t, max)
    }
}

/// The same initial state as [`lift::harness::run_to_kernel`] starts from.
pub fn entry_state(x: &xbe::Xbe) -> Result<(CpuAbi, Ctx), String> {
    let (cpu, mem) = lift::harness::initial_state(x)?;
    Ok((CpuAbi::from_interp(&cpu), Ctx::new(mem)))
}
