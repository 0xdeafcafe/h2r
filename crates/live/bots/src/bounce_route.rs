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
use crate::glitches::BounceRun;
use crate::glitches::prime::{Geo, PrimeRun, Primed, is_primed};
use crate::mind::RouteMotion;
use crate::nav::RouteWalk;
use crate::spots::{Crouch, Known, Site, dist_xy};
use crate::tuning::BounceTuning;
use std::sync::Arc;

use superbounce::learned::{How, Knobs};
use superbounce::{Poly, rule};

pub mod chain;
mod flight;

// ---- the map's walks and launch floors ----

/// How far sideways (world units) a launch floor may be from the slot it
/// drops onto: room for the air stick to bring the fall back to the edge.
pub const LEDGE_REACH: f32 = 3.0;

/// The drop every takeoff aims for: from the flight's apex to the target,
/// origin to origin
/// (`superbounce::rule::DROP_BEST`). The sweep's one tick then comes as
/// the body reaches the floor (docs/game/superbounce.md, "The bounce
/// rule").
pub const DROP_AIM: f32 = rule::DROP_BEST;
/// The landing aims this near (world units) a floor-polygon edge, or
/// nearer.
pub const EDGE_AIM: f32 = 0.05;
/// A target farther than [`EDGE_AIM`] from its floor's nearest edge is
/// moved toward the edge to this far from it.
pub const EDGE_HOLD: f32 = 0.02;
/// The drift (world units a second) a takeoff holds across the edge it
/// lands by: 0.30 leaves +13.0, a full launch (`rule::DRIFT_FULL`, 0.15,
/// is the least that launches fully).
pub const DRIFT_ACROSS: f32 = 0.3;
/// The floor polygons within this (world units, sideways) of a target
/// are the ones [`judge_takeoff`] steps over.
pub const FLOOR_REACH: f32 = 3.0;
/// [`judge_takeoff`] flies the takeoff on the flight model to the row
/// this far (world units) over the target, then hands the rest of the
/// fall to the bounce rule.
pub const JUDGE_OVER: f32 = 0.6;

/// How far a running jump's apex is over the floor it leaves: the
/// flight model's top for [`JUMP_VZ`], about 1.1.
pub fn jump_rise() -> f32 {
    flight::apex(0.0, JUMP_VZ)
}

/// A launch floor for a site that needs a drop: the nav mesh's point
/// nearest the site's target sideways, within [`LEDGE_REACH`], at the
/// height a drop wants over the target.
///
/// The floor's takeoff puts the apex [`DROP_AIM`] over the target, within
/// the drop window (`rule::DROP_MIN` to `rule::DROP_MAX`): a floor that
/// high to run off its edge, else one [`jump_rise`] lower for a running
/// jump. A measured recipe's explicit takeoff still wins in
/// [`launch_from`]; its old fall height does not choose this floor.
///
/// A V's arm ([`Site::arm`]) is dropped onto from the side it is
/// approached from: the search is moved half its reach that way, with
/// half the reach, and a floor on the arm's own side is none (docs/game/
/// bounce-search.md, "Vs: the mirror and its control"). The [`Ledge`] a
/// plan is made with.
pub fn launch_floor(m: &MapData, s: &Site) -> Option<Vec3> {
    let t = s.target();
    let (shift, reach) = match s.arm {
        Some(a) => (
            [a.from[0] * LEDGE_REACH / 2.0, a.from[1] * LEDGE_REACH / 2.0],
            LEDGE_REACH / 2.0,
        ),
        None => ([0.0, 0.0], LEDGE_REACH),
    };
    let find = |height: f32, up: f32, down: f32| {
        let over = [t[0] + shift[0], t[1] + shift[1], t[2] + height];
        m.nav
            .nearest(over, reach, up, down)
            .map(|(_, p)| p)
            .filter(|p| s.approached_from([p[0], p[1]]))
    };
    let (up, down) = (rule::DROP_MAX - DROP_AIM, DROP_AIM - rule::DROP_MIN);
    find(DROP_AIM, up, down).or_else(|| find(DROP_AIM - jump_rise(), up, down))
}

/// Where a drop onto `s` is taken from: the learned launch point
/// (`fx fy fz`, docs/game/bounce-search.md), else the spot's own takeoff
/// (a measured recipe's, [`crate::glitches::Bounce::takeoff`]), else what
/// `ledge` finds ([`launch_floor`] in play).
pub fn launch_from(s: &Site, ledge: Ledge) -> Option<Vec3> {
    s.knobs.from.or(s.bounce.takeoff).or_else(|| ledge(s))
}

/// The length of the walk along the nav mesh from `from` to `to`: the
/// [`Walk`] a plan is made with. It is the walk a route's step would take
/// ([`crate::nav::walk_length`]): no teleporter or crouch-jump link, and a
/// goal off the mesh only within a last stretch's reach of it. `None` for
/// a walk that would fail (`no_path`, `goal_off_mesh`).
pub fn path_length(m: &MapData, from: Vec3, to: Vec3) -> Option<f32> {
    crate::nav::walk_length(m, from, to, false, ARRIVED)
}

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
    /// Off the launch floor at `from` (a run off the edge, or a running
    /// jump where that can't land on the target fast enough:
    /// [`drop_takeoff`]) and down onto the site, aiming at its target,
    /// until it launches.
    Drop { from: Vec3, site: Site },
    /// The next bounce of a chain (docs/game/bots.md, "Chains"): in the
    /// air off the last launch, steer the landing onto the site's target,
    /// until it launches again or comes down without (the chain is over:
    /// any further `Chain` step is passed over).
    Chain(Site),
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
            Step::Chain(_) => "chain",
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
    /// The asker named the place: a spot to walk to, prime at or bounce
    /// on ([`plan_site`], [`prime_steps`]; `player.goto`).
    Asked,
}

