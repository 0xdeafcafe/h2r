//! The superbounce approach guide's rules (docs/game/superbounce.md, "The
//! approach guide"): where the player's path touches down, whether that
//! is lined up with the seam they are going for, and, in a fall, whether
//! the landing will bounce. All of it is pure; the mode runs it each tick
//! ([`guide`]) and the readout HUD draws it as world marks
//! (`hud_kit_core::huds::superbounce_guide`).
//!
//! - **The impact** ([`impact`]): the body's origin flown on the shared
//!   flight model (`superbounce_flight::fly`: the sim's fall, and the air
//!   stick driving the sideways velocity) until the next tick would touch
//!   the seam's surface ([`Seam::contact`]). The surface is the bounding
//!   sphere's centre's, as the depth rule was measured, so the origin
//!   comes down onto it less the body's lift (the centre over the
//!   origin). The tick before is the **last free row**: its place is the
//!   landing the pipper marks, and its centre's height decides the
//!   bounce. On the ground the flight is a jump pressed on the takeoff's
//!   line, shown only while the cue counts to it ([`guide_from`]).
//! - **Lined up** ([`caught`]): the landing is within the seam's catch
//!   (`superbounce::catch_miss`).
//! - **Depth** ([`depth`]): the one function that says whether the last
//!   free row ends low enough over the seam for the next tick to go deep
//!   into it. Its constants are measured on one seam
//!   (docs/game/notes/2026-11-22-seam-drop-depth.md).
//! - **The call** ([`call`]): what the box says of the landing.
//! - **The takeoff** ([`place`]): a fixed place on a real floor to leave
//!   for the seam, from a run's lip or the map's ([`Lip`]), flown as a
//!   walk-off and as a jump; the one that lands in the catch is kept.
//! - **The jump cue** ([`Cue`], [`settle`]): on the ground, what to do
//!   to reach the takeoff and leave it, by where the body is against it:
//!   `Climb up`, `Walk to line`, `Jump in 0.7 s`, `Jump now`, `Walk off`.
//! - **The chance** ([`chance`]): in the air, the bounce's predicted
//!   percentage, how sure the call is ([`confidence`]). A seam with no
//!   measured landing is capped ([`SCAN_ONLY_CAP`]).
//! - **Visibility** ([`strength`]): primed only.

use hud_kit_core::huds::superbounce_guide::{self as marks, Call, Layout};
use superbounce::{Kind, Spot, Vec3};

use crate::superbounce_assist::{drift_unit, stand_near};
use crate::superbounce_flight as flight;

/// The flight model's gravity (units/s²) and tick
/// (`superbounce_flight`): the fall speed gains `GRAVITY × TICK` a tick,
/// and the body moves the new speed times `TICK`.
pub const GRAVITY: f32 = flight::GRAVITY;
pub const TICK: f32 = flight::TICK;
/// On a flat floor the body's origin stops this far over the floor's
/// height at the touchdown (`sim_bench::drops::FALL_CLEARANCE`).
pub const CLEARANCE: f32 = sim_bench::drops::FALL_CLEARANCE;
/// A standing jump's launch (units/s up), as the bots' routes have it.
pub const JUMP_VZ: f32 = bots::bounce_route::JUMP_VZ;
/// The longest flight flown (`superbounce_flight::MAX_TICKS`).
pub const MAX_TICKS: u32 = flight::MAX_TICKS;
/// A last free row this far under the surface at its own place is not a
/// landing: the body never got over the seam's floor.
pub const UNDER: f32 = flight::UNDER;
/// How far a biped's bounding-sphere centre (object `+0x30`) sits over
/// its origin (`+0x64`) when it is not read: about 0.4 units, moving with
/// the pose and the facing. The takeoff solver ([`place`] from the mode)
/// flies with it, so a fixed takeoff does not move with the pose.
pub const CENTRE_LIFT: f32 = 0.4;

