//! What the executor reads from the game, behind a trait so it can be
//! tested without a guest. [`GuestReader`] is the real one: it reuses
//! `live-telemetry`'s readers, which are tested against synthetic guest
//! memory, and only converts their types to the wire's.

use std::ops::DerefMut;

use agent_proto::{Kind, Object, ObjectId, Physics};
use live_telemetry::{
    ActorState, MachineState, Motion, ObjectState, PlayerState, ThingState, VehicleState, layout,
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
    /// Fill a recycled mirror buffer at this tick boundary.
    fn live_capture(&mut self, layers: agent_proto::live::Layers, out: &mut agent_proto::live::World) -> bool {
        let mut ok = false;
        self.with_guest(&mut |m| ok = crate::live::capture::read(m, layers, out).is_ok());
        ok
    }

    /// Subscription metadata from the current game options and loaded cache header.
    fn live_metadata(&mut self) -> (u16, Option<u32>) {
        let mut rate = 30;
        let mut checksum = None;
        self.with_guest(&mut |m| {
            if let Ok(Some(g)) = guest_layout::game::GameGlobals::current(m)
                && let Ok(hz) = g.options().tick_rate(m)
                && hz > 0
            {
                rate = hz;
            }
            checksum = guest_layout::game::cache_checksum(m).ok();
        });
        (rate, checksum)
    }

    /// Runs `f` on guest memory, for the `sim.*` methods' own reads and
    /// their host writes (at a tick boundary). False where there is no
    /// guest (the default).
    fn with_guest(&mut self, f: &mut dyn FnMut(&mut dyn guest_layout::GuestMemory)) -> bool {
        let _ = f;
        false
    }
}

/// Names definition tags by datum handle (the loaded map's tag names),
/// for `world.*` results' `tag_name` ([`crate::Agent::set_tag_names`]).
pub trait TagNames: Send {
    /// The names of `datums`, in order; `None` where unnamed.
    fn names(&mut self, datums: &[u32]) -> Vec<Option<String>>;
}

/// Whether a standing capsule fits at a biped's origin, for `world.*`
/// results' `physics.standing_fits` ([`crate::Agent::set_standing_fits`]).
pub trait StandingFits: Send {
    /// `None` when the host cannot tell.
    fn standing_fits(&mut self, origin: [f64; 3]) -> Option<bool>;
}

/// Fills `physics.standing_fits` of each biped (an object whose physics
/// says whether it is airborne) from `fits`.
pub fn fill_standing_fits(fits: &mut dyn StandingFits, objects: &mut [Object]) {
    for o in objects.iter_mut() {
        if let (Some(pos), Some(p)) = (o.pos, o.physics.as_mut())
            && p.airborne.is_some()
        {
            p.standing_fits = fits.standing_fits(pos);
        }
    }
}

