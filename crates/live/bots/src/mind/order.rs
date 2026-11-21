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
use crate::brain::{Me, Perception, Seen};
use crate::bounce_route::{self, Ask, Mode, OrderRun, Why};
use crate::cache::MapData;
use crate::glitches::prime::is_primed;

/// A route asked for, not yet planned.
#[derive(Clone, Copy, Debug)]
pub(super) struct Asked {
    pub to: Vec3,
    pub mode: Mode,
    pub show_off: bool,
}

/// A route under way.
#[derive(Clone, Debug)]
pub(super) struct Ordered {
    run: OrderRun,
    to: Vec3,
    mode: Mode,
}

/// How the last route ended.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Ended {
    to: Vec3,
    mode: Mode,
    why: Why,
    steps: Vec<&'static str>,
    result: Result<(), &'static str>,
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
        });
    }

    /// Drops the route, asked or under way.
    pub fn cancel_order(&mut self) {
        self.order = None;
        self.asked = None;
    }

    /// The pair word is at -3 now, as far as the route knows: the crouch
    /// button stays up until the bounce.
    pub fn primed(&self) -> bool {
        self.order.as_ref().is_some_and(|o| o.run.primed())
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
                step: e.steps.len(),
                error: e.result.err(),
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
            self.end_order(o, Err("no_map"));
            return None;
        };
        if o.run.show_off && !seen.is_empty() {
            self.end_order(o, Err("enemy"));
            return None;
        }
        let out = o.run.step(m, me.position, p.tick, self.pair_word);
        if let Some(result) = out.done {
            self.end_order(o, result);
            return None;
        }
        self.order = Some(o);
        let pad = self.route_pad.report(me.aim, p.tick, out.motion);
        self.last_pad = pad;
        Some(pad)
    }

    fn begin_order(&mut self, p: &Perception, me: Me, map: Option<&MapData>, a: Asked) {
        let Some(m) = map else {
            self.ended = Some(Ended {
                to: a.to,
                mode: a.mode,
                why: Why::Plain,
                steps: Vec::new(),
                result: Err("no_map"),
            });
            return;
        };
        let walk = |from: Vec3, to: Vec3| path_length(m, from, to);
        let primed = self.pair_word.is_some_and(is_primed);
        let ask = Ask {
            from: me.position,
            to: a.to,
            primed,
            mode: a.mode,
        };
        let plan = bounce_route::plan(&ask, &self.known, &walk);
        let seed = u64::from(p.tick) ^ 0xb0_0c5;
        let mut run = OrderRun::new(plan, primed, p.tick, seed);
        run.show_off = a.show_off;
        self.order = Some(Ordered {
            run,
            to: a.to,
            mode: a.mode,
        });
    }

    fn end_order(&mut self, o: Ordered, result: Result<(), &'static str>) {
        self.ended = Some(Ended {
            to: o.to,
            mode: o.mode,
            why: o.run.plan.why,
            steps: o.run.plan.steps.iter().map(|s| s.name()).collect(),
            result,
        });
    }
}

/// The length of the walk along the nav mesh from `from` to `to`.
fn path_length(m: &MapData, from: Vec3, to: Vec3) -> Option<f32> {
    let path = m.nav.path(from, to)?;
    let mut at = from;
    let mut total = 0.0;
    for w in path {
        total += ((w.at[0] - at[0]).powi(2) + (w.at[1] - at[1]).powi(2) + (w.at[2] - at[2]).powi(2))
            .sqrt();
        at = w.at;
    }
    Some(total)
}

#[cfg(test)]
mod tests;
