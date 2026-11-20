//! One owner's IR to one Cranelift function (docs/lifter/cranelift-tier.md
//! section 3). The shape follows `emit::c`: registers and flags in
//! variables, one shared exit that spills them and returns the transfer,
//! a selector switch on entry, polls at loop headers.

use std::collections::{BTreeSet, HashMap};

use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{
    self as cir, AbiParam, ArgumentPurpose, Block, InstBuilder, MemFlagsData, Signature, StackSlotData, StackSlotKind,
    TrapCode, Type, types,
};
use cranelift_codegen::isa::CallConv;
use cranelift_frontend::{FunctionBuilder, Switch, Variable};
use cranelift_module::{DataDescription, FuncId, Linkage, Module};
use ir::{
    Addr, Atomic, AtomicOp, BinOp, CastOp, CmpOp, Control, Dest, Expr, Fault, Flag, FlagValue, Gpr, Intrinsic,
    IrFunction, IrInsn, RepKind, Segment, Slice, Stmt, StopReason, Target, Temp, UnOp, Value, Width,
};

use crate::abi::*;

/// Why a function could not be lowered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsupported(pub String);

type R<T> = Result<T, Unsupported>;

fn unsupported<T>(what: impl Into<String>) -> R<T> {
    Err(Unsupported(what.into()))
}

/// Imports a module's functions call, declared once per module.
#[derive(Default)]
pub struct Imports {
    ids: HashMap<String, FuncId>,
}

impl Imports {
    /// The module's id for `name` with `params` (all `i32`/`i64`) and
    /// `ret`, declaring it on first use.
    pub fn get(&mut self, m: &mut dyn Module, name: &str, params: &[Type], ret: Option<Type>) -> FuncId {
        if let Some(id) = self.ids.get(name) {
            return *id;
        }
        let mut sig = m.make_signature();
        sig.params.extend(params.iter().map(|t| AbiParam::new(*t)));
        sig.returns.extend(ret.map(AbiParam::new));
        let id = m.declare_function(name, Linkage::Import, &sig).expect("import declaration");
        self.ids.insert(name.to_owned(), id);
        id
    }
}

/// The handler signature: `TransferAbi h2fn(CpuAbi *, ExecCtxOpaque *,
/// uint32_t)`, with the 28-byte result through a `StructReturn` pointer
/// (x8 on AArch64, the first argument register on x86-64), which is how
/// C returns it.
pub fn handler_signature(cc: CallConv) -> Signature {
    let mut s = Signature::new(cc);
    s.params.push(AbiParam::special(types::I64, ArgumentPurpose::StructReturn));
    s.params.push(AbiParam::new(types::I64));
    s.params.push(AbiParam::new(types::I64));
    s.params.push(AbiParam::new(types::I32));
    s
}

/// Cranelift type of an IR width. W1 is an `i8` holding 0 or 1; W80 sits in
/// the low bits of an `i128`.
pub fn ty(w: Width) -> Type {
    match w {
        Width::W1 | Width::W8 => types::I8,
        Width::W16 => types::I16,
        Width::W32 => types::I32,
        Width::W64 => types::I64,
        Width::W80 | Width::W128 => types::I128,
    }
}

/// Index of a flag in the per-flag variables.
fn fi(f: Flag) -> usize {
    match f {
        Flag::Cf => 0,
        Flag::Pf => 1,
        Flag::Af => 2,
        Flag::Zf => 3,
        Flag::Sf => 4,
        Flag::Df => 5,
        Flag::Of => 6,
    }
}

/// EFLAGS bits with a variable of their own.
const FLAG_BITS: u32 = 0x0cd5;

/// Memory flags for `CpuAbi` and stack slots: never trap, alignment unknown
/// (CpuAbi is 4-aligned, its XMM fields are not 16-aligned).
fn mf() -> MemFlagsData {
    MemFlagsData::new().with_notrap()
}

pub fn shim_code(f: Fault) -> (u32, bool) {
    // (H2SHIM code, trap: saved EIP is the next instruction)
    match f {
        Fault::DivideError => (1, false),
        Fault::Breakpoint => (2, true),
        Fault::InvalidOpcode => (3, false),
        Fault::Interrupt(_) => (4, true),
        Fault::Overflow => (5, true),
        Fault::BoundRange => (6, false),
        Fault::GeneralProtection => (7, false),
        Fault::SimdException => (13, false),
        Fault::FpError => (14, false),
    }
}

fn stop_code(r: StopReason) -> u32 {
    match r {
        StopReason::Halt => SHIM_HALT,
        StopReason::UnknownTarget | StopReason::Unsupported => SHIM_UNKNOWN_TARGET,
        StopReason::DescentError => 11,
        StopReason::Unmodelled => SHIM_UNMODELLED,
    }
}

/// What lowering an owner needs to know besides its IR.
pub struct Shape<'a> {
    pub selectors: &'a BTreeSet<u32>,
    pub headers: &'a BTreeSet<u32>,
}

pub struct Lower<'a, 'b> {
    pub(crate) b: FunctionBuilder<'b>,
    pub(crate) m: &'a mut dyn Module,
    pub(crate) imports: &'a mut Imports,
    refs: HashMap<FuncId, cir::FuncRef>,
    pub(crate) cpu: cir::Value,
    pub(crate) ctx: cir::Value,
    gpr: [Variable; 8],
    flag: [Variable; 7],
    def: [Variable; 7],
    /// EFLAGS bits outside [`FLAG_BITS`], and their defined bits.
    other: Variable,
    other_def: Variable,
    exit: Block,
    /// Guest address to block: IR block starts and interior selectors.
    blocks: HashMap<u32, Block>,
    /// Loop header to its poll block, which local edges go through.
    polls: std::collections::BTreeMap<u32, Block>,
    pub(crate) temps: HashMap<u32, cir::Value>,
    /// In a `Repeat` body: the flag variables' values when it began, which
    /// every exit from the body restores.
    rep_saved: Option<Vec<cir::Value>>,
    /// A 128-byte scratch slot for helper arguments and results.
    pub(crate) scratch: cir::StackSlot,
    /// The x87 failure block: params (status, pc).
    x87_fail: Option<Block>,
    /// Blocks made by [`Lower::dest`] for tail destinations, filled once
    /// the branch that uses them is emitted.
    pending: Vec<(Block, u32)>,
}

