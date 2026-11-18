//! The bot's mind, stages 2 and 3 (docs/game/bots.md, sections 3.1 to
//! 3.4): what it sees through the map's walls, what it decides to do, how it
//! moves along the nav mesh, and how a human-like hand aims.
//!
//! [`Mind::step`] is called every frame with what the bot perceives and the
//! map's [`MapData`] (when built: without it the bot sees everyone and walks
//! straight, as stage 1 did). Time is the game's tick (30 a second), so the
//! frame rate does not change how a bot plays.

use input::host::{HostButton, HostPadState};

use crate::brain::{Perception, Seen, Vec3};
use crate::cache::MapData;
use crate::nav::Follower;

/// How good a bot is (docs/game/bots.md, section 3.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Skill {
    Easy,
    #[default]
    Normal,
    Heroic,
    Legendary,
}

impl Skill {
    pub const ALL: [Skill; 4] = [Skill::Easy, Skill::Normal, Skill::Heroic, Skill::Legendary];

    pub fn parse(s: &str) -> Option<Skill> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "easy" => Skill::Easy,
            "normal" => Skill::Normal,
            "heroic" => Skill::Heroic,
            "legendary" => Skill::Legendary,
            _ => return None,
        })
    }

    /// The aim model's numbers for this skill (section 3.4's table).
    pub fn aim(self) -> Aim {
        match self {
            Skill::Easy => Aim {
                reaction: 14,
                error: 0.10,
                lead: 0.3,
                gain: 1.6,
                fire_cone: 10f32.to_radians(),
                burst: Some((12, 18)),
                grenades_per_minute: 0.5,
            },
            Skill::Normal => Aim {
                reaction: 10,
                error: 0.06,
                lead: 0.6,
                gain: 2.5,
                fire_cone: 7f32.to_radians(),
                burst: Some((24, 8)),
                grenades_per_minute: 1.0,
            },
            Skill::Heroic => Aim {
                reaction: 7,
                error: 0.035,
                lead: 0.85,
                gain: 3.5,
                fire_cone: 5f32.to_radians(),
                burst: None,
                grenades_per_minute: 2.0,
            },
            Skill::Legendary => Aim {
                reaction: 5,
                error: 0.02,
                lead: 1.0,
                gain: 4.5,
                fire_cone: 4f32.to_radians(),
                burst: None,
                grenades_per_minute: 3.0,
            },
        }
    }
}

/// The aim model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aim {
    /// Ticks from first seeing a target to starting to turn to it.
    pub reaction: u32,
    /// The tracking error's spread (radians): a slowly wandering offset.
    pub error: f32,
    /// How much of the needed lead it gives a moving target.
    pub lead: f32,
    /// Right stick per radian of aim error.
    pub gain: f32,
    /// Fires while the aim is this close to the target.
    pub fire_cone: f32,
    /// Trigger on and off ticks, or `None` to hold it.
    pub burst: Option<(u32, u32)>,
    pub grenades_per_minute: f32,
}

/// Ticks a second.
pub const TICKS: f32 = 30.0;
/// The eye's and the target point's heights over a unit's origin.
pub const EYE: f32 = 0.6;
pub const CHEST: f32 = 0.4;
/// Notices an enemy within this angle of its aim, or this close anywhere.
pub const NOTICE_CONE: f32 = 65.0 * std::f32::consts::PI / 180.0;
pub const NOTICE_NEAR: f32 = 3.0;
/// Forgets a last known position after this long.
pub const MEMORY_TICKS: u32 = 300;
/// The speed lead is worked out for (world units a second).
pub const SHOT_SPEED: f32 = 90.0;
pub const MELEE_RANGE: f32 = 0.9;
pub const FIRE_RANGE: f32 = 25.0;
pub const GRENADE_RANGE: (f32, f32) = (3.0, 12.0);
pub const GRENADE_COOLDOWN: u32 = 240;
/// Crouch: clicking the left stick, Halo 2's default layout.
pub const CROUCH: HostButton = HostButton::LeftStick;

/// What the mind is doing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Doing {
    Idle,
    Fight(u32),
    Chase(Vec3),
    Roam,
}

#[derive(Clone, Copy, Debug)]
struct Track {
    at: Vec3,
    tick: u32,
    vel: Vec3,
}

