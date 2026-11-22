//! Practising superbounces (docs/game/bots.md, "Practising bounces"):
//! while the superbounce mode is on ([`crate::set_practice`]) a bot does
//! not fight at all. Its whole pad is a practice loop: pick a bounce site
//! (proven first, spread over the bots), run the bounce route to it
//! (`mind::order`: the walking prime, then the drop or the jump), tally
//! how it went, stand a moment, pick the next.
//!
//! A site is picked only when a route by it exists: each site is paired
//! once a map with the crouch spot to prime at for it
//! ([`crate::bounce_route::pair_site`]: a walk from the crouch spot reaches
//! the bounce's approach), and a pick checks the one walk from the bot to
//! that crouch spot. A site with no reachable crouch spot, or whose
//! approach a walk cannot reach, is never tried. A crouch spot or a site
//! whose tries keep failing on the way to it (or, a crouch spot, in its
//! prime) is given up for a while ([`CROUCH_STRIKES`], [`SITE_STRIKES`],
//! [`GIVE_UP_TICKS`]); strikes lapse ([`STRIKE_TICKS`]), a proven crouch
//! spot is never given up, and when nothing is left to try everything
//! given up comes back.
//!
//! In a drill ([`crate::drill`], a sim's `[drill]`) the bot tries one site
//! only: it walks to the drill's start, the host sets its word to -3, and
//! the try is the final approach and the jump.
//!
//! In a training run ([`crate::learn`], a sim's `[train]`;
//! docs/game/bounce-search.md) the bot flies the sites of the run's board
//! instead: it takes a site for a round ([`crate::learn::Board::pick`]),
//! flies each of the round's tries as a drill try with the search's knobs
//! (walk to the site's approach, the host sets the word, one flight), hands
//! each outcome to the board ([`crate::learn::Try`]) and, at the round's
//! end, writes the board's lines to the map's learned file
//! ([`crate::spots::store_learned`]). Outside training the learned knobs
//! ride on the sites ([`Site::knobs`]) and a `learned` site weighs more.
//!
//! The loop sits in front of everything else in [`Mind::step`]: no
//! target is chosen, nothing is shot, thrown or meleed, and being shot
//! changes nothing. A route an agent asks for (`bots.bounce_to`) runs as
//! usual and is not tallied.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use input::host::HostPadState;

use super::feel::{Rng, reached};
use super::order::Asked;
use super::{Mind, RouteMotion, Vec3, angles, dist_xy};
use crate::bounce_route::{
    Mode, Pairing, approach, launch_floor, pair_site, path_length, prime_start,
};
use crate::brain::{Me, Perception, Seen};
use crate::cache::MapData;
use crate::glitches::prime::is_primed;
use crate::learn::{self, SiteIn, Train, Try};
use crate::spots::{Crouch, Known, Site};
use superbounce::learned::{How, Knobs, State as LineState};

/// Sites this far away (XY) or nearer are practised at.
pub const REACH: f32 = 120.0;
/// The pause between tries, ticks: 1.5 to 5 s, drawn each time.
pub const PAUSE_MIN: u32 = 45;
pub const PAUSE_MAX: u32 = 150;
/// The first pause of a practice, ticks (a moment to look round).
pub const FIRST_PAUSE: (u32, u32) = (20, 60);
/// With no site to try, the wait before looking again, ticks.
pub const NOTHING_PAUSE: u32 = 300;
/// A bot's claim on its site lapses this many ticks after it was last
/// renewed (a bot that left or stopped).
pub const CLAIM_TICKS: u32 = 300;
/// How a site is weighed: proven, another bot on it, the last one tried,
/// a unit of distance, and the random part.
pub const W_PROVEN: f32 = 2.0;
pub const W_CLAIMED: f32 = -3.0;
pub const W_LAST: f32 = -1.5;
pub const W_DISTANCE: f32 = -1.0 / 60.0;
pub const W_JITTER: f32 = 0.8;
/// A site the map's learned file calls `learned` weighs this much more.
pub const W_LEARNED: f32 = 1.0;
/// Sites a pick pairs with a crouch spot at most (each up to two walks a
/// crouch spot, of [`crate::bounce_route::PAIR_CROUCHES`]); the rest wait for the
/// next pick, [`PAIRING_PAUSE`] later.
pub const PAIRS_PER_PICK: usize = 4;
/// Walks from the bot to a paired site's crouch spot a pick tries at most.
pub const REACH_QUERIES: usize = 4;
/// With pairings or walks still to try, the wait before the next pick,
/// ticks.
pub const PAIRING_PAUSE: u32 = 2;
/// A site the bot has no walk to from where it stood is passed over for
/// this long (ticks), not tried.
pub const OUT_OF_REACH_TICKS: u32 = 600;
/// A crouch spot whose tries failed this many times on the walk to it or
/// in the prime is given up for the map: its sites pair again without it.
pub const CROUCH_STRIKES: u32 = 2;
/// A site whose tries failed this many times on the walk to its approach
/// is given up.
pub const SITE_STRIKES: u32 = 3;
/// A strike this old (ticks) is forgotten: 2 min. Strikes count toward
/// giving up only while they come closer together than this.
pub const STRIKE_TICKS: u32 = 3600;
/// A crouch spot or a site given up comes back after this long (ticks):
/// 3 min.
pub const GIVE_UP_TICKS: u32 = 5400;
/// The drill: at its start within this (XY, world units; and
/// [`crate::nav::ARRIVE_Z`] in height); the pause between tries (ticks,
/// short: a body still for 38 ticks sleeps and loses its level pair);
/// how long it waits at the start for the host's word (ticks) before it
/// walks off [`DRILL_WAKE`] and back to wake the body.
pub const DRILL_AT: f32 = 0.7;
pub const DRILL_PAUSE: u32 = 10;
pub const DRILL_ARM_TICKS: u32 = 20;
pub const DRILL_WAKE: f32 = 1.0;
/// A training try whose walk to its start was begun this many times, or
/// that woke the body for the word this many times, without getting there
/// armed is void (`no_path`, `not_armed`).
pub const TRAIN_WALKS: u32 = 3;
pub const TRAIN_WAKES: u32 = 3;

/// One site's practice so far.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpotTally {
    /// The site's stand point.
    pub stand: Vec3,
    pub attempts: u32,
    pub bounces: u32,
    /// Why the others did not bounce, by count.
    pub why: BTreeMap<&'static str, u32>,
}

/// What a bot's practice says, for the agent (`bots.bounce_status`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PracticeView {
    /// `idle` (between tries), `route` (on one), `waiting` (no site).
    pub state: &'static str,
    /// The site being tried.
    pub site: Option<Vec3>,
    pub attempts: u32,
    pub bounces: u32,
    pub spots: Vec<SpotTally>,
    /// In a training run, the round the bot flies.
    pub train: Option<TrainView>,
}

/// A training bot's round, for the agent (`bots.bounce_status`'s
/// `practice.train`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrainView {
    /// The site's stand, as its learned line keys it.
    pub site: Vec3,
    /// The search's stage (`arms`, `shift`, `compass`, `scatter`) and, in
    /// `compass`, the round's axis.
    pub stage: &'static str,
    pub axis: Option<&'static str>,
    /// The try's slot in the round.
    pub slot: usize,
    /// The site's scored tries so far.
    pub tries: u32,
}

