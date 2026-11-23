//! The superbounce approach guide's rules (docs/game/superbounce.md, "The
//! approach guide"): where the player's path touches down, whether that
//! is lined up with the seam they are going for, and, in a fall, whether
//! the landing will bounce. All of it is pure; the mode runs it each tick
//! ([`guide`]) and the readout HUD draws it as world marks
//! (`hud_kit_core::huds::superbounce_guide`).
//!
//! - **The impact** ([`impact`]): the body's free flight stepped a game
//!   tick at a time under the sim's fall model
//!   (`sim_bench::drops::FALL_GRAVITY`: the fall speed gains 0.13904
//!   units/s a tick and the body moves that speed over 30) until the next
//!   tick would touch the seam's surface ([`Seam::contact`]). The tick
//!   before is the **last free row**: its place is the landing the
//!   pipper marks, and its height decides the bounce. On the ground the
//!   flight is a jump from where the player stands ([`jump`]).
//! - **Lined up** ([`caught`]): the landing is within the seam's catch
//!   (`superbounce::catch_miss`).
//! - **Depth** ([`depth`]): the one function that says whether the last
//!   free row ends low enough over the seam for the next tick to go deep
//!   into it. Its constants are measured on one seam
//!   (docs/game/notes/2026-11-22-seam-drop-depth.md).
//! - **The call** ([`call`]): what the box says of the landing.
//! - **The jump cue** ([`takeoff`], [`settle`]): on the ground, the
//!   takeoff line, where to press jump for the landing to bounce, solved
//!   along the body's path, and the count to it, `Jump in 0.7 s`, then
//!   `Jump now`; standing, a walk-up's line and `Walk to line`.
//! - **The chance** ([`chance`]): in the air, the bounce's predicted
//!   percentage, how sure the call is ([`confidence`]). A seam with no
//!   measured landing is capped ([`SCAN_ONLY_CAP`]).
//! - **Visibility** ([`strength`]): full while primed or within
//!   [`NEAR`] of the seam, fading out to [`FAR`].

use hud_kit_core::huds::superbounce_guide::{self as marks, Call, Cue, Layout};
use superbounce::{Kind, Spot, Vec3};

use crate::superbounce_assist::{drift_unit, stand_near};

/// The fall model's gravity (units/s²) and tick, the sim's: the fall
/// speed gains `GRAVITY × TICK` a tick, and the body moves the new speed
/// times `TICK` (`sim_bench::drops::fall_model`).
pub const GRAVITY: f32 = sim_bench::drops::FALL_GRAVITY;
pub const TICK: f32 = 1.0 / 30.0;
/// On a flat floor the body's position stops this far over the floor's
/// height at the touchdown (`sim_bench::drops::FALL_CLEARANCE`).
pub const CLEARANCE: f32 = sim_bench::drops::FALL_CLEARANCE;
/// A standing jump's launch (units/s up), as the bots' routes have it.
pub const JUMP_VZ: f32 = bots::bounce_route::JUMP_VZ;
/// The longest flight stepped: five seconds.
pub const MAX_TICKS: u32 = 150;
/// A last free row this far under the surface at its own place is not a
/// landing: the body never got over the seam's floor.
pub const UNDER: f32 = 0.05;

/// Not primed, the guide is at full strength within this distance of the
/// seam's landing point, and gone at [`FAR`]. Primed, it is at full
/// strength at any distance.
pub const NEAR: f32 = 4.0;
pub const FAR: f32 = 6.0;
/// The pipper shows while the landing is within this (X and Y) of the
/// seam's landing point: the floor it is predicted on is the seam's own,
/// which says nothing further out.
pub const PIPPER_RANGE: f32 = 3.0;

