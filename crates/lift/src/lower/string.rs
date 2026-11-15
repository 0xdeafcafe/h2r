//! String instructions (`movs`, `stos`, `lods`, `cmps`, `scas`) and their
//! REP forms.

use super::*;

impl Lower<'_> {
    // ---- string instructions -------------------------------------------

    pub(super) fn string(&mut self) -> R {
        let insn = self.insn;
        let w = Width::from_bytes(insn.memory_size().size() as u32).ok_or("string element size")?;
        let uses_esi = matches!(insn.op_kind(0), OpKind::MemorySegESI) || matches!(insn.op_kind(1), OpKind::MemorySegESI);
        let uses_edi = matches!(insn.op_kind(0), OpKind::MemoryESEDI) || matches!(insn.op_kind(1), OpKind::MemoryESEDI);
        let short = (0..insn.op_count())
            .any(|i| matches!(insn.op_kind(i), OpKind::MemorySegSI | OpKind::MemoryESDI | OpKind::MemorySegDI));
        if short {
            return Err("16-bit address size string op".into());
        }
        let src_seg = match insn.memory_segment() {
            Register::FS if uses_esi => Segment::Fs,
            Register::GS if uses_esi => return Err("GS-relative access".into()),
            _ => Segment::Flat,
        };
        let compares = matches!(insn.mnemonic(), Mnemonic::Cmpsb | Mnemonic::Cmpsw | Mnemonic::Cmpsd | Mnemonic::Scasb | Mnemonic::Scasw | Mnemonic::Scasd);
        // On movs/stos/lods, F2 (repne) repeats exactly like F3 (rep).
        let rep = match (insn.has_repe_prefix(), insn.has_repne_prefix()) {
            (true, _) if compares => Some(RepKind::RepE),
            (_, true) if compares => Some(RepKind::RepNe),
            (true, _) | (_, true) => Some(RepKind::Rep),
            _ => None,
        };
        let mnemonic = insn.mnemonic();
        let body = self.nested(|s| s.string_iteration(mnemonic, w, uses_esi, uses_edi, src_seg))?;
        match rep {
            Some(kind) => self.b.push(Stmt::Repeat { kind, body }),
            None => body.into_iter().for_each(|st| self.b.push(st)),
        }
        Ok(())
    }

    /// One iteration of a string instruction.
    pub(super) fn string_iteration(&mut self, mnemonic: Mnemonic, w: Width, uses_esi: bool, uses_edi: bool, src_seg: Segment) -> R {
        // Step: +size, or -size with DF set.
        let df = self.b.read_flag(Flag::Df);
        let step = self.b.select(df, c32(w.bytes().wrapping_neg()), c32(w.bytes()));
        let acc = Self::acc_pair(w).0;
        let esi = if uses_esi { Some(self.b.read_reg(Gpr::Esi, Slice::Full)) } else { None };
        let edi = if uses_edi { Some(self.b.read_reg(Gpr::Edi, Slice::Full)) } else { None };
        let src = |s| Addr { seg: src_seg, offset: s };
        let dst = |d| Addr { seg: Segment::Flat, offset: d };
        match mnemonic {
            Mnemonic::Movsb | Mnemonic::Movsw | Mnemonic::Movsd => {
                let v = self.b.load(src(esi.unwrap()), w);
                self.b.store(dst(edi.unwrap()), v);
            }
            Mnemonic::Stosb | Mnemonic::Stosw | Mnemonic::Stosd => {
                let v = self.b.read_reg(acc.0, acc.1);
                self.b.store(dst(edi.unwrap()), v);
            }
            Mnemonic::Lodsb | Mnemonic::Lodsw | Mnemonic::Lodsd => {
                let v = self.b.load(src(esi.unwrap()), w);
                self.b.write_reg(acc.0, acc.1, v);
            }
            Mnemonic::Cmpsb | Mnemonic::Cmpsw | Mnemonic::Cmpsd => {
                let x = self.b.load(src(esi.unwrap()), w);
                let y = self.b.load(dst(edi.unwrap()), w);
                self.string_flags(x, y);
            }
            Mnemonic::Scasb | Mnemonic::Scasw | Mnemonic::Scasd => {
                let x = self.b.read_reg(acc.0, acc.1);
                let y = self.b.load(dst(edi.unwrap()), w);
                self.string_flags(x, y);
            }
            _ => return Err("string op".into()),
        }
        if let Some(s) = esi {
            let n = self.b.add(s, step);
            self.b.write_reg(Gpr::Esi, Slice::Full, n);
        }
        if let Some(d) = edi {
            let n = self.b.add(d, step);
            self.b.write_reg(Gpr::Edi, Slice::Full, n);
        }
        Ok(())
    }

    /// `cmps`/`scas` flags (inside the iteration that produced them).
    pub(super) fn string_flags(&mut self, x: Value, y: Value) {
        let r = self.b.sub(x, y);
        self.flags_sub(x, y, r, true);
    }
}
