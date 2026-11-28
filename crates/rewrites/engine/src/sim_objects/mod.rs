//! The guest side of the object update, phase 3 of the simulation rewrite
//! (design: docs/engine/sim-rewrite-objects.md section 5; contracts:
//! docs/engine/object-update.md). Every body runs, always:
//! `sim_bridge::sim_objects` installs [`BODIES`] with no switch.
//!
//! A ported function is a [`Body`]: retail-site arms over the retail stack
//! frame. At every guest call it makes, `esp`, the frame slots it reads
//! later and all eight integer registers hold what retail holds, so no
//! Rust state survives the call and the lifted original can finish a frame
//! a body started, or the reverse.
//!
//! [`drive`] is the one local dispatcher. A call or return between two
//! bodies stays in Rust; only a transfer to code that is still guest goes
//! back to the runtime. [`BODIES`] lists every body; slice lanes append to
//! it, one row block each.
//!
//! A body whose point other features hook as well (a `before` or `after`,
//! an observer) is [`Table::guard`]ed: calls into it go through the
//! runtime, so its hooks run around it as around the original.

pub mod damage;
pub mod frame;
mod object;
// The after-root slot `+70` callees.
mod after_root;
mod wake;
mod walks;
pub mod functions;
// Slice 11: the small type updates.
pub(crate) mod alu;
mod bloc;
mod creature;
mod device;
mod item;
mod machine;
mod projectile;
// Slice 11, the projectile step `f8eb0` (x87 and `xmm` lane).
pub(crate) mod projectile_step;
mod scenery;
pub(crate) mod sse;
// Slice 4: the post-update shell.
mod bounds;
mod getters;
mod pose;
mod post;
mod transform;
mod xform;
// Slice 5: lights.
mod lights;
// Slice 6a: the unit update's shell and movement.
mod unit;
// Slice 9: the weapon update.
mod weapon;
// Slice 10: the vehicle update.
mod vehicle;
// Slice 6b: the unit update's state children.
mod unit_state;
// Slice 6c: the rest of the unit update.
mod unit_anim;
mod unit_rest;
// Slice 7a: the biped update; slice 7b its post-physics update
// (`biped::post`).
mod biped;
// The walks, the per-object updates and the multiplayer types' callbacks
// in Rust, always.
pub mod native_walk;

use crate::points::{self, Point};
use runtime::{CpuAbi, Guest, GuestFault, TransferAbi};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};

pub type Result<T> = std::result::Result<T, GuestFault>;

/// Where a body goes next. The body has already set every register and
/// stack slot retail has at that transfer, except the push of a call's
/// return address and the pop of a return, which [`drive`] makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    /// `call target`, returning to `site` (one of the body's
    /// [`Body::sites`]).
    Call { target: u32, site: u32 },
    /// Call with a return address the image resumes. A partially native
    /// function can hand its live retail frame back to the original at
    /// this boundary. Its installer requires the generated selector.
    CallOriginalResume { target: u32, site: u32 },
    /// `ret pop`.
    Ret { pop: u16 },
    /// `jmp target` out of the function (a tail call).
    Tail { target: u32 },
}

/// What a body runs on: the guest registers and guest memory, nothing
/// else. A body keeps no state of its own.
pub struct Cx<'a> {
    pub cpu: &'a mut CpuAbi,
    pub g: &'a mut Guest,
}