/// The seam's far face, as the depth rule has it: for each unit past the
/// edge along the drift the body is stopped this much higher. Measured
/// at Ascension's proven seam, where the cut between the last free rows
/// that bounced and those that did not runs along `z − 0.75·y`
/// (docs/game/notes/2026-11-22-seam-drop-depth.md).
pub const SEAM_SLOPE: f32 = 0.75;
/// The depth cut: a last free row bounces when its height over the edge,
/// less [`SEAM_SLOPE`] times its place past the edge, is under this. The
/// note's `z − 0.75·y < −5.755` with the seam's edge at y 4.0, z −3.2:
/// `−5.755 + 0.75 × 4.0 + 3.2`.
pub const DEPTH_CUT: f32 = 0.445;
/// How far under the cut the body is stopped ([`Seam::contact`]): what
/// makes a row the last free one. Inferred from the note's tables, not
/// measured on its own: their lowest last free rows end 0.10 to 0.11
/// under the cut (z −2.875 at y 3.97 to 3.99) and their highest 0.04
/// over it (z −2.738), and a tick falls 0.19 against the cut's line, so
/// the body is stopped 0.11 to 0.15 under the cut.
pub const DEPTH_BAND: f32 = 0.135;
/// The landing window: the last free row's place past the edge along the
/// drift (negative is before it, the side the landing comes from). The
/// note's y 3.93 to 3.99 against the edge at y 4.0; outside it no drop
/// bounced at any depth.
pub const LANDING_WINDOW: [f32; 2] = [-0.07, -0.01];
/// The slowest touchdown (units/s down) the depth rule is called at:
/// just under the note's slowest drop, 5.24. Its drops came in at 5.24
/// to 5.39; faster falls take the same cut, which no drop has measured.
pub const DEPTH_SPEED: f32 = 5.2;
/// The drift hint: the arrow is lit while the body moves along the
/// seam's drift faster than this. The note's bounces came at drifts of
/// 0.1 to 0.8 units/s, so the drift has no window of its own: the right
/// way, at any speed, is all the hint asks.
pub const DRIFT_HINT: f32 = 0.0;

/// A press of jump leaves the floor this many ticks later: the drill
/// runs' jumps left it 4 to 6 ticks after the press
/// (`bots::bounce_route::INPUT_LAG`,
/// docs/game/notes/2026-11-21-bot-bounce-sim.md). The takeoff line is
/// where to press, the ground of this many ticks short of where the body
/// leaves the floor.
pub const PRESS_LEAD: u32 = bots::bounce_route::INPUT_LAG;
/// `Jump now` shows from this many ticks before the body reaches the
/// line's band, and through the band: the tolerance for the eye and the
/// thumb, which act a frame or more after the cue shows.
pub const NOW_EARLY: u32 = 1;
/// The furthest along the run the line is looked for: three seconds.
pub const LOOK_AHEAD: u32 = 90;
/// Under this ground speed (units/s) the body is standing, and the line
/// is a walk-up's ([`WALK_UP`]).
pub const MOVING: f32 = 0.05;
/// Standing, or not moving toward a line, the line is for a walk up the
/// seam's drift at the speed the fall assist crosses a strip at
/// (`superbounce_assist::ARRIVE_SPEED`).
pub const WALK_UP: f32 = crate::superbounce_assist::ARRIVE_SPEED;
/// Takeoffs are tried this far apart along the run.
pub const TAKEOFF_STEP: f32 = 0.005;
/// A stretch of the run this long is passed over when the landings from
/// both its ends are clear of the landing window on the same side
/// ([`TAKEOFF_CLEAR`]): only near the window is every step tried.
pub const TAKEOFF_COARSE: f32 = 0.04;
/// A landing this far outside the landing window, or outside the catch,
/// is clear of it.
pub const TAKEOFF_CLEAR: f32 = 0.12;
/// The band is looked for from this far behind the body, so a band the
/// body is inside stays where it is.
pub const TAKEOFF_BEHIND: f32 = 0.3;
/// The cue holds a count's step up, `Jump now` against a count of 0.1,
/// and a count against no line, for at most this many ticks ([`settle`]).
pub const CUE_GRACE: u8 = 3;
/// The most a fall is promised, and one minus this the least: no number
/// of the depth rule's is sure enough for 100 or for 0.
pub const CONFIDENCE_TOP: f32 = 0.95;
/// How soft the depth cut is to the confidence: a last free row this far
/// under the cut reads 84%, as far over it 16%, on it 50%. Inferred, not
/// measured: the cut itself is found to ±0.003
/// (docs/game/notes/2026-11-22-seam-drop-depth.md), and the rest is the
/// predicted row's own error, which no run has measured. At this width
/// the middle of the note's bounce band, 0.04 to 0.05 under the cut,
/// reads 93 to 94%.
pub const DEPTH_SOFT: f32 = 0.02;
/// How soft the landing window's ends are to the confidence: a row this
/// far inside an end reads 84%, on it 50%. Inferred from the note's
/// spacing: y 3.93 bounced and 3.92 did not, so an end is known to about
/// 0.01. The window's middle, 0.03 from each end, reads 95%.
pub const WINDOW_SOFT: f32 = 0.01;
/// The most the chance says on a seam with no measured landing (a
/// scan's `stand`, `Spot::landed` clear). The depth rule's cut and
/// window are Ascension's proven seam's and are measured nowhere else
/// (docs/game/notes/2026-11-22-seam-drop-depth.md), so on a seam no run
/// has landed on, the rule itself is a guess.
pub const SCAN_ONLY_CAP: f32 = 0.6;

