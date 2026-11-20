//! [`Intrinsic::X87`](ir::Intrinsic::X87) as calls to the `h2x87_v1_*`
//! helpers (`crates/recompiler/x87`, the authority the C tier's native path is proved
//! equal to), after `emit::x87` and the `H2X87*` macros in `h2gen.h`.
//!
//! A helper status with any of the top byte set leaves through the
//! handler's x87 failure block (#MF as event 14, else stop 12). `Mark`
//! writes FIP/FOP/FCS (and FDP/FDS) to `X87Abi` directly, so it is current
//! at every helper call and exit, which is what the C's locals and spills
//! amount to.

use cranelift_codegen::ir::{self as cir, InstBuilder, Type, condcodes::IntCC, types};
use ir::x87::{X87Format, X87Op, X87Src};
use ir::{IrInsn, Temp, Value};

use crate::abi::*;
use crate::lower::{Lower, Unsupported};

const I: Type = types::I32;
const Q: Type = types::I64;

impl Lower<'_, '_> {
    fn x87_state(&mut self) -> cir::Value {
        self.b.ins().iadd_imm_u(self.cpu, OFF_X87 as i64)
    }

    /// Call `h2x87_v1_<name>(X87Abi *, args...)` and check its status.
    fn x87_call(&mut self, name: &str, params: &[Type], args: &[cir::Value]) -> cir::Value {
        let s = self.x87_state();
        let mut p = vec![Q];
        p.extend_from_slice(params);
        let mut a = vec![s];
        a.extend_from_slice(args);
        self.call(&format!("h2x87_v1_{name}"), &p, Some(I), &a).unwrap()
    }

    /// `H2X87(pc, status)`: leave through the failure block unless OK.
    fn x87_check(&mut self, status: cir::Value, pc: u32) {
        let bad = self.b.ins().band_imm_u(status, X87_STATUS_MASK as i64);
        let fail = self.x87_fail_block();
        let cont = self.b.create_block();
        let p = self.i32c(pc);
        self.b.ins().brif(bad, fail, &[cir::BlockArg::Value(status), cir::BlockArg::Value(p)], cont, &[]);
        self.b.switch_to_block(cont);
    }

    /// The memory-source bits as the helpers' `uint64_t src`, or the
    /// register index.
    fn x87_src(&mut self, src: X87Src, args: &[Value]) -> Result<cir::Value, Unsupported> {
        Ok(match src {
            X87Src::St(i) => self.b.ins().iconst(Q, i as i64),
            _ => {
                let v = self.val(&args[0])?;
                if self.b.func.dfg.value_type(v) == Q { v } else { self.b.ins().uextend(Q, v) }
            }
        })
    }

    fn zero_scratch(&mut self, bytes: i32) {
        let z = self.iconst(types::I128, 0);
        let mut off = 0;
        while off < bytes {
            self.slot_store(off, z);
            off += 16;
        }
    }