/// One ported function.
pub struct Body {
    /// Its entry.
    pub point: &'static Point,
    /// The retail return addresses of its guest calls, where `run` is
    /// entered again after a call.
    pub sites: &'static [u32],
    /// Run from `pc` (the entry or a site) to the next transfer.
    pub run: fn(&mut Cx<'_>, u32) -> Result<Next>,
}

/// Every body, appended to by each slice: one row block per slice
/// (docs/engine/sim-rewrite-objects.md section 12).
pub const BODIES: &[Body] = &[
    // Slice 1: the walks, the per-object updates, the type dispatchers,
    // the wake leaves and the early-mover wake.
    Body { point: &points::OBJECTS_PRE_PHYSICS, sites: &walks::PRE_SITES, run: walks::pre },
    Body { point: &points::OBJECTS_POST_PHYSICS, sites: &walks::POST_SITES, run: walks::post },
    Body { point: &points::OBJECTS_FINALIZE, sites: &walks::FINALIZE_SITES, run: walks::finalize },
    Body { point: &points::OBJECT_PRE_UPDATE, sites: &object::PRE, run: object::pre },
    Body { point: &points::OBJECT_POST_UPDATE, sites: &object::POST, run: object::post },
    Body { point: &points::OBJECT_TYPE_UPDATE, sites: &object::UPDATE_SITES, run: object::type_update },
    Body { point: &points::OBJECT_TYPE_POST_UPDATE, sites: &object::POST_UPDATE_SITES, run: object::type_post_update },
    Body { point: &points::OBJECT_TYPE_POSE, sites: &object::POSE_SITES, run: object::type_pose },
    Body { point: &points::OBJECT_TYPE_AFTER_ROOT, sites: &object::AFTER_ROOT_SITES, run: object::type_after_root },
    Body { point: &points::UNIT_AFTER_ROOT, sites: &after_root::UNIT_SITES, run: after_root::unit },
    Body { point: &points::SCENERY_AFTER_ROOT, sites: &after_root::SCENERY_SITES, run: after_root::scenery },
    Body { point: &points::OBJECT_WAKE, sites: &[], run: wake::wake },
    Body { point: &points::OBJECT_ROOT, sites: &[], run: wake::root },
    Body { point: &points::OBJECT_WAKE_FOR_POST, sites: &[], run: wake::wake_for_post },
    Body { point: &points::OBJECT_WAKE_CHILDREN, sites: &[], run: wake::wake_children },
    Body { point: &points::OBJECT_EARLY_MOVER_WAKE, sites: &wake::EARLY_MOVER_SITES, run: wake::early_mover_wake },
    // The leaves `bd090` calls: the node matrix address, the animation and
    // attachment tests and the node-mask operations (`getters`).
    Body { point: &points::OBJECT_NODE_MATRIX, sites: &[], run: getters::node_matrix },
    Body { point: &points::OBJECT_HAS_ANIMATION, sites: &[], run: getters::has_animation },
    Body { point: &points::OBJECT_ATTACHED, sites: &[], run: getters::attached },
    Body { point: &points::NODE_MASK_EMPTY, sites: &[], run: getters::mask_empty },
    Body { point: &points::NODE_MASK_INVERT, sites: &[], run: getters::mask_invert },
    Body { point: &points::NODE_MASK_TRIM, sites: &[], run: getters::mask_trim },
    Body { point: &points::NODE_MASK_SET, sites: &[], run: getters::mask_set },
    Body { point: &points::ANIMATION_VALID, sites: &[], run: getters::animation_valid },
    // Slice 3: function values, the imports and change colours
    // (`functions`).
    Body { point: &points::OBJECT_FUNCTION_VALUE, sites: &functions::value::VALUE_SITES, run: functions::value::value },
    Body { point: &points::OBJECT_FUNCTION_EVALUATE, sites: &functions::value::EVALUATE_SITES, run: functions::value::evaluate },
    Body { point: &points::OBJECT_FUNCTION_IMPORT, sites: &functions::value::IMPORT_SITES, run: functions::value::import },
    Body { point: &points::BASE_OBJECT_IMPORT, sites: &functions::base::BASE_SITES, run: functions::base::base },
    Body { point: &points::OBJECT_CHANGE_COLORS, sites: &functions::colors::COLORS_SITES, run: functions::colors::colors },
    Body { point: &points::UNIT_FUNCTION_IMPORT, sites: &functions::unit::UNIT_SITES, run: functions::unit::unit },
    Body { point: &points::UNIT_IMPORT_FRACTION, sites: &[], run: functions::unit::d1210 },
    Body { point: &points::UNIT_IMPORT_ANIMATION, sites: &functions::unit::D1410_SITES, run: functions::unit::d1410 },
    Body { point: &points::BIPED_FUNCTION_IMPORT, sites: &[], run: functions::biped::biped },
    Body { point: &points::VEHICLE_FUNCTION_IMPORT, sites: &functions::vehicle::VEHICLE_SITES, run: functions::vehicle::vehicle },
    Body { point: &points::WEAPON_FUNCTION_IMPORT, sites: &functions::weapon::WEAPON_SITES, run: functions::weapon::weapon },
    Body { point: &points::PROJECTILE_FUNCTION_IMPORT, sites: &[], run: functions::projectile::projectile },
    Body { point: &points::DEVICE_FUNCTION_IMPORT, sites: &[], run: functions::device::device },
    Body { point: &points::MACHINE_FUNCTION_IMPORT, sites: &functions::machine::MACHINE_SITES, run: functions::machine::machine },
    // Slice 8: the object damage update.
    Body { point: &points::OBJECT_DAMAGE_UPDATE, sites: &damage::SITES, run: damage::update },
    // Slice 11: the small type updates (the descriptors' slots `+40`,
    // `+44` and `+48` for item, scenery, device, machine, crate, creature
    // and projectile).
    Body { point: &points::ITEM_UPDATE, sites: &item::ITEM_SITES, run: item::item_update },
    Body { point: &points::SCENERY_UPDATE, sites: &scenery::SCENERY_SITES, run: scenery::scenery_update },
    Body { point: &points::DEVICE_UPDATE, sites: &device::DEVICE_SITES, run: device::device_update },
    Body { point: &points::MACHINE_UPDATE, sites: &machine::MACHINE_SITES, run: machine::machine_update },
    Body { point: &points::MACHINE_SLOT_48, sites: &[], run: machine::machine_slot_48 },
    Body { point: &points::PROJECTILE_UPDATE, sites: &projectile::PROJECTILE_SITES, run: projectile::projectile_update },
    Body { point: &points::CRATE_UPDATE, sites: &bloc::CRATE_SITES, run: bloc::crate_update },
    Body { point: &points::CREATURE_UPDATE, sites: &creature::CREATURE_SITES, run: creature::creature_update },
    Body { point: &points::CREATURE_POST_UPDATE, sites: &creature::CREATURE_POST_SITES, run: creature::creature_post_update },
    // Slice 11, the projectile step `f8eb0`: fused into `f8de0`'s call.
    Body { point: &points::PROJECTILE_STEP, sites: &projectile_step::STEP_SITES, run: projectile_step::projectile_step },
    // Slice 4: the post-update shell (`bc820`'s callees: the component
    // transform, the node matrices, the bounds, the attachment refresh
    // and the node-matrix notify) and the transform setters and getters.
    Body { point: &points::OBJECT_COMPONENT_TRANSFORM, sites: &post::COMPONENT_SITES, run: post::component_transform },
    Body { point: &points::OBJECT_NODE_MATRICES, sites: &pose::POSE_SITES, run: pose::node_matrices },
    Body { point: &points::OBJECT_BOUNDS, sites: &bounds::BOUNDS_SITES, run: bounds::bounds },
    Body { point: &points::OBJECT_ATTACHMENTS, sites: &post::ATTACH_SITES, run: post::attachments },
    Body { point: &points::OBJECT_NODES_NOTIFY, sites: &post::NOTIFY_SITES, run: post::nodes_notify },
    Body { point: &points::OBJECT_SET_TRANSFORM, sites: &transform::SET_TRANSFORM_SITES, run: transform::set_transform },
    Body { point: &points::OBJECT_SET_VELOCITY, sites: &transform::SET_VELOCITY_SITES, run: transform::set_velocity },
    Body { point: &points::OBJECT_WORLD_POSITION, sites: &[], run: transform::world_position },
    Body { point: &points::OBJECT_WORLD_BASIS, sites: &[], run: transform::world_basis },
    Body { point: &points::OBJECT_ROOT_VELOCITY, sites: &transform::VELOCITY_SITES, run: transform::root_velocity },
    // Slice 5: the light walk, the record update and its modulation step,
    // and the light-list operations (`lights`).
    Body { point: &points::LIGHTS_UPDATE, sites: &lights::walk::WALK_SITES, run: lights::walk::walk },
    Body { point: &points::LIGHT_RECORD_UPDATE, sites: &lights::record::RECORD_SITES, run: lights::record::record },
    Body { point: &points::LIGHT_RECORD_PHASE, sites: &lights::phase::PHASE_SITES, run: lights::phase::phase },
    Body { point: &points::LIGHT_LINK, sites: &lights::list::LINK_SITES, run: lights::list::link },
    Body { point: &points::LIGHT_UNLINK, sites: &lights::list::UNLINK_SITES, run: lights::list::unlink },
    Body { point: &points::LIGHT_EXPIRE, sites: &lights::list::EXPIRE_SITES, run: lights::list::expire },
    // Slice 6a: the unit update's shell (the `unit` descriptor's slot
    // `+40`), its movement, the direction clamp, the idle test and the aim
    // update (the port `sim_physics::unit_aim`, run as a body).
    Body { point: &points::UNIT_UPDATE, sites: &unit::SHELL_SITES, run: unit::shell },
    Body { point: &points::UNIT_MOVEMENT, sites: &unit::MOVEMENT_SITES, run: unit::movement },
    Body { point: &points::UNIT_CLAMP_DIRECTION, sites: &unit::CLAMP_SITES, run: unit::clamp },
    Body { point: &points::UNIT_IDLE_TEST, sites: &unit::IDLE_SITES, run: unit::idle },
    Body { point: &points::UNIT_AIM_UPDATE, sites: unit::AIM_SITES, run: unit::aim },
    // Slice 9: the weapon update (the `weap` descriptor's slot `+40`).
    Body { point: &points::WEAPON_UPDATE, sites: &weapon::WEAPON_SITES, run: weapon::weapon_update },
    // Slice 10: the vehicle update (the `vehi` descriptor's slot `+40`)
    // and the two leaves it calls.
    Body { point: &points::VEHICLE_UPDATE, sites: &vehicle::VEHICLE_SITES, run: vehicle::vehicle_update },
    Body { point: &points::VEHICLE_UPDATE_GATE, sites: &[], run: vehicle::vehicle_update_gate },
    Body { point: &points::VEHICLE_CONTACTS_RESET, sites: &[], run: vehicle::vehicle_contacts_reset },
    // Slice 6b: the unit update's control, state and powered seats (its
    // first, second and fourth calls) and the control refresh the seats
    // call (`unit_state`).
    Body { point: &points::UNIT_UPDATE_CONTROL, sites: &unit_state::control::CONTROL_SITES, run: unit_state::control::control },
    Body { point: &points::UNIT_UPDATE_STATE, sites: &unit_state::state::STATE_SITES, run: unit_state::state::state },
    Body { point: &points::UNIT_POWERED_SEATS, sites: &unit_state::seats::SEATS_SITES, run: unit_state::seats::seats },
    Body { point: &points::UNIT_CONTROL_REFRESH, sites: &unit_state::seats::REFRESH_SITES, run: unit_state::seats::refresh },
    // Slice 6c: the rest of the unit update (`c6b30`'s children after the
    // state and movement: the timers, the dialogue update, the team with
    // its seat iterator, active camouflage and the integrated light) and
    // the unit animation state `111650`.
    Body { point: &points::UNIT_TIMERS, sites: &unit_rest::TIMERS_SITES, run: unit_rest::timers },
    Body { point: &points::UNIT_DIALOGUE_UPDATE, sites: &unit_rest::DIALOGUE_SITES, run: unit_rest::dialogue },
    Body { point: &points::UNIT_TEAM_UPDATE, sites: &unit_rest::TEAM_SITES, run: unit_rest::team },
    Body { point: &points::UNIT_SEATS_BEGIN, sites: &[], run: unit_rest::seats_begin },
    Body { point: &points::UNIT_SEATS_NEXT, sites: &[], run: unit_rest::seats_next },
    Body { point: &points::UNIT_CAMO_UPDATE, sites: &unit_rest::CAMO_SITES, run: unit_rest::camo },
    Body { point: &points::UNIT_LIGHT_UPDATE, sites: &unit_rest::LIGHT_SITES, run: unit_rest::light },
    Body { point: &points::UNIT_ANIMATION_STATE, sites: &unit_anim::ANIM_SITES, run: unit_anim::anim },
    // Slice 7a: the biped update (the `bipd` descriptor's slot `+40`) and
    // the part of its tree that is ported (`biped`).
    Body { point: &points::BIPED_UPDATE, sites: &biped::UPDATE_SITES, run: biped::biped_update },
    Body { point: &points::OBJECT_CARRIER, sites: &[], run: biped::carrier },
    Body { point: &points::BIPED_TIMER_UPDATE, sites: &biped::TIMER_SITES, run: biped::timer_update },
    Body { point: &points::GAME_SECONDS_TO_TICKS, sites: &[], run: biped::seconds_to_ticks },
    Body { point: &points::UNIT_ACTIONS_UPDATE, sites: &biped::ACTIONS_SITES, run: biped::actions_update },
    Body { point: &points::BIPED_ACTION, sites: &biped::ACTION_SITES, run: biped::action },
    Body { point: &points::BIPED_ANIMATION_KIND, sites: &[], run: biped::animation_kind },
    Body { point: &points::BIPED_AIRBORNE_TEST, sites: &[], run: biped::airborne_test },
    Body { point: &points::BIPED_FALL_UPDATE, sites: &biped::FALL_SITES, run: biped::fall_update },
    Body { point: &points::BIPED_ANIMATION_STATE_UPDATE, sites: &biped::STATE_SITES, run: biped::animation_state },
    Body { point: &points::BIPED_LOOK_UPDATE, sites: &biped::LOOK_SITES, run: biped::look_update },
    // Slice 7b: the biped post-physics update (the `bipd` descriptor's slot
    // `+44`) and the part of its tree that is ported (`biped::post`).
    Body { point: &points::BIPED_POST_UPDATE, sites: &biped::POST_SITES, run: biped::post_update },
    Body { point: &points::BIPED_CROUCH_STEP, sites: &biped::CROUCH_STEP_SITES, run: biped::crouch_step },
    Body { point: &points::BIPED_CROUCH_HEIGHT, sites: &biped::CROUCH_HEIGHT_SITES, run: biped::crouch_height },
    Body { point: &points::BIPED_CROUCH_FORCED, sites: &[], run: biped::crouch_forced },
    Body { point: &points::BIPED_GROUND_PROBE, sites: &biped::PROBE_SITES, run: biped::ground_probe },
    Body { point: &points::BIPED_GROUND_FLAG, sites: &[], run: biped::ground_flag },
    Body { point: &points::BIPED_JUMP_UPDATE, sites: &biped::JUMP_SITES, run: biped::jump_update },
    Body { point: &points::BIPED_PHYSICS_INPUT, sites: &biped::INPUT_SITES, run: biped::physics_input },
    Body { point: &points::BIPED_LANDING_UPDATE, sites: &[], run: biped::landing_update },
    Body { point: &points::BIPED_CONTACT_UPDATE, sites: &biped::CONTACT_SITES, run: biped::contact_update },
];

/// The bodies [`drive`] runs and fuses, indexed by entry and site.
pub static TABLE: LazyLock<Table> =
    LazyLock::new(|| Table::new(BODIES.iter()).expect("BODIES is consistent"));

/// Whether an address starts a body or resumes one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Entry,
    Site,
}

