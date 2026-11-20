//! SSE register moves, shuffles and bitwise operations: exact bit copies,
//! no floating-point semantics.

use super::*;
use ir::SseOp;

impl Lower<'_> {
    // ---- SSE moves and bitwise ops --------------------------------------

    pub(super) fn xmm(&self, i: u32) -> R<u8> {
        let r = self.insn.op_register(i);
        if self.insn.op_kind(i) == OpKind::Register && (Register::XMM0..=Register::XMM7).contains(&r) {
            Ok(r as u8 - Register::XMM0 as u8)
        } else {
            Err(format!("operand {i} is not xmm0-7"))
        }
    }

    pub(super) fn read_xmm(&mut self, reg: u8) -> Value {
        let dst = self.b.temp(Width::W128);
        self.b.push(Stmt::ReadXmm { dst, reg });
        dst.into()
    }

    pub(super) fn write_xmm(&mut self, reg: u8, value: Value) {
        self.b.push(Stmt::WriteXmm { reg, value });
    }

    /// The memory operand's address, with the `#GP` check that legacy SSE
    /// 16-byte operands require (everything but `movups`). Alignment is a
    /// property of the linear address, so an FS-relative aligned access
    /// would need the FS base: refused rather than checked wrongly.
    pub(super) fn sse_addr(&mut self, aligned: bool) -> R<Addr> {
        let a = self.ea()?;
        if aligned {
            if a.seg != Segment::Flat {
                return Err("FS-relative aligned SSE operand".into());
            }
            let low = self.b.and(a.offset, c32(15));
            let bad = self.b.cmp(CmpOp::Ne, low, c32(0));
            self.b.fault_if(bad, Fault::GeneralProtection);
        }
        Ok(a)
    }

    /// Operand `i` as 128 bits: an XMM register or an aligned m128.
    pub(super) fn xmm_or_m128(&mut self, i: u32) -> R<Value> {
        if self.is_mem(i) {
            let a = self.sse_addr(true)?;
            Ok(self.b.load(a, Width::W128))
        } else {
            let r = self.xmm(i)?;
            Ok(self.read_xmm(r))
        }
    }

    /// Keep the bits of `old` outside `mask`, take `new`'s inside it.
    pub(super) fn merge128(&mut self, old: Value, new: Value, mask: u128) -> Value {
        let keep = self.b.and(old, k(Width::W128, !mask));
        let take = self.b.and(new, k(Width::W128, mask));
        self.b.or(keep, take)
    }

    pub(super) fn lane(&mut self, v: Value, i: u32) -> Value {
        let s = self.b.bin(BinOp::Shr, v, k(Width::W128, 32 * i as u128));
        self.b.trunc(s, Width::W32)
    }

    pub(super) fn pack_lanes(&mut self, lanes: [Value; 4]) -> Value {
        let mut acc = k(Width::W128, 0);
        for (i, l) in lanes.into_iter().enumerate() {
            let w = self.b.zext(l, Width::W128);
            let w = self.b.bin(BinOp::Shl, w, k(Width::W128, 32 * i as u128));
            acc = self.b.or(acc, w);
        }
        acc
    }

    /// `movss`: a load zeroes lanes 1-3; register to register keeps them.
    pub(super) fn movss(&mut self) -> R {
        if self.is_mem(0) {
            let r = self.xmm(1)?;
            let v = self.read_xmm(r);
            let lo = self.b.trunc(v, Width::W32);
            let a = self.ea()?;
            self.b.store(a, lo);
        } else if self.is_mem(1) {
            let d = self.xmm(0)?;
            let a = self.ea()?;
            let v = self.b.load(a, Width::W32);
            let v = self.b.zext(v, Width::W128);
            self.write_xmm(d, v);
        } else {
            let (d, s) = (self.xmm(0)?, self.xmm(1)?);
            let old = self.read_xmm(d);
            let src = self.read_xmm(s);
            let v = self.merge128(old, src, 0xFFFF_FFFF);
            self.write_xmm(d, v);
        }
        Ok(())
    }

    pub(super) fn movaps(&mut self, aligned: bool) -> R {
        if self.is_mem(0) {
            let r = self.xmm(1)?;
            let v = self.read_xmm(r);
            let a = self.sse_addr(aligned)?;
            self.b.store(a, v);
        } else {
            let d = self.xmm(0)?;
            let v = if self.is_mem(1) {
                let a = self.sse_addr(aligned)?;
                self.b.load(a, Width::W128)
            } else {
                let s = self.xmm(1)?;
                self.read_xmm(s)
            };
            self.write_xmm(d, v);
        }
        Ok(())
    }

    /// `movlps`/`movhps` between an XMM half and m64 (no alignment rule).
    pub(super) fn movlhps_mem(&mut self, high: bool) -> R {
        let shift = if high { 64 } else { 0 };
        if self.is_mem(0) {
            let r = self.xmm(1)?;
            let v = self.read_xmm(r);
            let v = self.b.bin(BinOp::Shr, v, k(Width::W128, shift));
            let half = self.b.trunc(v, Width::W64);
            let a = self.ea()?;
            self.b.store(a, half);
        } else {
            let d = self.xmm(0)?;
            if !self.is_mem(1) {
                return Err("register form".into());
            }
            let a = self.ea()?;
            let half = self.b.load(a, Width::W64);
            let old = self.read_xmm(d);
            let w = self.b.zext(half, Width::W128);
            let w = self.b.bin(BinOp::Shl, w, k(Width::W128, shift));
            let v = self.merge128(old, w, (u64::MAX as u128) << shift);
            self.write_xmm(d, v);
        }
        Ok(())
    }

    /// `movhlps` (dst.lo = src.hi) and `movlhps` (dst.hi = src.lo).
    pub(super) fn movhl(&mut self, hl: bool) -> R {
        let (d, s) = (self.xmm(0)?, self.xmm(1)?);
        let old = self.read_xmm(d);
        let src = self.read_xmm(s);
        let (moved, mask) = if hl {
            (self.b.bin(BinOp::Shr, src, k(Width::W128, 64)), u64::MAX as u128)
        } else {
            (self.b.bin(BinOp::Shl, src, k(Width::W128, 64)), (u64::MAX as u128) << 64)
        };
        let v = self.merge128(old, moved, mask);
        self.write_xmm(d, v);
        Ok(())
    }

    pub(super) fn sse_logic(&mut self) -> R {
        let d = self.xmm(0)?;
        let src = self.xmm_or_m128(1)?;
        let dst = self.read_xmm(d);
        let v = match self.insn.mnemonic() {
            Mnemonic::Xorps => self.b.xor(dst, src),
            Mnemonic::Andps => self.b.and(dst, src),
            Mnemonic::Orps => self.b.or(dst, src),
            _ => {
                let n = self.b.not(dst);
                self.b.and(n, src)
            }
        };
        self.write_xmm(d, v);
        Ok(())
    }

    /// `shufps` (lanes 0-1 picked from dst, 2-3 from src by the immediate),
    /// `unpcklps` ([d0 s0 d1 s1]) and `unpckhps` ([d2 s2 d3 s3]).
    pub(super) fn sse_shuffle(&mut self) -> R {
        let d = self.xmm(0)?;
        let src = self.xmm_or_m128(1)?;
        let dst = self.read_xmm(d);
        let lanes = match self.insn.mnemonic() {
            Mnemonic::Shufps => {
                let imm = self.insn.immediate8() as u32;
                [
                    self.lane(dst, imm & 3),
                    self.lane(dst, (imm >> 2) & 3),
                    self.lane(src, (imm >> 4) & 3),
                    self.lane(src, (imm >> 6) & 3),
                ]
            }
            Mnemonic::Unpcklps => [self.lane(dst, 0), self.lane(src, 0), self.lane(dst, 1), self.lane(src, 1)],
            _ => [self.lane(dst, 2), self.lane(src, 2), self.lane(dst, 3), self.lane(src, 3)],
        };
        let v = self.pack_lanes(lanes);
        self.write_xmm(d, v);
        Ok(())
    }

    /// `movmskps r32, xmm`: the four sign bits, zero-extended.
    pub(super) fn movmskps(&mut self) -> R {
        let s = self.xmm(1)?;
        let v = self.read_xmm(s);
        let mut acc = c32(0);
        for i in 0..4 {
            let sign = self.b.bit(v, 32 * i + 31);
            let w = self.b.zext(sign, Width::W32);
            let w = self.b.bin(BinOp::Shl, w, c32(i));
            acc = self.b.or(acc, w);
        }
        self.write(0, acc)
    }
}

