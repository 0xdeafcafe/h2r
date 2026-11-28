//! Ghost runs (docs/game/superbounce.md, "Ghost runs"): a recorded run
//! that bounced off a spot, replayed in the world on request as one faint
//! outline figure in the HUD's colour, and nothing else: no trail, no
//! ring, no button glyphs. Showing one is the `superbounce-ghost` feature,
//! off by default ([`enabled`]); the recording runs either way.
//!
//! **Recording.** `superbounce_mode` calls [`tick`] once a game tick with
//! every player's reads and the medal chain's awards. Each bot's ticks
//! (and, while `superbounce.verify` runs, the local player's: its trials)
//! go into a [`Track`]. When the chain files a bounce for that player, the
//! track keeps going to the top of the bounce and then makes a
//! [`Ghost`]: the ticks from the crouch that primed the word (a second
//! before it) to the top. The best run of each spot is kept in the map's
//! ghosts file (`superbounce::ghost`, `<map>.ghosts` beside the spots and
//! verified files): one that primed itself beats one whose word was set
//! for it, then the shortest wins. A kept run also proves its spot in the
//! verified file, as a verifier's trial that bounced does.
//!
//! **Showing.** With the feature on, the first local player looks at the
//! chosen strip (the navpoint's while primed, [`showable`]) when it has a
//! run ([`looked_at`]: in range and near the middle of the view), and the
//! prompt line offers [`BUTTON`] (or [`BUTTON_BUMPER_JUMPER`]) ([`hud_verb`]). A press plays the run
//! once; a press while it plays stops it; a press after it has ended plays
//! it again. The playback runs on the clock, not on game ticks
//! ([`pose_at`]): each image the window presents asks for the pose at its
//! own present time and gets it interpolated between two recorded ticks,
//! so the figure moves smoothly and at the recorded speed at 30, 60 or 120
//! frames a second ([`GhostSample`]).
//!
//! The rules are pure ([`Track`], [`looked_at`], [`pose_at`], [`marks`],
//! [`prompt`], [`proof`]); the statics and the guest reads are at the end.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use guest_layout::GuestMemory;
use gfx::world_lines::{ImageMarks, ImageValidity, LineStyle, WorldLine};
use hud_kit_core::views::superbounce::{Medal, Verb};
use superbounce::ghost::{self as file, Button, Frame, Ghost, MAX_FRAMES, Source, TICK_HZ};
use superbounce::verified::Entry;
use superbounce::{Kind, Spot, Status, Vec3};

use crate::superbounce_chain::{Award, CLEAR_FROM, SLOTS, Seen, TELEPORT};
use crate::superbounce_charge::LAUNCH_TICKS;
use crate::superbounce_font::Look;

// ---- the control ----

/// The button that shows a ghost, as the action table numbers physical
/// buttons (`superbounce_assist::button_down`): 2 is X. The fall assist
/// reads X only in the air, so a press counts only on the ground
/// ([`press_counts`]) and the two never clash. On the keyboard it is X's
/// key (`crates/platform/input/src/bindings.rs`). On Bumper Jumper X is
/// the flashlight, which drives the bounce view, so that layout uses
/// [`BUTTON_BUMPER_JUMPER`] ([`button_for`]).
pub const BUTTON: u8 = 2;
/// [`BUTTON`]'s glyph in the HUD font (docs/input/button-icons.md); the
/// prompts in `hud_kit_core::views::superbounce` write the same one.
pub const BUTTON_GLYPH: char = '\u{e102}';
/// The ghost button on the Bumper Jumper layout: B (1), on the ground.
pub const BUTTON_BUMPER_JUMPER: u8 = 1;
/// [`BUTTON_BUMPER_JUMPER`]'s glyph, B.
pub const BUTTON_BUMPER_JUMPER_GLYPH: char = '\u{e101}';
/// Where the player's profile keeps the button layout byte
/// (`layout_glue`'s settings records, `+4`), for the first local player.
const LAYOUT_BYTE: u32 = crate::layout_glue::SETTINGS_BASE
    + crate::layout_glue::SETTINGS_STRIDE * crate::superbounce_assist::PORT
    + 4;

/// The ghost button for the layout stored as profile byte `preset`: B on
/// Bumper Jumper, X on every other layout (and when it cannot be read).
pub fn button_for(preset: Option<u8>) -> u8 {
    match preset.and_then(input::layouts::Layout::from_preset) {
        Some(input::layouts::Layout::BumperJumper) => BUTTON_BUMPER_JUMPER,
        _ => BUTTON,
    }
}

/// A ghost button's glyph.
pub fn glyph_for(button: u8) -> char {
    if button == BUTTON_BUMPER_JUMPER {
        BUTTON_BUMPER_JUMPER_GLYPH
    } else {
        BUTTON_GLYPH
    }
}

/// The ghost button now in force, set from the profile's layout by
/// [`publish_layout`].
static CURRENT_BUTTON: AtomicU8 = AtomicU8::new(BUTTON);

/// The first local player's layout byte from guest memory, once a tick
/// (the tap's read): picks [`button_for`]'s button for the press and the
/// prompt. `None` (memory not readable) leaves X.
pub fn publish_layout(memory: Option<&dyn xapi::GuestMemory>) {
    let mut b = [0u8; 1];
    let preset = match memory.map(|m| m.read(LAYOUT_BYTE, &mut b)) {
        Some(Ok(())) => Some(b[0]),
        _ => None,
    };
    CURRENT_BUTTON.store(button_for(preset), Ordering::Relaxed);
}

/// The ghost button in force.
pub fn current_button() -> u8 {
    CURRENT_BUTTON.load(Ordering::Relaxed)
}
/// The glyphs drawn where the run pressed a button: A for the jump, the
/// left stick's click for crouch (the Default layout's).
pub const JUMP_GLYPH: char = '\u{e100}';
pub const CROUCH_GLYPH: char = '\u{e110}';