/// Why a set of bodies cannot form a [`Table`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableError {
    /// Two bodies claim this address (as an entry or a site), or one body
    /// lists it twice.
    Duplicate(u32),
}

/// A sorted index over a set of bodies: binary search by address.
pub struct Table {
    bodies: Vec<&'static Body>,
    index: Vec<(u32, Role, u16)>,
    /// Per body: calls into it go through the runtime ([`Table::guard`]).
    guarded: Vec<AtomicBool>,
    /// Per body: its segments read and write no float state
    /// ([`Table::float_free`]).
    no_float: Vec<bool>,
    nearest_x87: bool,
}

impl Table {
    pub fn new(bodies: impl IntoIterator<Item = &'static Body>) -> std::result::Result<Self, TableError> {
        let bodies: Vec<&'static Body> = bodies.into_iter().collect();
        let mut index = Vec::new();
        for (i, body) in bodies.iter().enumerate() {
            let i = u16::try_from(i).expect("fewer than 65536 bodies");
            index.push((body.point.base, Role::Entry, i));
            index.extend(body.sites.iter().map(|&site| (site, Role::Site, i)));
        }
        index.sort_by_key(|e| e.0);
        if let Some(w) = index.windows(2).find(|w| w[0].0 == w[1].0) {
            return Err(TableError::Duplicate(w[0].0));
        }
        let guarded = bodies.iter().map(|_| AtomicBool::new(false)).collect();
        let no_float = vec![false; bodies.len()];
        Ok(Table { bodies, index, guarded, no_float, nearest_x87: false })
    }

