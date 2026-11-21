//! Bounce routes (docs/game/bots.md, "Superbounce routes"): get a bot
//! from where it is to a point using a superbounce, and run that as the
//! bot's whole pad.
//!
//! [`plan`] is pure. From a start, a goal and a map's spots
//! ([`crate::spots::Known`]) it picks the bounce that best helps reach
//! the goal, or, for fun, the one with the highest launch within reach,
//! and lists the steps: walk to a crouch spot, prime
//! ([`crate::glitches::prime`]), walk (no crouch) to behind the bounce
//! spot, jump onto it, ride the launch steering toward the goal, then
//! walk the rest. With no bounce that helps it is the plain walk.
//!
//! [`OrderRun`] plays a plan tick by tick and gives the motion for
//! [`crate::mind::RoutePad`]. The brain holds one and the pad is the
//! run's until it is over; the bot fights and picks up nothing meanwhile.
//!
//! Time in game ticks (30 a second); lengths in world units.

use crate::brain::Vec3;
use crate::cache::MapData;
use crate::glitches::prime::{PrimeRun, Primed, is_primed};
use crate::glitches::BounceRun;
use crate::mind::RouteMotion;
use crate::nav::RouteWalk;
use crate::spots::{Known, Site, dist_xy};

/// Gravity (world units a second squared): the jump model's estimate
/// (`nav::route`).
pub const GRAVITY: f32 = 5.2;
/// How fast the bot can move sideways in the air (world units a second).
/// **[guess]**: not traced; air control is limited and the route steers
/// with the full stick.
pub const AIR_SPEED: f32 = 2.5;
/// Headroom over the goal the flight's top must have (world units).
pub const CLEARANCE: f32 = 0.3;
/// What priming costs in the walking it is compared with (world units).
pub const PRIME_COST: f32 = 8.0;
/// A bounce route must be this much shorter than the plain walk to be
/// worth it (world units).
pub const GAIN: f32 = 4.0;
/// For fun: a bounce site whose walk there is no longer than this.
pub const FUN_REACH: f32 = 60.0;

/// What the asker wants.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    /// A bounce only where it gets the bot there when walking cannot, or
    /// gets it there much sooner; else the plain walk.
    #[default]
    Help,
    /// The highest launch within reach, then on to the goal. A bounce
    /// for its own sake; the plain walk only when there is none.
    Fun,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "help" => Some(Mode::Help),
            "fun" => Some(Mode::Fun),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Mode::Help => "help",
            Mode::Fun => "fun",
        }
    }
}

/// One thing a route does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    /// Walk along the nav mesh to `to`. `crouch`: crouched the whole
    /// way (into a low ceiling); otherwise never crouching.
    Walk { to: Vec3, crouch: bool },
    /// Crouch, stand up and be forced, under the ceiling at `at`.
    Prime { at: Vec3 },
    /// Jump onto the site from behind it, until it launches.
    Bounce(Site),
    /// Ride the launch, steering toward `to`, until down.
    Fly { to: Vec3 },
    /// Walk the rest, as a plain order.
    Nav { to: Vec3 },
}

impl Step {
    pub fn name(&self) -> &'static str {
        match self {
            Step::Walk { crouch: true, .. } => "walk_crouched",
            Step::Walk { .. } => "walk",
            Step::Prime { .. } => "prime",
            Step::Bounce(_) => "bounce",
            Step::Fly { .. } => "fly",
            Step::Nav { .. } => "nav",
        }
    }
}

/// Why the plan is what it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// No bounce helps: the plain walk.
    Plain,
    /// A bounce reaches it, or reaches it sooner.
    Helps,
    /// A bounce for fun.
    Fun,
}

impl Why {
    pub fn name(self) -> &'static str {
        match self {
            Why::Plain => "plain",
            Why::Helps => "helps",
            Why::Fun => "fun",
        }
    }
}

