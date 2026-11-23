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
use hud_kit_core::huds::superbounce_guide as guide_marks;
use hud_kit_core::huds::superbounce_segments::{self as segs, Segments};
use hud_kit_core::model::Values;
use hud_kit_core::world::Pose;
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
/// Ticks a miss line holds against another: a second miss within two
/// seconds of it replaces it only with a higher speed in.
pub const MISS_KEEP_TICKS: u64 = 60;
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
/// the seam's landing point (`Predicted::aim`): off the seam, the way
/// from it to the point against the aim, and how far; caught and coming
/// down still or against the seam's drift, the way to drift; else caught.
pub fn fall(s: &State) -> Option<Fall> {
    let p = s.landing.filter(|_| s.airborne)?;
    let yaw = s.yaw.unwrap_or(0.0);
    let ratio = if p.need > 0.0 { p.speed / p.need } else { 1.0 };
    let steer = if p.miss() > 0.0 {
        let (dx, dy) = (p.aim[0] - p.at[0], p.aim[1] - p.at[1]);
        Steer::Off {
            heading: against_aim(dx, dy, yaw),
            metres: dx.hypot(dy) * METRES_PER_UNIT,
        }
    } else if p.wants_drift() {
        Steer::Drift {
            heading: against_aim(p.drift[0], p.drift[1], yaw),
        }
    } else {
        Steer::OnSeam
    };
    Some(Fall {
        speed: ratio,
        steer,
    })
}

