//! The bounce view (docs/game/superbounce.md, "The bounce view"): the
//! superbounce spots across the level on the flashlight, and on the
//! motion tracker.
//!
//! - **Tap the flashlight:** a scan pulse. A ring leaves the player at
//!   [`PULSE_SPEED`]; each spot lights as it passes, settles, and is gone
//!   [`PULSE_LIT`] + [`PULSE_FADE`] seconds later ([`pulse_strength`]). The
//!   tracker shows the ring.
//! - **Hold it:** every spot on the level gets a small mark with its
//!   distance, dashed and dimmer behind a wall, a chevron for a floor above
//!   or below, a dashed line from each crouch spot to its landing; and the
//!   tracker grows into a map of the floors near the player ([`GROW`] times
//!   its size, [`MAP_RANGE`] across), turned so ahead is up.
//! - **Always:** the next spot is a dot on the tracker with a ring round
//!   it, and the one after it a smaller dot; out of range they sit on its
//!   rim ([`tracker_blips`]).
//!
//! **The button** is the game's flashlight *action*, the unit control
//! flag Halo 2 tests at `0x000c5115` ([`UNIT_CONTROL_FLAGS`],
//! [`CONTROL_FLASHLIGHT`]; the converted Halo 1 scope reads the same bit,
//! `crates/extensions/h1-runtime/src/h1_scope_zoom.rs`), so a layout that
//! moves the flashlight (Bumper Jumper, the keyboard) moves the view with
//! it. The press is read, never taken away: on a converted Halo 1 map the
//! light comes on with the view.
//!
//! **Any frame rate.** [`Ticker::step`] runs once a 30 Hz tick from the
//! mode's tick ([`on_tick`]): the button, the line-of-sight tests (a few
//! spots a tick, [`superbounce::view::SightCache`]), the blips at the
//! tick's yaw (so they turn with the game's own blips), the map. It
//! publishes a [`Frame`] with the instants its animations started. What is
//! drawn ([`View::world`], [`View::hud`]) is a pure function of that frame
//! and the `now` the window gives each present: the pulse, the fades and
//! the tracker's growth run on the wall clock, and the same `now` draws the
//! same picture however many presents there are.
//!
//! **The tracker's place** is the retail one, `rects::TRACKER` of the HUD
//! kit (V, docs/game/hud-kit.md, "Anchors, the safe area and scaling"),
//! from the HUD frame's bottom-left corner, moved as the host moves the
//! HUD's bottom-left group ([`crate::render_aspect::hud_bottom_left_fit`]).
//! The marks over it are host marks ([`gfx::world_lines::HudMark`]), not
//! kit elements: the kit has no widget for a polygon.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use guest_layout::GuestMemory;
use superbounce::view::{self as geo, Geometry, Level, LineOfSight, SightCache};
use superbounce::{Kind, Spot, Status, Vec3};

use crate::superbounce_mode::METRES_PER_UNIT;

// ---- the button ----

/// The unit's control flags, from its body: the flags the control record
/// leaves on the unit (`0x000c5115` and `0x000c52be` test the flashlight
/// bit).
pub const UNIT_CONTROL_FLAGS: u32 = 0x148;
/// The flashlight action's bit in [`UNIT_CONTROL_FLAGS`] (the control
/// record's `0x02100004` while the flashlight's button is down,
/// docs/game/bots.md, the pad table).
pub const CONTROL_FLASHLIGHT: u32 = 0x4;
/// A press let go within this many ticks is a tap (a pulse); held this
/// long it is a hold (the marks and the map) until let go. 8 ticks is
/// 0.27 s.
pub const TAP_TICKS: u32 = 8;

/// What a tick's press did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    Tap,
    HoldStart,
    HoldEnd,
}

/// The flashlight button's ticks down.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Button {
    down: u32,
}

impl Button {
    /// The button up.
    pub const UP: Button = Button { down: 0 };

    /// One tick: `down` is the flashlight action this tick.
    pub fn step(&mut self, down: bool) -> Option<Press> {
        if down {
            self.down = self.down.saturating_add(1);
            (self.down == TAP_TICKS).then_some(Press::HoldStart)
        } else {
            match std::mem::take(&mut self.down) {
                0 => None,
                n if n < TAP_TICKS => Some(Press::Tap),
                _ => Some(Press::HoldEnd),
            }
        }
    }

    /// Held long enough to be a hold.
    pub fn holding(&self) -> bool {
        self.down >= TAP_TICKS
    }
}

// ---- the pulse ----

/// World units a second the pulse's ring travels: about a second across
/// a map the size of Zanzibar.
pub const PULSE_SPEED: f32 = 40.0;
/// Seconds a spot takes to settle from full strength once the ring has
/// passed it.
pub const PULSE_FLASH: f32 = 0.3;
/// A settled spot's strength.
pub const PULSE_SETTLED: f32 = 0.75;
/// Seconds from the ring passing until the spot starts to fade, and the
/// fade's length: gone 4 s after it lit.
pub const PULSE_LIT: f32 = 3.0;
pub const PULSE_FADE: f32 = 1.0;

/// How far the pulse's ring is from the player `age` seconds after the
/// tap, world units.
pub fn pulse_front(age: f32) -> f32 {
    age.max(0.0) * PULSE_SPEED
}

/// A spot `distance` world units away, `age` seconds after the tap: 0
/// until the ring reaches it, 1 then, settling to [`PULSE_SETTLED`] over
/// [`PULSE_FLASH`], held to [`PULSE_LIT`], gone after [`PULSE_FADE`] more.
pub fn pulse_strength(age: f32, distance: f32) -> f32 {
    let since = age - distance.max(0.0) / PULSE_SPEED;
    if !(since >= 0.0) {
        0.0
    } else if since < PULSE_FLASH {
        1.0 + (PULSE_SETTLED - 1.0) * since / PULSE_FLASH
    } else if since < PULSE_LIT {
        PULSE_SETTLED
    } else if since < PULSE_LIT + PULSE_FADE {
        PULSE_SETTLED * (1.0 - (since - PULSE_LIT) / PULSE_FADE)
    } else {
        0.0
    }
}

/// Whether a pulse `age` seconds old still shows: the furthest spot (or
/// the tracker's edge, `reach`, if further) has not faded yet.
pub fn pulse_alive(age: f32, reach: f32) -> bool {
    age >= 0.0 && age < reach.max(0.0) / PULSE_SPEED + PULSE_LIT + PULSE_FADE
}

// ---- the tracker's growth ----

/// Seconds the tracker takes to grow into the map, or shrink back.
pub const GROW_SECONDS: f32 = 0.2;
/// The map is this many times the tracker's size.
pub const GROW: f32 = 2.0;