/// A training run's bot ([`Mind::set_train`]).
#[derive(Clone, Debug, Default)]
struct Training {
    /// The run, and the map its board is for (lower case: the spot ids').
    run: Option<Train>,
    map: String,
    /// The board's site it holds for the round, the round's tries so far,
    /// and the try under way: its slot and knobs, its walks to the start
    /// and its wake-ups for the word.
    held: Option<usize>,
    done: Vec<Try>,
    slot: usize,
    knobs: Knobs,
    walks: u32,
    wakes: u32,
    /// It said it has nothing to fly.
    said: bool,
}

/// What the practice did, as it happened, for a host that tallies it (the
/// sim's bounce report, docs/runtime/sim.md, "Bounce practice"): drained
/// with [`Mind::take_practice_events`]. At most [`EVENTS`] wait; older
/// ones are dropped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PracticeEvent {
    /// A route to the site began.
    Started { site: Vec3, tick: u32 },
    /// The route's word reached -3: `read` from the game (else assumed
    /// after the crouch and stand-up); `own`: the route primed it at a
    /// crouch spot (else it started primed).
    Primed {
        site: Vec3,
        tick: u32,
        read: bool,
        own: bool,
    },
    /// The try is over, as the practice tallies it: bounced, or why not
    /// (the route's error, or the landing's result).
    Tried {
        site: Vec3,
        tick: u32,
        bounced: bool,
        why: Option<&'static str>,
    },
}

/// Practice events kept for the host at most.
pub const EVENTS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// Standing, looking round, until the tick; the yaw it looks about.
    Idle { until: u32, yaw: f32, since: u32 },
    /// On a route to this site.
    Route(Site),
    /// A drill's walk to its start.
    Walk,
}

/// A bot's practice.
#[derive(Clone, Debug)]
pub struct Practice {
    on: bool,
    owner: u64,
    phase: Option<Phase>,
    last: Option<Vec3>,
    rng: Rng,
    tally: Vec<SpotTally>,
    waiting: bool,
    /// Where the bots' claims are kept: [`CLAIMS`], or a test's own.
    board: &'static Board,
    /// Not yet taken by the host ([`Mind::take_practice_events`]).
    events: Vec<PracticeEvent>,
    /// The route under way has reported its prime.
    primed_seen: bool,
    /// Each site's pairing with a crouch spot, by its stand, made once for
    /// the map and spots in [`Practice::pairs_for`].
    pairs: HashMap<[u32; 3], Result<Pairing, &'static str>>,
    pairs_for: u64,
    /// Sites with no walk from where the bot last looked, until the tick.
    out_of_reach: Vec<([u32; 3], u32)>,
    /// The crouch spot the route under way primes at.
    route_crouch: Option<Crouch>,
    /// Failed tries by the crouch spot (its stand) and by the site, with
    /// the tick of the last, and the crouch spots and sites given up,
    /// until a tick, for the map in [`Practice::pairs_for`].
    crouch_strikes: HashMap<[u32; 3], (u32, u32)>,
    site_strikes: HashMap<[u32; 3], (u32, u32)>,
    gone: Vec<([u32; 3], u32)>,
    given_up: Vec<([u32; 3], u32)>,
    /// The map ([`Practice::pairs_for`]) it last said it had nothing to
    /// try on.
    said_nothing: u64,
    /// The drill, when the host runs one ([`Mind::set_drill`]); whether
    /// the bot asks the host for its word now, since when, and whether it
    /// said it found no drill site.
    drill: Option<crate::drill::Drill>,
    arm: bool,
    armed_since: Option<u32>,
    drill_said: bool,
    /// Whether it said which site the drill found.
    drill_found: bool,
    /// The training run, when the host runs one; where its board is kept
    /// ([`learn::shared`], or a test's own) and, for a test, the directory
    /// its learned file is written to instead of the verified one.
    training: Training,
    learn_board: &'static learn::Shared,
    learned_dir: Option<std::path::PathBuf>,
}

/// What a pick found.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Pick {
    /// The site, and the crouch spot paired with it (`None`: primed).
    Site(Site, Option<Crouch>),
    /// Pairings or walks still to try: look again shortly.
    Later,
    /// No site a route reaches.
    Nothing,
}

/// Why a pick found nothing: the sites skipped, by what skipped them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct NothingWhy {
    sites: usize,
    crouches: usize,
    /// Farther than [`REACH`].
    far: u32,
    /// Resting after a failed try (`stuck`'s blacklist).
    resting: u32,
    /// No walk from the bot to its crouch spot (or approach).
    no_walk: u32,
    /// Given up for a while.
    given_up: u32,
    /// No approach a walk reaches (or no launch floor).
    no_approach: u32,
    /// No crouch spot within reach whose walk gets to it.
    no_crouch: u32,
}

impl std::fmt::Display for NothingWhy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} sites, {} crouch spots; sites with no crouch spot to prime at {}, \
             no approach {}, no walk to them {}, resting {}, given up {}, too far {}",
            self.sites,
            self.crouches,
            self.no_crouch,
            self.no_approach,
            self.no_walk,
            self.resting,
            self.given_up,
            self.far
        )
    }
}

/// The log's line for a try (docs/game/bots.md, "Practising bounces"):
/// `bots: bounce try:`, the bot, drill or practice, the site and the aim,
/// what it came to, then how many jumps or drops it made and the last
/// ([`crate::bounce_route::Leap::describe`]: the takeoff, the ground
/// and vertical speed, the word, the air stick, the touchdown and its
/// result) and the results of those before it; or, with none, the step
/// it ended on.
pub(super) fn try_line(
    drill: bool,
    owner: u64,
    site: Site,
    why: Option<&str>,
    view: &super::OrderView,
) -> String {
    let p = |v: Vec3| format!("({:.2}, {:.2}, {:.2})", v[0], v[1], v[2]);
    let head = format!(
        "bots: bounce try: bot {owner:#x} {} site {} aim {}: {}",
        if drill { "drill" } else { "practice" },
        p(site.bounce.stand),
        p(site.target()),
        why.unwrap_or("bounce"),
    );
    match view.flights.split_last() {
        Some((last, before)) => {
            let mut line = format!(
                "{head}; jumps {}, the last: {}",
                before.len() + 1,
                last.describe()
            );
            if !before.is_empty() {
                let ends: Vec<_> = before.iter().map(|f| f.end.unwrap_or("open")).collect();
                line.push_str(&format!("; before: {}", ends.join(", ")));
            }
            line
        }
        None => format!(
            "{head}; no jump, ended at step {} ({})",
            view.step,
            view.steps.get(view.step).copied().unwrap_or("done")
        ),
    }
}

fn stand_key(p: Vec3) -> [u32; 3] {
    p.map(f32::to_bits)
}