    /// The bodies starting at `entries` read and write no MXCSR, x87 or
    /// XMM state, so they run whatever the float environment
    /// ([`Table::supports_at`]). An entry with no body here is ignored.
    pub fn float_free(mut self, entries: &[u32]) -> Self {
        for &entry in entries {
            if let Some((Role::Entry, body)) = self.find(entry) {
                self.no_float[usize::from(body)] = true;
            }
        }
        self
    }

    /// The x87 gain and filter curves require round-to-nearest at every entry,
    /// including a return from a guest callback that changed the control word.
    pub fn require_nearest_x87(mut self) -> Self {
        self.nearest_x87 = true;
        self
    }

    pub fn supports(&self, cpu: &CpuAbi) -> bool {
        supported(cpu) && (!self.nearest_x87 || cpu.x87.control & 0xc00 == 0)
    }

    /// Whether the body that `pc` enters or resumes may run on `cpu`: a
    /// [`Table::float_free`] body always may, any other when
    /// [`Table::supports`] holds.
    pub fn supports_at(&self, pc: u32, cpu: &CpuAbi) -> bool {
        matches!(self.find(pc), Some((_, body)) if self.no_float[usize::from(body)]) || self.supports(cpu)
    }

    /// The position in [`Table::bodies`] of the body `pc` enters or
    /// resumes.
    pub fn index(&self, pc: u32) -> Option<usize> {
        self.find(pc).map(|(_, body)| usize::from(body))
    }

