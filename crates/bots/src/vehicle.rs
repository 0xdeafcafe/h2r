//! Vehicles (docs/game/bots.md, section 3.9): what a bot knows about the
//! vehicles in the world ([`Sight`], read by [`read::sight`]), and the
//! pure planners that use it:
//!
//! - [`lock`]: holding a rocket launcher's reticle on a vehicle until it
//!   locks, then firing;
//! - [`plan`]: on foot, which free vehicle to go and take, and which enemy
//!   vehicle to board, and where to stand to get in;
//! - [`drive`]: in one, following a route, steering by the camera as a
//!   player does, getting unstuck, and when to get out.
//!
//! The mind glues them in (`mind::ride`). Everything here is plain data
//! and pure functions; only [`read`] touches guest memory.
//!
//! The vehicles' perception is kept apart from [`crate::brain::Perception`]
//! (the mind is handed it with `Mind::see_vehicles`), so a bot built
//! without it, and every test that builds a perception by hand, plays as
//! before: no vehicles.

pub mod drive;
pub mod flight;
pub mod lock;
pub mod plan;
pub mod read;

use crate::brain::Vec3;
use crate::mind::Skill;

/// A vehicle's kind: the `vehi` tag's type (`+0x1f0`, section 3.9).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Class {
    /// Scorpion.
    Tank,
    /// Warthog.
    Jeep,
    Boat,
    Plane,
    /// Ghost, Spectre, Wraith.
    Scout,
    /// Banshee.
    Fighter,
    /// A gun on its own, or mounted on another vehicle (the Warthog's
    /// chaingun, the Scorpion's).
    Turret,
    #[default]
    Unknown,
}

impl Class {
    /// From the `vehi` type field's value, in the tag definition's order.
    pub fn from_type(t: u16) -> Class {
        match t {
            0 => Class::Tank,
            1 => Class::Jeep,
            2 => Class::Boat,
            3 => Class::Plane,
            4 => Class::Scout,
            5 => Class::Fighter,
            6 => Class::Turret,
            _ => Class::Unknown,
        }
    }

    /// How bots use this kind of vehicle.
    pub fn spec(self) -> Spec {
        let (drive, driver_guns, splatter, half_width, height) = match self {
            Class::Jeep => (true, false, true, 0.85, 1.4),
            Class::Scout => (true, true, true, 0.85, 0.85),
            Class::Tank => (true, true, true, 1.25, 1.4),
            Class::Fighter => (true, true, false, 1.0, 1.2),
            _ => (false, false, false, 0.0, 0.0),
        };
        Spec {
            drive,
            driver_guns,
            splatter,
            half_width,
            height,
        }
    }
}

/// How bots use a kind of vehicle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spec {
    pub drive: bool,
    pub driver_guns: bool,
    pub splatter: bool,
    /// Conservative vehicle clearance envelope in world units.
    pub half_width: f32,
    pub height: f32,
}

/// What a seat is for: the seat's flags in the `vehi` tag's seats block
/// (section 3.9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeatRole {
    Driver,
    Gunner,
    Passenger,
}

/// A player in a seat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rider {
    /// The player's slot in the players array (`brain::Seen::id`).
    pub player: u32,
    pub team: i16,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Seat {
    pub role: SeatRole,
    pub rider: Option<Rider>,
    pub entries: Vec<Entry>,
    pub boards: Option<u16>,
}

/// The engine's resolved world-space seat marker and its admission limits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Entry {
    pub at: Vec3,
    pub forward: Vec3,
    pub radius: f32,
    pub facing: f32,
    pub look: f32,
    pub max_distance: f32,
}

/// One vehicle object. A mounted turret (the Warthog's gun) is one of its
/// own, [`Vehicle::on`] the vehicle it is mounted on.
#[derive(Clone, Debug, PartialEq)]
pub struct Vehicle {
    /// The object datum (raw handle).
    pub id: u32,
    pub class: Class,
    /// World position and axes (a mounted turret's are made world ones
    /// from its mount's).
    pub position: Vec3,
    pub forward: Vec3,
    pub up: Vec3,
    /// Body vitality, 0 to 1.
    pub health: f32,
    pub alternate_fire: bool,
    /// Model half-width and height, in world units.
    pub hull: Option<[f32; 2]>,
    /// The vehicle this one is mounted on.
    pub on: Option<u32>,
    pub seats: Vec<Seat>,
}

impl Vehicle {
    pub fn spec(&self) -> Spec {
        let mut spec = self.class.spec();
        if let Some([width, height]) = self.hull {
            spec.half_width = width + 0.12;
            spec.height = height + 0.12;
        }
        spec
    }

