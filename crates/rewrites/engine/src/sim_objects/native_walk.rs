//! The walks in Rust (docs/engine/object-update.md, "The walks in Rust"):
//! pre-physics `b6f10`, post-physics `b7060`, the per-object updates
//! `bc470` and `bc820` with the node matrices `bd090`, the four type
//! dispatchers, the wake leaves and the early-mover wake `109580`, and the
//! type callbacks of the objects a multiplayer game has most of: the biped
//! update `dd360` and post-physics update `dd990` with the ported part of
//! their trees (slices 7a and 7b), and the weapon update `fdfb0` (slice
//! 9). The dispatchers call them in Rust. On by default with
//! `impl.sim.native.objects.walk`; `sim_bridge::sim_objects` installs
//! [`TABLE`] when `impl.sim.native.objects` is off (on, every body is
//! installed and these with them).
//!
//! The Rust body answers every segment; nothing is compared. A body the
//! Rust cannot take for one call is a [`Refusal`]: the guest runs that
//! call (one object's `bd090`, one weapon's `fdfb0`) and the caller's Rust
//! frame resumes at its site. The walks, the updates, the dispatchers and
//! the wake leaves read and write no float state ([`FLOAT_FREE`]), so they
//! run whatever the float environment and never refuse; `bd090` and the
//! type callbacks ([`TYPES`]) run only inside it.
//!
//! **Other types.** Only [`POINTS`] and [`TYPES`] are in [`TABLE`]: the
//! unit level of a biped (`c6b30` and its tree), the item level of a
//! weapon (`10b4d0`), the other types' callbacks, damage, functions, the rest of the
//! post-update shell, lights and finalize `b7150` are calls to the guest,
//! through the runtime, as any callout. So are the two unit leaves the
//! biped tree calls, the direction clamp `cba50` and the animation state
//! `111650`.

use super::{BODIES, Body, Table};
use crate::points::{self, Point};
use runtime::{CpuAbi, Guest, TransferAbi};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// The walk bodies: the two walks, the per-object updates, `bd090`, the
/// type dispatchers, the wake leaves and the early-mover wake. [`TABLE`]
/// holds these and [`TYPES`].
pub const POINTS: &[&Point] = &[
    &points::OBJECTS_PRE_PHYSICS,
    &points::OBJECTS_POST_PHYSICS,
    &points::OBJECT_PRE_UPDATE,
    &points::OBJECT_POST_UPDATE,
    &points::OBJECT_NODE_MATRICES,
    &points::OBJECT_TYPE_UPDATE,
    &points::OBJECT_TYPE_POST_UPDATE,
    &points::OBJECT_TYPE_POSE,
    &points::OBJECT_TYPE_AFTER_ROOT,
    &points::OBJECT_WAKE,
    &points::OBJECT_ROOT,
    &points::OBJECT_WAKE_FOR_POST,
    &points::OBJECT_WAKE_CHILDREN,
    &points::OBJECT_EARLY_MOVER_WAKE,
];

