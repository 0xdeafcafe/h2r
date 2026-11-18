//! What the executor reads from the game, behind a trait so it can be
//! tested without a guest. [`GuestReader`] is the real one: it reuses
//! `live-telemetry`'s readers, which are tested against synthetic guest
//! memory, and only converts their types to the wire's.

use std::ops::DerefMut;

use agent_proto::{Kind, Object, ObjectId, Physics};
use live_telemetry::{
    ActorState, MachineState, Motion, ObjectState, PlayerState, VehicleState, layout,
};
use runtime::Guest;

/// The game's state as the stamp needs it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GameStatus {
    pub tick: Option<u64>,
    pub map: Option<String>,
    pub mode: Option<String>,
    pub bsp: Option<u32>,
}

pub trait Reader {
    /// Cheap: called at every boundary the executor serves.
    fn status(&mut self) -> GameStatus;
    /// Every object the API lists, unfiltered. Called per world request.
    fn objects(&mut self) -> Result<Vec<Object>, String>;
    /// Runs `f` on guest memory, for the `sim.*` methods' own reads and
    /// their host writes (at a tick boundary). False where there is no
    /// guest (the default).
    fn with_guest(&mut self, f: &mut dyn FnMut(&mut dyn engine::GuestMemory)) -> bool {
        let _ = f;
        false
    }
}

/// Reads through a function that locks guest memory for each read, so the
/// lock is never held while the executor waits (a paused game).
pub struct GuestReader<F> {
    guest: F,
}

impl<F, G> GuestReader<F>
where
    F: Fn() -> G,
    G: DerefMut<Target = Guest>,
{
    pub fn new(guest: F) -> Self {
        GuestReader { guest }
    }
}

impl<F, G> Reader for GuestReader<F>
where
    F: Fn() -> G,
    G: DerefMut<Target = Guest>,
{
    fn with_guest(&mut self, f: &mut dyn FnMut(&mut dyn engine::GuestMemory)) -> bool {
        let mut g = (self.guest)();
        f(&mut *g);
        true
    }

    fn status(&mut self) -> GameStatus {
        let g = (self.guest)();
        let info = live_telemetry::game_info(&g).ok();
        let tick = match &info {
            Some(i) => Some(i.tick),
            None => live_telemetry::game_tick(&g).ok(),
        };
        let map = info
            .as_ref()
            .map(|i| i.map.clone())
            .or_else(|| live_telemetry::loaded_map_name(&g).ok())
            .filter(|m| !m.is_empty());
        GameStatus {
            tick,
            map,
            mode: info.and_then(|i| known_mode(i.mode)),
            bsp: live_telemetry::active_bsp(&g).ok().flatten(),
        }
    }

    fn objects(&mut self) -> Result<Vec<Object>, String> {
        let g = (self.guest)();
        let players = live_telemetry::players(&g).map_err(|e| format!("players: {e:?}"))?;
        let mut out = Vec::new();
        for p in &players {
            // A player between lives has no unit; list them anyway.
            let unit = live_telemetry::object(&g, p.unit).ok().flatten();
            out.push(player_object(p, unit.as_ref()));
        }
        let world = live_telemetry::world(&g).map_err(|e| format!("world: {e:?}"))?;
        out.extend(world.actors.iter().map(|a| actor_object(Kind::Actor, a)));
        out.extend(world.vehicles.iter().map(vehicle_object));
        out.extend(world.machines.iter().map(machine_object));
        out.extend(world.bodies.iter().map(|a| actor_object(Kind::Body, a)));
        Ok(out)
    }
}

/// While a map loads, the options say neither game nor menu
/// (`unknown_0`): no mode, rather than a made-up one.
pub fn known_mode(mode: String) -> Option<String> {
    (!mode.starts_with("unknown_")).then_some(mode)
}

fn hex(v: u32) -> String {
    format!("{v:08x}")
}

fn id(kind: Kind, key: u32) -> String {
    ObjectId { kind, key }.to_string()
}