/// A route.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub steps: Vec<Step>,
    pub why: Why,
    /// The bounce used.
    pub site: Option<Site>,
    /// Walking, in world units, before the bounce plus after it (priming
    /// counted as [`PRIME_COST`]); `None` for the plain walk.
    pub cost: Option<f32>,
}

/// The ask.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ask {
    pub from: Vec3,
    pub to: Vec3,
    /// The pair word is already at -3 (the bot has primed and not
    /// bounced since).
    pub primed: bool,
    pub mode: Mode,
}

/// The length of the walk between two points on the nav mesh, `None`
/// when there is none.
pub type Walk<'a> = &'a dyn Fn(Vec3, Vec3) -> Option<f32>;

/// A launch from a site: how it flies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flight {
    /// Height of the top (world units, absolute).
    pub apex: f32,
    /// Seconds from the launch to coming down through the goal's height.
    pub time: f32,
    /// How far sideways the bot gets in that time.
    pub reach: f32,
}

/// The flight of a launch from `site` toward a point at height `z`:
/// up at `launch`, down under [`GRAVITY`], steering at [`AIR_SPEED`].
/// `None` when it never gets [`CLEARANCE`] above `z`.
pub fn flight(site: &Site, z: f32) -> Option<Flight> {
    let z0 = site.bounce.stand[2];
    let v = site.launch;
    if v <= 0.0 {
        return None;
    }
    let apex = z0 + v * v / (2.0 * GRAVITY);
    if apex < z + CLEARANCE {
        return None;
    }
    let time = (v + (v * v + 2.0 * GRAVITY * (z0 - z)).sqrt()) / GRAVITY;
    Some(Flight {
        apex,
        time,
        reach: AIR_SPEED * time,
    })
}

/// Where a flight toward `to` comes down: at `to` when it reaches it,
/// else as far along the way as it gets, at the site's own height.
pub fn landing(site: &Site, f: &Flight, to: Vec3) -> Vec3 {
    let s = site.bounce.stand;
    let gap = dist_xy(s, to);
    if gap <= f.reach {
        return to;
    }
    let k = f.reach / gap.max(1e-6);
    [s[0] + (to[0] - s[0]) * k, s[1] + (to[1] - s[1]) * k, s[2]]
}

struct Candidate {
    site: Site,
    crouch: Option<Vec3>,
    lead: f32,
    tail: f32,
}

impl Candidate {
    fn cost(&self) -> f32 {
        self.lead + self.tail + if self.crouch.is_some() { PRIME_COST } else { 0.0 }
    }
}

fn candidate(ask: &Ask, known: &Known, walk: Walk, site: Site) -> Option<Candidate> {
    if !site.standing() {
        return None;
    }
    let approach = site.bounce.from();
    let (crouch, lead) = if ask.primed {
        (None, walk(ask.from, approach)?)
    } else {
        // The crouch spot that makes the whole way shortest.
        let (c, l) = known
            .crouches
            .iter()
            .filter_map(|&c| Some((c, walk(ask.from, c)? + walk(c, approach)?)))
            .min_by(|a, b| a.1.total_cmp(&b.1))?;
        (Some(c), l)
    };
    // Flying toward the goal's height; for fun, as high as it gets.
    let z = if ask.mode == Mode::Fun {
        ask.to[2].min(site.bounce.stand[2] + site.launch * site.launch / (2.0 * GRAVITY) - CLEARANCE - 0.01)
    } else {
        ask.to[2]
    };
    let f = flight(&site, z)?;
    let land = landing(&site, &f, ask.to);
    let reaches = dist_xy(site.bounce.stand, ask.to) <= f.reach;
    let tail = if reaches { 0.0 } else { walk(land, ask.to)? };
    Some(Candidate {
        site,
        crouch,
        lead,
        tail,
    })
}