/// The seam the guide is on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seam {
    /// The collision edge.
    pub edge: [Vec3; 2],
    /// The landing point: the box's centre (the spot's `stand`, moved
    /// along a long strip to where the landing comes down).
    pub point: Vec3,
    /// The drift, a unit vector; zero for a seam with none.
    pub drift: [f32; 2],
    /// A run has measured a landing on it (`Spot::landed`): the chance
    /// is not capped at [`SCAN_ONLY_CAP`].
    pub measured: bool,
}

impl Seam {
    /// `spot` as a seam, its landing point the one nearest `xy`
    /// (`superbounce_assist::stand_near`).
    pub fn of(spot: &Spot, xy: [f32; 2]) -> Seam {
        Seam {
            edge: spot.edge,
            point: stand_near(spot, xy),
            drift: drift_unit(spot.drift),
            measured: spot.landed,
        }
    }

    /// The point of the edge nearest `xy`.
    pub fn edge_point(&self, xy: [f32; 2]) -> Vec3 {
        let [a, b] = self.edge;
        let e = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let len2 = e[0] * e[0] + e[1] * e[1];
        let t = if len2 > 1e-12 && len2.is_finite() {
            (((xy[0] - a[0]) * e[0] + (xy[1] - a[1]) * e[1]) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        [a[0] + t * e[0], a[1] + t * e[1], a[2] + t * e[2]]
    }

    /// The edge's direction, flat; +X for an edge with no length.
    pub fn along(&self) -> [f32; 2] {
        let [a, b] = self.edge;
        hud_kit_core::world::unit([b[0] - a[0], b[1] - a[1]])
    }

    /// Whether the seam has a drift, and so a far face and a depth.
    pub fn drifts(&self) -> bool {
        self.drift != [0.0; 2]
    }

    /// `p` in the seam's own frame: how far past the edge it is along the
    /// drift (0 for a seam with none), and how high over the edge.
    pub fn frame(&self, p: Vec3) -> [f32; 2] {
        let q = self.edge_point([p[0], p[1]]);
        let past = (p[0] - q[0]) * self.drift[0] + (p[1] - q[1]) * self.drift[1];
        [past, p[2] - q[2]]
    }

    /// The height the body's position is stopped at over `xy`: the
    /// seam's floor ([`CLEARANCE`] over the edge's height), or where its
    /// far face stops it first, [`DEPTH_CUT`] less [`DEPTH_BAND`] over
    /// the edge and [`SEAM_SLOPE`] higher for each unit past it.
    pub fn contact(&self, xy: [f32; 2]) -> f32 {
        let q = self.edge_point(xy);
        let floor = CLEARANCE;
        if !self.drifts() {
            return q[2] + floor;
        }
        let past = (xy[0] - q[0]) * self.drift[0] + (xy[1] - q[1]) * self.drift[1];
        q[2] + floor.max(DEPTH_CUT - DEPTH_BAND + SEAM_SLOPE * past)
    }
}

/// Where a free flight touches down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Impact {
    /// The last free row: the body's position on the tick before the
    /// contact tick.
    pub row: Vec3,
    /// Game ticks from now to that row.
    pub ticks: u32,
    /// The fall speed on that row, units/s down: the touchdown speed.
    pub speed: f32,
}

/// The touchdown of a body at `position` moving `velocity`, in free
/// flight onto the surface `contact` (the height the body is stopped at
/// over a point, [`Seam::contact`]): each tick the fall speed gains
/// [`GRAVITY`]`×`[`TICK`] and the body moves by its velocity times
/// [`TICK`], and the last row whose next would be at or under the surface
/// is the [`Impact`]. `None` when the flight does not come down onto the
/// surface within [`MAX_TICKS`], or only reaches it from under
/// ([`UNDER`]).
pub fn impact(position: Vec3, velocity: Vec3, contact: impl Fn([f32; 2]) -> f32) -> Option<Impact> {
    if !position.iter().chain(&velocity).all(|v| v.is_finite()) {
        return None;
    }
    let step = [velocity[0] * TICK, velocity[1] * TICK];
    let (mut p, mut down) = (position, -velocity[2]);
    for ticks in 0..=MAX_TICKS {
        let next_down = down + GRAVITY * TICK;
        let next = [p[0] + step[0], p[1] + step[1], p[2] - next_down * TICK];
        if next_down > 0.0 && next[2] <= contact([next[0], next[1]]) {
            let over = p[2] - contact([p[0], p[1]]);
            return (over >= -UNDER).then_some(Impact {
                row: p,
                ticks,
                speed: down.max(0.0),
            });
        }
        (p, down) = (next, next_down);
    }
    None
}

/// The velocity of a jump now by a body moving `velocity` on the ground:
/// its horizontal speed kept, [`JUMP_VZ`] up.
pub fn jump(velocity: Vec3) -> Vec3 {
    [velocity[0], velocity[1], JUMP_VZ]
}

/// How deep a landing goes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Depth {
    /// How far under [`DEPTH_CUT`] the last free row ends (negative:
    /// over it, too shallow).
    pub margin: f32,
    /// The row's place past the edge along the drift.
    pub past: f32,
}

