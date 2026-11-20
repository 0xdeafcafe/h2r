//! The guest side of the object update, phase 3 of the simulation rewrite
//! (design: docs/engine/sim-rewrite-objects.md section 5; contracts:
//! docs/engine/object-update.md). Everything here runs under
//! `impl.sim.native.objects` (default off), installed by
//! `sim_bridge::sim_objects`.
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
mod wake;
mod walks;
mod functions;
// Slice 11: the small type updates.
mod alu;
mod bloc;
mod creature;
mod device;
mod item;
mod machine;
mod projectile;
// Slice 11, the projectile step `f8eb0` (x87 and `xmm` lane).
mod projectile_step;
mod scenery;
mod sse;
// Slice 4: the post-update shell.
mod bounds;
mod pose;
mod post;
mod transform;
mod xform;

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
    Body { point: &points::OBJECT_WAKE, sites: &[], run: wake::wake },
    Body { point: &points::OBJECT_ROOT, sites: &[], run: wake::root },
    Body { point: &points::OBJECT_WAKE_FOR_POST, sites: &[], run: wake::wake_for_post },
    Body { point: &points::OBJECT_WAKE_CHILDREN, sites: &[], run: wake::wake_children },
    Body { point: &points::OBJECT_EARLY_MOVER_WAKE, sites: &wake::EARLY_MOVER_SITES, run: wake::early_mover_wake },
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
        Ok(Table { bodies, index, guarded })
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

    fn resumes(&self, addr: u32) -> bool {
        matches!(self.at(addr), Some((_, Role::Site)))
    }
}

/// The float environment every body assumes: MXCSR rounds to nearest,
/// masks every exception, and neither flushes to zero nor treats
/// denormals as zero; the x87 masks every exception, has no pending
/// error and does not use the reserved precision. When it does not hold, the original runs: the installer checks
/// it before the first body, [`drive`] before fusing into the next.
pub fn supported(cpu: &CpuAbi) -> bool {
    crate::sim_native::supported(cpu) && cpu.mxcsr & MXCSR_MODES == 0
}

/// MXCSR rounding control (bits 13 and 14), flush to zero (15) and
/// denormals are zero (6).
const MXCSR_MODES: u32 = 0xe040;

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
pub fn drive(cpu: &mut CpuAbi, g: &mut Guest, table: &Table, mut pc: u32) -> Result<TransferAbi> {
    loop {
        let (body, _) = table.at(pc).ok_or_else(|| unowned(pc))?;
        let next = (body.run)(&mut Cx { cpu: &mut *cpu, g: &mut *g }, pc)?;
        match next {
            Next::Call { target, site } => {
                debug_assert!(
                    body.sites.contains(&site),
                    "{} calls {target:#x} from an unlisted site {site:#x}",
                    body.point.name
                );
                frame::push(cpu, g, site)?;
                if table.fuses(target) && supported(cpu) {
                    pc = target;
                    continue;
                }
                return Ok(TransferAbi::call(target, site));
            }
            Next::Ret { pop } => {
                let target = frame::pop(cpu, g)?;
                cpu.esp = cpu.esp.wrapping_add(u32::from(pop));
                if table.resumes(target) && supported(cpu) {
                    pc = target;
                    continue;
                }
                return Ok(TransferAbi::ret(target));
            }
            Next::Tail { target } => {
                if table.fuses(target) && supported(cpu) {
                    pc = target;
                    continue;
                }
                return Ok(TransferAbi::jump(target));
            }
        }
    }
}

#[cfg(test)]
mod tests;
