//! The processor-model instructions an Xbox title runs in ring 0: `rdtsc`,
//! `cpuid`, `cli`/`sti` and `wbinvd`.

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
/// - leaf 2: one descriptor round: 4 KB and 4 MB TLBs, 16 KB L1 I and D,
///   and descriptor 0x41 for the 128 KB L2.
///
/// A leaf past 2, extended leaves included, returns leaf 2, as Intel parts
/// without them do.
pub const XBOX_CPUID: [[u32; 4]; 3] = [
    [2, 0x756E_6547, 0x6C65_746E, 0x4965_6E69],
    [0x0000_068A, 0, 0, 0x0383_F9FF],
    [0x0302_0101, 0, 0, 0x0C04_0841],
];

impl Lower<'_> {
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