/// The type callbacks in [`TABLE`]: the `bipd` descriptor's slots `+40`
/// (`dd360`, slice 7a) and `+44` (`dd990`, slice 7b) with every body their
/// trees fuse, and the `weap` slot `+40` (`fdfb0`, slice 9). Each checks
/// the float environment at its entry and its sites.
pub const TYPES: &[&Point] = &[
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

/// The entries whose bodies make no float instruction: every one in
/// [`POINTS`] but `bd090`.
pub const FLOAT_FREE: &[u32] = &[
    points::OBJECTS_PRE_PHYSICS.base,
    points::OBJECTS_POST_PHYSICS.base,
    points::OBJECT_PRE_UPDATE.base,
    points::OBJECT_POST_UPDATE.base,
    points::OBJECT_TYPE_UPDATE.base,
    points::OBJECT_TYPE_POST_UPDATE.base,
    points::OBJECT_TYPE_POSE.base,
    points::OBJECT_TYPE_AFTER_ROOT.base,
    points::OBJECT_WAKE.base,
    points::OBJECT_ROOT.base,
    points::OBJECT_WAKE_FOR_POST.base,
    points::OBJECT_WAKE_CHILDREN.base,
    points::OBJECT_EARLY_MOVER_WAKE.base,
];

/// The entries whose hooks never transfer to their own guest body: the
/// two walks and the per-object updates (`HookRef::replaces_guest`). Each
/// has resume sites, so `hooks::Plan::replaced` keeps its body lifted.
pub const REPLACES_GUEST: &[u32] = &[
    points::OBJECTS_PRE_PHYSICS.base,
    points::OBJECTS_POST_PHYSICS.base,
    points::OBJECT_PRE_UPDATE.base,
    points::OBJECT_POST_UPDATE.base,
];

/// [`POINTS`]' and [`TYPES`]' bodies, indexed, with [`FLOAT_FREE`]
/// marked.
pub static TABLE: LazyLock<Table> = LazyLock::new(|| {
    Table::new(BODIES.iter().filter(|b| POINTS.iter().chain(TYPES).any(|p| p.base == b.point.base)))
        .expect("BODIES is consistent")
        .float_free(FLOAT_FREE)
});

/// Why the guest runs one call of a body in [`TABLE`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Refusal {
    /// `bd090` or a type callback entered or resumed outside the float
    /// environment its arithmetic assumes ([`super::supported`]).
    FloatEnv,
}

impl Refusal {
    pub const ALL: [Refusal; 1] = [Refusal::FloatEnv];

    /// The name `sim.arms` and the counters use.
    pub fn name(self) -> &'static str {
        match self {
            Refusal::FloatEnv => "float_env",
        }
    }
}

/// One body's counters since the process started.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Stats {
    pub entry: u32,
    /// Segments the Rust body ran (entry or site to the next transfer).
    pub segments: u64,
    /// Calls or resumes the guest ran, by [`Refusal::ALL`] position.
    pub refused: [u64; Refusal::ALL.len()],
}

struct Counters {
    segments: Vec<AtomicU64>,
    refused: Vec<[AtomicU64; Refusal::ALL.len()]>,
}

static COUNTERS: LazyLock<Counters> = LazyLock::new(|| {
    let n = TABLE.bodies().len();
    Counters {
        segments: (0..n).map(|_| AtomicU64::new(0)).collect(),
        refused: (0..n).map(|_| std::array::from_fn(|_| AtomicU64::new(0))).collect(),
    }
});

/// One segment of a body in [`TABLE`], counted: the runner
/// [`super::drive_with`] takes.
pub fn segment(body: &Body, cpu: &mut CpuAbi, g: &mut Guest, pc: u32) -> super::Result<TransferAbi> {
    let transfer = body.segment(cpu, g, pc)?;
    if let Some(i) = TABLE.index(pc) {
        COUNTERS.segments[i].fetch_add(1, Ordering::Relaxed);
    }
    Ok(transfer)
}

/// The guest runs the call or resume at `pc` because of `why`.
pub fn refused(pc: u32, why: Refusal) {
    if let Some(i) = TABLE.index(pc) {
        let at = Refusal::ALL.iter().position(|&r| r == why).expect("listed");
        COUNTERS.refused[i][at].fetch_add(1, Ordering::Relaxed);
    }
}

/// Every body's counters, in [`TABLE`] order.
pub fn stats() -> Vec<Stats> {
    TABLE
        .bodies()
        .iter()
        .enumerate()
        .map(|(i, body)| Stats {
            entry: body.point.base,
            segments: COUNTERS.segments[i].load(Ordering::Relaxed),
            refused: std::array::from_fn(|r| COUNTERS.refused[i][r].load(Ordering::Relaxed)),
        })
        .collect()
}

#[cfg(test)]
mod tests;