/// The tracker's growth, 0 (the tracker) to 1 (the map), eased from
/// `from` toward `to` from the instant `at`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grow {
    pub from: f32,
    pub to: f32,
    pub at: Instant,
}

fn seconds(now: Instant, since: Instant) -> f32 {
    now.saturating_duration_since(since).as_secs_f32()
}

impl Grow {
    pub fn rest(at: Instant) -> Grow {
        Grow {
            from: 0.0,
            to: 0.0,
            at,
        }
    }

    /// The growth at `now`.
    pub fn value(&self, now: Instant) -> f32 {
        let p = (seconds(now, self.at) / GROW_SECONDS).clamp(0.0, 1.0);
        let eased = p * p * (3.0 - 2.0 * p);
        self.from + (self.to - self.from) * eased
    }

    /// Heading for `to` from `now`, from wherever it is then.
    pub fn toward(&self, to: f32, now: Instant) -> Grow {
        Grow {
            from: self.value(now),
            to,
            at: now,
        }
    }
}

// ---- the motion tracker ----

/// The HUD frame (i16 top, left, bottom, right), docs/game/hud-kit.md,
/// "Anchors".
pub const HUD_FRAME: u32 = 0x004b_9dd8;
/// The fullscreen HUD frame's bottom-left corner (V).
pub const FRAME_BOTTOM_LEFT: [f32; 2] = [48.0, 444.0];
/// The tracker's centre from the frame's bottom-left corner, and its
/// radius, HUD pixels: `rects::TRACKER`, x 47..130, y 360..443 (V).
pub const TRACKER_CENTRE: [f32; 2] = [40.5, -42.5];
pub const TRACKER_RADIUS: f32 = 41.5;
/// Where a blip at the range's edge sits, and the rim's chevrons (S: the
/// game's blip disc is inside the tracker's frame by about this).
pub const BLIP_RADIUS: f32 = 35.0;
pub const RIM_RADIUS: f32 = 38.5;
/// `hudg +0x290`: the motion sensor's range, world units
/// (`crates/formats/tags/src/defs/hudg.txt`, as the bots' radar reads it).
pub const HUDG_RADAR_RANGE: u32 = 0x290;
/// The range when the `hudg` cannot be read: 25 m.
pub const TRACKER_RANGE_FALLBACK: f32 = 25.0 / METRES_PER_UNIT;
/// The map's range (its rim from the player): 50 m.
pub const MAP_RANGE: f32 = 50.0 / METRES_PER_UNIT;
/// The map's floors and spots are drawn inside this much of its radius.
pub const MAP_INNER: f32 = 0.94;
/// Corners of the circle the map is clipped to.
pub const MAP_SIDES: usize = 32;

/// The motion sensor as the game has it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sensor {
    /// The variant shows the tracker (multiplayer `MOTION_SENSOR`; always
    /// outside multiplayer).
    pub enabled: bool,
    /// World units.
    pub range: f32,
}

impl Default for Sensor {
    fn default() -> Self {
        Sensor {
            enabled: true,
            range: TRACKER_RANGE_FALLBACK,
        }
    }
}

/// `p` from a player at `at` looking along `yaw`: world units to the
/// right and ahead, across the ground.
pub fn place(at: Vec3, yaw: f32, p: Vec3) -> [f32; 2] {
    let (dx, dy) = (p[0] - at[0], p[1] - at[1]);
    let (s, c) = yaw.sin_cos();
    [dx * s - dy * c, dx * c + dy * s]
}

/// Which spots out of range get a rim chevron.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rim {
    /// The nearest few.
    Nearest(usize),
    /// Every proven one: the map.
    Proven,
}

/// One spot on the tracker.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Blip {
    /// Its index in [`Frame::spots`].
    pub spot: usize,
    /// Right and ahead over the range: inside the unit circle, or on it
    /// for a rim chevron.
    pub at: [f32; 2],
    pub kind: Kind,
    pub proven: bool,
    pub level: Level,
    /// Out of range: a chevron on the rim pointing at it.
    pub rim: bool,
    /// The navpoint is on it.
    pub nav: bool,
}

/// The spots as blips for a player at `at` looking along `yaw` (the
/// tick's), `range` world units to the edge: those in range, nearest
/// first, then the rim's.
pub fn blips(at: Vec3, yaw: f32, range: f32, spots: &[ViewSpot], rim: Rim) -> Vec<Blip> {
    if !(range > 0.0) || !yaw.is_finite() {
        return Vec::new();
    }
    let mut placed: Vec<(f32, Blip)> = spots
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let p = place(at, yaw, s.stand);
            let flat = p[0].hypot(p[1]);
            let rim = flat > range;
            let at = if rim {
                [p[0] / flat, p[1] / flat]
            } else {
                [p[0] / range, p[1] / range]
            };
            (
                flat,
                Blip {
                    spot: i,
                    at,
                    kind: s.kind,
                    proven: s.proven,
                    level: s.level,
                    rim,
                    nav: s.nav,
                },
            )
        })
        .collect();
    placed.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out: Vec<Blip> = placed
        .iter()
        .filter(|(_, b)| !b.rim)
        .map(|(_, b)| *b)
        .collect();
    let far = placed.iter().filter(|(_, b)| b.rim).map(|(_, b)| *b);
    match rim {
        Rim::Nearest(n) => out.extend(far.take(n)),
        Rim::Proven => out.extend(far.filter(|b| b.proven)),
    }
    out
}

/// How many spots the tracker marks: the next one and the one after.
pub const TRACKER_MARKS: usize = 2;

/// The tracker's blips for a player at `at` looking along `yaw` (the
/// tick's), `range` world units to the edge: the next spot (the
/// navpoint's, [`ViewSpot::nav`]), then the one after it, the landing a
/// next crouch spot pairs with. One out of range sits on the rim, toward
/// it. None without a next spot.
pub fn tracker_blips(at: Vec3, yaw: f32, range: f32, spots: &[ViewSpot]) -> Vec<Blip> {
    if range.is_nan() || range <= 0.0 || !yaw.is_finite() {
        return Vec::new();
    }
    let Some(next) = spots.iter().position(|s| s.nav) else {
        return Vec::new();
    };
    let after = spots[next].pair.filter(|&i| i != next && i < spots.len());
    [Some(next), after]
        .into_iter()
        .flatten()
        .take(TRACKER_MARKS)
        .map(|i| {
            let s = &spots[i];
            let p = place(at, yaw, s.stand);
            let flat = p[0].hypot(p[1]);
            let rim = flat > range;
            Blip {
                spot: i,
                at: if rim {
                    [p[0] / flat, p[1] / flat]
                } else {
                    [p[0] / range, p[1] / range]
                },
                kind: s.kind,
                proven: s.proven,
                level: s.level,
                rim,
                nav: i == next,
            }
        })
        .collect()
}

