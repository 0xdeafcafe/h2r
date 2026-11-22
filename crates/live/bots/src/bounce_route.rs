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

// ---- the map's walks and launch floors ----

/// How far sideways (world units) a launch floor may be from the slot it
/// drops onto. **[guess]**: a 2.4-unit drop takes about a second, and the
/// stick moves a bot about 2.5 units a second in the air.
pub const LEDGE_REACH: f32 = 3.0;
/// A launch floor is at most this much (world units) under the drop
/// height, and at most [`LEDGE_OVER`] over it.
pub const LEDGE_UNDER: f32 = 0.2;
pub const LEDGE_OVER: f32 = 2.5;

/// A launch floor for a site that needs a drop: the nav mesh's point
/// nearest the site's target sideways, within [`LEDGE_REACH`], whose floor
/// is the drop height over the target, to [`LEDGE_UNDER`] below and
/// [`LEDGE_OVER`] above it. The [`Ledge`] a plan is made with.
pub fn launch_floor(m: &MapData, s: &Site) -> Option<Vec3> {
    let t = s.target();
    let over = [t[0], t[1], t[2] + s.drop_height()];
    m.nav
        .nearest(over, LEDGE_REACH, LEDGE_OVER, LEDGE_UNDER)
        .map(|(_, p)| p)
}