    pub fn bodies(&self) -> &[&'static Body] {
        &self.bodies
    }

    pub fn is_empty(&self) -> bool {
        self.bodies.is_empty()
    }

    /// The body `pc` enters or resumes, and which.
    pub fn at(&self, pc: u32) -> Option<(&'static Body, Role)> {
        let (role, body) = self.find(pc)?;
        Some((self.bodies[usize::from(body)], role))
    }

    fn find(&self, pc: u32) -> Option<(Role, u16)> {
        let i = self.index.binary_search_by_key(&pc, |e| e.0).ok()?;
        let (_, role, body) = self.index[i];
        Some((role, body))
    }

    /// Send calls into the body at `entry` through the runtime from now
    /// on: another feature hooks its point too (the Halo 1 health model's
    /// `before` and `after` on the damage update), and a call [`drive`]
    /// fused would skip those hooks. The body still runs, entered by the
    /// point's chain. False when no body starts at `entry`. The installer
    /// guards every such point when the plan is installed; a guard is
    /// never lifted, which costs a dispatch, never a hook.
    pub fn guard(&self, entry: u32) -> bool {
        match self.find(entry) {
            Some((Role::Entry, body)) => {
                self.guarded[usize::from(body)].store(true, Ordering::Relaxed);
                true
            }
            _ => false,
        }
    }