impl Depth {
    /// The last free row ends under the cut: the next tick goes deep
    /// enough into the seam.
    pub fn deep(&self) -> bool {
        self.margin > 0.0
    }
    /// The row is in the landing window ([`LANDING_WINDOW`]).
    pub fn in_window(&self) -> bool {
        (LANDING_WINDOW[0]..=LANDING_WINDOW[1]).contains(&self.past)
    }
    /// How far inside the landing window the row is, from its nearer
    /// end (negative: outside).
    pub fn window_margin(&self) -> f32 {
        (self.past - LANDING_WINDOW[0]).min(LANDING_WINDOW[1] - self.past)
    }
}

/// The depth rule: for a landing on `seam` whose last free row is
/// `impact.row`, how far under the cut the row ends, its height over the
/// edge less [`SEAM_SLOPE`] times its place past it against
/// [`DEPTH_CUT`]. The phase of the fall's ticks sets it, and that is
/// fixed once the body is in the air, so it is known before touchdown.
/// `None` for a seam with no drift: it has no far face to be deep into.
///
/// This is the one place the bounce's depth condition lives; a seam or a
/// speed that measures differently changes the constants here.
pub fn depth(seam: &Seam, impact: &Impact) -> Option<Depth> {
    if !seam.drifts() {
        return None;
    }
    let [past, over] = seam.frame(impact.row);
    Some(Depth {
        margin: DEPTH_CUT - (over - SEAM_SLOPE * past),
        past,
    })
}

/// Lined up: the landing is within the seam's catch, either side of its
/// edge.
pub fn caught(seam: &Seam, impact: &Impact) -> bool {
    superbounce::catch_miss(seam.edge, [impact.row[0], impact.row[1]]) <= 0.0
}

/// Whether the drift arrow is lit for a body moving `across` along the
/// seam's drift (units/s): the right way ([`DRIFT_HINT`]).
pub fn drift_lit(across: f32) -> bool {
    across > DRIFT_HINT
}

/// What the box says of `impact` on `seam`.
///
/// - Off the catch, or with no landing: [`Call::OffLine`].
/// - On the ground (the landing is a jump's from here): [`Call::LinedUp`].
///   A jump's phase is not called: it moves with every step.
/// - In the air, primed, on a seam with a depth, at [`DEPTH_SPEED`] or
///   faster: [`Call::WillBounce`] when the row is in the landing window
///   and deep, [`Call::TooShallow`] when it is not deep.
/// - Else [`Call::LinedUp`]: caught, with nothing measured to call.
pub fn call(seam: &Seam, impact: Option<&Impact>, airborne: bool, primed: bool) -> Call {
    let Some(i) = impact.filter(|i| caught(seam, i)) else {
        return Call::OffLine;
    };
    let called = airborne && primed && i.speed >= DEPTH_SPEED;
    match depth(seam, i).filter(|_| called) {
        Some(d) if !d.deep() => Call::TooShallow,
        Some(d) if d.in_window() => Call::WillBounce,
        _ => Call::LinedUp,
    }
}

/// How sure a fall's call is, before the seam's cap ([`cap`]): 0.5 on
/// the depth cut or on an end of the landing window, rising smoothly to
/// [`CONFIDENCE_TOP`] deep and in the window's middle, falling to one
/// minus it shallow or outside. The nearer of the two margins counts,
/// each over its own softness ([`DEPTH_SOFT`], [`WINDOW_SOFT`]), through
/// `tanh`. A landing outside the catch (`caught` false) is at the
/// bottom.
///
/// A fall is near-deterministic, so this is not a share of anything: it
/// is how far the predicted last free row is from the lines that decide
/// it, against how well those lines are known.
pub fn confidence(depth: &Depth, caught: bool) -> f32 {
    let low = 1.0 - CONFIDENCE_TOP;
    if !caught || depth.margin.is_nan() || depth.past.is_nan() {
        return low;
    }
    let m = (depth.margin / DEPTH_SOFT).min(depth.window_margin() / WINDOW_SOFT);
    0.5 + (CONFIDENCE_TOP - 0.5) * m.tanh()
}