/// What a map's pairings were made for: the map and its spots.
fn pairs_key(known: &Known, map: &MapData) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (map as *const MapData as usize).hash(&mut h);
    for s in &known.sites {
        stand_key(s.bounce.stand).hash(&mut h);
    }
    known.crouches.len().hash(&mut h);
    for c in &known.crouches {
        stand_key(c.stand).hash(&mut h);
    }
    h.finish()
}

impl Practice {
    pub fn new(seed: u64) -> Practice {
        Practice {
            on: false,
            owner: seed,
            phase: None,
            last: None,
            rng: Rng::new(seed ^ 0x9a_c71c),
            tally: Vec::new(),
            waiting: false,
            board: &CLAIMS,
            events: Vec::new(),
            primed_seen: false,
            pairs: HashMap::new(),
            pairs_for: 0,
            out_of_reach: Vec::new(),
            route_crouch: None,
            crouch_strikes: HashMap::new(),
            site_strikes: HashMap::new(),
            gone: Vec::new(),
            given_up: Vec::new(),
            said_nothing: 0,
            drill: None,
            arm: false,
            armed_since: None,
            drill_said: false,
            drill_found: false,
            training: Training::default(),
            learn_board: learn::shared(),
            learned_dir: None,
        }
    }

    fn event(&mut self, e: PracticeEvent) {
        if self.events.len() >= EVENTS {
            self.events.remove(0);
        }
        self.events.push(e);
    }

    pub fn on(&self) -> bool {
        self.on
    }

    /// Practising in a drill or a training run: the host reads the word
    /// between tries too, and writes it at a try's start.
    pub fn drilling(&self) -> bool {
        self.on && (self.drill.is_some() || self.training.run.is_some())
    }

    /// The round a training bot flies, read off the board.
    fn train_view(&self) -> Option<TrainView> {
        let i = self.training.held?;
        let slot = self.training.slot;
        on_board(self.learn_board, |b| {
            let s = &b.sites().get(i)?.search;
            Some(TrainView {
                site: s.setup().at,
                stage: s.entry().stage.name(),
                axis: s.axis().map(learn::Axis::name),
                slot,
                tries: s.entry().tries,
            })
        })
        .flatten()
    }

    fn is_gone(&self, c: &Crouch) -> bool {
        let k = stand_key(c.stand);
        self.gone.iter().any(|(g, _)| *g == k)
    }

    fn record(&mut self, stand: Vec3, bounced: bool, why: Option<&'static str>) {
        let i = match self
            .tally
            .iter()
            .position(|t| dist_xy(t.stand, stand) < 0.05 && (t.stand[2] - stand[2]).abs() < 0.5)
        {
            Some(i) => i,
            None => {
                self.tally.push(SpotTally {
                    stand,
                    ..SpotTally::default()
                });
                self.tally.len() - 1
            }
        };
        let t = &mut self.tally[i];
        t.attempts += 1;
        if bounced {
            t.bounces += 1;
        } else if let Some(w) = why {
            *t.why.entry(w).or_insert(0) += 1;
        }
    }

    pub fn view(&self) -> PracticeView {
        PracticeView {
            state: match self.phase {
                Some(Phase::Route(_)) => "route",
                _ if self.waiting => "waiting",
                _ => "idle",
            },
            site: match self.phase {
                Some(Phase::Route(s)) => Some(s.bounce.stand),
                _ => None,
            },
            attempts: self.tally.iter().map(|t| t.attempts).sum(),
            bounces: self.tally.iter().map(|t| t.bounces).sum(),
            spots: self.tally.clone(),
            train: self.train_view(),
        }
    }
}

/// Runs `f` on the training board kept in `board`, when it is built.
fn on_board<R>(board: &learn::Shared, f: impl FnOnce(&mut learn::Board) -> R) -> Option<R> {
    board
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
        .map(f)
}

/// A route's error that says nothing about the knobs: a training try that
/// ends so is void (docs/game/bounce-search.md, "A try and its score").
/// Everything else (`no_bounce`, `no_drop`, `timeout`, `try_over`) is a
/// scored try, and `died` a scored one with a death.
fn void_error(e: &str) -> bool {
    e.starts_with("path_lost")
        || matches!(
            e,
            "map_changed"
                | "practice_off"
                | "pair_lost"
                | "not_primed"
                | "no_route"
                | "no_map"
                | "no_path"
                | "goal_off_mesh"
                | "tail_stuck"
                | "stalled"
                | "state"
                | "enemy"
        )
}

/// The knobs a training try flies, as the log line appends them: the set
/// ones, by the learned file's keys.
pub fn knobs_text(k: &Knobs) -> String {
    let mut out: Vec<String> = Vec::new();
    if let Some(h) = k.how {
        out.push(format!("how={}", h.name()));
    }
    if let Some(f) = k.from {
        out.push(format!("fx={:.3} fy={:.3} fz={:.3}", f[0], f[1], f[2]));
    }
    if let Some(a) = k.aim {
        out.push(format!("ax={:.3} ay={:.3}", a[0], a[1]));
    }
    let mut num = |key: &str, v: Option<f32>, places: usize| {
        if let Some(v) = v {
            out.push(format!("{key}={v:.places$}"));
        }
    };
    num("yaw", k.yaw, 1);
    num("back", k.back, 2);
    num("run", k.run, 2);
    num("early", k.early, 3);
    num("gain", k.gain, 2);
    num("letgo", k.letgo, 3);
    num("drift", k.drift, 2);
    if let Some(v) = k.still {
        out.push(format!("still={v}"));
    }
    if out.is_empty() {
        "the route's own".to_string()
    } else {
        out.join(" ")
    }
}

/// A known site as a training board takes it (docs/game/bounce-search.md,
/// "The search"): the `how`s it allows, the route's own first (a standing
/// site: a stand; a drop: the flight model's choice, then the other), and
/// the route's own knobs from [`crate::bounce_route::flown`] with no line,
/// the search's centre for a site never flown.
pub fn site_in(
    s: &Site,
    ledge: crate::bounce_route::Ledge,
    tune: &crate::tuning::BounceTuning,
) -> SiteIn {
    let own = Site {
        knobs: Knobs::default(),
        ..*s
    };
    let (hows, base) = if own.standing() {
        let f = crate::bounce_route::flown(&own, own.bounce.from(), tune);
        (vec![How::Stand], f.knobs(None))
    } else {
        match crate::bounce_route::launch_from(&own, ledge) {
            Some(from) => {
                let f = crate::bounce_route::flown(&own, from, tune);
                let other = if f.jump() { How::Runoff } else { How::Jump };
                (vec![f.how, other], f.knobs(Some(from)))
            }
            // No launch floor: its tries are void (`no_launch_floor`).
            None => (vec![How::Runoff, How::Jump], Knobs::default()),
        }
    };
    SiteIn {
        at: s.bounce.stand,
        hows,
        base,
        run_off: tune.run_off_speed,
        need: s.need,
    }
}

// ---- the claims: which bot practises where ----

#[derive(Clone, Copy, Debug)]
pub struct Claim {
    owner: u64,
    key: [i32; 3],
    tick: u32,
}

/// The bots' claims on sites.
pub type Board = Mutex<Vec<Claim>>;