    /// Whether the body at `entry` is [`Table::guard`]ed.
    pub fn guarded(&self, entry: u32) -> bool {
        matches!(self.find(entry), Some((Role::Entry, body)) if self.guarded[usize::from(body)].load(Ordering::Relaxed))
    }

    /// Whether [`drive`] runs a call or tail jump to `target` itself: a
    /// body starts there and is not guarded.
    fn fuses(&self, target: u32) -> bool {
        matches!(self.find(target), Some((Role::Entry, body)) if !self.guarded[usize::from(body)].load(Ordering::Relaxed))
    }

    /// Whether a body starts at `addr`: the installer gives such a point
    /// to this switch and skips the per-function port there.
    pub fn owns(&self, addr: u32) -> bool {
        matches!(self.at(addr), Some((_, Role::Entry)))
    }

    /// Whether `addr` is a return site of a body: the installer registers
    /// the body to resume there.
    pub fn resumes(&self, addr: u32) -> bool {
        matches!(self.at(addr), Some((_, Role::Site)))
    }
}

/// The float environment every body assumes: MXCSR rounds to nearest,
/// masks every exception, and neither flushes to zero nor treats
/// denormals as zero; the x87 masks every exception, has no pending
/// error and does not use the reserved precision. When it does not hold,
/// the object installer counts the call (`native_walk::Refusal`) and the
/// Rust still runs it; [`drive`] does not fuse into the next body, except
/// a [`Table::float_free`] one, so that call is counted at its own hook.
pub fn supported(cpu: &CpuAbi) -> bool {
    crate::sim_native::supported(cpu) && cpu.mxcsr & MXCSR_MODES == 0
}

/// MXCSR rounding control (bits 13 and 14), flush to zero (15) and
/// denormals are zero (6).
const MXCSR_MODES: u32 = 0xe040;

/// How many blocks a body's segment loop, or transfers [`drive`]'s loop,
/// may run without leaving to the guest before a test build gives up.
/// Far above any real segment: the longest makes a few hundred.
const PROGRESS_CAP: u32 = 1 << 20;