/// Not primed, the nearest target within this of the player is the seam
/// [`target`] names.
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
/// docs/game/notes/2026-11-21-bot-bounce-sim.md). A jump's takeoff line
/// is where to press, the ground of this many ticks short of where the
/// body leaves the floor.
pub const PRESS_LEAD: u32 = bots::bounce_route::INPUT_LAG;
/// `Jump now` shows from this many ticks before the body reaches the
/// line's band, and through the band: the tolerance for the eye and the
/// thumb, which act a frame or more after the cue shows.
pub const NOW_EARLY: u32 = 1;
/// Under this ground speed (units/s) toward the line the body is not
/// running at it, and the cue is `Walk to line`.
pub const MOVING: f32 = 0.05;
/// The run a takeoff is solved for when no run measured its own: a
/// walking approach (`superbounce_assist::ARRIVE_SPEED`).
pub const WALK_UP: f32 = crate::superbounce_assist::ARRIVE_SPEED;
/// Takeoffs are tried this far apart along the floor.
pub const TAKEOFF_STEP: f32 = 0.005;
/// A stretch of the floor this long is passed over when the landings
/// from both its ends are clear of the landing window on the same side
/// ([`TAKEOFF_CLEAR`]): only near the window is every step tried.
pub const TAKEOFF_COARSE: f32 = 0.04;
/// A landing this far outside the landing window, or outside the catch,
/// is clear of it.
pub const TAKEOFF_CLEAR: f32 = 0.12;
/// A jump's takeoff is looked for up to this far back from the lip, on
/// the lip's floor.
pub const JUMP_REACH: f32 = 4.0;
/// A floor within this of the lip's height is the lip's own floor.
pub const SAME_FLOOR: f32 = 0.1;
/// The lip's floor is followed back from the lip in steps this long.
pub const FLOOR_STEP: f32 = 0.05;
/// The lip is an edge, one a walk-off leaves from, when the floor this
/// far past it along the heading is not the lip's.
pub const EDGE_PROBE: f32 = 0.1;
/// A walk-off's line: half its band along the run.
pub const DROP_HALF: f32 = 0.05;
/// Feet more than this under the takeoff's floor climb to it (`Climb
/// up`); more than this over it, they walk to it (`Walk to line`).
pub const LEVEL: f32 = 0.3;
/// The count runs while the body is within this of the line's centre
/// across the run ...
pub const LINE_ACROSS: f32 = 0.5;
/// ... and runs at most 45 degrees off the line's heading.
pub const HEADING_COS: f32 = std::f32::consts::FRAC_1_SQRT_2;
/// The cue holds a count's step up, `Jump now` against a count of 0.1,
/// and a count against no line, for at most this many ticks ([`settle`]).
pub const CUE_GRACE: u8 = 3;
/// The most the count says, in tenths of a second: `Jump in 9.9 s`.
pub const CUE_MAX: u8 = 99;
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

    /// The height the body's bounding-sphere centre is stopped at over
    /// `xy`, for a body whose centre is `lift` over its origin. Within the
    /// catch of the edge (`superbounce::catch_miss`) it is the measured
    /// rule: [`CLEARANCE`] over the edge's height, or where the far face
    /// stops it first, [`DEPTH_CUT`] less [`DEPTH_BAND`] over the edge and
    /// [`SEAM_SLOPE`] higher for each unit past it. Elsewhere it is the
    /// floor in the centre's frame: the origin's [`CLEARANCE`] over the
    /// edge's height, plus `lift`.
    pub fn contact(&self, xy: [f32; 2], lift: f32) -> f32 {
        let q = self.edge_point(xy);
        if superbounce::catch_miss(self.edge, xy) > 0.0 {
            return q[2] + CLEARANCE + lift;
        }
        if !self.drifts() {
            return q[2] + CLEARANCE;
        }
        let past = (xy[0] - q[0]) * self.drift[0] + (xy[1] - q[1]) * self.drift[1];
        q[2] + CLEARANCE.max(DEPTH_CUT - DEPTH_BAND + SEAM_SLOPE * past)
    }
}

/// Where a free flight touches down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Impact {
    /// The last free row: the body's origin on the tick before the
    /// contact tick.
    pub row: Vec3,
    /// Game ticks from now to that row.
    pub ticks: u32,
    /// The fall speed on that row, units/s down: the touchdown speed.
    pub speed: f32,
    /// The body's bounding-sphere centre over its origin, the frame the
    /// depth rule is in ([`Impact::centre`]).
    pub lift: f32,
}

