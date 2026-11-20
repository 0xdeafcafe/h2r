//! Moving in a fight (docs/game/bots.md, section 3.2, "Moving in a
//! fight"): where the feet go while the hands aim. Pure but for its own
//! seeded random numbers: what the bot knows in ([`FightIn`]), a walk
//! direction and the jump and crouch buttons out ([`FightMove`]).
//!
//! - **Strafing** in a rhythm the skill sets ([`Style`]): Easy holds one
//!   way for long, predictable stretches and sometimes just stands;
//!   Legendary changes often and at random.
//! - **Range**: closes in from beyond the weapon's range, backs off inside
//!   it ([`preferred_range`], a seam for the weapons decision).
//! - **Hit reactions**: a shield drop is a hit. It may reverse the strafe
//!   at once (a juke) or (at range) crouch for a moment.
//! - **Retreat and reload**: backs away to a spot the enemy cannot see
//!   ([`cover`], a ray against the walls), else straight back; never
//!   stands still to reload.
//! - **Footing**: every step is checked on the nav mesh ahead: no strafing
//!   off a ledge or into a wall; the other way, or forward, or back.
//! - **Stuck in a fight**: pushing a direction without moving flips the
//!   strafe and jumps.

use crate::brain::{Me, Vec3};
use crate::cache::MapData;
use crate::mind::{CHEST, EYE, Skill};
use crate::stuck::around_bodies;

/// The range a bot fights at with no word from the weapons decision:
/// closes in beyond the second, backs off inside the first (world units).
pub const DEFAULT_RANGE: (f32, f32) = (2.5, 8.0);

/// SEAM for the weapons decision (the bots-fight lane): the range, near
/// and far, this bot wants to fight at with what it holds. A shotgun or a
/// sword wants it short, a sniper long. Until that is known, every weapon
/// gets [`DEFAULT_RANGE`].
pub fn preferred_range(_me: &Me) -> (f32, f32) {
    DEFAULT_RANGE
}

/// How a skill moves in a fight.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    /// Ticks one strafe direction is held, least and most.
    pub strafe: (u32, u32),
    /// The chance a new strafe is standing still instead.
    pub stand: f32,
    /// The chance, on a hit, to reverse the strafe at once.
    pub juke: f32,
    /// Chance to jump at a fresh strafe on a clear floor.
    pub jump: f32,
    /// The chance, on a hit beyond [`CROUCH_RANGE`], to crouch a moment.
    pub crouch: f32,
    /// How hard it backs off inside its range (0: it never does).
    pub keep: f32,
    /// Backs off to cover (out of the enemy's sight) rather than straight.
    pub cover: bool,
}

impl Style {
    pub(crate) fn with_behavior(skill: Skill, behavior: crate::decisions::Behavior) -> Style {
        let mut style = Self::of(skill);
        let hold = behavior.strafe_hold();
        style.strafe = (
            (style.strafe.0 as f32 * hold) as u32,
            (style.strafe.1 as f32 * hold) as u32,
        );
        style.stand = (style.stand + behavior.stand_bonus()).clamp(0.0, 0.55);
        style.keep = (style.keep * behavior.spacing()).min(1.0);
        style
    }

    pub(crate) fn apply_weights(&mut self, weights: crate::name_profile::Weights) {
        let interval = 1.25 - 0.5 * weights.strafe;
        self.strafe = (
            (self.strafe.0 as f32 * interval).round() as u32,
            (self.strafe.1 as f32 * interval).round() as u32,
        );
        self.stand *= 1.0 - 0.35 * weights.strafe;
        self.juke += 0.3 * weights.strafe * (1.0 - self.juke);
        self.jump = weights.jump * 0.35;
    }