/// Where a jump bounces from, on the ground: the takeoff line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Takeoff {
    /// The line's centre: the middle of the band of places to press
    /// jump on, on the ground ([`CLEARANCE`] under the body).
    pub at: Vec3,
    /// The run's direction, a flat unit: the line lies across it.
    pub heading: [f32; 2],
    /// Half the band's length along the run.
    pub half: f32,
    /// The run's ground speed the line is for (units/s).
    pub speed: f32,
    /// How far along the run the band's near and far ends are from the
    /// body (negative: behind it); `None` for a walk-up's line
    /// ([`WALK_UP`]).
    pub ahead: Option<[f32; 2]>,
}

impl Takeoff {
    /// A walk-up's line: the body is not moving toward a band.
    pub fn walk(&self) -> bool {
        self.ahead.is_none()
    }

    /// Ticks until `Jump now`: the ticks of the run to the band's near
    /// end, rounded up, less [`NOW_EARLY`]; 0 within that of the band
    /// and inside it. `None` for a walk-up's line.
    pub fn jump_in(&self) -> Option<u32> {
        let [near, _] = self.ahead?;
        let step = self.speed * TICK;
        if !(step > 0.0 && near.is_finite()) {
            return None;
        }
        let ticks = (near / step).ceil().max(0.0) as u32;
        Some(ticks.saturating_sub(NOW_EARLY))
    }

    /// The cue for this line, before [`settle`]: `Walk to line` for a
    /// walk-up's, `Jump now` at 0 ticks, else the count in tenths of a
    /// second (`marks::tenths`).
    pub fn cue(&self) -> Cue {
        match self.jump_in() {
            None => Cue::Walk,
            Some(0) => Cue::Now,
            Some(t) => Cue::In(marks::tenths(t)),
        }
    }
}

/// How a landing stands against `seam`'s landing window, for the coarse
/// pass of [`band`]: `None` near the window and in or near the catch,
/// where every step is tried; else which side it is clear on (short,
/// long, off either end of the edge, or no landing at all).
fn clear(seam: &Seam, landing: Option<&Impact>) -> Option<u8> {
    let Some(i) = landing else { return Some(0) };
    let row = [i.row[0], i.row[1]];
    let past = seam.frame(i.row)[0];
    if past < LANDING_WINDOW[0] - TAKEOFF_CLEAR {
        Some(1)
    } else if past > LANDING_WINDOW[1] + TAKEOFF_CLEAR {
        Some(2)
    } else if superbounce::catch_miss(seam.edge, row) > TAKEOFF_CLEAR {
        let [a, b] = seam.edge;
        let t = (row[0] - a[0]) * (b[0] - a[0]) + (row[1] - a[1]) * (b[1] - a[1]);
        Some(if t < 0.0 { 3 } else { 4 })
    } else {
        None
    }
}

/// The first band of takeoffs along the run from `origin` the unit
/// `dir`, at distances `range`, whose jump at `velocity` would bounce on
/// `seam` (as [`call`] says in a primed fall), and whose far end is at or
/// past `keep`: as distances along the run, each end widened by half a
/// step ([`TAKEOFF_STEP`]). Every takeoff keeps `origin`'s height.
/// `coarse` passes over stretches clear of the window ([`clear`]); the
/// band is the same without it, which a test checks.
fn band_by(
    seam: &Seam,
    origin: Vec3,
    dir: [f32; 2],
    velocity: Vec3,
    range: [f32; 2],
    keep: f32,
    coarse: bool,
) -> Option<[f32; 2]> {
    let at = |d: f32| [origin[0] + dir[0] * d, origin[1] + dir[1] * d, origin[2]];
    let land = |d: f32| impact(at(d), velocity, |xy| seam.contact(xy));
    let side = |d: f32| clear(seam, land(d).as_ref());
    let d_of = |i: u32| range[0] + i as f32 * TAKEOFF_STEP;
    let span = range[1] - range[0];
    if !(span >= 0.0 && span.is_finite()) {
        return None;
    }
    let n = (span / TAKEOFF_STEP).floor() as u32;
    let skip = (TAKEOFF_COARSE / TAKEOFF_STEP).round() as u32;
    let (mut start, mut last) = (None::<f32>, 0.0f32);
    let mut i = 0;
    while i <= n {
        let d = d_of(i);
        if coarse && start.is_none() && i + skip <= n {
            let a = side(d);
            if a.is_some() && a == side(d_of(i + skip)) {
                i += skip;
                continue;
            }
        }
        if land(d).is_some_and(|l| call(seam, Some(&l), true, true) == Call::WillBounce) {
            start = start.or(Some(d));
            last = d;
        } else if let Some(s) = start.take()
            && last >= keep
        {
            return Some([s - TAKEOFF_STEP / 2.0, last + TAKEOFF_STEP / 2.0]);
        }
        i += 1;
    }
    start
        .filter(|_| last >= keep)
        .map(|s| [s - TAKEOFF_STEP / 2.0, last + TAKEOFF_STEP / 2.0])
}