impl Impact {
    /// The last free row's bounding-sphere centre: the row `lift` up.
    pub fn centre(&self) -> Vec3 {
        [self.row[0], self.row[1], self.row[2] + self.lift]
    }
}

/// The touchdown of a body whose origin is at `position`, moving
/// `velocity` with `stick` (world axes) held, onto the surface `contact`
/// (the height its centre is stopped at over a point, [`Seam::contact`]),
/// its centre `lift` over its origin. The origin is flown on the shared
/// flight model (`superbounce_flight::fly`) onto the surface less `lift`;
/// the last row whose next would be at or under it is the [`Impact`].
/// `None` when the flight does not come down onto the surface within
/// [`MAX_TICKS`], or only reaches it from under ([`UNDER`]).
pub fn impact(
    position: Vec3,
    velocity: Vec3,
    stick: [f32; 2],
    lift: f32,
    contact: impl Fn([f32; 2]) -> f32,
) -> Option<Impact> {
    if !lift.is_finite() {
        return None;
    }
    flight::hold(position, velocity, stick, |xy| contact(xy) - lift).map(|l| Impact {
        row: l.row,
        ticks: l.ticks,
        speed: l.speed,
        lift,
    })
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

/// The depth rule: for a landing on `seam` whose last free row's
/// bounding-sphere centre is `impact.centre()` (the frame the rule was
/// measured in), how far under the cut the row ends, its height over the
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
    let [past, over] = seam.frame(impact.centre());
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
/// - On the ground (the landing is a jump's from the takeoff's line):
///   [`Call::LinedUp`].
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

/// The jump cue on the box before the jump: what to do to reach the
/// takeoff and leave it (docs/game/superbounce.md, "The jump cue"). Its
/// words are [`Cue::text`]; the HUD kit's box names the same five
/// (`hud_kit_core::huds::superbounce_guide`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cue {
    /// The takeoff is on a floor over the player's: `Climb up`.
    Climb,
    /// On the takeoff's floor, not running at its line, or over it:
    /// `Walk to line`.
    WalkToLine,
    /// Running at a jump's line: this many tenths of a second to `Jump
    /// now`, `Jump in 0.7 s`.
    JumpIn(u8),
    /// On a jump's line: press jump.
    JumpNow,
    /// The takeoff is a walk-off, and the player is on its floor: walk
    /// off the lip.
    WalkOff,
}

/// The cues' words.
pub const CLIMB_UP: &str = "Climb up";
pub const WALK_TO_LINE: &str = "Walk to line";
pub const JUMP_IN: &str = "Jump in";
pub const JUMP_NOW: &str = "Jump now";
pub const WALK_OFF: &str = "Walk off";

impl Cue {
    /// The cue's label: `Climb up`, `Walk to line`, `Jump in`, `Jump
    /// now`, `Walk off`.
    pub fn label(self) -> &'static str {
        match self {
            Cue::Climb => CLIMB_UP,
            Cue::WalkToLine => WALK_TO_LINE,
            Cue::JumpIn(_) => JUMP_IN,
            Cue::JumpNow => JUMP_NOW,
            Cue::WalkOff => WALK_OFF,
        }
    }

    /// The box's words: the label, and a count's seconds, `Jump in 0.7 s`.
    pub fn text(self) -> String {
        match self {
            Cue::JumpIn(t) => {
                let t = t.min(CUE_MAX);
                format!("{JUMP_IN} {}.{} s", t / 10, t % 10)
            }
            c => c.label().to_owned(),
        }
    }

    /// A count or `Jump now`: the player is running at a jump's line, and
    /// the pipper shows where that jump comes down.
    pub fn counting(self) -> bool {
        matches!(self, Cue::JumpIn(_) | Cue::JumpNow)
    }
}

/// The count's tenths of a second for `ticks` game ticks (30 a second):
/// rounded up, so the last count before `Jump now` is `0.1`, and kept to
/// [`CUE_MAX`].
pub fn tenths(ticks: u32) -> u8 {
    ticks.div_ceil(3).min(u32::from(CUE_MAX)) as u8
}