/// One more block (or fused transfer) at `pc` since the body at `entry`
/// last left. In the unit tests and the differential harness, a loop
/// that never reaches a transfer (a `next` that maps back to itself, a
/// misread jump) fails here with the pc instead of hanging; elsewhere
/// this is nothing.
#[inline(always)]
fn progress(n: &mut u32, entry: u32, pc: u32) {
    if cfg!(any(test, feature = "differential")) {
        *n += 1;
        assert!(*n < PROGRESS_CAP, "body {entry:#x}: no transfer to the guest in {PROGRESS_CAP} steps, at {pc:#x}");
    }
}

fn unowned(pc: u32) -> GuestFault {
    GuestFault {
        addr: pc,
        len: 1,
        access: runtime::guest::Access::Execute,
        reason: runtime::guest::FaultReason::Unmapped,
    }
}

/// Run the body at `pc` (an entry or a site in `table`) and every body it
/// reaches by call, return or tail jump, until a transfer leaves the
/// table: a call to a function with no body (a true callout) or to a
/// guarded one, a return to a caller that is not a body site, or a tail
/// jump to guest code. That
/// transfer is returned for the runtime to take. The guest stack holds
/// every frame, so the runtime can stop at any callout and resume at the
/// site, which the installer registers.
///
/// No yield point is added: the scheduler gets control at true callouts,
/// as it does inside a lifted function.
pub fn drive(cpu: &mut CpuAbi, g: &mut Guest, table: &Table, pc: u32) -> Result<TransferAbi> {
    drive_with(cpu, g, table, pc, Body::segment)
}

impl Body {
    /// One segment, including the guest stack change made by its transfer.
    /// The caller decides whether to fuse into the target or visit the runtime.
    pub fn segment(&self, cpu: &mut CpuAbi, g: &mut Guest, pc: u32) -> Result<TransferAbi> {
        let next = (self.run)(&mut Cx { cpu, g }, pc)?;
        match next {
            Next::Call { target, site } => {
                debug_assert!(
                    self.sites.contains(&site),
                    "{} calls {target:#x} from an unlisted site {site:#x}",
                    self.point.name
                );
                frame::push(cpu, g, site)?;
                Ok(TransferAbi::call(target, site))
            }
            Next::CallOriginalResume { target, site } => {
                frame::push(cpu, g, site)?;
                Ok(TransferAbi::call(target, site))
            }
            Next::Ret { pop } => {
                let target = frame::pop(cpu, g)?;
                cpu.esp = cpu.esp.wrapping_add(u32::from(pop));
                Ok(TransferAbi::ret(target))
            }
            Next::Tail { target } => Ok(TransferAbi::jump(target)),
        }
    }
}

/// As [`drive`], with a caller-supplied runner for each individual segment,
/// including internal calls and resumes. Shadow checks use this boundary to
/// compare one segment before running its callee.
pub fn drive_with(
    cpu: &mut CpuAbi,
    g: &mut Guest,
    table: &Table,
    pc: u32,
    run_segment: impl FnMut(&Body, &mut CpuAbi, &mut Guest, u32) -> Result<TransferAbi>,
) -> Result<TransferAbi> {
    drive_admitting(cpu, g, table, pc, |_| true, run_segment)
}

/// As [`drive_with`], fusing a transfer the table would fuse only when
/// `admit(target)` says so, asked last; otherwise the transfer goes to the
/// runtime. The per-effect bodies' arm admits a segment whose turn is an
/// answer (`crate::effect_update::arm::admit`).
pub fn drive_admitting(
    cpu: &mut CpuAbi,
    g: &mut Guest,
    table: &Table,
    mut pc: u32,
    mut admit: impl FnMut(u32) -> bool,
    mut run_segment: impl FnMut(&Body, &mut CpuAbi, &mut Guest, u32) -> Result<TransferAbi>,
) -> Result<TransferAbi> {
    let (first, mut fused) = (pc, 0);
    loop {
        progress(&mut fused, first, pc);
        let (body, _) = table.at(pc).ok_or_else(|| unowned(pc))?;
        let transfer = run_segment(body, cpu, g, pc)?;
        let fuse = match transfer.kind {
            runtime::abi::CALL | runtime::abi::JUMP => table.fuses(transfer.target_pc),
            runtime::abi::RETURN => table.resumes(transfer.target_pc),
            _ => false,
        };
        if !fuse || !table.supports_at(transfer.target_pc, cpu) || !admit(transfer.target_pc) {
            return Ok(transfer);
        }
        pc = transfer.target_pc;
    }
}

#[cfg(test)]
mod tests;