/// [`band_by`] with its coarse pass.
fn band(
    seam: &Seam,
    origin: Vec3,
    dir: [f32; 2],
    velocity: Vec3,
    range: [f32; 2],
    keep: f32,
) -> Option<[f32; 2]> {
    band_by(seam, origin, dir, velocity, range, keep, true)
}

/// The takeoff line for a body on the ground at `position` moving
/// `velocity`, on `seam` (docs/game/superbounce.md, "The jump cue").
///
/// - **Moving** ([`MOVING`] or faster): along the body's path,
///   `position + velocity × t` at its height, the first band of places
///   whose jump (the run's speed kept, [`jump`]) would bounce, within
///   [`LOOK_AHEAD`] ticks of the run. The band is of takeoffs; the line
///   is [`PRESS_LEAD`] ticks of the run short of it, where to press.
/// - **Standing, or no band ahead**: the line for a walk up the seam's
///   drift at [`WALK_UP`], toward its landing point, at the body's
///   height. It has no count ([`Takeoff::walk`]).
///
/// `None` for a seam with no drift, and where no jump from the body's
/// height bounces.
pub fn takeoff(seam: &Seam, position: Vec3, velocity: Vec3) -> Option<Takeoff> {
    if !seam.drifts() || !position.iter().chain(&velocity).all(|v| v.is_finite()) {
        return None;
    }
    let ground = position[2] - CLEARANCE;
    let line = |origin: Vec3, dir: [f32; 2], [lo, hi]: [f32; 2], speed: f32, ahead| {
        let mid = (lo + hi) / 2.0;
        Takeoff {
            at: [origin[0] + dir[0] * mid, origin[1] + dir[1] * mid, ground],
            heading: dir,
            half: (hi - lo) / 2.0,
            speed,
            ahead,
        }
    };
    let lead = |speed: f32| speed * PRESS_LEAD as f32 * TICK;
    let speed = velocity[0].hypot(velocity[1]);
    if speed >= MOVING {
        let dir = [velocity[0] / speed, velocity[1] / speed];
        let (lead, reach) = (lead(speed), speed * LOOK_AHEAD as f32 * TICK);
        let found = band(
            seam,
            position,
            dir,
            jump(velocity),
            [lead - TAKEOFF_BEHIND, lead + reach],
            lead,
        );
        if let Some([lo, hi]) = found {
            let press = [lo - lead, hi - lead];
            return Some(line(position, dir, press, speed, Some(press)));
        }
    }
    // A walk up the drift to the seam's landing point, from as far back
    // as the longest flight carries.
    let dir = seam.drift;
    let reach = WALK_UP * MAX_TICKS as f32 * TICK;
    let origin = [
        seam.point[0] - dir[0] * reach,
        seam.point[1] - dir[1] * reach,
        position[2],
    ];
    let walk = [dir[0] * WALK_UP, dir[1] * WALK_UP, JUMP_VZ];
    let [lo, hi] = band(seam, origin, dir, walk, [0.0, reach], 0.0)?;
    let lead = lead(WALK_UP);
    Some(line(origin, dir, [lo - lead, hi - lead], WALK_UP, None))
}

/// The cue shown this tick, and how many ticks it has been held, from
/// the last tick's shown cue `prev` (held `held` ticks) and this tick's
/// own `raw`. For [`CUE_GRACE`] ticks at most it holds:
///
/// - a count against a count a tenth higher (a step back up is the
///   count's own jitter);
/// - `Jump now` against a count of 0.1;
/// - a count against no line, or a walk-up's.
///
/// Anything else shows at once: a count down, a new line, the walk-up,
/// and the end of `Jump now`, which never outlasts its band.
pub fn settle(prev: Option<Cue>, held: u8, raw: Option<Cue>) -> (Option<Cue>, u8) {
    let hold = match (prev, raw) {
        (Some(Cue::In(p)), Some(Cue::In(r))) => r == p.saturating_add(1),
        (Some(Cue::Now), Some(Cue::In(1))) => true,
        (Some(Cue::In(_)), None | Some(Cue::Walk)) => true,
        _ => false,
    };
    if hold && held < CUE_GRACE {
        (prev, held + 1)
    } else {
        (raw, 0)
    }
}