impl<'a, 'b> Lower<'a, 'b> {
    pub fn function(
        b: FunctionBuilder<'b>,
        m: &'a mut dyn Module,
        imports: &'a mut Imports,
        f: &IrFunction,
        shape: &Shape<'_>,
        fc: cranelift_codegen::isa::TargetFrontendConfig,
    ) -> R<()> {
        let mut b = b;
        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        let params = b.block_params(entry).to_vec();
        let (sret, cpu, ctx, start) = (params[0], params[1], params[2], params[3]);
        let gpr = std::array::from_fn(|_| b.declare_var(types::I32));
        let flag = std::array::from_fn(|_| b.declare_var(types::I8));
        let def = std::array::from_fn(|_| b.declare_var(types::I8));
        let other = b.declare_var(types::I32);
        let other_def = b.declare_var(types::I32);
        let exit = b.create_block();
        for _ in 0..6 {
            b.append_block_param(exit, types::I32);
        }
        let scratch = b.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 128, 4));
        let mut l = Lower {
            b,
            m,
            imports,
            refs: HashMap::new(),
            cpu,
            ctx,
            gpr,
            flag,
            def,
            other,
            other_def,
            exit,
            blocks: HashMap::new(),
            polls: std::collections::BTreeMap::new(),
            temps: HashMap::new(),
            rep_saved: None,
            scratch,
            x87_fail: None,
            pending: Vec::new(),
        };
        l.reload_all();

        // Blocks: every IR block, and every selector inside one.
        for blk in f.blocks.values() {
            let k = l.b.create_block();
            l.blocks.insert(blk.start, k);
            for (i, insn) in blk.insns.iter().enumerate() {
                if i > 0 && shape.selectors.contains(&insn.pc) {
                    let k = l.b.create_block();
                    l.blocks.insert(insn.pc, k);
                }
            }
            if shape.headers.contains(&blk.start) {
                let p = l.b.create_block();
                l.polls.insert(blk.start, p);
            }
        }

        // The selector switch; a selector for a header enters past its poll.
        let bad = l.b.create_block();
        let mut sw = Switch::new();
        for s in shape.selectors {
            let Some(&k) = l.blocks.get(s) else { return unsupported(format!("selector {s:#x} is not a block")) };
            sw.set_entry(*s as u128, k);
        }
        sw.emit(&mut l.b, start, bad);
        l.b.switch_to_block(bad);
        let ev = l.event(start, start, SHIM_BAD_ENTRY, start);
        l.exit_now(STOP, ENTRY, 0u32.into(), 0u32.into(), ev.into(), start.into());

        // Polls: one load of the context's preempt word.
        for (&h, &p) in &l.polls.clone() {
            l.b.switch_to_block(p);
            let word = l.b.ins().load(types::I64, mf(), l.ctx, 0);
            let v = l.b.ins().load(types::I32, mf(), word, 0);
            let body = l.blocks[&h];
            let leave = l.b.create_block();
            l.b.ins().brif(v, leave, &[], body, &[]);
            l.b.switch_to_block(leave);
            l.exit_now(JUMP, RESUME, h.into(), 0u32.into(), 0u32.into(), h.into());
        }

        for blk in f.blocks.values() {
            l.b.switch_to_block(l.blocks[&blk.start]);
            for (i, insn) in blk.insns.iter().enumerate() {
                if i > 0
                    && let Some(&k) = l.blocks.get(&insn.pc)
                {
                    l.b.ins().jump(k, &[]);
                    l.b.switch_to_block(k);
                }
                l.insn(insn, i + 1 == blk.insns.len())?;
            }
        }

        if let Some(fail) = l.x87_fail {
            l.b.switch_to_block(fail);
            let p = l.b.block_params(fail).to_vec();
            l.x87_fail_body(p[0], p[1]);
        }

        // The exit: spill, EIP, the transfer.
        l.b.switch_to_block(exit);
        let p = l.b.block_params(exit).to_vec();
        l.spill_all();
        l.store_cpu32(OFF_EIP, p[5]);
        let st = |l: &mut Lower, off: i32, v: cir::Value| {
            l.b.ins().store(mf(), v, sret, off);
        };
        let ver = l.b.ins().iconst(types::I32, ABI_VERSION as i64);
        let size = l.b.ins().iconst(types::I32, 28);
        st(&mut l, 0, ver);
        st(&mut l, 4, size);
        for (i, v) in p[..5].iter().enumerate() {
            st(&mut l, 8 + 4 * i as i32, *v);
        }
        l.b.ins().return_(&[]);
        l.b.seal_all_blocks();
        l.b.finalize(fc);
        Ok(())
    }

    // ---- helpers -----------------------------------------------------

    pub(crate) fn iconst(&mut self, t: Type, v: u128) -> cir::Value {
        if t == types::I128 {
            let lo = self.b.ins().iconst(types::I64, v as u64 as i64);
            let hi = self.b.ins().iconst(types::I64, (v >> 64) as u64 as i64);
            self.b.ins().iconcat(lo, hi)
        } else {
            let mask = if t.bits() == 64 { u64::MAX } else { (1u64 << t.bits()) - 1 };
            self.b.ins().iconst(t, (v as u64 & mask) as i64)
        }
    }

    pub(crate) fn i32c(&mut self, v: u32) -> cir::Value {
        self.b.ins().iconst(types::I32, v as i64)
    }

    pub(crate) fn val(&mut self, v: &Value) -> R<cir::Value> {
        match v {
            Value::Const { width, bits } => Ok(self.iconst(ty(*width), *bits)),
            Value::Temp(t) => match self.temps.get(&t.id) {
                Some(v) => Ok(*v),
                None => unsupported(format!("temp t{} used before definition", t.id)),
            },
        }
    }

    pub(crate) fn set(&mut self, t: Temp, v: cir::Value) {
        self.temps.insert(t.id, v);
    }

    /// Mask a computed value to `w`'s significant bits (W1, W80).
    fn fix(&mut self, w: Width, v: cir::Value) -> cir::Value {
        match w {
            Width::W1 => self.b.ins().band_imm_u(v, 1),
            Width::W80 => {
                let m = self.iconst(types::I128, Width::W80.mask());
                self.b.ins().band(v, m)
            }
            _ => v,
        }
    }

    pub(crate) fn call(&mut self, name: &str, params: &[Type], ret: Option<Type>, args: &[cir::Value]) -> Option<cir::Value> {
        let id = self.imports.get(&mut *self.m, name, params, ret);
        let r = match self.refs.get(&id) {
            Some(r) => *r,
            None => {
                let r = self.m.declare_func_in_func(id, self.b.func);
                self.refs.insert(id, r);
                r
            }
        };
        let c = self.b.ins().call(r, args);
        self.b.inst_results(c).first().copied()
    }

    pub(crate) fn load_cpu(&mut self, t: Type, off: i32) -> cir::Value {
        self.b.ins().load(t, mf(), self.cpu, off)
    }

    pub(crate) fn store_cpu32(&mut self, off: i32, v: cir::Value) {
        self.b.ins().store(mf(), v, self.cpu, off);
    }

    pub(crate) fn store_cpu(&mut self, off: i32, v: cir::Value) {
        self.b.ins().store(mf(), v, self.cpu, off);
    }

    pub(crate) fn slot_addr(&mut self, off: i32) -> cir::Value {
        self.b.ins().stack_addr(types::I64, self.scratch, off)
    }

    pub(crate) fn slot_store(&mut self, off: i32, v: cir::Value) {
        self.b.ins().stack_store(types::I64, v, self.scratch, off);
    }

    pub(crate) fn slot_load(&mut self, t: Type, off: i32) -> cir::Value {
        self.b.ins().stack_load(types::I64, t, self.scratch, off)
    }

    /// `h2shim_v1_event(ctx, pc, saved, code, arg)`.
    pub(crate) fn event(&mut self, pc: impl Into<V>, saved: impl Into<V>, code: u32, arg: impl Into<V>) -> cir::Value {
        let pc = self.v(pc.into());
        let saved = self.v(saved.into());
        let code = self.i32c(code);
        let arg = self.v(arg.into());
        let i = types::I32;
        self.call("h2shim_v1_event", &[types::I64, i, i, i, i], Some(i), &[self.ctx, pc, saved, code, arg]).unwrap()
    }

    pub(crate) fn v(&mut self, v: V) -> cir::Value {
        match v {
            V::C(c) => self.i32c(c),
            V::V(v) => v,
        }
    }

    /// Leave now through the exit (the current block ends; a fresh,
    /// unreachable one follows so emission can go on).
    pub(crate) fn exit_now(&mut self, kind: u32, route: u32, target: V, resume: V, event: V, eip: V) {
        self.restore_rep();
        let args: Vec<cir::BlockArg> = [V::C(kind), V::C(route), target, resume, event, eip]
            .into_iter()
            .map(|v| cir::BlockArg::Value(self.v(v)))
            .collect();
        self.b.ins().jump(self.exit, &args);
        let dead = self.b.create_block();
        self.b.switch_to_block(dead);
    }

    /// Run `body` (which must leave) when `cond` is non-zero.
    pub(crate) fn if_leave(&mut self, cond: cir::Value, body: impl FnOnce(&mut Self)) {
        let cold = self.b.create_block();
        let cont = self.b.create_block();
        self.b.set_cold_block(cold);
        self.b.ins().brif(cond, cold, &[], cont, &[]);
        self.b.switch_to_block(cold);
        body(self);
        self.b.switch_to_block(cont);
    }

    fn restore_rep(&mut self) {
        if let Some(saved) = self.rep_saved.clone() {
            for i in 0..7 {
                self.b.def_var(self.flag[i], saved[i]);
                self.b.def_var(self.def[i], saved[7 + i]);
            }
            self.b.def_var(self.other, saved[14]);
            self.b.def_var(self.other_def, saved[15]);
        }
    }

    /// A guest exception at `insn`: event, then FAULT with EIP = pc.
    pub(crate) fn raise(&mut self, f: Fault, insn: &IrInsn) {
        let (code, trap) = shim_code(f);
        let arg = match f {
            Fault::Interrupt(v) => V::C(v as u32),
            Fault::FpError => {
                let s = self.load_cpu(types::I16, OFF_X87_STATUS);
                V::V(self.b.ins().uextend(types::I32, s))
            }
            _ => V::C(0),
        };
        let saved = if trap { insn.next_pc() } else { insn.pc };
        let ev = self.event(insn.pc, saved, code, arg);
        self.exit_now(FAULT, ENTRY, V::C(0), V::C(0), ev.into(), V::C(insn.pc));
    }

    fn halt(&mut self, pc: u32, code: u32, arg: V) {
        let ev = self.event(pc, pc, code, arg);
        self.exit_now(STOP, ENTRY, V::C(0), V::C(0), ev.into(), V::C(pc));
    }

    // ---- architectural state -------------------------------------------

    /// Variables from `CpuAbi`.
    pub(crate) fn reload_all(&mut self) {
        for g in 0..8 {
            let v = self.load_cpu(types::I32, OFF_GPR + 4 * g as i32);
            self.b.def_var(self.gpr[g], v);
        }
        let fl = self.load_cpu(types::I32, OFF_EFLAGS);
        let fd = self.load_cpu(types::I32, OFF_EFLAGS_DEFINED);
        for f in Flag::ALL {
            let i = fi(f);
            let bit = f.eflags_bit() as i64;
            for (var, word) in [(self.flag[i], fl), (self.def[i], fd)] {
                let s = self.b.ins().ushr_imm_u(word, bit);
                let s = self.b.ins().band_imm_u(s, 1);
                let s = self.b.ins().ireduce(types::I8, s);
                self.b.def_var(var, s);
            }
        }
        let o = self.b.ins().band_imm_u(fl, !FLAG_BITS as i64);
        self.b.def_var(self.other, o);
        let o = self.b.ins().band_imm_u(fd, !FLAG_BITS as i64);
        self.b.def_var(self.other_def, o);
    }

    /// EFLAGS (with bit 1 set) and the defined mask from the variables.
    fn eflags(&mut self) -> (cir::Value, cir::Value) {
        let mut fl = self.b.use_var(self.other);
        let mut fd = self.b.use_var(self.other_def);
        for f in Flag::ALL {
            let i = fi(f);
            let bit = f.eflags_bit() as i64;
            for (var, word) in [(self.flag[i], &mut fl), (self.def[i], &mut fd)] {
                let v = self.b.use_var(var);
                let v = self.b.ins().uextend(types::I32, v);
                let v = self.b.ins().ishl_imm_u(v, bit);
                *word = self.b.ins().bor(*word, v);
            }
        }
        let fl = self.b.ins().bor_imm_u(fl, 2);
        (fl, fd)
    }

    /// Variables to `CpuAbi`.
    pub(crate) fn spill_all(&mut self) {
        for g in 0..8 {
            let v = self.b.use_var(self.gpr[g]);
            self.store_cpu32(OFF_GPR + 4 * g as i32, v);
        }
        let (fl, fd) = self.eflags();
        self.store_cpu32(OFF_EFLAGS, fl);
        self.store_cpu32(OFF_EFLAGS_DEFINED, fd);
    }

    fn read_reg(&mut self, g: Gpr, s: Slice) -> cir::Value {
        let r = self.b.use_var(self.gpr[g.index()]);
        match s {
            Slice::Full => r,
            Slice::Low16 => self.b.ins().ireduce(types::I16, r),
            Slice::Low8 => self.b.ins().ireduce(types::I8, r),
            Slice::High8 => {
                let h = self.b.ins().ushr_imm_u(r, 8);
                self.b.ins().ireduce(types::I8, h)
            }
        }
    }

    fn write_reg(&mut self, g: Gpr, s: Slice, v: cir::Value) {
        let var = self.gpr[g.index()];
        let v32 = match self.b.func.dfg.value_type(v) {
            types::I32 => v,
            t if t.bits() < 32 => self.b.ins().uextend(types::I32, v),
            _ => self.b.ins().ireduce(types::I32, v),
        };
        let new = match s {
            Slice::Full => v32,
            _ => {
                let (keep, low_mask, shift) = match s {
                    Slice::Low16 => (0xffff_0000u32, 0xffffu32, 0),
                    Slice::Low8 => (0xffff_ff00, 0xff, 0),
                    Slice::High8 => (0xffff_00ff, 0xff, 8),
                    Slice::Full => unreachable!(),
                };
                let old = self.b.use_var(var);
                let old = self.b.ins().band_imm_u(old, keep as i64);
                let nv = self.b.ins().band_imm_u(v32, low_mask as i64);
                let nv = if shift != 0 { self.b.ins().ishl_imm_u(nv, shift) } else { nv };
                self.b.ins().bor(old, nv)
            }
        };
        self.b.def_var(var, new);
    }

    fn write_flag(&mut self, f: Flag, v: Option<cir::Value>) {
        let i = fi(f);
        let (v, d) = match v {
            Some(v) => (v, 1),
            None => (self.b.ins().iconst(types::I8, 0), 0),
        };
        let d = self.b.ins().iconst(types::I8, d);
        self.b.def_var(self.flag[i], v);
        self.b.def_var(self.def[i], d);
    }

    fn write_eflags(&mut self, value: cir::Value, mask: u32) {
        for f in Flag::ALL {
            if mask & (1 << f.eflags_bit()) != 0 {
                let s = self.b.ins().ushr_imm_u(value, f.eflags_bit() as i64);
                let s = self.b.ins().band_imm_u(s, 1);
                let s = self.b.ins().ireduce(types::I8, s);
                self.write_flag(f, Some(s));
            }
        }
        let m = mask & !FLAG_BITS;
        if m != 0 {
            let o = self.b.use_var(self.other);
            let o = self.b.ins().band_imm_u(o, !m as i64);
            let n = self.b.ins().band_imm_u(value, m as i64);
            let o = self.b.ins().bor(o, n);
            self.b.def_var(self.other, o);
            let d = self.b.use_var(self.other_def);
            let d = self.b.ins().bor_imm_u(d, m as i64);
            self.b.def_var(self.other_def, d);
        }
    }

    fn addr(&mut self, a: &Addr) -> R<cir::Value> {
        let off = self.val(&a.offset)?;
        Ok(match a.seg {
            Segment::Flat => off,
            Segment::Fs => {
                let base = self.load_cpu(types::I32, OFF_FS_BASE);
                self.b.ins().iadd(base, off)
            }
        })
    }

    // ---- expressions ---------------------------------------------------

    fn expr(&mut self, dst: Temp, e: &Expr) -> R<cir::Value> {
        let w = dst.width;
        let t = ty(w);
        Ok(match e {
            Expr::Unary { op, a } => {
                let x = self.val(a)?;
                let aw = a.width();
                match op {
                    UnOp::Not => {
                        let r = self.b.ins().bnot(x);
                        self.fix(w, r)
                    }
                    UnOp::Neg => {
                        let r = self.b.ins().ineg(x);
                        self.fix(w, r)
                    }
                    UnOp::Parity => {
                        let x = if ty(aw) == types::I8 { x } else { self.b.ins().ireduce(types::I8, x) };
                        let p = self.b.ins().popcnt(x);
                        let p = self.b.ins().bxor_imm_u(p, 1);
                        self.b.ins().band_imm_u(p, 1)
                    }
                    UnOp::Ctz => self.b.ins().ctz(x),
                    UnOp::Clz => self.b.ins().clz(x),
                    UnOp::Bswap => match aw {
                        Width::W8 | Width::W1 => x,
                        Width::W16 | Width::W32 | Width::W64 | Width::W128 => self.b.ins().bswap(x),
                        Width::W80 => return unsupported("bswap of an 80-bit value"),
                    },
                }
            }
            Expr::Binary { op, a, b } => {
                let x = self.val(a)?;
                let y = self.val(b)?;
                let bits = a.width().bits();
                let tt = ty(a.width());
                match op {
                    BinOp::Add => {
                        let r = self.b.ins().iadd(x, y);
                        self.fix(w, r)
                    }
                    BinOp::Sub => {
                        let r = self.b.ins().isub(x, y);
                        self.fix(w, r)
                    }
                    BinOp::Mul => {
                        let r = self.b.ins().imul(x, y);
                        self.fix(w, r)
                    }
                    BinOp::And => self.b.ins().band(x, y),
                    BinOp::Or => self.b.ins().bor(x, y),
                    BinOp::Xor => self.b.ins().bxor(x, y),
                    BinOp::Shl | BinOp::Shr => {
                        let r = if *op == BinOp::Shl { self.b.ins().ishl(x, y) } else { self.b.ins().ushr(x, y) };
                        let big = self.ge_const(y, bits);
                        let z = self.iconst(tt, 0);
                        let r = self.b.ins().select(big, z, r);
                        self.fix(w, r)
                    }
                    BinOp::Sar => {
                        let big = self.ge_const(y, bits);
                        let top = self.iconst(tt, bits as u128 - 1);
                        let k = self.b.ins().select(big, top, y);
                        if w == Width::W80 {
                            return unsupported("80-bit arithmetic shift");
                        }
                        self.b.ins().sshr(x, k)
                    }
                    BinOp::Rol | BinOp::Ror => {
                        if !bits.is_power_of_two() || bits < 8 {
                            return unsupported(format!("rotate at {bits} bits"));
                        }
                        if *op == BinOp::Rol { self.b.ins().rotl(x, y) } else { self.b.ins().rotr(x, y) }
                    }
                    BinOp::Udiv | BinOp::Urem | BinOp::Sdiv | BinOp::Srem => {
                        if tt == types::I128 {
                            return unsupported("128-bit division");
                        }
                        self.div(*op, x, y, tt)
                    }
                }
            }
            Expr::Compare { op, a, b } => {
                let x = self.val(a)?;
                let y = self.val(b)?;
                let cc = match op {
                    CmpOp::Eq => IntCC::Equal,
                    CmpOp::Ne => IntCC::NotEqual,
                    CmpOp::Ult => IntCC::UnsignedLessThan,
                    CmpOp::Ule => IntCC::UnsignedLessThanOrEqual,
                    CmpOp::Slt => IntCC::SignedLessThan,
                    CmpOp::Sle => IntCC::SignedLessThanOrEqual,
                };
                if a.width() == Width::W1 && matches!(op, CmpOp::Slt | CmpOp::Sle) {
                    return unsupported("signed compare of W1 values");
                }
                self.b.ins().icmp(cc, x, y)
            }
            Expr::Cast { op, a } => {
                let x = self.val(a)?;
                let aw = a.width();
                let at = ty(aw);
                match op {
                    CastOp::Zext => {
                        if at == t {
                            x
                        } else {
                            self.b.ins().uextend(t, x)
                        }
                    }
                    CastOp::Sext => {
                        if aw == Width::W1 {
                            let n = self.b.ins().ineg(x);
                            if t == types::I8 { n } else { self.b.ins().sextend(t, n) }
                        } else if aw == Width::W80 {
                            // From bit 79 into the i128.
                            let s = self.b.ins().ishl_imm_u(x, 48);
                            self.b.ins().sshr_imm_u(s, 48)
                        } else if at == t {
                            x
                        } else {
                            let r = self.b.ins().sextend(t, x);
                            self.fix(w, r)
                        }
                    }
                    CastOp::Trunc => {
                        let r = if at == t { x } else { self.b.ins().ireduce(t, x) };
                        self.fix(w, r)
                    }
                }
            }
            Expr::Select { cond, a, b } => {
                let c = self.val(cond)?;
                let x = self.val(a)?;
                let y = self.val(b)?;
                self.b.ins().select(c, x, y)
            }
        })
    }

    /// `v >= n` (unsigned) as an `i8`, at `v`'s type.
    fn ge_const(&mut self, v: cir::Value, n: u32) -> cir::Value {
        let t = self.b.func.dfg.value_type(v);
        if t.bits() < 64 && n as u64 >= 1u64 << t.bits() {
            return self.b.ins().iconst(types::I8, 0);
        }
        let c = self.iconst(t, n as u128);
        self.b.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, v, c)
    }

    /// The IR's total division: no host trap is reachable.
    fn div(&mut self, op: BinOp, x: cir::Value, y: cir::Value, t: Type) -> cir::Value {
        let zero = self.b.ins().icmp_imm_u(IntCC::Equal, y, 0);
        let one = self.b.ins().iconst(t, 1);
        let z = self.b.ins().iconst(t, 0);
        match op {
            BinOp::Udiv | BinOp::Urem => {
                let d = self.b.ins().select(zero, one, y);
                if op == BinOp::Udiv {
                    let q = self.b.ins().udiv(x, d);
                    self.b.ins().select(zero, z, q)
                } else {
                    let r = self.b.ins().urem(x, d);
                    self.b.ins().select(zero, x, r)
                }
            }
            _ => {
                let m1 = self.b.ins().icmp_imm_s(IntCC::Equal, y, -1);
                let either = self.b.ins().bor(zero, m1);
                let d = self.b.ins().select(either, one, y);
                if op == BinOp::Sdiv {
                    let q = self.b.ins().sdiv(x, d);
                    let n = self.b.ins().ineg(x);
                    let q = self.b.ins().select(m1, n, q);
                    self.b.ins().select(zero, z, q)
                } else {
                    let r = self.b.ins().srem(x, d);
                    let r = self.b.ins().select(m1, z, r);
                    self.b.ins().select(zero, x, r)
                }
            }
        }
    }

    // ---- statements ----------------------------------------------------

    fn insn(&mut self, insn: &IrInsn, last: bool) -> R<()> {
        self.temps.clear();
        match &insn.atomic {
            Some(a) => self.atomic(insn, a)?,
            None => self.stmts(&insn.stmts, insn)?,
        }
        self.control(insn, last)
    }

    fn stmts(&mut self, list: &[Stmt], insn: &IrInsn) -> R<()> {
        for s in list {
            self.stmt(s, insn)?;
        }
        Ok(())
    }

    fn stmt(&mut self, s: &Stmt, insn: &IrInsn) -> R<()> {
        let pc = insn.pc;
        match s {
            Stmt::Assign { dst, expr } => {
                let v = self.expr(*dst, expr)?;
                self.set(*dst, v);
            }
            Stmt::ReadReg { dst, reg, slice } => {
                let v = self.read_reg(*reg, *slice);
                self.set(*dst, v);
            }
            Stmt::WriteReg { reg, slice, value } => {
                let v = self.val(value)?;
                self.write_reg(*reg, *slice, v);
            }
            Stmt::ReadFlag { dst, flag } => {
                let v = self.b.use_var(self.flag[fi(*flag)]);
                self.set(*dst, v);
            }
            Stmt::WriteFlag { flag, value } => match value {
                FlagValue::Defined(v) => {
                    let v = self.val(v)?;
                    self.write_flag(*flag, Some(v));
                }
                FlagValue::Undefined => self.write_flag(*flag, None),
            },
            Stmt::ReadEflags { dst } => {
                let (fl, _) = self.eflags();
                self.set(*dst, fl);
            }
            Stmt::WriteEflags { value, mask } => {
                let v = self.val(value)?;
                self.write_eflags(v, *mask);
            }
            Stmt::ReadXmm { dst, reg } => {
                let v = self.load_cpu(types::I128, OFF_XMM + 16 * *reg as i32);
                self.set(*dst, v);
            }
            Stmt::WriteXmm { reg, value } => {
                let v = self.val(value)?;
                self.store_cpu(OFF_XMM + 16 * *reg as i32, v);
            }
            Stmt::ReadMmx { dst, reg } => {
                let v = self.load_cpu(types::I64, OFF_X87_ST + 10 * *reg as i32);
                self.set(*dst, v);
            }
            Stmt::WriteMmx { reg, value } => {
                let v = self.val(value)?;
                let off = OFF_X87_ST + 10 * *reg as i32;
                self.store_cpu(off, v);
                let ones = self.b.ins().iconst(types::I16, 0xffff);
                self.store_cpu(off + 8, ones);
            }
            Stmt::Load { dst, addr } => {
                let a = self.addr(addr)?;
                let v = self.guest_load(pc, a, dst.width)?;
                self.set(*dst, v);
            }
            Stmt::Store { addr, value } => {
                let a = self.addr(addr)?;
                let v = self.val(value)?;
                self.guest_store(pc, a, value.width(), v)?;
            }
            Stmt::FaultIf { cond, fault } => {
                let c = self.val(cond)?;
                let fault = *fault;
                self.if_leave(c, |l| l.raise(fault, insn));
            }
            Stmt::StopIf { cond, reason } => {
                let c = self.val(cond)?;
                let code = stop_code(*reason);
                self.if_leave(c, |l| l.halt(pc, code, V::C(0)));
            }
            Stmt::If { cond, then, otherwise } => {
                let c = self.val(cond)?;
                let (t, e, j) = (self.b.create_block(), self.b.create_block(), self.b.create_block());
                self.b.ins().brif(c, t, &[], e, &[]);
                self.b.switch_to_block(t);
                self.stmts(then, insn)?;
                self.b.ins().jump(j, &[]);
                self.b.switch_to_block(e);
                self.stmts(otherwise, insn)?;
                self.b.ins().jump(j, &[]);
                self.b.switch_to_block(j);
            }
            Stmt::Intrinsic { op, args, dsts } => self.intrinsic(*op, args, dsts, insn)?,
            Stmt::Repeat { kind, body } => {
                let mut saved = Vec::with_capacity(16);
                for i in 0..7 {
                    saved.push(self.b.use_var(self.flag[i]));
                }
                for i in 0..7 {
                    saved.push(self.b.use_var(self.def[i]));
                }
                saved.push(self.b.use_var(self.other));
                saved.push(self.b.use_var(self.other_def));
                let (head, body_b, after) = (self.b.create_block(), self.b.create_block(), self.b.create_block());
                self.b.ins().jump(head, &[]);
                self.b.switch_to_block(head);
                let ecx = self.b.use_var(self.gpr[Gpr::Ecx.index()]);
                self.b.ins().brif(ecx, body_b, &[], after, &[]);
                self.b.switch_to_block(body_b);
                let outer = self.rep_saved.replace(saved);
                self.stmts(body, insn)?;
                self.rep_saved = outer;
                let ecx = self.b.use_var(self.gpr[Gpr::Ecx.index()]);
                let ecx = self.b.ins().iadd_imm_s(ecx, -1);
                self.b.def_var(self.gpr[Gpr::Ecx.index()], ecx);
                match kind {
                    RepKind::Rep => {
                        self.b.ins().jump(head, &[]);
                    }
                    RepKind::RepE | RepKind::RepNe => {
                        let zf = self.b.use_var(self.flag[fi(Flag::Zf)]);
                        // REPE stops when ZF is clear, REPNE when it is set.
                        if *kind == RepKind::RepE {
                            self.b.ins().brif(zf, head, &[], after, &[]);
                        } else {
                            self.b.ins().brif(zf, after, &[], head, &[]);
                        }
                    }
                }
                self.b.switch_to_block(after);
            }
            Stmt::LazyFlags(lz) => {
                // Eager: the formula's flags, here. Dead ones fold away.
                let args = lz.args.iter().map(|a| self.val(a)).collect::<R<Vec<_>>>()?;
                let outer = std::mem::take(&mut self.temps);
                for (i, a) in args.into_iter().enumerate() {
                    self.temps.insert(i as u32, a);
                }
                let mut values = Vec::new();
                let fm = lz.formula.clone();
                for (t, e) in &fm.body {
                    let v = self.expr(*t, e)?;
                    self.set(*t, v);
                }
                for (f, v) in &fm.flags {
                    values.push((*f, self.val(v)?));
                }
                self.temps = outer;
                for (f, v) in values {
                    self.write_flag(f, Some(v));
                }
            }
            Stmt::Unsupported { mnemonic, detail } => {
                let mn = self.cstring(mnemonic);
                let de = self.cstring(detail);
                let p = self.i32c(pc);
                let i = types::I32;
                let ev = self
                    .call("h2shim_v1_unsupported", &[types::I64, i, types::I64, types::I64], Some(i), &[self.ctx, p, mn, de])
                    .unwrap();
                self.exit_now(STOP, ENTRY, V::C(0), V::C(0), ev.into(), V::C(pc));
            }
        }
        Ok(())
    }

    /// A NUL-terminated constant string in the object.
    fn cstring(&mut self, s: &str) -> cir::Value {
        let mut bytes: Vec<u8> =
            s.bytes().map(|c| if c.is_ascii_graphic() || c == b' ' { c } else { b'?' }).collect();
        bytes.push(0);
        let id = self.m.declare_anonymous_data(false, false).expect("string data");
        let mut d = DataDescription::new();
        d.define(bytes.into_boxed_slice());
        self.m.define_data(id, &d).expect("string data");
        let gv = self.m.declare_data_in_func(id, self.b.func);
        self.b.ins().symbol_value(types::I64, gv)
    }

    /// A checked guest load through `h2abi_v1_read`.
    fn guest_load(&mut self, pc: u32, a: cir::Value, w: Width) -> R<cir::Value> {
        let n = w.bytes();
        if n == 0 {
            return unsupported("load of a W1 value");
        }
        if w == Width::W80 {
            let z = self.iconst(types::I128, 0);
            self.slot_store(0, z);
        }
        let buf = self.slot_addr(0);
        let (p, nn, one) = (self.i32c(pc), self.i32c(n), self.i32c(1));
        let i = types::I32;
        let st = self
            .call("h2abi_v1_read", &[types::I64, i, i, i, i, types::I64], Some(i), &[self.ctx, p, a, nn, one, buf])
            .unwrap();
        self.if_leave(st, |l| l.exit_now(FAULT, ENTRY, V::C(0), V::C(0), st.into(), V::C(pc)));
        Ok(self.slot_load(ty(w), 0))
    }

    fn guest_store(&mut self, pc: u32, a: cir::Value, w: Width, v: cir::Value) -> R<()> {
        let n = w.bytes();
        if n == 0 {
            return unsupported("store of a W1 value");
        }
        self.slot_store(0, v);
        let buf = self.slot_addr(0);
        let (p, nn, one) = (self.i32c(pc), self.i32c(n), self.i32c(1));
        let i = types::I32;
        let st = self
            .call("h2abi_v1_write", &[types::I64, i, i, i, i, types::I64], Some(i), &[self.ctx, p, a, nn, one, buf])
            .unwrap();
        self.if_leave(st, |l| l.exit_now(FAULT, ENTRY, V::C(0), V::C(0), st.into(), V::C(pc)));
        Ok(())
    }

    fn atomic(&mut self, insn: &IrInsn, a: &Atomic) -> R<()> {
        let pc = insn.pc;
        let li = insn.stmts.iter().position(|s| matches!(s, Stmt::Load { .. })).expect("validated atomic shape");
        let Stmt::Load { dst, addr } = &insn.stmts[li] else { unreachable!() };
        self.stmts(&insn.stmts[..li], insn)?;
        let addr = self.addr(addr)?;
        let n = dst.width.bytes();
        let operand = match a.operand {
            Some(v) => {
                let x = self.val(&v)?;
                match self.b.func.dfg.value_type(x) {
                    types::I32 => x,
                    t if t.bits() < 32 => self.b.ins().uextend(types::I32, x),
                    _ => return unsupported("atomic operand wider than 32 bits"),
                }
            }
            None => self.i32c(0),
        };
        self.spill_all();
        let out = self.slot_addr(0);
        let (p, nn) = (self.i32c(pc), self.i32c(n));
        let (i, q) = (types::I32, types::I64);
        let st = match a.op {
            AtomicOp::Xchg | AtomicOp::Cmpxchg => {
                let name = if a.op == AtomicOp::Xchg { "h2abi_v1_xchg" } else { "h2abi_v1_cmpxchg" };
                self.call(name, &[q, q, i, i, i, i, q], Some(i), &[self.cpu, self.ctx, p, addr, nn, operand, out])
            }
            op => {
                let tag = self.i32c(op.rmw_tag().expect("read-modify-write op"));
                self.call(
                    "h2abi_v1_atomic_rmw",
                    &[q, q, i, i, i, i, i, q],
                    Some(i),
                    &[self.cpu, self.ctx, p, addr, nn, tag, operand, out],
                )
            }
        }
        .unwrap();
        self.if_leave(st, |l| l.exit_now(FAULT, ENTRY, V::C(0), V::C(0), st.into(), V::C(pc)));
        self.reload_all();
        if let Some((g, s)) = a.old {
            let old = self.slot_load(types::I32, 0);
            self.write_reg(g, s, old);
        }
        Ok(())
    }

    fn intrinsic(&mut self, op: Intrinsic, args: &[Value], dsts: &[Temp], insn: &IrInsn) -> R<()> {
        let pc = insn.pc;
        match op {
            Intrinsic::X87ReadControl => {
                let v = self.load_cpu(types::I16, OFF_X87_CONTROL);
                self.set(dsts[0], v);
            }
            Intrinsic::X87ReadStatus => {
                let v = self.load_cpu(types::I16, OFF_X87_STATUS);
                self.set(dsts[0], v);
            }
            Intrinsic::MxcsrRead => {
                let v = self.load_cpu(types::I32, OFF_MXCSR);
                self.set(dsts[0], v);
            }
            Intrinsic::MxcsrWrite => {
                let v = self.val(&args[0])?;
                let bad = self.b.ins().band_imm_u(v, !MXCSR_MASK as i64);
                self.if_leave(bad, |l| l.raise(Fault::GeneralProtection, insn));
                self.store_cpu32(OFF_MXCSR, v);
            }
            Intrinsic::MmxEnter => {
                let s = self.load_cpu(types::I16, OFF_X87_STATUS);
                let s = self.b.ins().band_imm_u(s, !0x3800i64 & 0xffff);
                self.store_cpu(OFF_X87_STATUS, s);
                let z = self.b.ins().iconst(types::I16, 0);
                self.store_cpu(OFF_X87_TAG, z);
            }
            Intrinsic::Emms | Intrinsic::X87Wait => {
                self.fp_wait(insn);
                if op == Intrinsic::Emms {
                    let t = self.b.ins().iconst(types::I16, 0xffff);
                    self.store_cpu(OFF_X87_TAG, t);
                }
            }
            Intrinsic::ReadTsc => {
                let p = self.i32c(pc);
                let v = self.call("h2shim_v1_rdtsc", &[types::I64, types::I32], Some(types::I64), &[self.ctx, p]).unwrap();
                self.set(dsts[0], v);
            }
            Intrinsic::Sse(s) => {
                for (k, a) in args.iter().enumerate() {
                    let v = self.val(a)?;
                    self.slot_store(16 + 16 * k as i32, v);
                }
                let ap = self.slot_addr(16);
                let op_ = self.slot_addr(0);
                let name = format!("h2clif_sse_{}", s.name());
                let q = types::I64;
                let st = self.call(&name, &[q, q, q], Some(types::I32), &[self.cpu, ap, op_]).unwrap();
                self.if_leave(st, |l| raise_status(l, st, pc));
                let v = self.slot_load(ty(dsts[0].width), 0);
                self.set(dsts[0], v);
            }
            Intrinsic::X87(x) => self.x87(x, args, dsts, insn)?,
        }
        Ok(())
    }

    /// `fwait`: #MF with the status word when an exception is pending.
    fn fp_wait(&mut self, insn: &IrInsn) {
        let s = self.load_cpu(types::I16, OFF_X87_STATUS);
        let es = self.b.ins().band_imm_u(s, 0x80);
        self.if_leave(es, |l| l.raise(Fault::FpError, insn));
    }

    /// The x87 failure block's target: params (status, pc).
    pub(crate) fn x87_fail_block(&mut self) -> Block {
        if let Some(b) = self.x87_fail {
            return b;
        }
        let b = self.b.create_block();
        self.b.append_block_param(b, types::I32);
        self.b.append_block_param(b, types::I32);
        self.b.set_cold_block(b);
        self.x87_fail = Some(b);
        b
    }

    /// `h2_x87_fail`: #MF as event 14 with the status word, anything else
    /// a stop with code 12.
    fn x87_fail_body(&mut self, status: cir::Value, pc: cir::Value) {
        let top = self.b.ins().band_imm_u(status, X87_STATUS_MASK as i64);
        let mf_ = self.b.ins().icmp_imm_u(IntCC::Equal, top, X87_MF as i64);
        let (a, b) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(mf_, a, &[], b, &[]);
        self.b.switch_to_block(a);
        let sw = self.load_cpu(types::I16, OFF_X87_STATUS);
        let sw = self.b.ins().uextend(types::I32, sw);
        let ev = self.event(pc, pc, 14, sw);
        self.exit_now(FAULT, ENTRY, V::C(0), V::C(0), ev.into(), pc.into());
        self.b.ins().trap(TrapCode::user(1).unwrap());
        self.b.switch_to_block(b);
        let ev = self.event(pc, pc, SHIM_UNMODELLED, 0u32);
        self.exit_now(STOP, ENTRY, V::C(0), V::C(0), ev.into(), pc.into());
        self.b.ins().trap(TrapCode::user(1).unwrap());
    }

    // ---- control -------------------------------------------------------

    /// The block a local destination enters (a header's poll), or a fresh
    /// block that leaves for a tail destination.
    fn dest(&mut self, d: Dest) -> R<Block> {
        if !d.tail {
            if let Some(p) = self.polls.get(&d.pc) {
                return Ok(*p);
            }
            return match self.blocks.get(&d.pc) {
                Some(b) => Ok(*b),
                None => unsupported(format!("local destination {:#x} is not a block", d.pc)),
            };
        }
        let k = self.b.create_block();
        self.pending.push((k, d.pc));
        Ok(k)
    }

    /// Fill the tail blocks [`Lower::dest`] made, after the branch that
    /// uses them ended the current block.
    fn flush_pending(&mut self) {
        for (k, pc) in std::mem::take(&mut self.pending) {
            self.b.switch_to_block(k);
            self.exit_now(JUMP, ENTRY, V::C(pc), V::C(0), V::C(0), V::C(pc));
        }
        let dead = self.b.create_block();
        self.b.switch_to_block(dead);
    }

    fn target(&mut self, t: &Target) -> R<V> {
        Ok(match t {
            Target::Direct(a) => V::C(*a),
            Target::Dynamic(v) => V::V(self.val(v)?),
        })
    }

    fn xfer(&mut self, kind: u32, route: u32, target: V, resume: u32) {
        // The transfer's EIP is its target.
        let t = self.v(target);
        self.exit_now(kind, route, V::V(t), V::C(resume), V::C(0), V::V(t));
    }

    fn control(&mut self, insn: &IrInsn, last: bool) -> R<()> {
        let pc = insn.pc;
        match &insn.control {
            Control::Next => {
                if !last {
                    return Ok(());
                }
                let k = self.dest(Dest::local(insn.next_pc()))?;
                self.b.ins().jump(k, &[]);
                let dead = self.b.create_block();
                self.b.switch_to_block(dead);
            }
            Control::Jump { target: Target::Direct(t), tail: false, .. } => {
                let k = self.dest(Dest::local(*t))?;
                self.b.ins().jump(k, &[]);
                let dead = self.b.create_block();
                self.b.switch_to_block(dead);
            }
            Control::Jump { target, .. } => {
                let t = self.target(target)?;
                self.xfer(JUMP, ENTRY, t, 0);
            }
            Control::Branch { cond, taken, fallthrough } => {
                let c = self.val(cond)?;
                let t = self.dest(*taken)?;
                let f = self.dest(*fallthrough)?;
                self.b.ins().brif(c, t, &[], f, &[]);
                self.flush_pending();
            }
            Control::Switch { target, cases, .. } => {
                let v = self.val(target)?;
                let mut sw = Switch::new();
                let mut seen = BTreeSet::new();
                for c in cases {
                    if seen.insert(c.pc) {
                        let k = self.dest(*c)?;
                        sw.set_entry(c.pc as u128, k);
                    }
                }
                let default = self.b.create_block();
                sw.emit(&mut self.b, v, default);
                self.flush_pending();
                self.b.switch_to_block(default);
                self.halt(pc, SHIM_UNKNOWN_TARGET, V::V(v));
            }
            Control::Call { target, continuation, .. } => {
                let t = self.target(target)?;
                self.xfer(CALL, ENTRY, t, *continuation);
            }
            Control::Return { target, .. } => {
                let t = self.val(target)?;
                self.xfer(RETURN, RETURN_TARGET, V::V(t), 0);
            }
            Control::Fault(f) => self.raise(*f, insn),
            // The Unsupported statement has already stopped.
            Control::Stop(StopReason::Unsupported) => {}
            Control::Stop(r) => self.halt(pc, stop_code(*r), V::C(0)),
        }
        // Whatever follows a transfer is unreachable: close the dead block
        // (code after an unsupported instruction may have gone into it).
        if last {
            self.b.ins().trap(TrapCode::user(1).unwrap());
        }
        Ok(())
    }
}

/// An SSE helper's non-zero status is the `H2SHIM_*` fault to raise, with
/// argument 0 (SSE never raises #MF).
fn raise_status(l: &mut Lower<'_, '_>, code: cir::Value, pc: u32) {
    let p = l.i32c(pc);
    let z = l.i32c(0);
    let i = types::I32;
    let ev = l.call("h2shim_v1_event", &[types::I64, i, i, i, i], Some(i), &[l.ctx, p, p, code, z]).unwrap();
    l.exit_now(FAULT, ENTRY, V::C(0), V::C(0), ev.into(), V::C(pc));
}

/// A constant or a computed `i32`.
#[derive(Clone, Copy)]
pub enum V {
    C(u32),
    V(cir::Value),
}

impl From<u32> for V {
    fn from(c: u32) -> V {
        V::C(c)
    }
}

impl From<cir::Value> for V {
    fn from(v: cir::Value) -> V {
        V::V(v)
    }
}
