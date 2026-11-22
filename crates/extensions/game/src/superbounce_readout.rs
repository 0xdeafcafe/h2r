//! The superbounce readout in the game (docs/game/hud-kit.md, "The
//! superbounce readout"): `hud_kit_core`'s readout HUD, registered with
//! the HUD kit ([`crate::hud_kit`]) after the superbounce HUD and fed
//! each tick from [`superbounce_mode`](crate::superbounce_mode)'s state
//! through one adapter, [`view`].
//!
//! Everything it shows is drawn by Halo 2's own HUD: three rows of the
//! player's HUD definition right of the motion tracker, and three of the
//! game's navpoints. Which pieces show is the variant's
//! ([`crate::superbounce_options`]): a piece a variant turned off is left
//! out of the view.
//!
//! Why the last landing did not bounce ([`miss`]) is worked out here,
//! beside the tracker's verdict (`superbounce_charge::Landing`): the seam
//! is the one the landing was by, the distance is the origin's to its
//! edge, and the speed in is the fall speed on the tick before touchdown
//! (a short history of the state's, [`History`]) carried down to the
//! seam's height, as the tracker's own. The line stays [`INFO_TICKS`] and
//! goes when the next fall toward a seam begins ([`Info`]).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use hud_kit_core::huds::superbounce_readout::{
    self as rd, Fall, Miss, Next, Readout, Speed, Steer, View,
};
use hud_kit_core::model::Values;
use superbounce::Spot;

use crate::hud_kit::{self, Handle};
use crate::superbounce_charge::{GRAVITY, Landing, LandingResult, edge_distance_xy, need_of};
use crate::superbounce_mode::{self as mode, METRES_PER_UNIT, State};
use crate::superbounce_options::Options;
use crate::superbounce_second::Ranked;

/// The feature's id.
pub const FEATURE: &str = "superbounce-readout";

/// Ticks the miss line stays: four seconds.
pub const INFO_TICKS: u64 = 120;
/// Ticks of fall speed kept: over a second, more than the landing's
/// launch window.
pub const HISTORY: usize = 48;

/// A direction in the world (`dx`, `dy`) against the aim `yaw`: radians
/// clockwise from straight ahead, in (-pi, pi]. World yaw turns from +X
/// toward +Y, which is to the left.
pub fn against_aim(dx: f32, dy: f32, yaw: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let mut cw = (yaw - dy.atan2(dx)).rem_euclid(TAU);
    if cw > PI {
        cw -= TAU;
    }
    cw
}

/// The speed line's motion for `s`: the horizontal speed and its way
/// against the aim, and the fall speed while airborne and falling.
pub fn speed(s: &State) -> Option<Speed> {
    let v = s.velocity?;
    let down = (s.airborne && v[2] < 0.0).then(|| -v[2] * METRES_PER_UNIT);
    Some(Speed {
        heading: against_aim(v[0], v[1], s.yaw.unwrap_or(0.0)),
        across: v[0].hypot(v[1]) * METRES_PER_UNIT,
        down,
    })
}

/// The fall row for `s`: while airborne with a predicted landing, the
/// touchdown speed over the seam's need, and the coasting landing against
/// the seam: caught, or the way from it to the seam's nearest point
/// against the aim, and how far.
pub fn fall(s: &State) -> Option<Fall> {
    let p = s.landing.filter(|_| s.airborne)?;
    let ratio = if p.need > 0.0 { p.speed / p.need } else { 1.0 };
    let steer = if p.miss() <= 0.0 {
        Steer::OnSeam
    } else {
        let to = p.crease_point();
        let (dx, dy) = (to[0] - p.at[0], to[1] - p.at[1]);
        Steer::Off {
            heading: against_aim(dx, dy, s.yaw.unwrap_or(0.0)),
            metres: dx.hypot(dy) * METRES_PER_UNIT,
        }
    };
    Some(Fall {
        speed: ratio,
        steer,
    })
}

/// The fall row for the second bounce's best seam `r`: the touchdown
/// speed the flight gives there over its need, and the line-up: caught
/// coasting, or the way to steer against the aim `yaw` and how far.
pub fn line_up(r: &Ranked, yaw: f32) -> Fall {
    Fall {
        speed: if r.need > 0.0 { r.speed / r.need } else { 1.0 },
        steer: if r.caught {
            Steer::OnSeam
        } else {
            Steer::Off {
                heading: against_aim(r.steer[0], r.steer[1], yaw),
                metres: r.steer[0].hypot(r.steer[1]) * METRES_PER_UNIT,
            }
        },
    }
}