/// The bounce's predicted chance in a fall.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Chance {
    /// 0 to 1, after the seam's cap.
    pub value: f32,
    /// The seam has no measured landing and the value was over
    /// [`SCAN_ONLY_CAP`].
    pub capped: bool,
}

/// `value` under the seam's evidence: as it is on a seam with a measured
/// landing, at most [`SCAN_ONLY_CAP`] on one without; and whether that
/// cut it.
pub fn cap(value: f32, measured: bool) -> (f32, bool) {
    let capped = !measured && value > SCAN_ONLY_CAP;
    (if capped { SCAN_ONLY_CAP } else { value }, capped)
}

/// The chance that `body`'s fall bounces on `seam`, with `impact` the
/// fall's touchdown: at [`DEPTH_SPEED`] or faster, the [`confidence`] of
/// the landing's depth, capped by the seam's evidence ([`cap`]).
///
/// `None` on the ground (the jump cue says when to jump instead,
/// [`takeoff`]), and where nothing bounces or nothing is measured: not
/// primed, a seam with no drift, no landing by the seam, a fall slower
/// than the depth rule is measured at.
pub fn chance(seam: &Seam, body: &Body, impact: Option<&Impact>) -> Option<Chance> {
    if !body.primed || !body.airborne {
        return None;
    }
    let i = impact.filter(|i| i.speed >= DEPTH_SPEED)?;
    let (value, capped) = cap(confidence(&depth(seam, i)?, caught(seam, i)), seam.measured);
    Some(Chance { value, capped })
}

/// How strongly the guide draws for a player `distance` from the seam's
/// landing point: 1 while primed, or within [`NEAR`]; fading to 0 at
/// [`FAR`].
pub fn strength(primed: bool, distance: f32) -> f32 {
    if primed {
        1.0
    } else if !distance.is_finite() {
        0.0
    } else {
        ((FAR - distance) / (FAR - NEAR)).clamp(0.0, 1.0)
    }
}

/// The seam the guide is on, by its landing point, of these in order:
/// the seam a fall is predicted onto (`landing`), while primed the chosen
/// strip (`chosen`), else the nearest of `targets` (the tag and the
/// marked seams) within [`FAR`] of `position`.
pub fn target(
    position: Vec3,
    primed: bool,
    landing: Option<Vec3>,
    chosen: Option<Vec3>,
    targets: &[Vec3],
) -> Option<Vec3> {
    let far = |p: &Vec3| {
        ((p[0] - position[0]).powi(2) + (p[1] - position[1]).powi(2) + (p[2] - position[2]).powi(2))
            .sqrt()
    };
    landing.or(chosen.filter(|_| primed)).or_else(|| {
        targets
            .iter()
            .filter(|p| far(p) <= FAR)
            .min_by(|a, b| far(a).total_cmp(&far(b)))
            .copied()
    })
}

/// The bounce spot of `spots` whose landing point is `at` (a navpoint's,
/// a predicted landing's or a target's `stand`), the first of them.
pub fn spot_at<'a>(spots: impl IntoIterator<Item = &'a Spot>, at: Vec3) -> Option<&'a Spot> {
    spots.into_iter().find(|s| {
        s.kind == Kind::Bounce && (0..3).all(|k| (s.stand[k] - at[k]).abs() <= SAME)
    })
}

/// Two landing points this close on each axis are one seam's.
pub const SAME: f32 = 1e-3;

/// The player, for the guide.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Body {
    pub position: Vec3,
    pub velocity: Vec3,
    pub airborne: bool,
    pub primed: bool,
}

/// The guide on one seam, as of a tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Guide {
    pub seam: Seam,
    /// The touchdown: of the fall in the air, of a jump now on the
    /// ground. `None` when the path does not come down by the seam
    /// ([`PIPPER_RANGE`]).
    pub impact: Option<Impact>,
    /// The impact is a jump's from the ground, not a flight under way.
    pub jump: bool,
    pub call: Call,
    /// The landing's depth, on a seam that has one. On the ground it is
    /// the jump's from this very place, an estimate.
    pub depth: Option<Depth>,
    /// The body's speed along the seam's drift (units/s); `None` for a
    /// seam with none.
    pub across: Option<f32>,
    /// The bounce's predicted chance in a fall ([`chance`]).
    pub chance: Option<Chance>,
    /// On the ground while primed, the takeoff line ([`takeoff`]).
    pub takeoff: Option<Takeoff>,
    /// The jump cue on the box: the takeoff line's, settled against the
    /// last tick's ([`Guide::settled`]).
    pub cue: Option<Cue>,
    /// Ticks the cue has been held against this tick's own ([`settle`]).
    pub held: u8,
    /// The fade ([`strength`]).
    pub strength: f32,
}

