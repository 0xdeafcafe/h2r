//! Grenades (docs/game/bots.md, "Built: weapon skill"): the throw flown
//! against the map's collision ([`fly`]), the aim that lands it where it
//! should go off ([`solve`]), and when and what to throw ([`Thrower`]).
//!
//! A thrown grenade leaves the eye along the aim at its projectile's
//! speed, falls under gravity, and:
//!
//! - a **frag** bounces (some of its speed into a surface comes back,
//!   some of its speed along it is lost) and goes off [`Ballistics::fuse`]
//!   after it first touches anything;
//! - a **plasma** grenade sticks to the first thing it touches, a player
//!   included, and goes off its fuse later.
//!
//! Halo 2 has no cooking: the fuse starts at the first touch, whatever the
//! bot does. None of the numbers is read from the game yet: the grenade's
//! projectile tag is reached through the globals' grenades block, which is
//! not traced, and the gravity the projectile update applies (`0x000f9726`,
//! docs/live/live-telemetry.md) comes from a field not traced either.
//! [`FRAG`] and [`PLASMA`] are assumed values, named as such, for a run to
//! check (section "What needs a game run").
//!
//! Pure and deterministic: the thrower's chance comes from its own seeded
//! generator, and time is the game's tick (30 a second).

use crate::brain::{Seen, Vec3};
use crate::los::Walls;
use crate::mind::Skill;

/// The two grenade types, in the unit's order (`+0x23e`: frag, plasma).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Frag,
    Plasma,
}

impl Kind {
    /// The index into the unit's grenade counts and its selection
    /// (`+0x23c`).
    pub fn index(self) -> usize {
        match self {
            Kind::Frag => 0,
            Kind::Plasma => 1,
        }
    }
    pub fn from_index(i: i8) -> Option<Kind> {
        match i {
            0 => Some(Kind::Frag),
            1 => Some(Kind::Plasma),
            _ => None,
        }
    }
    pub fn ballistics(self) -> Ballistics {
        match self {
            Kind::Frag => FRAG,
            Kind::Plasma => PLASMA,
        }
    }
}

/// How a grenade flies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ballistics {
    /// Leaves the hand at this speed (world units a second).
    pub speed: f32,
    /// Falls this fast (world units a second squared).
    pub gravity: f32,
    /// Of its speed into a surface, this much comes back out.
    pub restitution: f32,
    /// Of its speed along a surface, this much is lost at each touch.
    pub friction: f32,
    /// Goes off this long (seconds) after its first touch.
    pub fuse: f32,
    /// Sticks to what it touches (plasma) rather than bouncing.
    pub sticks: bool,
    /// Kills or strips shields this close to where it goes off (world
    /// units).
    pub radius: f32,
}

/// Assumed: the gravity of Halo's world, 9.8 m/s squared in world units
/// of about 3 m (Halo: Combat Evolved's globals give about 3.2). Not read
/// from Halo 2.
pub const ASSUMED_GRAVITY: f32 = 3.2;

/// A frag grenade, assumed (not read): about 18 m of flat throw at 30
/// degrees, a lively bounce, a 1 s fuse from the first touch (the 1.1
/// update shortened it; docs/runtime/title-updates.md).
pub const FRAG: Ballistics = Ballistics {
    speed: 6.0,
    gravity: ASSUMED_GRAVITY,
    restitution: 0.35,
    friction: 0.3,
    fuse: 1.0,
    sticks: false,
    radius: 2.5,
};

/// A plasma grenade, assumed (not read): a little slower, sticks, goes off
/// a moment after.
pub const PLASMA: Ballistics = Ballistics {
    speed: 5.5,
    gravity: ASSUMED_GRAVITY,
    restitution: 0.0,
    friction: 1.0,
    fuse: 1.2,
    sticks: true,
    radius: 2.2,
};

// ---- the flight ----

