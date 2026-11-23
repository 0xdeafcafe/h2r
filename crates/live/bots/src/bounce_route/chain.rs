//! Chains and the flight net (docs/game/bots.md, "Chains" and "The
//! flight net"): after a bounce, the sites the bot can come down on from
//! its launch without touching the ground, so a practice route can plan
//! the next bounce of a chain ([`super::Step::Chain`]).
//!
//! A leg is a [`Trajectory`]: what a launch off one site can reach. The
//! one built in, [`Engine`], flies it on the flight model the HUD's
//! second bounce flies (`superbounce::flight`; docs/game/superbounce.md,
//! "The flight model" and "The second bounce"), by the same rule: the
//! launch straight up off the site, the measured gravity and air control,
//! a touchdown at the next site's need or faster, and a pursuit of the
//! next site's target that ends in its catch. So a bot and the HUD agree
//! on which seams a flight reaches.
//!
//! The [`Net`] is every leg between a map's sites, made once for a set of
//! sites: a site's row is the sites its launch reaches, proven ones first
//! (a bounce a run has proved, which the bots rely on), then the lowest
//! [`flight::cost`] (the HUD's rank). Sites no run has proved are in the
//! net, as links a bot takes seldom (`mind::practice::chain_pick`).

use super::CLEARANCE;
use crate::spots::{Site, dist_xy};
use superbounce::flight;

/// The most links a practice route plans after its first bounce.
pub const LINKS: usize = 3;
/// The longest leg (seconds): the HUD's second bounce ranks none longer
/// (`superbounce_second::MAX_TIME`).
pub const MAX_TIME: f32 = 8.0;

/// One leg of a chain: the flight off one site's launch down onto the
/// next.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Leg {
    /// Seconds from the launch to the touchdown (its last free row).
    pub time: f32,
    /// How far sideways (XY) the air stick must move the bot: from where
    /// the launch comes down with the stick let go to the next target.
    pub steer: f32,
    /// The touchdown's downward speed (world units a second).
    pub speed_in: f32,
    /// How far the pursuit of the next target comes down from it (world
    /// units, X and Y): within the catch.
    pub miss: f32,
    /// How far the full stick along the steer moves the landing
    /// (`superbounce::flight::reach_along`).
    pub reach: f32,
    /// [`flight::cost`]: the steer's share of the reach and the speed to
    /// spare, lower easier.
    pub cost: f32,
    /// The next site is proven (measured, or a verified entry: the bounce
    /// search's provable sites): a link the bot can rely on.
    pub sure: bool,
}

/// What a launch off a site can reach.
pub trait Trajectory {
    /// The leg off `from`'s launch down onto `to`, or `None` when the
    /// flight cannot come down on it.
    fn leg(&self, from: &Site, to: &Site) -> Option<Leg>;
}

/// The flight model's leg (`superbounce::flight`), by the HUD's second
/// bounce's rule.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Engine {
    /// How far from the next target (X and Y) the pursuit may come down:
    /// the catch (`superbounce::catch`).
    pub catch: f32,
    /// The longest leg, seconds ([`MAX_TIME`]).
    pub max_time: f32,
    /// The fastest touchdown a seam takes after a launch, when measured;
    /// `None`: any at or over the site's `need`.
    pub max_speed_in: Option<f32>,
}

impl Default for Engine {
    fn default() -> Engine {
        Engine {
            catch: superbounce::catch(),
            max_time: MAX_TIME,
            max_speed_in: None,
        }
    }
}

impl Trajectory for Engine {
    /// Off `from`'s target (where its bounce comes down) straight up at
    /// its launch, onto `to`'s target height:
    ///
    /// - with the stick let go it comes down within [`Engine::max_time`]
    ///   (a seam over the top is never reached), at `to`'s need or faster
    ///   and no faster than [`Engine::max_speed_in`];
    /// - `to` is not farther than the full stick could carry it in that
    ///   time (no flight needed to rule it out), and the pursuit of its
    ///   target (`superbounce::flight::pursue`) comes down within
    ///   [`Engine::catch`] of it;
    /// - a V's arm only from across its bisector.
    fn leg(&self, from: &Site, to: &Site) -> Option<Leg> {
        let v = from.launch;
        if v.is_nan() || v <= 0.0 {
            return None;
        }
        let start = from.target();
        let at = to.target();
        if !to.approached_from([start[0], start[1]]) {
            return None;
        }
        let floor = |_: [f32; 2]| at[2];
        let up = [0.0, 0.0, v];
        let down = flight::hold(start, up, [0.0; 2], floor)?;
        let time = down.time();
        if time > self.max_time {
            return None;
        }
        let speed_in = down.speed;
        if to.need > 0.0 && speed_in < to.need {
            return None;
        }
        if self.max_speed_in.is_some_and(|m| speed_in > m) {
            return None;
        }
        let landing = [down.row[0], down.row[1]];
        let gap = [at[0] - landing[0], at[1] - landing[1]];
        let steer = gap[0].hypot(gap[1]);
        if steer > flight::AIR_TOP * time + self.catch {
            return None;
        }
        let pursuit = flight::pursue(start, up, [at[0], at[1]], floor)?;
        if pursuit.miss > self.catch {
            return None;
        }
        let dir = if steer > 1e-6 {
            [gap[0] / steer, gap[1] / steer]
        } else {
            [0.0; 2]
        };
        let reach = flight::reach_along(start, up, landing, dir, floor);
        Some(Leg {
            time,
            steer,
            speed_in,
            miss: pursuit.miss,
            reach,
            cost: flight::cost(steer, reach, to.need, speed_in),
            sure: to.proven(),
        })
    }
}