    pub(crate) fn x87(&mut self, op: X87Op, args: &[Value], dsts: &[Temp], insn: &IrInsn) -> Result<(), Unsupported> {
        let pc = insn.pc;
        match op {
            X87Op::Mark { fop, mem } => {
                let p = self.i32c(pc);
                self.store_cpu32(OFF_X87 + X87_FIP, p);
                let f = self.b.ins().iconst(types::I16, fop as i64);
                self.store_cpu(OFF_X87 + X87_FOP, f);
                let cs = self.b.ins().iconst(types::I16, GUEST_CS as i64);
                self.store_cpu(OFF_X87 + X87_FCS, cs);
                if mem {
                    let ea = self.val(&args[0])?;
                    self.store_cpu32(OFF_X87 + X87_FDP, ea);
                    let ds = self.b.ins().iconst(types::I16, GUEST_DS as i64);
                    self.store_cpu(OFF_X87 + X87_FDS, ds);
                }
            }
            X87Op::Load(f) => {
                let st = match f {
                    X87Format::F32 | X87Format::I32 => {
                        let v = self.val(&args[0])?;
                        self.x87_call(&format!("ld_{}", f.suffix()), &[I], &[v])
                    }
                    X87Format::F64 | X87Format::I64 => {
                        let v = self.val(&args[0])?;
                        self.x87_call(&format!("ld_{}", f.suffix()), &[Q], &[v])
                    }
                    X87Format::I16 => {
                        let v = self.val(&args[0])?;
                        let v = self.b.ins().sextend(I, v);
                        self.x87_call("ld_i16", &[I], &[v])
                    }
                    X87Format::F80 | X87Format::Bcd => {
                        let v = self.val(&args[0])?;
                        self.slot_store(0, v);
                        let p = self.slot_addr(0);
                        self.x87_call(&format!("ld_{}", f.suffix()), &[Q], &[p])
                    }
                };
                self.x87_check(st, pc);
            }
            X87Op::LoadConst(c) => {
                let c = self.i32c(c as u32);
                let st = self.x87_call("ld_const", &[I], &[c]);
                self.x87_check(st, pc);
            }
            X87Op::LoadReg(i) => {
                let i = self.i32c(i as u32);
                let st = self.x87_call("ld_st", &[I], &[i]);
                self.x87_check(st, pc);
            }
            X87Op::StoreConvert { to, src } => {
                // Results start at 0, as the C's declarations do.
                self.zero_scratch(32);
                let i = self.i32c(src as u32);
                let (v, sw) = (self.slot_addr(0), self.slot_addr(16));
                let st = self.x87_call(&format!("st_{}", to.suffix()), &[I, Q, Q], &[i, v, sw]);
                // SUPPRESS: no write; anything else non-OK fails.
                let top = self.b.ins().band_imm_u(st, X87_STATUS_MASK as i64);
                let supp = self.b.ins().icmp_imm_u(IntCC::Equal, top, X87_SUPPRESS as i64);
                let ok_or_supp = self.b.ins().icmp_imm_u(IntCC::Equal, top, 0);
                let fine = self.b.ins().bor(supp, ok_or_supp);
                let fail = self.x87_fail_block();
                let cont = self.b.create_block();
                let p = self.i32c(pc);
                self.b.ins().brif(fine, cont, &[], fail, &[cir::BlockArg::Value(st), cir::BlockArg::Value(p)]);
                self.b.switch_to_block(cont);
                let w = self.b.ins().bxor_imm_u(supp, 1);
                let value = self.slot_load(crate::lower::ty(dsts[0].width), 0);
                let value = if dsts[0].width == ir::Width::W80 {
                    let m = self.iconst(types::I128, ir::Width::W80.mask());
                    self.b.ins().band(value, m)
                } else {
                    value
                };
                let sw = self.slot_load(types::I16, 16);
                self.set(dsts[0], value);
                self.set(dsts[1], sw);
                self.set(dsts[2], w);
            }
            X87Op::StoreCommit { pop } => {
                let sw = self.val(&args[0])?;
                let sw = self.b.ins().uextend(I, sw);
                let p = if pop {
                    let w = self.val(&args[1])?;
                    self.b.ins().uextend(I, w)
                } else {
                    self.i32c(0)
                };
                let st = self.x87_call("store_commit", &[I, I], &[sw, p]);
                self.x87_check(st, pc);
            }
            X87Op::StoreReg { dst, pop } => {
                let (d, p) = (self.i32c(dst as u32), self.i32c(pop as u32));
                let st = self.x87_call("st_st", &[I, I], &[d, p]);
                self.x87_check(st, pc);
            }
            X87Op::Arith { op, dst, src, pop } => {
                let sel = self.i32c(op as u32 | if pop { 0x100 } else { 0 });
                let dst = if matches!(src, X87Src::St(_)) { dst } else { 0 };
                let d = self.i32c(dst as u32);
                let k = self.i32c(src.kind());
                let s = self.x87_src(src, args)?;
                let st = self.x87_call("arith", &[I, I, I, Q], &[sel, d, k, s]);
                self.x87_check(st, pc);
            }
            X87Op::Unary(u) => {
                let u = self.i32c(u as u32);
                let st = self.x87_call("unary", &[I], &[u]);
                self.x87_check(st, pc);
            }
            X87Op::Cmp { kind, src, pop, to_flags } => {
                let k = self.i32c(kind as u32);
                let sk = self.i32c(src.kind());
                let s = self.x87_src(src, args)?;
                let p = self.i32c(pop as u32);
                let tf = self.i32c(to_flags as u32);
                let st = self.x87_call("cmp", &[I, I, Q, I, I], &[k, sk, s, p, tf]);
                let top = self.b.ins().band_imm_u(st, X87_STATUS_MASK as i64);
                let supp = self.b.ins().icmp_imm_u(IntCC::Equal, top, X87_SUPPRESS as i64);
                let ok = self.b.ins().icmp_imm_u(IntCC::Equal, top, 0);
                let fine = self.b.ins().bor(supp, ok);
                let fail = self.x87_fail_block();
                let cont = self.b.create_block();
                let pcv = self.i32c(pc);
                self.b.ins().brif(fine, cont, &[], fail, &[cir::BlockArg::Value(st), cir::BlockArg::Value(pcv)]);
                self.b.switch_to_block(cont);
                let payload = self.b.ins().band_imm_u(st, 0x00ff_ffff);
                self.set(dsts[0], payload);
                self.set(dsts[1], supp);
            }
            X87Op::Xam => {
                let st = self.x87_call("xam", &[], &[]);
                self.x87_check(st, pc);
            }
            X87Op::Xch(i) => {
                let i = self.i32c(i as u32);
                let st = self.x87_call("xch", &[I], &[i]);
                self.x87_check(st, pc);
            }
            X87Op::Free { reg, pop } => {
                let (r, p) = (self.i32c(reg as u32), self.i32c(pop as u32));
                let st = self.x87_call("free", &[I, I], &[r, p]);
                self.x87_check(st, pc);
            }
            X87Op::IncStp | X87Op::DecStp => {
                let d = self.i32c(if op == X87Op::IncStp { 1 } else { u32::MAX });
                let st = self.x87_call("adj_top", &[I], &[d]);
                self.x87_check(st, pc);
            }
            X87Op::Cmov(i) => {
                let i = self.i32c(i as u32);
                let t = self.val(&args[0])?;
                let t = self.b.ins().uextend(I, t);
                let st = self.x87_call("cmov", &[I, I], &[i, t]);
                self.x87_check(st, pc);
            }
            X87Op::LoadCw => {
                let cw = self.val(&args[0])?;
                let cw = self.b.ins().uextend(I, cw);
                let st = self.x87_call("ldcw", &[I], &[cw]);
                self.x87_check(st, pc);
            }
            X87Op::Clex | X87Op::Init => {
                let st = self.x87_call(if op == X87Op::Clex { "clex" } else { "init" }, &[], &[]);
                self.x87_check(st, pc);
            }
            X87Op::SaveImage(e) => {
                self.zero_scratch(112);
                let i = self.slot_addr(0);
                let k = self.i32c(e as u32);
                let st = self.x87_call("save_image", &[I, Q], &[k, i]);
                self.x87_check(st, pc);
                let mut off = 0;
                for (t, w) in dsts.iter().zip(e.chunks()) {
                    let v = self.slot_load(crate::lower::ty(*w), off);
                    self.set(*t, v);
                    off += w.bytes() as i32;
                }
            }
            X87Op::SaveCommit(e) => {
                let k = self.i32c(e as u32);
                let st = self.x87_call("save_commit", &[I], &[k]);
                self.x87_check(st, pc);
            }
            X87Op::LoadEnv(e) => {
                self.zero_scratch(112);
                let mut off = 0;
                for (a, w) in args.iter().zip(e.chunks()) {
                    let v = self.val(a)?;
                    self.slot_store(off, v);
                    off += w.bytes() as i32;
                }
                let i = self.slot_addr(0);
                let k = self.i32c(e as u32);
                let st = self.x87_call("load_env", &[I, Q], &[k, i]);
                self.x87_check(st, pc);
            }
            X87Op::Ftol2 => {
                self.zero_scratch(16);
                let o = self.slot_addr(0);
                let st = self.x87_call("ftol2", &[Q], &[o]);
                self.x87_check(st, pc);
                let v = self.slot_load(Q, 0);
                self.set(dsts[0], v);
            }
        }
        Ok(())
    }
}
