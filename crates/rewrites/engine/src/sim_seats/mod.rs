//! Seats and rules, phase 7 of the simulation rewrite
//! (docs/engine/sim-rewrite.md): the observer camera, per-seat control,
//! look response and targeting, the HUD's per-seat update and the
//! observer resource predictions, in Rust behind `impl.sim.native.seats`.
//!
//! Each [`Seat`] is one guest function with the sites its children return
//! to. A body runs one straight-line segment per transfer: it buffers its
//! stores in a [`crate::sim_native::plan`] and declines — leaving the CPU
//! and RAM untouched for the generated original — wherever the shape is
//! one it does not take over: a float environment it does not assume, an
//! aliased or unmapped operand, or a path the original owns (the
//! controller's device bits before the first look-curve call, a
//! resource type an edited image can name). The bound users' device
//! reading and the phantom action builder therefore keep running in the
//! generated code, and the unbound path the phantoms take is the body's
//! entry.
//!
//! The bodies replace `sim.native_observer_camera` and
//! `sim.observer-resources`; those switches are gone and the observer
//! camera stays as the `oracle.observer_camera` test oracle
//! (`crate::sim_physics::ORACLES`).

use crate::points::{self, Point};
use runtime::{CpuAbi, Guest, GuestFault, TransferAbi};

pub mod aim;
pub mod camera;
pub mod control;
pub mod hud;
pub mod look;
pub mod resources;
pub mod targeting;

/// One replaced function: the point it is entered at, the sites its
/// children return to, and the body. The body declines by returning
/// `None`; the caller then runs the original.
pub struct Seat {
    pub point: &'static Point,
    /// The shadow registry's name, and the label the plan installs under.
    pub name: &'static str,
    /// Return sites, sorted: the entries the dispatcher can resume at.
    pub sites: &'static [u32],
    pub run: fn(&mut CpuAbi, &mut Guest, u32) -> Result<Option<TransferAbi>, GuestFault>,
}

type Step = Result<Option<TransferAbi>, GuestFault>;

/// The observer camera: same entry, same code, guarded by the aliased-span
/// and float-environment checks the original port had.
fn observer(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Step {
    if !camera::supported(c, g) {
        return Ok(None);
    }
    camera::run(c, g, pc).map(Some)
}

/// The shot aim and the cone builder write straight through (no store
/// buffering), so their guard is the float environment alone.
fn shot(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Step {
    if !crate::sim_native::supported(c) {
        return Ok(None);
    }
    aim::shot(c, g, pc).map(Some)
}

fn params(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Step {
    if !crate::sim_native::supported(c) {
        return Ok(None);
    }
    aim::params(c, g, pc).map(Some)
}

pub const SEATS: &[Seat] = &[
    Seat {
        point: &points::PLAYER_LOOK_CURVE,
        name: "sim.seats.player_look_curve",
        sites: &[],
        run: look::curve,
    },
    Seat {
        point: &points::PLAYER_CONTROL_UPDATE,
        name: "sim.seats.player_control_update",
        sites: look::SITES,
        run: look::update,
    },
    Seat {
        point: &points::PLAYER_ACTION_TO_UNIT_CONTROL,
        name: "sim.seats.player_action_control",
        sites: control::ACTION_SITES,
        run: control::action,
    },
    Seat {
        point: &points::PLAYER_NEUTRAL_UNIT_CONTROL,
        name: "sim.seats.player_neutral_control",
        sites: control::NEUTRAL_SITES,
        run: control::neutral,
    },
    Seat {
        point: &points::UNIT_APPLY_CONTROL,
        name: "sim.seats.unit_apply_control",
        sites: &[],
        run: control::apply,
    },
    Seat {
        point: &points::HUD_SEAT_UPDATE,
        name: "sim.seats.hud_seat_update",
        sites: hud::SITES,
        run: hud::run,
    },
    Seat {
        point: &points::AIM_TARGET_UPDATE,
        name: "sim.seats.aim_target_update",
        sites: targeting::UPDATE_SITES,
        run: targeting::update,
    },
    Seat {
        point: &points::AIM_TARGET_SEARCH,
        name: "sim.seats.aim_target_search",
        sites: targeting::SEARCH_SITES,
        run: targeting::search,
    },
    Seat {
        point: &points::AIM_TARGET_COMPARE,
        name: "sim.seats.aim_target_compare",
        sites: &[],
        run: targeting::compare,
    },
    Seat {
        point: &points::SEAT_SHOT_AIM,
        name: "sim.seats.shot_aim",
        sites: aim::SHOT_SITES,
        run: shot,
    },
    Seat {
        point: &points::AIM_ASSIST_PARAMS,
        name: "sim.seats.aim_assist_params",
        sites: aim::SITES,
        run: params,
    },
    Seat {
        point: &points::OBSERVER_RESULT,
        name: "sim.seats.observer_camera",
        sites: &[],
        run: observer,
    },
    Seat {
        point: &points::PREDICT_RESOURCES,
        name: "sim.seats.predict_resources",
        sites: resources::SITES,
        run: resources::run,
    },
];
