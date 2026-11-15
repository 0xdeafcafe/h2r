//! Control transfers: jumps, table jumps, loops, calls and returns.

use super::*;

impl Lower<'_> {
    // ---- control flow ---------------------------------------------------

    pub(super) fn jmp(&mut self) -> R {
        let insn = self.insn;
        match insn.op_kind(0) {
            OpKind::NearBranch32 => {
                let t = insn.near_branch32();
                let tail = matches!(self.site, Site::Tail);
                self.control = Control::Jump { target: Target::Direct(t), tail, kind: CallKind::Direct };
            }
            OpKind::Register | OpKind::Memory => {
                // `66 FF /4`: a 16-bit target (IP = r/m16), not supported.
                if self.width(0)? != Width::W32 {
                    return Err("16-bit indirect jump".into());
                }
                let kind = self.indirect_kind();
                let t = self.read_as(0, Width::W32)?;
                self.control = match self.site {
                    Site::Switch(table) => self.switch(t, table),
                    Site::LocalJump => return Err("indirect jump the CFG calls local".into()),
                    _ => Control::Jump { target: Target::Dynamic(t), tail: true, kind },
                };
            }
            k => return Err(format!("jump operand {k:?}")),
        }
        Ok(())
    }

    /// A table jump's control. Cases start local; the lifter marks those
    /// that land on another function's entry.
    pub(super) fn switch(&self, target: Value, t: &analyze::JumpTable) -> Control {
        let shape = match (t.kind, t.bound, t.index_table) {
            (analyze::SwitchKind::Bounded, Some(entries), _) => SwitchShape::Bounded { entries },
            (analyze::SwitchKind::Indexed, _, Some((index_table, index_entries))) => {
                SwitchShape::Indexed { index_table, index_entries }
            }
            _ => SwitchShape::Unbounded,
        };
        // The entries the code can read: the whole table, plus the byte
        // table for an indexed switch.
        let entries_ro = self.env.is_readonly(t.table, t.count.saturating_mul(4));
        let index_ro = t.index_table.is_none_or(|(a, n)| self.env.is_readonly(a, n));
        Control::Switch {
            target,
            table: t.table,
            shape,
            immutable: entries_ro && index_ro && shape != SwitchShape::Unbounded,
            cases: t.targets.iter().map(|&pc| Dest::local(pc)).collect(),
        }
    }

    /// What a `jmp`/`call` through operand 0 goes through: a kernel import
    /// slot, or an unknown pointer.
    pub(super) fn indirect_kind(&self) -> CallKind {
        let insn = self.insn;
        if insn.op_kind(0) == OpKind::Memory
            && insn.memory_base() == Register::None
            && insn.memory_index() == Register::None
            && let Some(&ordinal) = self.env.kernel_slots.get(&insn.memory_displacement32())
        {
            CallKind::Kernel { slot: insn.memory_displacement32(), ordinal }
        } else {
            CallKind::Indirect
        }
    }

    pub(super) fn loop_insn(&mut self) -> R {
        if self.insn.code() != Code::Loop_rel8_32_ECX
            && self.insn.code() != Code::Loope_rel8_32_ECX
            && self.insn.code() != Code::Loopne_rel8_32_ECX
        {
            return Err("loop with CX counter".into());
        }
        // ECX -= 1 without touching flags, then branch on ECX != 0 (and ZF).
        let ecx = self.b.read_reg(Gpr::Ecx, Slice::Full);
        let n = self.b.sub(ecx, c32(1));
        self.b.write_reg(Gpr::Ecx, Slice::Full, n);
        let mut cond = self.b.cmp(CmpOp::Ne, n, c32(0));
        match self.insn.mnemonic() {
            Mnemonic::Loope => {
                let z = self.b.read_flag(Flag::Zf);
                cond = self.b.and(cond, z);
            }
            Mnemonic::Loopne => {
                let z = self.b.read_flag(Flag::Zf);
                let nz = self.b.not(z);
                cond = self.b.and(cond, nz);
            }
            _ => {}
        }
        self.branch(cond);
        Ok(())
    }

    pub(super) fn call(&mut self) -> R {
        let insn = self.insn;
        let next = insn.next_ip32();
        // The target is evaluated before the push, so `call [esp+4]` sees
        // the caller's ESP.
        let (target, kind) = match insn.op_kind(0) {
            OpKind::NearBranch32 => (Target::Direct(insn.near_branch32()), CallKind::Direct),
            OpKind::Register | OpKind::Memory => {
                // `66 FF /2`: pushes a 16-bit return address and loads IP
                // from r/m16; not supported.
                if self.width(0)? != Width::W32 {
                    return Err("16-bit indirect call".into());
                }
                let kind = self.indirect_kind();
                (Target::Dynamic(self.read_as(0, Width::W32)?), kind)
            }
            k => return Err(format!("call operand {k:?}")),
        };
        self.push_value(c32(next));
        let returns = match self.site {
            Site::Call { returns } => returns,
            _ => true,
        };
        self.control = Control::Call { target, continuation: next, returns, kind };
        Ok(())
    }

    pub(super) fn ret(&mut self) -> R {
        let cleanup = match self.insn.code() {
            Code::Retnd => 0,
            Code::Retnd_imm16 => self.insn.immediate16(),
            _ => return Err("16-bit or far return".into()),
        };
        let esp = self.esp();
        let t = self.b.load(Addr { seg: Segment::Flat, offset: esp }, Width::W32);
        let new = self.b.add(esp, c32(4 + cleanup as u32));
        self.b.write_reg(Gpr::Esp, Slice::Full, new);
        self.control = Control::Return { target: t, cleanup };
        Ok(())
    }
}
