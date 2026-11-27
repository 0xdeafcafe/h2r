//! The walk arm (docs/engine/object-update.md, "The walk arm"): the
//! object walks, the per-object updates with `bd090`, and the biped and
//! weapon descriptor callbacks, as Rust that proves itself against the
//! guest before it answers. On with `impl.sim.native.objects.walk` under
//! `impl.sim.native.objects`; `sim_bridge::sim_objects` installs it.
//!
//! **Segments.** The unit is a body segment: from a body's entry or one
//! of its sites to its next transfer ([`Body::segment`]). [`turn`] decides
//! who answers each one. Until the segment's entry has matched
//! [`PROVEN_AFTER`] segments, and on every [`AUDIT_EVERY`]th segment
//! after, the guest answers: [`predict`] runs the Rust segment on the live
//! state inside a [`Guest::trial`], which puts every byte back, and
//! [`Prediction::compare`] checks the guest's transfer, registers and
//! the bytes the Rust segment stored. Otherwise the Rust body answers, and
//! [`super::drive_admitting`] fuses into the next body only when that
//! one's turn is an answer too ([`admit`]).
//!
//! **Fallback.** One difference anywhere hands every segment of every
//! entry back to the guest until the next map ([`reset`]). The proof is
//! per entry and per map. The audit slot is the count of the entry's
//! segments since the map, a checkpoint revert or a restore
//! ([`new_run`]), so it depends on no thread.
//!
//! **Other types.** Only [`POINTS`] are in [`TABLE`]: the unit shell, the
//! vehicle, the small types, damage, functions and lights are calls to
//! the guest, through the runtime, as any callout.

use super::{BODIES, Body, Table};
use crate::points::{self, Point};
use ::sim_objects::phase::{Walk, WalkSet};
use ::sim_objects::view::Objects;
use guest_layout::ObjectType;
use runtime::{CpuAbi, Guest, TransferAbi, abi};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::{LazyLock, Mutex};

/// The arm's bodies: slice 1 (the walks, the per-object updates, the
/// type dispatchers, the wake leaves, the early-mover wake), `bd090`, and
/// the biped (slices 7a and 7b) and weapon (slice 9) trees.
pub const POINTS: &[&Point] = &[
    &points::OBJECTS_PRE_PHYSICS,
    &points::OBJECTS_POST_PHYSICS,
    &points::OBJECTS_FINALIZE,
    &points::OBJECT_PRE_UPDATE,
    &points::OBJECT_POST_UPDATE,
    &points::OBJECT_TYPE_UPDATE,
    &points::OBJECT_TYPE_POST_UPDATE,
    &points::OBJECT_TYPE_POSE,
    &points::OBJECT_TYPE_AFTER_ROOT,
    &points::OBJECT_WAKE,
    &points::OBJECT_ROOT,
    &points::OBJECT_WAKE_FOR_POST,
    &points::OBJECT_WAKE_CHILDREN,
    &points::OBJECT_EARLY_MOVER_WAKE,
    &points::OBJECT_NODE_MATRICES,
    &points::BIPED_UPDATE,
    &points::OBJECT_CARRIER,
    &points::BIPED_TIMER_UPDATE,
    &points::GAME_SECONDS_TO_TICKS,
    &points::UNIT_ACTIONS_UPDATE,
    &points::BIPED_ACTION,
    &points::BIPED_ANIMATION_KIND,
    &points::BIPED_AIRBORNE_TEST,
    &points::BIPED_FALL_UPDATE,
    &points::BIPED_ANIMATION_STATE_UPDATE,
    &points::BIPED_LOOK_UPDATE,
    &points::BIPED_POST_UPDATE,
    &points::BIPED_CROUCH_STEP,
    &points::BIPED_CROUCH_HEIGHT,
    &points::BIPED_CROUCH_FORCED,
    &points::BIPED_GROUND_PROBE,
    &points::BIPED_GROUND_FLAG,
    &points::BIPED_JUMP_UPDATE,
    &points::BIPED_PHYSICS_INPUT,
    &points::BIPED_LANDING_UPDATE,
    &points::BIPED_CONTACT_UPDATE,
    &points::WEAPON_UPDATE,
];

/// The types whose descriptor callbacks are in [`TABLE`].
pub const PORTED: [ObjectType; 2] = [ObjectType::Biped, ObjectType::Weapon];

/// [`POINTS`]' bodies, indexed.
pub static TABLE: LazyLock<Table> = LazyLock::new(|| {
    Table::new(BODIES.iter().filter(|b| POINTS.iter().any(|p| p.base == b.point.base)))
        .expect("BODIES is consistent")
});

/// Matched segments of an entry, with no difference anywhere, before its
/// body answers.
pub const PROVEN_AFTER: u64 = 4096;

/// After that, one segment of the entry in this many is still the
/// guest's, compared.
pub const AUDIT_EVERY: u64 = 64;

/// Entries compared without the XMM registers: their bodies do not keep
/// retail's XMM state, which no caller reads (the differential harness's
/// `FLOAT_OPT_OUT`).
const NO_XMM: [u32; 1] = [points::OBJECT_NODE_MATRICES.base];

/// The first difference an entry saw.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mismatch {
    /// The entry or site the segment began at.
    pub pc: u32,
    pub what: String,
    pub rust: String,
    pub guest: String,
}