/// SSE floating point: every operation is one [`Intrinsic::Sse`] carrying
/// MXCSR semantics, with the operands read (and faulting) first and the
/// destination written after (an `#XM` leaves it unwritten).
impl Lower<'_> {
    fn sse_op(&self) -> R<SseOp> {
        use Mnemonic as M;
        use SseOp::*;
        Ok(match self.insn.mnemonic() {
            M::Addss => AddSs,
            M::Addps => AddPs,
            M::Subss => SubSs,
            M::Subps => SubPs,
            M::Mulss => MulSs,
            M::Mulps => MulPs,
            M::Divss => DivSs,
            M::Divps => DivPs,
            M::Sqrtss => SqrtSs,
            M::Sqrtps => SqrtPs,
            M::Minss => MinSs,
            M::Minps => MinPs,
            M::Maxss => MaxSs,
            M::Maxps => MaxPs,
            M::Rcpss => RcpSs,
            M::Rcpps => RcpPs,
            M::Rsqrtss => RsqrtSs,
            M::Rsqrtps => RsqrtPs,
            M::Cmpss => CmpSs,
            M::Cmpps => CmpPs,
            M::Comiss => Comiss,
            M::Ucomiss => Ucomiss,
            M::Cvtsi2ss => Cvtsi2ss,
            M::Cvtss2si => Cvtss2si,
            M::Cvttss2si => Cvttss2si,
            M::Cvtpi2ps => Cvtpi2ps,
            M::Cvtps2pi => Cvtps2pi,
            M::Cvttps2pi => Cvttps2pi,
            m => return Err(format!("{m:?} is not an SSE FP operation")),
        })
    }

    /// Operand `i` as a scalar: lane 0 of an XMM register, or m32 (no
    /// alignment rule).
    fn xmm_or_m32(&mut self, i: u32) -> R<Value> {
        if self.is_mem(i) {
            let a = self.ea()?;
            Ok(self.b.load(a, Width::W32))
        } else {
            let r = self.xmm(i)?;
            let v = self.read_xmm(r);
            Ok(self.b.trunc(v, Width::W32))
        }
    }

    /// Operand `i` as two lanes: the low half of an XMM register, or m64.
    fn xmm_or_m64(&mut self, i: u32) -> R<Value> {
        if self.is_mem(i) {
            let a = self.ea()?;
            Ok(self.b.load(a, Width::W64))
        } else {
            let r = self.xmm(i)?;
            let v = self.read_xmm(r);
            Ok(self.b.trunc(v, Width::W64))
        }
    }

    fn sse_fp(&mut self, op: SseOp, args: Vec<Value>) -> Value {
        self.intrinsic(Intrinsic::Sse(op), args)[0]
    }

    /// Arithmetic, compares, sqrt/rcp/rsqrt and `cvtsi2ss`/`cvtpi2ps`: the
    /// destination XMM register is the result.
    pub(super) fn sse_arith(&mut self) -> R {
        use SseOp::*;
        let op = self.sse_op()?;
        let d = self.xmm(0)?;
        let scalar = matches!(
            op,
            AddSs | SubSs | MulSs | DivSs | SqrtSs | MinSs | MaxSs | RcpSs | RsqrtSs | CmpSs
        );
        let src = match op {
            _ if scalar => self.xmm_or_m32(1)?,
            Cvtsi2ss => self.read_as(1, Width::W32)?,
            Cvtpi2ps => self.mmx_or_m64(1)?,
            _ => self.xmm_or_m128(1)?,
        };
        let mut args = match op {
            SqrtPs | RcpPs | RsqrtPs => vec![src],
            _ => {
                let a = self.read_xmm(d);
                vec![a, src]
            }
        };
        if matches!(op, CmpSs | CmpPs) {
            args.push(k(Width::W8, (self.insn.immediate8() & 7) as u128));
        }
        let v = self.sse_fp(op, args);
        self.write_xmm(d, v);
        // cvtpi2ps makes the MMX transition even from m64 (unlike SSE2's
        // cvtpi2pd; SDM vol. 2, CVTPI2PS).
        if op == Cvtpi2ps {
            self.intrinsic(Intrinsic::MmxEnter, vec![]);
        }
        Ok(())
    }

    /// `comiss`/`ucomiss`: ZF, PF, CF from the comparison; OF, SF, AF = 0.
    pub(super) fn sse_comi(&mut self) -> R {
        let op = self.sse_op()?;
        let d = self.xmm(0)?;
        let b = self.xmm_or_m32(1)?;
        let a = self.read_xmm(d);
        let a = self.b.trunc(a, Width::W32);
        let f = self.sse_fp(op, vec![a, b]);
        let (zf, pf, cf) = (self.b.bit(f, 6), self.b.bit(f, 2), self.b.bit(f, 0));
        self.b.set_flag(Flag::Zf, zf);
        self.b.set_flag(Flag::Pf, pf);
        self.b.set_flag(Flag::Cf, cf);
        let zero = Value::constant(Width::W1, 0);
        for flag in [Flag::Of, Flag::Sf, Flag::Af] {
            self.b.set_flag(flag, zero);
        }
        Ok(())
    }

    /// `cvt[t]ss2si r32, xmm/m32` and `cvt[t]ps2pi mm, xmm/m64`.
    pub(super) fn sse_to_int(&mut self) -> R {
        let op = self.sse_op()?;
        if matches!(op, SseOp::Cvtss2si | SseOp::Cvttss2si) {
            let a = self.xmm_or_m32(1)?;
            let v = self.sse_fp(op, vec![a]);
            self.write(0, v)
        } else {
            let d = self.mm(0)?;
            let a = self.xmm_or_m64(1)?;
            let v = self.sse_fp(op, vec![a]);
            self.write_mmx(d, v);
            self.intrinsic(Intrinsic::MmxEnter, vec![]);
            Ok(())
        }
    }

    /// `movntps m128, xmm`: an aligned store (the non-temporal hint has no
    /// architectural effect).
    pub(super) fn movntps(&mut self) -> R {
        if !self.is_mem(0) {
            return Err("movntps to a register (#UD)".into());
        }
        self.movaps(true)
    }
}