/// The map round the player, in the tracker's terms: right and ahead
/// over [`MAP_RANGE`], clipped to the unit circle.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapView {
    /// Each floor and its height over the player's feet, lowest first.
    pub floors: Vec<(Vec<[f32; 2]>, f32)>,
    pub edges: Vec<[[f32; 2]; 2]>,
    /// Crouch spot to landing.
    pub pairs: Vec<[[f32; 2]; 2]>,
    pub blips: Vec<Blip>,
}

/// [`MapView`] of `local` (from [`Geometry::local_map`] at `at`), turned
/// for `yaw`, with the spots' pairs and blips.
pub fn map_view(local: &geo::LocalMap, at: Vec3, yaw: f32, spots: &[ViewSpot]) -> MapView {
    let (s, c) = yaw.sin_cos();
    let turn = |[dx, dy]: [f32; 2]| [(dx * s - dy * c) / MAP_RANGE, (dx * c + dy * s) / MAP_RANGE];
    let clip = geo::circle(1.0, MAP_SIDES);
    let floors = local
        .floors
        .iter()
        .filter_map(|f| {
            let turned: Vec<[f32; 2]> = f.verts.iter().map(|&v| turn(v)).collect();
            let kept = geo::clip_convex(&turned, &clip);
            (!kept.is_empty()).then_some((kept, f.rise))
        })
        .collect();
    let edges = local
        .edges
        .iter()
        .filter_map(|&[a, b]| geo::clip_segment(turn(a), turn(b), 1.0))
        .collect();
    let rel = |p: Vec3| turn([p[0] - at[0], p[1] - at[1]]);
    let pairs = spots
        .iter()
        .filter_map(|sp| {
            let j = sp.pair?;
            geo::clip_segment(rel(sp.stand), rel(spots.get(j)?.stand), 1.0)
        })
        .collect();
    MapView {
        floors,
        edges,
        pairs,
        blips: blips(at, yaw, MAP_RANGE, spots, Rim::Proven),
    }
}

// ---- the tick ----

/// The eye over the feet, world units (a standing player's camera).
pub const EYE: f32 = 0.62;
/// The point of a spot a sight line goes to, over its stand point.
pub const SPOT_LIFT: f32 = 0.25;
/// Sight tests a tick while marks show, and while they don't.
pub const SIGHT_BUSY: usize = 6;
pub const SIGHT_IDLE: usize = 2;

/// One spot as the view draws it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewSpot {
    pub stand: Vec3,
    pub kind: Kind,
    pub proven: bool,
    pub level: Level,
    /// From the player's feet, world units.
    pub distance: f32,
    /// Behind a wall from the eye; `None` before it is tested.
    pub hidden: Option<bool>,
    /// The landing this crouch spot pairs with ([`geo::pairs`]).
    pub pair: Option<usize>,
    /// The navpoint is on it.
    pub nav: bool,
}

/// What a tick knows, for the view.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    /// The player's feet and the tick's yaw; `None` without a unit.
    pub at: Option<Vec3>,
    pub yaw: f32,
    pub spots: Vec<ViewSpot>,
    /// When the last tap's pulse left, while it shows.
    pub pulse: Option<Instant>,
    pub grow: Grow,
    /// A hold (the button, or `superbounce.view_test`).
    pub holding: bool,
    /// The flashlight action this tick.
    pub button: bool,
    pub test_hold: bool,
    pub sensor: Sensor,
    /// The tracker's blips ([`tracker_blips`]).
    pub blips: Vec<Blip>,
    /// While held or shrinking back.
    pub map: Option<MapView>,
    /// Spots with a sight answer.
    pub tested: usize,
}

/// The view's mode at an instant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    None,
    Pulse,
    Hold,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::None => "none",
            Mode::Pulse => "pulse",
            Mode::Hold => "hold",
        }
    }
}

/// A likely spot's strength; a proven one's is 1 (as the world marks').
pub const LIKELY: f32 = 0.5;
/// A spot behind a wall is drawn dashed at this strength.
pub const HIDDEN: f32 = 0.55;
/// Below this a mark is not drawn.
pub const MIN_ALPHA: f32 = 0.01;

impl Frame {
    pub fn empty(now: Instant) -> Frame {
        Frame {
            at: None,
            yaw: 0.0,
            spots: Vec::new(),
            pulse: None,
            grow: Grow::rest(now),
            holding: false,
            button: false,
            test_hold: false,
            sensor: Sensor::default(),
            blips: Vec::new(),
            map: None,
            tested: 0,
        }
    }

    /// How far the pulse must reach: the furthest spot or the tracker's
    /// edge.
    fn reach(&self) -> f32 {
        self.spots
            .iter()
            .map(|s| s.distance)
            .fold(self.sensor.range, f32::max)
    }

    /// The pulse's age at `now`, while it shows.
    pub fn pulse_age(&self, now: Instant) -> Option<f32> {
        let age = seconds(now, self.pulse?);
        pulse_alive(age, self.reach()).then_some(age)
    }

    pub fn mode(&self, now: Instant) -> Mode {
        if self.holding {
            Mode::Hold
        } else if self.pulse_age(now).is_some() {
            Mode::Pulse
        } else {
            Mode::None
        }
    }

    /// Spot `i`'s strength at `now`: the hold's (the tracker's growth) or
    /// the pulse's, whichever is more, times [`LIKELY`] for a likely spot
    /// and [`HIDDEN`] behind a wall.
    pub fn strength(&self, i: usize, now: Instant) -> f32 {
        let Some(s) = self.spots.get(i) else {
            return 0.0;
        };
        let hold = self.grow.value(now);
        let pulse = self
            .pulse_age(now)
            .map_or(0.0, |age| pulse_strength(age, s.distance));
        let proven = if s.proven { 1.0 } else { LIKELY };
        let hidden = if s.hidden == Some(true) { HIDDEN } else { 1.0 };
        hold.max(pulse) * proven * hidden
    }

    /// The spots drawn at `now`, and how many of them are behind a wall.
    pub fn shown(&self, now: Instant) -> (usize, usize) {
        (0..self.spots.len())
            .filter(|&i| self.strength(i, now) > MIN_ALPHA)
            .fold((0, 0), |(n, h), i| {
                (n + 1, h + usize::from(self.spots[i].hidden == Some(true)))
            })
    }

    /// Whether anything could be drawn at or after `now`.
    pub fn drawable(&self, now: Instant) -> bool {
        self.at.is_some()
            && !self.spots.is_empty()
            && ((self.sensor.enabled && !self.blips.is_empty())
                || self.pulse_age(now).is_some()
                || self.holding
                || self.grow.value(now) > 0.0)
    }
}