impl Why {
    pub fn name(self) -> &'static str {
        match self {
            Why::Plain => "plain",
            Why::Helps => "helps",
            Why::Fun => "fun",
            Why::Asked => "asked",
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
        self.lead
            + self.tail
            + if self.crouch.is_some() {
                PRIME_COST
            } else {
                0.0
            }
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

/// The crouch spots a plan may prime at, with the walk to each: none
/// when the asker is primed already, else the [`NEAR_CROUCHES`] nearest
/// the start within `limit` (straight line), proven ones first.
fn primes(
    ask: &Ask,
    known: &Known,
    limit: f32,
    query: &dyn Fn(Vec3, Vec3) -> Option<f32>,
) -> Vec<Prime> {
    if ask.primed {
        return Vec::new();
    }
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
        .filter_map(|at| {
            Some(Prime {
                at,
                lead: query(ask.from, prime_start(&at))?,
            })
        })
        .collect()
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
        steps.extend(prime_steps(&at));
    }
    // From here to the bounce the crouch button stays up: another crouch
    // rebuilds the pair and the word is lost.
    match c.ledge {
        Some(from) => {
            steps.push(Step::Walk {
                to: from,
                crouch: false,
            });
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
    let plain_cost = (ask.mode == Mode::Help)
        .then(|| query(ask.from, ask.to))
        .flatten();
    // Nothing farther off than this (straight line) can be a route that
    // pays: the walk it must beat, or the reach for fun.
    let limit = match ask.mode {
        Mode::Help => plain_cost.unwrap_or(FUN_REACH),
        Mode::Fun => FUN_REACH,
    };
    let primes = primes(ask, known, limit, &query);
    if !ask.primed && primes.is_empty() {
        return plain;
    }
    let mut sites: Vec<(Site, Option<Vec3>)> = known
        .sites
        .iter()
        .copied()
        // A candidate crease is the training's until it is proved.
        .filter(Site::plays)
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
            *l = launch_from(s, ledge);
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

/// Priming at the crouch spot `c`: walk to its start, then prime. With
/// the way out known the walk there is a plain walk and the prime
/// crouch-walks in; else the walk goes in crouched, as under a low
/// ceiling.
pub fn prime_steps(c: &Crouch) -> [Step; 2] {
    [
        Step::Walk {
            to: prime_start(c),
            crouch: geo(c).is_none(),
        },
        Step::Prime(*c),
    ]
}

/// Where a bounce at `site` is taken from: its launch floor when it needs
/// a drop (`ledge`), else behind it for the standing jump. `None` for a
/// drop with no launch floor.
pub fn approach(site: &Site, ledge: Ledge) -> Option<Vec3> {
    if site.standing() {
        Some(site.bounce.from())
    } else {
        launch_from(site, ledge)
    }
}

/// The route from `from` that bounces on `site` and nothing else, for an
/// asker who named it (`player.goto`): prime at a crouch spot unless
/// `primed` (the one with the least walking to the bounce, proven first,
/// as [`plan_with`] picks), walk to the bounce's approach ([`approach`]),
/// take the bounce and ride the launch back down. No reach limit and no
/// walk to beat. `None` when no walk or no crouch spot gets there, or
/// the site needs a drop with no launch floor.
pub fn plan_site(
    from: Vec3,
    primed: bool,
    site: Site,
    known: &Known,
    walk: Walk,
    ledge: Ledge,
) -> Option<Plan> {
    let ask = Ask {
        from,
        to: site.target(),
        primed,
        mode: Mode::Fun,
    };
    let floor = if site.standing() {
        None
    } else {
        Some(launch_from(&site, ledge)?)
    };
    let primes = primes(&ask, known, f32::INFINITY, walk);
    if !primed && primes.is_empty() {
        return None;
    }
    let c = candidate(&ask, &primes, walk, site, floor)?;
    let mut plan = route(&ask, &c, Why::Asked);
    // Done once down: the asker is where it bounced.
    plan.steps.retain(|s| !matches!(s, Step::Nav { .. }));
    Some(plan)
}

/// `plan` (a route by one site, [`plan_site`]) going on into a chain
/// over `links` ([`chain::plan`]): after the first bounce, a
/// [`Step::Chain`] onto each link in turn, then the ride down off the
/// last. A plan with no bounce, or no links, is left as it is.
pub fn chain_onto(plan: &mut Plan, links: &[Site]) {
    let Some(last) = links.last() else {
        return;
    };
    let Some(i) = plan
        .steps
        .iter()
        .position(|s| matches!(s, Step::Bounce(_) | Step::Drop { .. }))
    else {
        return;
    };
    plan.steps.truncate(i + 1);
    plan.steps.extend(links.iter().map(|s| Step::Chain(*s)));
    plan.steps.push(Step::Fly { to: last.target() });
}

// ---- a site paired with a crouch spot ----

/// The crouch spots a pairing tries walking from: the nearest a site's
/// approach (straight line), proven first.
pub const PAIR_CROUCHES: usize = 6;
/// A crouch spot farther (straight line) than this from a site's approach
/// is not paired with it.
pub const PAIR_REACH: f32 = FUN_REACH;

/// What a route by one site needs, found once a site (the practice keeps
/// it, `mind::practice`): where the bounce is taken from, and the crouch
/// spot to prime at, from which a walk reaches it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pairing {
    /// Where the bounce is taken from ([`approach`]).
    pub approach: Vec3,
    /// The crouch spot: proven first, then the least walking from it to
    /// the approach. `None` when no crouch spot within [`PAIR_REACH`] has
    /// a walk there (only a primed bot can use the site).
    pub crouch: Option<Crouch>,
    /// That walk (world units).
    pub walk: Option<f32>,
}

/// Pairs `site` with the crouch spot to prime at for it: of the
/// [`PAIR_CROUCHES`] within [`PAIR_REACH`] of its approach, proven first
/// and nearest first, the ones whose start a walk reaches and from which a
/// walk reaches the approach, proven first, then the shortest walk. At
/// most two walks a crouch spot looked at. A crouch spot `usable` rules
/// out (one the practice has given up on) is not looked at.
/// `Err("no_approach")` for a drop with no launch floor, or a launch that
/// never gets the bot up.
pub fn pair_site(
    site: &Site,
    known: &Known,
    walk: Walk,
    ledge: Ledge,
    usable: &dyn Fn(&Crouch) -> bool,
) -> Result<Pairing, &'static str> {
    let to = approach(site, ledge).ok_or("no_approach")?;
    let ask = Ask {
        from: to,
        to: site.target(),
        primed: true,
        mode: Mode::Fun,
    };
    flight(site, flight_z(&ask, site)).ok_or("no_approach")?;
    let mut near: Vec<Crouch> = known
        .crouches
        .iter()
        .copied()
        .filter(|c| dist_xy(c.stand, to) <= PAIR_REACH && usable(c))
        .collect();
    near.sort_by(|a, b| {
        (!a.proven())
            .cmp(&!b.proven())
            .then(dist_xy(a.stand, to).total_cmp(&dist_xy(b.stand, to)))
    });
    // Its start must be walkable to (a scanned start can be off the
    // mesh), and the walk from it, as a route's candidate measures it,
    // must reach the approach.
    let best = near
        .into_iter()
        .take(PAIR_CROUCHES)
        .filter_map(|c| {
            walk(c.stand, prime_start(&c))?;
            Some((c, walk(c.stand, to)?))
        })
        .min_by(|(a, wa), (b, wb)| (!a.proven()).cmp(&!b.proven()).then(wa.total_cmp(wb)));
    Ok(Pairing {
        approach: to,
        crouch: best.map(|(c, _)| c),
        walk: best.map(|(_, w)| w),
    })
}

// ---- playing a plan ----

/// The leg a walk is on, for the reason a lost walk fails with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Leg {
    /// To a crouch spot's start (a prime is next).
    Crouch,
    /// To a bounce's approach or a drop's launch floor.
    Approach,
    /// Any other walk (the rest of a route, a plain walk).
    Nav,
}

/// The reason a walk the follower lost fails a route with: the check
/// (`nav::RouteWalk`'s `path_lost_stuck`, `_fell`, `_jump`, `_link`,
/// `_ended`) and
/// the leg, as `path_lost_<check>_<leg>`. Every one starts `path_lost`.
pub fn lost_on(check: &'static str, leg: Leg) -> &'static str {
    match (check, leg) {
        ("path_lost_stuck", Leg::Crouch) => "path_lost_stuck_crouch",
        ("path_lost_stuck", Leg::Approach) => "path_lost_stuck_approach",
        ("path_lost_stuck", Leg::Nav) => "path_lost_stuck_nav",
        ("path_lost_fell", Leg::Crouch) => "path_lost_fell_crouch",
        ("path_lost_fell", Leg::Approach) => "path_lost_fell_approach",
        ("path_lost_fell", Leg::Nav) => "path_lost_fell_nav",
        ("path_lost_link", Leg::Crouch) => "path_lost_link_crouch",
        ("path_lost_link", Leg::Approach) => "path_lost_link_approach",
        ("path_lost_link", Leg::Nav) => "path_lost_link_nav",
        ("path_lost_ended", Leg::Crouch) => "path_lost_ended_crouch",
        ("path_lost_ended", Leg::Approach) => "path_lost_ended_approach",
        ("path_lost_ended", Leg::Nav) => "path_lost_ended_nav",
        ("path_lost_jump", Leg::Crouch) => "path_lost_jump_crouch",
        ("path_lost_jump", Leg::Approach) => "path_lost_jump_approach",
        ("path_lost_jump", Leg::Nav) => "path_lost_jump_nav",
        (other, _) => other,
    }
}

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
/// A walk the follower gives up on (`path_lost_*`) is planned again from
/// where the bot is, this many times a step, before the route fails.
pub const WALK_REPLANS: u32 = 3;
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
/// The stick of a drop's slower run (0..1), the one it takes when a run
/// at full stick would carry the flight past the target however hard the
/// air stick brakes: the drill runs' walk, about 1.05 units/s
/// ([`ground_speed`]).
pub const RUN_OFF_SPEED: f32 = 0.6;
/// The air stick's correction, over the one that would just land the
/// flight on the target ([`air_stick`]; 1: just enough).
pub const AIR_GAIN: f32 = 1.2;
/// A drop is over this long (ticks) after leaving the floor.
pub const DROP_AIR_TICKS: u32 = 150;
/// A rise this far (world units) above the lowest point after falling is
/// a launch.
pub const LAUNCH_RISE: f32 = 0.4;
/// A drop is falling once this far (world units) under the top of its
/// flight (a jump off the floor rises first).
pub const DROP_FALLING: f32 = 0.2;
/// A touchdown: the tick before fell at least this fast (world units a
/// second), this one at most [`TOUCHED`] (superbounce_charge's
/// `TOUCHDOWN_VZ` and `LANDED_VZ`).
pub const TOUCHDOWN: f32 = 1.0;
pub const TOUCHED: f32 = 0.5;
/// A touchdown that comes out faster than this (world units a second up)
/// bounced, for the route's own log and training. The game's landing
/// classifier and medals count a launch from 3.5 over a wider window
/// (`superbounce_charge::BOUNCE_VZ` and `LAUNCH_TICKS`).
pub const BOUNCE_VZ: f32 = 8.0;
/// The route's log keeps this many entries.
pub const LOG: usize = 64;

const TICKS: f32 = 30.0;

/// Gravity on a falling body (world units a second squared): the
/// superbounce scan's fit of the recipe (`superbounce::bounce`'s
/// `GRAVITY`), and what the round-3 bounce-practice runs show: a standing
/// jump's vertical speed goes from +3.08 to -2.90 in 43 ticks
/// (docs/game/notes/2026-11-21-bot-bounce-sim.md).
pub const FALL_GRAVITY: f32 = 4.15;
/// A jump's vertical speed on its first tick (world units a second): the
/// same runs, every jump.
pub const JUMP_VZ: f32 = 3.08;
/// The ground speed at full stick (world units a second): the drill runs
/// at a stick of 1 ran at 1.96 to 2.12 (ground-truth 7,
/// docs/game/notes/2026-11-21-bot-bounce-sim.md).
pub const RUN_SPEED: f32 = 2.0;
/// The smallest stick that moves the body: past the move stick's dead
/// zone (`nav::route`'s 0.15). The runs' standing jumps held 0.15 of
/// stick toward the drift in the air and gained no sideways speed at all.
pub const MIN_STICK: f32 = 0.2;
/// A drop's run starts this far (world units) behind its launch point.
pub const RUN_UP: f32 = 0.6;
/// The jump is pressed this far (world units) short of the launch point,
/// and further by the ground the body covers in [`INPUT_LAG`].
pub const TAKEOFF_EARLY: f32 = 0.05;
/// Ticks from the pad asking to the body doing it: the drill runs' jumps
/// left the floor 4 to 6 ticks after the press, and their air steering
/// swung the sideways speed up and down 7 ticks a side, the swing of a
/// steering some 4 ticks late (docs/game/notes/2026-11-21-bot-bounce-sim.md).
pub const INPUT_LAG: u32 = 5;
/// In the air the stick lets go when the landing, as it flies, comes down
/// this near the target (world units): the recipe's "easing off the stick
/// in the air". Letting go is holding the sideways speed ([`air_stick`]).
pub const AIR_DEAD: f32 = 0.03;
/// How fast the air stick changes the sideways speed (world units a second
/// squared): 0.035 units/s a tick in every drill run, braking and pushing,
/// at every stick past the dead zone.
pub const AIR_ACCEL: f32 = 1.05;
/// The sideways speed a full air stick drives toward (world units a
/// second). **[rough]**: under a stick of 0.8 to 1 pushing on, the drill
/// runs' flights held 0.9 to 1.3 and never went faster, and a small stick
/// on slowed a 0.46 one to 0.40; a faster flight slows toward it.
pub const AIR_TOP: f32 = 1.2;
/// A landing, as it flies, this far (world units) past the target brakes
/// with the full stick.
pub const AIR_PAST: f32 = 0.3;

/// How long (seconds) a fall from `vz` (up positive) takes to come down
/// `height` (world units, positive down), under [`FALL_GRAVITY`]. `None`
/// when it never gets there.
pub fn fall_time(height: f32, vz: f32) -> Option<f32> {
    let d = vz * vz + 2.0 * FALL_GRAVITY * height;
    (d >= 0.0)
        .then(|| (vz + d.sqrt()) / FALL_GRAVITY)
        .filter(|t| *t > 0.0)
}

/// The ground speed (world units a second) a stick runs at, from what the
/// drill runs measured (docs/game/notes/2026-11-21-bot-bounce-sim.md):
/// nothing inside the dead zone, about 1.05 at 0.6 to 0.85 of stick (the
/// committed run asked for 0.6 to 0.8 and got 1.02 to 1.08; a gain of 2
/// on the same asked more and got 0.9 to 1.75), and 2.0 at 1. Not linear:
/// a walk, then a run. Straight lines between the points. **[needs more
/// points]**: nothing was measured between 0.15 and 0.6 or between 0.85
/// and 1.
pub fn ground_speed(stick: f32) -> f32 {
    const MAP: [(f32, f32); 4] = [(0.15, 0.0), (0.6, 1.05), (0.85, 1.1), (1.0, RUN_SPEED)];
    let s = stick.clamp(0.0, 1.0);
    if s <= MAP[0].0 {
        return 0.0;
    }
    if s >= 1.0 {
        return RUN_SPEED;
    }
    for w in MAP.windows(2) {
        let ((s0, v0), (s1, v1)) = (w[0], w[1]);
        if s <= s1 {
            return v0 + (v1 - v0) * (s - s0) / (s1 - s0);
        }
    }
    RUN_SPEED
}

/// How far (world units) a flight goes sideways in `t` seconds from a
/// sideways speed `u`, along its line: braking with the full air stick
/// the whole way (to a stop, no further), and pushing on with it (toward
/// [`AIR_TOP`], which a faster flight slows to).
pub fn air_reach(u: f32, t: f32) -> (f32, f32) {
    let a = AIR_ACCEL;
    let u = u.max(0.0);
    let t = t.max(0.0);
    let near = if u <= a * t {
        u * u / (2.0 * a)
    } else {
        u * t - 0.5 * a * t * t
    };
    // Toward the top at `a`, then at it.
    let (dv, sign) = if u >= AIR_TOP {
        (u - AIR_TOP, -1.0)
    } else {
        (AIR_TOP - u, 1.0)
    };
    let t1 = (dv / a).min(t);
    let far = u * t1 + sign * 0.5 * a * t1 * t1 + (u + sign * a * t1) * (t - t1);
    (near, far)
}

/// How a drop leaves its launch floor: the run's stick, and whether it
/// jumps; and what the flight model ([`fall_time`], [`air_reach`]) makes
/// of it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Takeoff {
    /// A running jump; else a run off the edge.
    pub jump: bool,
    /// The run's stick, and the ground speed it gives ([`ground_speed`]).
    pub stick: f32,
    pub speed: f32,
    /// Seconds from the floor to the target's height.
    pub time: f32,
    /// The speed it comes down at (world units a second, down).
    pub speed_in: f32,
    /// How far it goes sideways braking and pushing ([`air_reach`]), and
    /// how far the target is from the launch point.
    pub near: f32,
    pub far: f32,
    pub gap: f32,
}

impl Takeoff {
    /// The air stick can bring the flight down on the target.
    pub fn reaches(&self) -> bool {
        self.gap >= self.near && self.gap <= self.far
    }