fn route(ask: &Ask, c: &Candidate, why: Why) -> Plan {
    let mut steps = Vec::new();
    if let Some(at) = c.crouch {
        steps.push(Step::Walk {
            to: at,
            crouch: true,
        });
        steps.push(Step::Prime { at });
    }
    // From here to the bounce the crouch button stays up: another crouch
    // rebuilds the pair and the word is lost.
    steps.push(Step::Walk {
        to: c.site.bounce.from(),
        crouch: false,
    });
    steps.push(Step::Bounce(c.site));
    steps.push(Step::Fly { to: ask.to });
    steps.push(Step::Nav { to: ask.to });
    Plan {
        steps,
        why,
        site: Some(c.site),
        cost: Some(c.cost()),
    }
}

/// The route from `ask.from` to `ask.to`.
///
/// - [`Mode::Help`]: the cheapest bounce route (walking before and after,
///   and [`PRIME_COST`] for the priming) that is [`GAIN`] shorter than
///   the plain walk, or that reaches where the plain walk cannot;
/// - [`Mode::Fun`]: of the bounces within [`FUN_REACH`] of walking and
///   that get a launch toward the goal, the highest launch, the cheaper
///   on a tie;
/// - otherwise the plain walk.
///
/// A site needs a crouch spot to prime at unless the bot is primed
/// already: an unprimed landing does not bounce
/// (docs/game/superbounce.md). Sites that need a drop onto them are not
/// used.
pub fn plan(ask: &Ask, known: &Known, walk: Walk) -> Plan {
    let plain = Plan {
        steps: vec![Step::Nav { to: ask.to }],
        why: Why::Plain,
        site: None,
        cost: None,
    };
    let candidates = known
        .sites
        .iter()
        .filter_map(|&s| candidate(ask, known, walk, s));
    match ask.mode {
        Mode::Help => {
            let plain_cost = walk(ask.from, ask.to);
            let best = candidates
                .filter(|c| plain_cost.is_none_or(|p| c.cost() + GAIN <= p))
                .min_by(|a, b| a.cost().total_cmp(&b.cost()));
            best.map_or(plain, |c| route(ask, &c, Why::Helps))
        }
        Mode::Fun => {
            let best = candidates.filter(|c| c.lead <= FUN_REACH).max_by(|a, b| {
                a.site
                    .launch
                    .total_cmp(&b.site.launch)
                    .then(b.cost().total_cmp(&a.cost()))
            });
            best.map_or(plain, |c| route(ask, &c, Why::Fun))
        }
    }
}

// ---- playing a plan ----

/// A route gives up after this long (ticks): 200 s.
pub const ORDER_TICKS: u32 = 6000;
/// Tries at the bounce spot before giving it up.
pub const BOUNCE_TRIES: u32 = 8;
/// Within this of a walk's end it has arrived (world units).
pub const ARRIVED: f32 = 0.3;
/// The last walk ends this near the goal.
pub const GOAL: f32 = 0.6;
/// Walking back to prime again, at most this often.
pub const REPRIMES: u32 = 2;
/// The ride gives up waiting to come down after this long (ticks).
pub const FLY_TICKS: u32 = 450;
/// Coming down: still this long (ticks), vertical speed under
/// [`SETTLED`] (world units a second), after the top.
pub const LANDED_TICKS: u32 = 2;
pub const SETTLED: f32 = 0.3;
/// Falling this fast means the top is passed (world units a second).
pub const FALLING: f32 = 0.5;

const TICKS: f32 = 30.0;

#[derive(Clone, Debug)]
enum State {
    Fresh,
    Walk(RouteWalk),
    Prime(PrimeRun),
    Bounce(BounceRun),
    Fly { since: u32, peaked: bool, still: u32 },
}

/// What a route asks of the pad this tick.
#[derive(Clone, Copy, Debug, Default)]
pub struct OrderOut {
    pub motion: RouteMotion,
    /// Over: `Ok` arrived, `Err` why not.
    pub done: Option<Result<(), &'static str>>,
}

