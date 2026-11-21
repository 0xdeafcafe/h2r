//! Bounce routes (docs/game/bots.md, "Superbounce routes"): get a bot
//! from where it is to a point using a superbounce, and run that as the
//! bot's whole pad.
//!
//! [`plan`] is pure. From a start, a goal and a map's spots
//! ([`crate::spots::Known`]) it picks the bounce that best helps reach
//! the goal, or, for fun, the one with the highest launch within reach,
//! proven spots before likely ones, and lists the steps: walk to the
//! crouch spot's start, prime ([`crate::glitches::prime`]: crouch-walk in
//! under the lip, let go, walk out along the way out until the word reads
//! -3), walk (no crouch) to behind the bounce spot, or up to a launch
//! floor above it when the slot needs a drop ([`plan_with`]'s ledge),
//! jump or drop onto it, ride the launch steering toward the goal, then
//! walk the rest. With no bounce that helps it is the plain walk.
//!
//! [`OrderRun`] plays a plan tick by tick and gives the motion for
//! [`crate::mind::RoutePad`]. The brain holds one and the pad is the
//! run's until it is over; the bot fights and picks up nothing meanwhile.
//! Each step, its phase, its tries and the last landing by a strip are in
//! [`OrderRun::status`] and [`OrderRun::log`].
//!
//! Time in game ticks (30 a second); lengths in world units.

use crate::brain::Vec3;
use crate::cache::MapData;
use crate::glitches::prime::{Geo, PrimeRun, Primed, is_primed};
use crate::glitches::BounceRun;
use crate::mind::RouteMotion;
use crate::nav::RouteWalk;
use crate::spots::{Crouch, Known, Site, dist_xy};

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
    /// Crouch, stand up and be forced, at the crouch spot: in under its
    /// lip and out along its way out when that is known, else in place.
    Prime(Crouch),
    /// Jump onto the site from behind it, until it launches.
    Bounce(Site),
    /// Run off the launch floor at `from` and fall onto the site, aiming at
    /// its target, until it launches.
    Drop { from: Vec3, site: Site },
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
            Step::Drop { .. } => "drop",
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

/// A launch floor for a site that needs a drop: a point on the nav mesh at
/// least the site's drop height over its target and near it sideways, to
/// run off; `None` when there is none.
pub type Ledge<'a> = &'a dyn Fn(&Site) -> Option<Vec3>;

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
    crouch: Option<Crouch>,
    /// The launch floor, for a site that needs a drop.
    ledge: Option<Vec3>,
    lead: f32,
    tail: f32,
}

impl Candidate {
    fn cost(&self) -> f32 {
        self.lead + self.tail + if self.crouch.is_some() { PRIME_COST } else { 0.0 }
    }
}

/// The most path queries one plan makes, however many spots a map has.
pub const MAX_QUERIES: u32 = 32;
/// The crouch spots (nearest the start) and the sites (the likeliest)
/// a plan looks at.
pub const NEAR_CROUCHES: usize = 4;
pub const NEAR_SITES: usize = 8;

/// A crouch spot and the walk to it from the start, made once.
struct Prime {
    at: Crouch,
    lead: f32,
}

/// Where a prime at `c` starts walking: its first try's start when the
/// way out is known, else the spot itself.
pub fn prime_start(c: &Crouch) -> Vec3 {
    match geo(c) {
        Some(g) => {
            let s = g.start(1);
            [s[0], s[1], c.stand[2]]
        }
        None => c.stand,
    }
}

/// A crouch spot's walking geometry, when its way out is known.
pub fn geo(c: &Crouch) -> Option<Geo> {
    (c.out[0].hypot(c.out[1]) > 0.5).then_some(Geo {
        stand: c.stand,
        out: c.out,
        start: c.start,
    })
}