    /// Whether it lies the right way up (its up axis within about 60
    /// degrees of the world's).
    pub fn upright(&self) -> bool {
        self.up[2] > 0.5
    }
    /// A free seat of this kind, by index.
    pub fn free(&self, role: SeatRole) -> Option<usize> {
        self.seats
            .iter()
            .position(|s| s.role == role && s.rider.is_none())
    }
    pub fn enterable(&self, role: SeatRole) -> bool {
        self.seats.iter().any(|s| {
            s.role == role && s.rider.is_none() && s.boards.is_none() && !s.entries.is_empty()
        })
    }

    pub fn has(&self, role: SeatRole) -> bool {
        self.seats.iter().any(|s| s.role == role)
    }
    /// A point in its frame (forward, left), in the world, on its level.
    pub fn local(&self, at: [f32; 2]) -> Vec3 {
        let f = flat(self.forward);
        let left = [-f[1], f[0]];
        [
            self.position[0] + f[0] * at[0] + left[0] * at[1],
            self.position[1] + f[1] * at[0] + left[1] * at[1],
            self.position[2],
        ]
    }
}

/// `v`'s direction in the XY plane, unit length (+X when it has none).
pub fn flat(v: Vec3) -> [f32; 2] {
    let l = (v[0] * v[0] + v[1] * v[1]).sqrt();
    if l < 1e-6 {
        [1.0, 0.0]
    } else {
        [v[0] / l, v[1] / l]
    }
}

/// The bot's own seat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Riding {
    /// The vehicle object it sits in (a turret, for a Warthog's gunner).
    pub vehicle: u32,
    pub seat: u16,
    pub role: SeatRole,
}

/// What a bot knows about vehicles this tick.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sight {
    /// The bot's seat, while it rides.
    pub riding: Option<Riding>,
    /// Every vehicle in the world that is not wrecked.
    pub vehicles: Vec<Vehicle>,
}

impl Sight {
    pub fn get(&self, id: u32) -> Option<&Vehicle> {
        self.vehicles.iter().find(|v| v.id == id)
    }

    /// The vehicle `v` is part of: itself, or for a mounted turret the
    /// vehicle under it.
    pub fn root<'a>(&'a self, v: &'a Vehicle) -> &'a Vehicle {
        let mut at = v;
        for _ in 0..4 {
            match at.on.and_then(|id| self.get(id)) {
                Some(next) => at = next,
                None => break,
            }
        }
        at
    }

    /// The vehicle itself and the turrets mounted on it.
    pub fn parts(&self, root: u32) -> impl Iterator<Item = &Vehicle> {
        self.vehicles
            .iter()
            .filter(move |v| v.id == root || self.root(v).id == root)
    }

    /// Everyone aboard the vehicle `root` (its turrets included), with
    /// their seats' roles.
    pub fn crew(&self, root: u32) -> impl Iterator<Item = (Rider, SeatRole)> + '_ {
        self.parts(root)
            .flat_map(|v| v.seats.iter())
            .filter_map(|s| s.rider.map(|r| (r, s.role)))
    }

    /// The vehicle player `player` is in (the whole vehicle, not the
    /// turret) and their seat's role.
    pub fn ridden_by(&self, player: u32) -> Option<(&Vehicle, SeatRole)> {
        self.vehicles.iter().find_map(|v| {
            v.seats
                .iter()
                .find(|s| s.rider.is_some_and(|r| r.player == player))
                .map(|s| (self.root(v), s.role))
        })
    }

    /// The vehicle the bot is in, the whole of it.
    pub fn mine(&self) -> Option<&Vehicle> {
        let r = self.riding?;
        self.get(r.vehicle).map(|v| self.root(v))
    }
}

/// What skill changes about vehicles: data, per skill.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wheels {
    /// Takes a free vehicle this far away (world units, XY), 0 never.
    pub take_range: f32,
    /// Gets in a teammate's vehicle as its gunner.
    pub gunner: bool,
    /// Boards an enemy vehicle this close (XY)...
    pub board_range: f32,
    /// ...going no faster than this (world units a second); 0 never.
    pub board_speed: f32,
    /// Drives at infantry to run them over.
    pub splatter: bool,
    /// The rocket launcher's lock (section 3.9, "Lock-on").
    pub lock: lock::LockSkill,
}

impl Wheels {
    pub fn of(skill: Skill) -> Wheels {
        match skill {
            Skill::Easy => Wheels {
                take_range: 12.0,
                gunner: false,
                board_range: 0.0,
                board_speed: 0.0,
                splatter: false,
                lock: lock::LockSkill::of(skill),
            },
            Skill::Normal => Wheels {
                take_range: 20.0,
                gunner: true,
                board_range: 1.5,
                board_speed: 1.5,
                splatter: true,
                lock: lock::LockSkill::of(skill),
            },
            Skill::Heroic => Wheels {
                take_range: 28.0,
                gunner: true,
                board_range: 2.2,
                board_speed: 3.0,
                splatter: true,
                lock: lock::LockSkill::of(skill),
            },
            Skill::Legendary => Wheels {
                take_range: 35.0,
                gunner: true,
                board_range: 2.8,
                board_speed: 4.5,
                splatter: true,
                lock: lock::LockSkill::of(skill),
            },
        }
    }
}