/// Where a route stands, for the agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OrderStatus {
    pub step: usize,
    pub steps: usize,
    pub what: &'static str,
    /// The pair word is at -3 (read, or assumed after priming).
    pub primed: bool,
    /// Whether that was read from the game ([`OrderRun::step`]'s `pair`)
    /// or assumed.
    pub read: bool,
}

/// A plan being played.
#[derive(Clone, Debug)]
pub struct OrderRun {
    pub plan: Plan,
    at: usize,
    state: State,
    primed: bool,
    read: bool,
    started: u32,
    last: Option<Vec3>,
    reprimes: u32,
    seed: u64,
    /// A show-off: the bot gives it up when it sees an enemy.
    pub show_off: bool,
}

fn norm(d: [f32; 2]) -> ([f32; 2], f32) {
    let l = d[0].hypot(d[1]);
    if l < 1e-6 {
        ([0.0, 0.0], 0.0)
    } else {
        ([d[0] / l, d[1] / l], l)
    }
}

impl OrderRun {
    pub fn new(plan: Plan, primed: bool, tick: u32, seed: u64) -> OrderRun {
        OrderRun {
            plan,
            at: 0,
            state: State::Fresh,
            primed,
            read: false,
            started: tick,
            last: None,
            reprimes: 0,
            seed,
            show_off: false,
        }
    }

    /// The pair word is at -3 now: the crouch button must stay up.
    pub fn primed(&self) -> bool {
        self.primed
    }

    pub fn status(&self) -> OrderStatus {
        OrderStatus {
            step: self.at,
            steps: self.plan.steps.len(),
            what: self.plan.steps.get(self.at).map_or("done", Step::name),
            primed: self.primed,
            read: self.read,
        }
    }

    fn fail(&mut self, why: &'static str) -> OrderOut {
        self.primed = false;
        OrderOut {
            motion: RouteMotion::default(),
            done: Some(Err(why)),
        }
    }

    fn next(&mut self) {
        self.at += 1;
        self.state = State::Fresh;
    }

    /// This tick, standing at `at`. `pair`: the player's level pair word
    /// when it can be read (`bots::set_pair_reader`), `None` otherwise.
    pub fn step(&mut self, map: &MapData, at: Vec3, tick: u32, pair: Option<u32>) -> OrderOut {
        let vel = self.last.map(|l| {
            [
                (at[0] - l[0]) * TICKS,
                (at[1] - l[1]) * TICKS,
                (at[2] - l[2]) * TICKS,
            ]
        });
        self.last = Some(at);
        if tick.wrapping_sub(self.started) > ORDER_TICKS {
            return self.fail("timeout");
        }
        // The word read as something else on the ground: it was lost
        // (the pair was rebuilt). Back to the crouch spot.
        if self.primed
            && matches!(self.state, State::Walk(_))
            && pair.is_some_and(|w| !is_primed(w))
        {
            let prime = self.plan.steps[..self.at]
                .iter()
                .rposition(|s| matches!(s, Step::Prime { .. }));
            match prime {
                Some(i) if self.reprimes < REPRIMES => {
                    self.reprimes += 1;
                    self.primed = false;
                    self.at = i.saturating_sub(1);
                    self.state = State::Fresh;
                }
                _ => return self.fail("pair_lost"),
            }
        }
        for _ in 0..8 {
            let Some(step) = self.plan.steps.get(self.at).copied() else {
                self.primed = false;
                return OrderOut {
                    motion: RouteMotion::default(),
                    done: Some(Ok(())),
                };
            };
            let out = match step {
                Step::Walk { to, crouch } => self.walk(map, at, tick, to, crouch, ARRIVED),
                Step::Nav { to } => self.walk(map, at, tick, to, false, GOAL),
                Step::Prime { .. } => self.prime(tick, pair),
                Step::Bounce(site) => self.bounce(site, at, tick),
                Step::Fly { to } => self.fly(to, at, tick, vel),
            };
            match out {
                Ok(Some(mut motion)) => {
                    if self.primed {
                        // Never crouch between the prime and the bounce.
                        motion.crouch = false;
                    }
                    return OrderOut { motion, done: None };
                }
                // The step is over; the next one starts this tick.
                Ok(None) => self.next(),
                Err(why) => return self.fail(why),
            }
        }
        self.fail("stalled")
    }

