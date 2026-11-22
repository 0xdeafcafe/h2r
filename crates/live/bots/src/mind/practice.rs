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
//! prime) is given up for the map ([`CROUCH_STRIKES`], [`SITE_STRIKES`]).
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
use crate::bounce_route::{Mode, Pairing, launch_floor, pair_site, path_length, prime_start};
use crate::brain::{Me, Perception, Seen};
use crate::cache::MapData;
use crate::glitches::prime::is_primed;
use crate::spots::{Crouch, Known, Site};

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
/// is given up for the map.
pub const SITE_STRIKES: u32 = 3;

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
    /// Failed tries by the crouch spot (its stand) and by the site, and
    /// the crouch spots given up, for the map in [`Practice::pairs_for`].
    crouch_strikes: HashMap<[u32; 3], u32>,
    site_strikes: HashMap<[u32; 3], u32>,
    gone: Vec<[u32; 3]>,
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
        }
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
            if matches!(self.practice.phase, Some(Phase::Route(_))) {
                self.cancel_order("practice_off");
            }
            self.practice.phase = None;
            claim(
                self.practice.board,
                self.practice.owner,
                None,
                self.tick.unwrap_or(0),
            );
        }
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
        if !primed && self.known.crouches.is_empty() {
            return Pick::Nothing;
        }
        let for_map = pairs_key(&self.known, m);
        if self.practice.pairs_for != for_map {
            self.practice.pairs.clear();
            self.practice.out_of_reach.clear();
            self.practice.crouch_strikes.clear();
            self.practice.site_strikes.clear();
            self.practice.gone.clear();
            self.practice.pairs_for = for_map;
        }
        self.practice
            .out_of_reach
            .retain(|&(_, until)| !reached(tick, until));
        let owner = self.practice.owner;
        let last = self.practice.last;
        let mut ranked: Vec<(f32, Site)> = Vec::new();
        for s in &self.known.sites {
            let k = stand_key(s.bounce.stand);
            if dist_xy(s.bounce.stand, at) > REACH
                || self.stuck.blocked(s.bounce.stand, tick)
                || self.practice.out_of_reach.iter().any(|(o, _)| *o == k)
                || match self.practice.pairs.get(&k) {
                    // Known to have no route: never tried.
                    Some(Err(_)) => true,
                    Some(Ok(p)) => p.crouch.is_none() && !primed,
                    None => false,
                }
            {
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
                    let gone = &self.practice.gone;
                    let usable = |c: &Crouch| !gone.contains(&stand_key(c.stand));
                    let p = pair_site(&s, &self.known, &walk, &ledge, &usable);
                    self.practice.pairs.insert(k, p);
                    p
                }
            };
            let Ok(p) = pairing else { continue };
            let (to, crouch) = match (primed, p.crouch) {
                (true, _) => (p.approach, None),
                (false, Some(c)) => (prime_start(&c), Some(c)),
                (false, None) => continue,
            };
            if walks >= REACH_QUERIES {
                return Pick::Later;
            }
            walks += 1;
            if walk(at, to).is_some() {
                return Pick::Site(s, crouch);
            }
            self.practice
                .out_of_reach
                .push((k, tick.wrapping_add(OUT_OF_REACH_TICKS)));
        }
        Pick::Nothing
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
    /// ([`CROUCH_STRIKES`], [`SITE_STRIKES`]) is given up for the map. A
    /// landing that did not bounce counts against neither.
    fn practice_strike(
        &mut self,
        site: Site,
        steps: &[&'static str],
        step: usize,
        why: Option<&'static str>,
    ) {
        let Some(why) = why else { return };
        let walked = why.starts_with("path_lost")
            || matches!(why, "no_path" | "goal_off_mesh" | "tail_stuck");
        let at = steps.get(step).copied();
        let next = steps.get(step + 1).copied();
        let crouch_leg = (walked && next == Some("prime")) || at == Some("prime");
        let site_leg = walked && matches!(next, Some("bounce" | "drop"));
        let p = &mut self.practice;
        if crouch_leg && let Some(c) = p.route_crouch {
            let k = stand_key(c.stand);
            let n = p.crouch_strikes.entry(k).or_insert(0);
            *n += 1;
            if *n >= CROUCH_STRIKES && !p.gone.contains(&k) {
                p.gone.push(k);
                // Its sites pair again, without it.
                p.pairs
                    .retain(|_, v| !matches!(v, Ok(Pairing { crouch: Some(c), .. }) if stand_key(c.stand) == k));
            }
        }
        if site_leg {
            let k = stand_key(site.bounce.stand);
            let n = p.site_strikes.entry(k).or_insert(0);
            *n += 1;
            if *n >= SITE_STRIKES {
                p.pairs.insert(k, Err("given_up"));
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
            Some(Phase::Idle { .. }) => {}
        }
        // Standing between tries: looking round, slowly, about one way.
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
}

#[cfg(test)]
mod tests;