/// Crouch and character-physics state, from what telemetry read.
fn physics(m: &Motion) -> Physics {
    Physics {
        crouch: m.crouch,
        crouch_request: m.unit_flags.map(|f| f & layout::UNIT_FLAG_CROUCH != 0),
        mode: m.physics_mode,
        airborne: m.biped_flags.map(|f| f & 1 != 0),
        airborne_ticks: m.airborne_ticks,
        unit_flags: m.unit_flags.map(hex),
        biped_flags: m.biped_flags.map(|f| format!("{f:04x}")),
        hitbox_offset_ticks: m.hitbox_offset,
        // 0x000e0d70: counter × tick length × rate, at 30 ticks a second.
        hitbox_offset: m
            .hitbox_offset
            .map(|t| f64::from(t) / 30.0 * f64::from(layout::HITBOX_OFFSET_RATE)),
    }
}

/// The fields every unit shares (a player's unit, an actor, a body).
fn unit_fields(o: &mut Object, s: &ObjectState) {
    o.pos = Some(s.pos);
    o.forward = Some(s.forward);
    o.velocity = Some(s.motion.velocity);
    o.physics = Some(physics(&s.motion));
    o.team = s.team.map(i32::from);
    o.tag = s.definition.map(hex);
    o.vehicle = s.vehicle.map(|v| id(Kind::Vehicle, v));
    o.weapon = s.weapon.map(|w| hex(w.definition));
}

pub fn player_object(p: &PlayerState, unit: Option<&ObjectState>) -> Object {
    let mut o = Object {
        id: id(Kind::Player, p.player.index),
        kind: Kind::Player,
        ..Default::default()
    };
    if let Some(u) = unit {
        unit_fields(&mut o, u);
    }
    // The player's team is authoritative; the unit's may be unset.
    o.team = Some(p.player.team);
    o.name = Some(p.player.name.clone()).filter(|n| !n.is_empty());
    o.local_user = p.local_user;
    o.unit = (p.unit != u32::MAX).then(|| hex(p.unit));
    o
}

pub fn actor_object(kind: Kind, a: &ActorState) -> Object {
    let mut o = Object {
        id: id(kind, a.datum),
        kind,
        ..Default::default()
    };
    unit_fields(&mut o, &a.object);
    o
}

pub fn vehicle_object(v: &VehicleState) -> Object {
    Object {
        id: id(Kind::Vehicle, v.datum),
        kind: Kind::Vehicle,
        pos: Some(v.pos),
        forward: Some(v.forward),
        tag: v.definition.map(hex),
        occupied: Some(v.occupied),
        mounted_on: v.parent.map(|p| id(Kind::Vehicle, p)),
        ..Default::default()
    }
}