fn candidate(
    ask: &Ask,
    primes: &[Prime],
    walk: &dyn Fn(Vec3, Vec3) -> Option<f32>,
    site: Site,
    ledge: Option<Vec3>,
) -> Option<Candidate> {
    let approach = ledge.unwrap_or_else(|| site.bounce.from());
    let (crouch, lead) = if ask.primed {
        (None, walk(ask.from, approach)?)
    } else {
        // The crouch spot nearest the bounce, by straight line, proven
        // ones first: one path query a site.
        let c = primes.iter().min_by(|a, b| {
            (!a.at.proven())
                .cmp(&!b.at.proven())
                .then(dist_xy(a.at.stand, approach).total_cmp(&dist_xy(b.at.stand, approach)))
        })?;
        (Some(c.at), c.lead + walk(c.at.stand, approach)?)
    };
    // Flying toward the goal's height; for fun, as high as it gets.
    let z = flight_z(ask, &site);
    let f = flight(&site, z)?;
    let land = landing(&site, &f, ask.to);
    let reaches = dist_xy(site.bounce.stand, ask.to) <= f.reach;
    let tail = if reaches { 0.0 } else { walk(land, ask.to)? };
    Some(Candidate {
        site,
        crouch,
        ledge,
        lead,
        tail,
    })
}

fn flight_z(ask: &Ask, site: &Site) -> f32 {
    if ask.mode == Mode::Fun {
        ask.to[2].min(
            site.bounce.stand[2] + site.launch * site.launch / (2.0 * GRAVITY) - CLEARANCE - 0.01,
        )
    } else {
        ask.to[2]
    }
}

