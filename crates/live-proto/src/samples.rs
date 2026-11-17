//! Invented but realistic telemetry frames, for size and speed
//! measurements and codec tests (docs/live/live-telemetry.md, "Sizes"): a
//! 16-player multiplayer game and a campaign level with 60 actors, as the
//! game would send them at 15 Hz (every second tick). Deterministic.

use crate::game::{
    Frame, FrameActor, FrameHit, FrameMachine, FrameObject, FramePlayer, FrameVehicle, ObjectKind,
};

/// A small deterministic generator (xorshift).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }
    fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
        xs[(self.next() % xs.len() as u64) as usize]
    }
}

const WEAPONS: [&str; 8] = [
    "battle_rifle",
    "smg",
    "magnum",
    "shotgun",
    "plasma_rifle",
    "carbine",
    "sniper_rifle",
    "energy_blade",
];

fn aim(yaw: f64, pitch: f64) -> [f64; 3] {
    [
        yaw.cos() * pitch.cos(),
        yaw.sin() * pitch.cos(),
        pitch.sin(),
    ]
}

/// One moving unit: circles at its own speed and radius.
#[derive(Clone)]
struct Mover {
    centre: [f64; 3],
    radius: f64,
    speed: f64,
    phase: f64,
    weapon: &'static str,
}

impl Mover {
    fn new(rng: &mut Rng, extent: f64) -> Mover {
        Mover {
            centre: [
                rng.range(-extent, extent),
                rng.range(-extent, extent),
                rng.range(-2.0, 8.0),
            ],
            radius: rng.range(2.0, 12.0),
            speed: rng.range(1.0, 4.5),
            phase: rng.range(0.0, std::f64::consts::TAU),
            weapon: rng.pick(&WEAPONS),
        }
    }

    /// Position and heading at `t` seconds.
    fn at(&self, t: f64) -> ([f64; 3], f64) {
        let a = self.phase + t * self.speed / self.radius;
        (
            [
                self.centre[0] + self.radius * a.cos(),
                self.centre[1] + self.radius * a.sin(),
                self.centre[2] + 0.2 * (a * 3.0).sin(),
            ],
            a + std::f64::consts::FRAC_PI_2,
        )
    }
}

fn round(v: f64) -> f64 {
    // What f32 guest memory widened to f64 looks like.
    f64::from(v as f32)
}

fn r3(v: [f64; 3]) -> [f64; 3] {
    v.map(round)
}

fn player(index: u32, m: &Mover, t: f64, tick: u64, rng: &mut Rng) -> FramePlayer {
    let (pos, yaw) = m.at(t);
    let firing = (tick / 30 + u64::from(index)).is_multiple_of(3);
    FramePlayer {
        index,
        alive: true,
        pos: r3(pos),
        yaw: round(yaw),
        vehicle: None,
        species: Some(
            if index.is_multiple_of(2) {
                "spartan"
            } else {
                "elite"
            }
            .into(),
        ),
        aim: Some(r3(aim(yaw + 0.1 * (t * 2.0).sin(), 0.05 * (t * 1.3).cos()))),
        weapon: Some(m.weapon.into()),
        shots: firing.then(|| 1 + (rng.next() % 3) as u32),
        stance: Some(
            if (tick / 60 + u64::from(index)).is_multiple_of(5) {
                "crouch"
            } else {
                "stand"
            }
            .into(),
        ),
        speed: Some(round(m.speed)),
        actions: rng
            .next()
            .is_multiple_of(40)
            .then(|| vec!["reload".to_owned()]),
        grenade: Some("frag".into()),
    }
}

fn vehicle(id: u32, kind: &str, m: &Mover, t: f64, occupied: bool) -> FrameVehicle {
    let (pos, yaw) = if occupied { m.at(t) } else { m.at(0.0) };
    FrameVehicle {
        id: format!("{id:08x}"),
        kind: kind.into(),
        pos: r3(pos),
        forward: r3([yaw.cos(), yaw.sin(), 0.0]),
        up: [0.0, 0.0, 1.0],
        occupied,
        parent: None,
        aim: None,
    }
}