/// The order a row's legs come in: proven sites first, then the lowest
/// cost, then the least steering.
fn order(a: &Leg, b: &Leg) -> std::cmp::Ordering {
    (!a.sure)
        .cmp(&!b.sure)
        .then(a.cost.total_cmp(&b.cost))
        .then(a.steer.total_cmp(&b.steer))
}

/// The sites a launch off `from` can come down on, with their legs,
/// proven first, then the easiest ([`Leg::cost`]). `from` itself is among
/// them (straight back down onto the same seam) when its own flight
/// allows it.
pub fn reachable(from: &Site, sites: &[Site], model: &dyn Trajectory) -> Vec<(Site, Leg)> {
    let mut out: Vec<(Site, Leg)> = sites
        .iter()
        .filter_map(|s| Some((*s, model.leg(from, s)?)))
        .collect();
    out.sort_by(|a, b| order(&a.1, &b.1));
    out
}

/// Two stands this near (world units) are one site.
pub const SAME: f32 = 0.05;

/// The flight net: for each of a set of sites, the sites its launch can
/// come down on ([`reachable`]). Only sites a route plays
/// ([`Site::plays`]: a candidate crease is the training's until it is
/// proved) are in it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Net {
    sites: Vec<Site>,
    rows: Vec<Vec<(usize, Leg)>>,
}

impl Net {
    /// The net over `sites` on `model`: every leg between two sites a
    /// route plays.
    pub fn build(sites: &[Site], model: &dyn Trajectory) -> Net {
        let sites: Vec<Site> = sites.iter().copied().filter(Site::plays).collect();
        let rows = sites
            .iter()
            .map(|from| {
                let mut row: Vec<(usize, Leg)> = sites
                    .iter()
                    .enumerate()
                    .filter_map(|(i, to)| Some((i, model.leg(from, to)?)))
                    .collect();
                row.sort_by(|a, b| order(&a.1, &b.1));
                row
            })
            .collect();
        Net { sites, rows }
    }

    /// The sites in the net.
    pub fn sites(&self) -> &[Site] {
        &self.sites
    }

    /// How many legs the net holds.
    pub fn legs(&self) -> usize {
        self.rows.iter().map(Vec::len).sum()
    }

    /// The net's index of `site`: the site whose stand is within [`SAME`]
    /// of its stand at the same height to [`CLEARANCE`].
    pub fn index(&self, site: &Site) -> Option<usize> {
        let p = site.bounce.stand;
        self.sites.iter().position(|s| {
            let q = s.bounce.stand;
            dist_xy(p, q) < SAME && (p[2] - q[2]).abs() < CLEARANCE
        })
    }

    /// The sites a launch off `site` comes down on, with their legs, in
    /// the row's order (proven first, then the easiest); none for a site
    /// not in the net.
    pub fn from(&self, site: &Site) -> Vec<(Site, Leg)> {
        self.index(site).map_or_else(Vec::new, |i| {
            self.rows[i]
                .iter()
                .map(|&(j, leg)| (self.sites[j], leg))
                .collect()
        })
    }
}

/// A chain of up to `links` sites after `first`: each link one of the
/// sites the last one's launch reaches in `net` ([`Net::from`]), chosen
/// by `choose` (an index into them, or `None` to stop there). The chain
/// stops at the first link with nothing in reach or nothing chosen.
pub fn plan(
    first: &Site,
    net: &Net,
    links: usize,
    choose: &mut dyn FnMut(&Site, &[(Site, Leg)]) -> Option<usize>,
) -> Vec<Site> {
    let mut out = Vec::new();
    let mut from = *first;
    for _ in 0..links {
        let near = net.from(&from);
        let Some(i) = choose(&from, &near).filter(|&i| i < near.len()) else {
            break;
        };
        from = near[i].0;
        out.push(from);
    }
    out
}

#[cfg(test)]
mod tests;