/// The cue as the HUD kit's box takes it: one for one where the kit has
/// the cue, none where it does not yet (`Climb up`, `Walk off`, which the
/// world's takeoff bar says instead, `superbounce_world`).
pub fn kit_cue(c: Cue) -> Option<marks::Cue> {
    match c {
        Cue::WalkToLine => Some(marks::Cue::Walk),
        Cue::JumpIn(t) => Some(marks::Cue::In(t)),
        Cue::JumpNow => Some(marks::Cue::Now),
        Cue::Climb | Cue::WalkOff => None,
    }
}

/// How the body leaves the takeoff.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Leave {
    /// A jump pressed on the line.
    Jump,
    /// A walk off the lip.
    Drop,
}

/// The takeoff bar's labels.
pub const JUMP_FROM: &str = "Jump from here";
pub const DROP_FROM: &str = "Drop from here";

impl Leave {
    /// The takeoff bar's label: `Jump from here`, `Drop from here`.
    pub fn label(self) -> &'static str {
        match self {
            Leave::Jump => JUMP_FROM,
            Leave::Drop => DROP_FROM,
        }
    }
}

/// Where a takeoff is looked for: the lip of the floor the player leaves
/// for the seam. A ghost or verified run's own (`superbounce_ghost::lip`),
/// else the map's search (`superbounce_world::lip`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lip {
    /// On the floor, at its edge toward the seam (a run's: where it left
    /// the floor).
    pub at: Vec3,
    /// The way off it toward the seam, a flat unit.
    pub heading: [f32; 2],
    /// The ground speed a run left it at, when a run measured it.
    pub speed: Option<f32>,
    /// How a run left it; `None` from the search.
    pub run: Option<Leave>,
}

/// The takeoff: a fixed place on a real floor to leave for the seam
/// ([`place`]), and how.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Takeoff {
    /// The line's centre, on the floor: a jump's is the middle of the
    /// band of places to press jump on, a walk-off's the lip.
    pub at: Vec3,
    /// The way to run over it, a flat unit: the line lies across it.
    pub heading: [f32; 2],
    /// Half the band's length along the run.
    pub half: f32,
    /// The ground speed the takeoff is solved for (units/s).
    pub speed: f32,
    pub leave: Leave,
    /// A flight from it lands in the seam's catch at its need. False for
    /// a lip no modelled flight from bounces: still where to go.
    pub checked: bool,
}

impl Takeoff {
    /// How far ahead along the heading the line's centre is from
    /// `position`, and how far off it across the run.
    pub fn ahead(&self, position: Vec3) -> [f32; 2] {
        let rel = [self.at[0] - position[0], self.at[1] - position[1]];
        let h = self.heading;
        [
            rel[0] * h[0] + rel[1] * h[1],
            (rel[0] * h[1] - rel[1] * h[0]).abs(),
        ]
    }

    /// Ticks until `Jump now` for a body at `position` running
    /// `velocity` at a jump's line: the ticks of the run to the band's
    /// near end, rounded up, less [`NOW_EARLY`]; 0 within that of the
    /// band and inside it. `None` for a walk-off, and when the body is
    /// not running at the line: slower than [`MOVING`] along its
    /// heading, more than 45 degrees off it, wider than [`LINE_ACROSS`],
    /// or past the band.
    pub fn jump_in(&self, position: Vec3, velocity: Vec3) -> Option<u32> {
        if self.leave != Leave::Jump {
            return None;
        }
        let [ahead, across] = self.ahead(position);
        let along = velocity[0] * self.heading[0] + velocity[1] * self.heading[1];
        let speed = velocity[0].hypot(velocity[1]);
        let running = along >= MOVING && along >= speed * HEADING_COS;
        if !(running && across <= LINE_ACROSS && ahead + self.half >= 0.0) {
            return None;
        }
        let ticks = ((ahead - self.half) / (along * TICK)).ceil().max(0.0);
        ticks
            .is_finite()
            .then(|| (ticks as u32).saturating_sub(NOW_EARLY))
    }