/// Something solid a grenade can touch.
pub trait Solid {
    /// The first thing on the way from `a` to `b`: how far along (0 to 1)
    /// and the surface's unit normal, facing back toward `a`.
    fn hit(&self, a: Vec3, b: Vec3) -> Option<(f32, Vec3)>;
}

impl Solid for Walls {
    fn hit(&self, a: Vec3, b: Vec3) -> Option<(f32, Vec3)> {
        Walls::hit(self, a, b)
    }
}

/// A flat, endless floor at this height: the world with no map data.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Floor(pub f32);

impl Solid for Floor {
    fn hit(&self, a: Vec3, b: Vec3) -> Option<(f32, Vec3)> {
        (a[2] >= self.0 && b[2] < self.0)
            .then(|| ((a[2] - self.0) / (a[2] - b[2]), [0.0, 0.0, 1.0]))
    }
}

/// A player a plasma grenade can stick to: where its feet are and how fast
/// it moves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Body {
    pub at: Vec3,
    pub vel: Vec3,
}

/// A player's body, for sticking: an upright line from this over its
/// feet to [`BODY_TOP`], this thick.
pub const BODY_BOTTOM: f32 = 0.1;
pub const BODY_TOP: f32 = 0.65;
pub const BODY_RADIUS: f32 = 0.25;
/// Flown in steps of this (seconds): two to a tick.
pub const STEP: f32 = 1.0 / 60.0;
/// Given up on after this long in the air (seconds): fell off the map.
pub const MAX_FLIGHT: f32 = 5.0;
/// Slower than this after a touch on a floor, it lies still.
pub const REST_SPEED: f32 = 0.4;
/// A surface whose normal points this much up is a floor.
pub const FLOOR_NORMAL: f32 = 0.7;

/// Where a grenade goes and what it does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flight {
    /// Where it goes off.
    pub blast: Vec3,
    /// When (seconds after the throw).
    pub time: f32,
    pub bounces: u8,
    /// The body (its index) a plasma grenade stuck to.
    pub stuck: Option<usize>,
    /// Where it first touched something.
    pub first: Option<Vec3>,
    /// It went off on a surface or a body; false: it fell out of the world
    /// (or never touched anything).
    pub landed: bool,
}

fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn len(a: Vec3) -> f32 {
    dot(a, a).sqrt()
}
fn unit(a: Vec3) -> Vec3 {
    scale(a, 1.0 / len(a).max(1e-9))
}

/// The direction of `yaw` (counter-clockwise from +X) and `pitch` (up).
pub fn direction(yaw: f32, pitch: f32) -> Vec3 {
    [
        pitch.cos() * yaw.cos(),
        pitch.cos() * yaw.sin(),
        pitch.sin(),
    ]
}

/// The body (its index) the segment `p` to `q` passes through, at time `t`.
fn touches(bodies: &[Body], p: Vec3, q: Vec3, t: f32) -> Option<usize> {
    bodies.iter().position(|b| {
        let at = add(b.at, scale(b.vel, t));
        // The closest point of the segment to the body's upright line:
        // flat distance under the radius, height within the body.
        let d = sub(q, p);
        let flat = d[0] * d[0] + d[1] * d[1];
        let s = if flat < 1e-12 {
            0.0
        } else {
            (((at[0] - p[0]) * d[0] + (at[1] - p[1]) * d[1]) / flat).clamp(0.0, 1.0)
        };
        let c = add(p, scale(d, s));
        let h = ((c[0] - at[0]).powi(2) + (c[1] - at[1]).powi(2)).sqrt();
        h < BODY_RADIUS && (at[2] + BODY_BOTTOM..=at[2] + BODY_TOP).contains(&c[2])
    })
}