fn route(ask: &Ask, c: &Candidate, why: Why) -> Plan {
    let mut steps = Vec::new();
    if let Some(at) = c.crouch {
        // With the way out known the walk to its start is a plain walk
        // and the prime crouch-walks in; else walk in crouched, as under a
        // low ceiling.
        steps.push(Step::Walk {
            to: prime_start(&at),
            crouch: geo(&at).is_none(),
        });
        steps.push(Step::Prime(at));
    }
    // From here to the bounce the crouch button stays up: another crouch
    // rebuilds the pair and the word is lost.
    match c.ledge {
        Some(from) => {
            steps.push(Step::Walk { to: from, crouch: false });
            steps.push(Step::Drop { from, site: c.site });
        }
        None => {
            steps.push(Step::Walk {
                to: c.site.bounce.from(),
                crouch: false,
            });
            steps.push(Step::Bounce(c.site));
        }
    }
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
/// used ([`plan_with`] uses them). Proven sites and crouch spots come
/// before likely ones.
pub fn plan(ask: &Ask, known: &Known, walk: Walk) -> Plan {
    plan_with(ask, known, walk, &|_| None)
}

/// [`plan`], with a site that needs a drop used from its launch floor,
/// when `ledge` finds one.
pub fn plan_with(ask: &Ask, known: &Known, walk: Walk, ledge: Ledge) -> Plan {
    let plain = Plan {
        steps: vec![Step::Nav { to: ask.to }],
        why: Why::Plain,
        site: None,
        cost: None,
    };
    // Every path query goes through here: a map can have hundreds of
    // spots and a plan is made inside one frame.
    let used = std::cell::Cell::new(0u32);
    let query = |a: Vec3, b: Vec3| -> Option<f32> {
        if used.get() >= MAX_QUERIES {
            return None;
        }
        used.set(used.get() + 1);
        walk(a, b)
    };
    let plain_cost = (ask.mode == Mode::Help).then(|| query(ask.from, ask.to)).flatten();
    // Nothing farther off than this (straight line) can be a route that
    // pays: the walk it must beat, or the reach for fun.
    let limit = match ask.mode {
        Mode::Help => plain_cost.unwrap_or(FUN_REACH),
        Mode::Fun => FUN_REACH,
    };
    let primes: Vec<Prime> = if ask.primed {
        Vec::new()
    } else {
        let mut near: Vec<Crouch> = known
            .crouches
            .iter()
            .copied()
            .filter(|c| dist_xy(ask.from, c.stand) <= limit)
            .collect();
        near.sort_by(|a, b| {
            (!a.proven())
                .cmp(&!b.proven())
                .then(dist_xy(ask.from, a.stand).total_cmp(&dist_xy(ask.from, b.stand)))
        });
        near.into_iter()
            .take(NEAR_CROUCHES)
            .filter_map(|at| Some(Prime { at, lead: query(ask.from, prime_start(&at))? }))
            .collect()
    };
    if !ask.primed && primes.is_empty() {
        return plain;
    }
    let mut sites: Vec<(Site, Option<Vec3>)> = known
        .sites
        .iter()
        .copied()
        .filter(|s| dist_xy(ask.from, s.bounce.from()) <= limit)
        .filter(|s| flight(s, flight_z(ask, s)).is_some())
        .map(|s| (s, None))
        .collect();
    let proven_first = |a: &Site, b: &Site| (!a.proven()).cmp(&!b.proven());
    match ask.mode {
        Mode::Help => sites.sort_by(|(a, _), (b, _)| {
            let d = |s: &Site| dist_xy(ask.from, s.bounce.stand) + dist_xy(s.bounce.stand, ask.to);
            proven_first(a, b).then(d(a).total_cmp(&d(b)))
        }),
        Mode::Fun => sites.sort_by(|(a, _), (b, _)| {
            proven_first(a, b).then(b.launch.total_cmp(&a.launch)).then(
                dist_xy(ask.from, a.bounce.stand).total_cmp(&dist_xy(ask.from, b.bounce.stand)),
            )
        }),
    }
    // A site that needs a drop is used from its launch floor, if any.
    let mut looked = 0;
    for (s, l) in &mut sites {
        if !s.standing() && looked < 2 * NEAR_SITES {
            looked += 1;
            *l = ledge(s);
        }
    }
    sites.retain(|(s, l)| s.standing() || l.is_some());
    sites.truncate(NEAR_SITES);
    let candidates = sites
        .into_iter()
        .filter_map(|(s, l)| candidate(ask, &primes, &query, s, l));
    match ask.mode {
        Mode::Help => {
            let best = candidates
                .filter(|c| plain_cost.is_none_or(|p| c.cost() + GAIN <= p))
                .min_by(|a, b| {
                    // A proven site before a cheaper likely one.
                    (!a.site.proven())
                        .cmp(&!b.site.proven())
                        .then(a.cost().total_cmp(&b.cost()))
                });
            best.map_or(plain, |c| route(ask, &c, Why::Helps))
        }
        Mode::Fun => {
            let best = candidates.filter(|c| c.lead <= FUN_REACH).max_by(|a, b| {
                // A proven site before a higher likely one.
                a.site
                    .proven()
                    .cmp(&b.site.proven())
                    .then(a.site.launch.total_cmp(&b.site.launch))
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
/// Drops onto a slot before giving it up: each walks back up to the
/// launch floor.
pub const DROP_TRIES: u32 = 3;
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
/// A drop lines up at its launch floor this long (ticks) at most.
pub const LINE_UP_TICKS: u32 = 45;
/// Lined up: within this of the launch point (world units).
pub const LINE_UP: f32 = 0.15;
/// Running off the launch floor gives up after this long (ticks) without
/// falling.
pub const RUN_OFF_TICKS: u32 = 60;
/// Below the launch floor by this much (world units) the drop has begun.
pub const LEFT_FLOOR: f32 = 0.15;
/// The run off the floor (stick, 0..1): a walk, so the landing is not far
/// past the slot.
pub const RUN_OFF_SPEED: f32 = 0.5;
/// In the air the stick asks for this much per world unit the landing is
/// off the target (to 1).
pub const AIR_GAIN: f32 = 2.5;
/// A drop is over this long (ticks) after leaving the floor.
pub const DROP_AIR_TICKS: u32 = 150;
/// A rise this far (world units) above the lowest point after falling is
/// a launch.
pub const LAUNCH_RISE: f32 = 0.4;
/// A touchdown: the tick before fell at least this fast (world units a
/// second), this one at most [`TOUCHED`] (superbounce_charge's
/// `TOUCHDOWN_VZ` and `LANDED_VZ`).
pub const TOUCHDOWN: f32 = 1.0;
pub const TOUCHED: f32 = 0.5;
/// A touchdown that comes out faster than this (world units a second up)
/// bounced (superbounce_charge's `BOUNCE_VZ`).
pub const BOUNCE_VZ: f32 = 8.0;
/// The route's log keeps this many entries.
pub const LOG: usize = 64;

const TICKS: f32 = 30.0;

/// A drop's phase.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Dropping {
    /// At the launch floor, lining up, from the tick.
    Line(u32),
    /// Walking off the floor toward the target, from the tick.
    Run(u32),
    /// Falling, from the tick; the lowest height seen once falling.
    Air { since: u32, low: Option<f32> },
}

#[derive(Clone, Debug)]
enum State {
    Fresh,
    Walk(RouteWalk),
    Prime(PrimeRun),
    Bounce(BounceRun),
    Drop(Dropping),
    Fly { since: u32, peaked: bool, still: u32 },
}

/// A touchdown by the route's strip, as the bot saw it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Touchdown {
    pub tick: u32,
    /// The origin on the tick before touchdown.
    pub at: Vec3,
    /// The speed in, down, world units a second.
    pub speed_in: f32,
    /// The vertical speed out (the best of the touchdown tick and the two
    /// after), world units a second.
    pub vz_out: f32,
    /// How far from the target it came down (world units, sideways).
    pub miss: f32,
    /// The word on the tick before touchdown, when it could be read.
    pub word: Option<u32>,
}

impl Touchdown {
    pub fn bounced(&self) -> bool {
        self.vz_out > BOUNCE_VZ
    }

    /// What it came to, by its first failed test, as the game's landing
    /// classifier names it: `bounce`, `not_primed`, `missed` (more than
    /// `width / 2` off the target), `too_slow` (under `need`), `no_bounce`.
    pub fn result(&self, width: f32, need: f32) -> &'static str {
        if self.bounced() {
            "bounce"
        } else if self.word.is_some_and(|w| !is_primed(w)) {
            "not_primed"
        } else if self.miss > width / 2.0 {
            "missed"
        } else if need > 0.0 && self.speed_in < need {
            "too_slow"
        } else {
            "no_bounce"
        }
    }
}

/// Watches the vertical speed for touchdowns during a bounce or a drop.
#[derive(Clone, Copy, Debug, Default)]
struct Watch {
    prev: Option<(Vec3, f32, Option<u32>)>,
    /// The vertical speed the tick before `prev`: the touchdown tick's
    /// own reads short when the body stops partway through it.
    before: Option<f32>,
    /// A touchdown still reading its vertical speed out, and ticks left.
    open: Option<(Touchdown, u32)>,
}

impl Watch {
    /// This tick; a touchdown once its speed out is read.
    fn step(&mut self, at: Vec3, vz: f32, word: Option<u32>, tick: u32, target: Vec3) -> Option<Touchdown> {
        let mut done = None;
        if let Some((mut t, left)) = self.open.take() {
            t.vz_out = t.vz_out.max(vz);
            if left <= 1 {
                done = Some(t);
            } else {
                self.open = Some((t, left - 1));
            }
        } else if let Some((p, pvz, pword)) = self.prev
            && -pvz.min(self.before.unwrap_or(pvz)) >= TOUCHDOWN
            && vz > -TOUCHED
        {
            // The speed in: the faster of the last two ticks down, since
            // a speed worked from positions loses the part of the tick
            // after contact.
            let t = Touchdown {
                tick,
                at: p,
                speed_in: -pvz.min(self.before.unwrap_or(pvz)),
                vz_out: vz,
                miss: dist_xy(p, target),
                word: pword,
            };
            self.open = Some((t, 2));
        }
        self.before = self.prev.map(|(_, v, _)| v);
        self.prev = Some((at, vz, word));
        done
    }
}

/// What a route asks of the pad this tick.
#[derive(Clone, Copy, Debug, Default)]
pub struct OrderOut {
    pub motion: RouteMotion,
    /// Over: `Ok` arrived, `Err` why not.
    pub done: Option<Result<(), &'static str>>,
}

/// Where a route stands, for the agent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrderStatus {
    pub step: usize,
    pub steps: usize,
    pub what: &'static str,
    /// The step's phase: a prime's `start`, `in`, `release`, `out`; a
    /// drop's `line_up`, `run_off`, `air`; a bounce's `settle` or `air`; a
    /// walk's `walking`; else empty.
    pub phase: &'static str,
    /// The step's try (from 1; 0 for a step that does not retry).
    pub tries: u32,
    /// The pair word is at -3 (read, or assumed after priming).
    pub primed: bool,
    /// Whether that was read from the game ([`OrderRun::step`]'s `pair`)
    /// or assumed.
    pub read: bool,
    /// The last touchdown during a bounce or a drop.
    pub landing: Option<Touchdown>,
}

/// One line of a route's log: a step begun or ended, or a landing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Logged {
    pub tick: u32,
    pub step: &'static str,
    /// `start`, `done`, `retry`, `reprime`, a landing's result, or why the
    /// step failed.
    pub what: &'static str,
    pub at: Vec3,
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
    drops: u32,
    seed: u64,
    watch: Watch,
    /// The site touchdowns are read against.
    watching: Option<Site>,
    landing: Option<Touchdown>,
    log: Vec<Logged>,
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
            drops: 0,
            seed,
            watch: Watch::default(),
            watching: None,
            landing: None,
            log: Vec::new(),
            show_off: false,
        }
    }

    /// The pair word is at -3 now: the crouch button must stay up.
    pub fn primed(&self) -> bool {
        self.primed
    }

    /// The log, oldest first (at most [`LOG`]).
    pub fn log(&self) -> &[Logged] {
        &self.log
    }

    fn note(&mut self, tick: u32, what: &'static str, at: Vec3) {
        let step = self.plan.steps.get(self.at).map_or("done", Step::name);
        if self.log.len() >= LOG {
            self.log.remove(0);
        }
        self.log.push(Logged { tick, step, what, at });
    }

    pub fn status(&self) -> OrderStatus {
        let (phase, tries) = match &self.state {
            State::Fresh => ("", 0),
            State::Walk(_) => ("walking", 0),
            State::Prime(p) => (p.phase(), p.tries()),
            State::Bounce(b) => (if b.airborne() { "air" } else { "settle" }, b.tries + 1),
            State::Drop(d) => (
                match d {
                    Dropping::Line(_) => "line_up",
                    Dropping::Run(_) => "run_off",
                    Dropping::Air { .. } => "air",
                },
                self.drops + 1,
            ),
            State::Fly { .. } => ("air", 0),
        };
        OrderStatus {
            step: self.at,
            steps: self.plan.steps.len(),
            what: self.plan.steps.get(self.at).map_or("done", Step::name),
            phase,
            tries,
            primed: self.primed,
            read: self.read,
            landing: self.landing,
        }
    }

    fn fail(&mut self, why: &'static str, tick: u32, at: Vec3) -> OrderOut {
        self.note(tick, why, at);
        self.primed = false;
        OrderOut {
            motion: RouteMotion::default(),
            done: Some(Err(why)),
        }
    }

    fn next(&mut self, tick: u32, at: Vec3) {
        self.note(tick, "done", at);
        self.at += 1;
        self.state = State::Fresh;
    }

    /// Back to step `i` (a walk), starting it afresh.
    fn back_to(&mut self, i: usize, tick: u32, at: Vec3, why: &'static str) {
        self.note(tick, why, at);
        self.at = i;
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
            return self.fail("timeout", tick, at);
        }
        // Touchdowns during a bounce or a drop, against the site's target,
        // read to the end after the step has moved on (a launch moves it
        // on before the speed out is read).
        match self.plan.steps.get(self.at) {
            Some(Step::Bounce(s)) | Some(Step::Drop { site: s, .. }) => self.watching = Some(*s),
            _ if self.watch.open.is_none() => {
                self.watching = None;
                self.watch = Watch::default();
            }
            _ => {}
        }
        if let (Some(site), Some(v)) = (self.watching, vel)
            && let Some(t) = self.watch.step(at, v[2], pair, tick, site.target())
        {
            self.landing = Some(t);
            let width = if site.width > 0.0 { site.width } else { crate::spots::SLOT_WIDTH };
            self.note(tick, t.result(width, site.need), t.at);
        }
        // The word read as something else on the ground: it was lost
        // (the pair was rebuilt). Back to the crouch spot.
        let grounded = match &self.state {
            State::Walk(_) => true,
            State::Drop(Dropping::Line(_) | Dropping::Run(_)) => true,
            _ => false,
        };
        if self.primed && grounded && pair.is_some_and(|w| !is_primed(w)) {
            let prime = self.plan.steps[..self.at]
                .iter()
                .rposition(|s| matches!(s, Step::Prime(_)));
            match prime {
                Some(i) if self.reprimes < REPRIMES => {
                    self.reprimes += 1;
                    self.primed = false;
                    self.back_to(i.saturating_sub(1), tick, at, "reprime");
                }
                _ => return self.fail("pair_lost", tick, at),
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
            if matches!(self.state, State::Fresh) {
                self.note(tick, "start", at);
            }
            let out = match step {
                Step::Walk { to, crouch } => self.walk(map, at, tick, to, crouch, ARRIVED),
                Step::Nav { to } => self.walk(map, at, tick, to, false, GOAL),
                Step::Prime(c) => self.prime(c, at, tick, pair),
                Step::Bounce(site) => self.bounce(site, at, tick),
                Step::Drop { from, site } => self.drop(from, site, at, tick),
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
                Ok(None) => self.next(tick, at),
                Err("drop_again") => {
                    // Back up to the launch floor for another drop.
                    self.back_to(self.at.saturating_sub(1), tick, at, "retry");
                }
                Err(why) => return self.fail(why, tick, at),
            }
        }
        self.fail("stalled", tick, at)
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

    fn prime(
        &mut self,
        c: Crouch,
        at: Vec3,
        tick: u32,
        pair: Option<u32>,
    ) -> Result<Option<RouteMotion>, &'static str> {
        if matches!(self.state, State::Fresh) {
            self.state = State::Prime(match geo(&c) {
                Some(g) => PrimeRun::at(tick, g),
                None => PrimeRun::new(tick),
            });
        }
        let State::Prime(run) = &mut self.state else {
            return Err("state");
        };
        let out = run.step_at(tick, at, pair);
        match out.done {
            None => {
                let (dir, speed) = out.walk.map_or(([0.0; 2], 0.0), norm);
                Ok(Some(RouteMotion {
                    dir,
                    speed: speed.min(1.0),
                    // Face the way it walks; standing still, as it was.
                    yaw: (speed > 0.0).then(|| dir[1].atan2(dir[0])),
                    crouch: out.crouch,
                    ..RouteMotion::default()
                }))
            }
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
            let mut b = site.bounce;
            b.stand = site.target();
            self.state = State::Bounce(BounceRun::new(b, BOUNCE_TRIES, tick, seed));
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

    /// Off the launch floor at `from` and down onto the site's target:
    /// line up facing it, walk off, steer in the air so the landing is on
    /// the target, then see whether it launched.
    fn drop(
        &mut self,
        from: Vec3,
        site: Site,
        at: Vec3,
        tick: u32,
    ) -> Result<Option<RouteMotion>, &'static str> {
        if matches!(self.state, State::Fresh) {
            self.state = State::Drop(Dropping::Line(tick));
        }
        let State::Drop(d) = &mut self.state else {
            return Err("state");
        };
        let target = site.target();
        let (to_target, gap) = norm([target[0] - at[0], target[1] - at[1]]);
        let face = Some(to_target[1].atan2(to_target[0]));
        let motion = |dir: [f32; 2], speed: f32| RouteMotion {
            dir,
            speed,
            yaw: face,
            jump: false,
            crouch: false,
        };
        match *d {
            Dropping::Line(since) => {
                let (to_from, off) = norm([from[0] - at[0], from[1] - at[1]]);
                if off <= LINE_UP || tick.wrapping_sub(since) > LINE_UP_TICKS {
                    *d = Dropping::Run(tick);
                    return Ok(Some(motion(to_target, RUN_OFF_SPEED)));
                }
                Ok(Some(motion(to_from, (off * 4.0).clamp(0.15, 0.6))))
            }
            Dropping::Run(since) => {
                if at[2] < from[2] - LEFT_FLOOR {
                    *d = Dropping::Air { since: tick, low: None };
                } else if tick.wrapping_sub(since) > RUN_OFF_TICKS {
                    return Err("no_drop");
                }
                Ok(Some(motion(to_target, RUN_OFF_SPEED)))
            }
            Dropping::Air { since, low } => {
                let low = Some(low.map_or(at[2], |l: f32| l.min(at[2])));
                *d = Dropping::Air { since, low };
                if low.is_some_and(|l| at[2] > l + LAUNCH_RISE) {
                    // Sank into the slot and was thrown up.
                    self.primed = false;
                    return Ok(None);
                }
                let landed = self
                    .landing
                    .is_some_and(|t| t.tick >= since && !t.bounced());
                if landed || tick.wrapping_sub(since) > DROP_AIR_TICKS {
                    self.drops += 1;
                    return if self.drops < DROP_TRIES {
                        Err("drop_again")
                    } else {
                        Err("no_bounce")
                    };
                }
                // Steer the landing onto the target: harder the further off.
                Ok(Some(motion(to_target, (gap * AIR_GAIN).min(1.0))))
            }
        }
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