    /// It comes down at least as fast as `need` (0: anything).
    pub fn fast_enough(&self, need: f32) -> bool {
        need <= 0.0 || self.speed_in >= need
    }

    /// How far (world units) the target is outside what it reaches.
    pub fn off(&self) -> f32 {
        (self.near - self.gap).max(self.gap - self.far).max(0.0)
    }
}

/// The flight of a drop from `from` onto `site`'s target, run at `stick`,
/// jumping or not. `None` when it never comes down there.
pub fn takeoff(from: Vec3, site: &Site, stick: f32, jump: bool) -> Option<Takeoff> {
    let target = site.target();
    let h = from[2] - target[2];
    let vz = if jump { JUMP_VZ } else { 0.0 };
    let time = fall_time(h, vz)?;
    let speed = ground_speed(stick);
    let (near, far) = air_reach(speed, time);
    Some(Takeoff {
        jump,
        stick,
        speed,
        time,
        speed_in: (vz * vz + 2.0 * FALL_GRAVITY * h).max(0.0).sqrt(),
        near,
        far,
        gap: dist_xy(from, target),
    })
}

/// How a drop from `from` onto `site` leaves the floor, from the flight
/// model. A run off the edge at full stick when it comes down fast enough
/// (`site.need`) and the air stick can land it on the target; else one
/// at `slow` (the tuning's `run_off_speed`) that can; else a running jump
/// at full stick, then at `slow`, that comes down fast enough and gets
/// there. When none does: of those fast enough (else of all), the one
/// whose target is least outside its reach.
pub fn drop_takeoff(from: Vec3, site: &Site, slow: f32) -> Takeoff {
    let tries = [(1.0, false), (slow, false), (1.0, true), (slow, true)];
    let all: Vec<Takeoff> = tries
        .iter()
        .filter_map(|&(stick, jump)| takeoff(from, site, stick, jump))
        .collect();
    if let Some(t) = all.iter().find(|t| t.reaches() && t.fast_enough(site.need)) {
        return *t;
    }
    let fast: Vec<&Takeoff> = all.iter().filter(|t| t.fast_enough(site.need)).collect();
    let pool: Vec<&Takeoff> = if fast.is_empty() {
        all.iter().collect()
    } else {
        fast
    };
    pool.into_iter()
        .min_by(|a, b| a.off().total_cmp(&b.off()))
        .copied()
        // Not even a fall (the target over the floor): the running jump.
        .unwrap_or(Takeoff {
            jump: true,
            stick: 1.0,
            speed: RUN_SPEED,
            time: 0.0,
            speed_in: 0.0,
            near: 0.0,
            far: 0.0,
            gap: dist_xy(from, site.target()),
        })
}

/// Where a drop onto `target` aims, and the way across the edge it lands
/// by (unit, X and Y): the target itself when its floor's nearest edge
/// (`rule::edge_under`) is within [`EDGE_AIM`], else the point
/// [`EDGE_HOLD`] from that edge on the way to it. With no floor under
/// the target: the target, and no way across.
pub fn edge_aim(target: Vec3, floors: &[Poly]) -> (Vec3, [f32; 2]) {
    // The target is an origin; its floor is a centimetre under it.
    let Some(e) = rule::edge_under(floors, [target[0], target[1], target[2] + 0.05]) else {
        return (target, [0.0; 2]);
    };
    if e.distance <= EDGE_AIM {
        return (target, e.across);
    }
    let [a, b] = e.line;
    let ab = [b[0] - a[0], b[1] - a[1]];
    let l2 = ab[0] * ab[0] + ab[1] * ab[1];
    let k = if l2 > 1e-12 {
        (((target[0] - a[0]) * ab[0] + (target[1] - a[1]) * ab[1]) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let foot = [a[0] + ab[0] * k, a[1] + ab[1] * k];
    let (to, gap) = norm([foot[0] - target[0], foot[1] - target[1]]);
    let go = (gap - EDGE_HOLD).max(0.0);
    (
        [target[0] + to[0] * go, target[1] + to[1] * go, target[2]],
        e.across,
    )
}

/// The sideways velocity `v` with at least [`DRIFT_ACROSS`] of it across
/// the edge whose way across is `across` (unit), the way `v` already
/// goes across it (or along `across` for none): the drift a takeoff
/// holds into the landing.
pub fn drift_across(v: [f32; 2], across: [f32; 2]) -> [f32; 2] {
    if across == [0.0; 2] {
        return v;
    }
    let a = v[0] * across[0] + v[1] * across[1];
    let s = if a < 0.0 { -1.0 } else { 1.0 };
    let want = s * a.abs().max(DRIFT_ACROSS);
    [v[0] + across[0] * (want - a), v[1] + across[1] * (want - a)]
}

/// What the bounce rule (`superbounce::rule::predict`) makes of a drop
/// from `from` onto `site` taken as `t`, over `floors` (the map's floor
/// polygons; those within [`FLOOR_REACH`] of the target are stepped).
/// The flight model flies the takeoff, steered at the aim
/// ([`edge_aim`]), down to the row [`JUDGE_OVER`] over the target, led
/// by the sideways ground the rest of the fall covers; from that row the
/// rule steps the fall with the drift held [`DRIFT_ACROSS`] across the
/// edge ([`drift_across`]). The drop's apex sets the phase the body
/// reaches the floor in, so a takeoff whose apex is out of the drop
/// window comes back caught, stopped or landed. `None` when the flight
/// does not come down there.
pub fn judge_takeoff(
    from: Vec3,
    site: &Site,
    t: &Takeoff,
    floors: &[Poly],
) -> Option<rule::Verdict> {
    let target = site.target();
    let near = rule::floors_near(floors, [target[0], target[1]], FLOOR_REACH);
    let (aim, across) = edge_aim(target, &near);
    let (line, gap) = norm([aim[0] - from[0], aim[1] - from[1]]);
    let line = if gap > 1e-6 { line } else { [1.0, 0.0] };
    let vz = if t.jump { JUMP_VZ } else { 0.0 };
    let v = [line[0] * t.speed, line[1] * t.speed, vz];
    let raised = |_: [f32; 2]| target[2] + JUDGE_OVER;
    let first = flight::pursue(from, v, [aim[0], aim[1]], raised)?.landing;
    // The rest of the fall carries the body on: aim that much short.
    let rest = if first.speed > 0.1 {
        JUDGE_OVER / first.speed
    } else {
        0.0
    };
    let lead = [
        aim[0] - first.velocity[0] * rest,
        aim[1] - first.velocity[1] * rest,
    ];
    let l = flight::pursue(from, v, lead, raised).map_or(first, |p| p.landing);
    let drift = drift_across(l.velocity, across);
    Some(rule::predict(
        l.row,
        [l.velocity[0], l.velocity[1], -l.speed],
        drift,
        &near,
    ))
}

/// The air stick (world axes, 0..1) for a flight at sideways speed `v`
/// whose landing, with that speed held, misses the target by `miss`
/// (target minus landing) with `rem` seconds left. The air stick drives
/// the sideways speed toward the stick times [`AIR_TOP`] at
/// [`AIR_ACCEL`], so the stick says the speed to end at: the one that,
/// reached at that rate and then held, puts the landing on the target,
/// the change `gain` times what is just enough. A miss under `letgo`
/// ([`AIR_DEAD`] unless a learned line says, [`Flown::letgo`]) holds the
/// speed; one the stick can't make up in the time left, or a landing
/// [`AIR_PAST`] beyond the target along the flight, is the full stick
/// toward the target's side.
pub fn air_stick(v: [f32; 2], miss: [f32; 2], rem: f32, gain: f32, letgo: f32) -> ([f32; 2], f32) {
    let (to, off) = norm(miss);
    let (along, speed) = norm(v);
    let past = -(miss[0] * along[0] + miss[1] * along[1]);
    let want = if off < letgo {
        v
    } else if past > AIR_PAST && speed > 0.0 {
        return (to, 1.0);
    } else {
        // The change e that lands it: |e| (rem - |e| / 2a) = off.
        let a = AIR_ACCEL;
        let d = rem * rem - 2.0 * off / a;
        let e = if d >= 0.0 {
            a * (rem - d.sqrt()) * gain
        } else {
            // More than it can make up: past the top, the full stick.
            2.0 * AIR_TOP + speed
        };
        [v[0] + to[0] * e, v[1] + to[1] * e]
    };
    // The stick is the speed to end at, over the top; past the top, all
    // of it.
    let (dir, mag) = norm([want[0] / AIR_TOP, want[1] / AIR_TOP]);
    let mag = mag.min(1.0);
    (dir, if mag < MIN_STICK { 0.0 } else { mag })
}

// ---- the knobs a site is flown with ----

/// The numbers a route flies a site by (docs/game/bounce-search.md, "How
/// the route uses a line"): each is the site's learned knob
/// ([`Site::knobs`]) when its line sets one, else the route's own. Made by
/// [`flown`], the one place a knob meets the route; [`OrderRun`]'s drop,
/// its air steering and its standing bounce read these and nothing else
/// for them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flown {
    /// A run off the edge, a running jump, or a standing jump.
    pub how: How,
    /// Where it lands: [`Site::target`] (the learned aim first).
    pub target: Vec3,
    /// Degrees the run turns off the launch-to-target line, and the run's
    /// heading (unit, X and Y) it makes.
    pub yaw: f32,
    pub heading: [f32; 2],
    /// How far behind the launch point the run starts ([`RUN_UP`]).
    pub back: f32,
    /// The run's stick ([`drop_takeoff`]'s).
    pub stick: f32,
    /// The jump is pressed this far short of the launch point
    /// ([`TAKEOFF_EARLY`]).
    pub early: f32,
    /// The air stick's correction (the tuning's `air_gain`) and where it
    /// lets go ([`AIR_DEAD`]).
    pub gain: f32,
    pub letgo: f32,
    /// A standing jump's air stick toward the drift (the tuning's
    /// `drift`) and its stand before the jump (`still_ticks`).
    pub drift: f32,
    pub still: u32,
}

impl Flown {
    /// A running jump.
    pub fn jump(&self) -> bool {
        self.how == How::Jump
    }

    /// Where a drop from `from` lines up: [`Flown::back`] behind it,
    /// against the heading.
    pub fn run_up(&self, from: Vec3) -> Vec3 {
        [
            from[0] - self.heading[0] * self.back,
            from[1] - self.heading[1] * self.back,
            from[2],
        ]
    }

    /// The same numbers as knobs, every one its `how` uses set (and the
    /// launch point `from`, a drop's): the search's centre for a site with
    /// no line (`learn::Setup::base`).
    pub fn knobs(&self, from: Option<Vec3>) -> Knobs {
        Knobs {
            how: Some(self.how),
            from,
            aim: Some([self.target[0], self.target[1]]),
            yaw: Some(self.yaw),
            back: Some(self.back),
            run: Some(self.stick),
            early: Some(self.early),
            gain: Some(self.gain),
            letgo: Some(self.letgo),
            drift: Some(self.drift),
            still: Some(self.still),
        }
        .for_how()
    }
}

/// The best takeoff from `from` onto `site` that jumps (or not) as `jump`
/// says, as [`drop_takeoff`] picks among its own: one that reaches and
/// comes down fast enough, at full stick then at `slow`; else the one
/// least outside its reach.
fn takeoff_as(from: Vec3, site: &Site, slow: f32, jump: bool) -> Option<Takeoff> {
    let all: Vec<Takeoff> = [1.0, slow]
        .iter()
        .filter_map(|&stick| takeoff(from, site, stick, jump))
        .collect();
    all.iter()
        .find(|t| t.reaches() && t.fast_enough(site.need))
        .or_else(|| all.iter().min_by(|a, b| a.off().total_cmp(&b.off())))
        .copied()
}

/// How `site` is flown from the launch point `from` (a standing site: its
/// approach; only the heading reads it) with the route's tuning `tune`:
/// each knob the site's line sets, else the route's own value
/// (docs/game/bounce-search.md, "The knobs"). With no knob set it is
/// exactly what the route plays without a learned file. A drop's `how`
/// with no `run` set takes the stick [`drop_takeoff`] would for that
/// `how`.
pub fn flown(site: &Site, from: Vec3, tune: &BounceTuning) -> Flown {
    let k = site.knobs;
    let target = site.target();
    let (line, _) = norm([target[0] - from[0], target[1] - from[1]]);
    let yaw = k.yaw.unwrap_or(0.0);
    let heading = if yaw == 0.0 {
        line
    } else {
        let (s, c) = yaw.to_radians().sin_cos();
        [line[0] * c - line[1] * s, line[0] * s + line[1] * c]
    };
    let (how, stick) = if site.standing() {
        (How::Stand, k.run.unwrap_or(1.0))
    } else {
        let model = drop_takeoff(from, site, tune.run_off_speed);
        let jump = match k.how {
            Some(h) => h == How::Jump,
            None => model.jump,
        };
        let stick = k.run.unwrap_or_else(|| {
            if jump == model.jump {
                model.stick
            } else {
                takeoff_as(from, site, tune.run_off_speed, jump).map_or(1.0, |t| t.stick)
            }
        });
        (if jump { How::Jump } else { How::Runoff }, stick)
    };
    Flown {
        how,
        target,
        yaw,
        heading,
        back: k.back.unwrap_or(RUN_UP),
        stick,
        early: k.early.unwrap_or(TAKEOFF_EARLY),
        gain: k.gain.unwrap_or(tune.air_gain),
        letgo: k.letgo.unwrap_or(AIR_DEAD),
        drift: k.drift.unwrap_or(tune.drift),
        still: k.still.unwrap_or(tune.still_ticks),
    }
}

/// A drop's phase.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Dropping {
    /// Walking to the run-up, [`RUN_UP`] behind the launch point, from
    /// the tick.
    Line(u32),
    /// Running at the launch point toward the target, from the tick.
    Run(u32),
    /// The jump held, then the run until the body leaves the floor, from
    /// the tick.
    Jump(u32),
    /// Off the floor, from the tick; the highest point, and the lowest
    /// once falling.
    Air {
        since: u32,
        top: f32,
        low: Option<f32>,
    },
}

#[derive(Clone, Debug)]
enum State {
    Fresh,
    Walk(RouteWalk),
    /// Before a prime: shooting out the glass its walk in goes through
    /// ([`OrderRun::shooting`]), from the tick, piece by piece.
    Break(Breaking),
    Prime(PrimeRun),
    Bounce(BounceRun),
    Drop(Dropping),
    /// In the air toward a chain's next site, from the tick; the highest
    /// point, and the lowest once falling.
    Chain {
        since: u32,
        top: f32,
        low: Option<f32>,
    },
    Fly {
        since: u32,
        peaked: bool,
        still: u32,
    },
}

/// Breaking the glass before a crouch spot's walk in (docs/game/bots.md,
/// "Breaking the glass"): the pieces (`crate::glass`) in its way, by
/// breakable index and where a shot at each aims, lowest first.
#[derive(Clone, Debug)]
struct Breaking {
    since: u32,
    pieces: Vec<(u8, Vec3)>,
    /// The piece being shot: the lowest still whole.
    aim: Option<Vec3>,
}

/// Breaking the glass gives up after this long (ticks), the route failing
/// with `glass_whole`: 5 s, enough to empty a clip into it.
pub const BREAK_TICKS: u32 = 150;

/// The pieces of glass the walk in of crouch spot `c` goes through
/// (`crate::glass::in_the_way`, from its prime's start to its stand) that
/// `mask` says are whole, lowest first. None when its way out isn't known
/// (it primes in place) or the mask can't be read.
pub fn glass_in_the_way(
    map: &MapData,
    c: &Crouch,
    mask: Option<&crate::glass::Mask>,
) -> Vec<(u8, Vec3)> {
    let (Some(mask), Some(_)) = (mask, geo(c)) else {
        return Vec::new();
    };
    let panes = &map.walls.glass;
    crate::glass::in_the_way(panes, prime_start(c), c.stand)
        .into_iter()
        .map(|i| &panes[i])
        .filter(|p| crate::glass::intact(mask, p.index))
        .map(|p| (p.index, p.center()))
        .collect()
}

fn prime_run(c: &Crouch, tick: u32) -> PrimeRun {
    match geo(c) {
        Some(g) => PrimeRun::at(tick, g),
        None => PrimeRun::new(tick),
    }
}

/// One tick of breaking the glass, standing at `at` (the prime's start):
/// face the lowest piece still whole and shoot it. `Ok(None)` once every
/// piece is broken; `glass_whole` after [`BREAK_TICKS`], or when the mask
/// can no longer be read.
fn break_glass(
    b: &mut Breaking,
    at: Vec3,
    tick: u32,
    mask: Option<&crate::glass::Mask>,
) -> Result<Option<RouteMotion>, &'static str> {
    let Some(mask) = mask else {
        return Err("glass_whole");
    };
    let Some(&(_, aim)) = b
        .pieces
        .iter()
        .find(|(i, _)| crate::glass::intact(mask, *i))
    else {
        b.aim = None;
        return Ok(None);
    };
    if tick.wrapping_sub(b.since) > BREAK_TICKS {
        return Err("glass_whole");
    }
    b.aim = Some(aim);
    let d = [aim[0] - at[0], aim[1] - at[1]];
    Ok(Some(RouteMotion {
        yaw: Some(d[1].atan2(d[0])),
        ..RouteMotion::default()
    }))
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
    /// the one catch, `superbounce::CATCH`, off the target), `too_slow`
    /// (under `need`), `no_bounce`.
    pub fn result(&self, need: f32) -> &'static str {
        if self.bounced() {
            "bounce"
        } else if self.word.is_some_and(|w| !is_primed(w)) {
            "not_primed"
        } else if self.miss > superbounce::catch() {
            "missed"
        } else if need > 0.0 && self.speed_in < need {
            "too_slow"
        } else {
            "no_bounce"
        }
    }
}