/// The process's claims: every bot of the game's.
pub static CLAIMS: Board = Mutex::new(Vec::new());

fn key(p: Vec3) -> [i32; 3] {
    p.map(|c| (c * 4.0).round() as i32)
}

fn fresh(c: &Claim, tick: u32) -> bool {
    reached(tick, c.tick) && tick.wrapping_sub(c.tick) < CLAIM_TICKS
}

/// Claims `stand` for `owner` at `tick` (renewing it), dropping its other
/// claims; `None` drops them all.
pub fn claim(board: &Board, owner: u64, stand: Option<Vec3>, tick: u32) {
    let mut v = board.lock().unwrap_or_else(|e| e.into_inner());
    v.retain(|c| c.owner != owner && fresh(c, tick));
    if let Some(s) = stand {
        v.push(Claim {
            owner,
            key: key(s),
            tick,
        });
    }
}

/// Whether another bot practises at `stand` now.
pub fn claimed_by_other(board: &Board, owner: u64, stand: Vec3, tick: u32) -> bool {
    let k = key(stand);
    let v = board.lock().unwrap_or_else(|e| e.into_inner());
    v.iter()
        .any(|c| c.owner != owner && c.key == k && fresh(c, tick))
}

/// The weight of practising at `s` from `at`: higher is better, before
/// the random part.
pub fn weight(s: &Site, at: Vec3, claimed: bool, last: Option<Vec3>) -> f32 {
    let mut w = W_DISTANCE * dist_xy(s.bounce.stand, at);
    if s.proven() {
        w += W_PROVEN;
    }
    if claimed {
        w += W_CLAIMED;
    }
    if last.is_some_and(|l| dist_xy(l, s.bounce.stand) < 0.05) {
        w += W_LAST;
    }
    if s.line == Some(LineState::Learned) {
        w += W_LEARNED;
    }
    w
}

impl Mind {
    /// Turns the practice on or off (the host follows the superbounce
    /// mode, [`crate::set_practice`]). Off drops a practice route under way.
    pub fn set_practice(&mut self, on: bool) {
        if self.practice.on == on {
            return;
        }
        self.practice.on = on;
        if !on {
            if matches!(self.practice.phase, Some(Phase::Route(_) | Phase::Walk)) {
                self.cancel_order("practice_off");
            }
            self.train_let_go();
            self.practice.phase = None;
            claim(
                self.practice.board,
                self.practice.owner,
                None,
                self.tick.unwrap_or(0),
            );
        }
    }

    /// The host's drill ([`crate::drill::current`]), or none.
    pub fn set_drill(&mut self, d: Option<crate::drill::Drill>) {
        if self.practice.drill != d {
            self.practice.drill = d;
            self.practice.armed_since = None;
            self.practice.drill_said = false;
            self.practice.drill_found = false;
        }
        if d.is_none() {
            self.practice.arm = false;
        }
    }

    /// The host's training run ([`crate::learn::current`]) on `map` (its
    /// short name), or none. A change lets go of the site it held.
    pub fn set_train(&mut self, t: Option<Train>, map: &str) {
        let map = map.to_ascii_lowercase();
        let tr = &self.practice.training;
        if tr.run == t && tr.map == map {
            return;
        }
        self.train_let_go();
        self.practice.training = Training {
            run: t,
            map,
            ..Training::default()
        };
        if t.is_none() {
            self.practice.arm = false;
        }
    }

    /// Lets go of the training site held, its round to be flown again
    /// from the same state.
    fn train_let_go(&mut self) {
        let tr = &mut self.practice.training;
        if tr.held.take().is_some() {
            let owner = self.practice.owner;
            on_board(self.practice.learn_board, |b| b.release(owner));
        }
        tr.done.clear();
    }

    /// A drill's bot at its start, not primed: the host should set its
    /// word ([`crate::drill::request_arm`]).
    pub fn drill_wants_arm(&self) -> bool {
        self.practice.drilling() && self.practice.arm
    }

    /// The practice so far, for the agent.
    pub fn practice_view(&self) -> Option<PracticeView> {
        self.practice.on.then(|| self.practice.view())
    }

    /// What the practice did since the last call, oldest first.
    pub fn take_practice_events(&mut self) -> Vec<PracticeEvent> {
        std::mem::take(&mut self.practice.events)
    }

    /// Reports the route's prime once, when its word first reads -3.
    fn practice_watch_prime(&mut self, site: Site, tick: u32) {
        if self.practice.primed_seen {
            return;
        }
        if let Some((read, own)) = self.route_primed() {
            self.practice.primed_seen = true;
            self.practice.event(PracticeEvent::Primed {
                site: site.bounce.stand,
                tick,
                read,
                own,
            });
        }
    }