/// The Warthog's chaingun: a turret riding vehicle `parent`, its position
/// parent-relative as the game has it, sweeping its aim.
fn turret(id: u32, parent: u32, t: f64) -> FrameVehicle {
    let yaw = 0.8 * (t * 1.7).sin();
    FrameVehicle {
        id: format!("{id:08x}"),
        kind: "chaingun".into(),
        pos: [-0.6, 0.0, 0.75],
        forward: [1.0, 0.0, 0.0],
        up: [0.0, 0.0, 1.0],
        occupied: true,
        parent: Some(format!("{parent:08x}")),
        aim: Some(r3(aim(yaw, 0.1))),
    }
}

/// `frames` live frames (15 Hz) of a 16-player Slayer game with a few
/// vehicles, hits each frame and weapons picked up from their spawns.
pub fn multiplayer(frames: usize) -> Vec<Frame> {
    let mut rng = Rng(0x5eed_0016);
    let movers: Vec<Mover> = (0..16).map(|_| Mover::new(&mut rng, 40.0)).collect();
    let rides: Vec<Mover> = (0..4).map(|_| Mover::new(&mut rng, 40.0)).collect();
    (0..frames)
        .map(|n| {
            let tick = 9000 + 2 * n as u64;
            let t = n as f64 / 15.0;
            let players = movers
                .iter()
                .enumerate()
                .map(|(i, m)| player(i as u32, m, t, tick, &mut rng))
                .collect();
            let vehicles = rides
                .iter()
                .enumerate()
                .map(|(i, m)| {
                    let kind = ["warthog", "ghost", "banshee", "mongoose"][i];
                    vehicle(0x9004_0004 + i as u32 * 0x10001, kind, m, t, i < 2)
                })
                .chain([turret(0x9008_0008, 0x9004_0004, t)])
                .collect();
            let hits = (0..(rng.next() % 4))
                .map(|_| FrameHit {
                    from: Some(format!("p{}", rng.next() % 16)),
                    to: Some(format!("p{}", rng.next() % 16)),
                    weapon: Some(rng.pick(&WEAPONS).into()),
                    pos: r3(movers[(rng.next() % 16) as usize].at(t).0),
                    amount: Some(round(rng.range(0.05, 0.4))),
                    kind: Some("bullet".into()),
                })
                .collect();
            // Weapons taken from their spawns come back after 30 s.
            let objects = (0..24u32)
                .filter(|i| (n as u64 / 450 + u64::from(*i)).is_multiple_of(3))
                .map(|i| FrameObject {
                    k: ObjectKind::NetgameEquipment,
                    i,
                    p: None,
                    f: None,
                    u: None,
                    gone: true,
                })
                .collect();
            Frame {
                game_id: "7f3c0b5e2a9d4c1f8e6b3a2d1c0f9e8d".into(),
                tick,
                players,
                vehicles,
                actors: Vec::new(),
                hits,
                bsp: Some(0),
                machines: Vec::new(),
                objects,
            }
        })
        .collect()
}

const SPECIES: [&str; 6] = ["grunt", "jackal", "elite", "marine", "hunter", "brute"];
const TEAMS: [&str; 3] = ["covenant", "human", "player"];

