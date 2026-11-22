//! An autopilot for a seat that has no brain of its own (docs/game/bots.md,
//! "Driving a player"): the local player, sent somewhere by an agent
//! (`player.goto`, docs/runtime/agent-api.md). It is the bots' route
//! machinery with a watchdog round it, not a second one: the plan comes
//! from [`crate::bounce_route`] (a walk, [`bounce_route::prime_steps`] or
//! [`bounce_route::plan_site`]), it is played by [`OrderRun`] over the nav
//! mesh's [`crate::nav::RouteWalk`], and its motion is what a bot's pad
//! would be given.
//!
//! What it adds is for a body the bots' brain does not own: a walk that
//! strays more than [`STRAY`] from its corridor, makes no progress for
//! [`STUCK_TICKS`], or that the follower gives up on (`path_lost_*`) is
//! planned again from where the body is, up to [`REPLANS`] times; the
//! whole goto gives up after its timeout. A jump is asked for as a fresh
//! press ([`Goto::step`] never holds A two ticks running).
//!
//! Pure: positions and the pair word in, motion out. Time in ticks; lengths
//! in world units.

use crate::bounce_route::{self, OrderRun, Plan, Step, Why};
use crate::brain::Vec3;
use crate::cache::MapData;
use crate::glitches::prime::is_primed;
use crate::mind::RouteMotion;
use crate::nav::{Kind, Waypoint};
use crate::spots::{Crouch, Known, Site, dist_xy};

/// A point counts as reached this near it, unless the asker says.
pub const WITHIN: f32 = 0.3;
/// The nearest and farthest a point's `within` may be.
pub const WITHIN_MIN: f32 = 0.05;
pub const WITHIN_MAX: f32 = 10.0;
/// A goto gives up after this long unless the asker says: 2 minutes.
pub const TIMEOUT: u32 = 3_600;
/// Further from its corridor than this (sideways) the walk plans again.
pub const STRAY: f32 = 1.5;
/// A walk whose distance left has not come down by [`STUCK_GAIN`] in
/// this long plans again.
pub const STUCK_TICKS: u32 = 150;
pub const STUCK_GAIN: f32 = 0.5;
/// Plans made again before the goto gives up.
pub const REPLANS: u32 = 6;
/// The most path points [`Goto::status`] gives.
pub const PATH_POINTS: usize = 32;

/// Where to go.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    /// A point on the nav mesh.
    Point(Vec3),
    /// A crouch spot: its start ([`bounce_route::prime_start`]).
    Crouch(Crouch),
    /// A bounce site: its approach ([`bounce_route::approach`]).
    Site(Site),
}

impl Target {
    pub fn kind(&self) -> &'static str {
        match self {
            Target::Point(_) => "point",
            Target::Crouch(_) => "crouch",
            Target::Site(_) => "bounce",
        }
    }
}

/// What to do on arriving.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Then {
    /// Stand there.
    #[default]
    Stop,
    /// Prime: at a crouch spot, its recipe; at a bounce site, prime at the
    /// crouch spot the planner picks for it and walk to its approach.
    CrouchRecipe,
    /// A bounce site's whole route: prime (unless primed), walk to the
    /// approach, jump or drop onto it, ride the launch down.
    Bounce,
}

impl Then {
    pub fn parse(s: &str) -> Option<Then> {
        match s {
            "stop" => Some(Then::Stop),
            "crouch_recipe" => Some(Then::CrouchRecipe),
            "bounce" => Some(Then::Bounce),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Then::Stop => "stop",
            Then::CrouchRecipe => "crouch_recipe",
            Then::Bounce => "bounce",
        }
    }
}

/// The ask.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ask {
    pub target: Target,
    /// How near a [`Target::Point`] counts as there.
    pub within: f32,
    pub then: Then,
    /// Ticks before it gives up.
    pub timeout: u32,
}