    /// The site to practise at next, from `at`: the best weighed of the
    /// sites a route reaches. A site is paired with its crouch spot the
    /// first time it is looked at ([`pair_site`], kept for the map); a
    /// paired site is picked when a walk from `at` reaches its crouch spot
    /// (or, primed, its approach). A pick makes at most [`PAIRS_PER_PICK`]
    /// pairings and [`REACH_QUERIES`] walks.
    fn pick_practice(&mut self, at: Vec3, tick: u32, map: Option<&MapData>) -> Pick {
        let Some(m) = map else {
            return Pick::Nothing;
        };
        // A site needs a crouch spot to prime at (or the bot primed).
        let primed = self.pair_word.is_some_and(is_primed);
        let for_map = pairs_key(&self.known, m);
        if !primed && self.known.crouches.is_empty() {
            self.say_nothing(for_map, "no crouch spot is known on the map");
            return Pick::Nothing;
        }
        if self.practice.pairs_for != for_map {
            self.practice.pairs.clear();
            self.practice.out_of_reach.clear();
            self.practice.crouch_strikes.clear();
            self.practice.site_strikes.clear();
            self.practice.gone.clear();
            self.practice.given_up.clear();
            self.practice.pairs_for = for_map;
        }
        self.practice
            .out_of_reach
            .retain(|&(_, until)| !reached(tick, until));
        // What was given up comes back in time; a crouch spot back pairs
        // again.
        let gone = self.practice.gone.len();
        self.practice
            .gone
            .retain(|&(_, until)| !reached(tick, until));
        if self.practice.gone.len() != gone {
            self.practice.pairs.clear();
        }
        self.practice
            .given_up
            .retain(|&(_, until)| !reached(tick, until));
        let mut why = NothingWhy {
            sites: self.known.sites.len(),
            crouches: self.known.crouches.len(),
            ..NothingWhy::default()
        };
        let owner = self.practice.owner;
        let last = self.practice.last;
        let mut ranked: Vec<(f32, Site)> = Vec::new();
        for s in &self.known.sites {
            let k = stand_key(s.bounce.stand);
            let skip = if dist_xy(s.bounce.stand, at) > REACH {
                Some(&mut why.far)
            } else if self.stuck.blocked(s.bounce.stand, tick) {
                Some(&mut why.resting)
            } else if self.practice.out_of_reach.iter().any(|(o, _)| *o == k) {
                Some(&mut why.no_walk)
            } else if self.practice.given_up.iter().any(|(g, _)| *g == k) {
                Some(&mut why.given_up)
            } else {
                match self.practice.pairs.get(&k) {
                    // Known to have no route: never tried.
                    Some(Err(_)) => Some(&mut why.no_approach),
                    Some(Ok(p)) if p.crouch.is_none() && !primed => Some(&mut why.no_crouch),
                    _ => None,
                }
            };
            if let Some(n) = skip {
                *n += 1;
                continue;
            }
            let claimed = claimed_by_other(self.practice.board, owner, s.bounce.stand, tick);
            let w = weight(s, at, claimed, last) + W_JITTER * self.practice.rng.unit();
            ranked.push((w, *s));
        }
        ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
        let walk = |a: Vec3, b: Vec3| path_length(m, a, b);
        let ledge = |s: &Site| launch_floor(m, s);
        let (mut paired, mut walks) = (0, 0);
        for (_, s) in ranked {
            let k = stand_key(s.bounce.stand);
            let pairing = match self.practice.pairs.get(&k) {
                Some(p) => *p,
                None => {
                    if paired >= PAIRS_PER_PICK {
                        return Pick::Later;
                    }
                    paired += 1;
                    let practice = &self.practice;
                    let usable = |c: &Crouch| !practice.is_gone(c);
                    let p = pair_site(&s, &self.known, &walk, &ledge, &usable);
                    self.practice.pairs.insert(k, p);
                    p
                }
            };
            let Ok(p) = pairing else {
                why.no_approach += 1;
                continue;
            };
            let (to, crouch) = match (primed, p.crouch) {
                (true, _) => (p.approach, None),
                (false, Some(c)) => (prime_start(&c), Some(c)),
                (false, None) => {
                    why.no_crouch += 1;
                    continue;
                }
            };
            if walks >= REACH_QUERIES {
                return Pick::Later;
            }
            walks += 1;
            if walk(at, to).is_some() {
                return Pick::Site(s, crouch);
            }
            why.no_walk += 1;
            self.practice
                .out_of_reach
                .push((k, tick.wrapping_add(OUT_OF_REACH_TICKS)));
        }
        // Never nothing for what was given up: it all comes back.
        if !self.practice.gone.is_empty() || !self.practice.given_up.is_empty() {
            let p = &mut self.practice;
            p.gone.clear();
            p.given_up.clear();
            p.crouch_strikes.clear();
            p.site_strikes.clear();
            p.pairs.clear();
            return Pick::Later;
        }
        if self.practice.said_nothing != for_map {
            // The proven crouch spots, which pair first: whether a walk
            // reaches each one's start from here and from its stand.
            let yes = |w: Option<f32>| if w.is_some() { "yes" } else { "no" };
            let proven: Vec<String> = self
                .known
                .crouches
                .iter()
                .filter(|c| c.proven())
                .take(4)
                .map(|c| {
                    let start = prime_start(c);
                    format!(
                        "proven crouch spot ({:.2}, {:.2}, {:.2}): a walk to its start {}, from its stand {}",
                        c.stand[0],
                        c.stand[1],
                        c.stand[2],
                        yes(walk(at, start)),
                        yes(walk(c.stand, start))
                    )
                })
                .collect();
            let mut said = why.to_string();
            for p in proven {
                said.push_str("; ");
                said.push_str(&p);
            }
            self.say_nothing(for_map, &said);
        }
        Pick::Nothing
    }

    /// Says once a map why the practice has nothing to try (a sim's log:
    /// a map with no tries shows why).
    fn say_nothing(&mut self, for_map: u64, why: &str) {
        if self.practice.said_nothing != for_map {
            self.practice.said_nothing = for_map;
            eprintln!("bots: bounce practice: nothing to try: {why}");
        }
    }

    fn practice_idle(&mut self, me: Me, tick: u32, pause: u32) {
        let yaw = angles(me.aim).0;
        self.practice.phase = Some(Phase::Idle {
            until: tick.wrapping_add(pause),
            yaw,
            since: tick,
        });
    }

