//! The per-effect bodies' arm (docs/engine/effects-parts.md, "The
//! per-effect bodies"): the walk arm's rule (`crate::sim_objects::walk_arm`)
//! over [`super::TABLE`].
//!
//! Each segment (a body's entry or site to its next transfer) gets a
//! [`turn`]. Until the segment's entry has matched [`PROVEN_AFTER`]
//! segments, and on every [`AUDIT_EVERY`]th after, the guest answers:
//! [`predict`] runs the Rust segment inside a [`Guest::trial`], which puts
//! every byte back, and [`Prediction::compare`] checks the guest's
//! transfer, every register, the flags both sides define, x87, XMM and
//! each byte the Rust segment stored. Otherwise the Rust body answers.
//! One difference hands all four entries back to the guest until the next
//! map ([`reset`]); [`new_run`] restarts the audit slots at a revert or a
//! restore.

use super::TABLE;
use crate::sim_objects::{Body, Result};
use runtime::{CpuAbi, Guest, TransferAbi, abi};
use std::collections::BTreeMap;
use std::sync::{LazyLock, Mutex};

pub use crate::sim_objects::walk_arm::{Mismatch, Stats, Turn};

/// Matched segments of an entry, with no difference anywhere, before its
/// body answers.
pub const PROVEN_AFTER: u64 = 4096;

/// After that, one segment of the entry in this many is still the
/// guest's, compared.
pub const AUDIT_EVERY: u64 = 64;

#[derive(Default)]
struct State {
    entries: BTreeMap<u32, Stats>,
    slots: BTreeMap<u32, u64>,
    /// A difference was seen: the guest answers until the next map.
    failed: bool,
}

impl State {
    fn answers(&self, entry: u32) -> bool {
        let slot = self.slots.get(&entry).copied().unwrap_or(0);
        !self.failed
            && slot % AUDIT_EVERY != 0
            && self.entries.get(&entry).is_some_and(|s| s.mismatched == 0 && s.matched >= PROVEN_AFTER)
    }

    fn advance(&mut self, entry: u32) {
        *self.slots.entry(entry).or_default() += 1;
    }
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(Mutex::default);

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The entry whose body `pc` enters or resumes.
pub fn entry_of(pc: u32) -> Option<u32> {
    TABLE.at(pc).map(|(body, _)| body.point.base)
}

/// The turn of the segment at `pc`, taking its slot; `None` when no body
/// owns `pc`.
pub fn turn(pc: u32) -> Option<Turn> {
    let entry = entry_of(pc)?;
    let mut s = state();
    let answer = s.answers(entry);
    s.advance(entry);
    Some(if answer { Turn::Answer } else { Turn::Compare })
}

/// Whether a fused transfer to `pc` may stay in Rust: its segment's turn
/// is an answer. Takes the slot only then.
pub fn admit(pc: u32) -> bool {
    let Some(entry) = entry_of(pc) else {
        return false;
    };
    let mut s = state();
    let answer = s.answers(entry);
    if answer {
        s.advance(entry);
    }
    answer
}

fn count(pc: u32, f: impl FnOnce(&mut Stats)) {
    if let Some(entry) = entry_of(pc) {
        f(state().entries.entry(entry).or_default());
    }
}

/// The Rust body answered the segment at `pc`.
pub fn answered(pc: u32) {
    count(pc, |s| s.answered += 1);
}

/// The segment at `pc` was left to the guest unpredicted.
pub fn declined(pc: u32) {
    count(pc, |s| s.declined += 1);
}

/// Forget the map: the proof, the counters, the slots and the fallback.
pub fn reset() {
    *state() = State::default();
}

/// Start the audit slots over (checkpoint revert, restore).
pub fn new_run() {
    state().slots.clear();
}

/// Every entry's counters, in entry address order.
pub fn stats() -> Vec<(u32, Stats)> {
    state().entries.iter().map(|(&k, v)| (k, v.clone())).collect()
}

/// Whether a difference has handed the bodies back to the guest.
pub fn failed() -> bool {
    state().failed
}

/// The Rust segment's outcome, for the guest's to be compared with.
#[must_use]
pub struct Prediction {
    entry: u32,
    pc: u32,
    cpu: CpuAbi,
    transfer: TransferAbi,
    stores: Vec<(u32, u8)>,
}

/// Run the Rust segment at `pc` on a copy of `cpu` and the live state,
/// then put every byte back. `None` (counted as declined) when no body
/// owns `pc` or the segment faults.
pub fn predict(cpu: &CpuAbi, g: &mut Guest, pc: u32) -> Option<Prediction> {
    let (body, _) = TABLE.at(pc)?;
    let entry = body.point.base;
    let mut c = *cpu;
    let (result, stores) = g.trial(|g| body.segment(&mut c, g, pc));
    let Ok(transfer) = result else {
        declined(pc);
        return None;
    };
    // The generated tiers leave the outgoing target in EIP.
    c.eip = transfer.target_pc;
    Some(Prediction { entry, pc, cpu: c, transfer, stores: stores.bytes })
}

impl Prediction {
    /// The guest could not run to a transfer.
    pub fn guest_error(self) {
        state().entries.entry(self.entry).or_default().interrupted += 1;
    }