    /// The cue for a body at `position` moving `velocity`, before
    /// [`settle`]:
    ///
    /// - feet more than [`LEVEL`] under the takeoff's floor: `Climb up`;
    /// - more than that over it: `Walk to line`;
    /// - on it, a walk-off: `Walk off`;
    /// - on it, a jump: the count to the line ([`Takeoff::jump_in`]),
    ///   `Jump now` at 0, `Walk to line` when not running at it.
    pub fn cue(&self, position: Vec3, velocity: Vec3) -> Cue {
        let feet = position[2] - CLEARANCE;
        if feet < self.at[2] - LEVEL {
            return Cue::Climb;
        }
        if feet > self.at[2] + LEVEL {
            return Cue::WalkToLine;
        }
        if self.leave == Leave::Drop {
            return Cue::WalkOff;
        }
        match self.jump_in(position, velocity) {
            None => Cue::WalkToLine,
            Some(0) => Cue::JumpNow,
            Some(t) => Cue::JumpIn(tenths(t)),
        }
    }

    /// Where a body at `position` moving `velocity` leaves the floor if it
    /// presses jump on the line's centre: the run carried to the centre
    /// and [`PRESS_LEAD`] ticks on, at the body's height. `None` when it
    /// is not running at the line ([`Takeoff::jump_in`]).
    pub fn leaves_at(&self, position: Vec3, velocity: Vec3) -> Option<Vec3> {
        self.jump_in(position, velocity)?;
        let [ahead, _] = self.ahead(position);
        let along = velocity[0] * self.heading[0] + velocity[1] * self.heading[1];
        let t = ahead.max(0.0) / along + PRESS_LEAD as f32 * TICK;
        Some([
            position[0] + velocity[0] * t,
            position[1] + velocity[1] * t,
            position[2],
        ])
    }
}

/// How a landing stands against `seam`'s landing window, for the coarse
/// pass of [`band_by`]: `None` near the window and in or near the catch,
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

/// The flight of a takeoff with no real stick: a body leaving at
/// `velocity` holds the stick that keeps it going
/// (`superbounce_flight::keep`), so a run faster than the air stick's top
/// slows to it.
fn solve(position: Vec3, velocity: Vec3, seam: &Seam, lift: f32) -> Option<Impact> {
    let stick = flight::keep([velocity[0], velocity[1]]);
    impact(position, velocity, stick, lift, |xy| seam.contact(xy, lift))
}

/// The first band of takeoffs along the line from `origin` the unit
/// `dir`, at distances `range`, whose flight at `velocity` onto `seam`
/// ([`solve`], the centre `lift` over the origin) is `good`: as distances
/// along the line, each end widened by half a step ([`TAKEOFF_STEP`]).
/// Every takeoff keeps `origin`'s height. `coarse` passes over stretches
/// clear of the landing window ([`clear`]); for `good` landings inside
/// the window the band is the same without it, which a test checks.
#[allow(clippy::too_many_arguments)]
fn band_by(
    seam: &Seam,
    origin: Vec3,
    dir: [f32; 2],
    velocity: Vec3,
    lift: f32,
    range: [f32; 2],
    coarse: bool,
    good: &dyn Fn(&Impact) -> bool,
) -> Option<[f32; 2]> {
    let at = |d: f32| [origin[0] + dir[0] * d, origin[1] + dir[1] * d, origin[2]];
    let land = |d: f32| solve(at(d), velocity, seam, lift);
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
        if land(d).is_some_and(|l| good(&l)) {
            start = start.or(Some(d));
            last = d;
        } else if let Some(s) = start {
            return Some([s - TAKEOFF_STEP / 2.0, last + TAKEOFF_STEP / 2.0]);
        }
        i += 1;
    }
    start.map(|s| [s - TAKEOFF_STEP / 2.0, last + TAKEOFF_STEP / 2.0])
}

/// A landing that bounces, as [`call`] says in a primed fall.
fn bounces(seam: &Seam, i: &Impact) -> bool {
    call(seam, Some(i), true, true) == Call::WillBounce
}

/// A landing in the seam's catch at its `need` (units/s down).
fn lands(seam: &Seam, need: f32, i: &Impact) -> bool {
    caught(seam, i) && i.speed >= need
}