/// The vertical speed off the floor is the best of this many ticks
/// (ticks) from the takeoff: the press is on the ground, and the body
/// leaves it about [`INPUT_LAG`] later (the drill runs: 4 to 6).
pub const TAKEOFF_TICKS: u32 = 10;
/// The most [`Leap`]s a run keeps.
pub const FLIGHTS: usize = 8;

/// One jump or drop of a bounce step, from the takeoff to the landing:
/// what the practice's line for the try says
/// (docs/game/bots.md, "Practising bounces").
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Leap {
    /// The tick it left: the jump pressed, or, a walk-off, the floor left.
    pub tick: u32,
    /// Where it left from.
    pub takeoff: Vec3,
    /// The ground speed then (world units a second, from positions).
    pub speed: f32,
    /// The best vertical speed in the [`TAKEOFF_TICKS`] from the takeoff
    /// (world units a second, up positive).
    pub vz: f32,
    /// The pair word on the takeoff tick, when it could be read.
    pub word: Option<u32>,
    /// The stick the run asked for in the air (world axes, direction
    /// times speed), summed over `ticks`; `idle` of them let it go; the
    /// most it asked.
    pub stick_sum: [f32; 2],
    pub stick_max: f32,
    pub ticks: u32,
    pub idle: u32,
    /// The touchdown by the strip it ended in, when one was seen.
    pub landing: Option<Touchdown>,
    /// What it came to: the landing's result, or why it ended without
    /// one (`no_landing`, a route error).
    pub end: Option<&'static str>,
}