/// The vehicle it is going for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Going {
    pub pick: plan::Pick,
    /// The tick it set out.
    pub since: u32,
    /// Which side it is trying ([`plan::approach`]).
    pub side: u32,
    /// The tick it started holding X at this side.
    pub holding: Option<u32>,
}

/// What the mind keeps about vehicles from tick to tick.
#[derive(Clone, Debug, Default)]
pub struct Memory {
    /// This tick's sight (`Mind::see_vehicles`).
    pub sight: Sight,
    /// Each vehicle's last position, the tick, and its speed (world units
    /// a second) from the one before.
    tracks: Vec<(u32, Vec3, u32, f32)>,
    /// Vehicles left alone until a tick: given up on, or just left.
    shunned: Vec<(u32, u32)>,
    pub going: Option<Going>,
    pub lock: lock::Lock,
    pub route: Option<drive::Route>,
    pub unstick: drive::Unstick,
    pub flight: flight::Flight,
    /// Getting out, and why.
    pub leaving: Option<drive::Leave>,
    /// Ticks its vehicle has lain flipped; ticks nobody has driven it.
    pub flipped: u32,
    pub driverless: u32,
    /// The vehicle it rode last tick (the whole one).
    was_in: Option<u32>,
    last_tick: Option<u32>,
}

impl Memory {
    /// Once a tick: the vehicles' speeds, how long its own has been
    /// flipped or driverless, and getting in or out.
    pub fn update(&mut self, tick: u32) {
        let dt = self.last_tick.map_or(1, |t| tick.wrapping_sub(t).max(1));
        self.last_tick = Some(tick);
        let mut tracks = Vec::with_capacity(self.sight.vehicles.len());
        for v in &self.sight.vehicles {
            let speed = match self.tracks.iter().find(|t| t.0 == v.id) {
                Some(&(_, at, t, _)) if tick > t => {
                    let d = ((v.position[0] - at[0]).powi(2)
                        + (v.position[1] - at[1]).powi(2)
                        + (v.position[2] - at[2]).powi(2))
                    .sqrt();
                    d * crate::mind::TICKS / (tick - t) as f32
                }
                Some(&(_, _, _, s)) => s,
                None => 0.0,
            };
            tracks.push((v.id, v.position, tick, speed));
        }
        self.tracks = tracks;
        self.shunned.retain(|&(_, until)| tick < until);
        let mine = self.sight.mine().map(|v| (v.id, v.upright()));
        let driven = mine.is_some_and(|(id, _)| {
            self.sight
                .crew(id)
                .any(|(_, role)| role == SeatRole::Driver)
        });
        match mine {
            Some((id, upright)) => {
                if self.was_in != Some(id) {
                    // In: whatever it was going for is done.
                    self.going = None;
                    self.route = None;
                    self.unstick.reset();
                    self.flight = flight::Flight::default();
                    self.leaving = None;
                    self.flipped = 0;
                    self.driverless = 0;
                }
                self.flipped = if upright { 0 } else { self.flipped + dt };
                self.driverless = if driven { 0 } else { self.driverless + dt };
            }
            None => {
                if let Some(left) = self.was_in {
                    // Out: that one is left alone a while, so it does not
                    // get straight back in.
                    self.shun(left, tick);
                    self.leaving = None;
                    self.route = None;
                }
            }
        }
        self.was_in = mine.map(|(id, _)| id);
    }

    /// A vehicle's speed (world units a second), 0 when not tracked.
    pub fn speed(&self, id: u32) -> f32 {
        self.tracks.iter().find(|t| t.0 == id).map_or(0.0, |t| t.3)
    }

    /// Leaves vehicle `id` alone for [`plan::SHUN_TICKS`].
    pub fn shun(&mut self, id: u32, tick: u32) {
        self.shunned.retain(|s| s.0 != id);
        self.shunned.push((id, tick + plan::SHUN_TICKS));
    }

    pub fn shunned(&self, id: u32, tick: u32) -> bool {
        self.shunned
            .iter()
            .any(|&(v, until)| v == id && tick < until)
    }

    /// A new life: nothing it was doing with vehicles carries on.
    pub fn reset(&mut self) {
        self.going = None;
        self.lock.reset();
        self.route = None;
        self.unstick.reset();
        self.leaving = None;
        self.flipped = 0;
        self.driverless = 0;
        self.was_in = None;
    }
}

#[cfg(test)]
pub(crate) mod tests;
