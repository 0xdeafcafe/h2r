//! The walks in Rust (docs/engine/object-update.md, "The walks in Rust"):
//! pre-physics `b6f10`, post-physics `b7060` and finalize `b7150`, the
//! per-object updates `bc470` and `bc820` with the node matrices `bd090`,
//! the four type dispatchers, the wake leaves, the early-mover wake
//! `109580`, the leaves `bd090` calls (`b8bd0`, `bf5a0`, `bfc90`, `bff10`,
//! `bfe80`, `bfed0`, `bfe20`, `b6780`), and the type callbacks of the
//! objects a multiplayer game has: the unit update `c6b30` with its tree
//! (slices 6a, 6b and 6c), the biped update `dd360` and post-physics update
//! `dd990` with the ported part of their trees (slices 7a and 7b), the
//! weapon update `fdfb0` (slice 9), the item level `10b4d0` that weapons,
//! equipment and garbage run first, the vehicle update `efde0` with its two
//! leaves (slice 10), and the projectile update `f8de0` with its step
//! `f8eb0`, the scenery update `10a2f0`, the device update `106790` that
//! machines, controls and light fixtures run first, the machine update
//! `1152e0`, the crate update `11bce0` and the creature update `118700`
//! and post-physics update `118b80` (slice 11). [`TABLE`] names that
//! set, the walk table the engine's differential tests drive;
//! `sim_bridge::sim_objects` installs every body in [`BODIES`] (the
//! pose and after-root dispatchers, damage, functions, the post-update
//! shell and lights with these), and [`WITHOUT_AIM`]'s hand-off applies
//! there too: the animation subsystem owns the aim update `c7840`.
//!
//! The Rust body answers every segment; nothing is compared. A body
//! entered outside the float environment its arithmetic assumes is
//! counted as a [`Refusal`] and still answered. The walks, the updates,
//! the dispatchers and the wake leaves read and write no float state
//! ([`FLOAT_FREE`]), so they are never counted; `bd090` and the type
//! callbacks ([`TYPES`]) are.
//!
//! Every callee of a body that has no body of its own is a call to the
//! guest, made in place when the runtime can
//! (`runtime::ExecCtx::call_inline`, from `sim_bridge::sim_objects`),
//! else through the runner, counted either way ([`callout`]). The
//! counters ([`stats`]) cover every body in `super::TABLE`.

use super::{BODIES, Body, Table};
use crate::points::{self, Point};
use runtime::{CpuAbi, Fallback, Guest, TransferAbi};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// The walk bodies: the three walks, the per-object updates, `bd090`, the
/// type dispatchers, the wake leaves, the early-mover wake and the leaves
/// `bd090` calls (`getters`). [`TABLE`] holds these and [`TYPES`].
pub const POINTS: &[&Point] = &[
    &points::OBJECTS_PRE_PHYSICS,
    &points::OBJECTS_POST_PHYSICS,
    &points::OBJECTS_FINALIZE,
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
    &points::OBJECT_NODE_MATRIX,
    &points::OBJECT_HAS_ANIMATION,
    &points::OBJECT_ATTACHED,
    &points::NODE_MASK_EMPTY,
    &points::NODE_MASK_INVERT,
    &points::NODE_MASK_TRIM,
    &points::NODE_MASK_SET,
    &points::ANIMATION_VALID,
];