impl Leap {
    fn new(tick: u32, at: Vec3, v: Vec3, word: Option<u32>) -> Leap {
        Leap {
            tick,
            takeoff: at,
            speed: v[0].hypot(v[1]),
            vz: v[2],
            word,
            stick_sum: [0.0; 2],
            stick_max: 0.0,
            ticks: 0,
            idle: 0,
            landing: None,
            end: None,
        }
    }

    /// The mean air stick (world axes).
    pub fn stick_mean(&self) -> [f32; 2] {
        let n = self.ticks.max(1) as f32;
        [self.stick_sum[0] / n, self.stick_sum[1] / n]
    }

    /// One line: the takeoff, the air, the landing and what it came to.
    pub fn describe(&self) -> String {
        let p = |v: Vec3| format!("({:.2}, {:.2}, {:.2})", v[0], v[1], v[2]);
        let word = self
            .word
            .map_or_else(|| "unread".to_string(), |w| (w as i32).to_string());
        let s = self.stick_mean();
        let down = match self.landing {
            Some(t) => format!(
                "down {} miss {:.3} in {:.2} out {:+.2}",
                p(t.at),
                t.miss,
                t.speed_in,
                t.vz_out
            ),
            None => "no touchdown seen".to_string(),
        };
        format!(
            "takeoff {} ground {:.2} vz {:+.2} word {}; air {} ticks stick mean ({:+.2}, {:+.2}) max {:.2} let go {}; {}: {}",
            p(self.takeoff),
            self.speed,
            self.vz,
            word,
            self.ticks,
            s[0],
            s[1],
            self.stick_max,
            self.idle,
            down,
            self.end.unwrap_or("open")
        )
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
    fn step(
        &mut self,
        at: Vec3,
        vz: f32,
        word: Option<u32>,
        tick: u32,
        target: Vec3,
    ) -> Option<Touchdown> {
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
    /// walk's `walking`; a prime's break step's `break`; else empty.
    pub phase: &'static str,
    /// The step's try (from 1; 0 for a step that does not retry).
    pub tries: u32,
    /// The chain's links tried (begun in the air off a launch) and made
    /// (launched again) so far ([`Step::Chain`]).
    pub chain: (u32, u32),
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
    /// `start`, `done`, `retry`, `reprime`, `break_glass` and
    /// `glass_broken` (a prime's break step), a landing's result, or why
    /// the step failed.
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
    /// The walk under way planned again ([`WALK_REPLANS`]).
    replans: u32,
    /// The jump and drop links its walks gave up at
    /// ([`RouteWalk::failed_link`]): no walk of the route takes them again.
    avoid: Vec<Vec3>,
    drops: u32,
    seed: u64,
    watch: Watch,
    /// The site touchdowns are read against.
    watching: Option<Site>,
    landing: Option<Touchdown>,
    log: Vec<Logged>,
    /// The jump or drop in the air now, those over (at most
    /// [`FLIGHTS`]), and whether the last tick was in the air.
    flight: Option<Leap>,
    flights: Vec<Leap>,
    was_air: bool,
    /// A show-off: the bot gives it up when it sees an enemy.
    pub show_off: bool,
    /// How near the goal a [`Step::Nav`] ends ([`GOAL`] unless
    /// [`OrderRun::arrive_within`] says).
    goal: f32,
    /// The numbers it plays by, taken when it starts
    /// ([`crate::tuning::bounce`]).
    tune: crate::tuning::BounceTuning,
    /// The loaded BSP's breakable mask this tick ([`OrderRun::see_glass`]).
    glass: Option<crate::glass::Mask>,
    /// A chain's links tried and made ([`Step::Chain`]), and whether one
    /// came down without launching (the rest are passed over).
    chain: (u32, u32),
    chain_broken: bool,
    /// The map's floor polygons ([`OrderRun::see_floors`]): a drop's takeoff
    /// is judged on them before it is taken ([`judge_takeoff`]). Empty:
    /// not judged.
    floors: Arc<Vec<Poly>>,
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
            replans: 0,
            avoid: Vec::new(),
            drops: 0,
            seed,
            watch: Watch::default(),
            watching: None,
            landing: None,
            log: Vec::new(),
            flight: None,
            flights: Vec::new(),
            was_air: false,
            show_off: false,
            goal: GOAL,
            tune: crate::tuning::bounce().sane(),
            glass: None,
            chain: (0, 0),
            chain_broken: false,
            floors: Arc::default(),
        }
    }