/// What a tick reads.
#[derive(Clone, Copy, Debug)]
pub struct Input<'a> {
    pub now: Instant,
    /// The flashlight action.
    pub down: bool,
    /// `superbounce.view_test {"mode":"hold"}`.
    pub test_hold: bool,
    /// `superbounce.view_test {"mode":"pulse"}` since the last tick.
    pub test_pulse: bool,
    /// `superbounce.view_test {"mode":"none"}` since the last tick.
    pub test_cancel: bool,
    pub at: Option<Vec3>,
    pub yaw: Option<f32>,
    pub spots: &'a [Spot],
    pub sensor: Sensor,
    /// The navpoint's spot.
    pub nav: Option<Vec3>,
}

/// The view's state from tick to tick.
#[derive(Clone, Debug)]
pub struct Ticker {
    button: Button,
    pulse: Option<Instant>,
    grow: Option<Grow>,
    holding: bool,
    sight: SightCache,
    /// The spot list the sight answers are for: its length and first
    /// stand point.
    key: (usize, [u32; 3]),
}

impl Default for Ticker {
    fn default() -> Self {
        Ticker::new()
    }
}

impl Ticker {
    pub const fn new() -> Ticker {
        Ticker {
            button: Button { down: 0 },
            pulse: None,
            grow: None,
            holding: false,
            sight: SightCache::new(),
            key: (0, [0; 3]),
        }
    }

    /// One tick: the press, the sight tests, and the frame to draw.
    pub fn step(
        &mut self,
        input: &Input<'_>,
        sight: &dyn LineOfSight,
        geometry: &Geometry,
    ) -> Frame {
        let now = input.now;
        let mut grow = self.grow.unwrap_or_else(|| Grow::rest(now));
        if input.test_cancel {
            self.pulse = None;
        }
        match self.button.step(input.down) {
            Some(Press::Tap) => self.pulse = Some(now),
            Some(Press::HoldStart | Press::HoldEnd) | None => {}
        }
        if input.test_pulse {
            self.pulse = Some(now);
        }
        let holding = self.button.holding() || input.test_hold;
        if holding != self.holding {
            grow = grow.toward(if holding { 1.0 } else { 0.0 }, now);
            self.holding = holding;
        }
        self.grow = Some(grow);

        let mut frame = Frame {
            pulse: self.pulse,
            grow,
            holding,
            button: input.down,
            test_hold: input.test_hold,
            sensor: input.sensor,
            ..Frame::empty(now)
        };
        let (Some(at), Some(yaw)) = (input.at, input.yaw.filter(|y| y.is_finite())) else {
            return frame;
        };
        frame.at = Some(at);
        frame.yaw = yaw;

        let key = (
            input.spots.len(),
            input
                .spots
                .first()
                .map_or([0; 3], |s| s.stand.map(f32::to_bits)),
        );
        if key != self.key {
            self.sight.clear();
            self.key = key;
        }
        let eye = [at[0], at[1], at[2] + EYE];
        let targets: Vec<Vec3> = input
            .spots
            .iter()
            .map(|s| [s.stand[0], s.stand[1], s.stand[2] + SPOT_LIFT])
            .collect();
        let showing = holding || frame.pulse_age(now).is_some();
        let budget = if showing { SIGHT_BUSY } else { SIGHT_IDLE };
        self.sight.step(eye, &targets, budget, sight);
        frame.tested = self.sight.tested();

        let mut pair = vec![None; input.spots.len()];
        for (c, b) in geo::pairs(input.spots) {
            pair[c] = Some(b);
        }
        frame.spots = input
            .spots
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let d = [s.stand[0] - at[0], s.stand[1] - at[1], s.stand[2] - at[2]];
                ViewSpot {
                    stand: s.stand,
                    kind: s.kind,
                    proven: s.status == Status::Proven,
                    level: geo::level(at[2], s.stand[2]),
                    distance: (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt(),
                    hidden: self.sight.get(i).map(|seen| seen.hidden),
                    pair: pair[i],
                    nav: input.nav.is_some_and(|n| {
                        (n[0] - s.stand[0]).hypot(n[1] - s.stand[1]) < 0.05
                            && (n[2] - s.stand[2]).abs() < 0.05
                    }),
                }
            })
            .collect();
        // A pulse that has faded is forgotten.
        if self.pulse.is_some() && frame.pulse_age(now).is_none() {
            self.pulse = None;
            frame.pulse = None;
        }
        frame.blips = tracker_blips(at, yaw, input.sensor.range, &frame.spots);
        if holding || grow.value(now) > 0.0 || grow.to > 0.0 {
            let local = geometry.local_map(at, MAP_RANGE);
            frame.map = Some(map_view(&local, at, yaw, &frame.spots));
        }
        frame
    }
}

// ---- drawing ----

/// A mark's size is its distance from the eye times this (about 1.1
/// degrees), within [`MARK_MIN`] and [`MARK_MAX`] world units.
pub const MARK_ANGLE: f32 = 0.02;
pub const MARK_MIN: f32 = 0.06;
pub const MARK_MAX: f32 = 1.2;
/// A mark floats this far over its spot's stand point, under its figure.
pub const MARK_RAISE: f32 = 0.3;
/// Marks this far away or nearer carry their distance; further ones are
/// the figure alone.
pub const LABEL_METRES: f32 = 40.0;
/// A crouch spot's ring has this many sides.
pub const RING_SIDES: usize = 12;
/// The line from a crouch spot to its landing, at this much of the hold.
pub const PAIR_ALPHA: f32 = 0.5;
/// The world labels' size: a little under the HUD's own (2).
pub const LABEL_SCALE: f32 = 1.5;

/// One world mark of the view.
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub edges: Vec<[Vec3; 2]>,
    pub anchor: Vec3,
    pub alpha: f32,
    pub dashed: bool,
    pub label: Option<String>,
}

/// A mark's size, `distance` world units from the eye.
pub fn mark_size(distance: f32) -> f32 {
    (distance * MARK_ANGLE).clamp(MARK_MIN, MARK_MAX)
}

/// The flat unit direction across the line of sight from `eye` to `at`:
/// a mark stands in the upright plane facing the eye.
fn across(eye: Vec3, at: Vec3) -> [f32; 2] {
    let (dx, dy) = (at[0] - eye[0], at[1] - eye[1]);
    let l = dx.hypot(dy);
    if l > 1e-4 {
        [-dy / l, dx / l]
    } else {
        [1.0, 0.0]
    }
}