/// One bot's mind.
#[derive(Clone, Debug)]
pub struct Mind {
    pub skill: Skill,
    pub doing: Doing,
    rng: u64,
    tick: Option<u32>,
    /// The target and the tick it was first seen.
    target: Option<(u32, u32)>,
    tracks: std::collections::HashMap<u32, Track>,
    last_seen: Option<(u32, Vec3, u32)>,
    /// The tracking error (yaw, pitch).
    err: (f32, f32),
    strafe: (f32, u32),
    grenade_ready: u32,
    grenade_until: u32,
    follower: Option<Follower>,
    goal: Option<Vec3>,
    /// The teabag layer (§3.5).
    pub emote: crate::emote::Emoter,
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn len(a: Vec3) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}
fn up(p: Vec3, h: f32) -> Vec3 {
    [p[0], p[1], p[2] + h]
}
fn angles(v: Vec3) -> (f32, f32) {
    let flat = (v[0] * v[0] + v[1] * v[1]).sqrt();
    (v[1].atan2(v[0]), v[2].atan2(flat))
}
fn wrap(a: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    (a + PI).rem_euclid(TAU) - PI
}
fn angle_between(a: Vec3, b: Vec3) -> f32 {
    let d = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2]) / (len(a) * len(b)).max(1e-9);
    d.clamp(-1.0, 1.0).acos()
}

impl Mind {
    pub fn new(skill: Skill, seed: u64) -> Self {
        Mind {
            skill,
            doing: Doing::Idle,
            rng: seed | 1,
            tick: None,
            target: None,
            tracks: Default::default(),
            last_seen: None,
            err: (0.0, 0.0),
            strafe: (1.0, 0),
            grenade_ready: 0,
            grenade_until: 0,
            follower: None,
            goal: None,
            emote: crate::emote::Emoter::new(crate::emote::DEFAULT_TAUNT, seed ^ 0x7ea_ba6),
        }
    }