    /// The loaded BSP's breakable mask this tick (`crate::guest::breakable`),
    /// before [`OrderRun::step`]: which pieces of glass are whole.
    pub fn see_glass(&mut self, mask: Option<crate::glass::Mask>) {
        self.glass = mask;
    }

    /// The map's floor polygons (`crate::spots::floors`), when it is
    /// played for real: a drop's takeoff is judged on them
    /// ([`judge_takeoff`]) before the run at the launch point, and one
    /// the bounce rule says will not bounce ends the run
    /// (`takeoff_rejected`). A training run's search is left unjudged.
    pub fn see_floors(&mut self, floors: Arc<Vec<Poly>>) {
        self.floors = floors;
    }

    /// While breaking the glass before a prime: the point the bot shoots
    /// at (the pad aims there and pulls the trigger).
    pub fn shooting(&self) -> Option<Vec3> {
        match &self.state {
            State::Break(b) => b.aim,
            _ => None,
        }
    }

    /// The same, started primed from a word read at -3 (a drill's word
    /// set by the host, or one kept from before): [`OrderStatus::read`].
    pub fn primed_read(mut self) -> OrderRun {
        self.read = self.primed;
        self
    }

    /// The jumps and drops so far, oldest first (at most [`FLIGHTS`]),
    /// the one in the air last.
    pub fn flights(&self) -> Vec<Leap> {
        self.flights
            .iter()
            .chain(self.flight.as_ref())
            .copied()
            .collect()
    }

    fn end_flight(&mut self, end: &'static str) {
        if let Some(mut f) = self.flight.take() {
            f.end.get_or_insert(end);
            if self.flights.len() >= FLIGHTS {
                self.flights.remove(0);
            }
            self.flights.push(f);
        }
    }

    /// The flight's bookkeeping, after this tick's motion: a takeoff on
    /// the tick the step goes airborne, the vertical speed off the floor,
    /// and the stick asked in the air.
    fn track_flight(
        &mut self,
        at: Vec3,
        vel: Option<Vec3>,
        pair: Option<u32>,
        tick: u32,
        m: &RouteMotion,
    ) {
        let air = match &self.state {
            State::Drop(Dropping::Jump(_) | Dropping::Air { .. }) | State::Chain { .. } => true,
            State::Bounce(b) => b.airborne(),
            _ => false,
        };
        let v = vel.unwrap_or([0.0; 3]);
        if air && !self.was_air {
            self.end_flight("no_landing");
            self.flight = Some(Leap::new(tick, at, v, pair));
        }
        self.was_air = air;
        let Some(f) = self.flight.as_mut().filter(|_| air) else {
            return;
        };
        if tick.wrapping_sub(f.tick) <= TAKEOFF_TICKS {
            f.vz = f.vz.max(v[2]);
        }
        f.ticks += 1;
        f.stick_sum[0] += m.dir[0] * m.speed;
        f.stick_sum[1] += m.dir[1] * m.speed;
        f.stick_max = f.stick_max.max(m.speed);
        if m.speed <= 0.0 {
            f.idle += 1;
        }
    }

    /// The same, with the last walk ([`Step::Nav`]) ending within `within`
    /// of its goal instead of [`GOAL`].
    pub fn arrive_within(mut self, within: f32) -> OrderRun {
        self.goal = within;
        self
    }