/// The type callbacks in [`TABLE`]: the `unit` descriptor's slot `+40`
/// (`c6b30`, slices 6a, 6b and 6c, which bipeds and vehicles inherit), the
/// `bipd` descriptor's slots `+40` (`dd360`, slice 7a) and `+44` (`dd990`,
/// slice 7b) with every body their trees fuse, the `weap` slot `+40`
/// (`fdfb0`, slice 9), the `item` slot `+40` (`10b4d0`, which `weap`,
/// `eqip` and `garb` inherit; `eqip` and `garb` have no slot of their own),
/// the `vehi` slot `+40` (`efde0`, slice 10) with its gate and contact
/// reset, the `proj` slot `+40` (`f8de0`) with its step, the `scen` slot
/// `+40` (`10a2f0`), the `devi` slot `+40` (`106790`, which `mach`, `ctrl`
/// and `lifi` inherit; `ctrl` and `lifi` have no slot of their own), the
/// `mach` slot `+40` (`1152e0`), the `bloc` slot `+40` (`11bce0`)
/// and the `crea` slots `+40` (`118700`) and `+44` (`118b80`, slice 11).
/// Each checks the float environment at its entry and its sites. The
/// opt-in creature probe (`game::actor_net::creature`) skips the
/// creature's two for a replica with a `before`; with it on they are
/// shared points, guarded, so the dispatchers' calls go through its
/// chain.
pub const TYPES: &[&Point] = &[
    // The unit level: `c6b30`, its nine children and theirs, and the
    // animation state `111650` the biped tree calls.
    &points::UNIT_UPDATE,
    &points::UNIT_UPDATE_CONTROL,
    &points::UNIT_UPDATE_STATE,
    &points::UNIT_IDLE_TEST,
    &points::UNIT_MOVEMENT,
    &points::UNIT_CLAMP_DIRECTION,
    &points::UNIT_AIM_UPDATE,
    &points::UNIT_POWERED_SEATS,
    &points::UNIT_CONTROL_REFRESH,
    &points::UNIT_TIMERS,
    &points::UNIT_DIALOGUE_UPDATE,
    &points::UNIT_TEAM_UPDATE,
    &points::UNIT_SEATS_BEGIN,
    &points::UNIT_SEATS_NEXT,
    &points::UNIT_CAMO_UPDATE,
    &points::UNIT_LIGHT_UPDATE,
    &points::UNIT_ANIMATION_STATE,
    // The biped and the weapon.
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
    // The item level of a weapon, equipment and garbage; the vehicle,
    // projectile and scenery levels; the device level of a device,
    // machine, control and light fixture, and the machine level; the
    // crate and the creature.
    &points::ITEM_UPDATE,
    &points::VEHICLE_UPDATE,
    &points::VEHICLE_UPDATE_GATE,
    &points::VEHICLE_CONTACTS_RESET,
    &points::PROJECTILE_UPDATE,
    &points::PROJECTILE_STEP,
    &points::SCENERY_UPDATE,
    &points::DEVICE_UPDATE,
    &points::MACHINE_UPDATE,
    &points::CRATE_UPDATE,
    &points::CREATURE_UPDATE,
    &points::CREATURE_POST_UPDATE,
    // The after-root slot `+70` of `unit` and `scen`.
    &points::UNIT_AFTER_ROOT,
    &points::SCENERY_AFTER_ROOT,
];

/// The entries whose bodies make no float instruction: every one in
/// [`POINTS`] but `bd090`.
pub const FLOAT_FREE: &[u32] = &[
    points::OBJECTS_PRE_PHYSICS.base,
    points::OBJECTS_POST_PHYSICS.base,
    points::OBJECTS_FINALIZE.base,
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
    points::OBJECT_NODE_MATRIX.base,
    points::OBJECT_HAS_ANIMATION.base,
    points::OBJECT_ATTACHED.base,
    points::NODE_MASK_EMPTY.base,
    points::NODE_MASK_INVERT.base,
    points::NODE_MASK_TRIM.base,
    points::NODE_MASK_SET.base,
    points::ANIMATION_VALID.base,
];