    /// A failed try counts against what it failed at: the walk to the
    /// crouch spot or its prime against the crouch spot, the walk to the
    /// approach against the site. One that has failed often enough
    /// ([`CROUCH_STRIKES`], [`SITE_STRIKES`]), each strike within
    /// [`STRIKE_TICKS`] of the one before, is given up for
    /// [`GIVE_UP_TICKS`]; a proven crouch spot never is. A landing that
    /// did not bounce counts against neither.
    fn practice_strike(
        &mut self,
        site: Site,
        steps: &[&'static str],
        step: usize,
        why: Option<&'static str>,
    ) {
        let Some(why) = why else { return };
        let tick = self.tick.unwrap_or(0);
        let walked = why.starts_with("path_lost")
            || matches!(why, "no_path" | "goal_off_mesh" | "tail_stuck");
        let at = steps.get(step).copied();
        let next = steps.get(step + 1).copied();
        let crouch_leg = (walked && next == Some("prime")) || at == Some("prime");
        let site_leg = walked && matches!(next, Some("bounce" | "drop"));
        let strike = |strikes: &mut HashMap<[u32; 3], (u32, u32)>, k: [u32; 3]| {
            let e = strikes.entry(k).or_insert((0, tick));
            if tick.wrapping_sub(e.1) > STRIKE_TICKS {
                e.0 = 0;
            }
            *e = (e.0 + 1, tick);
            e.0
        };
        let until = tick.wrapping_add(GIVE_UP_TICKS);
        let p = &mut self.practice;
        if crouch_leg && let Some(c) = p.route_crouch {
            let k = stand_key(c.stand);
            let n = strike(&mut p.crouch_strikes, k);
            if n >= CROUCH_STRIKES && !c.proven() && !p.is_gone(&c) {
                p.gone.push((k, until));
                p.crouch_strikes.remove(&k);
                // Its sites pair again, without it.
                p.pairs
                    .retain(|_, v| !matches!(v, Ok(Pairing { crouch: Some(c), .. }) if stand_key(c.stand) == k));
            }
        }
        if site_leg {
            let k = stand_key(site.bounce.stand);
            let n = strike(&mut p.site_strikes, k);
            if n >= SITE_STRIKES {
                p.given_up.push((k, until));
                p.site_strikes.remove(&k);
            }
        }
    }

    /// The route asked for by the practice has ended: tally it.
    fn practice_tally(&mut self, site: Site) {
        let view = self.order_view();
        // Ended by something that says nothing about the site.
        if matches!(view.error, Some("died" | "map_changed" | "practice_off")) {
            return;
        }
        let bounced = view.state == "done" && view.site.is_some();
        let why = if bounced {
            None
        } else {
            let landing = view.landing.filter(|t| !t.bounced()).map(|t| {
                let width = if site.width > 0.0 {
                    site.width
                } else {
                    crate::spots::SLOT_WIDTH
                };
                t.result(width, site.need)
            });
            Some(match (view.error, landing) {
                (Some("no_bounce"), Some(l)) => l,
                (Some(e), _) => e,
                (None, _) => "no_route",
            })
        };
        if !bounced {
            self.practice_strike(site, &view.steps, view.step, why);
        }
        eprintln!(
            "{}",
            try_line(
                self.practice.drill.is_some(),
                self.practice.owner,
                site,
                why,
                &view
            )
        );
        self.practice.record(site.bounce.stand, bounced, why);
        let tick = self.tick.unwrap_or(0);
        self.practice.event(PracticeEvent::Tried {
            site: site.bounce.stand,
            tick,
            bounced,
            why,
        });
        self.practice.last = Some(site.bounce.stand);
        if !bounced {
            // Something else next time; back to it after a while.
            self.stuck
                .blacklist(site.bounce.stand, self.tick.unwrap_or(0));
        }
    }

    /// The practice's pad this tick while it is on; `None` while off.
    pub(super) fn practice_pad(
        &mut self,
        p: &Perception,
        me: Me,
        map: Option<&MapData>,
        seen: &[Seen],
    ) -> Option<HostPadState> {
        if !self.practice.on {
            return None;
        }
        if self.practice.training.run.is_some() {
            return Some(self.train_pad(p, me, map, seen));
        }
        if let Some(d) = self.practice.drill {
            return Some(self.drill_pad(p, me, map, seen, d));
        }
        let tick = p.tick;
        // A route under way, the practice's or an agent's.
        if self.asked.is_some() || self.order.is_some() {
            if let Some(Phase::Route(s)) = self.practice.phase {
                claim(
                    self.practice.board,
                    self.practice.owner,
                    Some(s.bounce.stand),
                    tick,
                );
            }
            if let Some(pad) = self.order_pad(p, me, map, seen) {
                if let Some(Phase::Route(s)) = self.practice.phase {
                    self.practice_watch_prime(s, tick);
                }
                return Some(pad);
            }
        }
        match self.practice.phase {
            // The practice's route is over.
            Some(Phase::Route(site)) => {
                self.practice_tally(site);
                claim(self.practice.board, self.practice.owner, None, tick);
                let pause = self.practice.rng.ticks(PAUSE_MIN, PAUSE_MAX);
                self.practice_idle(me, tick, pause);
            }
            None => {
                let pause = self.practice.rng.ticks(FIRST_PAUSE.0, FIRST_PAUSE.1);
                self.practice_idle(me, tick, pause);
            }
            Some(Phase::Idle { until, .. }) if reached(tick, until) => {
                match self.pick_practice(me.position, tick, map) {
                    Pick::Site(site, crouch) => {
                        self.practice.waiting = false;
                        self.practice.route_crouch = crouch;
                        claim(
                            self.practice.board,
                            self.practice.owner,
                            Some(site.bounce.stand),
                            tick,
                        );
                        self.asked = Some(Asked {
                            to: site.bounce.stand,
                            mode: Mode::Fun,
                            show_off: false,
                            site: Some(site),
                            crouch,
                            plain: false,
                        });
                        self.practice.phase = Some(Phase::Route(site));
                        self.practice.primed_seen = false;
                        self.practice.event(PracticeEvent::Started {
                            site: site.bounce.stand,
                            tick,
                        });
                        if let Some(pad) = self.order_pad(p, me, map, seen) {
                            self.practice_watch_prime(site, tick);
                            return Some(pad);
                        }
                        // No route to it (or no map): tally and wait.
                        self.practice_tally(site);
                        self.practice_idle(me, tick, PAUSE_MAX);
                    }
                    Pick::Later => {
                        self.practice_idle(me, tick, PAIRING_PAUSE);
                    }
                    Pick::Nothing => {
                        self.practice.waiting = true;
                        self.practice_idle(me, tick, NOTHING_PAUSE);
                    }
                }
            }
            // A drill's walk, the drill gone: pick as usual.
            Some(Phase::Walk) => self.practice_idle(me, tick, PAUSE_MIN),
            Some(Phase::Idle { .. }) => {}
        }
        self.practice_stand(me, tick)
    }

    /// Standing between tries: looking round, slowly, about one way.
    fn practice_stand(&mut self, me: Me, tick: u32) -> Option<HostPadState> {
        let yaw = match self.practice.phase {
            Some(Phase::Idle { yaw, since, .. }) => {
                yaw + 0.6 * (tick.wrapping_sub(since) as f32 / 45.0).sin()
            }
            _ => angles(me.aim).0,
        };
        let pad = self.route_pad.report(
            me.aim,
            tick,
            RouteMotion {
                yaw: Some(yaw),
                ..RouteMotion::default()
            },
        );
        self.last_pad = pad;
        Some(pad)
    }

    /// The drill's known site: the one nearest the drill's `site` within
    /// [`crate::drill::SAME`].
    fn drill_site(&self, d: crate::drill::Drill) -> Option<Site> {
        let same = crate::drill::SAME;
        self.known
            .sites
            .iter()
            .filter(|s| {
                dist_xy(s.bounce.stand, d.site) <= same
                    && (s.bounce.stand[2] - d.site[2]).abs() <= same
            })
            .min_by(|a, b| {
                dist_xy(a.bounce.stand, d.site).total_cmp(&dist_xy(b.bounce.stand, d.site))
            })
            .copied()
    }

    /// The drill's pad: walk to the start, stand there while the host sets
    /// the word (walking off and back if it doesn't come: a sleeping body
    /// has no level pair to write), then the try from there, primed: the
    /// route by the one site with no prime ([`crate::bounce_route::plan_site`]),
    /// tallied as a practice try. Then back to the start.
    fn drill_pad(
        &mut self,
        p: &Perception,
        me: Me,
        map: Option<&MapData>,
        seen: &[Seen],
        d: crate::drill::Drill,
    ) -> HostPadState {
        let tick = p.tick;
        self.practice.arm = false;
        if (self.asked.is_some() || self.order.is_some())
            && let Some(pad) = self.order_pad(p, me, map, seen)
        {
            if let Some(Phase::Route(s)) = self.practice.phase {
                self.practice_watch_prime(s, tick);
            }
            return pad;
        }
        match self.practice.phase {
            Some(Phase::Route(site)) => {
                self.practice_tally(site);
                self.practice_idle(me, tick, DRILL_PAUSE);
            }
            // At the start, or not: look at once.
            Some(Phase::Walk) => self.practice_idle(me, tick, 1),
            None => self.practice_idle(me, tick, DRILL_PAUSE),
            Some(Phase::Idle { until, .. }) if reached(tick, until) => {
                if let Some(pad) = self.drill_next(p, me, map, seen, d) {
                    return pad;
                }
            }
            Some(Phase::Idle { .. }) => {}
        }
        self.practice_stand(me, tick).unwrap_or_default()
    }

    /// The drill's next move from standing: a walk, a wait for the word,
    /// or the try. `None`: stand this tick.
    fn drill_next(
        &mut self,
        p: &Perception,
        me: Me,
        map: Option<&MapData>,
        seen: &[Seen],
        d: crate::drill::Drill,
    ) -> Option<HostPadState> {
        let tick = p.tick;
        let (Some(m), Some(site)) = (map, self.drill_site(d)) else {
            if !self.practice.drill_said {
                self.practice.drill_said = true;
                eprintln!(
                    "bots: bounce drill: no known site within {} of {:?} ({} sites known)",
                    crate::drill::SAME,
                    d.site,
                    self.known.sites.len()
                );
            }
            self.practice.waiting = true;
            self.practice_idle(me, tick, NOTHING_PAUSE);
            return None;
        };
        self.practice.waiting = false;
        let ledge = |s: &Site| launch_floor(m, s);
        let start = d.start.or_else(|| approach(&site, &ledge));
        if !self.practice.drill_found {
            // Said once a drill: the site found (after a "no known site"
            // while the spots were still coming in, the drill goes on).
            self.practice.drill_found = true;
            let p = |v: Vec3| format!("({:.2}, {:.2}, {:.2})", v[0], v[1], v[2]);
            eprintln!(
                "bots: bounce drill: site {} ({}), aim {}, start {} ({} sites known)",
                p(site.bounce.stand),
                if site.standing() {
                    "a standing jump"
                } else {
                    "a drop"
                },
                p(site.target()),
                start.map_or_else(|| "none".to_string(), p),
                self.known.sites.len()
            );
        }
        let Some(start) = start else {
            if !self.practice.drill_said {
                self.practice.drill_said = true;
                eprintln!("bots: bounce drill: the site has no approach and no start was given");
            }
            self.practice.waiting = true;
            self.practice_idle(me, tick, NOTHING_PAUSE);
            return None;
        };
        let at = me.position;
        let there =
            dist_xy(at, start) <= DRILL_AT && (at[2] - start[2]).abs() <= crate::nav::ARRIVE_Z;
        let walk = |mind: &mut Mind, to: Vec3| {
            mind.asked = Some(Asked {
                to,
                mode: Mode::Help,
                show_off: false,
                site: None,
                crouch: None,
                plain: true,
            });
            mind.practice.phase = Some(Phase::Walk);
            mind.practice.armed_since = None;
            mind.order_pad(p, me, map, seen)
        };
        if !there {
            if let Some(pad) = walk(self, start) {
                return Some(pad);
            }
            // No walk there from here: try again in a while.
            self.practice_idle(me, tick, PAUSE_MAX);
            return None;
        }
        if !self.pair_word.is_some_and(is_primed) {
            let since = *self.practice.armed_since.get_or_insert(tick);
            if tick.wrapping_sub(since) > DRILL_ARM_TICKS {
                // No word yet: the body may sleep (no level pair to write).
                // Walk off along the way in and come back.
                let t = site.target();
                let away = [start[0] - t[0], start[1] - t[1]];
                let l = away[0].hypot(away[1]).max(1e-3);
                let off = [
                    start[0] + away[0] / l * DRILL_WAKE,
                    start[1] + away[1] / l * DRILL_WAKE,
                    start[2],
                ];
                if let Some(pad) = walk(self, off) {
                    return Some(pad);
                }
                self.practice.armed_since = None;
            }
            self.practice.arm = true;
            self.practice_idle(me, tick, 1);
            return None;
        }
        // Primed at the start: the try.
        self.practice.armed_since = None;
        self.asked = Some(Asked {
            to: site.bounce.stand,
            mode: Mode::Fun,
            show_off: false,
            site: Some(site),
            crouch: None,
            plain: false,
        });
        self.practice.route_crouch = None;
        self.practice.phase = Some(Phase::Route(site));
        self.practice.primed_seen = false;
        self.practice.event(PracticeEvent::Started {
            site: site.bounce.stand,
            tick,
        });
        if let Some(pad) = self.order_pad(p, me, map, seen) {
            self.practice_watch_prime(site, tick);
            return Some(pad);
        }
        // No route from here (the word lost before it began, or no plan).
        self.practice_tally(site);
        self.practice_idle(me, tick, DRILL_PAUSE);
        None
    }
}

// ---- the training run (docs/game/bounce-search.md) ----

impl Mind {
    /// The training run's pad: the drill's loop over the board's sites. A
    /// route under way is the try's; a landing that did not launch ends it
    /// (one flight a try). Between tries: the next slot of the round held,
    /// or a new pick.
    fn train_pad(
        &mut self,
        p: &Perception,
        me: Me,
        map: Option<&MapData>,
        seen: &[Seen],
    ) -> HostPadState {
        let tick = p.tick;
        self.practice.arm = false;
        if (self.asked.is_some() || self.order.is_some())
            && let Some(pad) = self.order_pad(p, me, map, seen)
        {
            let Some(Phase::Route(s)) = self.practice.phase else {
                return pad;
            };
            self.practice_watch_prime(s, tick);
            let over = self
                .order_view()
                .flights
                .first()
                .is_some_and(|f| f.end.is_some() && !f.landing.is_some_and(|t| t.bounced()));
            if !over {
                return pad;
            }
            self.cancel_order("try_over");
        }
        match self.practice.phase {
            Some(Phase::Route(site)) => {
                self.train_tally(site);
                self.practice_idle(me, tick, DRILL_PAUSE);
            }
            Some(Phase::Walk) => self.practice_idle(me, tick, 1),
            None => self.practice_idle(me, tick, DRILL_PAUSE),
            Some(Phase::Idle { until, .. }) if reached(tick, until) => {
                if let Some(pad) = self.train_next(p, me, map, seen) {
                    return pad;
                }
            }
            Some(Phase::Idle { .. }) => {}
        }
        self.practice_stand(me, tick).unwrap_or_default()
    }