/// A spot's figure of `size` about `centre`, upright and facing `eye`: a
/// ring for a crouch spot, a diamond for a landing.
pub fn figure(kind: Kind, centre: Vec3, eye: Vec3, size: f32) -> Vec<[Vec3; 2]> {
    let side = across(eye, centre);
    let p = |u: f32, v: f32| {
        [
            centre[0] + side[0] * u,
            centre[1] + side[1] * u,
            centre[2] + v,
        ]
    };
    let points: Vec<Vec3> = match kind {
        Kind::Crouch => (0..RING_SIDES)
            .map(|i| {
                let a = std::f32::consts::TAU * i as f32 / RING_SIDES as f32;
                p(size * a.cos(), size * a.sin())
            })
            .collect(),
        Kind::Bounce => vec![p(0.0, size), p(size, 0.0), p(0.0, -size), p(-size, 0.0)],
    };
    let n = points.len();
    (0..n).map(|i| [points[i], points[(i + 1) % n]]).collect()
}

/// The chevron over a figure for a spot on a floor above (pointing up), or
/// under it for one below (pointing down); none on the same floor.
pub fn level_chevron(level: Level, centre: Vec3, eye: Vec3, size: f32) -> Vec<[Vec3; 2]> {
    let (base, tip) = match level {
        Level::Same => return Vec::new(),
        Level::Above => (size * 1.5, size * 2.1),
        Level::Below => (-size * 1.5, -size * 2.1),
    };
    let side = across(eye, centre);
    let p = |u: f32, v: f32| {
        [
            centre[0] + side[0] * u,
            centre[1] + side[1] * u,
            centre[2] + v,
        ]
    };
    let (l, t, r) = (p(-size * 0.6, base), p(0.0, tip), p(size * 0.6, base));
    vec![[l, t], [t, r]]
}

/// A distance as the view writes it: whole metres, at least 1.
pub fn metres(units: f32) -> String {
    format!("{} m", ((units * METRES_PER_UNIT).round() as i32).max(1))
}

/// A map figure's size, HUD pixels.
pub const BLIP_SIZE: f32 = 2.5;
/// The navpoint's ring round its blip.
pub const NAV_RING: f32 = 4.5;
/// The tracker's dots, HUD pixels: the next spot's radius, the one
/// after's, that one's strength, and a dot's sides.
pub const NEXT_DOT: f32 = 2.5;
pub const AFTER_DOT: f32 = 1.8;
pub const AFTER_ALPHA: f32 = 0.55;
pub const DOT_SIDES: usize = 10;
/// The pulse's ring on the tracker at its start.
pub const PULSE_RING_ALPHA: f32 = 0.9;
/// The map's background and floors.
pub const MAP_BACK: f32 = 0.12;
pub const MAP_BACK_ALPHA: f32 = 0.85;
pub const FLOOR_ALPHA: f32 = 0.55;

/// One HUD-space shape, HUD pixels before the host's fit.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Path {
        points: Vec<[f32; 2]>,
        closed: bool,
        width: f32,
        dashed: bool,
        color: [f32; 3],
        alpha: f32,
    },
    Fill {
        points: Vec<[f32; 2]>,
        color: [f32; 3],
        alpha: f32,
    },
    Text {
        at: [f32; 2],
        text: String,
        color: [f32; 3],
        alpha: f32,
        /// HUD pixels a font pixel.
        pixel: f32,
    },
}

impl Shape {
    /// The shape moved by the host's HUD fit `[sx, sy, ox, oy]`
    /// ([`crate::render_aspect`]'s `UiFit::transform`); widths and font
    /// pixels scale with the height, as the window scales them.
    pub fn fitted(&self, [sx, sy, ox, oy]: [f32; 4]) -> Shape {
        let f = |p: [f32; 2]| [p[0] * sx + ox, p[1] * sy + oy];
        match self {
            Shape::Path {
                points,
                closed,
                width,
                dashed,
                color,
                alpha,
            } => Shape::Path {
                points: points.iter().map(|&p| f(p)).collect(),
                closed: *closed,
                width: width * sy,
                dashed: *dashed,
                color: *color,
                alpha: *alpha,
            },
            Shape::Fill {
                points,
                color,
                alpha,
            } => Shape::Fill {
                points: points.iter().map(|&p| f(p)).collect(),
                color: *color,
                alpha: *alpha,
            },
            Shape::Text {
                at,
                text,
                color,
                alpha,
                pixel,
            } => Shape::Text {
                at: f(*at),
                text: text.clone(),
                color: *color,
                alpha: *alpha,
                pixel: pixel * sy,
            },
        }
    }
}

/// Where the tracker is, HUD pixels before the host's fit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackerPlace {
    pub centre: [f32; 2],
    pub radius: f32,
}

impl TrackerPlace {
    /// The tracker of a HUD frame whose bottom-left corner is `corner`.
    pub fn from_corner(corner: [f32; 2]) -> TrackerPlace {
        TrackerPlace {
            centre: [corner[0] + TRACKER_CENTRE[0], corner[1] + TRACKER_CENTRE[1]],
            radius: TRACKER_RADIUS,
        }
    }

    /// The map's radius and centre at growth `g`: its box's bottom-left
    /// corner stays on the tracker's.
    pub fn map(&self, g: f32) -> ([f32; 2], f32) {
        let r = self.radius * (1.0 + (GROW - 1.0) * g.clamp(0.0, 1.0));
        (
            [
                self.centre[0] - self.radius + r,
                self.centre[1] + self.radius - r,
            ],
            r,
        )
    }
}

pub(crate) fn ring(centre: [f32; 2], radius: f32, sides: usize) -> Vec<[f32; 2]> {
    (0..sides)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / sides as f32;
            [centre[0] + radius * a.cos(), centre[1] + radius * a.sin()]
        })
        .collect()
}

fn scaled(c: [f32; 3], k: f32) -> [f32; 3] {
    c.map(|v| (v * k).clamp(0.0, 1.0))
}

/// A tracker blip at `p` (HUD pixels), as the game's own blips are: a
/// filled dot. The next spot's is [`NEXT_DOT`] in radius with a ring
/// round it ([`NAV_RING`]); the one after is smaller and fainter.
fn tracker_shapes(b: &Blip, p: [f32; 2], colour: [f32; 3], alpha: f32, out: &mut Vec<Shape>) {
    let (radius, alpha) = if b.nav {
        (NEXT_DOT, alpha)
    } else {
        (AFTER_DOT, alpha * AFTER_ALPHA)
    };
    if alpha <= MIN_ALPHA {
        return;
    }
    out.push(Shape::Fill {
        points: ring(p, radius, DOT_SIDES),
        color: colour,
        alpha,
    });
    if b.nav {
        out.push(Shape::Path {
            points: ring(p, NAV_RING, 12),
            closed: true,
            width: 1.0,
            dashed: false,
            color: colour,
            alpha,
        });
    }
}