/// One entry's counters since the map loaded.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Stats {
    pub matched: u64,
    pub mismatched: u64,
    /// Segments the Rust body answered.
    pub answered: u64,
    /// Segments left to the guest unpredicted: the float environment, or
    /// the Rust segment faulted.
    pub declined: u64,
    /// The guest faulted, stopped or yielded before its transfer.
    pub interrupted: u64,
    pub first: Option<Mismatch>,
}

/// The walks' slot-pass visits since the map loaded, predicted at each
/// walk's entry from its [`WalkSet`]: of a type in [`PORTED`], or not.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Visits {
    pub ported: u64,
    pub guest: u64,
}

#[derive(Default)]
struct State {
    entries: BTreeMap<u32, Stats>,
    slots: BTreeMap<u32, u64>,
    /// A difference was seen: the guest answers until the next map.
    failed: bool,
    visits: Visits,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(Mutex::default);

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

thread_local! {
    static SET: RefCell<WalkSet> = RefCell::new(WalkSet::default());
}

/// Who answers a segment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Turn {
    /// The Rust body runs and commits.
    Answer,
    /// The guest runs; the Rust segment is predicted and compared.
    Compare,
}

/// The entry whose body `pc` enters or resumes.
pub fn entry_of(pc: u32) -> Option<u32> {
    TABLE.at(pc).map(|(body, _)| body.point.base)
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

/// The turn of the segment at `pc`, taking its slot; `None` when no arm
/// body owns `pc`.
pub fn turn(pc: u32) -> Option<Turn> {
    let entry = entry_of(pc)?;
    let mut s = state();
    let answer = s.answers(entry);
    s.advance(entry);
    Some(if answer { Turn::Answer } else { Turn::Compare })
}

/// Whether a fused transfer to `pc` may stay in Rust: its segment's turn
/// is an answer. Takes the slot only then, so a refused transfer's
/// segment gets its turn from [`turn`] when the runtime enters it.
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

/// The Rust body answered the segment at `pc`.
pub fn answered(pc: u32) {
    if let Some(entry) = entry_of(pc) {
        state().entries.entry(entry).or_default().answered += 1;
    }
}

/// The segment at `pc` was left to the guest unpredicted.
pub fn declined(pc: u32) {
    if let Some(entry) = entry_of(pc) {
        state().entries.entry(entry).or_default().declined += 1;
    }
}

/// Forget the map: the proof, the counters, the slots and the fallback.
/// Called at map load.
pub fn reset() {
    *state() = State::default();
}

/// Start the audit slots over (checkpoint revert, restore): the proof,
/// the counters and the fallback stay.
pub fn new_run() {
    state().slots.clear();
}

/// Every entry's counters, in entry address order.
pub fn stats() -> Vec<(u32, Stats)> {
    state().entries.iter().map(|(&k, v)| (k, v.clone())).collect()
}

/// Whether a difference has handed the walks back to the guest.
pub fn failed() -> bool {
    state().failed
}

/// The predicted visits since the map loaded.
pub fn visits() -> Visits {
    state().visits
}

/// A walk enters: gather the pool and count what its slot pass will
/// visit. A pool that does not read counts nothing.
pub fn walk_entered(g: &Guest, walk: Walk) {
    let m: &dyn crate::memory::GuestMemory = g;
    let Ok(Some(pool)) = Objects::read(m) else {
        return;
    };
    let counts = SET.with(|set| {
        let mut set = set.borrow_mut();
        set.gather(m, &pool).ok()?;
        Some(set.visits_by_kind(walk))
    });
    let Some(counts) = counts else {
        return;
    };
    let ported: u64 = PORTED.iter().map(|&k| u64::from(counts[k as usize])).sum();
    let all: u64 = counts.iter().map(|&n| u64::from(n)).sum();
    let mut s = state();
    s.visits.ported += ported;
    s.visits.guest += all - ported;
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

/// Run the Rust segment at `pc` on `cpu`'s copy and the live state, then
/// put every byte back. `None` (counted as declined) when no arm body
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
    /// outcome: the transfer, the integer registers, the flags both sides
    /// define, MXCSR, x87, the XMM registers (except [`NO_XMM`]), and every
    /// byte the Rust segment stored. Bytes only the guest stored are the
    /// differential harness's to check.
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
        if !NO_XMM.contains(&self.entry) {
            for (i, (a, b)) in want.xmm.iter().zip(&cpu.xmm).enumerate() {
                if a != b {
                    return miss(
                        format!("xmm{i}"),
                        format!("{:#034x}", u128::from_le_bytes(*a)),
                        format!("{:#034x}", u128::from_le_bytes(*b)),
                    );
                }
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

/// The walk a body entry starts, for [`walk_entered`].
pub fn walk_at(pc: u32) -> Option<Walk> {
    if pc == points::OBJECTS_PRE_PHYSICS.base {
        Some(Walk::Pre)
    } else if pc == points::OBJECTS_POST_PHYSICS.base {
        Some(Walk::Post)
    } else {
        None
    }
}

/// A body's segment for the answer path: counted as answered.
pub fn answer_segment(body: &Body, cpu: &mut CpuAbi, g: &mut Guest, pc: u32) -> super::Result<TransferAbi> {
    let transfer = body.segment(cpu, g, pc)?;
    answered(pc);
    Ok(transfer)
}

#[cfg(test)]
mod tests;