impl Ask {
    /// An ask with the defaults: [`WITHIN`], [`TIMEOUT`].
    pub fn new(target: Target, then: Then) -> Ask {
        Ask {
            target,
            within: WITHIN,
            then,
            timeout: TIMEOUT,
        }
    }

    /// Whether `then` makes sense at the target: a recipe needs a spot, a
    /// bounce a bounce site.
    pub fn check(&self) -> Result<(), &'static str> {
        match (self.target, self.then) {
            (Target::Point(_), Then::CrouchRecipe | Then::Bounce) => {
                Err("then: crouch_recipe and bounce need a spot, not a point")
            }
            (Target::Crouch(_), Then::Bounce) => Err("then: bounce needs a bounce spot"),
            _ => Ok(()),
        }
    }
}

/// The plan for `ask` from `from`: a walk to a point; for a crouch spot,
/// the walk to its start and, for its recipe, the prime; for a bounce
/// site, the walk to its approach, or the bots' route by it
/// ([`bounce_route::plan_site`]), cut after the walk to the approach for
/// the recipe alone. `Err` names why there is none.
pub fn plan(
    ask: &Ask,
    from: Vec3,
    primed: bool,
    known: &Known,
    map: &MapData,
) -> Result<Plan, &'static str> {
    ask.check()?;
    let walk = |a: Vec3, b: Vec3| bounce_route::path_length(map, a, b);
    let ledge = |s: &Site| bounce_route::launch_floor(map, s);
    let asked = |steps: Vec<Step>, site: Option<Site>| Plan {
        steps,
        why: Why::Asked,
        site,
        cost: None,
    };
    match (ask.target, ask.then) {
        (Target::Point(to), _) => Ok(Plan {
            steps: vec![Step::Nav { to }],
            why: Why::Plain,
            site: None,
            cost: None,
        }),
        (Target::Crouch(c), then) => {
            let [walk_there, prime] = bounce_route::prime_steps(&c);
            Ok(asked(
                match then {
                    Then::Stop => vec![walk_there],
                    _ => vec![walk_there, prime],
                },
                None,
            ))
        }
        (Target::Site(s), Then::Stop) => {
            let to = bounce_route::approach(&s, &ledge).ok_or("no_launch_floor")?;
            Ok(asked(vec![Step::Walk { to, crouch: false }], Some(s)))
        }
        (Target::Site(s), then) => {
            let mut p =
                bounce_route::plan_site(from, primed, s, known, &walk, &ledge).ok_or("no_route")?;
            if then == Then::CrouchRecipe {
                // Up to the walk to the approach: the bounce is the asker's.
                if let Some(i) = p
                    .steps
                    .iter()
                    .position(|st| matches!(st, Step::Bounce(_) | Step::Drop { .. }))
                {
                    p.steps.truncate(i);
                }
            }
            Ok(p)
        }
    }
}

/// The length of the walk from `at` through `ahead`.
pub fn remaining(at: Vec3, ahead: &[Waypoint]) -> f32 {
    let mut from = at;
    let mut total = 0.0;
    for w in ahead {
        total += dist3(from, w.at);
        from = w.at;
    }
    total
}

/// How far `at` is (sideways) from the leg it walks: from the waypoint
/// passed, or `anchor` (where the walk began) when none is, to the next
/// one. 0 off a walking leg: a jump, a drop or a lift has its own line.
pub fn off_path(at: Vec3, behind: Option<Waypoint>, anchor: Vec3, ahead: &[Waypoint]) -> f32 {
    let Some(next) = ahead.first() else {
        return 0.0;
    };
    if next.kind != Kind::Walk || behind.is_some_and(|b| b.kind != Kind::Walk) {
        return 0.0;
    }
    let a = behind.map_or(anchor, |b| b.at);
    segment_xy(at, a, next.at)
}