/// A map blip's figures at `p` (HUD pixels): ring or diamond, the floor's
/// chevron, the navpoint's ring; a rim blip is a chevron pointing out.
fn blip_shapes(b: &Blip, p: [f32; 2], colour: [f32; 3], alpha: f32, out: &mut Vec<Shape>) {
    let alpha = alpha * if b.proven { 1.0 } else { LIKELY };
    if alpha <= MIN_ALPHA {
        return;
    }
    let path = |points: Vec<[f32; 2]>, closed: bool| Shape::Path {
        points,
        closed,
        width: 1.0,
        dashed: false,
        color: colour,
        alpha,
    };
    if b.rim {
        // Out along the blip's direction; screen y runs down.
        let (dx, dy) = (b.at[0], -b.at[1]);
        let (nx, ny) = (-dy, dx);
        let s = BLIP_SIZE;
        out.push(path(
            vec![
                [p[0] - dx * s + nx * s, p[1] - dy * s + ny * s],
                [p[0] + dx * s * 0.6, p[1] + dy * s * 0.6],
                [p[0] - dx * s - nx * s, p[1] - dy * s - ny * s],
            ],
            false,
        ));
        return;
    }
    let s = BLIP_SIZE;
    match b.kind {
        Kind::Crouch => out.push(path(ring(p, s, 8), true)),
        Kind::Bounce => out.push(Shape::Fill {
            points: vec![
                [p[0], p[1] - s],
                [p[0] + s, p[1]],
                [p[0], p[1] + s],
                [p[0] - s, p[1]],
            ],
            color: colour,
            alpha,
        }),
    }
    match b.level {
        Level::Same => {}
        Level::Above => out.push(path(
            vec![
                [p[0] - 1.5, p[1] - s - 1.5],
                [p[0], p[1] - s - 3.0],
                [p[0] + 1.5, p[1] - s - 1.5],
            ],
            false,
        )),
        Level::Below => out.push(path(
            vec![
                [p[0] - 1.5, p[1] + s + 1.5],
                [p[0], p[1] + s + 3.0],
                [p[0] + 1.5, p[1] + s + 1.5],
            ],
            false,
        )),
    }
    if b.nav {
        out.push(path(ring(p, NAV_RING, 12), true));
    }
}

/// What a present draws: the tick's frame, the HUD colour, where the
/// tracker is and how the host fits it.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub frame: Arc<Frame>,
    pub colour: [f32; 3],
    pub tracker: TrackerPlace,
    pub fit: [f32; 4],
}