    fn next(&mut self) -> u64 {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        self.rng.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn uniform(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
    fn normal(&mut self) -> f32 {
        let (u, v) = (self.uniform().max(1e-7), self.uniform());
        (-2.0 * u.ln()).sqrt() * (std::f32::consts::TAU * v).cos()
    }

    /// Who the bot can see this tick: enemies in its notice cone (or very
    /// near) with nothing solid between its eye and their chest.
    pub fn visible(p: &Perception, map: Option<&MapData>) -> Vec<Seen> {
        let Some(me) = p.me else { return Vec::new() };
        let eye = up(me.position, EYE);
        p.others
            .iter()
            .filter(|o| !p.teams || o.team != me.team)
            .filter(|o| {
                let to = sub(up(o.position, CHEST), eye);
                len(to) < NOTICE_NEAR || angle_between(to, me.aim) < NOTICE_CONE
            })
            .filter(|o| map.is_none_or(|m| m.walls.clear(eye, up(o.position, CHEST))))
            .copied()
            .collect()
    }

    /// This frame's pad.
    pub fn step(&mut self, p: &Perception, map: Option<&MapData>) -> HostPadState {
        let mut pad = HostPadState::default();
        let Some(me) = p.me else {
            self.follower = None;
            self.target = None;
            self.doing = Doing::Idle;
            return pad;
        };
        let new_tick = self.tick != Some(p.tick);
        let dt = self.tick.map_or(1, |t| p.tick.wrapping_sub(t)).clamp(1, 30);
        self.tick = Some(p.tick);
        let aim = self.skill.aim();

        // Tracks: velocity from the last position.
        if new_tick {
            for o in &p.others {
                let vel = match self.tracks.get(&o.id) {
                    Some(t) if p.tick > t.tick => {
                        scale(sub(o.position, t.at), TICKS / (p.tick - t.tick) as f32)
                    }
                    Some(t) => t.vel,
                    None => [0.0; 3],
                };
                self.tracks.insert(
                    o.id,
                    Track {
                        at: o.position,
                        tick: p.tick,
                        vel,
                    },
                );
            }
            // The tracking error wanders (Ornstein-Uhlenbeck, per tick).
            let theta = 0.15f32;
            for _ in 0..dt.min(5) {
                let k = aim.error * (2.0 * theta).sqrt();
                let (ny, np) = (self.normal(), self.normal());
                self.err.0 += -theta * self.err.0 + k * ny;
                self.err.1 += -theta * self.err.1 + k * np * 0.5;
            }
        }

        let seen = Self::visible(p, map);
        let eye = up(me.position, EYE);
        // The target: the one we have if still seen, else the nearest seen.
        let keep = self
            .target
            .and_then(|(id, t0)| seen.iter().find(|o| o.id == id).map(|o| (*o, t0)));
        let chosen = keep.or_else(|| {
            seen.iter()
                .min_by(|a, b| len(sub(a.position, eye)).total_cmp(&len(sub(b.position, eye))))
                .map(|o| (*o, p.tick))
        });
        self.target = chosen.map(|(o, t0)| (o.id, t0));
        if let Some((o, _)) = chosen {
            self.last_seen = Some((o.id, o.position, p.tick));
        }
        if self
            .last_seen
            .is_some_and(|(_, _, t)| p.tick.wrapping_sub(t) > MEMORY_TICKS)
        {
            self.last_seen = None;
        }

        self.doing = match (chosen, self.last_seen) {
            (Some((o, _)), _) => Doing::Fight(o.id),
            (None, Some((_, at, _))) => Doing::Chase(at),
            (None, None) => Doing::Roam,
        };

        // Where to look and where to go.
        let mut look: Option<Vec3> = None;
        let mut move_dir: Option<[f32; 2]> = None;
        match self.doing {
            Doing::Fight(_) => {
                let (o, t0) = chosen.unwrap();
                let target = up(o.position, CHEST);
                let dist = len(sub(target, eye));
                let vel = self.tracks.get(&o.id).map_or([0.0; 3], |t| t.vel);
                let lead = scale(vel, aim.lead * dist / SHOT_SPEED);
                let reacted = p.tick.wrapping_sub(t0) >= aim.reaction;
                if reacted {
                    look = Some(add(target, lead));
                    // Fire while near the target, in bursts.
                    let off = angle_between(sub(target, eye), me.aim);
                    let cone = aim.fire_cone + (0.4 / dist.max(0.5)).min(0.3);
                    let burst_on = aim
                        .burst
                        .is_none_or(|(on, off)| p.tick.wrapping_sub(t0) % (on + off) < on);
                    if off < cone && dist < FIRE_RANGE && burst_on {
                        pad.right_trigger = 1.0;
                    }
                    if dist < MELEE_RANGE && p.tick % 20 < 2 {
                        pad.buttons.set(HostButton::East, true);
                    }
                    // A grenade now and then, in range, not too often.
                    if new_tick
                        && p.tick >= self.grenade_ready
                        && (GRENADE_RANGE.0..GRENADE_RANGE.1).contains(&dist)
                    {
                        let chance = aim.grenades_per_minute / (60.0 * TICKS) * dt as f32;
                        if self.uniform() < chance {
                            self.grenade_until = p.tick + 3;
                            self.grenade_ready = p.tick + GRENADE_COOLDOWN;
                        }
                    }
                    if p.tick < self.grenade_until {
                        pad.left_trigger = 1.0;
                        look = look.map(|l| up(l, 0.15 * dist));
                    }
                }
                // Until it reacts it stands, caught out. Then it strafes,
                // closing in from afar and backing off close up.
                if !reacted {
                    self.follower = None;
                } else {
                    if p.tick >= self.strafe.1 {
                        let flip = if self.uniform() < 0.5 { -1.0 } else { 1.0 };
                        let hold = 15 + (self.uniform() * 30.0) as u32;
                        self.strafe = (flip, p.tick + hold);
                    }
                    let to = sub(o.position, me.position);
                    let flat = (to[0] * to[0] + to[1] * to[1]).sqrt().max(1e-6);
                    let (fx, fy) = (to[0] / flat, to[1] / flat);
                    let side = [-fy * self.strafe.0, fx * self.strafe.0];
                    let fwd = if flat > 8.0 {
                        1.0
                    } else if flat < 2.5 {
                        -0.7
                    } else {
                        0.0
                    };
                    let mut d = [side[0] + fx * fwd, side[1] + fy * fwd];
                    if map.is_none() {
                        // Stage 1's walk: straight at it.
                        d = [fx, fy];
                    }
                    if let Some(m) = map
                        && m.nav
                            .locate(add(me.position, [d[0] * 0.6, d[1] * 0.6, 0.0]), 1.0)
                            .is_none()
                    {
                        // Off the mesh that way: the other side.
                        self.strafe.0 = -self.strafe.0;
                        d = [-side[0] + fx * fwd, -side[1] + fy * fwd];
                    }
                    move_dir = Some(d);
                    self.follower = None;
                }
            }
            Doing::Chase(at) => {
                move_dir = self.go(me.position, at, p.tick, map, &mut pad);
                if move_dir.is_none() {
                    // Got there and nobody is here.
                    self.last_seen = None;
                }
            }
            Doing::Roam => {
                if self.goal.is_none() || self.follower.as_ref().is_none_or(Follower::done) {
                    self.goal = self.roam_goal(me.position, map);
                    self.follower = None;
                }
                if let Some(g) = self.goal {
                    move_dir = self.go(me.position, g, p.tick, map, &mut pad);
                    if move_dir.is_none() {
                        self.goal = None;
                    }
                }
            }
            Doing::Idle => {}
        }

        // Turn: toward what it looks at, else along the way it walks.
        let want = match (look, move_dir) {
            (Some(l), _) => {
                let (y, pi) = angles(sub(l, eye));
                Some((y + self.err.0, pi + self.err.1))
            }
            (None, Some(d)) if d[0].abs() + d[1].abs() > 1e-3 => Some((d[1].atan2(d[0]), 0.0)),
            _ => None,
        };
        let (yaw, pitch) = angles(me.aim);
        if let Some((wy, wp)) = want {
            pad.right_stick = (
                (-wrap(wy - yaw) * aim.gain).clamp(-1.0, 1.0),
                ((wp - pitch) * aim.gain).clamp(-1.0, 1.0),
            );
        }
        if let Some(d) = move_dir {
            let (c, s) = (yaw.cos(), yaw.sin());
            let (fwd, right) = (d[0] * c + d[1] * s, d[0] * s - d[1] * c);
            let m = (fwd * fwd + right * right).sqrt();
            let k = if m > 1.0 { 1.0 / m } else { 1.0 };
            pad.left_stick = (right * k, fwd * k);
        }
        // An emote over a dumb death: stand, crouch in rhythm, nothing
        // else. No objective is live until stage 5.
        if let Some(crouch) = self
            .emote
            .step(me.position, !seen.is_empty(), false, p.tick)
        {
            pad = HostPadState::default();
            pad.buttons.set(CROUCH, crouch);
        }
        pad
    }

    /// Walks toward `goal` along the nav mesh (straight without one).
    /// `None` once there.
    fn go(
        &mut self,
        at: Vec3,
        goal: Vec3,
        tick: u32,
        map: Option<&MapData>,
        pad: &mut HostPadState,
    ) -> Option<[f32; 2]> {
        let Some(m) = map else {
            let d = [goal[0] - at[0], goal[1] - at[1]];
            let l = (d[0] * d[0] + d[1] * d[1]).sqrt();
            return (l > crate::nav::REACHED).then(|| [d[0] / l, d[1] / l]);
        };
        let stale = self
            .follower
            .as_ref()
            .and_then(Follower::goal)
            .is_none_or(|g| len(sub(g, goal)) > 1.0);
        if stale {
            self.follower = m
                .nav
                .path(at, goal)
                .map(|p| Follower::new(p, u64::from(tick)));
            self.follower.as_ref()?;
        }
        let f = self.follower.as_mut()?;
        let s = f.steer(at, u64::from(tick));
        if s.replan {
            let done = f.done();
            self.follower = None;
            return (!done).then_some([0.0, 0.0]);
        }
        if s.jump {
            pad.buttons.set(HostButton::South, true);
        }
        Some(s.dir)
    }

    /// Somewhere to wander to: the farthest of three polygons picked at
    /// random.
    fn roam_goal(&mut self, at: Vec3, map: Option<&MapData>) -> Option<Vec3> {
        let m = map?;
        let n = m.nav.polys.len();
        if n == 0 {
            return None;
        }
        let here = m.nav.locate(at, 1.0);
        let mut best: Option<Vec3> = None;
        for _ in 0..3 {
            let i = (self.next() % n as u64) as u32;
            let c = m.nav.polys[i as usize].center;
            let reachable = here.is_some_and(|h| h == i || m.nav.corridor(h, i).is_some());
            if reachable && best.is_none_or(|b| len(sub(c, at)) > len(sub(b, at))) {
                best = Some(c);
            }
        }
        best
    }
}

#[cfg(test)]
mod tests;