/// `frames` live frames of a campaign level: one player, 60 actors, 8
/// vehicles, moving machines and scenery knocked about.
pub fn campaign(frames: usize) -> Vec<Frame> {
    let mut rng = Rng(0x5eed_0060);
    let hero = Mover::new(&mut rng, 60.0);
    let actors: Vec<(Mover, &str, &str)> = (0..60)
        .map(|i| {
            let m = Mover::new(&mut rng, 60.0);
            let species = rng.pick(&SPECIES);
            let team = if species == "marine" {
                TEAMS[1]
            } else {
                TEAMS[0]
            };
            let _ = i;
            (m, species, team)
        })
        .collect();
    let rides: Vec<Mover> = (0..8).map(|_| Mover::new(&mut rng, 60.0)).collect();
    let crates: Vec<([f64; 3], f64)> = (0..40)
        .map(|_| {
            (
                [
                    rng.range(-60.0, 60.0),
                    rng.range(-60.0, 60.0),
                    rng.range(0.0, 4.0),
                ],
                rng.range(0.0, 6.0),
            )
        })
        .collect();
    (0..frames)
        .map(|n| {
            let tick = 30000 + 2 * n as u64;
            let t = n as f64 / 15.0;
            let mut p = player(0, &hero, t, tick, &mut rng);
            p.species = Some("spartan".into());
            let actor_list = actors
                .iter()
                .enumerate()
                .map(|(i, (m, species, team))| {
                    // Some stand still (guards), most patrol.
                    let (pos, yaw) = if i % 4 == 0 { m.at(0.0) } else { m.at(t) };
                    let firing = (tick / 20 + i as u64).is_multiple_of(7);
                    FrameActor {
                        id: format!("{:08x}", 0xe100_0010u32 + i as u32 * 0x10001),
                        species: (*species).into(),
                        pos: r3(pos),
                        yaw: round(yaw),
                        aim: Some(r3(aim(yaw, 0.02))),
                        weapon: Some(m.weapon.into()),
                        shots: firing.then_some(2),
                        team: Some((*team).into()),
                        vehicle: None,
                        stance: Some("stand".into()),
                        speed: Some(if i % 4 == 0 { 0.0 } else { round(m.speed) }),
                        actions: None,
                        grenade: Some("plasma".into()),
                    }
                })
                .collect();
            let vehicles = rides
                .iter()
                .enumerate()
                .map(|(i, m)| {
                    let kind = [
                        "warthog", "ghost", "wraith", "banshee", "spectre", "scorpion", "ghost",
                        "warthog",
                    ][i];
                    vehicle(0x9104_0004 + i as u32 * 0x10001, kind, m, t, i < 3)
                })
                .collect();
            let hits = (0..(rng.next() % 6))
                .map(|_| FrameHit {
                    from: Some(if rng.next().is_multiple_of(2) {
                        "p0".into()
                    } else {
                        format!(
                            "{:08x}",
                            0xe100_0010u32 + (rng.next() % 60) as u32 * 0x10001
                        )
                    }),
                    to: Some(format!(
                        "{:08x}",
                        0xe100_0010u32 + (rng.next() % 60) as u32 * 0x10001
                    )),
                    weapon: Some(rng.pick(&WEAPONS).into()),
                    pos: r3(hero.at(t).0),
                    amount: Some(round(rng.range(0.05, 0.6))),
                    kind: Some("bullet".into()),
                })
                .collect();
            // An elevator and two doors moving; all twelve machines in the
            // first frame.
            let machines = (0..12u32)
                .filter(|i| n == 0 || *i < 3)
                .map(|i| {
                    let travel = if i < 3 {
                        ((t * 0.2 + f64::from(i)).sin() + 1.0) / 2.0
                    } else {
                        0.0
                    };
                    FrameMachine {
                        node: Some(format!("door_{i}")),
                        id: format!("{:08x}", 0xd200_0020u32 + i * 0x10001),
                        kind: if i == 0 {
                            "elevator".into()
                        } else {
                            "door".into()
                        },
                        pos: r3([f64::from(i) * 7.0, -20.0, 3.0 * travel]),
                        forward: [1.0, 0.0, 0.0],
                        up: [0.0, 0.0, 1.0],
                        position: Some(round(travel)),
                    }
                })
                .collect();
            // Crates knocked about early on settle; ten more gone (blown
            // up), and placed weapons picked up.
            let mut objects: Vec<FrameObject> = crates
                .iter()
                .enumerate()
                .filter(|(i, _)| *i < 25)
                .map(|(i, (at, yaw))| {
                    let settle = (t * 0.5).min(1.0);
                    let moving = i < 5;
                    let shift = if moving { settle * 3.0 } else { 1.5 };
                    FrameObject {
                        k: ObjectKind::Crate,
                        i: i as u32,
                        p: Some(r3([at[0] + shift, at[1], at[2]])),
                        f: Some(r3([yaw.cos(), yaw.sin(), 0.0])),
                        u: Some([0.0, 0.0, 1.0]),
                        gone: false,
                    }
                })
                .collect();
            objects.extend((0..10).map(|i| FrameObject {
                k: ObjectKind::Scenery,
                i: 100 + i,
                p: None,
                f: None,
                u: None,
                gone: true,
            }));
            objects.extend((0..6).map(|i| FrameObject {
                k: ObjectKind::Weapon,
                i,
                p: None,
                f: None,
                u: None,
                gone: true,
            }));
            objects.sort_by_key(|o| (o.k, o.i));
            Frame {
                game_id: "c4a1d2e3f4a5b6c7d8e9f0a1b2c3d4e5".into(),
                tick,
                players: vec![p],
                vehicles,
                actors: actor_list,
                hits,
                bsp: Some(1),
                machines,
                objects,
            }
        })
        .collect()
}