/// Flies a grenade thrown from `from` along `dir` against `world`, sticking
/// (plasma) to any of `bodies` it passes through.
pub fn fly(world: &dyn Solid, from: Vec3, dir: Vec3, b: &Ballistics, bodies: &[Body]) -> Flight {
    let mut p = from;
    let mut v = scale(unit(dir), b.speed);
    let mut t = 0.0f32;
    let mut fuse: Option<f32> = None;
    let mut bounces = 0u8;
    let mut first: Option<Vec3> = None;
    let mut resting = false;
    let steps = (MAX_FLIGHT / STEP) as u32;
    for _ in 0..steps {
        if let Some(f) = fuse
            && t >= f
        {
            return Flight {
                blast: p,
                time: t,
                bounces,
                stuck: None,
                first,
                landed: true,
            };
        }
        t += STEP;
        if resting {
            continue;
        }
        let v1 = [v[0], v[1], v[2] - b.gravity * STEP];
        let q = add(p, scale(add(v, v1), 0.5 * STEP));
        if b.sticks
            && let Some(i) = touches(bodies, p, q, t)
        {
            let when = t + b.fuse;
            let body = bodies[i];
            return Flight {
                blast: add(add(body.at, scale(body.vel, when)), [0.0, 0.0, 0.4]),
                time: when,
                bounces,
                stuck: Some(i),
                first: Some(q),
                landed: true,
            };
        }
        let Some((s, n)) = world.hit(p, q) else {
            p = q;
            v = v1;
            continue;
        };
        let c = add(p, scale(sub(q, p), s));
        first.get_or_insert(c);
        if b.sticks {
            return Flight {
                blast: c,
                time: t + b.fuse,
                bounces,
                stuck: None,
                first,
                landed: true,
            };
        }
        fuse.get_or_insert(t + b.fuse);
        bounces = bounces.saturating_add(1);
        let into = scale(n, dot(v1, n));
        let along = sub(v1, into);
        v = sub(scale(along, 1.0 - b.friction), scale(into, b.restitution));
        p = add(c, scale(n, 0.02));
        if len(v) < REST_SPEED && n[2] > FLOOR_NORMAL {
            resting = true;
            v = [0.0; 3];
        }
    }
    Flight {
        blast: p,
        time: t,
        bounces,
        stuck: None,
        first,
        landed: false,
    }
}

// ---- the aim ----

/// What [`solve`] is asked.
#[derive(Clone, Copy, Debug)]
pub struct Ask<'a> {
    /// The hand: where the grenade leaves from (the eye).
    pub from: Vec3,
    /// Where it should go off: the target's feet.
    pub at: Vec3,
    /// The target's velocity...
    pub vel: Vec3,
    /// ...and how much of it to lead (0 none, 1 all).
    pub lead: f32,
    pub ballistics: Ballistics,
    /// Players a plasma grenade may stick to; `stick` the target's index.
    pub bodies: &'a [Body],
    pub stick: Option<usize>,
    /// Tries throws off to the side, to bank off walls and roll round
    /// corners, when no straight one lands.
    pub banks: bool,
}

/// The throw found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Toss {
    pub dir: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub flight: Flight,
    /// How far from where it should go off it goes off (world units); 0
    /// stuck to the target.
    pub miss: f32,
    /// Thrown off to the side of the target (a bank or a roll).
    pub bank: bool,
}

/// Pitches tried: from, to, step (radians).
pub const PITCH_SWEEP: (f32, f32, f32) = (
    -20.0 * std::f32::consts::PI / 180.0,
    70.0 * std::f32::consts::PI / 180.0,
    5.0 * std::f32::consts::PI / 180.0,
);
/// Then the best is refined in steps of this (radians), this many each
/// side.
pub const PITCH_FINE: (f32, i32) = (1.0 * std::f32::consts::PI / 180.0, 4);
/// Off-target yaws tried for banks and rolls (degrees).
pub const BANKS: [f32; 10] = [
    8.0, -8.0, 16.0, -16.0, 25.0, -25.0, 40.0, -40.0, 60.0, -60.0,
];
/// A throw is taken only if it goes off this close to where it should
/// (world units).
pub const ACCEPT: f32 = 1.0;
/// A bank is preferred to a straight throw only if it is better by this.
pub const BANK_COST: f32 = 0.3;