/// The fall row for the second bounce's best seam `r`: the touchdown
/// speed the flight gives there over its need, and the line-up as
/// [`fall`]'s: off the seam, the way to steer against the aim `yaw` and
/// how far; caught and wanting its drift, the way to drift; else caught.
pub fn line_up(r: &Ranked, yaw: f32) -> Fall {
    Fall {
        speed: if r.need > 0.0 { r.speed / r.need } else { 1.0 },
        steer: if !r.caught {
            Steer::Off {
                heading: against_aim(r.steer[0], r.steer[1], yaw),
                metres: r.steer[0].hypot(r.steer[1]) * METRES_PER_UNIT,
            }
        } else if crate::superbounce_assist::wants_drift(r.drift, r.across) {
            Steer::Drift {
                heading: against_aim(r.drift[0], r.drift[1], yaw),
            }
        } else {
            Steer::OnSeam
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

/// The readout's name for a target mode.
pub fn kinds(m: crate::superbounce_options::TargetMode) -> rd::Kinds {
    use crate::superbounce_options::TargetMode;
    match m {
        TargetMode::Common => rd::Kinds::Common,
        TargetMode::Likely => rd::Kinds::Likely,
        TargetMode::All => rd::Kinds::All,
    }
}

/// Half the angle, either side of the aim, a seam's navpoint is kept
/// within: 40 degrees.
pub const TARGET_CONE: f32 = 40.0 * std::f32::consts::PI / 180.0;

/// The mode's target navpoints for `s` that are within [`TARGET_CONE`] of
/// the aim. The game clamps an off-screen navpoint to the edge of its
/// waypoint area, where its arrow lands on the tracker, the readout or the
/// score plate; one behind or to the side is cleared instead. With no
/// position or aim known, all are kept.
pub fn ahead(s: &State) -> [Option<crate::superbounce_targets::Mark>; 2] {
    s.marks.targets.map(|m| {
        let m = m?;
        let (Some(p), Some(yaw)) = (s.position, s.yaw) else {
            return Some(m);
        };
        (against_aim(m.at[0] - p[0], m.at[1] - p[1], yaw).abs() <= TARGET_CONE).then_some(m)
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
/// - The switch's line: the kinds the player has just switched the
///   targets to (`superbounce_targets::Switch`), over the other lines.
/// - The navpoints: the mode's marks (`superbounce_targets::Marks`), each
///   with its distance in metres.
/// - The approach guide: the state's `guide`, with `approach_guide`. On
///   the ground its jump cue is the box's label; in a fall its verdict is.
///   No label shows its chance.
/// - The charge segments: [`segments`], with `charge_segments`.
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
        kinds: s.switched.map(kinds),
        tag: metres(s.marks.tag),
        targets: ahead(s).map(metres),
        guide: s.guide.filter(|_| o.approach_guide).map(|g| g.view()),
        segments: segments(s).filter(|_| o.charge_segments),
    }
}

/// The charge segments for `s` (docs/game/hud-kit.md, "The charge
/// segments"): the mode's charge as its stage and how many are lit,
/// once the map is scanned and has a spot; `None`, hidden, before that
/// and on a map with none. In the air too: primed is worth seeing on the
/// way down.
pub fn segments(s: &State) -> Option<Segments> {
    let ready = matches!(s.scan, mode::ScanState::Ready) && s.bounce + s.crouch > 0;
    ready.then(|| segs::segments(crate::superbounce_hud::charge(s)))
}

/// The approach guide's marks' poses for `s` under `o`: each of
/// `guide_marks::MARKS` with its pose, `None` for one with nothing to
/// lie on.
pub fn guide_poses(
    s: &State,
    o: Options,
) -> [(&'static str, Option<Pose>); guide_marks::MARKS.len()] {
    let poses = s
        .guide
        .filter(|_| o.approach_guide)
        .map(|g| guide_marks::poses(&g.layout()))
        .unwrap_or_default();
    guide_marks::MARKS.map(|id| (id, poses.iter().find(|(i, _)| *i == id).map(|(_, p)| *p)))
}

/// `hudg` `waypoint_bottom_inset` (`+0xf0`,
/// `crates/formats/tags/src/defs/hudg.txt`): how far the waypoint area's
/// bottom is moved in from the safe bounds.
pub const HUDG_WAYPOINT_BOTTOM_INSET: u32 = 0xf0;
/// The bottom inset with the mode on, in the HUD's 480-high frame: the
/// area's bottom clears the motion tracker, the speed row and the score
/// plate (y 376 and down), so the one target arrow the game clamps to the
/// edge ([`ahead`]) sits above them.
pub const WAYPOINT_BOTTOM_INSET: f32 = 80.0;

/// The inset to write for a `hudg` that holds `cur` with `orig` the map's
/// own: the mode's ([`WAYPOINT_BOTTOM_INSET`], never lowering the map's)
/// while `on`, else the map's.
pub fn waypoint_inset(orig: f32, on: bool) -> f32 {
    if on {
        orig.max(WAYPOINT_BOTTOM_INSET)
    } else {
        orig
    }
}

/// The `hudg` last seen, the map's own bottom inset, and the value this
/// wrote there.
static INSET: Mutex<Option<(u32, f32, f32)>> = Mutex::new(None);

/// Keeps the loaded `hudg`'s waypoint bottom inset at [`waypoint_inset`]
/// for the mode `on` or off, each tick. The map's own value is read once
/// per `hudg` and put back when the mode goes off. A map with no `hudg`
/// loaded is left alone.
pub fn keep_waypoint_inset(mem: &mut dyn engine::memory::GuestMemory, on: bool) {
    let Some(h) = mem
        .u32(engine::points::HUD_GLOBALS_TAG.base)
        .ok()
        .filter(|&h| h != 0)
    else {
        return;
    };
    let at = h.wrapping_add(HUDG_WAYPOINT_BOTTOM_INSET);
    let Ok(cur) = mem.f32(at) else {
        return;
    };
    let mut seen = lock(&INSET);
    let orig = match *seen {
        // The value this wrote: the map's is still the one kept.
        Some((addr, orig, wrote)) if addr == h && cur == wrote => orig,
        _ => cur,
    };
    let want = waypoint_inset(orig, on);
    if cur != want && mem.set_f32(at, want).is_err() {
        return;
    }
    *seen = Some((h, orig, want));
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

/// The speed in a miss line carries, metres a second; 0 for one with none.
fn speed_in_of(m: &Miss) -> f32 {
    match *m {
        Miss::TooSlow { had, .. } | Miss::NoBounce { had } => had,
        _ => 0.0,
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
    /// line down; a miss within [`MISS_KEEP_TICKS`] of the line shown
    /// replaces it only with a higher speed in), a fall toward a seam on a later tick takes it down,
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
            // A touchdown at a seam's foot (a hop, a stumble) files a
            // landing too; it does not wipe out the real miss before it.
            self.shown = match (miss(l), self.shown) {
                (Some(new), Some((old, at)))
                    if now.saturating_sub(at) < MISS_KEEP_TICKS
                        && speed_in_of(&new) <= speed_in_of(&old) =>
                {
                    Some((old, at))
                }
                (new, _) => new.map(|m| (m, now)),
            };
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
            for id in guide_marks::MARKS {
                self.handle.world_mark(id, None);
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
        let o = crate::superbounce_options::current();
        let v = view(&s, o, info);
        self.handle.publish(rd::values(&v));
        for (id, pose) in guide_poses(&s, o) {
            self.handle.world_mark(id, pose);
        }
        self.handle.navpoint_at(rd::TAG, s.marks.tag.map(|m| m.at));
        for (id, m) in rd::TARGETS.iter().zip(ahead(&s)) {
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