/// The takeoff from `lip` for `seam`, whose landing needs `need` units/s
/// down, for a body whose centre is `lift` over its origin (the mode
/// passes [`CENTRE_LIFT`]), with `floor(xy, top)` the height of the
/// highest floor under `xy` at or below `top`
/// (`superbounce_capsule::Room::floor_at`) (docs/game/superbounce.md,
/// "The takeoff").
///
/// The lip is put on the real floor under it; `None` when there is none
/// within [`SAME_FLOOR`]. Two flights are flown from it ([`solve`]: the
/// shared flight model, holding the stick that keeps the run going), both
/// leaving at the run's speed (the lip's own, else [`WALK_UP`]) along its
/// heading:
///
/// - **a walk-off** from the lip itself, when it is an edge (the floor
///   [`EDGE_PROBE`] past it is not its own);
/// - **a jump** from each place back along the lip's floor, up to
///   [`JUMP_REACH`]: the first band of them, from the lip back, whose
///   landing is good. The line is [`PRESS_LEAD`] ticks of the run short
///   of the band: where to press.
///
/// The one kept is the first of: a walk-off that bounces (as a primed
/// fall's [`call`] says), a jump band that bounces, a walk-off that lands
/// in the catch at the need, a jump band that does. With none, the lip
/// itself, as the run left it (else a walk-off from an edge, a jump from
/// anywhere else), unchecked.
pub fn place(
    seam: &Seam,
    need: f32,
    lip: &Lip,
    lift: f32,
    floor: impl Fn([f32; 2], f32) -> Option<f32>,
) -> Option<Takeoff> {
    let h = lip.heading;
    let hl = h[0].hypot(h[1]);
    if !(lip.at.iter().all(|v| v.is_finite()) && hl > 1e-6 && hl.is_finite()) {
        return None;
    }
    let h = [h[0] / hl, h[1] / hl];
    let z = floor([lip.at[0], lip.at[1]], lip.at[2] + SAME_FLOOR)
        .filter(|z| (z - lip.at[2]).abs() <= SAME_FLOOR)?;
    let speed = lip
        .speed
        .filter(|s| s.is_finite() && *s >= MOVING)
        .unwrap_or(WALK_UP);
    let back = |d: f32| [lip.at[0] - h[0] * d, lip.at[1] - h[1] * d];
    let own = |xy: [f32; 2]| floor(xy, z + SAME_FLOOR).is_some_and(|f| (f - z).abs() <= SAME_FLOOR);
    let edge = !own(back(-EDGE_PROBE));
    let line = |d: f32, half: f32, leave: Leave, checked: bool| {
        let [x, y] = back(d);
        Takeoff {
            at: [x, y, z],
            heading: h,
            half,
            speed,
            leave,
            checked,
        }
    };
    let body = [lip.at[0], lip.at[1], z + CLEARANCE];
    // The walk-off: off the lip at the run's speed, no jump.
    let drop = edge
        .then(|| solve(body, [h[0] * speed, h[1] * speed, 0.0], seam, lift))
        .flatten();
    let dropped = line(0.0, DROP_HALF, Leave::Drop, true);
    // The jump: from each place back along the lip's floor.
    let steps = (JUMP_REACH / FLOOR_STEP).round() as u32;
    let reach = (1..=steps)
        .map(|k| k as f32 * FLOOR_STEP)
        .take_while(|d| own(back(*d)))
        .last()
        .unwrap_or(0.0);
    let up = [h[0] * speed, h[1] * speed, JUMP_VZ];
    let lead = speed * PRESS_LEAD as f32 * TICK;
    let jumped =
        |[lo, hi]: [f32; 2]| line((lo + hi) / 2.0 + lead, (hi - lo) / 2.0, Leave::Jump, true);
    let band = |coarse: bool, good: &dyn Fn(&Impact) -> bool| {
        band_by(seam, body, [-h[0], -h[1]], up, lift, [0.0, reach], coarse, good)
    };
    if drop.is_some_and(|i| bounces(seam, &i)) {
        return Some(dropped);
    }
    if let Some(b) = band(true, &|i: &Impact| bounces(seam, i)) {
        return Some(jumped(b));
    }
    if drop.is_some_and(|i| lands(seam, need, &i)) {
        return Some(dropped);
    }
    if let Some(b) = band(false, &|i: &Impact| lands(seam, need, i)) {
        return Some(jumped(b));
    }
    let leave = lip
        .run
        .unwrap_or(if edge { Leave::Drop } else { Leave::Jump });
    Some(line(0.0, DROP_HALF, leave, false))
}