// ---- the numbers ----

/// Ticks kept before the crouch that primed the word: the approach.
pub const LEAD_IN: usize = 30;
/// How far back from the first primed tick the crouch that primed it may
/// end, ticks.
pub const PRIME_LOOKBACK: usize = 300;
/// After the bounce the run ends at the top, or at the latest this many
/// ticks after it.
pub const APEX_LIMIT: usize = 90;
/// A take-off from the ground at this vertical speed or more (units/s) is
/// a jump.
pub const JUMP_VZ: f32 = 1.0;
/// A spot counts as looked at within this distance of the eye (world
/// units)...
pub const LOOK_RANGE: f32 = 15.0;
/// ...and this angle off the aim (radians, 6 degrees), or the angle a
/// [`MARK_RADIUS`] mark spans when that is wider (close by).
pub const LOOK_CONE: f32 = 0.105;
pub const MARK_RADIUS: f32 = 0.4;
/// The eye above the origin (world units), standing.
pub const EYE_HEIGHT: f32 = 0.62;
/// After the last frame the figure holds there this long (seconds).
pub const END_HOLD: f32 = 1.0;
/// The figure's strength: faint, so it reads as a hint beside the one
/// marked strip.
pub const FIGURE_ALPHA: f32 = 0.35;
/// A run is the chosen strip's when its spot is within this of the
/// strip's stand point.
pub const CHOSEN_NEAR: f32 = 0.3;
/// The feature that shows ghosts, off by default.
pub const FEATURE: &str = "superbounce-ghost";
/// Bots' slots are looked up again every this many ticks.
pub const BOT_REFRESH: u32 = 30;

// ---- recording ----

/// One player's tick as the recorder takes it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sample {
    pub tick: u32,
    /// The unit's datum: a new one is a new life.
    pub unit: u32,
    pub at: Vec3,
    /// The aim's yaw, radians.
    pub yaw: f32,
    /// Vertical speed, units/s, up positive.
    pub vz: f32,
    pub airborne: bool,
    /// The level pair's word is -3.
    pub primed: bool,
    /// The unit's crouch request (unit flags bit 23).
    pub request: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Flight {
    spot: Vec3,
    /// The touchdown tick's place in the buffer.
    touchdown: usize,
    launch: f32,
}

/// One player's last ticks, and the bounce it is climbing out of.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    buf: VecDeque<Sample>,
    flight: Option<Flight>,
}

fn distance(a: Vec3, b: Vec3) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

impl Track {
    /// Where the player was `n` ticks before the last.
    pub fn at_back(&self, n: usize) -> Option<Vec3> {
        let i = self.buf.len().checked_sub(n + 1)?;
        Some(self.buf[i].at)
    }

    /// The facing at the last tick.
    pub fn last_yaw(&self) -> Option<f32> {
        self.buf.back().map(|s| s.yaw)
    }

    /// Ticks held.
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn clear(&mut self) {
        self.buf.clear();
        self.flight = None;
    }

    /// One tick: `s` is the player's (`None` without a unit), `bounce` the
    /// spot's `stand` when the chain filed a bounce this tick: the chain
    /// files it when its launch window closes, [`LAUNCH_TICKS`] after the
    /// touchdown, so the flight is counted from that many ticks back, its
    /// launch the best vertical speed since. A teleport,
    /// a new unit or a gap in the ticks starts the track again. Returns
    /// the run once a bounce reaches its top.
    pub fn step(
        &mut self,
        s: Option<Sample>,
        bounce: Option<Vec3>,
        source: Source,
    ) -> Option<Ghost> {
        let Some(s) = s else {
            self.clear();
            return None;
        };
        if let Some(last) = self.buf.back() {
            if s.tick == last.tick {
                return None;
            }
            if last.unit != s.unit
                || distance(last.at, s.at) > TELEPORT
                || s.tick != last.tick.wrapping_add(1)
            {
                self.clear();
            }
        }
        self.buf.push_back(s);
        let i = self.buf.len() - 1;
        if let Some(spot) = bounce {
            let window = LAUNCH_TICKS as usize;
            if self.flight.is_none() && i > window {
                let touchdown = i - window;
                let launch = self
                    .buf
                    .range(touchdown..)
                    .map(|s| s.vz)
                    .fold(f32::NEG_INFINITY, f32::max);
                self.flight = Some(Flight {
                    spot,
                    touchdown,
                    launch,
                });
            }
            return None;
        }
        if let Some(f) = self.flight {
            let since = i - f.touchdown;
            if s.vz <= 0.0 || since >= APEX_LIMIT || (!s.airborne && since >= CLEAR_FROM as usize) {
                self.flight = None;
                let g = build(&self.buf, f, source);
                self.trim();
                return g;
            }
            return None;
        }
        self.trim();
        None
    }

    fn trim(&mut self) {
        while self.buf.len() > MAX_FRAMES {
            self.buf.pop_front();
        }
    }
}

/// Where a run that touched down at `td` starts: the crouch that primed
/// the word (the run of crouched ticks that ends at most
/// [`PRIME_LOOKBACK`] before the first primed tick, or on it), else the
/// first primed tick; then [`LEAD_IN`] ticks earlier, as far as the
/// buffer goes.
fn prime_start(buf: &VecDeque<Sample>, td: usize) -> usize {
    let mut p0 = td;
    while p0 > 0 && buf[p0 - 1].primed {
        p0 -= 1;
    }
    let lo = p0.saturating_sub(PRIME_LOOKBACK);
    let hi = p0.min(td.saturating_sub(1));
    let start = match (lo..=hi).rev().find(|&i| buf[i].request) {
        Some(mut k) => {
            while k > 0 && buf[k - 1].request {
                k -= 1;
            }
            k
        }
        None => p0,
    };
    start.saturating_sub(LEAD_IN)
}