/// The ranked line's seams for `s`: each ranked seam's bearing against
/// the aim and its distance in metres.
pub fn next(s: &State) -> [Option<Next>; rd::NEXT] {
    let yaw = s.yaw.unwrap_or(0.0);
    s.second.map(|r| {
        let (r, p) = (r?, s.position?);
        Some(Next {
            heading: against_aim(r.at[0] - p[0], r.at[1] - p[1], yaw),
            metres: r.distance * METRES_PER_UNIT,
        })
    })
}

/// The readout's [`View`] of the mode's state `s` under the variant's
/// options `o`, with the miss line `info`.
///
/// - The speed line: [`speed`], with `velocity`.
/// - The fall row: in the flight after a bounce the best ranked seam's
///   ([`line_up`]), else [`fall`]; with `alignment`.
/// - The miss line: `info`, with `miss_info`.
/// - The ranked line: [`next`], the state's `second` (which the mode
///   leaves empty with `second_bounce` off).
/// - The navpoints: the mode's marks (`superbounce_targets::Marks`), each
///   with its distance in metres.
pub fn view(s: &State, o: Options, info: Option<Miss>) -> View {
    let metres =
        |m: Option<crate::superbounce_targets::Mark>| m.map(|m| m.distance * METRES_PER_UNIT);
    View {
        speed: speed(s).filter(|_| o.velocity),
        fall: s.second[0]
            .map(|r| line_up(&r, s.yaw.unwrap_or(0.0)))
            .or_else(|| fall(s))
            .filter(|_| o.alignment),
        info: info.filter(|_| o.miss_info),
        next: next(s),
        tag: metres(s.marks.tag),
        targets: s.marks.targets.map(metres),
    }
}

/// The values with the mode off: every element hidden.
pub fn hidden() -> Values {
    rd::values(&View::default())
}

/// The fall speed of the last ticks, by game tick.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct History {
    ticks: VecDeque<(u32, f32)>,
}

impl History {
    /// Keeps `vz` (world units a second, up positive) for `tick`; the
    /// oldest go past [`HISTORY`]. A tick not after the last (a new game)
    /// starts the history again.
    pub fn push(&mut self, tick: u32, vz: f32) {
        if self.ticks.back().is_some_and(|(t, _)| *t >= tick) {
            self.ticks.clear();
        }
        self.ticks.push_back((tick, vz));
        while self.ticks.len() > HISTORY {
            self.ticks.pop_front();
        }
    }

    /// The vertical speed on the tick before `touchdown`.
    pub fn before(&self, touchdown: u32) -> Option<f32> {
        let want = touchdown.checked_sub(1)?;
        self.ticks.iter().find(|(t, _)| *t == want).map(|(_, v)| *v)
    }

    pub fn clear(&mut self) {
        self.ticks.clear();
    }
}

/// The speed in (world units a second, positive) of a touchdown whose
/// origin was at height `from` with vertical speed `vz_before` on the
/// tick before, onto a seam at height `to`: the fall carried down under
/// gravity, as `superbounce_charge::strip_landing`'s.
pub fn speed_in(vz_before: f32, from: f32, to: f32) -> f32 {
    let height = (from - to).max(0.0);
    (vz_before * vz_before + 2.0 * GRAVITY * height).sqrt()
}

/// Why `landing` did not bounce, with its numbers in metres and metres a
/// second: by the seam of `spots` it was by
/// (`superbounce_charge::landing_spot`), with the vertical speed
/// `vz_before` on the tick before. `None` for a bounce, and when the seam
/// or a speed the line needs is not known.
pub fn miss(landing: &Landing, spots: &[Spot], vz_before: Option<f32>) -> Option<Miss> {
    let spot = || crate::superbounce_charge::landing_spot(spots, landing.at).map(|i| &spots[i]);
    let had =
        |s: &Spot| vz_before.map(|vz| speed_in(vz, landing.at[2], s.stand[2]) * METRES_PER_UNIT);
    match landing.result {
        LandingResult::Bounce => None,
        LandingResult::NotPrimed => Some(Miss::NotPrimed),
        LandingResult::Missed => {
            let s = spot()?;
            Some(Miss::Missed {
                by: edge_distance_xy(s, [landing.at[0], landing.at[1]]) * METRES_PER_UNIT,
            })
        }
        LandingResult::TooSlow => {
            let s = spot()?;
            Some(Miss::TooSlow {
                had: had(s)?,
                need: need_of(s) * METRES_PER_UNIT,
            })
        }
        LandingResult::NoBounce => Some(Miss::NoBounce { had: had(spot()?)? }),
    }
}