pub fn machine_object(m: &MachineState) -> Object {
    Object {
        id: id(Kind::Machine, m.datum),
        kind: Kind::Machine,
        pos: Some(m.pos),
        forward: Some(m.forward),
        tag: m.definition.map(hex),
        placement: m.placement,
        position: m.position,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use live_telemetry::Weapon;

    fn unit() -> ObjectState {
        ObjectState {
            pos: [1.0, 2.0, 3.0],
            forward: [1.0, 0.0, 0.0],
            vehicle: Some(0xe000_0007),
            definition: Some(0x1234_0001),
            weapon: Some(Weapon {
                datum: 0xe001_0002,
                definition: 0xabcd_0003,
            }),
            team: Some(2),
            motion: Motion {
                velocity: [0.5, 0.0, 0.0],
                crouch: Some(1.0),
                physics_mode: Some(1),
                unit_flags: Some(1 << 23),
                biped_flags: Some(0x0001),
                airborne_ticks: Some(4),
                hitbox_offset: Some(-44),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn a_loading_mode_is_no_mode() {
        assert_eq!(known_mode("unknown_0".into()), None);
        assert_eq!(known_mode("campaign".into()).as_deref(), Some("campaign"));
    }

    #[test]
    fn an_actor_carries_its_unit_and_salted_id() {
        let o = actor_object(
            Kind::Actor,
            &ActorState {
                datum: 0xe123_0045,
                object: unit(),
            },
        );
        assert_eq!(o.id, "actor.e1230045");
        assert_eq!(o.kind, Kind::Actor);
        assert_eq!(o.pos, Some([1.0, 2.0, 3.0]));
        assert_eq!(o.velocity, Some([0.5, 0.0, 0.0]));
        let p = o.physics.expect("units carry physics");
        assert_eq!(p.crouch, Some(1.0));
        assert_eq!(p.crouch_request, Some(true));
        assert_eq!(p.mode, Some(1));
        assert_eq!(p.airborne, Some(true));
        assert_eq!(p.airborne_ticks, Some(4));
        assert_eq!(p.unit_flags.as_deref(), Some("00800000"));
        assert_eq!(p.biped_flags.as_deref(), Some("0001"));
        assert_eq!(p.hitbox_offset_ticks, Some(-44));
        assert!((p.hitbox_offset.unwrap() + 0.22).abs() < 1e-6);
        assert_eq!(o.team, Some(2));
        assert_eq!(o.tag.as_deref(), Some("12340001"));
        assert_eq!(o.vehicle.as_deref(), Some("vehicle.e0000007"));
        assert_eq!(o.weapon.as_deref(), Some("abcd0003"));
        assert_eq!(o.name, None);
    }

    fn player(unit: u32) -> PlayerState {
        PlayerState {
            datum: 0xe000_0000,
            player: live_proto::game::Player {
                index: 1,
                name: "Chief".into(),
                xuid: None,
                team: 0,
                local: Some(true),
                appearance: None,
                bot: None,
            },
            appearance: Default::default(),
            local_user: Some(0),
            machine: [0; 6],
            unit,
            stats: live_telemetry::PlayerStats::ZERO,
        }
    }

    #[test]
    fn a_player_takes_the_team_from_the_player_record() {
        let o = player_object(&player(0xe005_0010), Some(&unit()));
        assert_eq!(o.id, "player.1");
        assert_eq!(o.team, Some(0));
        assert_eq!(o.name.as_deref(), Some("Chief"));
        assert_eq!(o.unit.as_deref(), Some("e0050010"));
        assert_eq!(o.local_user, Some(0));
        assert_eq!(o.pos, Some([1.0, 2.0, 3.0]));
    }

    #[test]
    fn a_dead_player_is_listed_without_a_unit() {
        let o = player_object(&player(u32::MAX), None);
        assert_eq!(o.unit, None);
        assert_eq!(o.pos, None);
        assert_eq!(o.team, Some(0));
    }

    #[test]
    fn machines_and_vehicles_keep_their_own_fields() {
        let m = machine_object(&MachineState {
            datum: 0xe100_0002,
            definition: None,
            placement: Some(4),
            pos: [0.0; 3],
            forward: [0.0; 3],
            up: [0.0; 3],
            position: Some(0.25),
        });
        assert_eq!(m.id, "machine.e1000002");
        assert_eq!((m.placement, m.position), (Some(4), Some(0.25)));
        let v = vehicle_object(&VehicleState {
            datum: 0xe200_0003,
            definition: Some(7),
            pos: [0.0; 3],
            forward: [0.0; 3],
            up: [0.0; 3],
            occupied: true,
            parent: None,
            aim: None,
        });
        assert_eq!(v.mounted_on, None);
        let turret = vehicle_object(&VehicleState {
            datum: 0xe300_0004,
            definition: None,
            pos: [0.0, 0.0, 1.0],
            forward: [1.0, 0.0, 0.0],
            up: [0.0; 3],
            occupied: false,
            parent: Some(0xe200_0003),
            aim: None,
        });
        assert_eq!(turret.mounted_on.as_deref(), Some("vehicle.e2000003"));
        assert_eq!(
            (v.id.as_str(), v.occupied),
            ("vehicle.e2000003", Some(true))
        );
        assert_eq!(v.tag.as_deref(), Some("00000007"));
    }
}