    pub fn of(skill: Skill) -> Style {
        match skill {
            Skill::Easy => Style {
                strafe: (40, 80),
                stand: 0.35,
                juke: 0.0,
                jump: 0.0,
                crouch: 0.0,
                keep: 0.0,
                cover: false,
            },
            Skill::Normal => Style {
                strafe: (20, 50),
                stand: 0.15,
                juke: 0.2,
                jump: 0.0,
                crouch: 0.0,
                keep: 0.6,
                cover: true,
            },
            Skill::Heroic => Style {
                strafe: (12, 35),
                stand: 0.05,
                juke: 0.5,
                jump: 0.0,
                crouch: 0.1,
                keep: 0.85,
                cover: true,
            },
            Skill::Legendary => Style {
                strafe: (8, 26),
                stand: 0.0,
                juke: 0.7,
                jump: 0.0,
                crouch: 0.15,
                keep: 1.0,
                cover: true,
            },
        }
    }
}

/// A shield drop of more than this in a tick is a hit.
pub const HIT: f32 = 0.03;
/// At most one hit reaction this often (ticks).
pub const REACT_COOLDOWN: u32 = 20;
/// Jump is held this many ticks...
pub const JUMP_TICKS: u32 = 3;
/// ...and not again for this long.
pub const JUMP_COOLDOWN: u32 = 45;
/// Crouches on a hit only this far from the enemy...
pub const CROUCH_RANGE: f32 = 8.0;
/// ...for this long.
pub const CROUCH_TICKS: u32 = 12;
/// Pushing a way for this long without moving [`FIGHT_PROGRESS`] is stuck.
pub const FIGHT_STUCK: u32 = 15;
pub const FIGHT_PROGRESS: f32 = 0.15;
/// A cover spot is looked for again this often.
pub const COVER_TICKS: u32 = 15;
/// Cover is looked for this far away (world units).
pub const COVER_RADII: [f32; 2] = [1.5, 3.0];
/// A step is checked this far ahead...
pub const PROBES: [f32; 2] = [0.5, 1.0];
/// ...and refused down a drop deeper than this, or up one higher.
pub const LEDGE: f32 = 0.5;
/// Walls are checked at this height over the floor.
pub const KNEE: f32 = 0.35;

/// What the feet need to know.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FightIn {
    pub tick: u32,
    pub skill: Skill,
    /// The bot's and the enemy's positions (unit origins).
    pub at: Vec3,
    pub foe: Vec3,
    /// The bot's shield now: a drop since last tick is a hit.
    pub shield: f32,
    /// Run straight in (nothing to shoot: melee).
    pub rush: bool,
    /// Back off (shields down).
    pub retreat: bool,
    /// Reloading or switching: back off a little, keep moving.
    pub reloading: bool,
    /// The range to keep, near and far ([`preferred_range`]).
    pub range: (f32, f32),
}

/// The feet's answer.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FightMove {
    /// The way to walk, world XY, length up to 1 (zero: stand).
    pub walk: [f32; 2],
    pub jump: bool,
    pub crouch: bool,
}

/// One bot's fight footwork.
#[derive(Clone, Debug)]
pub struct Moves {
    rng: u64,
    /// The strafe: +1 left of the enemy line, -1 right, 0 standing; and
    /// the tick it changes.
    pub side: f32,
    /// Deflection for this strafe, set once per rhythm change.
    pace: f32,
    until: u32,
    jump_until: u32,
    jump_ready: u32,
    crouch_until: u32,
    react_ready: u32,
    shield: Option<f32>,
    anchor: Option<(Vec3, u32)>,
    /// The cover spot and the tick it is looked for again.
    pub cover: Option<(Vec3, u32)>,
    /// Jukes so far (for tests and logs).
    pub jukes: u32,
}

fn dist_xy(a: Vec3, b: Vec3) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}
fn norm2(d: [f32; 2]) -> f32 {
    (d[0] * d[0] + d[1] * d[1]).sqrt()
}

impl Moves {
    pub fn new(seed: u64) -> Self {
        Moves {
            rng: (seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x6d6f_7665_7321) | 1,
            side: 1.0,
            pace: 0.8,
            until: 0,
            jump_until: 0,
            jump_ready: 0,
            crouch_until: 0,
            react_ready: 0,
            shield: None,
            anchor: None,
            cover: None,
            jukes: 0,
        }
    }