fn build(buf: &VecDeque<Sample>, f: Flight, source: Source) -> Option<Ghost> {
    let end = buf.len();
    let mut start = prime_start(buf, f.touchdown);
    if end - start > MAX_FRAMES {
        start = end - MAX_FRAMES;
    }
    if f.touchdown <= start {
        return None;
    }
    let frames = (start..end)
        .map(|i| {
            let s = buf[i];
            let took_off =
                i > 0 && !buf[i - 1].airborne && s.airborne && s.vz >= JUMP_VZ && i < f.touchdown;
            let mut flags = 0;
            for (on, bit) in [
                (s.request, file::CROUCH),
                (took_off, file::JUMP),
                (s.airborne, file::AIRBORNE),
                (s.primed, file::PRIMED),
            ] {
                if on {
                    flags |= bit;
                }
            }
            Frame {
                at: s.at,
                yaw: s.yaw,
                flags,
            }
        })
        .collect();
    Some(Ghost {
        spot: f.spot,
        source,
        launch: f.launch,
        touchdown: f.touchdown - start,
        frames,
    })
}

/// The bounce spot a touchdown at `at` (the origin the tick before) was
/// on: the nearest strip within the landing radius, its `stand`.
pub fn spot_at(spots: &[Spot], at: Vec3) -> Option<Vec3> {
    superbounce::nearest(spots, at)
        .filter(|(_, d)| *d <= crate::superbounce_charge::LANDING_RADIUS)
        .map(|(s, _)| s.stand)
}

/// The verified entry a kept run makes of its spot: proven, landed where
/// the run landed, with `old`'s tally (if any) plus this one.
pub fn proof(g: &Ghost, old: Option<&Entry>) -> Entry {
    let landing = g.landing();
    Entry {
        kind: Kind::Bounce,
        status: Status::Proven,
        at: g.spot,
        offset: [landing[0] - g.spot[0], landing[1] - g.spot[1]],
        yaw: None,
        height: None,
        trials: old.map_or(0, |e| e.trials) + 1,
        ok: old.map_or(0, |e| e.ok) + 1,
        best: old.map_or(0.0, |e| e.best).max(g.launch),
    }
}

// ---- looking at a spot ----

/// The point of `points` the eye at `eye` looking along `aim` looks at:
/// within [`LOOK_RANGE`], and within [`LOOK_CONE`] of the aim (or the
/// angle a [`MARK_RADIUS`] mark spans, when wider); the nearest to the aim
/// if several.
pub fn looked_at(eye: Vec3, aim: Vec3, points: &[Vec3]) -> Option<usize> {
    let n = (aim[0] * aim[0] + aim[1] * aim[1] + aim[2] * aim[2]).sqrt();
    if !(n > 1e-4 && n.is_finite()) {
        return None;
    }
    points
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            let d = [p[0] - eye[0], p[1] - eye[1], p[2] - eye[2]];
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            if !(len > 1e-3 && len <= LOOK_RANGE) {
                return None;
            }
            let cos = (d[0] * aim[0] + d[1] * aim[1] + d[2] * aim[2]) / (len * n);
            let angle = cos.clamp(-1.0, 1.0).acos();
            let cone = LOOK_CONE.max((MARK_RADIUS / len).atan());
            (angle <= cone).then_some((i, angle))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

// ---- the prompt ----

/// What the prompt line offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GhostPrompt {
    /// Looking at a spot with a run: show it.
    Show,
    /// A run is playing: stop it.
    Stop,
}

/// The prompt for a player looking at a spot with a run (`looked`) while
/// one plays or not.
pub fn prompt(looked: bool, playing: bool) -> Option<GhostPrompt> {
    if playing {
        Some(GhostPrompt::Stop)
    } else if looked {
        Some(GhostPrompt::Show)
    } else {
        None
    }
}

/// The prompt line's verb with the ghost's prompt `ghost`: the recipe's
/// verbs come first; the ghost's takes the line when there is none, or
/// over the general "hold crouch and walk under".
/// `button` is the ghost button ([`button_for`]): the prompt shows its glyph.
pub fn hud_verb_with(verb: Option<Verb>, ghost: Option<GhostPrompt>, button: u8) -> Option<Verb> {
    let b = button == BUTTON_BUMPER_JUMPER;
    let g = ghost.map(|g| match (g, b) {
        (GhostPrompt::Show, false) => Verb::ShowGhost,
        (GhostPrompt::Stop, false) => Verb::StopGhost,
        (GhostPrompt::Show, true) => Verb::ShowGhostB,
        (GhostPrompt::Stop, true) => Verb::StopGhostB,
    });
    match verb {
        None | Some(Verb::HoldAndWalkUnder) => g.or(verb),
        Some(v) => Some(v),
    }
}

/// [`hud_verb_with`] with the prompt of the last tick: the HUD's hook.
pub fn hud_verb(verb: Option<Verb>) -> Option<Verb> {
    hud_verb_with(verb, current_prompt(), current_button())
}

// ---- playback ----

/// Where the figure is at a moment of the playback.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub at: Vec3,
    pub yaw: f32,
    /// The run's tick, with the fraction to the next.
    pub tick: f32,
    /// The frame at or before it.
    pub index: usize,
    pub crouch: bool,
    pub airborne: bool,
    /// Past the last frame, holding.
    pub done: bool,
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn lerp_angle(a: f32, b: f32, t: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let d = (b - a + PI).rem_euclid(TAU) - PI;
    a + d * t
}