fn miss(ask: &Ask<'_>, f: &Flight) -> f32 {
    if !f.landed {
        return f32::INFINITY;
    }
    if ask.stick.is_some() && f.stuck == ask.stick {
        return 0.0;
    }
    let want = add(ask.at, scale(ask.vel, ask.lead * f.time));
    len(sub(f.blast, want))
}

/// The throw at `yaw`, its best pitch: swept, then refined.
fn best_pitch(world: &dyn Solid, ask: &Ask<'_>, yaw: f32, bank: bool) -> Option<Toss> {
    let throw = |pitch: f32| {
        let dir = direction(yaw, pitch);
        let flight = fly(world, ask.from, dir, &ask.ballistics, ask.bodies);
        Toss {
            dir,
            yaw,
            pitch,
            flight,
            miss: miss(ask, &flight),
            bank,
        }
    };
    let better = |a: Option<Toss>, b: Toss| match a {
        Some(a) if a.miss <= b.miss => Some(a),
        _ => Some(b),
    };
    let (lo, hi, step) = PITCH_SWEEP;
    let mut best: Option<Toss> = None;
    let n = ((hi - lo) / step).round() as i32;
    for i in 0..=n {
        best = better(best, throw(lo + step * i as f32));
    }
    let coarse = best?;
    for i in -PITCH_FINE.1..=PITCH_FINE.1 {
        if i != 0 {
            best = better(best, throw(coarse.pitch + PITCH_FINE.0 * i as f32));
        }
    }
    best.filter(|t| t.miss.is_finite())
}

/// The throw that goes off nearest where it should (`ask.at`, led): a
/// straight one first; if none goes off within [`ACCEPT`] and banks are
/// allowed, one thrown off to the side that bounces off a wall or rolls
/// round a corner. `None`: no throw lands it (the arc is blocked).
pub fn solve(world: &dyn Solid, ask: &Ask<'_>) -> Option<Toss> {
    // Straight at where the target will be: led by a guess at the flight
    // time, then by the flight time of the best throw at that.
    let yaw_at = |time: f32| {
        let to = sub(add(ask.at, scale(ask.vel, ask.lead * time)), ask.from);
        to[1].atan2(to[0])
    };
    let to = sub(ask.at, ask.from);
    let guess = (to[0] * to[0] + to[1] * to[1]).sqrt() / (ask.ballistics.speed * 0.85).max(0.1);
    let mut yaw0 = yaw_at(guess);
    let mut straight = best_pitch(world, ask, yaw0, false);
    if ask.lead > 0.0
        && len(ask.vel) > 0.05
        && let Some(s) = straight
    {
        let yaw1 = yaw_at(s.flight.time);
        if let Some(t) = best_pitch(world, ask, yaw1, false)
            && t.miss < s.miss
        {
            straight = Some(t);
            yaw0 = yaw1;
        }
    }
    if straight.is_some_and(|t| t.miss <= ACCEPT) || !ask.banks {
        return straight.filter(|t| t.miss <= ACCEPT);
    }
    let mut best = straight;
    for deg in BANKS {
        let yaw = yaw0 + deg.to_radians();
        if let Some(t) = best_pitch(world, ask, yaw, true)
            && best.is_none_or(|b| t.miss + BANK_COST < b.miss)
        {
            best = Some(t);
        }
    }
    best.filter(|t| t.miss <= ACCEPT)
}

/// A throw is safe: it goes off no nearer than `self_safe` to the
/// thrower's feet `me`, and (`careful`) not within `friendly` of a
/// teammate.
pub fn safe(blast: Vec3, me: Vec3, mates: &[Vec3], careful: bool) -> bool {
    len(sub(blast, me)) >= SELF_SAFE
        && (!careful || mates.iter().all(|m| len(sub(blast, *m)) >= FRIENDLY))
}