/// The cue shown this tick, and how many ticks it has been held, from
/// the last tick's shown cue `prev` (held `held` ticks) and this tick's
/// own `raw`. For [`CUE_GRACE`] ticks at most it holds:
///
/// - a count against a count a tenth higher (a step back up is the
///   count's own jitter);
/// - `Jump now` against a count of 0.1;
/// - a count against no line, or `Walk to line`.
///
/// Anything else shows at once: a count down, a new line, `Walk to
/// line`, `Climb up`, `Walk off`, and the end of `Jump now`, which never
/// outlasts its band.
pub fn settle(prev: Option<Cue>, held: u8, raw: Option<Cue>) -> (Option<Cue>, u8) {
    let hold = match (prev, raw) {
        (Some(Cue::JumpIn(p)), Some(Cue::JumpIn(r))) => r == p.saturating_add(1),
        (Some(Cue::JumpNow), Some(Cue::JumpIn(1))) => true,
        (Some(Cue::JumpIn(_)), None | Some(Cue::WalkToLine)) => true,
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
/// [`Takeoff::cue`]), and where nothing bounces or nothing is measured: not
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

/// How strongly the guide draws: 1 while primed, else 0. The guide is
/// for a primed player alone: not primed, there is nothing to land yet
/// (docs/game/superbounce.md, "The approach guide").
pub fn strength(primed: bool) -> f32 {
    if primed { 1.0 } else { 0.0 }
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
    spots
        .into_iter()
        .find(|s| s.kind == Kind::Bounce && (0..3).all(|k| (s.stand[k] - at[k]).abs() <= SAME))
}

/// Two landing points this close on each axis are one seam's.
pub const SAME: f32 = 1e-3;

/// The player, for the guide.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Body {
    /// The origin (object `+0x64`), the frame every spot's heights are in.
    pub position: Vec3,
    pub velocity: Vec3,
    pub airborne: bool,
    pub primed: bool,
    /// The bounding-sphere centre over the origin (units): the frame the
    /// depth rule is in ([`CENTRE_LIFT`] when it is not read).
    pub lift: f32,
    /// The stick held (world axes, `superbounce_flight::held`); the
    /// keeping stick (`superbounce_flight::keep`) when it is not read.
    pub stick: [f32; 2],
}

/// The guide on one seam, as of a tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Guide {
    pub seam: Seam,
    /// The touchdown: of the fall in the air; on the ground, of the jump
    /// pressed on the takeoff's line while the cue counts to it
    /// ([`Cue::counting`]). `None` when the path does not come down by
    /// the seam ([`PIPPER_RANGE`]), and on the ground otherwise.
    pub impact: Option<Impact>,
    /// The impact is a jump's from the ground, not a flight under way.
    pub jump: bool,
    pub call: Call,
    /// The landing's depth, on a seam that has one. On the ground it is
    /// the jump's from the line, an estimate.
    pub depth: Option<Depth>,
    /// The body's speed along the seam's drift (units/s); `None` for a
    /// seam with none.
    pub across: Option<f32>,
    /// The bounce's predicted chance in a fall ([`chance`]).
    pub chance: Option<Chance>,
    /// On the ground, the takeoff ([`place`]), when the mode found one
    /// for this seam.
    pub takeoff: Option<Takeoff>,
    /// Ticks to `Jump now` ([`Takeoff::jump_in`]), while the cue counts.
    pub jump_in: Option<u32>,
    /// The jump cue on the box: the takeoff's, settled against the last
    /// tick's ([`Guide::settled`]).
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
        self.impact
            .map(|i| [i.row[0], i.row[1], self.seam.point[2]])
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
        let (cue, held) = settle(
            prev.and_then(|p| p.cue),
            prev.map_or(0, |p| p.held),
            self.cue,
        );
        self.cue = cue;
        self.held = held;
        self
    }

    /// What the marks show. The takeoff is the world's bar
    /// (`superbounce_world`), not a guide mark: one drawer for one place.
    pub fn view(&self) -> marks::Guide {
        marks::Guide {
            call: self.call,
            pipper: self.impact.is_some(),
            ticks: self.impact.is_some() && !self.caught(),
            drift: self.across.map(drift_lit),
            chance: self.chance.map(|c| marks::percent(c.value)),
            cue: self.cue.and_then(kit_cue),
            takeoff: false,
        }
    }

    /// Where the marks lie.
    pub fn layout(&self) -> Layout {
        let [a, b] = self.seam.edge;
        let half = (b[0] - a[0]).hypot(b[1] - a[1]) / 2.0;
        Layout {
            // The box is the catch, so it lies on the collision edge
            // itself, at the point of it beside the landing point: the
            // edge runs through its middle.
            seam: self
                .seam
                .edge_point([self.seam.point[0], self.seam.point[1]]),
            along: self.seam.along(),
            // A busy point's edge has no length: the full box.
            half_along: if half > 1e-4 {
                half.min(marks::BOX_ALONG)
            } else {
                marks::BOX_ALONG
            },
            drift: self.seam.drift,
            impact: self.pipper(),
            takeoff: None,
            strength: self.strength,
        }
    }
}

