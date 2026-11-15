//! Integer instructions: data movement, ALU, multiply/divide, shifts
//! and rotates, bit operations, atomics' read-modify-write forms.

use super::*;

impl Lower<'_> {
    // ---- data movement --------------------------------------------------

    pub(super) fn mov(&mut self) -> R {
        let w = self.width(0)?;
        let v = self.read_as(1, w)?;
        self.write(0, v)
    }

    pub(super) fn extend(&mut self, signed: bool) -> R {
        let w = self.width(0)?;
        let v = self.read(1)?;
        let v = if signed { self.b.sext(v, w) } else { self.b.zext(v, w) };
        self.write(0, v)
    }

    pub(super) fn lea(&mut self) -> R {
        let w = self.width(0)?;
        // lea computes the offset only: no segment base, no access.
        let a = self.ea()?;
        let v = self.b.trunc(a.offset, w);
        self.write(0, v)
    }

    pub(super) fn xchg(&mut self) -> R {
        // Exchange with memory locks the bus whether or not LOCK is given.
        self.atomic |= self.is_mem(0) || self.is_mem(1);
        // The register side is read first, so an atomic form's operand is
        // ready before its memory access.
        let (a, b) = if self.is_mem(0) {
            let b = self.read(1)?;
            (self.read(0)?, b)
        } else {
            let a = self.read(0)?;
            (a, self.read(1)?)
        };
        if self.is_mem(0) {
            self.atomic_operand = Some(b);
            self.atomic_old = gpr(self.insn.op_register(1));
        }
        // Memory side first: a faulting store leaves the register alone.
        if self.is_mem(0) {
            self.write(0, b)?;
            self.write(1, a)
        } else {
            self.write(1, a)?;
            self.write(0, b)
        }
    }

    pub(super) fn push_insn(&mut self) -> R {
        let size = (-self.insn.stack_pointer_increment()) as u32;
        let w = Width::from_bytes(size).ok_or("odd push size")?;
        if self.insn.op_kind(0) == OpKind::Register && gpr(self.insn.op_register(0)).is_none() {
            return Err(self.bad_reg(0));
        }
        // `push esp` pushes the value before the decrement, which reading
        // first gives for free.
        let v = self.read_as(0, w)?;
        let v = self.b.zext(v, w);
        self.push_value(v);
        Ok(())
    }

    pub(super) fn pop_insn(&mut self) -> R {
        let w = self.width(0)?;
        let esp = self.esp();
        let v = self.b.load(Addr { seg: Segment::Flat, offset: esp }, w);
        let new = self.b.add(esp, c32(w.bytes()));
        if self.is_mem(0) {
            // A memory destination's address uses ESP *after* the pop.
            let mut a = self.ea()?;
            if self.insn.memory_base() == Register::ESP {
                a.offset = self.b.add(a.offset, c32(w.bytes()));
                self.ea = Some(a);
            }
            self.b.store(a, v);
            self.b.write_reg(Gpr::Esp, Slice::Full, new);
        } else {
            // ESP first, so `pop esp` ends with the loaded value.
            self.b.write_reg(Gpr::Esp, Slice::Full, new);
            self.write(0, v)?;
        }
        Ok(())
    }

    pub(super) fn pushad(&mut self) -> R {
        if self.insn.code() != Code::Pushad {
            return Err("16-bit pusha".into());
        }
        let esp = self.esp();
        let vals: Vec<Value> = Gpr::ALL.iter().map(|g| self.b.read_reg(*g, Slice::Full)).collect();
        for (i, v) in vals.into_iter().enumerate() {
            let at = self.b.sub(esp, c32(4 * (i as u32 + 1)));
            self.b.store(Addr { seg: Segment::Flat, offset: at }, v);
        }
        let new = self.b.sub(esp, c32(32));
        self.b.write_reg(Gpr::Esp, Slice::Full, new);
        Ok(())
    }

    pub(super) fn popad(&mut self) -> R {
        if self.insn.code() != Code::Popad {
            return Err("16-bit popa".into());
        }
        let esp = self.esp();
        let mut vals = Vec::new();
        for i in 0..8u32 {
            let at = self.b.add(esp, c32(4 * (7 - i)));
            vals.push(self.b.load(Addr { seg: Segment::Flat, offset: at }, Width::W32));
        }
        for (g, v) in Gpr::ALL.iter().zip(vals) {
            if *g != Gpr::Esp {
                self.b.write_reg(*g, Slice::Full, v);
            }
        }
        let new = self.b.add(esp, c32(32));
        self.b.write_reg(Gpr::Esp, Slice::Full, new);
        Ok(())
    }

    pub(super) fn pushfd(&mut self) -> R {
        let dst = self.b.temp(Width::W32);
        self.b.push(Stmt::ReadEflags { dst });
        // VM (17) and RF (16) read as zero in the pushed image.
        let v = self.b.and(dst.into(), c32(!0x0003_0000));
        self.push_value(v);
        Ok(())
    }

    /// `popfd`. The status flags, DF, AC and ID are ordinary state. The
    /// system flags (TF, IF, IOPL, NT) are not modelled: interrupts and
    /// single-stepping belong to the runtime, not to lifted code. A `popfd`
    /// that would change one of them stops with [`StopReason::Unmodelled`]
    /// before anything is written, rather than setting TF with no trap or
    /// silently toggling IF. Restoring the value a `pushfd` saved, the
    /// usual pattern, never changes them and runs normally.
    pub(super) fn popfd(&mut self) -> R {
        let esp = self.esp();
        let v = self.b.load(Addr { seg: Segment::Flat, offset: esp }, Width::W32);
        let cur = self.b.temp(Width::W32);
        self.b.push(Stmt::ReadEflags { dst: cur });
        let diff = self.b.xor(v, cur.into());
        let sys = self.b.and(diff, c32(POPFD_SYSTEM));
        let changes = self.b.cmp(CmpOp::Ne, sys, c32(0));
        self.b.push(Stmt::StopIf { cond: changes, reason: StopReason::Unmodelled });
        let new = self.b.add(esp, c32(4));
        self.b.write_reg(Gpr::Esp, Slice::Full, new);
        self.b.push(Stmt::WriteEflags { value: v, mask: POPFD_MASK });
        Ok(())
    }

    pub(super) fn lahf(&mut self) {
        let dst = self.b.temp(Width::W32);
        self.b.push(Stmt::ReadEflags { dst });
        let low = self.b.trunc(dst.into(), Width::W8);
        let v = self.b.and(low, k(Width::W8, 0xD5));
        let v = self.b.or(v, k(Width::W8, 0x02));
        self.b.write_reg(Gpr::Eax, Slice::High8, v);
    }

    pub(super) fn sahf(&mut self) {
        let ah = self.b.read_reg(Gpr::Eax, Slice::High8);
        let v = self.b.zext(ah, Width::W32);
        self.b.push(Stmt::WriteEflags { value: v, mask: 0xD5 });
    }

    pub(super) fn leave(&mut self) -> R {
        if self.insn.code() != Code::Leaved {
            return Err("16-bit leave".into());
        }
        let ebp = self.b.read_reg(Gpr::Ebp, Slice::Full);
        let saved = self.b.load(Addr { seg: Segment::Flat, offset: ebp }, Width::W32);
        let new = self.b.add(ebp, c32(4));
        self.b.write_reg(Gpr::Esp, Slice::Full, new);
        self.b.write_reg(Gpr::Ebp, Slice::Full, saved);
        Ok(())
    }

    pub(super) fn enter(&mut self) -> R {
        if self.insn.code() != Code::Enterd_imm16_imm8 || self.insn.immediate8_2nd() != 0 {
            return Err("enter with nesting level or 16-bit".into());
        }
        let size = self.insn.immediate16() as u32;
        let ebp = self.b.read_reg(Gpr::Ebp, Slice::Full);
        self.push_value(ebp);
        let frame = self.esp();
        self.b.write_reg(Gpr::Ebp, Slice::Full, frame);
        let new = self.b.sub(frame, c32(size));
        self.b.write_reg(Gpr::Esp, Slice::Full, new);
        Ok(())
    }

    /// `cbw`/`cwde`: sign-extend the low half of the accumulator into it.
    pub(super) fn widen_acc(&mut self) {
        let (from, to) = if self.insn.mnemonic() == Mnemonic::Cbw {
            (Slice::Low8, Slice::Low16)
        } else {
            (Slice::Low16, Slice::Full)
        };
        let v = self.b.read_reg(Gpr::Eax, from);
        let v = self.b.sext(v, to.width());
        self.b.write_reg(Gpr::Eax, to, v);
    }

    /// `cwd`/`cdq`: fill DX/EDX with the accumulator's sign.
    pub(super) fn sign_into_dx(&mut self) {
        let s = if self.insn.mnemonic() == Mnemonic::Cwd { Slice::Low16 } else { Slice::Full };
        let v = self.b.read_reg(Gpr::Eax, s);
        let n = s.width().bits() - 1;
        let v = self.b.bin(BinOp::Sar, v, k(s.width(), n as u128));
        self.b.write_reg(Gpr::Edx, s, v);
    }

    pub(super) fn xlat(&mut self) -> R {
        if self.insn.memory_base() != Register::EBX {
            return Err("16-bit xlat".into());
        }
        let seg = if self.insn.memory_segment() == Register::FS { Segment::Fs } else { Segment::Flat };
        let ebx = self.b.read_reg(Gpr::Ebx, Slice::Full);
        let al = self.b.read_reg(Gpr::Eax, Slice::Low8);
        let al = self.b.zext(al, Width::W32);
        let a = self.b.add(ebx, al);
        let v = self.b.load(Addr { seg, offset: a }, Width::W8);
        self.b.write_reg(Gpr::Eax, Slice::Low8, v);
        Ok(())
    }

    // ---- arithmetic -----------------------------------------------------

    /// Two-operand ALU forms; `store` is false for `cmp`/`test`.
    pub(super) fn alu(&mut self, op: Alu, store: bool) -> R {
        let w = self.width(0)?;
        // Source first: an atomic form's operand is ready before its load.
        let x = self.read_as(1, w)?;
        let a = self.read(0)?;
        self.atomic_operand = Some(x);
        let res = match op {
            Alu::Add | Alu::Adc => {
                let mut r = self.b.add(a, x);
                if op == Alu::Adc {
                    let cf = self.b.read_flag(Flag::Cf);
                    let cf = self.b.zext(cf, w);
                    r = self.b.add(r, cf);
                }
                self.flags_add(a, x, r, true);
                r
            }
            Alu::Sub | Alu::Sbb => {
                let mut r = self.b.sub(a, x);
                if op == Alu::Sbb {
                    let cf = self.b.read_flag(Flag::Cf);
                    let cf = self.b.zext(cf, w);
                    r = self.b.sub(r, cf);
                }
                self.flags_sub(a, x, r, true);
                r
            }
            Alu::And | Alu::Or | Alu::Xor => {
                let bop = match op {
                    Alu::And => BinOp::And,
                    Alu::Or => BinOp::Or,
                    _ => BinOp::Xor,
                };
                let r = self.b.bin(bop, a, x);
                self.flags_logic(r);
                r
            }
        };
        if store {
            self.write(0, res)?;
        }
        Ok(())
    }

    pub(super) fn inc_dec(&mut self, inc: bool) -> R {
        let w = self.width(0)?;
        let a = self.read(0)?;
        let one = k(w, 1);
        let r = if inc { self.b.add(a, one) } else { self.b.sub(a, one) };
        // CF is preserved: the only difference from add/sub 1.
        if inc {
            self.flags_add(a, one, r, false);
        } else {
            self.flags_sub(a, one, r, false);
        }
        self.write(0, r)
    }

    pub(super) fn neg(&mut self) -> R {
        let w = self.width(0)?;
        let a = self.read(0)?;
        let zero = k(w, 0);
        let r = self.b.un(UnOp::Neg, a);
        // Same as 0 - a, except CF is simply "operand was not zero".
        self.flags_sub(zero, a, r, false);
        let nz = self.b.cmp(CmpOp::Ne, a, zero);
        self.b.set_flag(Flag::Cf, nz);
        self.write(0, r)
    }

    /// Accumulator, high register and width for the one-operand
    /// `mul`/`imul`/`div`/`idiv` forms: AL/AH via AX for bytes, DX:AX,
    /// EDX:EAX.
    pub(super) fn acc_pair(w: Width) -> ((Gpr, Slice), (Gpr, Slice)) {
        match w {
            Width::W8 => ((Gpr::Eax, Slice::Low8), (Gpr::Eax, Slice::High8)),
            Width::W16 => ((Gpr::Eax, Slice::Low16), (Gpr::Edx, Slice::Low16)),
            _ => ((Gpr::Eax, Slice::Full), (Gpr::Edx, Slice::Full)),
        }
    }

    /// One-operand `mul`/`imul`: the double-width product of the
    /// accumulator and the operand lands in the register pair. CF = OF =
    /// "the high half carries information"; SF/ZF/AF/PF are undefined.
    pub(super) fn mul_wide(&mut self, signed: bool) -> R {
        let w = self.width(0)?;
        let w2 = w.double().ok_or("multiply width")?;
        let src = self.read(0)?;
        let ((lg, ls), (hg, hs)) = Self::acc_pair(w);
        let acc = self.b.read_reg(lg, ls);
        let (a, x) = if signed {
            (self.b.sext(acc, w2), self.b.sext(src, w2))
        } else {
            (self.b.zext(acc, w2), self.b.zext(src, w2))
        };
        let p = self.b.bin(BinOp::Mul, a, x);
        let lo = self.b.trunc(p, w);
        let hi_wide = self.b.bin(BinOp::Shr, p, k(w2, w.bits() as u128));
        let hi = self.b.trunc(hi_wide, w);
        let carries = if signed {
            let back = self.b.sext(lo, w2);
            self.b.cmp(CmpOp::Ne, back, p)
        } else {
            self.b.cmp(CmpOp::Ne, hi, k(w, 0))
        };
        if w == Width::W8 {
            // AX = product; one 16-bit write, not two byte merges.
            self.b.write_reg(Gpr::Eax, Slice::Low16, p);
        } else {
            self.b.write_reg(lg, ls, lo);
            self.b.write_reg(hg, hs, hi);
        }
        self.mul_flags(carries);
        Ok(())
    }

    pub(super) fn mul_flags(&mut self, carries: Value) {
        self.b.set_flag(Flag::Cf, carries);
        self.b.set_flag(Flag::Of, carries);
        for f in [Flag::Sf, Flag::Zf, Flag::Af, Flag::Pf] {
            self.b.undef_flag(f);
        }
    }

    pub(super) fn imul(&mut self) -> R {
        match self.insn.op_count() {
            1 => self.mul_wide(true),
            n => {
                // imul r, r/m (, imm): truncated product; CF = OF = "the
                // signed product did not fit".
                let w = self.width(0)?;
                let w2 = w.double().ok_or("multiply width")?;
                let (a, x) = if n == 2 {
                    (self.read(0)?, self.read(1)?)
                } else {
                    (self.read(1)?, self.read_as(2, w)?)
                };
                let wa = self.b.sext(a, w2);
                let wx = self.b.sext(x, w2);
                let p = self.b.bin(BinOp::Mul, wa, wx);
                let lo = self.b.trunc(p, w);
                let back = self.b.sext(lo, w2);
                let over = self.b.cmp(CmpOp::Ne, back, p);
                self.write(0, lo)?;
                self.mul_flags(over);
                Ok(())
            }
        }
    }

    /// `div`/`idiv`: divide the double-width register pair by the operand.
    /// `#DE` on a zero divisor or a quotient that does not fit, checked
    /// before any register changes. All six status flags are undefined.
    pub(super) fn div(&mut self, signed: bool) -> R {
        let w = self.width(0)?;
        let w2 = w.double().ok_or("divide width")?;
        let d = self.read(0)?;
        let ((lg, ls), (hg, hs)) = Self::acc_pair(w);
        let dividend = if w == Width::W8 {
            self.b.read_reg(Gpr::Eax, Slice::Low16)
        } else {
            let lo = self.b.read_reg(lg, ls);
            let hi = self.b.read_reg(hg, hs);
            let lo = self.b.zext(lo, w2);
            let hi = self.b.zext(hi, w2);
            let hi = self.b.bin(BinOp::Shl, hi, k(w2, w.bits() as u128));
            self.b.or(hi, lo)
        };
        let zero = self.b.is_zero(d);
        self.b.fault_if(zero, Fault::DivideError);
        let (q, r, fits) = if signed {
            let dd = self.b.sext(d, w2);
            let q = self.b.bin(BinOp::Sdiv, dividend, dd);
            let r = self.b.bin(BinOp::Srem, dividend, dd);
            // The double-width quotient must survive truncation. This also
            // catches MIN/-1 in the double width, which Sdiv wraps to MIN.
            let qt = self.b.trunc(q, w);
            let back = self.b.sext(qt, w2);
            (q, r, self.b.cmp(CmpOp::Eq, back, q))
        } else {
            let dd = self.b.zext(d, w2);
            let q = self.b.bin(BinOp::Udiv, dividend, dd);
            let r = self.b.bin(BinOp::Urem, dividend, dd);
            let max = k(w2, w.mask());
            (q, r, self.b.cmp(CmpOp::Ule, q, max))
        };
        let over = self.b.not(fits);
        self.b.fault_if(over, Fault::DivideError);
        let q = self.b.trunc(q, w);
        let r = self.b.trunc(r, w);
        self.b.write_reg(lg, ls, q);
        self.b.write_reg(hg, hs, r);
        for f in Flag::STATUS {
            self.b.undef_flag(f);
        }
        Ok(())
    }

    // ---- shifts and rotates --------------------------------------------

    /// The shift count operand, masked to five bits as every 32-bit x86
    /// shift and rotate does. `Ok(Err(n))` is a constant count.
    pub(super) fn shift_count(&mut self, i: u32) -> R<Result<Value, u32>> {
        if self.insn.op_count() <= i {
            return Ok(Err(1));
        }
        match self.insn.op_kind(i) {
            OpKind::Immediate8 => Ok(Err(self.insn.immediate8() as u32 & 0x1F)),
            OpKind::Register if self.insn.op_register(i) == Register::CL => {
                let cl = self.b.read_reg(Gpr::Ecx, Slice::Low8);
                Ok(Ok(self.b.and(cl, k(Width::W8, 0x1F))))
            }
            k => Err(format!("shift count {k:?}")),
        }
    }

    pub(super) fn shift(&mut self, kind: Shift) -> R {
        let w = self.width(0)?;
        let bits = w.bits();
        let count = self.shift_count(1)?;
        // A zero count changes no register and no flag; a memory operand is
        // still read and written back, so its permissions are checked.
        if count == Err(0) {
            return self.write_back(0);
        }
        let a = self.read(0)?;
        // `c` is the count at the operand's width; `c8` is the 5-bit count.
        let c8 = match count {
            Ok(v) => v,
            Err(n) => k(Width::W8, n as u128),
        };
        let c = self.b.zext(c8, w);
        let one = k(w, 1);
        // RCL/RCR: the rotate amount, count mod (bits + 1).
        let mut rotated = None;

        let (res, cf, of) = match kind {
            Shift::Shl => {
                let res = self.b.bin(BinOp::Shl, a, c);
                // Last bit shifted out: bit (bits - c) of a, computed in 64
                // bits so counts above the width give 0.
                let a64 = self.b.zext(a, Width::W64);
                let c64 = self.b.zext(c8, Width::W64);
                let t = self.b.bin(BinOp::Shl, a64, c64);
                let cf = self.b.bit(t, bits);
                let msb = self.b.msb(res);
                let of = self.b.xor(msb, cf);
                (res, cf, of)
            }
            Shift::Shr | Shift::Sar => {
                let op = if kind == Shift::Shr { BinOp::Shr } else { BinOp::Sar };
                let res = self.b.bin(op, a, c);
                let cm1 = self.b.sub(c, one);
                let t = self.b.bin(op, a, cm1);
                let cf = self.b.trunc(t, Width::W1);
                // OF for a 1-bit shift: SHR gives the old sign, SAR 0.
                let of = if kind == Shift::Shr { self.b.msb(a) } else { Value::bool(false) };
                (res, cf, of)
            }
            Shift::Rol | Shift::Ror => {
                let op = if kind == Shift::Rol { BinOp::Rol } else { BinOp::Ror };
                let res = self.b.bin(op, a, c);
                let msb = self.b.msb(res);
                let (cf, of) = if kind == Shift::Rol {
                    let cf = self.b.trunc(res, Width::W1);
                    (cf, self.b.xor(msb, cf))
                } else {
                    let next = self.b.bit(res, bits - 2);
                    (msb, self.b.xor(msb, next))
                };
                (res, cf, of)
            }
            Shift::Rcl | Shift::Rcr => {
                // Rotate the (bits+1)-bit value CF:a, in 64 bits. Counts
                // reduce mod (bits + 1) for 8/16-bit operands; a 32-bit
                // operand's 5-bit count is already below 33.
                let span = bits + 1;
                let old_cf = self.b.read_flag(Flag::Cf);
                let cfw = self.b.zext(old_cf, Width::W64);
                let cfw = self.b.bin(BinOp::Shl, cfw, k(Width::W64, bits as u128));
                let a64 = self.b.zext(a, Width::W64);
                let v = self.b.or(cfw, a64);
                let c64 = self.b.zext(c8, Width::W64);
                let n = self.b.bin(BinOp::Urem, c64, k(Width::W64, span as u128));
                let back = self.b.sub(k(Width::W64, span as u128), n);
                let (l, r) = if kind == Shift::Rcl {
                    (self.b.bin(BinOp::Shl, v, n), self.b.bin(BinOp::Shr, v, back))
                } else {
                    (self.b.bin(BinOp::Shr, v, n), self.b.bin(BinOp::Shl, v, back))
                };
                rotated = Some(n);
                let rot = self.b.or(l, r);
                let rot = self.b.and(rot, k(Width::W64, (1u128 << span) - 1));
                let res = self.b.trunc(rot, w);
                let cf = self.b.bit(rot, bits);
                let of = if kind == Shift::Rcl {
                    let msb = self.b.msb(res);
                    self.b.xor(msb, cf)
                } else {
                    // Before the rotate: MSB(dest) XOR CF.
                    let msb = self.b.msb(a);
                    self.b.xor(msb, old_cf)
                };
                (res, cf, of)
            }
        };

        // The flags, as statements: at the top level for a constant count,
        // inside `if count != 0` for CL, since a zero count changes no flag.
        let shifts = matches!(kind, Shift::Shl | Shift::Shr | Shift::Sar);
        let effects = |s: &mut Self| -> R {
            // OF is defined only for 1-bit shifts and rotates.
            let is_one = match count {
                Err(n) => Err(n == 1),
                Ok(c8) => Ok(s.b.cmp(CmpOp::Eq, c8, k(Width::W8, 1))),
            };
            if let Some(n) = rotated {
                // RCL/RCR by a multiple of bits + 1 leaves the value, CF and
                // OF as they were; the flags change only for other counts.
                let flags = |s: &mut Self| -> R {
                    s.b.set_flag(Flag::Cf, cf);
                    s.flag_where(Flag::Of, is_one, of);
                    Ok(())
                };
                match n.as_const() {
                    Some(0) => {}
                    Some(_) => flags(s)?,
                    None if bits == 32 => flags(s)?,
                    None => {
                        let nz = s.b.cmp(CmpOp::Ne, n, k(Width::W64, 0));
                        s.if_else(nz, flags, |_| Ok(()))?;
                    }
                }
                return Ok(());
            }
            // SHL/SHR CF is undefined once the count reaches the width
            // (only possible for 8/16-bit operands).
            let cf_defined = match (kind, count) {
                (Shift::Shl | Shift::Shr, Err(n)) => Err(n < bits),
                (Shift::Shl | Shift::Shr, Ok(c8)) if bits < 32 => {
                    Ok(s.b.cmp(CmpOp::Ult, c8, k(Width::W8, bits as u128)))
                }
                _ => Err(true),
            };
            s.flag_where(Flag::Cf, cf_defined, cf);
            s.flag_where(Flag::Of, is_one, of);
            if shifts {
                s.szp(res);
                s.b.undef_flag(Flag::Af);
            }
            Ok(())
        };
        self.shift_effects(count, a, res, effects)
    }

    /// Write a shift's result and apply its flag `effects`. A constant
    /// count is known nonzero here. For a CL count the destination is
    /// written unconditionally, with the old value when the count is zero
    /// (a memory operand's write permission is checked either way, as on
    /// hardware), and the flags only change inside `if count != 0`.
    fn shift_effects(&mut self, count: Result<Value, u32>, old: Value, res: Value, effects: impl FnOnce(&mut Self) -> R) -> R {
        match count {
            Err(_) => {
                self.write(0, res)?;
                effects(self)
            }
            Ok(c8) => {
                let nz = self.b.cmp(CmpOp::Ne, c8, k(Width::W8, 0));
                let out = self.b.select(nz, res, old);
                self.write(0, out)?;
                self.if_else(nz, effects, |_| Ok(()))
            }
        }
    }

    /// Read operand `i` and write it back unchanged: a zero-count shift of
    /// memory, which faults like any read-modify-write. Nothing for a
    /// register.
    fn write_back(&mut self, i: u32) -> R {
        if self.is_mem(i) {
            let v = self.read(i)?;
            self.write(i, v)?;
        }
        Ok(())
    }

    /// `shld`/`shrd` on 16- and 32-bit operands. The count is masked to
    /// five bits; for a 16-bit operand a count above 16 leaves the result
    /// and every status flag undefined (SDM). The IR still computes a
    /// deterministic value then (the destination:source concatenation
    /// shifted in 64 bits) and marks the flags undefined.
    pub(super) fn double_shift(&mut self, left: bool) -> R {
        let w = self.width(0)?;
        if !matches!(w, Width::W16 | Width::W32) {
            return Err("double shift width".into());
        }
        let bits = w.bits();
        let count = self.shift_count(2)?;
        if count == Err(0) {
            return self.write_back(0);
        }
        let src = self.read(1)?;
        let a = self.read(0)?;
        let c8 = match count {
            Ok(v) => v,
            Err(n) => k(Width::W8, n as u128),
        };
        // Concatenate into 64 bits and shift once.
        let a64 = self.b.zext(a, Width::W64);
        let s64 = self.b.zext(src, Width::W64);
        let c64 = self.b.zext(c8, Width::W64);
        let half = k(Width::W64, bits as u128);
        // CF is the last bit shifted out of the destination half: bit
        // `2*bits - c` of the left concatenation, bit `c - 1` of the right.
        let (res, cf) = if left {
            let hi = self.b.bin(BinOp::Shl, a64, half);
            let v = self.b.or(hi, s64);
            let t = self.b.bin(BinOp::Shl, v, c64);
            let r = self.b.bin(BinOp::Shr, t, half);
            let at = self.b.sub(k(Width::W64, 2 * bits as u128), c64);
            let out = self.b.bin(BinOp::Shr, v, at);
            (self.b.trunc(r, w), self.b.trunc(out, Width::W1))
        } else {
            let hi = self.b.bin(BinOp::Shl, s64, half);
            let v = self.b.or(hi, a64);
            let t = self.b.bin(BinOp::Shr, v, c64);
            let at = self.b.sub(c64, k(Width::W64, 1));
            let out = self.b.bin(BinOp::Shr, v, at);
            (self.b.trunc(t, w), self.b.trunc(out, Width::W1))
        };
        let msb_old = self.b.msb(a);
        let msb_new = self.b.msb(res);
        let of = self.b.xor(msb_old, msb_new);
        // As for single shifts: no flag change for a zero count, OF only
        // for a count of one; every status flag undefined past the width.
        let defined = |s: &mut Self| -> R {
            s.b.set_flag(Flag::Cf, cf);
            let is_one = match count {
                Err(n) => Err(n == 1),
                Ok(c8) => Ok(s.b.cmp(CmpOp::Eq, c8, k(Width::W8, 1))),
            };
            s.flag_where(Flag::Of, is_one, of);
            s.szp(res);
            s.b.undef_flag(Flag::Af);
            Ok(())
        };
        let undefined = |s: &mut Self| -> R {
            for f in Flag::STATUS {
                s.b.undef_flag(f);
            }
            Ok(())
        };
        let effects = |s: &mut Self| -> R {
            match count {
                _ if bits == 32 => defined(s),
                Err(n) if n <= bits => defined(s),
                Err(_) => undefined(s),
                Ok(c8) => {
                    let over = s.b.cmp(CmpOp::Ult, k(Width::W8, bits as u128), c8);
                    s.if_else(over, undefined, defined)
                }
            }
        };
        self.shift_effects(count, a, res, effects)
    }

    pub(super) fn bit_test(&mut self) -> R {
        let w = self.width(0)?;
        let bits = w.bits();
        let m = self.insn.mnemonic();
        let imm = matches!(self.insn.op_kind(1), OpKind::Immediate8);
        // Bit index within the addressed word.
        let off = self.read_as(1, w)?;
        self.atomic_operand = Some(off);
        let idx = self.b.and(off, k(w, (bits - 1) as u128));
        let a = if self.is_mem(0) && !imm {
            // A register offset addresses memory beyond the operand:
            // ea + (offset sar log2(bits)) * bytes, signed.
            let mut addr = self.ea()?;
            let off32 = self.b.sext(off, Width::W32);
            let words = self.b.bin(BinOp::Sar, off32, c32(bits.trailing_zeros()));
            let delta = self.b.bin(BinOp::Shl, words, c32(w.bytes().trailing_zeros()));
            addr.offset = self.b.add(addr.offset, delta);
            self.ea = Some(addr);
            self.b.load(addr, w)
        } else {
            self.read(0)?
        };
        let sh = self.b.bin(BinOp::Shr, a, idx);
        let cf = self.b.trunc(sh, Width::W1);
        let mask = self.b.bin(BinOp::Shl, k(w, 1), idx);
        let new = match m {
            Mnemonic::Bts => Some(self.b.or(a, mask)),
            Mnemonic::Btr => {
                let nm = self.b.not(mask);
                Some(self.b.and(a, nm))
            }
            Mnemonic::Btc => Some(self.b.xor(a, mask)),
            _ => None,
        };
        if let Some(v) = new {
            self.write(0, v)?;
        }
        self.b.set_flag(Flag::Cf, cf);
        for f in [Flag::Of, Flag::Sf, Flag::Af, Flag::Pf] {
            self.b.undef_flag(f);
        }
        Ok(())
    }

    /// `bsf`/`bsr`. A zero source sets ZF and leaves the destination as it
    /// was (the SDM says undefined; P6-family hardware leaves it alone).
    pub(super) fn bit_scan(&mut self, forward: bool) -> R {
        let w = self.width(0)?;
        let src = self.read(1)?;
        let old = self.read(0)?;
        let zf = self.b.is_zero(src);
        let n = if forward {
            self.b.un(UnOp::Ctz, src)
        } else {
            let lz = self.b.un(UnOp::Clz, src);
            self.b.sub(k(w, (w.bits() - 1) as u128), lz)
        };
        let v = self.b.select(zf, old, n);
        self.write(0, v)?;
        self.b.set_flag(Flag::Zf, zf);
        for f in [Flag::Cf, Flag::Of, Flag::Sf, Flag::Af, Flag::Pf] {
            self.b.undef_flag(f);
        }
        Ok(())
    }

    pub(super) fn cmpxchg(&mut self) -> R {
        let w = self.width(0)?;
        let (ag, as_) = Self::acc_pair(w).0;
        let src = self.read(1)?;
        let acc = self.b.read_reg(ag, as_);
        let old = self.read(0)?;
        self.atomic_operand = Some(src);
        let r = self.b.sub(acc, old);
        self.flags_sub(acc, old, r, true);
        let eq = self.b.cmp(CmpOp::Eq, acc, old);
        let new_dst = self.b.select(eq, src, old);
        let new_acc = self.b.select(eq, acc, old);
        if self.is_mem(0) {
            // The processor always writes the destination back.
            self.write(0, new_dst)?;
            self.b.write_reg(ag, as_, new_acc);
        } else {
            // Accumulator first: if the destination *is* the accumulator
            // and the compare succeeded, the destination write must win.
            self.b.write_reg(ag, as_, new_acc);
            self.write(0, new_dst)?;
        }
        Ok(())
    }

    pub(super) fn xadd(&mut self) -> R {
        let x = self.read(1)?;
        let a = self.read(0)?;
        self.atomic_operand = Some(x);
        self.atomic_old = gpr(self.insn.op_register(1));
        let sum = self.b.add(a, x);
        self.flags_add(a, x, sum, true);
        // TEMP = SRC + DEST; SRC = DEST; DEST = TEMP. Memory first.
        if self.is_mem(0) {
            self.write(0, sum)?;
            self.write(1, a)
        } else {
            self.write(1, a)?;
            self.write(0, sum)
        }
    }

    pub(super) fn flag_insn(&mut self) {
        match self.insn.mnemonic() {
            Mnemonic::Clc => self.b.set_flag(Flag::Cf, Value::bool(false)),
            Mnemonic::Stc => self.b.set_flag(Flag::Cf, Value::bool(true)),
            Mnemonic::Cmc => {
                let cf = self.b.read_flag(Flag::Cf);
                let n = self.b.not(cf);
                self.b.set_flag(Flag::Cf, n);
            }
            Mnemonic::Cld => self.b.set_flag(Flag::Df, Value::bool(false)),
            _ => self.b.set_flag(Flag::Df, Value::bool(true)),
        }
    }
}
