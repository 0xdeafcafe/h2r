//! MMX, and the SSE integer instructions on MMX registers (`pshufw`,
//! `pinsrw`, `pmaxsw`, ...): plain IR, lane by lane. No helpers are needed,
//! as everything is exact integer arithmetic.
//!
//! MMX registers are the x87 significands ([`Stmt::ReadMmx`]); every
//! instruction that names one ends with [`Intrinsic::MmxEnter`] (TOP = 0,
//! all tags valid), after its faulting accesses, so a fault leaves the x87
//! state alone. `emms` empties the tag word.

use super::*;

/// The lane width of an MMX operation.
const B: Width = Width::W8;
const W: Width = Width::W16;
const D: Width = Width::W32;
const Q: Width = Width::W64;

impl Lower<'_> {
    pub(super) fn mm(&self, i: u32) -> R<u8> {
        let r = self.insn.op_register(i);
        if self.insn.op_kind(i) == OpKind::Register && (Register::MM0..=Register::MM7).contains(&r) {
            Ok(r as u8 - Register::MM0 as u8)
        } else {
            Err(format!("operand {i} is not mm0-7"))
        }
    }

    pub(super) fn read_mmx(&mut self, reg: u8) -> Value {
        let dst = self.b.temp(Q);
        self.b.push(Stmt::ReadMmx { dst, reg });
        dst.into()
    }

    pub(super) fn write_mmx(&mut self, reg: u8, value: Value) {
        self.b.push(Stmt::WriteMmx { reg, value });
    }

    /// Operand `i` as 64 bits: an MMX register or m64 (MMX has no alignment
    /// rule).
    pub(super) fn mmx_or_m64(&mut self, i: u32) -> R<Value> {
        if self.is_mem(i) {
            let a = self.ea()?;
            Ok(self.b.load(a, Q))
        } else {
            let r = self.mm(i)?;
            Ok(self.read_mmx(r))
        }
    }

    fn split(&mut self, v: Value, w: Width) -> Vec<Value> {
        if w == Q {
            return vec![v];
        }
        let n = w.bits();
        (0..64 / n)
            .map(|i| {
                let s = self.b.bin(BinOp::Shr, v, k(Q, (i * n) as u128));
                self.b.trunc(s, w)
            })
            .collect()
    }

    fn join(&mut self, lanes: &[Value]) -> Value {
        if lanes.len() == 1 {
            return lanes[0];
        }
        let n = lanes[0].width().bits();
        let mut acc = k(Q, 0);
        for (i, l) in lanes.iter().enumerate() {
            let w = self.b.zext(*l, Q);
            let w = self.b.bin(BinOp::Shl, w, k(Q, i as u128 * n as u128));
            acc = self.b.or(acc, w);
        }
        acc
    }

    fn lanewise(&mut self, a: Value, b: Value, w: Width, f: impl Fn(&mut Self, Value, Value) -> Value) -> Value {
        let (la, lb) = (self.split(a, w), self.split(b, w));
        let mut out = Vec::with_capacity(la.len());
        for (x, y) in la.into_iter().zip(lb) {
            out.push(f(self, x, y));
        }
        self.join(&out)
    }

    /// `v` as a 32-bit integer, sign- or zero-extended.
    fn widen(&mut self, v: Value, signed: bool) -> Value {
        match (v.width(), signed) {
            (Width::W32, _) => v,
            (_, true) => self.b.sext(v, D),
            (_, false) => self.b.zext(v, D),
        }
    }

    /// Clamp the signed 32-bit `x` to `lo..=hi` and narrow it to `w`.
    fn saturate(&mut self, x: Value, lo: i32, hi: i32, w: Width) -> Value {
        let below = self.b.cmp(CmpOp::Slt, x, c32(lo as u32));
        let above = self.b.cmp(CmpOp::Slt, c32(hi as u32), x);
        let v = self.b.select(below, c32(lo as u32), x);
        let v = self.b.select(above, c32(hi as u32), v);
        if w == D { v } else { self.b.trunc(v, w) }
    }

    fn range(w: Width, signed: bool) -> (i32, i32) {
        match (w, signed) {
            (Width::W8, true) => (-128, 127),
            (Width::W8, false) => (0, 255),
            (Width::W16, true) => (-32768, 32767),
            _ => (0, 65535),
        }
    }

    /// Saturating add/sub of lanes `x`, `y` of width `w`.
    fn sat_arith(&mut self, x: Value, y: Value, w: Width, signed: bool, add: bool) -> Value {
        let (x, y) = (self.widen(x, signed), self.widen(y, signed));
        let r = if add { self.b.add(x, y) } else { self.b.sub(x, y) };
        let (lo, hi) = Self::range(w, signed);
        self.saturate(r, lo, hi, w)
    }

    fn ones(w: Width) -> Value {
        k(w, w.mask())
    }

    /// The shift count of `psll`/`psrl`/`psra`: an immediate, or the whole
    /// 64 bits of an MMX register or m64.
    fn mmx_count(&mut self) -> R<Result<u64, Value>> {
        if self.insn.op_kind(1) == OpKind::Immediate8 {
            Ok(Ok(self.insn.immediate8() as u64))
        } else {
            Ok(Err(self.mmx_or_m64(1)?))
        }
    }

    /// Lane shift: counts of the lane width or more give 0 (logical) or the
    /// sign (arithmetic), which is what the IR's shifts do at count = width.
    fn mmx_shift(&mut self, op: BinOp, w: Width) -> R<Value> {
        let count = self.mmx_count()?;
        let d = self.mm(0)?;
        let a = self.read_mmx(d);
        let n = w.bits() as u64;
        let c = match count {
            Ok(imm) => k(w, imm.min(n) as u128),
            Err(c) if w == Q => c,
            Err(c) => {
                let small = self.b.cmp(CmpOp::Ult, c, k(Q, n as u128));
                let t = self.b.trunc(c, w);
                self.b.select(small, t, k(w, n as u128))
            }
        };
        Ok(self.lanewise(a, a, w, |l, x, _| l.b.bin(op, x, c)))
    }

    /// `punpckl*` (low halves interleaved; the memory form reads m32) and
    /// `punpckh*` (high halves).
    fn unpack(&mut self, w: Width, high: bool) -> R<Value> {
        let src = if self.is_mem(1) {
            let a = self.ea()?;
            let n = self.insn.memory_size().size() as u32;
            let mw = Width::from_bytes(n).ok_or("odd punpck memory size")?;
            let v = self.b.load(a, mw);
            if mw == Q {
                v
            } else {
                // m32 is the low half; the high half is never used.
                self.b.zext(v, Q)
            }
        } else {
            let r = self.mm(1)?;
            self.read_mmx(r)
        };
        let d = self.mm(0)?;
        let dst = self.read_mmx(d);
        let (ld, ls) = (self.split(dst, w), self.split(src, w));
        let half = ld.len() / 2;
        let base = if high { half } else { 0 };
        let mut out = Vec::new();
        for i in 0..half {
            out.push(ld[base + i]);
            out.push(ls[base + i]);
        }
        Ok(self.join(&out))
    }

    /// `packsswb`, `packssdw`, `packuswb`: dst's lanes then src's, each
    /// saturated to the narrower width.
    fn pack(&mut self, from: Width, to: Width, signed: bool) -> R<Value> {
        let src = self.mmx_or_m64(1)?;
        let d = self.mm(0)?;
        let dst = self.read_mmx(d);
        let mut lanes = self.split(dst, from);
        lanes.extend(self.split(src, from));
        let (lo, hi) = Self::range(to, signed);
        let mut out = Vec::new();
        for l in lanes {
            let x = self.widen(l, true);
            out.push(self.saturate(x, lo, hi, to));
        }
        Ok(self.join(&out))
    }

    /// `op mm, mm/m64`, lane by lane.
    fn mmx_binary(&mut self, w: Width, f: impl Fn(&mut Self, Value, Value) -> Value) -> R<Value> {
        let src = self.mmx_or_m64(1)?;
        let d = self.mm(0)?;
        let dst = self.read_mmx(d);
        Ok(self.lanewise(dst, src, w, f))
    }

    /// The MMX instructions (and SSE's MMX-register integer ones). Any
    /// other mnemonic is not implemented.
    pub(super) fn mmx(&mut self) -> R {
        use Mnemonic as M;
        let m = self.insn.mnemonic();
        let ss = |signed: bool, add: bool| move |l: &mut Self, x: Value, y: Value| {
            let w = x.width();
            l.sat_arith(x, y, w, signed, add)
        };
        let v = match m {
            M::Emms => {
                self.intrinsic(Intrinsic::Emms, vec![]);
                return Ok(());
            }
            M::Movd => return self.movd(),
            M::Movq | M::Movntq => return self.movq(),
            M::Pextrw | M::Pmovmskb => return self.mmx_to_gpr(),

            M::Paddb | M::Paddw | M::Paddd | M::Paddq => {
                let w = [B, W, D, Q][[M::Paddb, M::Paddw, M::Paddd, M::Paddq].iter().position(|x| *x == m).unwrap()];
                self.mmx_binary(w, |l, x, y| l.b.add(x, y))?
            }
            M::Psubb | M::Psubw | M::Psubd | M::Psubq => {
                let w = [B, W, D, Q][[M::Psubb, M::Psubw, M::Psubd, M::Psubq].iter().position(|x| *x == m).unwrap()];
                self.mmx_binary(w, |l, x, y| l.b.sub(x, y))?
            }
            M::Paddsb => self.mmx_binary(B, ss(true, true))?,
            M::Paddsw => self.mmx_binary(W, ss(true, true))?,
            M::Paddusb => self.mmx_binary(B, ss(false, true))?,
            M::Paddusw => self.mmx_binary(W, ss(false, true))?,
            M::Psubsb => self.mmx_binary(B, ss(true, false))?,
            M::Psubsw => self.mmx_binary(W, ss(true, false))?,
            M::Psubusb => self.mmx_binary(B, ss(false, false))?,
            M::Psubusw => self.mmx_binary(W, ss(false, false))?,

            M::Pmullw => self.mmx_binary(W, |l, x, y| l.b.bin(BinOp::Mul, x, y))?,
            M::Pmulhw | M::Pmulhuw => {
                let signed = m == M::Pmulhw;
                self.mmx_binary(W, move |l, x, y| {
                    let (x, y) = (l.widen(x, signed), l.widen(y, signed));
                    let p = l.b.bin(BinOp::Mul, x, y);
                    let h = l.b.bin(BinOp::Shr, p, c32(16));
                    l.b.trunc(h, W)
                })?
            }
            M::Pmaddwd => {
                let src = self.mmx_or_m64(1)?;
                let d = self.mm(0)?;
                let dst = self.read_mmx(d);
                let (a, b) = (self.split(dst, W), self.split(src, W));
                let mut p = Vec::new();
                for (x, y) in a.into_iter().zip(b) {
                    let (x, y) = (self.widen(x, true), self.widen(y, true));
                    p.push(self.b.bin(BinOp::Mul, x, y));
                }
                let lo = self.b.add(p[0], p[1]);
                let hi = self.b.add(p[2], p[3]);
                self.join(&[lo, hi])
            }

            M::Pand => self.mmx_binary(Q, |l, x, y| l.b.and(x, y))?,
            M::Por => self.mmx_binary(Q, |l, x, y| l.b.or(x, y))?,
            M::Pxor => self.mmx_binary(Q, |l, x, y| l.b.xor(x, y))?,
            M::Pandn => self.mmx_binary(Q, |l, x, y| {
                let n = l.b.not(x);
                l.b.and(n, y)
            })?,

            M::Psllw => self.mmx_shift(BinOp::Shl, W)?,
            M::Pslld => self.mmx_shift(BinOp::Shl, D)?,
            M::Psllq => self.mmx_shift(BinOp::Shl, Q)?,
            M::Psrlw => self.mmx_shift(BinOp::Shr, W)?,
            M::Psrld => self.mmx_shift(BinOp::Shr, D)?,
            M::Psrlq => self.mmx_shift(BinOp::Shr, Q)?,
            M::Psraw => self.mmx_shift(BinOp::Sar, W)?,
            M::Psrad => self.mmx_shift(BinOp::Sar, D)?,

            M::Punpcklbw => self.unpack(B, false)?,
            M::Punpcklwd => self.unpack(W, false)?,
            M::Punpckldq => self.unpack(D, false)?,
            M::Punpckhbw => self.unpack(B, true)?,
            M::Punpckhwd => self.unpack(W, true)?,
            M::Punpckhdq => self.unpack(D, true)?,
            M::Packsswb => self.pack(W, B, true)?,
            M::Packssdw => self.pack(D, W, true)?,
            M::Packuswb => self.pack(W, B, false)?,

            M::Pcmpeqb | M::Pcmpeqw | M::Pcmpeqd | M::Pcmpgtb | M::Pcmpgtw | M::Pcmpgtd => {
                use M::*;
                let (w, eq) = match m {
                    Pcmpeqb => (B, true),
                    Pcmpeqw => (W, true),
                    Pcmpeqd => (D, true),
                    Pcmpgtb => (B, false),
                    Pcmpgtw => (W, false),
                    _ => (D, false),
                };
                self.mmx_binary(w, move |l, x, y| {
                    let c = if eq { l.b.cmp(CmpOp::Eq, x, y) } else { l.b.cmp(CmpOp::Slt, y, x) };
                    l.b.select(c, Self::ones(w), k(w, 0))
                })?
            }

            M::Pmaxsw | M::Pminsw | M::Pmaxub | M::Pminub => {
                let (w, op, max) = match m {
                    M::Pmaxsw => (W, CmpOp::Slt, true),
                    M::Pminsw => (W, CmpOp::Slt, false),
                    M::Pmaxub => (B, CmpOp::Ult, true),
                    _ => (B, CmpOp::Ult, false),
                };
                self.mmx_binary(w, move |l, x, y| {
                    let lt = l.b.cmp(op, x, y);
                    if max { l.b.select(lt, y, x) } else { l.b.select(lt, x, y) }
                })?
            }
            M::Pavgb | M::Pavgw => {
                let w = if m == M::Pavgb { B } else { W };
                self.mmx_binary(w, move |l, x, y| {
                    let (x, y) = (l.widen(x, false), l.widen(y, false));
                    let s = l.b.add(x, y);
                    let s = l.b.add(s, c32(1));
                    let s = l.b.bin(BinOp::Shr, s, c32(1));
                    l.b.trunc(s, w)
                })?
            }
            M::Psadbw => {
                let src = self.mmx_or_m64(1)?;
                let d = self.mm(0)?;
                let dst = self.read_mmx(d);
                let (a, b) = (self.split(dst, B), self.split(src, B));
                let mut sum = c32(0);
                for (x, y) in a.into_iter().zip(b) {
                    let (x, y) = (self.widen(x, false), self.widen(y, false));
                    let lt = self.b.cmp(CmpOp::Ult, x, y);
                    let xy = self.b.sub(x, y);
                    let yx = self.b.sub(y, x);
                    let diff = self.b.select(lt, yx, xy);
                    sum = self.b.add(sum, diff);
                }
                self.b.zext(sum, Q)
            }

            M::Pshufw => {
                let src = self.mmx_or_m64(1)?;
                let imm = self.insn.immediate8() as usize;
                let words = self.split(src, W);
                let out: Vec<Value> = (0..4).map(|i| words[(imm >> (2 * i)) & 3]).collect();
                self.join(&out)
            }
            M::Pinsrw => {
                let v = self.read(1)?;
                let v = if v.width() == W { v } else { self.b.trunc(v, W) };
                let d = self.mm(0)?;
                let dst = self.read_mmx(d);
                let mut words = self.split(dst, W);
                words[(self.insn.immediate8() & 3) as usize] = v;
                self.join(&words)
            }
            _ => return Err("not implemented".into()),
        };
        let d = self.mm(0)?;
        self.write_mmx(d, v);
        self.intrinsic(Intrinsic::MmxEnter, vec![]);
        Ok(())
    }

    /// `movd mm, r/m32` (zero-extended) and `movd r/m32, mm` (low half).
    fn movd(&mut self) -> R {
        if let Ok(d) = self.mm(0) {
            let v = self.read_as(1, D)?;
            if v.width() != D {
                return Err("movd source is not 32-bit".into());
            }
            let v = self.b.zext(v, Q);
            self.write_mmx(d, v);
        } else {
            let s = self.mm(1)?;
            if !self.is_mem(0) && self.width(0)? != D {
                return Err("movd destination is not a 32-bit register".into());
            }
            let v = self.read_mmx(s);
            let lo = self.b.trunc(v, D);
            self.write(0, lo)?;
        }
        self.intrinsic(Intrinsic::MmxEnter, vec![]);
        Ok(())
    }

    /// `movq mm, mm/m64`, `movq m64, mm` and `movntq m64, mm`.
    fn movq(&mut self) -> R {
        if self.is_mem(0) {
            let s = self.mm(1)?;
            let v = self.read_mmx(s);
            let a = self.ea()?;
            self.b.store(a, v);
        } else {
            let d = self.mm(0)?;
            let v = self.mmx_or_m64(1)?;
            self.write_mmx(d, v);
        }
        self.intrinsic(Intrinsic::MmxEnter, vec![]);
        Ok(())
    }

    /// `pextrw r32, mm, imm8` (the word, zero-extended) and
    /// `pmovmskb r32, mm` (the eight byte sign bits).
    fn mmx_to_gpr(&mut self) -> R {
        let s = self.mm(1)?;
        if self.width(0)? != D {
            return Err("destination is not a 32-bit register".into());
        }
        let v = self.read_mmx(s);
        let r = if self.insn.mnemonic() == Mnemonic::Pextrw {
            let words = self.split(v, W);
            let w = words[(self.insn.immediate8() & 3) as usize];
            self.b.zext(w, D)
        } else {
            let mut acc = c32(0);
            for i in 0..8 {
                let sign = self.b.bit(v, 8 * i + 7);
                let w = self.b.zext(sign, D);
                let w = self.b.bin(BinOp::Shl, w, c32(i));
                acc = self.b.or(acc, w);
            }
            acc
        };
        self.write(0, r)?;
        self.intrinsic(Intrinsic::MmxEnter, vec![]);
        Ok(())
    }
}