impl Guide {
    /// Where the pipper is: the last free row's place on the seam's
    /// floor.
    pub fn pipper(&self) -> Option<Vec3> {
        self.impact.map(|i| [i.row[0], i.row[1], self.seam.point[2]])
    }

    /// The landing is within the catch.
    pub fn caught(&self) -> bool {
        self.impact.is_some_and(|i| caught(&self.seam, &i))
    }

    /// This guide with its cue settled against the last tick's guide
    /// `prev` ([`settle`]), when that was on the same seam (its edge) and
    /// on the ground too; else the cue is this tick's own.
    pub fn settled(mut self, prev: Option<&Guide>) -> Guide {
        let prev = prev.filter(|p| p.seam.edge == self.seam.edge && p.jump && self.jump);
        let (cue, held) = settle(prev.and_then(|p| p.cue), prev.map_or(0, |p| p.held), self.cue);
        self.cue = cue;
        self.held = held;
        self
    }

    /// What the marks show.
    pub fn view(&self) -> marks::Guide {
        marks::Guide {
            call: self.call,
            pipper: self.impact.is_some(),
            ticks: self.impact.is_some() && !self.caught(),
            drift: self.across.map(drift_lit),
            chance: self.chance.map(|c| marks::percent(c.value)),
            cue: self.cue,
            takeoff: self.takeoff.is_some(),
        }
    }

    /// Where the marks lie.
    pub fn layout(&self) -> Layout {
        let [a, b] = self.seam.edge;
        let half = (b[0] - a[0]).hypot(b[1] - a[1]) / 2.0;
        Layout {
            seam: self.seam.point,
            along: self.seam.along(),
            // A busy point's edge has no length: the full box.
            half_along: if half > 1e-4 { half.min(marks::BOX_ALONG) } else { marks::BOX_ALONG },
            drift: self.seam.drift,
            impact: self.pipper(),
            takeoff: self.takeoff.map(|t| marks::Line {
                at: t.at,
                heading: t.heading,
                half: t.half,
            }),
            strength: self.strength,
        }
    }
}

/// The guide for `body` on the bounce spot `spot`, or `None` when it is
/// not shown ([`strength`] 0, or `spot` is not a bounce spot). Its cue is
/// this tick's own; the mode settles it against the last tick's
/// ([`Guide::settled`]).
pub fn guide(body: &Body, spot: &Spot) -> Option<Guide> {
    if spot.kind != Kind::Bounce {
        return None;
    }
    let at = [body.position[0], body.position[1]];
    let velocity = if body.airborne { body.velocity } else { jump(body.velocity) };
    // The surface is the edge's and the drift's, whatever the landing
    // point; the point then slides along the strip to the landing.
    let surface = Seam::of(spot, at);
    let flight = impact(body.position, velocity, |xy| surface.contact(xy));
    let seam = Seam::of(spot, flight.map_or(at, |i| [i.row[0], i.row[1]]));
    let d = [
        seam.point[0] - body.position[0],
        seam.point[1] - body.position[1],
        seam.point[2] - body.position[2],
    ];
    let strength = strength(body.primed, (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt());
    if strength <= 0.0 {
        return None;
    }
    let impact = flight.filter(|i| {
        (i.row[0] - seam.point[0]).hypot(i.row[1] - seam.point[1]) <= PIPPER_RANGE
    });
    let takeoff = (body.primed && !body.airborne)
        .then(|| takeoff(&seam, body.position, body.velocity))
        .flatten();
    Some(Guide {
        seam,
        impact,
        chance: chance(&seam, body, impact.as_ref()),
        takeoff,
        cue: takeoff.map(|t| t.cue()),
        held: 0,
        jump: !body.airborne,
        call: call(&seam, impact.as_ref(), body.airborne, body.primed),
        depth: impact.and_then(|i| depth(&seam, &i)),
        across: seam
            .drifts()
            .then(|| body.velocity[0] * seam.drift[0] + body.velocity[1] * seam.drift[1]),
        strength,
    })
}

#[cfg(test)]
#[path = "superbounce_guide_tests.rs"]
mod tests;