/// Never a grenade that goes off this close to the thrower (world units).
pub const SELF_SAFE: f32 = 3.0;
/// Nor, careful, this close to a teammate.
pub const FRIENDLY: f32 = 3.0;

// ---- when and what ----

/// The game's left trigger throws; with a grenade type not held, the
/// switch grenades button (Black on the default layout) changes it.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Hand {
    /// A solved throw is waiting for the off hand to be released.
    pub drop_left: bool,
    /// Look at this point (far along the throw) instead.
    pub look: Option<Vec3>,
    /// Pull the left trigger.
    pub throw: bool,
    /// Press switch grenades.
    pub swap: bool,
}

/// What skill changes in grenades.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GrenadeSkill {
    pub per_minute: f32,
    /// Banks off walls and rolls round corners.
    pub banks: bool,
    /// Throws a plasma to stick a close target.
    pub stick: bool,
    /// How much of a moving target's lead it gives.
    pub lead: f32,
    /// The throw's aim is off by about this (radians, one standard
    /// deviation).
    pub noise: f32,
    /// Lets go once its aim is this close to the throw (radians).
    pub tolerance: f32,
    /// Holds a grenade that would go off near a teammate.
    pub careful: bool,
    /// Throws more often before pushing in with a short gun.
    pub push: bool,
}

impl GrenadeSkill {
    pub fn of(skill: Skill) -> GrenadeSkill {
        let per_minute = skill.aim().grenades_per_minute;
        let deg = |d: f32| d.to_radians();
        match skill {
            Skill::Easy => GrenadeSkill {
                per_minute,
                banks: false,
                stick: false,
                lead: 0.0,
                noise: deg(6.0),
                tolerance: deg(8.0),
                careful: false,
                push: false,
            },
            Skill::Normal => GrenadeSkill {
                per_minute,
                banks: false,
                stick: true,
                lead: 0.5,
                noise: deg(3.0),
                tolerance: deg(5.0),
                careful: true,
                push: true,
            },
            Skill::Heroic => GrenadeSkill {
                per_minute,
                banks: true,
                stick: true,
                lead: 0.85,
                noise: deg(1.5),
                tolerance: deg(4.0),
                careful: true,
                push: true,
            },
            Skill::Legendary => GrenadeSkill {
                per_minute,
                banks: true,
                stick: true,
                lead: 1.0,
                noise: deg(0.8),
                tolerance: deg(3.0),
                careful: true,
                push: true,
            },
        }
    }
}

/// Who or where a grenade is for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mark {
    /// The target's feet, or where it went out of view.
    pub at: Vec3,
    pub vel: Vec3,
    /// In view now (a plasma can stick it); not: where it was last seen.
    pub seen: bool,
}

/// What the mind gives [`Thrower::step`] this tick.
#[derive(Clone, Copy, Debug)]
pub struct Moment<'a> {
    pub tick: u32,
    /// Ticks since the last step (0 on a repeated frame: no new roll).
    pub dt: u32,
    pub eye: Vec3,
    pub feet: Vec3,
    pub aim: Vec3,
    /// Whom to throw at, if anyone.
    pub mark: Option<Mark>,
    /// Other enemies in view (for groups).
    pub enemies: &'a [Seen],
    /// Teammates' feet.
    pub mates: &'a [Vec3],
    /// Grenades carried (frag, plasma); `None` not read (frags assumed).
    pub grenades: Option<[i8; 2]>,
    /// The type selected, if read.
    pub selected: Option<Kind>,
    /// Cannot throw now: dual wielding, carrying, no hands.
    pub busy: bool,
    /// Closing in with a short gun: a grenade first.
    pub pushing: bool,
}