    /// The walk under way, while a walking step is.
    pub fn walking(&self) -> Option<&RouteWalk> {
        match &self.state {
            State::Walk(w) => Some(w),
            _ => None,
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
        self.log.push(Logged {
            tick,
            step,
            what,
            at,
        });
    }

    pub fn status(&self) -> OrderStatus {
        let (phase, tries) = match &self.state {
            State::Fresh => ("", 0),
            State::Walk(_) => ("walking", 0),
            State::Break(_) => ("break", 0),
            State::Prime(p) => (p.phase(), p.tries()),
            State::Bounce(b) => (if b.airborne() { "air" } else { "settle" }, b.tries + 1),
            State::Drop(d) => (
                match d {
                    Dropping::Line(_) => "line_up",
                    Dropping::Run(_) => "run_off",
                    Dropping::Jump(_) => "jump",
                    Dropping::Air { .. } => "air",
                },
                self.drops + 1,
            ),
            State::Chain { .. } | State::Fly { .. } => ("air", 0),
        };
        OrderStatus {
            step: self.at,
            steps: self.plan.steps.len(),
            what: self.plan.steps.get(self.at).map_or("done", Step::name),
            phase,
            tries,
            chain: self.chain,
            primed: self.primed,
            read: self.read,
            landing: self.landing,
        }
    }

    fn fail(&mut self, why: &'static str, tick: u32, at: Vec3) -> OrderOut {
        self.note(tick, why, at);
        self.end_flight(why);
        self.primed = false;
        OrderOut {
            motion: RouteMotion::default(),
            done: Some(Err(why)),
        }
    }

    fn next(&mut self, tick: u32, at: Vec3) {
        self.note(tick, "done", at);
        self.replans = 0;
        self.at += 1;
        self.state = State::Fresh;
    }

    /// Back to step `i` (a walk), starting it afresh.
    fn back_to(&mut self, i: usize, tick: u32, at: Vec3, why: &'static str) {
        self.note(tick, why, at);
        self.replans = 0;
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
            // A chain's link: once the last touchdown's speed out is read,
            // against the link's target.
            Some(Step::Chain(s)) => {
                if self.watch.open.is_none() {
                    self.watching = Some(*s);
                }
            }
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
            let result = t.result(site.need);
            self.note(tick, result, t.at);
            if let Some(f) = self.flight.as_mut().filter(|f| f.landing.is_none()) {
                f.landing = Some(t);
                f.end = Some(result);
            }
        }
        // The word read as something else on the ground: it was lost
        // (the pair was rebuilt). Back to the crouch spot.
        // Settling on the stand before the jump counts: a jump with the
        // word lost is a landing that cannot bounce (`not_primed`).
        let grounded = match &self.state {
            State::Walk(_) => true,
            State::Bounce(b) => !b.airborne(),
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
                self.end_flight("no_landing");
                return OrderOut {
                    motion: RouteMotion::default(),
                    done: Some(Ok(())),
                };
            };
            if matches!(self.state, State::Fresh) {
                self.note(tick, "start", at);
            }
            // The walk onto a bounce or a drop is the approach.
            let approach = matches!(
                self.plan.steps.get(self.at + 1),
                Some(Step::Bounce(_) | Step::Drop { .. })
            );
            let speed = if approach {
                self.tune.approach_speed
            } else {
                1.0
            };
            let out = match step {
                Step::Walk { to, crouch } => {
                    self.walk(map, at, tick, to, crouch, ARRIVED, speed, true)
                }
                Step::Nav { to } => self.walk(map, at, tick, to, false, self.goal, 1.0, false),
                Step::Prime(c) => self.prime(map, c, at, tick, pair),
                Step::Bounce(site) => self.bounce(site, at, tick),
                Step::Drop { from, site } => self.drop(from, site, at, tick, vel),
                Step::Chain(site) => self.chain(site, at, tick, vel, pair),
                Step::Fly { to } => self.fly(to, at, tick, vel),
            };
            match out {
                Ok(Some(mut motion)) => {
                    if self.primed {
                        // Never crouch between the prime and the bounce.
                        motion.crouch = false;
                    }
                    self.track_flight(at, vel, pair, tick, &motion);
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
        speed: f32,
        near: bool,
    ) -> Result<Option<RouteMotion>, &'static str> {
        // A walk to a crouch spot's start ducks under a low ceiling: the
        // prime crouches next anyway.
        let duck = self.leg() == Leg::Crouch;
        let begin = |avoid: &[Vec3]| -> Result<RouteWalk, &'static str> {
            let w = RouteWalk::new_avoiding(map, at, to, crouch, tick, avoid)?;
            let w = if duck { w.ducking() } else { w };
            // A walk to a prime's start or a bounce's approach: the next
            // step steers the rest of the way itself.
            Ok(if near { w.ending_near() } else { w })
        };
        if matches!(self.state, State::Fresh) {
            self.state = State::Walk(begin(&self.avoid)?);
        }
        let State::Walk(w) = &mut self.state else {
            return Err("state");
        };
        let out = w.step(map, at, tick, tolerance, speed);
        let link = match out {
            Err("path_lost_jump") => w.failed_link(),
            _ => None,
        };
        match out {
            Err(lost) if lost.starts_with("path_lost") => {
                if self.replans < WALK_REPLANS {
                    // Plan the walk again from here, around a jump or drop
                    // link it could not make (a rail the mesh lacks).
                    self.replans += 1;
                    self.note(tick, lost, at);
                    self.avoid.extend(link);
                    self.state = State::Walk(begin(&self.avoid)?);
                    return Ok(Some(RouteMotion::default()));
                }
                Err(lost_on(lost, self.leg()))
            }
            out => out,
        }
    }

    /// Which leg the walk under way is: to a crouch spot (a prime next),
    /// to a bounce's approach (a bounce or a drop next), or another walk.
    fn leg(&self) -> Leg {
        match self.plan.steps.get(self.at + 1) {
            Some(Step::Prime(_)) => Leg::Crouch,
            Some(Step::Bounce(_) | Step::Drop { .. }) => Leg::Approach,
            _ => Leg::Nav,
        }
    }

    fn prime(
        &mut self,
        map: &MapData,
        c: Crouch,
        at: Vec3,
        tick: u32,
        pair: Option<u32>,
    ) -> Result<Option<RouteMotion>, &'static str> {
        if matches!(self.state, State::Fresh) {
            let pieces = glass_in_the_way(map, &c, self.glass.as_ref());
            if pieces.is_empty() {
                self.state = State::Prime(prime_run(&c, tick));
            } else {
                self.note(tick, "break_glass", at);
                self.state = State::Break(Breaking {
                    since: tick,
                    pieces,
                    aim: None,
                });
            }
        }
        let glass = self.glass;
        let broke = match &mut self.state {
            State::Break(b) => Some(break_glass(b, at, tick, glass.as_ref())),
            _ => None,
        };
        match broke {
            Some(Ok(None)) => {
                self.note(tick, "glass_broken", at);
                self.state = State::Prime(prime_run(&c, tick));
                return Ok(Some(RouteMotion::default()));
            }
            Some(out) => return out,
            None => {}
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
            let f = flown(&site, site.bounce.from(), &self.tune);
            let mut b = site.bounce;
            b.stand = f.target;
            let tune = BounceTuning {
                drift: f.drift,
                still_ticks: f.still,
                ..self.tune
            };
            self.state =
                State::Bounce(BounceRun::new(b, self.tune.bounce_tries, tick, seed).tuned(tune));
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

    /// Off the launch floor at `from` and down onto the site's target, as
    /// [`flown`] says (the site's learned knobs, else the route's own: the
    /// way [`drop_takeoff`] picks): line up [`Flown::back`] ([`RUN_UP`])
    /// behind `from`, run along the heading at the takeoff's stick,
    /// steered back onto the line through `from`, and either run off the
    /// edge or, a running jump, press jump short of `from` by
    /// [`Flown::early`] ([`TAKEOFF_EARLY`]) and the ground the body covers
    /// in [`INPUT_LAG`], holding the run until the body leaves the floor.
    /// In the air steer the landing onto the target
    /// ([`OrderRun::drop_air`]). Then see whether it launched.
    /// Why the drop from `from` onto `site`, flown as `f`, is not taken:
    /// `takeoff_rejected` when the bounce rule ([`judge_takeoff`], on the
    /// map's floors) says it will not bounce. `None` with no floors seen,
    /// or when the flight model has no takeoff for it (the run finds out).
    fn takeoff_rejected(&self, from: Vec3, site: &Site, f: &Flown) -> Option<&'static str> {
        if self.floors.is_empty() {
            return None;
        }
        let t = takeoff(from, site, f.stick, f.jump())?;
        let v = judge_takeoff(from, site, &t, &self.floors)?;
        (!v.outcome.bounces()).then_some("takeoff_rejected")
    }

