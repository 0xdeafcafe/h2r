//! The processor-model instructions an Xbox title runs in ring 0: `rdtsc`,
//! `cpuid`, `cli`/`sti`, `wbinvd`, `sgdt` and the reads of CS the flat
//! segment model makes constant.

use super::*;

/// What `cpuid` returns, by leaf: `[eax, ebx, ecx, edx]`. The Xbox CPU is a
/// 733 MHz Pentium III (Coppermine core, 128 KB L2). Community reports of
/// the retail console give signature 0x68A (family 6, model 8, stepping
/// 10); the rest is what that part reports, not a hardware dump:
///
/// - leaf 0: highest basic leaf 2 (the serial number, leaf 3, is off) and
///   "GenuineIntel";
/// - leaf 1: the signature, brand index 0, and the Pentium III features
///   FPU VME DE PSE TSC MSR PAE MCE CX8 SEP MTRR PGE MCA CMOV PAT PSE36 MMX
///   FXSR SSE (no APIC, no PSN);
/// - leaf 2: one descriptor round: 4 KB and 4 MB TLBs, 16 KB L1 I and D
///   (4-way, 32-byte lines), and 0x41 for the 128 KB L2. The Xbox's L2 is
///   8-way, but Intel defines no 128 KB 8-way descriptor with the P6's
///   32-byte lines (0x79 has 64-byte sectored lines), so this reports 4-way
///   with the right size and line, as the Coppermine Celeron does. Code
///   that sizes buffers from leaf 2 gets the right answer; only the
///   associativity is off.
///
/// A leaf past 2, extended leaves included, returns leaf 2, as Intel parts
/// without them do.
pub const XBOX_CPUID: [[u32; 4]; 3] = [
    [2, 0x756E_6547, 0x6C65_746E, 0x4965_6E69],
    [0x0000_068A, 0, 0, 0x0383_F9FF],
    [0x0302_0101, 0, 0, 0x0C04_0841],
];

/// The code selector of the flat model. Xbox titles run ring 0 with one flat
/// 4 GB code segment at GDT index 1, so `mov r/m, cs` reads 8 and a far
/// `jmp 0008:target` only reloads CS with what it already holds.
pub const XBOX_CS: u16 = 0x08;

/// What `sgdt` stores: the limit (eight descriptors) and linear base of the
/// modelled GDT. These are model values, not a dump: nothing in guest
/// memory backs the base, so code that goes on to read or write the
/// descriptors (the XAPI helper at 0x2d6434 swaps descriptor 1) faults on
/// the access until a runtime maps it. The title does not read them
/// otherwise.
pub const XBOX_GDT: (u16, u32) = (0x3F, 0x8001_0000);

impl Lower<'_> {
    /// `mov r/m, cs`: [`XBOX_CS`], zero-extended into a 32-bit destination.
    /// The other segment registers are not modelled.
    pub(super) fn mov_from_cs(&mut self) -> R {
        let w = self.width(0)?;
        self.write(0, k(w, XBOX_CS as u128))
    }

    /// `jmp ptr16:32` to the flat code selector: the CS reload changes
    /// nothing, so it is a direct jump. Any other selector is not modelled.
    pub(super) fn far_jmp(&mut self) -> R {
        let insn = self.insn;
        if insn.far_branch_selector() != XBOX_CS {
            return Err(format!("far jump to selector {:#x}", insn.far_branch_selector()));
        }
        let tail = matches!(self.site, Site::Tail);
        self.control = Control::Jump { target: Target::Direct(insn.far_branch32()), tail, kind: CallKind::Direct };
        Ok(())
    }

    /// `sgdt m`: the 16-bit limit at `m`, the 32-bit base at `m+2`
    /// ([`XBOX_GDT`]). Not privileged, so it runs in any ring.
    pub(super) fn sgdt(&mut self) -> R {
        let a = self.ea()?;
        let at2 = Addr { seg: a.seg, offset: self.b.add(a.offset, c32(2)) };
        self.b.store(a, k(Width::W16, XBOX_GDT.0 as u128));
        self.b.store(at2, c32(XBOX_GDT.1));
        Ok(())
    }

    /// `rdtsc`: EDX:EAX = the time-stamp counter, which only the runtime
    /// knows (see [`Intrinsic::ReadTsc`]). Titles run in ring 0, where
    /// CR4.TSD cannot fault it. No flags change.
    pub(super) fn rdtsc(&mut self) {
        let t = self.intrinsic(Intrinsic::ReadTsc, vec![])[0];
        let lo = self.b.trunc(t, Width::W32);
        let shifted = self.b.bin(BinOp::Shr, t, k(Width::W64, 32));
        let hi = self.b.trunc(shifted, Width::W32);
        self.b.write_reg(Gpr::Eax, Slice::Full, lo);
        self.b.write_reg(Gpr::Edx, Slice::Full, hi);
    }

    /// `cpuid`: [`XBOX_CPUID`] for the leaf in EAX; ECX is ignored, as no
    /// leaf here has subleaves. No flags change.
    pub(super) fn cpuid(&mut self) {
        let leaf = self.b.read_reg(Gpr::Eax, Slice::Full);
        let is = |l: &mut Self, n: u32| l.b.cmp(CmpOp::Eq, leaf, c32(n));
        let (zero, one) = (is(self, 0), is(self, 1));
        let [l0, l1, l2] = XBOX_CPUID;
        for (i, reg) in [Gpr::Eax, Gpr::Ebx, Gpr::Ecx, Gpr::Edx].into_iter().enumerate() {
            let rest = self.b.select(one, c32(l1[i]), c32(l2[i]));
            let v = self.b.select(zero, c32(l0[i]), rest);
            self.b.write_reg(reg, Slice::Full, v);
        }
    }
}