    /// Check the guest's outcome: a difference fails the arm.
    pub fn compare(self, cpu: &CpuAbi, g: &Guest, transfer: TransferAbi) {
        if matches!(transfer.kind, abi::FAULT | abi::STOP)
            || (transfer.kind == abi::JUMP && transfer.route == abi::RESUME)
        {
            self.guest_error();
            return;
        }
        let difference = self.difference(cpu, g, transfer);
        let mut guard = state();
        let s = &mut *guard;
        let stats = s.entries.entry(self.entry).or_default();
        match difference {
            Some(m) => {
                stats.mismatched += 1;
                stats.first.get_or_insert(m);
                s.failed = true;
            }
            None => stats.matched += 1,
        }
    }

    /// The first difference between the prediction and the guest's
    /// outcome, in the walk arm's order.
    pub fn difference(&self, cpu: &CpuAbi, g: &Guest, transfer: TransferAbi) -> Option<Mismatch> {
        let miss = |what: String, rust: String, guest: String| Some(Mismatch { pc: self.pc, what, rust, guest });
        if self.transfer != transfer {
            return miss("transfer".into(), format!("{:x?}", self.transfer), format!("{transfer:x?}"));
        }
        let want = &self.cpu;
        for (name, a, b) in [
            ("eax", want.eax, cpu.eax),
            ("ecx", want.ecx, cpu.ecx),
            ("edx", want.edx, cpu.edx),
            ("ebx", want.ebx, cpu.ebx),
            ("esp", want.esp, cpu.esp),
            ("ebp", want.ebp, cpu.ebp),
            ("esi", want.esi, cpu.esi),
            ("edi", want.edi, cpu.edi),
            ("eip", want.eip, cpu.eip),
            ("mxcsr", want.mxcsr, cpu.mxcsr),
        ] {
            if a != b {
                return miss(name.into(), format!("{a:#010x}"), format!("{b:#010x}"));
            }
        }
        let both = want.eflags_defined & cpu.eflags_defined;
        if want.eflags & both != cpu.eflags & both {
            return miss("eflags".into(), format!("{:#x}", want.eflags & both), format!("{:#x}", cpu.eflags & both));
        }
        if want.x87 != cpu.x87 {
            return miss("x87".into(), format!("{:x?}", want.x87), format!("{:x?}", cpu.x87));
        }
        for (i, (a, b)) in want.xmm.iter().zip(&cpu.xmm).enumerate() {
            if a != b {
                return miss(
                    format!("xmm{i}"),
                    format!("{:#034x}", u128::from_le_bytes(*a)),
                    format!("{:#034x}", u128::from_le_bytes(*b)),
                );
            }
        }
        for &(addr, value) in &self.stores {
            let got = g.u8(addr).ok();
            if got != Some(value) {
                return miss(
                    format!("memory {addr:#010x}"),
                    format!("{value:#04x}"),
                    got.map_or_else(|| "unmapped".into(), |b| format!("{b:#04x}")),
                );
            }
        }
        None
    }
}

/// A body's segment for the answer path: counted as answered.
pub fn answer_segment(body: &Body, cpu: &mut CpuAbi, g: &mut Guest, pc: u32) -> Result<TransferAbi> {
    let transfer = body.segment(cpu, g, pc)?;
    answered(pc);
    Ok(transfer)
}