impl View {
    /// The world marks at `now`: each spot's figure, chevron and
    /// distance, and while held the crouch-to-landing lines.
    pub fn world(&self, now: Instant) -> Vec<Mark> {
        let f = &*self.frame;
        let Some(at) = f.at else {
            return Vec::new();
        };
        let eye = [at[0], at[1], at[2] + EYE];
        let mut out = Vec::new();
        for (i, s) in f.spots.iter().enumerate() {
            let alpha = f.strength(i, now);
            if alpha <= MIN_ALPHA {
                continue;
            }
            let base = [s.stand[0], s.stand[1], s.stand[2] + MARK_RAISE];
            let d = {
                let v = [base[0] - eye[0], base[1] - eye[1], base[2] - eye[2]];
                (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
            };
            let size = mark_size(d);
            let centre = [base[0], base[1], base[2] + size];
            let mut edges = figure(s.kind, centre, eye, size);
            edges.extend(level_chevron(s.level, centre, eye, size));
            out.push(Mark {
                edges,
                anchor: [centre[0], centre[1], centre[2] + size * 2.6],
                alpha,
                dashed: s.hidden == Some(true),
                label: (s.distance * METRES_PER_UNIT <= LABEL_METRES).then(|| metres(s.distance)),
            });
        }
        let hold = f.grow.value(now);
        if hold > MIN_ALPHA {
            for s in &f.spots {
                let Some(j) = s.pair else { continue };
                let Some(b) = f.spots.get(j) else { continue };
                let both = if s.proven && b.proven { 1.0 } else { LIKELY };
                let lift = |p: Vec3| [p[0], p[1], p[2] + MARK_RAISE];
                out.push(Mark {
                    edges: vec![[lift(s.stand), lift(b.stand)]],
                    anchor: lift(s.stand),
                    alpha: hold * PAIR_ALPHA * both,
                    dashed: true,
                    label: None,
                });
            }
        }
        out
    }

    /// The tracker's shapes at `now`, HUD pixels before the fit: the
    /// blips, the pulse's ring and the map.
    pub fn hud(&self, now: Instant) -> Vec<Shape> {
        let f = &*self.frame;
        let mut out = Vec::new();
        if f.at.is_none() {
            return out;
        }
        let colour = self.colour;
        let g = f.grow.value(now);
        let t = self.tracker;
        // The blips, fading as the map grows over them.
        if f.sensor.enabled && g < 1.0 {
            for b in &f.blips {
                let r = if b.rim { RIM_RADIUS } else { BLIP_RADIUS };
                let p = [t.centre[0] + b.at[0] * r, t.centre[1] - b.at[1] * r];
                tracker_shapes(b, p, colour, 1.0 - g, &mut out);
            }
        }
        let (centre, radius) = t.map(g);
        // The map.
        if g > MIN_ALPHA
            && let Some(map) = &f.map
        {
            let inner = radius * MAP_INNER;
            let at = |p: [f32; 2]| [centre[0] + p[0] * inner, centre[1] - p[1] * inner];
            out.push(Shape::Fill {
                points: ring(centre, radius, 48),
                color: scaled(colour, MAP_BACK),
                alpha: MAP_BACK_ALPHA * g,
            });
            let span = geo::MAP_BELOW + geo::MAP_ABOVE;
            for (points, rise) in &map.floors {
                let shade = 0.35 + 0.3 * ((rise + geo::MAP_BELOW) / span).clamp(0.0, 1.0);
                out.push(Shape::Fill {
                    points: points.iter().map(|&p| at(p)).collect(),
                    color: scaled(colour, shade),
                    alpha: FLOOR_ALPHA * g,
                });
            }
            for &[a, b] in &map.edges {
                out.push(Shape::Path {
                    points: vec![at(a), at(b)],
                    closed: false,
                    width: 1.0,
                    dashed: false,
                    color: colour,
                    alpha: 0.9 * g,
                });
            }
            for &[a, b] in &map.pairs {
                out.push(Shape::Path {
                    points: vec![at(a), at(b)],
                    closed: false,
                    width: 1.0,
                    dashed: true,
                    color: colour,
                    alpha: 0.6 * g,
                });
            }
            for b in &map.blips {
                let p = at(b.at);
                blip_shapes(b, p, colour, g, &mut out);
            }
            // The player, pointing ahead.
            out.push(Shape::Fill {
                points: vec![
                    [centre[0], centre[1] - 4.0],
                    [centre[0] + 3.0, centre[1] + 3.0],
                    [centre[0] - 3.0, centre[1] + 3.0],
                ],
                color: colour,
                alpha: g,
            });
            out.push(Shape::Path {
                points: ring(centre, radius, 64),
                closed: true,
                width: 1.5,
                dashed: false,
                color: colour,
                alpha: g,
            });
            out.push(Shape::Text {
                at: [
                    centre[0] + radius * 0.8 + 8.0,
                    centre[1] + radius * 0.8 + 4.0,
                ],
                text: metres(MAP_RANGE),
                color: colour,
                alpha: g,
                pixel: 1.0,
            });
        }
        // The pulse's ring, on the tracker or the map, whichever shows.
        if let Some(age) = f.pulse_age(now) {
            let range = f.sensor.range + (MAP_RANGE - f.sensor.range) * g;
            let front = pulse_front(age);
            if front < range {
                let reach = BLIP_RADIUS / TRACKER_RADIUS * radius;
                out.push(Shape::Path {
                    points: ring(centre, front / range * reach, 48),
                    closed: true,
                    width: 1.5,
                    dashed: false,
                    color: colour,
                    alpha: PULSE_RING_ALPHA * (1.0 - front / range),
                });
            }
        }
        out
    }
}

// ---- the game side ----

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

static TICKER: Mutex<Ticker> = Mutex::new(Ticker::new());
static FRAME: Mutex<Option<Arc<Frame>>> = Mutex::new(None);
/// `superbounce.view_test`: the hold it keeps, and the pulse or the
/// cancel it asks of the next tick.
static TEST_HOLD: AtomicBool = AtomicBool::new(false);
static TEST_ASK: AtomicU8 = AtomicU8::new(ASK_NONE);
const ASK_NONE: u8 = 0;
const ASK_PULSE: u8 = 1;
const ASK_CANCEL: u8 = 2;

/// The last tick's frame.
pub fn current() -> Option<Arc<Frame>> {
    lock(&FRAME).clone()
}

/// The first local player's flashlight action.
pub(crate) fn flashlight_down(m: &dyn GuestMemory) -> bool {
    crate::skulls_custom::local_units(m)
        .ok()
        .and_then(|u| u.first().map(|u| u.body))
        .and_then(|body| m.u32(body.wrapping_add(UNIT_CONTROL_FLAGS)).ok())
        .is_some_and(|flags| flags & CONTROL_FLASHLIGHT != 0)
}

/// The motion sensor's range and whether the variant shows it.
fn sensor(m: &dyn GuestMemory) -> Sensor {
    use crate::skulls_custom::{GAME_GLOBALS, GLOBALS_MODE};
    let range = m
        .u32(guest_layout::points::HUD_GLOBALS_TAG.base)
        .ok()
        .filter(|&h| h != 0)
        .and_then(|h| m.f32(h.wrapping_add(HUDG_RADAR_RANGE)).ok())
        .filter(|r| r.is_finite() && *r > 0.0)
        .unwrap_or(TRACKER_RANGE_FALLBACK);
    let enabled = (|| {
        let g = m.u32(GAME_GLOBALS).ok().filter(|&g| g != 0)?;
        if m.u32(g.wrapping_add(GLOBALS_MODE)).ok()? != crate::skulls_mp::MODE_MULTIPLAYER {
            return Some(true);
        }
        let at = crate::variant_ext::ACTIVE_VARIANT + variants::record::FLAGS_AT as u32;
        let flags = m.u32(g.wrapping_add(at)).ok()?;
        Some(flags & live_proto::hopper::variant::flag::MOTION_SENSOR != 0)
    })()
    .unwrap_or(true);
    Sensor { enabled, range }
}

/// The mode's tick: reads the button and the sensor, steps the view and
/// publishes its frame. `spots` and `room` are the tick's own.
pub fn on_tick(o: &hooks::Obs, spots: &[Spot], room: &crate::superbounce_capsule::Room) {
    let (down, sensor) = {
        let guest = o.guest();
        let m: &dyn GuestMemory = &*guest;
        (flashlight_down(m), sensor(m))
    };
    let s = crate::superbounce_mode::state();
    let ask = TEST_ASK.swap(ASK_NONE, Ordering::Relaxed);
    let input = Input {
        now: Instant::now(),
        down,
        test_hold: TEST_HOLD.load(Ordering::Relaxed),
        test_pulse: ask == ASK_PULSE,
        test_cancel: ask == ASK_CANCEL,
        at: s.position,
        yaw: s.yaw,
        spots,
        sensor,
        nav: s.nav_target.map(|n| n.at),
    };
    let geometry = room.view();
    let frame = lock(&TICKER).step(&input, &**geometry, geometry);
    *lock(&FRAME) = Some(Arc::new(frame));
}

/// Forgets the view: a new game, or the mode off.
pub fn reset() {
    *lock(&TICKER) = Ticker::new();
    *lock(&FRAME) = None;
}

// ---- the agent API ----

/// `superbounce.view`'s result for `frame` at `now`.
pub fn reply(
    frame: Option<&Frame>,
    active: bool,
    now: Instant,
) -> agent_proto::superbounce_view::View {
    use agent_proto::superbounce_view as wire;
    let blip = |b: &Blip| wire::Blip {
        kind: match b.kind {
            Kind::Bounce => "bounce",
            Kind::Crouch => "crouch",
        }
        .into(),
        proven: b.proven,
        x: f64::from(b.at[0]),
        y: f64::from(b.at[1]),
        level: b.level.name().into(),
        rim: b.rim,
        nav: b.nav,
    };
    let Some(f) = frame.filter(|_| active) else {
        return wire::View {
            active,
            mode: Mode::None.name().into(),
            test_hold: TEST_HOLD.load(Ordering::Relaxed),
            ..Default::default()
        };
    };
    let (marks, hidden) = f.shown(now);
    let age = f.pulse_age(now);
    wire::View {
        active,
        mode: f.mode(now).name().into(),
        button: f.button,
        test_hold: f.test_hold,
        pulse_age: age.map(f64::from),
        pulse_front: age.map(|a| f64::from(pulse_front(a))),
        grow: f64::from(f.grow.value(now)),
        spots: f.spots.len(),
        marks,
        hidden,
        tested: f.tested,
        tracker: wire::Tracker {
            enabled: f.sensor.enabled,
            range: f64::from(f.sensor.range),
            blips: f.blips.iter().map(blip).collect(),
        },
        map: f.map.as_ref().map(|m| wire::Map {
            floors: m.floors.len(),
            edges: m.edges.len(),
            pairs: m.pairs.len(),
            blips: m.blips.iter().map(blip).collect(),
        }),
    }
}

/// `superbounce.view` now.
pub fn view_reply() -> agent_proto::superbounce_view::View {
    reply(
        current().as_deref(),
        crate::superbounce_mode::active(),
        Instant::now(),
    )
}

/// `superbounce.view_test {"mode": ...}`: `pulse` fires a pulse at the
/// next tick, `hold` holds until `none`, `none` lets go and stops the
/// pulse. Refused while the mode is off.
pub fn view_test(mode: &str) -> Result<agent_proto::superbounce_view::ViewTest, String> {
    if !crate::superbounce_mode::active() {
        return Err("the superbounce mode is off (play a game whose script requires hud.superbounce)".into());
    }
    match mode {
        "pulse" => TEST_ASK.store(ASK_PULSE, Ordering::Relaxed),
        "hold" => TEST_HOLD.store(true, Ordering::Relaxed),
        "none" => {
            TEST_HOLD.store(false, Ordering::Relaxed);
            TEST_ASK.store(ASK_CANCEL, Ordering::Relaxed);
        }
        other => return Err(format!("mode is pulse, hold or none, not {other:?}")),
    }
    Ok(agent_proto::superbounce_view::ViewTest {
        mode: mode.into(),
        test_hold: TEST_HOLD.load(Ordering::Relaxed),
    })
}

/// Adds `superbounce.view` and `superbounce.view_test` to the agent.
pub fn install(agent: &agent::Agent) {
    use agent::Access;
    use agent_proto::superbounce_view as wire;
    use agent_proto::{RpcError, code};
    agent.add_game_method(wire::VIEW, Access::Read, |params, _ctx| {
        if !(params.is_null() || params.as_object().is_some_and(|f| f.is_empty())) {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                "superbounce.view takes no parameters",
            ));
        }
        Ok(serde_json::to_value(view_reply()).expect("superbounce.view serialises"))
    });
    agent.add_game_method(wire::VIEW_TEST, Access::Act, |params, _ctx| {
        let p: wire::ViewTestParams = serde_json::from_value(params)
            .map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))?;
        let r = view_test(&p.mode).map_err(|why| RpcError::new(code::INVALID_PARAMS, why))?;
        Ok(serde_json::to_value(r).expect("superbounce.view_test serialises"))
    });
}

