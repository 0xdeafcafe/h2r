//! The mind with bounce routes (docs/game/bots.md, "Superbounce routes"):
//! the glue between [`Mind::step`] and [`crate::bounce_route`].
//!
//! A route is asked for ([`Mind::order_bounce`]: from where the bot is to
//! a point, using a superbounce) or begun by a show-off. It is planned at
//! the next sample, from the bot's position and the map's nav mesh, and
//! played as the bot's whole pad ([`RoutePad`]) until it is over: the
//! bot does not fight, pick up or crouch of its own accord meanwhile. A
//! show-off route gives itself up when the bot sees an enemy.

use input::host::HostPadState;

use super::{Mind, Vec3};
use crate::bounce_route::{
    self, Ask, Leap, Logged, Mode, OrderRun, Touchdown, Why, launch_floor, path_length,
};
use crate::brain::{Me, Perception, Seen};
use crate::cache::MapData;
use crate::glitches::prime::is_primed;

/// A route asked for, not yet planned.
#[derive(Clone, Copy, Debug)]
pub(super) struct Asked {
    pub to: Vec3,
    pub mode: Mode,
    pub show_off: bool,
    /// A show-off's or a practice's one site: the plan looks at no other.
    pub site: Option<crate::spots::Site>,
    /// A practice's crouch spot for its site (`mind::practice`, paired
    /// once per site): the route primes there and nowhere else.
    pub crouch: Option<crate::spots::Crouch>,
    /// The walk alone, no bounce (a drill's walk to its start,
    /// `mind::practice`).
    pub plain: bool,
}

/// A route under way.
#[derive(Clone, Debug)]
pub(super) struct Ordered {
    run: OrderRun,
    to: Vec3,
    mode: Mode,
    /// A show-off's site, to give up on when the route fails.
    site: Option<Vec3>,
}

/// How the last route ended.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Ended {
    to: Vec3,
    mode: Mode,
    why: Why,
    steps: Vec<&'static str>,
    result: Result<(), &'static str>,
    /// The step it ended on.
    step: usize,
    landing: Option<Touchdown>,
    log: Vec<Logged>,
    site: Option<crate::spots::Site>,
    flights: Vec<Leap>,
}

/// Where a bot's bounce route stands, for the agent.
#[derive(Clone, Debug, PartialEq)]
pub struct OrderView {
    /// `none`, `queued` (asked, not yet planned), `running`, `done` or
    /// `failed`.
    pub state: &'static str,
    pub to: Option<Vec3>,
    pub mode: Option<Mode>,
    /// Why the plan is what it is (`plain`: no bounce helped).
    pub why: Option<Why>,
    /// The plan's steps, by name ([`crate::bounce_route::Step::name`]).
    pub steps: Vec<&'static str>,
    /// The step it is on.
    pub step: usize,
    /// The pair word is at -3: primed and not yet bounced.
    pub primed: bool,
    /// Whether that was read from the game or assumed after the
    /// crouch and stand-up.
    pub primed_read: bool,
    /// Why a failed route failed.
    pub error: Option<&'static str>,
    /// The step's phase ([`crate::bounce_route::OrderStatus::phase`]).
    pub phase: &'static str,
    /// The step's try (from 1; 0 for a step that does not retry).
    pub tries: u32,
    /// The last touchdown during a bounce or a drop.
    pub landing: Option<Touchdown>,
    /// What the route did, oldest first.
    pub log: Vec<Logged>,
    /// Its jumps and drops, oldest first ([`OrderRun::flights`]).
    pub flights: Vec<Leap>,
    /// The bounce the plan uses.
    pub site: Option<crate::spots::Site>,
    /// The practice loop's tally while the superbounce mode is on
    /// (`mind::practice`).
    pub practice: Option<super::PracticeView>,
}

impl Mind {
    /// Asks for a bounce route from where the bot is to `to`. Planned at
    /// the next sample; replaces any route under way.
    pub fn order_bounce(&mut self, to: Vec3, mode: Mode) {
        self.order = None;
        self.asked = Some(Asked {
            to,
            mode,
            show_off: false,
            site: None,
            crouch: None,
            plain: false,
        });
    }