/// Throws at targets this far (world units).
pub const RANGE: (f32, f32) = (3.0, 12.0);
/// Sticks a plasma on a target in view this close.
pub const STICK_RANGE: f32 = 8.0;
/// Another enemy this close to the target makes a group...
pub const GROUP: f32 = 2.5;
/// ...worth this many times the rate.
pub const GROUP_RATE: f32 = 4.0;
/// Where a target went out of view: this many times the rate.
pub const UNSEEN_RATE: f32 = 2.0;
/// Before pushing in with a short gun: this many times the rate.
pub const PUSH_RATE: f32 = 3.0;
/// Ticks between throws at least.
pub const COOLDOWN: u32 = 240;
/// With no safe arc, tries again after this long (ticks).
pub const RETRY: u32 = 20;
/// Turns to the throw for this long at most (ticks), then gives it up.
pub const AIM_WAIT: u32 = 30;
/// The left trigger is held this long for a throw.
pub const PRESS: u32 = 3;
/// After pressing switch grenades, waits this long for the change.
pub const SWAP_WAIT: u32 = 8;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Plan {
    dir: Vec3,
    kind: Kind,
    since: u32,
    press_until: Option<u32>,
    swapped: Option<u32>,
}

/// The grenade hand, kept by the mind.
#[derive(Clone, Debug)]
pub struct Thrower {
    rng: u64,
    ready: u32,
    plan: Option<Plan>,
    /// Throws made (for tests and the log).
    pub throws: u32,
    /// The last throw solved: for tests and the agent API.
    pub last: Option<Toss>,
}

impl Default for Thrower {
    fn default() -> Self {
        Thrower::new(0x9e37)
    }
}

impl Thrower {
    pub fn new(seed: u64) -> Thrower {
        Thrower {
            rng: seed | 1,
            ready: 0,
            plan: None,
            throws: 0,
            last: None,
        }
    }

