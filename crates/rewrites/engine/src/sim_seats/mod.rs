//! Seats and rules, phase 7 of the simulation rewrite
//! (docs/engine/sim-rewrite.md): the observer camera, per-seat control,
//! look response and targeting, the HUD's per-seat update and the
//! observer resource predictions, in Rust. Always installed, with no
//! switch (`sim_bridge::sim_seats`): a decline in the live game is
//! `unimplemented!("seats: <seat>: ...")`.
//!
//! Each [`Seat`] is one guest function with the sites its children return
//! to. A body runs one straight-line segment per transfer: it buffers its
//! stores in a [`crate::sim_native::plan`] and declines, leaving the CPU
//! and RAM untouched, wherever the shape is one it does not take: a float
//! environment it does not assume, an aliased or unmapped operand, or a
//! resource type an edited image can name. The live game's [`SEATS`]
//! leave the action builder `186ab0` to the generated code; the phantom
//! executor runs it in Rust ([`BUILDER`], with [`INTERACTION`] and the
//! seats under them: [`phantom_body`]). The bound controller's device
//! read is the kernel [`device`], laid down inside the control update's
//! body by [`device::arm`].
//!
//! The aim-assist cone `1a50a0` is not a seat: the idiomatic kernel
//! `sim_objects::aim_assist` answers it through [`aim::arm`], and its
//! register-level port stays as the differential oracle
//! [`PARAMS_ORACLE`].
//!
//! The observer camera stays as the `oracle.observer_camera` test oracle
//! (`crate::sim_physics::ORACLES`).

use crate::points::{self, Point};
use runtime::{CpuAbi, Guest, GuestFault, TransferAbi};

pub mod aim;
pub mod announce;
pub mod builder;
pub mod camera;
pub mod control;
pub mod device;
pub mod hud;
pub mod input;
pub mod interaction;
pub mod leaf;
pub mod look;
pub mod player_objects;
pub mod resources;
pub mod respawn;
pub mod targeting;

/// One replaced function: the point it is entered at, the sites its
/// children return to, and the body. The body declines by returning
/// `None`: the live game's seats panic on it by name, and the phantom
/// executor runs the original.
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

/// The aim-assist cone's register-level port: the differential oracle of
/// `1a50a0`. The point itself belongs to the kernel [`aim::arm`]
/// (`sim_bridge::present`), so it is not one of the [`SEATS`].
pub const PARAMS_ORACLE: Seat = Seat {
    point: &points::AIM_ASSIST_PARAMS,
    name: "sim.seats.aim_assist_params",
    sites: aim::SITES,
    run: params,
};

/// The local action builder `0x186ab0`: not one of the [`SEATS`], which
/// the live game installs, but the phantom executor's
/// (docs/perf/sim-phantom-build.md, "The builder body in Rust").
pub const BUILDER: Seat = Seat {
    point: &points::PLAYER_ACTION_BUILD,
    name: "sim.seats.phantom_action_build",
    sites: builder::SITES,
    run: builder::run,
};

/// The interaction search `0x151110`, under the builder.
pub const INTERACTION: Seat = Seat {
    point: &points::PLAYER_INTERACTION_SEARCH,
    name: "sim.seats.phantom_interaction_search",
    sites: interaction::SITES,
    run: interaction::run,
};

/// The seats under the builder the phantom executor also runs in Rust:
/// the control update over [`look`] and the target update and search
/// under it. The aim-assist cone `0x1a50a0` is not one: its kernel arm
/// and the `aim-assist` feature keep per-player host state.
const UNDER_BUILDER: [u32; 5] = [
    points::PLAYER_LOOK_CURVE.base,
    points::PLAYER_CONTROL_UPDATE.base,
    points::AIM_TARGET_UPDATE.base,
    points::AIM_TARGET_SEARCH.base,
    points::AIM_TARGET_COMPARE.base,
];

/// Every seat the phantom executor installs: the builder, the interaction
/// search, and [`UNDER_BUILDER`]'s.
pub fn phantom_body() -> impl Iterator<Item = &'static Seat> {
    [&BUILDER, &INTERACTION]
        .into_iter()
        .chain(SEATS.iter().filter(|s| UNDER_BUILDER.contains(&s.point.base)))
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
        point: &points::OBSERVER_RESULT,
        name: "sim.seats.observer_camera",
        sites: &[],
        run: observer,
    },
    Seat {
        point: &points::GAME_EVENT_ANNOUNCE,
        name: "sim.seats.event_announce",
        sites: announce::SITES,
        run: announce::run,
    },
    Seat {
        point: &points::PLAYER_PICKUP_SCAN,
        name: "sim.seats.player_pickup_scan",
        sites: player_objects::SITES,
        run: player_objects::run,
    },
    Seat {
        point: &points::PREDICT_RESOURCES,
        name: "sim.seats.predict_resources",
        sites: resources::SITES,
        run: resources::run,
    },
];