/// Fills each object's `tag_name` from its `tag`, asking `names` once for
/// the distinct datums.
pub fn name_tags(names: &mut dyn TagNames, objects: &mut [Object]) {
    let datum = |o: &Object| {
        o.tag
            .as_deref()
            .and_then(|t| u32::from_str_radix(t, 16).ok())
    };
    let mut datums: Vec<u32> = objects.iter().filter_map(datum).collect();
    datums.sort_unstable();
    datums.dedup();
    if datums.is_empty() {
        return;
    }
    let named = names.names(&datums);
    for o in objects.iter_mut() {
        if let Some(i) = datum(&*o).and_then(|d| datums.binary_search(&d).ok()) {
            o.tag_name = named.get(i).cloned().flatten();
        }
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
    fn with_guest(&mut self, f: &mut dyn FnMut(&mut dyn guest_layout::GuestMemory)) -> bool {
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
        // Scenery, items, controls, crates and creatures: their own pass.
        let things = live_telemetry::things(&g, |t| Kind::of_object_type(t).is_some())
            .map_err(|e| format!("objects: {e:?}"))?;
        out.extend(things.iter().filter_map(thing_object));
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
        havok_pos: m.havok.map(|(p, _)| p),
        havok_velocity: m.havok.map(|(_, v)| v),
        support: m.support.as_ref().map(support),
        // The host's test fills it ([`crate::Agent::set_standing_fits`]).
        standing_fits: None,
    }
}

/// A biped's ground support, raw words as hex.
fn support(s: &live_telemetry::Support) -> agent_proto::Support {
    agent_proto::Support {
        flags: format!("{:02x}", s.flags),
        copied: s.flags & layout::BIPED_SUPPORT_COPIED != 0,
        words: s.words.iter().map(|&w| hex(w)).collect(),
        point: s.point,
        normal: s.normal,
        contact_point: s.contact_point,
        ground_state: s.ground_state,
        contacts: s
            .contacts
            .iter()
            .map(|c| agent_proto::CharacterContact {
                key: hex(c.key),
                other: hex(c.other),
                point: c.point,
                normal: c.normal,
                value: c.value,
                flags: hex(c.flags),
            })
            .collect(),
    }
}

/// `yaw` and `pitch` from `forward`.
fn angles(o: &mut Object) {
    if let Some((yaw, pitch)) = o.forward.and_then(agent_proto::sim::yaw_pitch) {
        o.yaw = Some(yaw);
        o.pitch = Some(pitch);
    }
}

/// The fields every unit shares (a player's unit, an actor, a body).
/// `forward` is where the unit aims (its aiming vector, which follows the
/// camera), the body's own forward `body_forward`: a biped's body turns
/// only as it moves, so its forward stays put while the view turns.
fn unit_fields(o: &mut Object, s: &ObjectState) {
    o.pos = Some(s.pos);
    o.forward = Some(s.aim.unwrap_or(s.forward));
    o.body_forward = Some(s.forward);
    angles(o);
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
    let mut o = Object {
        id: id(Kind::Vehicle, v.datum),
        kind: Kind::Vehicle,
        pos: Some(v.pos),
        forward: Some(v.forward),
        tag: v.definition.map(hex),
        occupied: Some(v.occupied),
        mounted_on: v.parent.map(|p| id(Kind::Vehicle, p)),
        ..Default::default()
    };
    angles(&mut o);
    o
}

pub fn machine_object(m: &MachineState) -> Object {
    let mut o = Object {
        id: id(Kind::Machine, m.datum),
        kind: Kind::Machine,
        pos: Some(m.pos),
        forward: Some(m.forward),
        tag: m.definition.map(hex),
        placement: m.placement,
        position: m.position,
        ..Default::default()
    };
    angles(&mut o);
    o
}

/// An object of a kind [`Kind::of_object_type`] names. One attached to
/// another (a held weapon, a grenade a unit carries) is left out: its pose
/// is relative to what holds it, and a unit's `weapon` already names it.
pub fn thing_object(t: &ThingState) -> Option<Object> {
    let kind = Kind::of_object_type(t.object_type)?;
    if t.parent.is_some() {
        return None;
    }
    let mut o = Object {
        id: id(kind, t.datum),
        kind,
        pos: Some(t.pos),
        forward: Some(t.forward),
        tag: t.definition.map(hex),
        placement: t.placement,
        ..Default::default()
    };
    angles(&mut o);
    Some(o)
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
                havok: Some(([1.0, 2.0, 3.4], [0.5, 0.0, 0.0])),
                support: Some(live_telemetry::Support {
                    flags: 0x10,
                    words: [2, u32::MAX, 0x0b11, u32::MAX, 0x2000_0b11],
                    point: [43.5, -79.25, 1.2],
                    normal: [0.5, 0.0, 0.85],
                    contact_point: [43.5, -79.25, 1.2],
                    ground_state: [0.0, 0.0, -1.0],
                    contacts: vec![live_telemetry::CharacterContact {
                        key: 0x2000_0b11,
                        other: u32::MAX,
                        point: [43.5, -79.25, 1.2],
                        normal: [0.5, 0.0, 0.85],
                        value: 0.0,
                        flags: 6,
                    }],
                }),
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
        assert_eq!(p.havok_pos, Some([1.0, 2.0, 3.4]));
        assert_eq!(p.havok_velocity, Some([0.5, 0.0, 0.0]));
        assert!((p.hitbox_offset.unwrap() + 0.22).abs() < 1e-6);
        let s = p.support.expect("a biped's support");
        assert_eq!(s.flags, "10");
        assert!(s.copied);
        assert_eq!(
            s.words,
            ["00000002", "ffffffff", "00000b11", "ffffffff", "20000b11"]
        );
        assert_eq!(s.normal, [0.5, 0.0, 0.85]);
        assert_eq!(s.ground_state, [0.0, 0.0, -1.0]);
        assert_eq!(s.contacts.len(), 1);
        assert_eq!(s.contacts[0].key, "20000b11");
        assert_eq!(s.contacts[0].other, "ffffffff");
        assert_eq!(s.contacts[0].flags, "00000006");
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
            controller: Some(0),
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
    fn a_unit_faces_where_it_aims_and_keeps_its_body_forward() {
        // The body faces +x; the view has turned to -y and looks 30 up.
        let mut u = unit();
        let (c, s) = (30f64.to_radians().cos(), 30f64.to_radians().sin());
        u.aim = Some([0.0, -c, s]);
        let o = player_object(&player(0xe005_0010), Some(&u));
        assert_eq!(o.forward, Some([0.0, -c, s]));
        assert_eq!(o.body_forward, Some([1.0, 0.0, 0.0]));
        assert!((o.yaw.unwrap() - 270.0).abs() < 1e-9, "{:?}", o.yaw);
        assert!((o.pitch.unwrap() - 30.0).abs() < 1e-9, "{:?}", o.pitch);
        // No aim read (not finite or not unit length): the body's forward.
        let o = actor_object(
            Kind::Actor,
            &ActorState {
                datum: 1,
                object: unit(),
            },
        );
        assert_eq!(o.forward, Some([1.0, 0.0, 0.0]));
        assert_eq!((o.yaw, o.pitch), (Some(0.0), Some(0.0)));
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
            velocity: None,
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
            velocity: None,
        });
        assert_eq!(turret.mounted_on.as_deref(), Some("vehicle.e2000003"));
        assert_eq!(
            (v.id.as_str(), v.occupied),
            ("vehicle.e2000003", Some(true))
        );
        assert_eq!(v.tag.as_deref(), Some("00000007"));
    }

    fn thing(object_type: u8, parent: Option<u32>) -> ThingState {
        ThingState {
            datum: 0xe500_0005,
            object_type,
            definition: Some(0xf00d_0042),
            parent,
            placement: Some(3),
            pos: [10.0, 20.0, 1.0],
            forward: [0.0, 1.0, 0.0],
            up: [0.0, 0.0, 1.0],
        }
    }

    #[test]
    fn scenery_items_and_controls_are_objects_and_held_ones_are_not() {
        let s = thing_object(&thing(6, None)).unwrap();
        assert_eq!((s.id.as_str(), s.kind), ("scenery.e5000005", Kind::Scenery));
        assert_eq!(s.pos, Some([10.0, 20.0, 1.0]));
        assert_eq!(s.tag.as_deref(), Some("f00d0042"));
        assert_eq!(s.placement, Some(3));
        assert!((s.yaw.unwrap() - 90.0).abs() < 1e-9, "{:?}", s.yaw);
        assert_eq!(s.pitch, Some(0.0));
        let kinds: Vec<_> = [2, 3, 8, 11, 12]
            .into_iter()
            .map(|t| thing_object(&thing(t, None)).unwrap().kind)
            .collect();
        assert_eq!(
            kinds,
            [
                Kind::Weapon,
                Kind::Equipment,
                Kind::Control,
                Kind::Crate,
                Kind::Creature
            ]
        );
        // A held weapon, and the types with readers of their own.
        assert_eq!(thing_object(&thing(2, Some(0xe000_0001))), None);
        for t in [0, 1, 4, 5, 7, 9, 10] {
            assert_eq!(thing_object(&thing(t, None)), None, "type {t}");
        }
    }

    struct Named(Vec<Vec<u32>>);
    impl TagNames for Named {
        fn names(&mut self, datums: &[u32]) -> Vec<Option<String>> {
            self.0.push(datums.to_vec());
            datums
                .iter()
                .map(|d| {
                    (*d == 0xf00d_0042)
                        .then(|| "scenery\\teleporter_base\\teleporter_base".to_owned())
                })
                .collect()
        }
    }

    #[test]
    fn tags_are_named_once_per_datum() {
        let mut objects = vec![
            thing_object(&thing(6, None)).unwrap(),
            thing_object(&thing(6, None)).unwrap(),
            Object {
                tag: Some("00000007".into()),
                ..Default::default()
            },
            Object::default(),
        ];
        let mut names = Named(Vec::new());
        name_tags(&mut names, &mut objects);
        assert_eq!(names.0, [vec![7, 0xf00d_0042]]);
        let teleporter = Some("scenery\\teleporter_base\\teleporter_base");
        assert_eq!(objects[0].tag_name.as_deref(), teleporter);
        assert_eq!(objects[1].tag_name.as_deref(), teleporter);
        assert_eq!(objects[2].tag_name, None, "not named");
        assert_eq!(objects[3].tag_name, None, "no tag");
        // Nothing to name: no call.
        name_tags(&mut names, &mut [Object::default()]);
        assert_eq!(names.0.len(), 1);
    }

    #[test]
    fn standing_fits_is_filled_for_bipeds_only() {
        struct Low;
        impl StandingFits for Low {
            fn standing_fits(&mut self, origin: [f64; 3]) -> Option<bool> {
                Some(origin[2] > 0.0)
            }
        }
        let biped = |z: f64| Object {
            pos: Some([0.0, 0.0, z]),
            physics: Some(Physics {
                airborne: Some(false),
                ..Default::default()
            }),
            ..Default::default()
        };
        let vehicle = Object {
            pos: Some([0.0, 0.0, 1.0]),
            physics: Some(Physics::default()),
            ..Default::default()
        };
        let mut objects = vec![biped(1.0), biped(-1.0), vehicle, Object::default()];
        fill_standing_fits(&mut Low, &mut objects);
        let fits: Vec<_> = objects
            .iter()
            .map(|o| o.physics.as_ref().and_then(|p| p.standing_fits))
            .collect();
        assert_eq!(fits, [Some(true), Some(false), None, None]);
    }
}