/// Where a drop onto `s` is taken from: the spot's own takeoff (a
/// measured recipe's, [`crate::glitches::Bounce::takeoff`]), else what
/// `ledge` finds ([`launch_floor`] in play).
pub fn launch_from(s: &Site, ledge: Ledge) -> Option<Vec3> {
    s.bounce.takeoff.or_else(|| ledge(s))
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
    /// Off the launch floor at `from` (a running jump when a walk-off
    /// would land too slow) and down onto the site, aiming at its target,
    /// until it launches.
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
/// A drop is falling once this far (world units) under the top of its
/// flight (a jump off the floor rises first).
pub const DROP_FALLING: f32 = 0.2;
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

/// Gravity on a falling body (world units a second squared): the
/// superbounce scan's fit of the recipe (`superbounce::bounce`'s
/// `GRAVITY`), and what the round-3 bounce-practice runs show: a standing
/// jump's vertical speed goes from +3.08 to -2.90 in 43 ticks
/// (docs/game/notes/2026-11-21-bot-bounce-sim.md).
pub const FALL_GRAVITY: f32 = 4.15;
/// A jump's vertical speed on its first tick (world units a second): the
/// same runs, every jump.
pub const JUMP_VZ: f32 = 3.08;
/// The ground speed at full stick (world units a second): the same runs,
/// a bot walking on flat floor at a stick of 1 (about 2).
pub const RUN_SPEED: f32 = 2.0;
/// The smallest stick that moves the body: past the move stick's dead
/// zone (`nav::route`'s 0.15). The runs' standing jumps held 0.15 of
/// stick toward the drift in the air and gained no sideways speed at all.
pub const MIN_STICK: f32 = 0.2;
/// A running jump starts this far (world units) behind its launch point.
pub const RUN_UP: f32 = 0.6;
/// The jump is pressed this far (world units) short of the launch point.
pub const TAKEOFF_EARLY: f32 = 0.05;
/// In the air the stick lets go when the landing, as it flies, comes down
/// this near the target (world units): the recipe's "easing off the stick
/// in the air".
pub const AIR_DEAD: f32 = 0.03;

/// How long (seconds) a fall from `vz` (up positive) takes to come down
/// `height` (world units, positive down), under [`FALL_GRAVITY`]. `None`
/// when it never gets there.
pub fn fall_time(height: f32, vz: f32) -> Option<f32> {
    let d = vz * vz + 2.0 * FALL_GRAVITY * height;
    (d >= 0.0)
        .then(|| (vz + d.sqrt()) / FALL_GRAVITY)
        .filter(|t| *t > 0.0)
}

/// Whether a drop from `from` onto `site` jumps: a walk off the floor
/// lands at `sqrt(2 g h)`, slower than the site needs. The scan's falls
/// assume a jump (`superbounce::bounce`: a standing jump lands at 3), and
/// so does the measured recipe (a running jump, a 2.43 drop, -5.4).
pub fn drop_jumps(from: Vec3, site: &Site) -> bool {
    let h = (from[2] - site.target()[2]).max(0.0);
    site.need > 0.0 && (2.0 * FALL_GRAVITY * h).sqrt() < site.need
}

/// The ground speed (world units a second) a running jump from `from`
/// takes to come down on `to`: the gap over the flight's time.
pub fn takeoff_speed(from: Vec3, to: Vec3) -> f32 {
    let gap = dist_xy(from, to);
    fall_time(from[2] - to[2], JUMP_VZ).map_or(0.0, |t| gap / t)
}

/// The stick for a ground speed (world units a second): 0 for none, else
/// at least [`MIN_STICK`].
fn stick_for(speed: f32) -> f32 {
    if speed <= 0.01 {
        0.0
    } else {
        (speed / RUN_SPEED).clamp(MIN_STICK, 1.0)
    }
}

/// A drop's phase.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Dropping {
    /// Walking to the run-up (a jump: [`RUN_UP`] behind the launch point;
    /// a walk-off: the launch point), from the tick.
    Line(u32),
    /// Running at the launch point toward the target, from the tick.
    Run(u32),
    /// The jump held, from the tick.
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
    Prime(PrimeRun),
    Bounce(BounceRun),
    Drop(Dropping),
    Fly {
        since: u32,
        peaked: bool,
        still: u32,
    },
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

/// The vertical speed off the floor is the best of this many ticks
/// (ticks) from the takeoff: the press is on the ground.
pub const TAKEOFF_TICKS: u32 = 6;
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
    /// The walk under way planned again ([`WALK_REPLANS`]).
    replans: u32,
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
            State::Drop(Dropping::Jump(_) | Dropping::Air { .. }) => true,
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
            let width = if site.width > 0.0 {
                site.width
            } else {
                crate::spots::SLOT_WIDTH
            };
            let result = t.result(width, site.need);
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
                Step::Prime(c) => self.prime(c, at, tick, pair),
                Step::Bounce(site) => self.bounce(site, at, tick),
                Step::Drop { from, site } => self.drop(from, site, at, tick, vel),
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

    #[allow(clippy::too_many_arguments)]
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
        if matches!(self.state, State::Fresh) {
            let w = RouteWalk::new(map, at, to, crouch, tick)?;
            // A walk to a prime's start or a bounce's approach: the next
            // step steers the rest of the way itself.
            self.state = State::Walk(if near { w.ending_near() } else { w });
        }
        let State::Walk(w) = &mut self.state else {
            return Err("state");
        };
        match w.step(map, at, tick, tolerance, speed) {
            Err(lost) if lost.starts_with("path_lost") => {
                if self.replans < WALK_REPLANS {
                    // Plan the walk again from here.
                    self.replans += 1;
                    self.note(tick, lost, at);
                    let w = RouteWalk::new(map, at, to, crouch, tick)?;
                    self.state = State::Walk(if near { w.ending_near() } else { w });
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
            self.state = State::Bounce(BounceRun::new(b, self.tune.bounce_tries, tick, seed));
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

    /// Off the launch floor at `from` and down onto the site's target. A
    /// drop that needs a jump ([`drop_jumps`]) is the recipe's: line up
    /// [`RUN_UP`] behind `from`, run at the target at the speed that
    /// carries the flight onto it ([`takeoff_speed`]), jump at `from`, and
    /// in the air steer the landing, as it flies, onto the target, easing
    /// off once it is on it ([`AIR_DEAD`]). Else walk off at the tuning's
    /// `run_off_speed` and steer the same way. Then see whether it launched.
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
        let State::Drop(d) = &mut self.state else {
            return Err("state");
        };
        let t = self.tune;
        let target = site.target();
        let jumps = drop_jumps(from, &site);
        let (heading, _) = norm([target[0] - from[0], target[1] - from[1]]);
        let face = Some(heading[1].atan2(heading[0]));
        let motion = |dir: [f32; 2], speed: f32, jump: bool| RouteMotion {
            dir,
            speed,
            yaw: face,
            jump,
            crouch: false,
        };
        let v = vel.unwrap_or([0.0; 3]);
        // The run: at the speed that carries the flight onto the target,
        // steered back onto the line through `from`.
        let run = |at: Vec3| {
            if !jumps {
                return motion(heading, t.run_off_speed, false);
            }
            let want = takeoff_speed(from, target);
            let have = v[0] * heading[0] + v[1] * heading[1];
            let side = [-heading[1], heading[0]];
            let lateral = (from[0] - at[0]) * side[0] + (from[1] - at[1]) * side[1];
            let lateral = (2.0 * lateral).clamp(-0.6, 0.6);
            let (dir, _) = norm([
                heading[0] + side[0] * lateral,
                heading[1] + side[1] * lateral,
            ]);
            motion(dir, stick_for(want + 0.5 * (want - have)), false)
        };
        match *d {
            Dropping::Line(since) => {
                let back = if jumps {
                    [
                        from[0] - heading[0] * RUN_UP,
                        from[1] - heading[1] * RUN_UP,
                        from[2],
                    ]
                } else {
                    from
                };
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
                    // Walked off (or, a jump, ran off before the launch point).
                    *d = Dropping::Air {
                        since: tick,
                        top: at[2],
                        low: None,
                    };
                    return Ok(Some(self.drop_air(target, at, v)));
                }
                let past = (at[0] - from[0]) * heading[0] + (at[1] - from[1]) * heading[1];
                if jumps && past >= -TAKEOFF_EARLY {
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
                if tick.wrapping_sub(since) < t.jump_ticks {
                    let mut m = run(at);
                    m.jump = true;
                    return Ok(Some(m));
                }
                *d = Dropping::Air {
                    since,
                    top: at[2],
                    low: None,
                };
                Ok(Some(self.drop_air(target, at, v)))
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
                Ok(Some(self.drop_air(target, at, v)))
            }
        }
    }

    /// The stick in the air: toward where the landing must be from where
    /// it comes down as it flies (the remaining fall at the vertical speed
    /// now, the sideways speed held), `air_gain` a unit off, none once
    /// within [`AIR_DEAD`].
    fn drop_air(&self, target: Vec3, at: Vec3, v: Vec3) -> RouteMotion {
        let rem = fall_time(at[2] - target[2], v[2]).unwrap_or(0.0).max(0.1);
        let lands = [at[0] + v[0] * rem, at[1] + v[1] * rem];
        let (dir, off) = norm([target[0] - lands[0], target[1] - lands[1]]);
        let speed = if off < AIR_DEAD {
            0.0
        } else {
            (off * self.tune.air_gain).clamp(MIN_STICK, 1.0)
        };
        let (to_target, _) = norm([target[0] - at[0], target[1] - at[1]]);
        RouteMotion {
            dir,
            speed,
            yaw: (to_target != [0.0; 2]).then(|| to_target[1].atan2(to_target[0])),
            jump: false,
            crouch: false,
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
            speed: self.tune.fly_speed,
            yaw: Some(dir[1].atan2(dir[0])),
            jump: false,
            crouch: false,
        }))
    }
}

#[cfg(test)]
mod tests;