/// The figure `seconds` into a playback of `g`: the frames are 1/30 s
/// apart, and a time between two is interpolated, so any frame rate gets
/// its own pose at the recorded speed. It holds on the last frame for
/// [`END_HOLD`]; `None` after that (the playback is over).
pub fn pose_at(g: &Ghost, seconds: f32) -> Option<Pose> {
    let last = g.frames.len().checked_sub(1)?;
    let seconds = if seconds.is_finite() {
        seconds.max(0.0)
    } else {
        0.0
    };
    let t = seconds * TICK_HZ;
    if t > last as f32 + END_HOLD * TICK_HZ {
        return None;
    }
    let done = t > last as f32;
    let t = t.min(last as f32);
    let i = (t.floor() as usize).min(last);
    let j = (i + 1).min(last);
    let frac = t - i as f32;
    let (a, b) = (g.frames[i], g.frames[j]);
    Some(Pose {
        at: std::array::from_fn(|k| lerp(a.at[k], b.at[k], frac)),
        yaw: lerp_angle(a.yaw, b.yaw, frac),
        tick: t,
        index: i,
        crouch: a.crouch(),
        airborne: a.airborne(),
        done,
    })
}

/// One group of lines or a label of the ghost.
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub edges: Vec<[Vec3; 2]>,
    pub anchor: Vec3,
    pub dashed: bool,
    pub label: Option<String>,
    pub style: LineStyle,
}

/// The figure's outline, its right half from the neck down to the
/// crotch, in figure heights: across, up. Mirrored for the left.
const OUTLINE: [[f32; 2]; 14] = [
    [0.05, 0.82],
    [0.21, 0.78],
    [0.25, 0.70],
    [0.25, 0.47],
    [0.19, 0.44],
    [0.16, 0.66],
    [0.14, 0.52],
    [0.15, 0.40],
    [0.14, 0.20],
    [0.14, 0.03],
    [0.15, 0.00],
    [0.04, 0.00],
    [0.04, 0.38],
    [0.00, 0.42],
];
const HEAD: ([f32; 2], f32) = ([0.0, 0.905], 0.075);
const CHEST: [[f32; 2]; 2] = [[-0.09, 0.56], [0.09, 0.74]];

fn figure_style() -> LineStyle {
    LineStyle {
        width: 1.5,
        alpha: FIGURE_ALPHA,
        ..LineStyle::default()
    }
}

/// The outline figure standing at `pose`, flat to a viewer at `viewer`
/// (facing the way it faces without one): a closed outline, a head, a
/// chest plate and a tick on the floor the way it faces. Crouched, it is
/// as tall as the crouched capsule.
pub fn figure(pose: &Pose, viewer: Option<Vec3>) -> Vec<[Vec3; 2]> {
    let facing = [pose.yaw.cos(), pose.yaw.sin()];
    let toward = viewer
        .map(|v| [pose.at[0] - v[0], pose.at[1] - v[1]])
        .filter(|d| d[0].hypot(d[1]) > 0.1)
        .map(|d| {
            let n = d[0].hypot(d[1]);
            [d[0] / n, d[1] / n]
        })
        .unwrap_or(facing);
    let right = [-toward[1], toward[0]];
    let across = superbounce::STANDING;
    let up = if pose.crouch {
        superbounce::CROUCHED
    } else {
        superbounce::STANDING
    };
    let p = |[u, v]: [f32; 2]| -> Vec3 {
        [
            pose.at[0] + right[0] * u * across,
            pose.at[1] + right[1] * u * across,
            pose.at[2] + v * up,
        ]
    };
    let mut loop_pts: Vec<[f32; 2]> = OUTLINE.to_vec();
    loop_pts.extend(OUTLINE.iter().rev().skip(1).map(|[u, v]| [-u, *v]));
    let mut edges: Vec<[Vec3; 2]> = Vec::with_capacity(loop_pts.len() + 16);
    for k in 0..loop_pts.len() {
        edges.push([p(loop_pts[k]), p(loop_pts[(k + 1) % loop_pts.len()])]);
    }
    let (c, r) = HEAD;
    let head = |k: usize| {
        let a = std::f32::consts::TAU * (k % 8) as f32 / 8.0;
        p([c[0] + r * a.cos(), c[1] + r * a.sin() * across / up])
    };
    for k in 0..8 {
        edges.push([head(k), head(k + 1)]);
    }
    let [[x0, y0], [x1, y1]] = CHEST;
    let chest = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
    for k in 0..4 {
        edges.push([p(chest[k]), p(chest[(k + 1) % 4])]);
    }
    let foot = [pose.at[0], pose.at[1], pose.at[2] + 0.02];
    edges.push([
        foot,
        [
            foot[0] + facing[0] * 0.3,
            foot[1] + facing[1] * 0.3,
            foot[2],
        ],
    ]);
    edges
}

/// Everything drawn `seconds` into a playback of `g` for a viewer at
/// `viewer`: the figure, faint, and nothing else. Empty once the playback
/// is over.
pub fn marks(g: &Ghost, seconds: f32, viewer: Option<Vec3>) -> Vec<Mark> {
    let Some(pose) = pose_at(g, seconds) else {
        return Vec::new();
    };
    vec![Mark {
        edges: figure(&pose, viewer),
        anchor: pose.at,
        dashed: false,
        label: None,
        style: figure_style(),
    }]
}

/// The runs a player may be shown: with the feature `on`, those of the
/// `chosen` strip (within [`CHOSEN_NEAR`] of its stand point), by index
/// in `ghosts`. None with the feature off or no chosen strip.
pub fn showable(ghosts: &[Ghost], chosen: Option<Vec3>, on: bool) -> Vec<usize> {
    let Some(c) = chosen.filter(|_| on) else {
        return Vec::new();
    };
    ghosts
        .iter()
        .enumerate()
        .filter(|(_, g)| distance(g.spot, c) <= CHOSEN_NEAR)
        .map(|(i, _)| i)
        .collect()
}