/// The entries whose hooks never transfer to their own guest body: the
/// three walks and the per-object updates (`HookRef::replaces_guest`).
/// Each has resume sites, so `hooks::Plan::replaced` keeps its body lifted.
pub const REPLACES_GUEST: &[u32] = &[
    points::OBJECTS_PRE_PHYSICS.base,
    points::OBJECTS_POST_PHYSICS.base,
    points::OBJECTS_FINALIZE.base,
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

/// [`TABLE`] without the aim update `c7840`: the animation subsystem
/// implements that point, so the movement
/// `c60c0` calls it as a callout and its hook answers
/// (`sim_bridge::sim_objects`, as for the full table). The counters stay
/// [`TABLE`]'s.
pub static WITHOUT_AIM: LazyLock<Table> = LazyLock::new(|| {
    Table::new(TABLE.bodies().iter().copied().filter(|b| b.point.base != points::UNIT_AIM_UPDATE.base))
        .expect("BODIES is consistent")
        .float_free(FLOAT_FREE)
});

/// A named error a body's call is counted under. The Rust still answers
/// the call; no refusal hands it to the guest.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Refusal {
    /// A body that uses float state entered or resumed outside the float
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
    /// Calls or resumes counted under a named error, by
    /// [`Refusal::ALL`] position; the Rust answered them.
    pub refused: [u64; Refusal::ALL.len()],
    /// True callouts from the body that the runtime made in place and that
    /// came back to their site in Rust (`runtime::ExecCtx::call_inline`).
    pub inline: u64,
    /// True callouts left to the runner, by [`Fallback::ALL`] position.
    pub left: [u64; Fallback::ALL.len()],
}

struct Counters {
    segments: Vec<AtomicU64>,
    refused: Vec<[AtomicU64; Refusal::ALL.len()]>,
    inline: Vec<AtomicU64>,
    left: Vec<[AtomicU64; Fallback::ALL.len()]>,
}

static COUNTERS: LazyLock<Counters> = LazyLock::new(|| {
    let n = super::TABLE.bodies().len();
    Counters {
        segments: (0..n).map(|_| AtomicU64::new(0)).collect(),
        refused: (0..n).map(|_| std::array::from_fn(|_| AtomicU64::new(0))).collect(),
        inline: (0..n).map(|_| AtomicU64::new(0)).collect(),
        left: (0..n).map(|_| std::array::from_fn(|_| AtomicU64::new(0))).collect(),
    }
});

/// One segment of a body, counted: the runner [`super::drive_with`]
/// takes.
pub fn segment(body: &Body, cpu: &mut CpuAbi, g: &mut Guest, pc: u32) -> super::Result<TransferAbi> {
    let transfer = body.segment(cpu, g, pc)?;
    if let Some(i) = super::TABLE.index(pc) {
        COUNTERS.segments[i].fetch_add(1, Ordering::Relaxed);
    }
    Ok(transfer)
}

/// The call or resume at `pc` is counted under `why`.
pub fn refused(pc: u32, why: Refusal) {
    if let Some(i) = super::TABLE.index(pc) {
        let at = Refusal::ALL.iter().position(|&r| r == why).expect("listed");
        COUNTERS.refused[i][at].fetch_add(1, Ordering::Relaxed);
    }
}

/// One true callout of the body that resumes at `site`: made in place and
/// back in Rust (`None`), or left to the runner for `why`.
pub fn callout(site: u32, left: Option<Fallback>) {
    if let Some(i) = super::TABLE.index(site) {
        match left {
            None => COUNTERS.inline[i].fetch_add(1, Ordering::Relaxed),
            Some(why) => COUNTERS.left[i][why.index()].fetch_add(1, Ordering::Relaxed),
        };
    }
}

/// Every body's counters, in `super::TABLE` order.
pub fn stats() -> Vec<Stats> {
    super::TABLE
        .bodies()
        .iter()
        .enumerate()
        .map(|(i, body)| Stats {
            entry: body.point.base,
            segments: COUNTERS.segments[i].load(Ordering::Relaxed),
            refused: std::array::from_fn(|r| COUNTERS.refused[i][r].load(Ordering::Relaxed)),
            inline: COUNTERS.inline[i].load(Ordering::Relaxed),
            left: std::array::from_fn(|r| COUNTERS.left[i][r].load(Ordering::Relaxed)),
        })
        .collect()
}

#[cfg(test)]
mod tests;