    /// The site the bot flies: the one it holds, else the board's pick
    /// (built from the known sites at the first pick, once the map's scan
    /// is in).
    fn train_pick(&mut self, at: Vec3, m: &MapData) -> Option<usize> {
        let owner = self.practice.owner;
        let tr = &self.practice.training;
        let run = tr.run?;
        let mut g = self
            .practice
            .learn_board
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if g.is_none() {
            if self.known.sites.is_empty() || !crate::spots::has_scan(&tr.map) {
                return None;
            }
            let tune = crate::tuning::bounce().sane();
            let ledge = |s: &Site| launch_floor(m, s);
            let sites = self
                .known
                .sites
                .iter()
                .map(|s| site_in(s, &ledge, &tune))
                .collect();
            let lines = crate::spots::learned_for(&tr.map);
            *g = Some(learn::Board::new(&tr.map, run, sites, &lines));
        }
        g.as_mut()?.pick(owner, at)
    }

    /// The training's next move from standing: a walk to the try's start,
    /// a wait for the word, or the try. `None`: stand this tick.
    fn train_next(
        &mut self,
        p: &Perception,
        me: Me,
        map: Option<&MapData>,
        seen: &[Seen],
    ) -> Option<HostPadState> {
        let tick = p.tick;
        let Some(m) = map else {
            self.practice_idle(me, tick, NOTHING_PAUSE);
            return None;
        };
        let Some(i) = self.train_pick(me.position, m) else {
            if !self.practice.training.said {
                self.practice.training.said = true;
                eprintln!(
                    "bots: train: nothing to fly ({} sites known; the board is built once the map's scan is in)",
                    self.known.sites.len()
                );
            }
            self.practice.waiting = true;
            self.practice_idle(me, tick, NOTHING_PAUSE);
            return None;
        };
        self.practice.waiting = false;
        self.practice.training.held = Some(i);
        let next = on_board(self.practice.learn_board, |b| {
            let s = &b.sites().get(i)?.search;
            Some((s.setup().at, s.next_slot(&self.practice.training.done)))
        })
        .flatten();
        let Some((at, next)) = next else {
            self.train_let_go();
            self.practice_idle(me, tick, PAUSE_MIN);
            return None;
        };
        let Some((slot, knobs)) = next else {
            // The round's tries are all in.
            self.train_fold(i);
            self.practice_idle(me, tick, 1);
            return None;
        };
        let Some(own) = self
            .known
            .sites
            .iter()
            .find(|s| s.bounce.stand == at)
            .copied()
        else {
            self.train_try(Try::void("no_site"));
            self.practice_idle(me, tick, 1);
            return None;
        };
        // The try flies the board's knobs over the site's.
        let site = Site {
            knobs: knobs.or(own.knobs),
            ..own
        };
        let tr = &mut self.practice.training;
        tr.slot = slot;
        tr.knobs = site.knobs;
        let ledge = |s: &Site| launch_floor(m, s);
        let Some(start) = approach(&site, &ledge) else {
            self.train_try(Try::void("no_launch_floor"));
            self.practice_idle(me, tick, 1);
            return None;
        };
        let at = me.position;
        let there =
            dist_xy(at, start) <= DRILL_AT && (at[2] - start[2]).abs() <= crate::nav::ARRIVE_Z;
        let walk = |mind: &mut Mind, to: Vec3| {
            mind.asked = Some(Asked {
                to,
                mode: Mode::Help,
                show_off: false,
                site: None,
                crouch: None,
                plain: true,
            });
            mind.practice.phase = Some(Phase::Walk);
            mind.practice.armed_since = None;
            mind.order_pad(p, me, map, seen)
        };
        if !there {
            self.practice.training.walks += 1;
            if self.practice.training.walks <= TRAIN_WALKS
                && let Some(pad) = walk(self, start)
            {
                return Some(pad);
            }
            self.train_try(Try::void("no_path"));
            self.practice_idle(me, tick, 1);
            return None;
        }
        // There: the walks count again from nothing (a wake-up walks off).
        self.practice.training.walks = 0;
        if !self.pair_word.is_some_and(is_primed) {
            let since = *self.practice.armed_since.get_or_insert(tick);
            if tick.wrapping_sub(since) > DRILL_ARM_TICKS {
                // No word yet: walk off and back to wake the body, as the
                // drill does.
                self.practice.training.wakes += 1;
                if self.practice.training.wakes > TRAIN_WAKES {
                    self.practice.armed_since = None;
                    self.train_try(Try::void("not_armed"));
                    self.practice_idle(me, tick, 1);
                    return None;
                }
                let t = site.target();
                let away = [start[0] - t[0], start[1] - t[1]];
                let l = away[0].hypot(away[1]).max(1e-3);
                let off = [
                    start[0] + away[0] / l * DRILL_WAKE,
                    start[1] + away[1] / l * DRILL_WAKE,
                    start[2],
                ];
                if let Some(pad) = walk(self, off) {
                    return Some(pad);
                }
                self.practice.armed_since = None;
            }
            self.practice.arm = true;
            self.practice_idle(me, tick, 1);
            return None;
        }
        // Armed at the start: the try.
        self.practice.armed_since = None;
        self.practice.training.walks = 0;
        self.practice.training.wakes = 0;
        self.asked = Some(Asked {
            to: site.bounce.stand,
            mode: Mode::Fun,
            show_off: false,
            site: Some(site),
            crouch: None,
            plain: false,
        });
        self.practice.route_crouch = None;
        self.practice.phase = Some(Phase::Route(site));
        self.practice.primed_seen = false;
        self.practice.event(PracticeEvent::Started {
            site: site.bounce.stand,
            tick,
        });
        if let Some(pad) = self.order_pad(p, me, map, seen) {
            self.practice_watch_prime(site, tick);
            return Some(pad);
        }
        // No route from here: tallied (void) next.
        self.train_tally(site);
        self.practice_idle(me, tick, DRILL_PAUSE);
        None
    }