    fn walk(
        &mut self,
        map: &MapData,
        at: Vec3,
        tick: u32,
        to: Vec3,
        crouch: bool,
        tolerance: f32,
    ) -> Result<Option<RouteMotion>, &'static str> {
        if matches!(self.state, State::Fresh) {
            self.state = State::Walk(RouteWalk::new(map, at, to, crouch, tick)?);
        }
        let State::Walk(w) = &mut self.state else {
            return Err("state");
        };
        w.step(map, at, tick, tolerance, 1.0)
    }

    fn prime(&mut self, tick: u32, pair: Option<u32>) -> Result<Option<RouteMotion>, &'static str> {
        if matches!(self.state, State::Fresh) {
            self.state = State::Prime(PrimeRun::new(tick));
        }
        let State::Prime(run) = &mut self.state else {
            return Err("state");
        };
        let out = run.step(tick, pair);
        match out.done {
            None => Ok(Some(RouteMotion {
                crouch: out.crouch,
                ..RouteMotion::default()
            })),
            Some(Primed::Failed) => Err("prime_failed"),
            Some(how) => {
                self.primed = true;
                self.read = how == Primed::Read;
                Ok(None)
            }
        }
    }

    fn bounce(
        &mut self,
        site: Site,
        at: Vec3,
        tick: u32,
    ) -> Result<Option<RouteMotion>, &'static str> {
        if matches!(self.state, State::Fresh) {
            let seed = self.seed ^ u64::from(tick);
            self.state = State::Bounce(BounceRun::new(site.bounce, BOUNCE_TRIES, tick, seed));
        }
        let State::Bounce(run) = &mut self.state else {
            return Err("state");
        };
        let o = run.step(at, tick);
        match o.done {
            Some(true) => {
                // The landing used the word up: a crouch is fine again.
                self.primed = false;
                return Ok(None);
            }
            Some(false) => return Err("no_bounce"),
            None => {}
        }
        let (dir, speed) = o.walk.map_or(([0.0; 2], 0.0), norm);
        let face = if site.bounce.drift[0].hypot(site.bounce.drift[1]) > 1e-3 {
            site.bounce.drift
        } else {
            let t = run.target;
            norm([t[0] - at[0], t[1] - at[1]]).0
        };
        Ok(Some(RouteMotion {
            dir,
            speed,
            yaw: Some(face[1].atan2(face[0])),
            jump: o.jump,
            crouch: false,
        }))
    }

    fn fly(
        &mut self,
        to: Vec3,
        at: Vec3,
        tick: u32,
        vel: Option<Vec3>,
    ) -> Result<Option<RouteMotion>, &'static str> {
        if matches!(self.state, State::Fresh) {
            self.state = State::Fly {
                since: tick,
                peaked: false,
                still: 0,
            };
        }
        let State::Fly {
            since,
            peaked,
            still,
        } = &mut self.state
        else {
            return Err("state");
        };
        let vz = vel.map_or(0.0, |v| v[2]);
        if vz < -FALLING {
            *peaked = true;
        }
        *still = if *peaked && vz.abs() < SETTLED {
            *still + 1
        } else {
            0
        };
        if *still >= LANDED_TICKS || tick.wrapping_sub(*since) > FLY_TICKS {
            return Ok(None);
        }
        let (dir, _) = norm([to[0] - at[0], to[1] - at[1]]);
        Ok(Some(RouteMotion {
            dir,
            speed: 1.0,
            yaw: Some(dir[1].atan2(dir[0])),
            jump: false,
            crouch: false,
        }))
    }
}

#[cfg(test)]
mod tests;