// ---- the window ----

/// Registers the view's world-line source (windowed hosts), beside the
/// mode's own.
#[cfg(feature = "gpu")]
pub use gpu::Source;

#[cfg(feature = "gpu")]
mod gpu {
    use super::*;
    use gfx::world_lines::{
        Glow, HudMark, ImageMarks, ImageValidity, LabelFace, LineStyle, WorldLine,
    };

    /// The bounce view's marks while the mode is on.
    pub struct Source;

    /// The HUD frame's bottom-left corner from the guest, or the
    /// fullscreen one.
    fn corner(m: &dyn GuestMemory) -> [f32; 2] {
        let e = |i: u32| m.u16(HUD_FRAME + 2 * i).ok().map(|v| f32::from(v as i16));
        match (e(0), e(1), e(2), e(3)) {
            (Some(top), Some(left), Some(bottom), Some(right))
                if (0.0..right).contains(&left)
                    && right <= 640.0
                    && (0.0..bottom).contains(&top)
                    && bottom <= 480.0 =>
            {
                [left, bottom]
            }
            _ => FRAME_BOTTOM_LEFT,
        }
    }

    impl crate::world_lines::WorldLineSource for Source {
        fn active(&self) -> bool {
            crate::superbounce_mode::active()
        }

        fn capture(&self, guest: &dyn GuestMemory) -> Option<Arc<dyn ImageMarks>> {
            let frame = current()?;
            if !frame.drawable(Instant::now()) {
                return None;
            }
            let view = View {
                frame,
                colour: crate::superbounce_font::look(guest).colour,
                tracker: TrackerPlace::from_corner(corner(guest)),
                fit: crate::render_aspect::hud_bottom_left_fit(),
            };
            Some(Arc::new(Sample(view)))
        }
    }

    #[derive(Debug)]
    struct Sample(View);

    impl Sample {
        fn draw(&self, now: Instant, visitor: &mut dyn FnMut(WorldLine<'_>)) {
            let style = |alpha: f32| LineStyle {
                width: 2.0,
                alpha,
                glow: Some(Glow {
                    width: 3.0,
                    alpha: 0.4,
                }),
                label_scale: LABEL_SCALE,
                label_centered: true,
                label_face: LabelFace::Hud,
                ..LineStyle::default()
            };
            for m in self.0.world(now) {
                visitor(WorldLine {
                    edges: &m.edges,
                    anchor: m.anchor,
                    color: self.0.colour,
                    dashed: m.dashed,
                    label: m.label.as_deref(),
                    style: style(m.alpha),
                });
            }
        }
    }

    impl ImageMarks for Sample {
        fn lines(&self, visitor: &mut dyn FnMut(WorldLine<'_>)) {
            self.draw(Instant::now(), visitor);
        }

        fn visit(
            &self,
            now: &dyn Fn() -> Instant,
            visitor: &mut dyn FnMut(WorldLine<'_>),
        ) -> Option<ImageValidity> {
            let validity = self.validity(now)?;
            self.draw(now(), visitor);
            Some(validity)
        }

        fn hud(&self, now: &dyn Fn() -> Instant, visitor: &mut dyn FnMut(HudMark<'_>)) {
            for shape in self.0.hud(now()) {
                emit(&shape.fitted(self.0.fit), visitor);
            }
        }
    }

    /// Hands a [`Shape`] (already in the window's HUD space) to the
    /// window as the [`HudMark`] it is.
    pub(crate) fn emit(shape: &Shape, visitor: &mut dyn FnMut(HudMark<'_>)) {
        match shape {
            Shape::Path {
                points,
                closed,
                width,
                dashed,
                color,
                alpha,
            } => visitor(HudMark::Path {
                points,
                closed: *closed,
                width: *width,
                dashed: *dashed,
                color: *color,
                alpha: *alpha,
            }),
            Shape::Fill {
                points,
                color,
                alpha,
            } => visitor(HudMark::Fill {
                points,
                color: *color,
                alpha: *alpha,
            }),
            Shape::Text {
                at,
                text,
                color,
                alpha,
                pixel,
            } => visitor(HudMark::Text {
                at: *at,
                text,
                color: *color,
                alpha: *alpha,
                pixel: *pixel,
                centered: true,
            }),
        }
    }
}

#[cfg(test)]
#[path = "superbounce_view_tests.rs"]
mod tests;