/// Where a run left the floor for its bounce: the approach guide's lip
/// (`superbounce_guide::Lip`, docs/game/superbounce.md, "The takeoff").
/// The last ground frame before the flight that touched down, the way
/// and the ground speed of its first tick in the air, and whether that
/// tick was a jump's (`file::JUMP`) or a walk-off's. `None` for a run
/// with no ground frame before its touchdown, or one that left it
/// standing still (no heading).
pub fn lip(g: &Ghost) -> Option<crate::superbounce_guide::Lip> {
    use crate::superbounce_guide::{Leave, Lip};
    let end = g.touchdown.min(g.frames.len().checked_sub(1)?);
    let k = (0..end)
        .rev()
        .find(|&k| !g.frames[k].airborne() && g.frames[k + 1].airborne())?;
    let (on, off) = (g.frames[k], g.frames[k + 1]);
    let d = [off.at[0] - on.at[0], off.at[1] - on.at[1]];
    let step = d[0].hypot(d[1]);
    (step > 1e-4 && step.is_finite()).then(|| Lip {
        at: on.at,
        heading: [d[0] / step, d[1] / step],
        speed: Some(step * TICK_HZ),
        run: Some(if off.jump() { Leave::Jump } else { Leave::Drop }),
    })
}

/// The lip of the kept run of the strip at `stand` on `map` ([`lip`]):
/// the first run within [`CHOSEN_NEAR`] of it that has one.
pub fn lip_for(map: &str, stand: Vec3) -> Option<crate::superbounce_guide::Lip> {
    ghosts_for(map)
        .iter()
        .filter(|g| distance(g.spot, stand) <= CHOSEN_NEAR)
        .find_map(lip)
}

/// One playback as the window draws it: the run, when it started, where
/// the viewer stood when the image was captured, and the HUD's look. Each
/// present asks [`ImageMarks::visit`] with its own time, so an image shown
/// at 120 frames a second draws four poses, each where the run was then.
#[derive(Debug)]
pub struct GhostSample {
    pub ghost: Arc<Ghost>,
    pub started: Instant,
    pub viewer: Option<Vec3>,
    pub look: Look,
}