    /// Drops the route, asked or under way, and records why (`died`,
    /// `map_changed`) so [`Mind::order_view`] can say.
    pub fn cancel_order(&mut self, why: &'static str) {
        let asked = self.asked.take();
        if let Some(o) = self.order.take() {
            self.end_order(o, Err(why), 0);
        } else if let Some(a) = asked {
            self.ended = Some(Ended::empty(a.to, a.mode, Why::Plain, Err(why)));
        }
    }

    /// A route is asked for or under way, so the host should read the
    /// pair word ([`crate::set_pair_reader`]).
    pub fn wants_pair(&self) -> bool {
        self.asked.is_some() || self.order.is_some() || self.practice.drilling()
    }

    /// The pair word is at -3 now, as far as the route knows: the crouch
    /// button stays up until the bounce.
    pub fn primed(&self) -> bool {
        self.order.as_ref().is_some_and(|o| o.run.primed())
    }

    /// The route under way's priming once its word is at -3: whether that
    /// was read from the game (else assumed), and whether the route primed
    /// it itself (it has a prime step) rather than starting primed. `None`
    /// before, after and with no route.
    pub(super) fn route_primed(&self) -> Option<(bool, bool)> {
        let o = self.order.as_ref()?;
        o.run.primed().then(|| {
            let own = o
                .run
                .plan
                .steps
                .iter()
                .any(|s| matches!(s, bounce_route::Step::Prime(_)));
            (o.run.status().read, own)
        })
    }

    /// The route, asked, under way or last over.
    pub fn order_view(&self) -> OrderView {
        let none = OrderView {
            state: "none",
            to: None,
            mode: None,
            why: None,
            steps: Vec::new(),
            step: 0,
            primed: false,
            primed_read: false,
            error: None,
            phase: "",
            tries: 0,
            landing: None,
            log: Vec::new(),
            flights: Vec::new(),
            site: None,
            practice: self.practice_view(),
        };
        if let Some(a) = self.asked {
            return OrderView {
                state: "queued",
                to: Some(a.to),
                mode: Some(a.mode),
                ..none
            };
        }
        if let Some(o) = &self.order {
            let s = o.run.status();
            return OrderView {
                state: "running",
                to: Some(o.to),
                mode: Some(o.mode),
                why: Some(o.run.plan.why),
                steps: o.run.plan.steps.iter().map(|s| s.name()).collect(),
                step: s.step,
                primed: s.primed,
                primed_read: s.read,
                phase: s.phase,
                tries: s.tries,
                landing: s.landing,
                log: o.run.log().to_vec(),
                flights: o.run.flights(),
                site: o.run.plan.site,
                ..none
            };
        }
        match &self.ended {
            Some(e) => OrderView {
                state: if e.result.is_ok() { "done" } else { "failed" },
                to: Some(e.to),
                mode: Some(e.mode),
                why: Some(e.why),
                steps: e.steps.clone(),
                step: e.step,
                error: e.result.err(),
                landing: e.landing,
                log: e.log.clone(),
                flights: e.flights.clone(),
                site: e.site,
                ..none
            },
            None => none,
        }
    }

    /// The route's pad this tick, when one is under way (planning it
    /// first when it was just asked for); `None` leaves the pad to the
    /// brain.
    pub(super) fn order_pad(
        &mut self,
        p: &Perception,
        me: Me,
        map: Option<&MapData>,
        seen: &[Seen],
    ) -> Option<HostPadState> {
        if let Some(a) = self.asked.take() {
            self.begin_order(p, me, map, a);
        }
        let mut o = self.order.take()?;
        let Some(m) = map else {
            self.end_order(o, Err("no_map"), p.tick);
            return None;
        };
        if o.run.show_off && !seen.is_empty() {
            self.end_order(o, Err("enemy"), p.tick);
            return None;
        }
        let out = o.run.step(m, me.position, p.tick, self.pair_word);
        if let Some(result) = out.done {
            self.end_order(o, result, p.tick);
            return None;
        }
        self.order = Some(o);
        let pad = self.route_pad.report(me.aim, p.tick, out.motion);
        self.last_pad = pad;
        Some(pad)
    }