    fn next(&mut self) -> u64 {
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

    /// Forgets a throw under way (the bot died).
    pub fn reset(&mut self) {
        self.plan = None;
    }

    /// A throw is being aimed or made.
    pub fn busy(&self) -> bool {
        self.plan.is_some()
    }

    /// This tick: aim and make a throw under way, or roll for a new one at
    /// `m.mark` and solve it against `world`.
    pub fn step(&mut self, m: &Moment<'_>, world: &dyn Solid, skill: Skill) -> Hand {
        self.step_with_left(m, world, skill, false, 1.0)
    }

    pub(crate) fn step_with_left(
        &mut self,
        m: &Moment<'_>,
        world: &dyn Solid,
        skill: Skill,
        dual: bool,
        rate: f32,
    ) -> Hand {
        let mut k = GrenadeSkill::of(skill);
        k.per_minute *= rate;
        if self.plan.is_none() {
            self.consider(m, world, &k);
        }
        if dual
            && !m.busy
            && let Some(plan) = self.plan
        {
            if m.tick.wrapping_sub(plan.since) <= AIM_WAIT {
                return Hand {
                    drop_left: true,
                    ..Default::default()
                };
            }
            self.plan = None;
            self.ready = m.tick.wrapping_add(RETRY);
        }
        self.follow(m, &k)
    }

    fn consider(&mut self, m: &Moment<'_>, world: &dyn Solid, k: &GrenadeSkill) {
        let Some(mark) = m.mark else { return };
        if m.busy || m.dt == 0 || m.tick < self.ready {
            return;
        }
        let have = m.grenades.unwrap_or([1, 0]);
        if have[0] <= 0 && have[1] <= 0 {
            return;
        }
        let d = len(sub(mark.at, m.feet));
        if !(RANGE.0..RANGE.1).contains(&d) {
            return;
        }
        // A group: the enemies near the target. Aimed between them.
        let near: Vec<Vec3> = m
            .enemies
            .iter()
            .map(|e| e.position)
            .filter(|p| len(sub(*p, mark.at)) > 0.3 && len(sub(*p, mark.at)) < GROUP)
            .collect();
        let mut rate = 1.0;
        if !near.is_empty() {
            rate *= GROUP_RATE;
        }
        if !mark.seen {
            rate *= UNSEEN_RATE;
        }
        if m.pushing && k.push {
            rate *= PUSH_RATE;
        }
        let chance = k.per_minute / (60.0 * 30.0) * m.dt as f32 * rate;
        if self.uniform() >= chance {
            return;
        }
        // Which: a plasma to stick a target in view and close, a frag
        // otherwise (it bounces into cover); whichever it has.
        let plasma_stick = k.stick && mark.seen && d < STICK_RANGE && have[1] > 0;
        let kind = if plasma_stick || have[0] <= 0 {
            Kind::Plasma
        } else {
            Kind::Frag
        };
        let mut at = mark.at;
        if !near.is_empty() && !plasma_stick {
            let n = near.len() as f32 + 1.0;
            let sum = near.iter().fold(mark.at, |a, p| add(a, *p));
            at = scale(sum, 1.0 / n);
        }
        let body = [Body {
            at: mark.at,
            vel: mark.vel,
        }];
        // Only one in view can be stuck.
        let bodies: &[Body] = if mark.seen { &body } else { &[] };
        let ask = Ask {
            from: m.eye,
            at,
            vel: mark.vel,
            lead: k.lead,
            ballistics: kind.ballistics(),
            bodies,
            stick: (kind == Kind::Plasma && mark.seen).then_some(0),
            banks: k.banks,
        };
        let Some(toss) = solve(world, &ask) else {
            // The arc is blocked: not now.
            self.ready = m.tick + RETRY;
            return;
        };
        if !safe(toss.flight.blast, m.feet, m.mates, k.careful) {
            self.ready = m.tick + RETRY;
            return;
        }
        self.last = Some(toss);
        // A human hand: the aim is a little off.
        let yaw = toss.yaw + self.normal() * k.noise;
        let pitch = toss.pitch + self.normal() * k.noise;
        self.plan = Some(Plan {
            dir: direction(yaw, pitch),
            kind,
            since: m.tick,
            press_until: None,
            swapped: None,
        });
    }

    fn follow(&mut self, m: &Moment<'_>, k: &GrenadeSkill) -> Hand {
        let Some(mut plan) = self.plan else {
            return Hand::default();
        };
        let mut hand = Hand {
            look: Some(add(m.eye, scale(plan.dir, 10.0))),
            ..Hand::default()
        };
        if let Some(until) = plan.press_until {
            if m.tick < until {
                hand.throw = true;
            } else {
                self.plan = None;
                self.throws += 1;
                hand.look = None;
            }
            return hand;
        }
        if m.busy || m.tick.wrapping_sub(plan.since) > AIM_WAIT {
            self.plan = None;
            self.ready = m.tick + RETRY;
            return Hand::default();
        }
        // The right type in hand first.
        if let Some(sel) = m.selected
            && sel != plan.kind
        {
            match plan.swapped {
                Some(at) if m.tick.wrapping_sub(at) < SWAP_WAIT => {
                    hand.swap = m.tick.wrapping_sub(at) < 2;
                }
                Some(_) => {
                    // It did not change (none of that type left?).
                    self.plan = None;
                    self.ready = m.tick + RETRY;
                    return Hand::default();
                }
                None => {
                    plan.swapped = Some(m.tick);
                    hand.swap = true;
                }
            }
            self.plan = Some(plan);
            return hand;
        }
        let off = (dot(unit(m.aim), plan.dir)).clamp(-1.0, 1.0).acos();
        if off < k.tolerance {
            plan.press_until = Some(m.tick + PRESS);
            self.ready = m.tick + COOLDOWN;
            hand.throw = true;
        }
        self.plan = Some(plan);
        hand
    }
}

#[cfg(test)]
mod tests;