impl GhostSample {
    fn draw(&self, now: Instant, visitor: &mut dyn FnMut(WorldLine<'_>)) {
        let seconds = now.saturating_duration_since(self.started).as_secs_f32();
        for m in marks(&self.ghost, seconds, self.viewer) {
            visitor(WorldLine {
                edges: &m.edges,
                anchor: m.anchor,
                color: self.look.colour,
                dashed: m.dashed,
                label: m.label.as_deref(),
                style: m.style,
            });
        }
    }
}

impl ImageMarks for GhostSample {
    fn lines(&self, visitor: &mut dyn FnMut(WorldLine<'_>)) {
        self.draw(Instant::now(), visitor);
    }

    fn visit(
        &self,
        now: &dyn Fn() -> Instant,
        visitor: &mut dyn FnMut(WorldLine<'_>),
    ) -> Option<ImageValidity> {
        self.draw(now(), visitor);
        Some(ImageValidity::Image)
    }
}

// ---- the ghosts files ----

/// `<map>.ghosts`, in the bots directory beside the spots and verified
/// files.
pub fn file_name(map: &str) -> String {
    bots::cache::file_name(map, 0).replace(".0.bots", ".ghosts")
}

#[derive(Clone, Debug, Default)]
struct Kept {
    ghosts: Arc<Vec<Ghost>>,
    /// Why the file was not read; it is not written over then.
    error: Option<String>,
    /// The directory it was read from.
    dir: Option<PathBuf>,
}

static STORE: Mutex<Option<HashMap<String, Kept>>> = Mutex::new(None);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The bots directory, where the verified files are.
fn dir() -> Option<PathBuf> {
    bots::spots::verified_dir()
}

fn read(dir: Option<&PathBuf>, map: &str) -> Kept {
    let Some(d) = dir else {
        return Kept::default();
    };
    let (ghosts, error) = match std::fs::read_to_string(d.join(file_name(map))) {
        Ok(text) => match file::parse(&text) {
            Ok(g) => (g, None),
            Err(why) => (Vec::new(), Some(why)),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Vec::new(), None),
        Err(e) => (Vec::new(), Some(e.to_string())),
    };
    if let Some(why) = &error {
        eprintln!("superbounce: {map}'s ghosts not read, and not written over: {why}");
    }
    Kept {
        ghosts: Arc::new(ghosts),
        error,
        dir: Some(d.clone()),
    }
}

fn kept(map: &str) -> Kept {
    let key = map.to_ascii_lowercase();
    let d = dir();
    let mut s = lock(&STORE);
    let maps = s.get_or_insert_with(HashMap::new);
    match maps.get(&key) {
        Some(k) if k.dir == d => k.clone(),
        _ => {
            let k = read(d.as_ref(), &key);
            maps.insert(key, k.clone());
            k
        }
    }
}

/// The ghost runs of `map` (any case): read once from the bots directory,
/// then kept; empty without a directory or a file.
pub fn ghosts_for(map: &str) -> Arc<Vec<Ghost>> {
    kept(map).ghosts
}

/// Forgets what was kept of `map`'s ghosts file and reads it again
/// (`superbounce.reload`). The run counts before and after.
pub fn reload(map: &str) -> (usize, usize) {
    let before = ghosts_for(map).len();
    if let Some(maps) = lock(&STORE).as_mut() {
        maps.remove(&map.to_ascii_lowercase());
    }
    (before, ghosts_for(map).len())
}

/// Keeps `g` for `map` if it beats the run of its spot, and writes the
/// file (unless it could not be read). Whether it was kept.
fn keep(map: &str, g: Ghost) -> bool {
    let key = map.to_ascii_lowercase();
    let mut k = kept(&key);
    let mut all = (*k.ghosts).clone();
    if !file::offer(&mut all, g) {
        return false;
    }
    if let (Some(d), None) = (&k.dir, &k.error) {
        let path = d.join(file_name(&key));
        let written = std::fs::create_dir_all(d)
            .and_then(|_| bots::autonav::write_atomic(&path, file::to_text(&all).as_bytes()));
        if let Err(e) = written {
            eprintln!("superbounce: {} not written: {e}", path.display());
        }
    }
    k.ghosts = Arc::new(all);
    lock(&STORE).get_or_insert_with(HashMap::new).insert(key, k);
    true
}

/// Marks the spot of a kept run proven in the verified file, unless it is
/// already.
fn prove(map: &str, g: &Ghost) {
    let probe = Spot {
        kind: Kind::Bounce,
        stand: g.spot,
        ..Spot::default()
    };
    let entries = bots::spots::verified_for(map);
    let old = superbounce::verified::entry_for(&entries, &probe);
    if old.is_some_and(|e| e.status == Status::Proven) {
        return;
    }
    if let Err(e) = bots::spots::store_verified(map, &[proof(g, old)]) {
        eprintln!("superbounce: a ghost run's spot not written as proven: {e}");
    }
}

// ---- the game ----

/// The button as the assist's tap read it: held, and pressed since the
/// last tick.
static HELD: AtomicBool = AtomicBool::new(false);
static PRESSED: AtomicBool = AtomicBool::new(false);
/// A playback is on (the world-line source polls it).
static PLAYING: AtomicBool = AtomicBool::new(false);
/// [`GhostPrompt`] of the last tick: 0 none, 1 show, 2 stop.
static PROMPT: AtomicU8 = AtomicU8::new(0);

/// Whether [`BUTTON`] is down in the pad the guest last read on the first
/// controller (`superbounce_assist`'s tap, while the mode is on); a press
/// is its going down.
pub fn publish_button(down: Option<bool>) {
    if down == Some(true) {
        if !HELD.swap(true, Ordering::Relaxed) {
            PRESSED.store(true, Ordering::Relaxed);
        }
    } else {
        HELD.store(false, Ordering::Relaxed);
    }
}

fn take_press() -> bool {
    PRESSED.swap(false, Ordering::Relaxed)
}

/// Whether a press taken this tick counts: only while the player is on the
/// ground, where the fall assist does not read X. An unknown player (none
/// local) counts as on the ground.
fn press_counts(pressed: bool, airborne: Option<bool>) -> bool {
    pressed && airborne != Some(true)
}

/// The prompt of the last tick.
pub fn current_prompt() -> Option<GhostPrompt> {
    match PROMPT.load(Ordering::Relaxed) {
        1 => Some(GhostPrompt::Show),
        2 => Some(GhostPrompt::Stop),
        _ => None,
    }
}

fn publish_prompt(p: Option<GhostPrompt>) {
    let v = match p {
        None => 0,
        Some(GhostPrompt::Show) => 1,
        Some(GhostPrompt::Stop) => 2,
    };
    PROMPT.store(v, Ordering::Relaxed);
}

/// Whether a playback is on.
pub fn playing() -> bool {
    PLAYING.load(Ordering::Relaxed)
}

#[derive(Clone, Debug)]
struct Playback {
    index: usize,
    ghost: Arc<Ghost>,
    started: Instant,
}

impl Playback {
    fn over(&self, now: Instant) -> bool {
        pose_at(
            &self.ghost,
            now.saturating_duration_since(self.started).as_secs_f32(),
        )
        .is_none()
    }
}

struct Runtime {
    tracks: Vec<Track>,
    bots: Vec<u8>,
    bots_at: Option<u32>,
    map: Option<String>,
    playback: Option<Playback>,
    looked: Option<usize>,
    /// The first local player's eye, for the figure's facing.
    viewer: Option<Vec3>,
    recorded: u32,
}

impl Runtime {
    fn new() -> Runtime {
        Runtime {
            tracks: vec![Track::default(); SLOTS],
            bots: Vec::new(),
            bots_at: None,
            map: None,
            playback: None,
            looked: None,
            viewer: None,
            recorded: 0,
        }
    }

    fn set_playback(&mut self, p: Option<Playback>) {
        PLAYING.store(p.is_some(), Ordering::Relaxed);
        self.playback = p;
        publish_prompt(prompt(self.looked.is_some(), self.playback.is_some()));
    }
}

static RUNTIME: Mutex<Option<Runtime>> = Mutex::new(None);

/// Forgets the tracks, the playback and the prompt: a new game, the end
/// of one, or the mode going off.
pub fn reset() {
    *lock(&RUNTIME) = None;
    PLAYING.store(false, Ordering::Relaxed);
    publish_prompt(None);
    PRESSED.store(false, Ordering::Relaxed);
}

/// The slots of the bots (phantoms) in the game.
fn bot_slots() -> Vec<u8> {
    bots::phantom::list()
        .into_iter()
        .filter_map(|p| p.player.and_then(|i| u8::try_from(i).ok()))
        .collect()
}

/// A unit's crouch request and aim.
fn read_unit(m: &dyn GuestMemory, unit: u32) -> Option<(bool, Vec3)> {
    use live_telemetry::layout::{UNIT_AIM, UNIT_FLAG_CROUCH, UNIT_FLAGS};
    let (_, body) = guest_layout::view::objects::get(m, guest_layout::handle::DatumHandle(unit)).ok()??;
    let request = m
        .u32(body.addr.wrapping_add(UNIT_FLAGS))
        .is_ok_and(|f| f & UNIT_FLAG_CROUCH != 0);
    let aim = m
        .vec3(body.addr.wrapping_add(UNIT_AIM))
        .ok()
        .filter(|a| a.iter().all(|v| v.is_finite()))?;
    Some((request, aim))
}

fn yaw_of(aim: Vec3) -> Option<f32> {
    (aim[0].hypot(aim[1]) > 0.05).then(|| aim[1].atan2(aim[0]))
}

/// One game tick of the mode (`superbounce_mode`): records the bots' and
/// the verifier's runs from `players` and the chain's `awards`, finds the
/// spot the first local player looks at, and takes a press of [`BUTTON`] on the ground.
/// Only the `chosen` strip's run is offered, and only with the feature on
/// ([`showable`]).
pub fn tick(
    m: &dyn GuestMemory,
    tick: u32,
    players: &[Seen],
    awards: &[Award],
    map: Option<&str>,
    spots: &[Spot],
    chosen: Option<Vec3>,
) {
    let mut guard = lock(&RUNTIME);
    let rt = guard.get_or_insert_with(Runtime::new);
    if rt.map.as_deref() != map {
        rt.map = map.map(str::to_owned);
        rt.set_playback(None);
        for t in &mut rt.tracks {
            t.clear();
        }
    }
    if rt
        .bots_at
        .is_none_or(|at| tick.wrapping_sub(at) >= BOT_REFRESH)
    {
        rt.bots = bot_slots();
        rt.bots_at = Some(tick);
    }
    let verifying = crate::superbounce_verify::running();
    for slot in 0..SLOTS {
        let seen = players.iter().find(|p| usize::from(p.slot) == slot);
        let source = if rt.bots.contains(&(slot as u8)) {
            Some(Source::Bot)
        } else if verifying && seen.is_some_and(|p| p.user.is_some()) {
            Some(Source::Verify)
        } else {
            None
        };
        let track = &mut rt.tracks[slot];
        let Some(source) = source else {
            if !track.is_empty() {
                track.clear();
            }
            continue;
        };
        let sample = seen.and_then(|p| p.tick).map(|t| {
            let unit = read_unit(m, t.unit);
            Sample {
                tick,
                unit: t.unit,
                at: t.position,
                yaw: unit
                    .and_then(|(_, aim)| yaw_of(aim))
                    .or(track.last_yaw())
                    .unwrap_or(0.0),
                vz: t.velocity[2],
                airborne: t.airborne,
                primed: t.word.is_some_and(crate::superbounce_mode::primed),
                request: unit.is_some_and(|(r, _)| r),
            }
        });
        let bounced = awards.iter().any(|a| {
            usize::from(a.slot) == slot
                && !a.medals.is_empty()
                && !a.medals.contains(&Medal::Bounced)
        });
        // The chain files a bounce when its window closes: the origin
        // judged is the tick before the touchdown, LAUNCH_TICKS before the
        // last one kept.
        let bounce = if bounced {
            track
                .at_back(LAUNCH_TICKS as usize)
                .and_then(|p| spot_at(spots, p))
        } else {
            None
        };
        if let Some(g) = track.step(sample, bounce, source)
            && let Some(map) = map
        {
            let g2 = g.clone();
            if keep(map, g) {
                rt.recorded += 1;
                prove(map, &g2);
            }
        }
    }
    // The first local player: what it looks at.
    let local = players
        .iter()
        .filter(|p| p.tick.is_some())
        .filter_map(|p| Some((p.user?, p.tick?)))
        .min_by_key(|(user, _)| *user)
        .map(|(_, t)| t);
    let ghosts = map.map(ghosts_for).unwrap_or_default();
    let eye = local.map(|t| [t.position[0], t.position[1], t.position[2] + EYE_HEIGHT]);
    rt.viewer = eye;
    let on = enabled();
    let offered = showable(&ghosts, chosen, on);
    rt.looked = match (eye, local.and_then(|t| read_unit(m, t.unit))) {
        (Some(eye), Some((_, aim))) if !offered.is_empty() => {
            let points: Vec<Vec3> = offered.iter().map(|&i| ghosts[i].spot).collect();
            looked_at(eye, aim, &points).map(|k| offered[k])
        }
        _ => None,
    };
    // Off: nothing plays, whoever asked.
    if !on && rt.playback.is_some() {
        rt.set_playback(None);
    }
    let now = Instant::now();
    if rt.playback.as_ref().is_some_and(|p| p.over(now)) {
        rt.set_playback(None);
    }
    // Taken every tick, so a press in the air is spent, not kept for the
    // landing.
    if press_counts(take_press(), local.map(|t| t.airborne)) {
        if rt.playback.is_some() {
            rt.set_playback(None);
        } else if let Some(i) = rt.looked
            && let Some(g) = ghosts.get(i)
        {
            rt.set_playback(Some(Playback {
                index: i,
                ghost: Arc::new(g.clone()),
                started: now,
            }));
        }
    }
    publish_prompt(prompt(rt.looked.is_some(), rt.playback.is_some()));
}

/// The playback to draw, with the viewer: `None` while none is on.
pub fn sample(look: Look) -> Option<GhostSample> {
    let g = lock(&RUNTIME);
    let rt = g.as_ref()?;
    let p = rt.playback.as_ref()?;
    Some(GhostSample {
        ghost: Arc::clone(&p.ghost),
        started: p.started,
        viewer: rt.viewer,
        look,
    })
}

// ---- the agent ----

/// `superbounce.ghosts` takes no parameters: null or `{}`.
pub fn no_params(params: &serde_json::Value) -> Result<(), &'static str> {
    if params.is_null() || params.as_object().is_some_and(|f| f.is_empty()) {
        Ok(())
    } else {
        Err("superbounce.ghosts takes no parameters")
    }
}

fn wide(v: Vec3) -> [f64; 3] {
    v.map(f64::from)
}

/// A run as the agent lists it.
pub fn run_reply(index: usize, g: &Ghost) -> agent_proto::superbounce::GhostRun {
    use agent_proto::superbounce as wire;
    wire::GhostRun {
        index,
        at: wide(g.spot),
        landing: wide(g.landing()),
        source: g.source.name().into(),
        ticks: g.ticks(),
        launch: f64::from(g.launch),
        crouch_primed: g.crouch_primed(),
        presses: g
            .presses()
            .into_iter()
            .map(|p| wire::GhostPress {
                frame: p.frame as u32,
                button: match p.button {
                    Button::Crouch => "crouch",
                    Button::Jump => "jump",
                }
                .into(),
            })
            .collect(),
    }
}

/// `superbounce.ghosts` as the agent answers it.
pub fn ghosts_reply() -> agent_proto::superbounce::Ghosts {
    use agent_proto::superbounce as wire;
    let g = lock(&RUNTIME);
    let rt = g.as_ref();
    let map = rt.and_then(|r| r.map.clone());
    let k = map.as_deref().map(kept).unwrap_or_default();
    let file = map
        .as_deref()
        .and_then(|m| dir().map(|d| d.join(file_name(&m.to_ascii_lowercase()))))
        .map(|p| p.display().to_string());
    let now = Instant::now();
    let playing = rt.and_then(|r| r.playback.as_ref()).and_then(|p| {
        let seconds = now.saturating_duration_since(p.started).as_secs_f32();
        let pose = pose_at(&p.ghost, seconds)?;
        Some(wire::GhostPlaying {
            index: p.index,
            seconds: f64::from(seconds),
            tick: f64::from(pose.tick),
            at: wide(pose.at),
            done: pose.done,
        })
    });
    wire::Ghosts {
        active: crate::superbounce_mode::active(),
        map,
        file,
        error: k.error.clone(),
        ghosts: k
            .ghosts
            .iter()
            .enumerate()
            .map(|(i, g)| run_reply(i, g))
            .collect(),
        looked_at: rt.and_then(|r| r.looked),
        prompt: hud_verb_with(None, current_prompt(), current_button())
            .map(|v| v.text().to_owned()),
        playing,
        recorded: rt.map_or(0, |r| r.recorded),
    }
}

/// `superbounce.ghost_show`: plays run `spot` of the loaded map (the one
/// the player looks at without it) from its start, or stops the playback.
pub fn ghost_show(
    p: &agent_proto::superbounce::GhostShowParams,
) -> Result<agent_proto::superbounce::GhostShow, String> {
    use agent_proto::superbounce as wire;
    if !crate::superbounce_mode::active() {
        return Err(
            "the superbounce mode is off (play a game whose script requires hud.superbounce)"
                .into(),
        );
    }
    if !enabled() && !p.stop {
        return Err(format!(
            "ghosts are off: start the game with H2_FEATURES={FEATURE}"
        ));
    }
    let mut g = lock(&RUNTIME);
    let rt = g.get_or_insert_with(Runtime::new);
    if p.stop {
        if p.spot.is_some() {
            return Err("give spot or stop, not both".into());
        }
        rt.set_playback(None);
        return Ok(wire::GhostShow::default());
    }
    let Some(map) = rt.map.clone() else {
        return Err("no map loaded".into());
    };
    let ghosts = ghosts_for(&map);
    let Some(index) = p.spot.or(rt.looked) else {
        return Err("give spot (superbounce.ghosts lists them), or look at one".into());
    };
    let Some(ghost) = ghosts.get(index) else {
        return Err(format!("no ghost {index}: {map} has {}", ghosts.len()));
    };
    let ticks = ghost.ticks();
    rt.set_playback(Some(Playback {
        index,
        ghost: Arc::new(ghost.clone()),
        started: Instant::now(),
    }));
    Ok(wire::GhostShow {
        playing: true,
        index: Some(index),
        ticks: Some(ticks),
    })
}

// ---- the feature ----

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Whether ghosts are shown: the `superbounce-ghost` feature is on. It is
/// off unless asked for (`H2_FEATURES=superbounce-ghost`).
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// The feature: a switch for showing ghost runs. The drawing is the
/// window's; it has no guest hooks.
pub struct SuperbounceGhost;

impl hooks::Feature for SuperbounceGhost {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(FEATURE, "Superbounce ghost runs shown in the world")
            .doc("docs/game/superbounce.md")
            .presentation_only()
            .off_by_default()
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        // The playback is asked for again after a load.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

/// Adds the feature to `registry`; ghosts show only with
/// `H2_FEATURES=superbounce-ghost`.
pub fn install_feature(registry: &mut hooks::Registry) -> anyhow::Result<()> {
    let on = registry
        .add(&SuperbounceGhost)
        .map_err(|e| anyhow::anyhow!("superbounce ghost: {e}"))?;
    ENABLED.store(on, Ordering::Relaxed);
    Ok(())
}

/// Registers the methods with the agent (`agent_glue`).
pub fn install(agent: &agent::Agent) {
    use agent::Access;
    use agent_proto::{RpcError, code, superbounce as wire};
    agent.add_game_method(wire::GHOSTS, Access::Read, |params, _ctx| {
        no_params(&params).map_err(|why| RpcError::new(code::INVALID_PARAMS, why))?;
        Ok(serde_json::to_value(ghosts_reply()).expect("superbounce.ghosts serialises"))
    });
    agent.add_game_method(wire::GHOST_SHOW, Access::Act, |params, _ctx| {
        let p: wire::GhostShowParams = if params.is_null() {
            Default::default()
        } else {
            serde_json::from_value(params)
                .map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))?
        };
        let r = ghost_show(&p).map_err(|why| RpcError::new(code::INVALID_PARAMS, why))?;
        Ok(serde_json::to_value(r).expect("superbounce.ghost_show serialises"))
    });
}

/// Registers the ghost's world-line source before boot (windowed hosts).
#[cfg(feature = "gpu")]
pub fn register(host: &mut crate::Host) {
    host.add_world_line_source(Arc::new(gpu::Lines));
}

#[cfg(feature = "gpu")]
mod gpu {
    use super::*;
    use crate::world_lines::WorldLineSource;

    /// Draws the playback while one is on.
    pub struct Lines;

    impl WorldLineSource for Lines {
        fn active(&self) -> bool {
            super::playing()
        }

        fn capture(&self, guest: &dyn GuestMemory) -> Option<Arc<dyn ImageMarks>> {
            let s = sample(crate::superbounce_font::look(guest))?;
            Some(Arc::new(s) as Arc<dyn ImageMarks>)
        }
    }
}

#[cfg(test)]
mod tests;