    fn drop(
        &mut self,
        from: Vec3,
        site: Site,
        at: Vec3,
        tick: u32,
        vel: Option<Vec3>,
    ) -> Result<Option<RouteMotion>, &'static str> {
        if matches!(self.state, State::Fresh) {
            self.state = State::Drop(Dropping::Line(tick));
        }
        let t = self.tune;
        let mut f = flown(&site, from, &t);
        if !self.floors.is_empty() {
            // Fly to the same edge-adjusted aim the rule judges, not back
            // to the target's original point away from the edge.
            let near = rule::floors_near(&self.floors, [f.target[0], f.target[1]], FLOOR_REACH);
            f.target = edge_aim(f.target, &near).0;
        }
        // Lined up: the bounce rule judges the takeoff before the run.
        if let State::Drop(Dropping::Line(since)) = self.state {
            let lined_up = dist_xy(f.run_up(from), at) <= t.line_up
                || tick.wrapping_sub(since) > t.line_up_ticks;
            if lined_up && let Some(why) = self.takeoff_rejected(from, &site, &f) {
                return Err(why);
            }
        }
        let State::Drop(d) = &mut self.state else {
            return Err("state");
        };
        let heading = f.heading;
        let face = Some(heading[1].atan2(heading[0]));
        let motion = |dir: [f32; 2], speed: f32, jump: bool| RouteMotion {
            dir,
            speed,
            yaw: face,
            jump,
            crouch: false,
        };
        let v = vel.unwrap_or([0.0; 3]);
        // The run: at the takeoff's stick, steered back onto the line
        // through `from`.
        let run = |at: Vec3| {
            let side = [-heading[1], heading[0]];
            let lateral = (from[0] - at[0]) * side[0] + (from[1] - at[1]) * side[1];
            let lateral = (2.0 * lateral).clamp(-0.6, 0.6);
            let (dir, _) = norm([
                heading[0] + side[0] * lateral,
                heading[1] + side[1] * lateral,
            ]);
            motion(dir, f.stick.max(MIN_STICK), false)
        };
        let on_floor = |at: Vec3| (at[2] - from[2]).abs() < LEFT_FLOOR && v[2].abs() < FALLING;
        match *d {
            Dropping::Line(since) => {
                let back = f.run_up(from);
                let (to_back, off) = norm([back[0] - at[0], back[1] - at[1]]);
                if off <= t.line_up || tick.wrapping_sub(since) > t.line_up_ticks {
                    *d = Dropping::Run(tick);
                    return Ok(Some(run(at)));
                }
                Ok(Some(motion(
                    to_back,
                    (off * 4.0).clamp(MIN_STICK, 0.6),
                    false,
                )))
            }
            Dropping::Run(since) => {
                if at[2] < from[2] - LEFT_FLOOR {
                    // Ran off (or, a jump, ran off before its press).
                    *d = Dropping::Air {
                        since: tick,
                        top: at[2],
                        low: None,
                    };
                    return Ok(Some(self.drop_air(&f, at, v)));
                }
                let past = (at[0] - from[0]) * heading[0] + (at[1] - from[1]) * heading[1];
                let have = (v[0] * heading[0] + v[1] * heading[1]).max(0.0);
                let lead = f.early + have * INPUT_LAG as f32 / TICKS;
                if f.jump() && past >= -lead {
                    *d = Dropping::Jump(tick);
                    let mut m = run(at);
                    m.jump = true;
                    return Ok(Some(m));
                }
                if tick.wrapping_sub(since) > RUN_OFF_TICKS {
                    return Err("no_drop");
                }
                Ok(Some(run(at)))
            }
            Dropping::Jump(since) => {
                let held = tick.wrapping_sub(since);
                if held < t.jump_ticks {
                    let mut m = run(at);
                    m.jump = true;
                    return Ok(Some(m));
                }
                // The body acts on the press late: keep running until it
                // leaves the floor.
                if on_floor(at) && held < t.jump_ticks + 3 * INPUT_LAG {
                    return Ok(Some(run(at)));
                }
                *d = Dropping::Air {
                    since,
                    top: at[2],
                    low: None,
                };
                Ok(Some(self.drop_air(&f, at, v)))
            }
            Dropping::Air { since, top, low } => {
                let top = top.max(at[2]);
                // The lowest point once falling: a jump rises first.
                let low = if at[2] < top - DROP_FALLING {
                    Some(low.map_or(at[2], |l: f32| l.min(at[2])))
                } else {
                    low
                };
                *d = Dropping::Air { since, top, low };
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
                    return if self.drops < t.drop_tries {
                        Err("drop_again")
                    } else {
                        Err("no_bounce")
                    };
                }
                Ok(Some(self.drop_air(&f, at, v)))
            }
        }
    }

    /// The stick in the air ([`air_stick`]): where the landing comes down
    /// with the sideways speed held, from where the body will be once the
    /// stick acts ([`INPUT_LAG`] on) and the fall left then, against the
    /// target; [`Flown::gain`] (the tuning's `air_gain`) and
    /// [`Flown::letgo`].
    fn drop_air(&self, f: &Flown, at: Vec3, v: Vec3) -> RouteMotion {
        let target = f.target;
        let lag = INPUT_LAG as f32 / TICKS;
        let rem = fall_time(at[2] - target[2], v[2]).unwrap_or(0.0);
        let rem_after = (rem - lag).max(1.0 / TICKS);
        let ahead = rem.min(lag);
        let lands = [
            at[0] + v[0] * (ahead + rem_after),
            at[1] + v[1] * (ahead + rem_after),
        ];
        let miss = [target[0] - lands[0], target[1] - lands[1]];
        let (dir, speed) = air_stick([v[0], v[1]], miss, rem_after, f.gain, f.letgo);
        let (to_target, _) = norm([target[0] - at[0], target[1] - at[1]]);
        RouteMotion {
            dir,
            speed,
            yaw: (to_target != [0.0; 2]).then(|| to_target[1].atan2(to_target[0])),
            jump: false,
            crouch: false,
        }
    }

    /// A chain's next bounce ([`Step::Chain`]): from the last launch, in
    /// the air, steer the landing onto `site`'s target as a drop's air does
    /// ([`OrderRun::drop_air`]), and see whether it launches again. Over
    /// (the next step) either way: a link that comes down without
    /// launching ends the chain, and the links after it are passed over.
    fn chain(
        &mut self,
        site: Site,
        at: Vec3,
        tick: u32,
        vel: Option<Vec3>,
        pair: Option<u32>,
    ) -> Result<Option<RouteMotion>, &'static str> {
        let v = vel.unwrap_or([0.0; 3]);
        let f = flown(&site, site.bounce.stand, &self.tune);
        if matches!(self.state, State::Fresh) {
            if self.chain_broken {
                return Ok(None);
            }
            // The bounce before is still reading its speed out: steer, and
            // begin the link once its touchdown is filed with its flight.
            if self.watch.open.is_some() {
                return Ok(Some(self.drop_air(&f, at, v)));
            }
            self.chain.0 += 1;
            // The flight before ended in its bounce: this link's is a new
            // one, in the air already.
            self.end_flight("bounce");
            self.flight = Some(Leap::new(tick, at, v, pair));
            self.was_air = true;
            self.state = State::Chain {
                since: tick,
                top: at[2],
                low: None,
            };
        }
        let State::Chain { since, top, low } = &mut self.state else {
            return Err("state");
        };
        let since = *since;
        *top = top.max(at[2]);
        if at[2] < *top - DROP_FALLING {
            *low = Some(low.map_or(at[2], |l: f32| l.min(at[2])));
        }
        if low.is_some_and(|l| at[2] > l + LAUNCH_RISE) {
            // Down on the link and thrown up again.
            self.chain.1 += 1;
            return Ok(None);
        }
        let landed = self.landing.is_some_and(|t| t.tick > since && !t.bounced());
        if landed || tick.wrapping_sub(since) > FLY_TICKS {
            self.chain_broken = true;
            return Ok(None);
        }
        Ok(Some(self.drop_air(&f, at, v)))
    }

    fn fly(
        &mut self,
        to: Vec3,
        at: Vec3,
        tick: u32,
        vel: Option<Vec3>,
    ) -> Result<Option<RouteMotion>, &'static str> {
        if matches!(self.state, State::Fresh) {
            // A chain that came down without launching is down already.
            if self.chain_broken {
                return Ok(None);
            }
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
            speed: self.tune.fly_speed,
            yaw: Some(dir[1].atan2(dir[0])),
            jump: false,
            crouch: false,
        }))
    }
}

#[cfg(test)]
mod tests;