/// [`guide_from`] with no takeoff: no jump cue on the ground.
pub fn guide(body: &Body, spot: &Spot) -> Option<Guide> {
    guide_from(body, spot, None)
}

/// The guide for `body` on the bounce spot `spot`, with `takeoff` the
/// takeoff the mode placed for it ([`place`]), or `None` when it is not
/// shown: not primed ([`strength`]), or `spot` is not a bounce spot. Its
/// cue is this tick's own; the mode settles it against the last tick's
/// ([`Guide::settled`]).
///
/// In the air the impact is the fall's. On the ground it is the jump
/// pressed on the takeoff's line, and only while the cue counts to it
/// ([`Cue::counting`]): the floor under the body is not the seam's, so a
/// jump from anywhere else says nothing. Either is flown with the
/// player's stick as it is held now.
pub fn guide_from(body: &Body, spot: &Spot, takeoff: Option<Takeoff>) -> Option<Guide> {
    if spot.kind != Kind::Bounce || !body.primed {
        return None;
    }
    let at = [body.position[0], body.position[1]];
    let takeoff = takeoff.filter(|_| !body.airborne);
    let cue = takeoff.map(|t| t.cue(body.position, body.velocity));
    let jump_in = takeoff
        .filter(|_| cue.is_some_and(Cue::counting))
        .and_then(|t| t.jump_in(body.position, body.velocity));
    let from = if body.airborne {
        Some((body.position, body.velocity))
    } else {
        takeoff
            .filter(|_| jump_in.is_some())
            .and_then(|t| t.leaves_at(body.position, body.velocity))
            .map(|p| (p, jump(body.velocity)))
    };
    // The surface is the edge's and the drift's, whatever the landing
    // point; the point then slides along the strip to the landing.
    let surface = Seam::of(spot, at);
    let lift = body.lift;
    let flight =
        from.and_then(|(p, v)| impact(p, v, body.stick, lift, |xy| surface.contact(xy, lift)));
    let seam = Seam::of(spot, flight.map_or(at, |i| [i.row[0], i.row[1]]));
    let impact = flight
        .filter(|i| (i.row[0] - seam.point[0]).hypot(i.row[1] - seam.point[1]) <= PIPPER_RANGE);
    Some(Guide {
        seam,
        impact,
        chance: chance(&seam, body, impact.as_ref()),
        takeoff,
        jump_in,
        cue,
        held: 0,
        jump: !body.airborne,
        call: call(&seam, impact.as_ref(), body.airborne, body.primed),
        depth: impact.and_then(|i| depth(&seam, &i)),
        across: seam
            .drifts()
            .then(|| body.velocity[0] * seam.drift[0] + body.velocity[1] * seam.drift[1]),
        strength: strength(body.primed),
    })
}

#[cfg(test)]
#[path = "superbounce_guide_tests.rs"]
mod tests;