    fn uniform(&mut self) -> f32 {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        (self.rng.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Out of the fight: forget the progress and the cover; the shield is
    /// read afresh next time.
    pub fn calm(&mut self) {
        self.anchor = None;
        self.cover = None;
        self.shield = None;
    }

    pub(crate) fn release(&mut self, tick: u32) {
        self.calm();
        self.until = tick;
        self.jump_until = tick;
        self.jump_ready = tick;
        self.crouch_until = tick;
        self.react_ready = tick;
    }

    /// This tick's footwork. Without a map: straight at the enemy (or
    /// straight back when retreating), as stage 1 did.
    pub fn fight(&mut self, i: &FightIn, map: Option<&MapData>, bodies: &[Vec3]) -> FightMove {
        self.fight_with_style(i, map, bodies, Style::of(i.skill))
    }

    pub(crate) fn fight_with_style(
        &mut self,
        i: &FightIn,
        map: Option<&MapData>,
        bodies: &[Vec3],
        st: Style,
    ) -> FightMove {
        let t = i.tick;
        let to = [i.foe[0] - i.at[0], i.foe[1] - i.at[1]];
        let flat = norm2(to).max(1e-6);
        let f = [to[0] / flat, to[1] / flat];
        let mut out = FightMove::default();

        // A hit: juke or crouch, at the skill's odds.
        let hurt = self.shield.is_some_and(|s| i.shield < s - HIT);
        self.shield = Some(i.shield);
        if hurt && reached(t, self.react_ready) {
            self.react_ready = t.wrapping_add(REACT_COOLDOWN);
            if self.uniform() < st.juke {
                self.side = if self.side == 0.0 { 1.0 } else { -self.side };
                self.until = t.wrapping_add(st.strafe.0);
                self.jukes += 1;
            }
            if !i.rush && flat > CROUCH_RANGE && self.uniform() < st.crouch {
                self.crouch_until = t.wrapping_add(CROUCH_TICKS);
            }
        }

        let Some(m) = map else {
            let back = i.retreat && !i.rush;
            let d = if back { [-f[0], -f[1]] } else { f };
            out.walk = around_bodies(i.at, d, bodies);
            out.jump = !reached(t, self.jump_until);
            out.crouch = !out.jump && !reached(t, self.crouch_until);
            return out;
        };

        let mut new_strafe = false;
        let mut d = if i.rush {
            f
        } else {
            // A new strafe now and then: a side, or (for some) standing.
            if reached(t, self.until) {
                new_strafe = true;
                let stand = self.uniform() < st.stand;
                let flip = self.uniform() < 0.5;
                self.side = if stand {
                    0.0
                } else if flip {
                    -1.0
                } else {
                    1.0
                };
                let span = st.strafe.1.saturating_sub(st.strafe.0) as f32;
                self.until = t.wrapping_add(st.strafe.0 + (self.uniform() * span) as u32);
                self.pace = 0.62 + 0.36 * self.uniform();
            }
            let side = [-f[1] * self.side * self.pace, f[0] * self.side * self.pace];
            let (near, far) = i.range;
            let fwd = if i.retreat {
                -0.8
            } else if i.reloading {
                // Not closing in with an empty gun.
                if flat < far { -0.5 } else { 0.0 }
            } else if flat > far {
                1.0
            } else if flat < near {
                -0.7 * st.keep
            } else {
                0.0
            };
            let mut d = [side[0] + f[0] * fwd, side[1] + f[1] * fwd];
            // Backing off: to where the enemy cannot see, if there is one.
            if (i.retreat || i.reloading) && st.cover {
                if self.cover.is_none_or(|(_, again)| reached(t, again)) {
                    let spot = cover(m, i.at, i.foe);
                    self.cover = spot.map(|c| (c, t.wrapping_add(COVER_TICKS)));
                }
                if let Some((c, _)) = self.cover {
                    let dc = [c[0] - i.at[0], c[1] - i.at[1]];
                    let l = norm2(dc);
                    d = if l > 0.3 {
                        [dc[0] / l, dc[1] / l]
                    } else {
                        [0.0, 0.0]
                    };
                }
            } else {
                self.cover = None;
            }
            d
        };

        // Footing: the way it wants, else the other side, else straight
        // forward or back, else stand.
        if norm2(d) > 1e-3 && !standable(m, i.at, d) {
            let side = [-f[1] * self.side * self.pace, f[0] * self.side * self.pace];
            let along = d[0] * f[0] + d[1] * f[1];
            let other = [-side[0] + f[0] * along, -side[1] + f[1] * along];
            let fwd = if along.abs() > 0.1 {
                [f[0] * along.signum(), f[1] * along.signum()]
            } else {
                f
            };
            let back = [-fwd[0], -fwd[1]];
            if !i.rush && norm2(side) > 0.5 && standable(m, i.at, other) {
                self.side = -self.side;
                d = other;
            } else if standable(m, i.at, fwd) {
                d = fwd;
            } else if standable(m, i.at, back) {
                d = back;
            } else {
                d = [0.0, 0.0];
            }
        }

        // A discretionary combat hop uses the same jump recovery as
        // unsticking. Only try on a moving, clear strafe outside cover.
        if new_strafe && !i.retreat && !i.reloading && self.side != 0.0
            && reached(t, self.jump_ready) && st.jump > 0.0 && self.uniform() < st.jump
            && standable(m, i.at, d)
            && m.walls.clear(
                [i.at[0], i.at[1], i.at[2] + EYE],
                [i.at[0], i.at[1], i.at[2] + EYE + 0.7],
            )
        {
            self.jump_until = t.wrapping_add(JUMP_TICKS);
            self.jump_ready = t.wrapping_add(JUMP_COOLDOWN);
        }
        // Stuck in the fight: pushing without moving flips and jumps.
        if norm2(d) > 0.3 {
            let (anchor, since) = *self.anchor.get_or_insert((i.at, t));
            if dist_xy(i.at, anchor) >= FIGHT_PROGRESS {
                self.anchor = Some((i.at, t));
            } else if t.wrapping_sub(since) >= FIGHT_STUCK {
                self.anchor = Some((i.at, t));
                self.side = if self.side == 0.0 { 1.0 } else { -self.side };
                self.until = t.wrapping_add(st.strafe.0);
                if reached(t, self.jump_ready) {
                    self.jump_until = t.wrapping_add(JUMP_TICKS);
                    self.jump_ready = t.wrapping_add(JUMP_COOLDOWN);
                }
            }
        } else {
            self.anchor = None;
        }

        let l = norm2(d);
        if l > 1.0 {
            d = [d[0] / l, d[1] / l];
        }
        out.walk = around_bodies(i.at, d, bodies);
        out.jump = !reached(t, self.jump_until);
        out.crouch = !out.jump && !reached(t, self.crouch_until);
        out
    }
}

/// Whether `tick` is at or past `until`, across the counter's wrap.
fn reached(tick: u32, until: u32) -> bool {
    tick.wrapping_sub(until) < u32::MAX / 2
}

/// The floor under a biped at `at` (its height), if on the mesh.
fn floor(m: &MapData, at: Vec3) -> Option<f32> {
    m.nav
        .locate(at, 1.0)
        .map(|i| m.nav.polys[i as usize].z_at(at))
}

/// The floor at `q`, a step from a floor at `from_z`: within [`LEDGE`] up
/// or down, else `None`.
fn step_floor(m: &MapData, q: [f32; 2], from_z: f32) -> Option<f32> {
    let p = [q[0], q[1], from_z + LEDGE - 0.1];
    m.nav
        .locate(p, 2.0 * LEDGE - 0.1)
        .map(|i| m.nav.polys[i as usize].z_at(p))
}

/// Whether a step from `at` along `d` stays on the floor: the mesh goes on
/// [`PROBES`] ahead with no drop or rise past [`LEDGE`], and no wall
/// stands in the way at knee height. Off the mesh (no floor found under
/// it), only that the point ahead is on it.
pub fn standable(m: &MapData, at: Vec3, d: [f32; 2]) -> bool {
    step_clear(m, at, d, &PROBES)
}

/// Shorter check for a bend just before a cleared corner. Long probes
/// would keep the hand straight until it had already gone past the turn.
pub(crate) fn bend_clear(m: &MapData, at: Vec3, d: [f32; 2]) -> bool {
    step_clear(m, at, d, &[0.22, 0.45])
}

/// Check the standing capsule against overhead collision at the next step.
pub(crate) fn headroom(m: &MapData, at: Vec3, dir: [f32; 2]) -> bool {
    let len = norm2(dir);
    if len < 1e-3 {
        return true;
    }
    [0.14, 0.32].into_iter().all(|distance| {
        let q = [
            at[0] + dir[0] / len * distance,
            at[1] + dir[1] / len * distance,
        ];
        let Some(z) = step_floor(m, q, at[2]) else {
            return true;
        };
        crate::nav::standing_clear(&m.walls, [q[0], q[1], z])
    })
}

fn step_clear(m: &MapData, at: Vec3, d: [f32; 2], probes: &[f32]) -> bool {
    let l = norm2(d);
    if l < 1e-3 {
        return true;
    }
    let u = [d[0] / l, d[1] / l];
    let Some(here) = floor(m, at) else {
        return m
            .nav
            .locate([at[0] + u[0] * 0.6, at[1] + u[1] * 0.6, at[2]], 1.0)
            .is_some();
    };
    if !headroom(m, at, u) {
        return false;
    }
    let mut prev = here;
    for &s in probes {
        let q = [at[0] + u[0] * s, at[1] + u[1] * s];
        let Some(z) = step_floor(m, q, prev) else {
            return false;
        };
        if !m
            .walls
            .clear([at[0], at[1], here + KNEE], [q[0], q[1], z + KNEE])
        {
            return false;
        }
        prev = z;
    }
    true
}

/// A spot within [`COVER_RADII`] the bot can walk to straight, and the
/// enemy at `foe` cannot see (its eye to the bot's chest there, through
/// the walls): the nearest such, favouring ones away from the enemy.
pub fn cover(m: &MapData, at: Vec3, foe: Vec3) -> Option<Vec3> {
    let here = floor(m, at)?;
    let lift = at[2] - here;
    let eye = [foe[0], foe[1], foe[2] + EYE];
    let away = {
        let a = [at[0] - foe[0], at[1] - foe[1]];
        let l = norm2(a).max(1e-6);
        [a[0] / l, a[1] / l]
    };
    let mut best: Option<(f32, Vec3)> = None;
    for r in COVER_RADII {
        for k in 0..8 {
            let ang = k as f32 * std::f32::consts::FRAC_PI_4;
            let dir = [ang.cos(), ang.sin()];
            if !standable(m, at, dir) {
                continue;
            }
            let q = [at[0] + dir[0] * r, at[1] + dir[1] * r];
            let Some(z) = step_floor(m, q, here) else {
                continue;
            };
            if !m
                .walls
                .clear([at[0], at[1], here + KNEE], [q[0], q[1], z + KNEE])
            {
                continue;
            }
            let chest = [q[0], q[1], z + lift + CHEST];
            if m.walls.clear(eye, chest) {
                continue;
            }
            let score = r - 0.5 * (dir[0] * away[0] + dir[1] * away[1]);
            if best.is_none_or(|(s, _)| score < s) {
                best = Some((score, [q[0], q[1], z + lift]));
            }
        }
    }
    best.map(|(_, c)| c)
}

#[cfg(test)]
mod tests;