    /// A training try's route is over: its one flight as the board takes
    /// it ([`Try::from_leap`], the miss from the site's own target), or
    /// void for an end that says nothing about the knobs; logged, tallied,
    /// and handed to the board.
    fn train_tally(&mut self, site: Site) {
        let view = self.order_view();
        let target = site.own_target();
        let first = view.flights.first();
        let t = match view.error {
            Some("died") => Try::from_leap(first, target, true),
            Some(e) if void_error(e) => Try::void(e),
            _ => Try::from_leap(first, target, false),
        };
        let bounced = t.bounced();
        let why = if bounced {
            None
        } else {
            Some(
                t.void
                    .or_else(|| first.and_then(|f| f.end))
                    .or(view.error)
                    .unwrap_or("no_landing"),
            )
        };
        eprintln!(
            "{}; knobs {}",
            try_line(true, self.practice.owner, site, why, &view).replacen(
                " drill site ",
                " train site ",
                1
            ),
            knobs_text(&site.knobs)
        );
        if t.scored() {
            self.practice.record(site.bounce.stand, bounced, why);
        }
        let tick = self.tick.unwrap_or(0);
        self.practice.event(PracticeEvent::Tried {
            site: site.bounce.stand,
            tick,
            bounced,
            why,
        });
        self.practice.last = Some(site.bounce.stand);
        self.train_try(t);
    }

    /// One try of the round held is in: the round ends when the search
    /// has no slot left for it.
    fn train_try(&mut self, t: Try) {
        let Some(i) = self.practice.training.held else {
            return;
        };
        let tr = &mut self.practice.training;
        tr.walks = 0;
        tr.wakes = 0;
        tr.done.push(t);
        let done = &self.practice.training.done;
        let over =
            on_board(self.practice.learn_board, |b| b.next(i, done).is_none()).unwrap_or(true);
        if over {
            self.train_fold(i);
        }
    }

    /// The round's end: the search folds it, the log gets its line, and
    /// the board's lines go to the map's learned file.
    fn train_fold(&mut self, i: usize) {
        let done = std::mem::take(&mut self.practice.training.done);
        self.practice.training.held = None;
        let folded = on_board(self.practice.learn_board, |b| {
            let f = b.fold(i, &done)?;
            Some((b.describe(i, &f), b.entries()))
        })
        .flatten();
        let Some((line, entries)) = folded else {
            return;
        };
        eprintln!("bots: train: {line}");
        let map = self.practice.training.map.clone();
        let stored = match &self.practice.learned_dir {
            Some(dir) => crate::spots::write_learned(dir, &map, &entries).map(|(p, _)| Some(p)),
            None => crate::spots::store_learned(&map, &entries),
        };
        if let Err(e) = stored {
            eprintln!("bots: train: {map}'s learned file not written: {e}");
        }
    }
}

#[cfg(test)]
mod tests;