fn segment_xy(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let (ex, ey) = (b[0] - a[0], b[1] - a[1]);
    let len2 = ex * ex + ey * ey;
    let t = if len2 > 1e-9 {
        (((p[0] - a[0]) * ex + (p[1] - a[1]) * ey) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (a[0] + t * ex - p[0]).hypot(a[1] + t * ey - p[1])
}

fn dist3(a: Vec3, b: Vec3) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// What a goto asks of the pad this tick.
#[derive(Clone, Copy, Debug, Default)]
pub struct Out {
    pub motion: RouteMotion,
    /// Over: `Ok` arrived (and did what it was asked), `Err` why not.
    pub done: Option<Result<(), &'static str>>,
}

/// Where a goto stands.
#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    /// `running`, `done` or `failed`.
    pub state: &'static str,
    /// The plan's steps (`bounce_route::Step::name`); empty between plans.
    pub steps: Vec<&'static str>,
    pub step: usize,
    /// The step's phase (`bounce_route::OrderStatus::phase`).
    pub phase: &'static str,
    /// The level pair word is at -3 (read, or assumed after a prime).
    pub primed: bool,
    /// Where the walk under way goes: the body, then the waypoints ahead
    /// (at most [`PATH_POINTS`]); empty when not walking.
    pub path: Vec<Vec3>,
    /// World units left along that path; `None` when not walking.
    pub distance: Option<f32>,
    pub replans: u32,
    /// Why it last planned again: `strayed`, `stuck`, or the route's
    /// `path_lost_<check>_<leg>` ([`bounce_route::lost_on`]), `stalled`,
    /// `pair_lost`.
    pub replanned: Option<&'static str>,
    pub error: Option<&'static str>,
}

/// A goto being played.
#[derive(Clone, Debug)]
pub struct Goto {
    pub ask: Ask,
    run: Option<OrderRun>,
    started: u32,
    seed: u64,
    replans: u32,
    replanned: Option<&'static str>,
    /// The step and where the walk under way began, for [`off_path`].
    walk: Option<(usize, Vec3)>,
    /// The least distance left in the walk under way, and when.
    best: f32,
    best_tick: u32,
    /// The run had primed, for a plan made again with no word to read.
    primed: bool,
    jumped: bool,
    end: Option<Result<(), &'static str>>,
}

impl Goto {
    pub fn new(ask: Ask, tick: u32, seed: u64) -> Goto {
        Goto {
            ask,
            run: None,
            started: tick,
            seed,
            replans: 0,
            replanned: None,
            walk: None,
            best: f32::INFINITY,
            best_tick: tick,
            primed: false,
            jumped: false,
            end: None,
        }
    }

    /// The run, once planned.
    pub fn run(&self) -> Option<&OrderRun> {
        self.run.as_ref()
    }

    pub fn ended(&self) -> Option<Result<(), &'static str>> {
        self.end
    }

    fn finish(&mut self, r: Result<(), &'static str>) -> Out {
        self.end = Some(r);
        self.run = None;
        Out {
            motion: RouteMotion::default(),
            done: Some(r),
        }
    }

    /// Plans again from where the body is next tick, or gives up.
    fn replan(&mut self, why: &'static str) -> Out {
        if let Some(r) = &self.run {
            self.primed = r.primed();
        }
        if self.replans >= REPLANS {
            return self.finish(Err(match why {
                "strayed" => "stuck",
                w => w,
            }));
        }
        self.replans += 1;
        self.replanned = Some(why);
        self.run = None;
        self.walk = None;
        Out::default()
    }

    /// The watchdog, while a walk is under way: why to plan again, if it
    /// strayed from its corridor or has stopped getting nearer.
    fn watch(&mut self, at: Vec3, tick: u32) -> Option<&'static str> {
        let run = self.run.as_ref()?;
        let Some(w) = run.walking() else {
            self.walk = None;
            self.best_tick = tick;
            return None;
        };
        let step = run.status().step;
        let f = w.follower();
        let anchor = match self.walk {
            Some((s, a)) if s == step => a,
            _ => {
                self.walk = Some((step, at));
                self.best = f32::INFINITY;
                self.best_tick = tick;
                at
            }
        };
        let left = remaining(at, f.ahead());
        // A lift or a teleporter keeps its own time (the follower's).
        let waiting = f.ahead().first().is_some_and(|w| w.kind.traversal());
        if left < self.best - STUCK_GAIN || waiting {
            self.best = left;
            self.best_tick = tick;
        }
        if off_path(at, f.behind(), anchor, f.ahead()) > STRAY {
            return Some("strayed");
        }
        (tick.wrapping_sub(self.best_tick) > STUCK_TICKS).then_some("stuck")
    }

    /// This tick, the body at `at`. `pair`: its level pair word when it
    /// can be read.
    pub fn step(
        &mut self,
        map: &MapData,
        known: &Known,
        at: Vec3,
        tick: u32,
        pair: Option<u32>,
    ) -> Out {
        if let Some(r) = self.end {
            return Out {
                motion: RouteMotion::default(),
                done: Some(r),
            };
        }
        if tick.wrapping_sub(self.started) > self.ask.timeout {
            return self.finish(Err("timeout"));
        }
        if self.run.is_none() {
            let primed = pair.map_or(self.primed, is_primed);
            match plan(&self.ask, at, primed, known, map) {
                Ok(p) => {
                    let run = OrderRun::new(p, primed, tick, self.seed ^ u64::from(tick));
                    self.run = Some(run.arrive_within(self.ask.within));
                    self.best = f32::INFINITY;
                    self.best_tick = tick;
                }
                Err(why) => return self.finish(Err(why)),
            }
        }
        if let Some(why) = self.watch(at, tick) {
            return self.replan(why);
        }
        let run = self.run.as_mut().expect("planned above");
        let o = run.step(map, at, tick, pair);
        match o.done {
            None => {
                let mut motion = o.motion;
                // A fresh press each time: a held A jumps once.
                motion.jump = motion.jump && !self.jumped;
                self.jumped = motion.jump;
                Out { motion, done: None }
            }
            Some(Ok(())) => self.finish(Ok(())),
            Some(Err(why))
                if why.starts_with("path_lost") || why == "stalled" || why == "pair_lost" =>
            {
                self.replan(why)
            }
            Some(Err(why)) => self.finish(Err(why)),
        }
    }

    /// Where it stands, the body at `at`.
    pub fn status(&self, at: Option<Vec3>) -> Status {
        let state = match self.end {
            None => "running",
            Some(Ok(())) => "done",
            Some(Err(_)) => "failed",
        };
        let mut s = Status {
            state,
            steps: Vec::new(),
            step: 0,
            phase: "",
            primed: self.primed,
            path: Vec::new(),
            distance: None,
            replans: self.replans,
            replanned: self.replanned,
            error: self.end.and_then(|r| r.err()),
        };
        let Some(run) = &self.run else {
            return s;
        };
        let o = run.status();
        s.steps = run.plan.steps.iter().map(Step::name).collect();
        s.step = o.step;
        s.phase = o.phase;
        s.primed = o.primed;
        if let (Some(w), Some(at)) = (run.walking(), at) {
            let ahead = w.follower().ahead();
            s.distance = Some(remaining(at, ahead));
            s.path = std::iter::once(at)
                .chain(ahead.iter().map(|w| w.at))
                .take(PATH_POINTS)
                .collect();
        }
        s
    }
}

/// The straight-line distance (XY) from `at` to the point a target is
/// reached at, for a status with no walk under way.
pub fn straight(target: &Target, at: Vec3, map: &MapData) -> Option<f32> {
    let p = match target {
        Target::Point(p) => *p,
        Target::Crouch(c) => bounce_route::prime_start(c),
        Target::Site(s) => bounce_route::approach(s, &|s| bounce_route::launch_floor(map, s))?,
    };
    Some(dist_xy(at, p))
}

#[cfg(test)]
mod tests;