    fn begin_order(&mut self, p: &Perception, me: Me, map: Option<&MapData>, a: Asked) {
        let Some(m) = map else {
            self.ended = Some(Ended::empty(a.to, a.mode, Why::Plain, Err("no_map")));
            return;
        };
        let walk = |from: Vec3, to: Vec3| path_length(m, from, to);
        let ledge = |s: &crate::spots::Site| launch_floor(m, s);
        let primed = self.pair_word.is_some_and(is_primed);
        let ask = Ask {
            from: me.position,
            to: a.to,
            primed,
            mode: a.mode,
        };
        let plan = match a.site {
            // A drill's walk to its start: the walk alone.
            _ if a.plain => bounce_route::Plan {
                steps: vec![bounce_route::Step::Nav { to: a.to }],
                why: Why::Plain,
                site: None,
                cost: None,
            },
            // A practice's site: the route by it alone, priming at the
            // crouch spot paired with it, over once down from the launch.
            Some(site) if !a.show_off => {
                let crouches = match a.crouch {
                    Some(c) => vec![c],
                    None => self.known.crouches.clone(),
                };
                let one = crate::spots::Known {
                    sites: vec![site],
                    crouches,
                };
                match bounce_route::plan_site(me.position, primed, site, &one, &walk, &ledge) {
                    Some(plan) => plan,
                    None => {
                        self.stuck.blacklist(site.bounce.stand, p.tick);
                        self.ended = Some(Ended::empty(a.to, a.mode, Why::Asked, Err("no_route")));
                        return;
                    }
                }
            }
            // A show-off looks at its one site.
            Some(site) => {
                let one = crate::spots::Known {
                    sites: vec![site],
                    crouches: self.known.crouches.clone(),
                };
                bounce_route::plan_with(&ask, &one, &walk, &ledge)
            }
            None => bounce_route::plan_with(&ask, &self.known, &walk, &ledge),
        };
        if (a.show_off || a.site.is_some()) && plan.site.is_none() {
            // No route by the one site (a show-off's, a practice's): give
            // the site up for a while.
            if let Some(site) = a.site {
                self.stuck.blacklist(site.bounce.stand, p.tick);
            }
            self.ended = Some(Ended::empty(a.to, a.mode, plan.why, Err("no_route")));
            return;
        }
        let seed = u64::from(p.tick) ^ 0xb0_0c5;
        // A plain walk keeps no word: it neither primes nor needs one.
        // Primed here is the word read at -3 (a drill's, set by the host).
        let mut run = OrderRun::new(plan, primed && !a.plain, p.tick, seed).primed_read();
        run.show_off = a.show_off;
        self.order = Some(Ordered {
            run,
            to: a.to,
            mode: a.mode,
            site: a.site.map(|s| s.bounce.stand),
        });
    }

    fn end_order(&mut self, o: Ordered, result: Result<(), &'static str>, tick: u32) {
        if result.is_err()
            && let Some(stand) = o.site
        {
            self.stuck.blacklist(stand, tick);
        }
        let status = o.run.status();
        self.ended = Some(Ended {
            to: o.to,
            mode: o.mode,
            why: o.run.plan.why,
            steps: o.run.plan.steps.iter().map(|s| s.name()).collect(),
            result,
            step: if result.is_ok() {
                status.steps
            } else {
                status.step
            },
            landing: status.landing,
            log: o.run.log().to_vec(),
            site: o.run.plan.site,
            flights: o.run.flights(),
        });
    }
}

impl Ended {
    fn empty(to: Vec3, mode: Mode, why: Why, result: Result<(), &'static str>) -> Ended {
        Ended {
            to,
            mode,
            why,
            steps: Vec::new(),
            result,
            step: 0,
            landing: None,
            log: Vec::new(),
            site: None,
            flights: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests;