/// The miss line, tick to tick.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Info {
    /// The state has been seen once: the landing it held then is not
    /// news.
    seen: bool,
    /// The last landing's tick.
    landing: Option<u32>,
    /// The line, and the readout's tick it came up on.
    shown: Option<(Miss, u64)>,
}

impl Info {
    /// One tick, the readout's `now`th: a landing filed since the last
    /// tick puts its line up (`miss` works it out; a bounce takes the
    /// line down), a fall toward a seam on a later tick takes it down,
    /// and it goes after [`INFO_TICKS`]. The line to show.
    pub fn step(
        &mut self,
        s: &State,
        now: u64,
        miss: impl FnOnce(&Landing) -> Option<Miss>,
    ) -> Option<Miss> {
        let landing = s.last_landing.as_ref();
        let tick = landing.map(|l| l.tick);
        if let Some(l) = landing.filter(|_| self.seen && tick != self.landing) {
            self.shown = miss(l).map(|m| (m, now));
        } else if s.airborne && s.landing.is_some() {
            self.shown = None;
        }
        self.seen = true;
        self.landing = tick.or(self.landing);
        if self
            .shown
            .is_some_and(|(_, at)| now.saturating_sub(at) >= INFO_TICKS)
        {
            self.shown = None;
        }
        self.shown.map(|(m, _)| m)
    }
}

struct Live {
    handle: Handle,
    history: History,
    info: Info,
    ticks: u64,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Live {
    fn tick(&mut self) {
        self.ticks += 1;
        let Some(s) = mode::hud_state() else {
            self.history.clear();
            self.info = Info::default();
            self.handle.publish(hidden());
            for id in [rd::TAG, rd::TARGET_1, rd::TARGET_2] {
                self.handle.navpoint_at(id, None);
            }
            return;
        };
        if let (Some(tick), Some(vz)) = (s.tick, s.vz) {
            self.history.push(tick, vz);
        }
        let history = &self.history;
        let info = self.info.step(&s, self.ticks, |l| {
            miss(l, &mode::current_spots(), history.before(l.tick))
        });
        let v = view(&s, crate::superbounce_options::current(), info);
        self.handle.publish(rd::values(&v));
        self.handle.navpoint_at(rd::TAG, s.marks.tag.map(|m| m.at));
        for (id, m) in rd::TARGETS.iter().zip(s.marks.targets) {
            self.handle.navpoint_at(id, m.map(|m| m.at));
        }
    }
}

struct SuperbounceReadout {
    live: Arc<Mutex<Live>>,
}

impl hooks::Feature for SuperbounceReadout {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(FEATURE, "Superbounce readout on the HUD kit")
            .doc("docs/game/hud-kit.md")
            .presentation_only()
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        // Rebuilt from the mode's state each tick; the miss line and the
        // speed history are presentation only.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        let live = Arc::clone(&self.live);
        b.frame()
            .on_tick("superbounce readout: values", move |_| lock(&live).tick());
        Ok(())
    }
}

/// Registers the readout with the HUD kit and adds the feature that feeds
/// it. Called after the superbounce HUD is registered
/// (`superbounce_hud::install`), so that HUD's navpoint is the kit's
/// first and the readout's three follow it.
pub fn install(registry: &mut hooks::Registry) -> anyhow::Result<()> {
    let handle = hud_kit::register(Arc::new(Readout));
    handle.publish(hidden());
    let live = Live {
        handle,
        history: History::default(),
        info: Info::default(),
        ticks: 0,
    };
    registry
        .add(&SuperbounceReadout {
            live: Arc::new(Mutex::new(live)),
        })
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("superbounce readout: {e}"))
}

#[cfg(test)]
#[path = "superbounce_readout_tests.rs"]
mod tests;
