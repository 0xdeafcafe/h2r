//! Seam drops (docs/runtime/sim.md, "Seam drops"): every seat of a match
//! is dropped, again and again, onto points of candidate collision edges
//! with the Havok pair word held at -3, and each drop's outcome is
//! recorded. No walking and no nav: a seat is placed by teleport, settles,
//! is primed, lifted, released, and the fall is watched tick by tick.
//!
//! Pure, like the rest of the crate: the match's `[drops]` table
//! ([`DropsConfig`]) and its checks, the targets and the drops planned on
//! them ([`Target`], [`plan`]), the split over processes
//! ([`shard_targets`]), the shared queue the seats pull from ([`Queue`]),
//! one drop tick by tick ([`DropRun`]), the control gate ([`gate`]) and
//! the match's whole job ([`Job`]). The report, the fits and the proven
//! targets are [`report`]'s. The game glue (`sim_drops` in the game
//! crate) reads each seat's body and level pairs into a [`Sense`] a tick
//! and carries out the [`Act`] it gets back.
//!
//! **Holding the prime.** A teleport onto the floor, a settle with the
//! word written 0 (a teleport keeps the word, and a primed body settling
//! on a seam could bounce), the word written -3 on the still body, a
//! teleport straight up, and then -3 written again at the start of every
//! tick of the fall over every pair the body has against a fixed body.
//! The word of each pair is read before each write, so a pair that
//! vanishes, is rebuilt (a new agent) or is new on the touchdown tick
//! shows. The touchdown tick is decided by the word it ran with
//! (docs/game/superbounce.md, "The touchdown tick"): the pairs the tick
//! before had, all written -3, must come out of the tick still -3, or the
//! drop is `prime_lost`, never `no_bounce`.

use std::collections::VecDeque;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub mod report;

#[cfg(test)]
mod tests;

pub use report::{DropsReport, Gain, Proven, markdown, proven, summarize};

/// The run's report block's schema tag.
pub const FORMAT: &str = "h2-sim-drops/1";
/// The pair word of a primed body.
pub const PRIMED: u32 = 0xffff_fffd;
/// Gravity (world units a second squared): `superbounce::speed::GRAVITY`.
pub const GRAVITY: f32 = 4.15;
/// A body falling at least this fast (units/s, down negative) on the row
/// before is touching down when it stops falling (the HUD's
/// `TOUCHDOWN_VZ`).
pub const TOUCHDOWN_VZ: f32 = -1.0;
/// It has stopped falling when its vertical speed is over this (the
/// HUD's `LANDED_VZ`).
pub const LANDED_VZ: f32 = -0.5;
/// A launch (units/s up, the best of the touchdown row and the three
/// after) at least this fast is a bounce: the scan's smallest candidate
/// launch. An unprimed landing comes out near 0.
pub const BOUNCE_VZ: f32 = 3.0;
/// Default heights (world units, origin over the target's floor).
pub const HEIGHTS: [f32; 6] = [0.5, 1.0, 2.0, 3.5, 5.0, 8.0];
/// Default offsets across the edge: -0.12 to 0.12 in 0.02 steps.
pub const ACROSS: Across = Across::Range {
    from: -0.12,
    to: 0.12,
    step: 0.02,
};
/// Default points along an edge.
pub const ALONG: u32 = 5;
/// Seats when the file has no `[[bot]]`: every phantom a match can hold.
pub const SEATS: u32 = crate::config::MAX_BOTS as u32;
/// Tick defaults and caps.
pub const SETTLE_TICKS: u32 = 240;
pub const FALL_TICKS: u32 = 300;
pub const AFTER_TICKS: u32 = 15;
pub const MAX_TICKS: u32 = 3600;
/// Setup failures retried this many times a drop.
pub const RETRIES: u32 = 2;
/// Still rows (on the ground, slower than [`STILL_SPEED`]) that settle a
/// body. Far under the 38 still ticks after which a body sleeps and its
/// pair is rebuilt at 0 (`superbounce_charge::SLEEP_TICKS`).
pub const SETTLE_STILL: u32 = 3;
pub const STILL_SPEED: f32 = 0.1;
/// Faster than this while settling, the body's velocity is zeroed.
pub const STOP_SPEED: f32 = 1.0;
/// Rows a teleport is waited for.
pub const TELEPORT_WAIT: u32 = 15;
/// Rows after the match starts before the first drop: the spawns settle.
pub const START_TICKS: u32 = 30;
/// A drop starts only this far (horizontally) from every other seat and
/// every other seat's drop, so falling bodies never meet.
pub const CLEAR: f32 = 1.2;
/// The rest point a drop settles on is this far over the floor, plus the
/// across offset and the lead (a ramp beside a crease rises at most 45°).
pub const REST_LIFT: f32 = 0.05;
/// Every seat idle this many rows with drops left: the next drop is taken
/// whoever stands near it.
pub const STALL_TICKS: u32 = 60;
/// How far into the queue a seat looks for a drop clear of the others.
pub const QUEUE_SCAN: usize = 2048;
/// The most drops a run plans.
pub const MAX_DROPS: usize = 250_000;
/// The control's map and drop: the Ascension slot seam, its stepped
/// run's origin (docs/game/superbounce.md, "The pair word", "The
/// touchdown tick"). A free fall from 3.51 reaches the stepped run's 5.4
/// units/s touchdown under [`GRAVITY`]; 3.6 and 4.5 are just over it.
pub const CONTROL_MAP: &str = "ascension";
pub const CONTROL_POINT: [f32; 3] = [-10.54, 4.04, -3.19];
pub const CONTROL_EDGE: [[f32; 3]; 2] = [[-11.10, 4.0, -3.2], [-9.98, 4.0, -3.2]];
pub const CONTROL_HEIGHTS: [f32; 2] = [3.6, 4.5];
pub const CONTROL_ID: &str = "control";

fn is_false(b: &bool) -> bool {
    !*b
}

fn one() -> u32 {
    1
}

// ---- the table ----

/// A match's `[drops]` table (docs/runtime/sim.md, "Seam drops"). Plain
/// values first, tables last: TOML writes them in that order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DropsConfig {
    /// The map, when the match file leaves `map` out (the same if both).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<String>,
    /// `h2 sim drops`'s results directory, relative to the file (default
    /// `results/<file stem>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out: Option<PathBuf>,
    /// Seats when the file has no `[[bot]]` (default 15, the most).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seats: Option<u32>,
    /// Points along each edge (default 5); a busy point or a point target
    /// has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub along: Option<u32>,
    /// Each drop this many times (default 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeats: Option<u32>,
    /// `targets = "scan"`: at most this many of the scan's, in its order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_targets: Option<u32>,
    /// The launch (units/s up) that counts as a bounce (default 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounce_vz: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settle_ticks: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fall_ticks: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_ticks: Option<u32>,
    /// Setup failures retried this many times a drop (default 2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retries: Option<u32>,
    /// No control drop (a map with no known one).
    #[serde(default, skip_serializing_if = "is_false")]
    pub no_control: bool,
    /// This run's share of a `h2 sim drops` batch (set by the batch).
    #[serde(default)]
    pub shard: u32,
    #[serde(default = "one")]
    pub shards: u32,
    /// Origin heights over the target's floor (default [`HEIGHTS`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub heights: Vec<f32>,
    /// Offsets across the edge, toward its drift (default [`ACROSS`]): a
    /// list, or `{ from, to, step }`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub across: Option<Across>,
    /// A horizontal push through the fall, from either side of the edge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approach: Option<Approach>,
    /// The known-good drop every run starts with. Default on Ascension:
    /// the slot seam ([`CONTROL_POINT`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control: Option<ControlSpec>,
    /// `"scan"`, every bounce candidate of the map's `.spots`, or a list.
    #[serde(default)]
    pub targets: Targets,
}

impl Default for DropsConfig {
    fn default() -> DropsConfig {
        DropsConfig {
            map: None,
            out: None,
            seats: None,
            along: None,
            repeats: None,
            max_targets: None,
            bounce_vz: None,
            settle_ticks: None,
            fall_ticks: None,
            after_ticks: None,
            retries: None,
            no_control: false,
            shard: 0,
            shards: 1,
            heights: Vec::new(),
            across: None,
            approach: None,
            control: None,
            targets: Targets::default(),
        }
    }
}

/// What a drops table aims at.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Targets {
    /// `"scan"`.
    Word(String),
    List(Vec<TargetSpec>),
}

impl Default for Targets {
    fn default() -> Targets {
        Targets::Word("scan".into())
    }
}

impl Targets {
    pub fn is_scan(&self) -> bool {
        matches!(self, Targets::Word(w) if w == "scan")
    }
}

/// The offsets across an edge.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Across {
    List(Vec<f32>),
    Range { from: f32, to: f32, step: f32 },
}

impl Across {
    /// The offsets, each rounded to 0.1 mm.
    pub fn values(&self) -> Result<Vec<f32>, String> {
        let round = |v: f32| (v * 10_000.0).round() / 10_000.0;
        let v = match self {
            Across::List(v) => v.clone(),
            &Across::Range { from, to, step } => {
                if !(from.is_finite() && to.is_finite() && step.is_finite()) || step <= 0.0 {
                    return Err("across: from, to and a step over 0".into());
                }
                if to < from {
                    return Err("across: to is under from".into());
                }
                let n = ((to - from) / step + 1e-3).floor() as usize;
                if n > 1000 {
                    return Err("across: at most 1000 offsets".into());
                }
                (0..=n).map(|i| round(from + step * i as f32)).collect()
            }
        };
        if v.is_empty() {
            return Err("across: at least one offset".into());
        }
        if let Some(bad) = v.iter().find(|x| !(x.is_finite() && x.abs() <= 1.0)) {
            return Err(format!("across: {bad} is outside -1..1"));
        }
        Ok(v.into_iter().map(round).collect())
    }
}

/// Which way the approach pushes, across the edge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Side {
    /// A straight drop.
    #[default]
    #[serde(rename = "none")]
    None,
    /// Toward the edge's drift (positive across).
    #[serde(rename = "+")]
    Plus,
    /// Against it.
    #[serde(rename = "-")]
    Minus,
}

impl Side {
    pub fn name(self) -> &'static str {
        match self {
            Side::None => "none",
            Side::Plus => "+",
            Side::Minus => "-",
        }
    }

    fn sign(self) -> f32 {
        match self {
            Side::None => 0.0,
            Side::Plus => 1.0,
            Side::Minus => -1.0,
        }
    }
}

/// `[drops.approach]`: the stick held across the edge through the fall.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Approach {
    /// The stick, 0..1 of full.
    pub stick: f32,
    /// The drop starts this far back (world units) against the push, so
    /// it lands nearer the point (default 0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead: Option<f32>,
    /// Each drop once from each side (default `["+", "-"]`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub from: Vec<Side>,
}

/// `[drops.control]`: a known-good drop.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlSpec {
    /// The drop's point: X and Y exactly, Z its floor's origin.
    pub point: [f32; 3],
    /// Heights over the floor; the control passes when one bounces.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub heights: Vec<f32>,
    /// The seam it lands on, for the across offset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge: Option<[[f32; 3]; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drift: Option<[f32; 2]>,
}

impl ControlSpec {
    /// Ascension's slot seam (docs/game/superbounce.md, "The touchdown
    /// tick").
    pub fn ascension() -> ControlSpec {
        ControlSpec {
            point: CONTROL_POINT,
            heights: CONTROL_HEIGHTS.to_vec(),
            edge: Some(CONTROL_EDGE),
            drift: Some([0.0, 1.0]),
        }
    }
}

/// One explicit target: an edge or a point.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// `slot`, `crease`, `vee`, `busy` (the scan's), or anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<String>,
    /// The collision edge, its two end vertices. A busy point's is its
    /// vertex twice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge: Option<[[f32; 3]; 2]>,
    /// A point instead of an edge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub point: Option<[f32; 3]>,
    /// The across direction's side (X and Y); default the edge's left.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drift: Option<[f32; 2]>,
    /// The spot's `stand` as the scan gave it, for the verified file
    /// (default the edge's middle).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stand: Option<[f32; 3]>,
}

/// A table's settings, the defaults filled in.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub seats: u32,
    pub heights: Vec<f32>,
    pub across: Vec<f32>,
    pub along: u32,
    pub repeats: u32,
    pub sides: Vec<Side>,
    pub stick: f32,
    pub lead: f32,
    pub bounce_vz: f32,
    pub settle_ticks: u32,
    pub fall_ticks: u32,
    pub after_ticks: u32,
    pub retries: u32,
    pub control: Option<ControlSpec>,
}

fn finite(v: &[f32]) -> bool {
    v.iter().all(|c| c.is_finite())
}

fn heights_ok(name: &str, h: &[f32], p: &mut Vec<String>) {
    if let Some(bad) = h.iter().find(|x| !(x.is_finite() && **x >= 0.2 && **x <= 20.0)) {
        p.push(format!("{name}: {bad} is outside 0.2..20"));
    }
}

fn ticks(name: &str, v: Option<u32>, default: u32, p: &mut Vec<String>) -> u32 {
    match v {
        Some(0) => {
            p.push(format!("drops: {name} is at least 1"));
            default
        }
        Some(t) if t > MAX_TICKS => {
            p.push(format!("drops: {name} is at most {MAX_TICKS}"));
            default
        }
        Some(t) => t,
        None => default,
    }
}

impl DropsConfig {
    /// The settings, or every problem with them. `map` is the match's.
    pub fn settings(&self, map: &str) -> Result<Settings, Vec<String>> {
        let mut p = Vec::new();
        let seats = self.seats.unwrap_or(SEATS);
        if seats == 0 || seats > SEATS {
            p.push(format!("drops: seats is 1 to {SEATS}"));
        }
        let heights = if self.heights.is_empty() {
            HEIGHTS.to_vec()
        } else {
            self.heights.clone()
        };
        heights_ok("drops: heights", &heights, &mut p);
        let across = self.across.as_ref().unwrap_or(&ACROSS).values().unwrap_or_else(|e| {
            p.push(format!("drops: {e}"));
            vec![0.0]
        });
        let along = self.along.unwrap_or(ALONG);
        if along == 0 || along > 64 {
            p.push("drops: along is 1 to 64 points".into());
        }
        let repeats = self.repeats.unwrap_or(1);
        if repeats == 0 || repeats > 20 {
            p.push("drops: repeats is 1 to 20".into());
        }
        let (sides, stick, lead) = match &self.approach {
            None => (vec![Side::None], 0.0, 0.0),
            Some(a) => {
                if !(a.stick.is_finite() && (0.0..=1.0).contains(&a.stick)) {
                    p.push("drops: approach.stick is 0..1".into());
                }
                let lead = a.lead.unwrap_or(0.0);
                if !(lead.is_finite() && (0.0..=3.0).contains(&lead)) {
                    p.push("drops: approach.lead is 0..3 units".into());
                }
                let mut from = if a.from.is_empty() {
                    vec![Side::Plus, Side::Minus]
                } else {
                    a.from.clone()
                };
                from.dedup();
                (from, a.stick, lead)
            }
        };
        let bounce_vz = self.bounce_vz.unwrap_or(BOUNCE_VZ);
        if !(bounce_vz.is_finite() && bounce_vz > 0.5) {
            p.push("drops: bounce_vz is over 0.5 units/s".into());
        }
        let retries = self.retries.unwrap_or(RETRIES);
        if retries > 10 {
            p.push("drops: retries is at most 10".into());
        }
        let control = match (&self.control, self.no_control) {
            (Some(_), true) => {
                p.push("drops: a [drops.control] and no_control = true; use one".into());
                None
            }
            (Some(c), false) => {
                let mut c = c.clone();
                if c.heights.is_empty() {
                    c.heights = CONTROL_HEIGHTS.to_vec();
                }
                heights_ok("drops: control.heights", &c.heights, &mut p);
                if !finite(&c.point)
                    || c.edge.is_some_and(|e| !finite(&e[0]) || !finite(&e[1]))
                    || c.drift.is_some_and(|d| !finite(&d))
                {
                    p.push("drops: control: point, edge and drift are world coordinates".into());
                }
                Some(c)
            }
            (None, true) => None,
            (None, false) if map.eq_ignore_ascii_case(CONTROL_MAP) => Some(ControlSpec::ascension()),
            (None, false) => {
                p.push(format!(
                    "drops: no known control drop on {map}; give [drops.control] or no_control = true"
                ));
                None
            }
        };
        if self.shards == 0 || self.shards > 256 {
            p.push("drops: shards is 1 to 256".into());
        } else if self.shard >= self.shards {
            p.push(format!(
                "drops: shard {} is not under shards {}",
                self.shard, self.shards
            ));
        }
        match &self.targets {
            Targets::Word(w) if w == "scan" => {}
            Targets::Word(w) => p.push(format!("drops: targets = \"{w}\": \"scan\" or a list")),
            Targets::List(l) => {
                for (i, t) in l.iter().enumerate() {
                    if let Err(e) = Target::from_spec(i, t) {
                        p.push(format!("drops: target {}: {e}", i + 1));
                    }
                }
            }
        }
        let settle_ticks = ticks("settle_ticks", self.settle_ticks, SETTLE_TICKS, &mut p);
        let fall_ticks = ticks("fall_ticks", self.fall_ticks, FALL_TICKS, &mut p);
        let after_ticks = ticks("after_ticks", self.after_ticks, AFTER_TICKS, &mut p);
        if !p.is_empty() {
            return Err(p);
        }
        Ok(Settings {
            seats,
            heights,
            across,
            along,
            repeats,
            sides,
            stick,
            lead,
            bounce_vz,
            settle_ticks,
            fall_ticks,
            after_ticks,
            retries,
            control,
        })
    }

    /// Every problem with the table in a match on `map`, with or without
    /// `superbounce`, a `[drill]` and a `[train]`.
    pub fn problems(&self, map: &str, superbounce: bool, drill: bool, train: bool) -> Vec<String> {
        let mut p = self.settings(map).err().unwrap_or_default();
        if superbounce {
            p.push("drops: leave superbounce out; the drops drive every seat, the bots don't practise".into());
        }
        if drill || train {
            p.push("drops: a [drill] or a [train] is another way to fly a site; use one".into());
        }
        if let Some(m) = &self.map
            && !map.is_empty()
            && !m.eq_ignore_ascii_case(map)
        {
            p.push(format!("drops: map = \"{m}\" but the match's map is \"{map}\""));
        }
        p
    }

    /// The seats a file with no `[[bot]]` gets.
    pub fn seat_bots(&self) -> Vec<crate::config::BotSpec> {
        let n = self.seats.unwrap_or(SEATS).min(SEATS);
        (0..n)
            .map(|i| crate::config::BotSpec {
                name: Some(format!("Dropper {}", i + 1)),
                ..crate::config::BotSpec::default()
            })
            .collect()
    }
}

// ---- targets ----

fn sub2(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn dot2(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}

fn xy(p: [f32; 3]) -> [f32; 2] {
    [p[0], p[1]]
}

fn unit2(v: [f32; 2]) -> Option<[f32; 2]> {
    let l = v[0].hypot(v[1]);
    (l.is_finite() && l > 1e-4).then(|| [v[0] / l, v[1] / l])
}

/// Horizontal distance from `p` to the segment `a`..`b`.
pub fn segment_xy(a: [f32; 2], b: [f32; 2], p: [f32; 2]) -> f32 {
    let ab = sub2(b, a);
    let l2 = dot2(ab, ab);
    let t = if l2 < 1e-12 {
        0.0
    } else {
        (dot2(sub2(p, a), ab) / l2).clamp(0.0, 1.0)
    };
    let c = [a[0] + ab[0] * t, a[1] + ab[1] * t];
    let d = sub2(p, c);
    d[0].hypot(d[1])
}

/// A target, resolved: its edge, the unit direction across it (toward
/// its drift), and the stand point its verified entry is about.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Target {
    pub id: String,
    pub shape: String,
    pub edge: [[f32; 3]; 2],
    pub normal: [f32; 2],
    pub stand: [f32; 3],
    #[serde(default, skip_serializing_if = "is_false")]
    pub control: bool,
}

impl Target {
    /// Target `i` of a list.
    pub fn from_spec(i: usize, t: &TargetSpec) -> Result<Target, String> {
        let edge = match (t.edge, t.point) {
            (Some(_), Some(_)) => return Err("an edge or a point, not both".into()),
            (None, None) => return Err("an edge or a point".into()),
            (Some(e), None) => e,
            (None, Some(p)) => [p, p],
        };
        if !finite(&edge[0]) || !finite(&edge[1]) {
            return Err("the edge is world coordinates".into());
        }
        if let Some(d) = t.drift
            && !finite(&d)
        {
            return Err("drift is two numbers".into());
        }
        if let Some(s) = t.stand
            && !finite(&s)
        {
            return Err("stand is world coordinates".into());
        }
        let shape = t.shape.clone().unwrap_or_else(|| {
            if t.point.is_some() { "point" } else { "edge" }.into()
        });
        Ok(Target::new(
            t.id.clone().unwrap_or_else(|| format!("t{i:03}")),
            shape,
            edge,
            t.drift,
            t.stand,
            false,
        ))
    }

    fn new(
        id: String,
        shape: String,
        edge: [[f32; 3]; 2],
        drift: Option<[f32; 2]>,
        stand: Option<[f32; 3]>,
        control: bool,
    ) -> Target {
        let drift = drift.and_then(unit2);
        let normal = match unit2(sub2(xy(edge[1]), xy(edge[0]))) {
            Some(d) => {
                let left = [-d[1], d[0]];
                match drift {
                    Some(dr) if dot2(dr, left) < 0.0 => [-left[0], -left[1]],
                    _ => left,
                }
            }
            None => drift.unwrap_or([0.0, 1.0]),
        };
        let mid = [
            (edge[0][0] + edge[1][0]) / 2.0,
            (edge[0][1] + edge[1][1]) / 2.0,
            (edge[0][2] + edge[1][2]) / 2.0 + 0.01,
        ];
        Target {
            id,
            shape,
            edge,
            normal,
            stand: stand.unwrap_or(mid),
            control,
        }
    }

    /// The control as a target.
    pub fn control(c: &ControlSpec) -> Target {
        let edge = c.edge.unwrap_or([c.point, c.point]);
        Target::new(
            CONTROL_ID.into(),
            CONTROL_ID.into(),
            edge,
            c.drift,
            Some(c.point),
            true,
        )
    }

    /// A busy point or a point target: one place, no length.
    pub fn is_point(&self) -> bool {
        unit2(sub2(xy(self.edge[1]), xy(self.edge[0]))).is_none()
    }

    /// The edge's point `t` (0..1) of the way along.
    pub fn at(&self, t: f32) -> [f32; 3] {
        let [a, b] = self.edge;
        [
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t,
        ]
    }

    /// How far across the edge's line `p` is, signed toward the drift.
    pub fn across_of(&self, p: [f32; 2]) -> f32 {
        dot2(sub2(p, xy(self.edge[0])), self.normal)
    }

    /// How far (0..1) along the edge `p` is.
    pub fn along_of(&self, p: [f32; 2]) -> f32 {
        let ab = sub2(xy(self.edge[1]), xy(self.edge[0]));
        let l2 = dot2(ab, ab);
        if l2 < 1e-12 {
            0.0
        } else {
            (dot2(sub2(p, xy(self.edge[0])), ab) / l2).clamp(0.0, 1.0)
        }
    }

    /// How far `p` is from the edge, horizontally (a busy point's: from
    /// its vertex).
    pub fn miss(&self, p: [f32; 2]) -> f32 {
        segment_xy(xy(self.edge[0]), xy(self.edge[1]), p)
    }

    /// The along points (0..1) a drop lands on: `n` evenly spaced, each
    /// in the middle of its share, so none sits on a vertex.
    pub fn alongs(&self, n: u32) -> Vec<f32> {
        if self.is_point() || n <= 1 {
            return vec![if self.is_point() { 0.0 } else { 0.5 }];
        }
        (0..n).map(|i| (i as f32 + 0.5) / n as f32).collect()
    }
}

// ---- the plan ----

/// One planned drop.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Drop {
    /// Its index in the run's plan.
    pub id: u32,
    /// Its target's index in the run's targets.
    pub target: u32,
    /// 0..1 along the edge.
    pub along: f32,
    /// Across the edge, toward the drift.
    pub across: f32,
    /// The origin's height over the floor at the point.
    pub height: f32,
    pub side: Side,
    pub repeat: u32,
    /// The point aimed at, on the floor (the edge's height).
    pub point: [f32; 3],
    /// Where the lift puts the origin: over the point (back by the lead
    /// against an approach), `height` up.
    pub start: [f32; 3],
    /// Where it settles first, under `start`.
    pub rest: [f32; 3],
    pub control: bool,
}

/// How many drops `t` gets under `s`.
pub fn drops_per_target(t: &Target, s: &Settings) -> usize {
    if t.control {
        return s.control.as_ref().map_or(0, |c| c.heights.len());
    }
    t.alongs(s.along).len() * s.across.len() * s.heights.len() * s.sides.len() * s.repeats as usize
}

fn push_drop(out: &mut Vec<Drop>, s: &Settings, ti: usize, t: &Target, point: [f32; 3], along: f32, across: f32, height: f32, side: Side, repeat: u32) {
    let back = side.sign() * s.lead;
    let sx = point[0] - t.normal[0] * back;
    let sy = point[1] - t.normal[1] * back;
    out.push(Drop {
        id: out.len() as u32,
        target: ti as u32,
        along,
        across,
        height,
        side,
        repeat,
        point,
        start: [sx, sy, point[2] + height],
        rest: [sx, sy, point[2] + REST_LIFT + across.abs() + s.lead],
        control: t.control,
    });
}

/// The run's drops over `targets`: a control target's heights at its
/// point, every other target's along points × across offsets × heights ×
/// sides × repeats, target by target.
pub fn plan(s: &Settings, targets: &[Target]) -> Vec<Drop> {
    let mut out = Vec::new();
    for (ti, t) in targets.iter().enumerate() {
        if t.control {
            let Some(c) = &s.control else { continue };
            let along = t.along_of(xy(c.point));
            let floor = if t.is_point() { c.point[2] } else { t.at(along)[2] };
            let point = [c.point[0], c.point[1], floor];
            let across = t.across_of(xy(c.point));
            for &h in &c.heights {
                push_drop(&mut out, s, ti, t, point, along, across, h, Side::None, 0);
            }
            continue;
        }
        for a in t.alongs(s.along) {
            let on = t.at(a);
            for &h in &s.heights {
                for &c in &s.across {
                    let point = [on[0] + t.normal[0] * c, on[1] + t.normal[1] * c, on[2]];
                    for &side in &s.sides {
                        for r in 0..s.repeats {
                            push_drop(&mut out, s, ti, t, point, a, c, h, side, r);
                        }
                    }
                }
            }
        }
    }
    out
}

/// Splits `targets` (with their drop counts, `weights`) into `shards`
/// runs, the heaviest first onto the lightest run so far (ties: the lower
/// run), each run's targets kept in the list's order. At most one run a
/// target, at least one run.
pub fn shard_targets<T: Clone>(targets: &[T], weights: &[usize], shards: usize) -> Vec<Vec<T>> {
    let n = shards.min(targets.len()).max(1);
    let mut order: Vec<usize> = (0..targets.len()).collect();
    order.sort_by(|&a, &b| weights[b].cmp(&weights[a]).then(a.cmp(&b)));
    let mut load = vec![0usize; n];
    let mut owner = vec![0usize; targets.len()];
    for i in order {
        let (best, _) = load
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.cmp(b.1).then(a.0.cmp(&b.0)))
            .expect("at least one run");
        load[best] += weights[i];
        owner[i] = best;
    }
    let mut out = vec![Vec::new(); n];
    for (i, t) in targets.iter().enumerate() {
        out[owner[i]].push(t.clone());
    }
    out
}

// ---- the queue ----

/// The drops not yet taken, in plan order. A seat takes the first one
/// clear of every other seat and their drops.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Queue {
    left: VecDeque<u32>,
}

impl Queue {
    pub fn new(ids: impl IntoIterator<Item = u32>) -> Queue {
        Queue {
            left: ids.into_iter().collect(),
        }
    }

    pub fn len(&self) -> usize {
        self.left.len()
    }

    pub fn is_empty(&self) -> bool {
        self.left.is_empty()
    }

    /// The first drop (of the first [`QUEUE_SCAN`]) whose start is at
    /// least `clear` from every point of `busy`, taken.
    pub fn take(&mut self, drops: &[Drop], busy: &[[f32; 2]], clear: f32) -> Option<u32> {
        let i = self.left.iter().take(QUEUE_SCAN).position(|&id| {
            let p = xy(drops[id as usize].start);
            busy.iter().all(|b| {
                let d = sub2(p, *b);
                d[0].hypot(d[1]) >= clear
            })
        })?;
        self.left.remove(i)
    }

    /// The first drop, whoever is near it.
    pub fn take_any(&mut self) -> Option<u32> {
        self.left.pop_front()
    }

    /// A drop to try again, last.
    pub fn put_back(&mut self, id: u32) {
        self.left.push_back(id);
    }
}

// ---- one drop ----

/// One of a body's pairs against a fixed body: its agent (which changes
/// when the pair is rebuilt) and its word.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairSense {
    pub agent: u32,
    pub word: u32,
}

/// What the glue reads of a seat at the start of a tick: the state the
/// tick before left.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sense {
    /// The unit's origin; `None` between lives.
    pub pos: Option<[f32; 3]>,
    pub vel: Option<[f32; 3]>,
    pub airborne: bool,
    /// Its pairs against fixed bodies, words as read (before any write).
    pub pairs: Vec<PairSense>,
    /// Its body sleeps (an inactive island).
    pub asleep: bool,
    /// The teleport asked for came back this tick.
    pub teleported: Option<Result<(), String>>,
}

/// What the glue does for a seat this tick, before the tick runs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Act {
    /// Teleport the unit's origin here, facing this way (X and Y).
    pub teleport: Option<([f32; 3], [f32; 2])>,
    /// Write this word over every pair against a fixed body whose word
    /// differs.
    pub word: Option<u32>,
    /// Zero the body's velocity.
    pub stop: bool,
    /// The walk, a world direction scaled 0..1.
    pub walk: [f32; 2],
    /// Read the contacts now (the touchdown) and hand them to
    /// [`Job::contacts`].
    pub contacts: bool,
}

/// What came of a drop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Launched at least `bounce_vz`.
    Bounced,
    /// Touched down primed and didn't launch.
    NoBounce,
    /// Touched down without the prime holding through the touchdown tick.
    PrimeLost,
    /// Never dropped as planned: a refused teleport, no settle, no
    /// touchdown, the seat died.
    #[default]
    SetupFailed,
}

impl Outcome {
    pub const ALL: [Outcome; 4] = [
        Outcome::Bounced,
        Outcome::NoBounce,
        Outcome::PrimeLost,
        Outcome::SetupFailed,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Outcome::Bounced => "bounced",
            Outcome::NoBounce => "no_bounce",
            Outcome::PrimeLost => "prime_lost",
            Outcome::SetupFailed => "setup_failed",
        }
    }
}

/// One contact point at touchdown (`live_telemetry::havok::Point`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ContactPoint {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub distance: f32,
    pub impulse: f32,
}

/// The touchdown: the last falling row's state (the tick the capsule met
/// the floor began from it) and the row after.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Touchdown {
    /// Rows from the release to the touchdown tick's start.
    pub t: u32,
    /// The game tick the touchdown ran on.
    pub tick: u64,
    pub pos: [f32; 3],
    pub vel: [f32; 3],
    /// The speed in, downward.
    pub speed: f32,
    pub after_pos: [f32; 3],
    pub after_vel: [f32; 3],
    /// Across the edge's line at touchdown, signed toward the drift.
    pub across: f32,
    /// How far from the edge (a point's: from it), horizontally.
    pub miss: f32,
    /// The body's contacts after the touchdown tick (at most 8).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contacts: Vec<ContactPoint>,
}

/// The launch out of a touchdown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Launch {
    /// The best vertical speed of the touchdown row and the three after.
    pub speed: f32,
    /// The climb from the lowest point at touchdown to the top.
    pub height: f32,
    /// The top wasn't reached in `after_ticks`: the rest of the climb is
    /// ballistic, `vz² / 2g`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub estimated: bool,
}

/// A run of rows with the same word and pair count. `word` is the first
/// pair's word that isn't -3, else -3; `None` with no pair.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordSpan {
    pub word: Option<i32>,
    pub pairs: u8,
    pub ticks: u32,
}

/// The first row (from the release) of each thing a drop can show.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Firsts {
    /// The first fall row after the first write whose pairs read anything
    /// but -3 (or none): the prime slipping, even if the write held it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lost: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lost_why: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub touchdown: Option<u32>,
    /// The first row moving up at least `bounce_vz`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<u32>,
}

/// One row of a traced drop.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Row {
    /// Rows from the release (the arm row is the one before it, -1).
    pub t: i32,
    pub phase: String,
    pub pos: Option<[f32; 3]>,
    pub vz: Option<f32>,
    pub airborne: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub asleep: bool,
    /// The pairs as read, before the write.
    pub pairs: Vec<PairSense>,
    /// The word written this row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrote: Option<i32>,
}

/// One drop's record.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub drop: u32,
    pub target: String,
    pub shape: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub control: bool,
    pub point: [f32; 3],
    pub height: f32,
    pub across: f32,
    pub along: f32,
    pub side: Side,
    pub repeat: u32,
    pub seat: u8,
    /// Tries (setup failures are tried again).
    pub attempts: u32,
    /// The game tick of the release (or of the drop's first row).
    pub tick: u64,
    pub outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// The origin's height at the release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_z: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub touchdown: Option<Touchdown>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<Launch>,
    /// Whether the touchdown tick ran with every pair at -3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prime_held: Option<bool>,
    /// The fall's words, run-length (one row a tick).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<WordSpan>,
    pub first: Firsts,
    /// Every row from the arm on: the control's drops and the first drop
    /// of each outcome only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<Row>,
}

/// Whether the touchdown tick ran primed: `before` the pairs of the row
/// before it (every one written -3), `after` the pairs it left. `Err`
/// says how it was lost: `no_pair` (nothing to write before it),
/// `rebuilt` (a pair it had is gone), `reset (word w)` (a pair it had
/// came out not -3, which only a rebuild does), `new_pair (word w)` (a
/// pair made in the tick, not -3).
pub fn prime_check(before: &[PairSense], after: &[PairSense]) -> Result<(), String> {
    if before.is_empty() {
        return Err("no_pair".into());
    }
    for p in before {
        match after.iter().find(|q| q.agent == p.agent) {
            None => return Err("rebuilt".into()),
            Some(q) if q.word != PRIMED => return Err(format!("reset (word {})", q.word as i32)),
            Some(_) => {}
        }
    }
    if let Some(q) = after
        .iter()
        .find(|q| q.word != PRIMED && !before.iter().any(|p| p.agent == q.agent))
    {
        return Err(format!("new_pair (word {})", q.word as i32));
    }
    Ok(())
}

/// Why a fall row's pairs aren't all -3, if they aren't.
fn slipping(pairs: &[PairSense]) -> Option<String> {
    if pairs.is_empty() {
        return Some("no_pair".into());
    }
    pairs
        .iter()
        .find(|p| p.word != PRIMED)
        .map(|p| format!("word {}", p.word as i32))
}

fn span_word(pairs: &[PairSense]) -> Option<i32> {
    if pairs.is_empty() {
        return None;
    }
    Some(
        pairs
            .iter()
            .find(|p| p.word != PRIMED)
            .map_or(PRIMED as i32, |p| p.word as i32),
    )
}

fn len3(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// The teleport onto the rest point: asked, rows waited.
    Place(bool, u32),
    /// Rows settled, still rows in a row.
    Settle(u32, u32),
    /// The lift asked at the arm row; rows waited.
    Lift(u32),
    Fall(u32),
    /// Rows after the touchdown row.
    After(u32),
    Done,
}

/// The fall row before, after its write.
#[derive(Clone, Debug, PartialEq)]
struct Prev {
    n: u32,
    tick: u64,
    pos: [f32; 3],
    vel: [f32; 3],
    pairs: Vec<PairSense>,
}

/// One drop, a row a tick: place, settle, arm, lift, fall, watch.
#[derive(Clone, Debug)]
pub struct DropRun {
    pub drop: Drop,
    pub seat: u8,
    target: Target,
    settle_ticks: u32,
    fall_ticks: u32,
    after_ticks: u32,
    bounce_vz: f32,
    walk: [f32; 2],
    facing: [f32; 2],
    phase: Phase,
    first_tick: Option<u64>,
    release: Option<u64>,
    start_z: Option<f32>,
    falling: bool,
    prev: Option<Prev>,
    touch: Option<Touchdown>,
    touch_n: u32,
    prime: Option<Result<(), String>>,
    launch_vz: f32,
    low: f32,
    apex: f32,
    last: (f32, f32),
    apex_reached: bool,
    spans: Vec<WordSpan>,
    rows: Vec<Row>,
    first: Firsts,
    failed: Option<String>,
}

impl DropRun {
    pub fn new(drop: Drop, target: &Target, s: &Settings, seat: u8) -> DropRun {
        let push = side_push(drop.side, target.normal, s.stick);
        let facing = unit2(push).unwrap_or(target.normal);
        DropRun {
            drop,
            seat,
            target: target.clone(),
            settle_ticks: s.settle_ticks,
            fall_ticks: s.fall_ticks,
            after_ticks: s.after_ticks,
            bounce_vz: s.bounce_vz,
            walk: push,
            facing,
            phase: Phase::Place(false, 0),
            first_tick: None,
            release: None,
            start_z: None,
            falling: false,
            prev: None,
            touch: None,
            touch_n: 0,
            prime: None,
            launch_vz: 0.0,
            low: 0.0,
            apex: 0.0,
            last: (0.0, 0.0),
            apex_reached: false,
            spans: Vec::new(),
            rows: Vec::new(),
            first: Firsts::default(),
            failed: None,
        }
    }

    pub fn done(&self) -> bool {
        self.phase == Phase::Done
    }

    /// The phase's name.
    pub fn phase(&self) -> &'static str {
        match self.phase {
            Phase::Place(..) => "place",
            Phase::Settle(..) => "settle",
            Phase::Lift(_) => "lift",
            Phase::Fall(_) => "fall",
            Phase::After(_) => "after",
            Phase::Done => "done",
        }
    }

    fn fail(&mut self, why: impl Into<String>) {
        self.failed.get_or_insert_with(|| why.into());
        self.phase = Phase::Done;
    }

    /// The contacts after the touchdown tick.
    pub fn set_contacts(&mut self, mut points: Vec<ContactPoint>) {
        points.truncate(8);
        if let Some(t) = &mut self.touch {
            t.contacts = points;
        }
    }

    /// One row.
    pub fn step(&mut self, tick: u64, sense: &Sense) -> Act {
        self.first_tick.get_or_insert(tick);
        let mut act = Act::default();
        match self.phase {
            Phase::Done => {}
            Phase::Place(false, _) => {
                act.teleport = Some((self.drop.rest, self.facing));
                self.phase = Phase::Place(true, 0);
            }
            Phase::Place(true, waited) => match &sense.teleported {
                Some(Err(e)) => self.fail(format!("teleport: {e}")),
                Some(Ok(())) => {
                    self.phase = Phase::Settle(0, 0);
                    self.settle(sense, &mut act);
                }
                None if waited + 1 >= TELEPORT_WAIT => self.fail("teleport: never ran"),
                None => self.phase = Phase::Place(true, waited + 1),
            },
            Phase::Settle(..) => self.settle(sense, &mut act),
            Phase::Lift(waited) => match &sense.teleported {
                Some(Err(e)) => self.fail(format!("lift: {e}")),
                Some(Ok(())) => {
                    self.phase = Phase::Fall(0);
                    self.fall(tick, sense, &mut act);
                }
                None if waited + 1 >= TELEPORT_WAIT => self.fail("lift: never ran"),
                None => {
                    // The word again while the lift is waited for.
                    act.word = Some(PRIMED);
                    self.phase = Phase::Lift(waited + 1);
                }
            },
            Phase::Fall(_) => self.fall(tick, sense, &mut act),
            Phase::After(_) => self.after(sense),
        }
        act
    }

    fn row(&self, t: i32, phase: &str, sense: &Sense, wrote: Option<u32>) -> Row {
        Row {
            t,
            phase: phase.into(),
            pos: sense.pos,
            vz: sense.vel.map(|v| v[2]),
            airborne: sense.airborne,
            asleep: sense.asleep,
            pairs: sense.pairs.clone(),
            wrote: wrote.map(|w| w as i32),
        }
    }

    fn span(&mut self, pairs: &[PairSense]) {
        let word = span_word(pairs);
        let n = pairs.len().min(255) as u8;
        match self.spans.last_mut() {
            Some(s) if s.word == word && s.pairs == n => s.ticks += 1,
            _ => self.spans.push(WordSpan {
                word,
                pairs: n,
                ticks: 1,
            }),
        }
    }

    fn settle(&mut self, sense: &Sense, act: &mut Act) {
        let Phase::Settle(n, still) = self.phase else {
            return;
        };
        if sense.pos.is_none() {
            return self.fail("died");
        }
        let speed = sense.vel.map_or(0.0, len3);
        // Cleared: a teleport keeps the word, and a primed body landing
        // on the seam here would bounce.
        act.word = Some(0);
        act.stop = n == 0 || speed > STOP_SPEED;
        let still = if !sense.airborne && speed < STILL_SPEED {
            still + 1
        } else {
            0
        };
        if still >= SETTLE_STILL {
            // Armed on the still body, then lifted straight up: the lift
            // keeps the word and the (zero) velocity.
            act.word = Some(PRIMED);
            act.stop = false;
            act.teleport = Some((self.drop.start, self.facing));
            self.rows.push(self.row(-1, "arm", sense, Some(PRIMED)));
            self.phase = Phase::Lift(0);
        } else if n + 1 >= self.settle_ticks {
            self.fail(format!("did not settle in {} ticks", self.settle_ticks));
        } else {
            self.phase = Phase::Settle(n + 1, still);
        }
    }

    fn fall(&mut self, tick: u64, sense: &Sense, act: &mut Act) {
        let Phase::Fall(n) = self.phase else {
            return;
        };
        let (Some(pos), Some(vel)) = (sense.pos, sense.vel) else {
            return self.fail("died");
        };
        if n == 0 {
            self.start_z = Some(pos[2]);
            self.release = Some(tick);
        }
        let vz = vel[2];
        self.span(&sense.pairs);
        if self.falling && vz > LANDED_VZ {
            // The capsule met the floor in the tick between the row
            // before and this one.
            if let Some(prev) = self.prev.clone() {
                self.prime = Some(prime_check(&prev.pairs, &sense.pairs));
                let p2 = [prev.pos[0], prev.pos[1]];
                self.touch = Some(Touchdown {
                    t: prev.n,
                    tick: prev.tick,
                    pos: prev.pos,
                    vel: prev.vel,
                    speed: -prev.vel[2],
                    after_pos: pos,
                    after_vel: vel,
                    across: self.target.across_of(p2),
                    miss: self.target.miss(p2),
                    contacts: Vec::new(),
                });
                self.first.touchdown = Some(prev.n);
                self.low = prev.pos[2].min(pos[2]);
            } else {
                self.low = pos[2];
            }
            self.touch_n = n;
            self.launch_vz = vz;
            self.apex = pos[2];
            self.last = (pos[2], vz);
            if vz >= self.bounce_vz {
                self.first.launch = Some(n);
            }
            act.contacts = true;
            self.rows.push(self.row(n as i32, "touchdown", sense, None));
            self.phase = Phase::After(0);
            return;
        }
        if vz < TOUCHDOWN_VZ {
            self.falling = true;
        }
        if n > 0
            && self.first.lost.is_none()
            && let Some(why) = slipping(&sense.pairs)
        {
            self.first.lost = Some(n);
            self.first.lost_why = Some(why);
        }
        act.word = Some(PRIMED);
        act.walk = self.walk;
        self.rows.push(self.row(n as i32, "fall", sense, Some(PRIMED)));
        self.prev = Some(Prev {
            n,
            tick,
            pos,
            vel,
            pairs: sense.pairs.clone(),
        });
        if n + 1 >= self.fall_ticks {
            self.fail(format!("no touchdown in {} ticks", self.fall_ticks));
        } else {
            self.phase = Phase::Fall(n + 1);
        }
    }

    fn after(&mut self, sense: &Sense) {
        let Phase::After(n) = self.phase else {
            return;
        };
        let (Some(pos), Some(vel)) = (sense.pos, sense.vel) else {
            // Gone after the touchdown: what was seen stands.
            self.phase = Phase::Done;
            return;
        };
        let vz = vel[2];
        let t = self.touch_n + 1 + n;
        if n < 3 {
            self.launch_vz = self.launch_vz.max(vz);
            self.low = self.low.min(pos[2]);
        }
        if vz >= self.bounce_vz && self.first.launch.is_none() {
            self.first.launch = Some(t);
        }
        self.apex = self.apex.max(pos[2]);
        self.last = (pos[2], vz);
        self.rows.push(self.row(t as i32, "after", sense, None));
        if n + 1 >= 3 && vz <= 0.0 {
            self.apex_reached = true;
            self.phase = Phase::Done;
        } else if n + 1 >= self.after_ticks.max(3) {
            self.phase = Phase::Done;
        } else {
            self.phase = Phase::After(n + 1);
        }
    }

    /// The outcome so far (final once [`DropRun::done`]).
    pub fn outcome(&self) -> (Outcome, Option<String>) {
        if let Some(why) = &self.failed {
            return (Outcome::SetupFailed, Some(why.clone()));
        }
        if self.touch.is_none() {
            return (Outcome::SetupFailed, Some("no touchdown".into()));
        }
        if self.launch_vz >= self.bounce_vz {
            return (Outcome::Bounced, None);
        }
        match &self.prime {
            Some(Err(why)) => (Outcome::PrimeLost, Some(why.clone())),
            _ => (Outcome::NoBounce, None),
        }
    }

    /// The drop's record; `rows` keeps its traced rows.
    pub fn record(&self, rows: bool) -> Record {
        let (outcome, why) = self.outcome();
        let launch = self.touch.as_ref().map(|_| {
            let (z, vz) = self.last;
            let (height, estimated) = if self.apex_reached || vz <= 0.0 {
                (self.apex - self.low, false)
            } else {
                (z - self.low + vz * vz / (2.0 * GRAVITY), true)
            };
            Launch {
                speed: self.launch_vz,
                height: height.max(0.0),
                estimated,
            }
        });
        Record {
            drop: self.drop.id,
            target: self.target.id.clone(),
            shape: self.target.shape.clone(),
            control: self.drop.control,
            point: self.drop.point,
            height: self.drop.height,
            across: self.drop.across,
            along: self.drop.along,
            side: self.drop.side,
            repeat: self.drop.repeat,
            seat: self.seat,
            attempts: 1,
            tick: self.release.or(self.first_tick).unwrap_or(0),
            outcome,
            why,
            start_z: self.start_z,
            touchdown: self.touch.clone(),
            launch,
            prime_held: self.prime.as_ref().map(|p| p.is_ok()),
            words: self.spans.clone(),
            first: self.first.clone(),
            rows: if rows { self.rows.clone() } else { Vec::new() },
        }
    }
}

/// The walk an approach from `side` holds: across the edge at `stick`.
fn side_push(side: Side, normal: [f32; 2], stick: f32) -> [f32; 2] {
    let k = side.sign() * stick;
    [normal[0] * k, normal[1] * k]
}

// ---- the control gate ----

/// Where the control stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Gate {
    /// No control: the sweep runs at once.
    Off,
    /// Control drops still to run: nothing else runs.
    Running,
    /// A control drop bounced.
    Passed,
    /// None did: the run stops with this error.
    Failed(String),
}

impl Gate {
    pub fn name(&self) -> &'static str {
        match self {
            Gate::Off => "off",
            Gate::Running => "running",
            Gate::Passed => "passed",
            Gate::Failed(_) => "failed",
        }
    }
}

/// The gate after the control's finished drops `done` (each its last
/// try's outcome and why), with more to run when `pending`. One bounce
/// passes it; with none to run and no bounce it fails, naming each
/// outcome: `control did not bounce: prime_lost (rebuilt), no_bounce`.
pub fn gate(done: &[(Outcome, Option<String>)], pending: bool) -> Gate {
    if done.iter().any(|(o, _)| *o == Outcome::Bounced) {
        return Gate::Passed;
    }
    if pending {
        return Gate::Running;
    }
    if done.is_empty() {
        return Gate::Failed("control did not bounce: no control drop ran".into());
    }
    let mut seen: Vec<String> = Vec::new();
    for (o, why) in done {
        let s = match why {
            Some(w) => format!("{} ({w})", o.name()),
            None => o.name().to_string(),
        };
        if !seen.contains(&s) {
            seen.push(s);
        }
    }
    Gate::Failed(format!("control did not bounce: {}", seen.join(", ")))
}

// ---- the job ----

/// One seat's sense this tick.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SeatSense {
    pub seat: u8,
    pub sense: Sense,
}

/// One seat's act this tick.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SeatAct {
    pub seat: u8,
    pub act: Act,
}

#[derive(Clone, Debug)]
struct SeatJob {
    seat: u8,
    home: Option<[f32; 3]>,
    last: Option<[f32; 3]>,
    run: Option<DropRun>,
    /// Going home: rows waited for the teleport.
    homing: Option<u32>,
    at_home: bool,
}

/// A run's report block (`h2-sim-drops/1`), in the match report's `drops`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DropsRun {
    pub format: String,
    pub map: String,
    pub shard: u32,
    pub shards: u32,
    pub seats: u32,
    pub planned: u32,
    pub done: u32,
    /// The control's state: `off`, `running`, `passed`, `failed`.
    pub control: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub targets: Vec<Target>,
    pub records: Vec<Record>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// A match's drops: the plan, the queue, the gate and the seats.
#[derive(Clone, Debug)]
pub struct Job {
    map: String,
    shard: u32,
    shards: u32,
    s: Settings,
    targets: Vec<Target>,
    drops: Vec<Drop>,
    queue: Queue,
    control_left: VecDeque<u32>,
    gate: Gate,
    seats: Vec<SeatJob>,
    attempts: Vec<u32>,
    records: Vec<Record>,
    traced: Vec<Outcome>,
    started: Option<u64>,
    stalled: u32,
    notes: Vec<String>,
    finished: bool,
}

impl Job {
    /// The job for `seats` seats on `map`. The targets must be a list:
    /// `h2 sim drops` resolves `"scan"` from the map's `.spots`.
    pub fn new(cfg: &DropsConfig, map: &str, seats: u32) -> Result<Job, String> {
        let s = cfg.settings(map).map_err(|p| p.join("\n"))?;
        let Targets::List(list) = &cfg.targets else {
            return Err("drops: targets = \"scan\" is resolved by `h2 sim drops`, which writes each run's targets; a run on its own needs a list".into());
        };
        let mut targets = Vec::new();
        if let Some(c) = &s.control {
            targets.push(Target::control(c));
        }
        for (i, t) in list.iter().enumerate() {
            targets.push(Target::from_spec(i, t)?);
        }
        let drops = plan(&s, &targets);
        if drops.len() > MAX_DROPS {
            return Err(format!(
                "drops: {} drops in one run, at most {MAX_DROPS}; fewer targets, heights or offsets, or more runs",
                drops.len()
            ));
        }
        let control_left: VecDeque<u32> = drops.iter().filter(|d| d.control).map(|d| d.id).collect();
        let queue = Queue::new(drops.iter().filter(|d| !d.control).map(|d| d.id));
        let gate = if control_left.is_empty() {
            Gate::Off
        } else {
            Gate::Running
        };
        let n = seats.clamp(1, SEATS);
        Ok(Job {
            map: map.to_string(),
            shard: cfg.shard,
            shards: cfg.shards,
            attempts: vec![0; drops.len()],
            s,
            targets,
            drops,
            queue,
            control_left,
            gate,
            seats: (0..n)
                .map(|i| SeatJob {
                    seat: i as u8,
                    home: None,
                    last: None,
                    run: None,
                    homing: None,
                    at_home: true,
                })
                .collect(),
            records: Vec::new(),
            traced: Vec::new(),
            started: None,
            stalled: 0,
            notes: Vec::new(),
            finished: false,
        })
    }

    pub fn finished(&self) -> bool {
        self.finished
    }

    pub fn gate(&self) -> &Gate {
        &self.gate
    }

    /// The named error that stopped the run, if one did.
    pub fn error(&self) -> Option<&str> {
        match &self.gate {
            Gate::Failed(e) => Some(e),
            _ => None,
        }
    }

    pub fn planned(&self) -> usize {
        self.drops.len()
    }

    pub fn records(&self) -> &[Record] {
        &self.records
    }

    pub fn targets(&self) -> &[Target] {
        &self.targets
    }

    pub fn note(&mut self, n: impl Into<String>) {
        let n = n.into();
        if !self.notes.contains(&n) && self.notes.len() < 32 {
            self.notes.push(n);
        }
    }

    /// The contacts the glue read for `seat` at its touchdown (the row
    /// whose act asked for them; the drop runs on after it).
    pub fn contacts(&mut self, seat: u8, points: Vec<ContactPoint>) {
        if let Some(r) = self
            .seats
            .iter_mut()
            .find(|s| s.seat == seat)
            .and_then(|s| s.run.as_mut())
        {
            r.set_contacts(points);
        }
    }

    fn control_active(&self) -> bool {
        self.seats
            .iter()
            .any(|s| s.run.as_ref().is_some_and(|r| r.drop.control))
    }

    /// The busy places for seat `i`: every other seat and its drop.
    fn busy(&self, i: usize) -> Vec<[f32; 2]> {
        let mut v = Vec::new();
        for (j, s) in self.seats.iter().enumerate() {
            if j == i {
                continue;
            }
            if let Some(p) = s.last {
                v.push(xy(p));
            }
            if let Some(r) = &s.run {
                v.push(xy(r.drop.start));
            }
        }
        v
    }

    fn next_drop(&mut self, i: usize) -> Option<u32> {
        match self.gate {
            Gate::Failed(_) => None,
            Gate::Running => {
                if self.control_active() {
                    None
                } else {
                    self.control_left.pop_front()
                }
            }
            Gate::Off | Gate::Passed => {
                let busy = self.busy(i);
                match self.queue.take(&self.drops, &busy, CLEAR) {
                    Some(id) => Some(id),
                    None if self.stalled >= STALL_TICKS => {
                        self.note("the last drops ran with another seat near them");
                        self.queue.take_any()
                    }
                    None => None,
                }
            }
        }
    }

    fn finish_run(&mut self, run: DropRun) {
        let id = run.drop.id as usize;
        self.attempts[id] += 1;
        let attempts = self.attempts[id];
        let (outcome, why) = run.outcome();
        if outcome == Outcome::SetupFailed && attempts <= self.s.retries {
            if run.drop.control {
                self.control_left.push_front(run.drop.id);
            } else {
                self.queue.put_back(run.drop.id);
            }
            return;
        }
        let first_of_kind = !self.traced.contains(&outcome);
        if first_of_kind {
            self.traced.push(outcome);
        }
        let mut rec = run.record(run.drop.control || first_of_kind);
        rec.attempts = attempts;
        if run.drop.control {
            let done: Vec<(Outcome, Option<String>)> = self
                .records
                .iter()
                .filter(|r| r.control)
                .map(|r| (r.outcome, r.why.clone()))
                .chain(std::iter::once((outcome, why)))
                .collect();
            let pending = !self.control_left.is_empty() || self.control_active();
            self.gate = gate(&done, pending);
            if self.gate == Gate::Passed {
                self.control_left.clear();
            }
            if matches!(self.gate, Gate::Failed(_)) {
                self.finished = true;
            }
        }
        self.records.push(rec);
    }

    /// One tick: every seat's sense in, every seat's act out.
    pub fn tick(&mut self, tick: u64, senses: &[SeatSense]) -> Vec<SeatAct> {
        for ss in senses {
            if let Some(s) = self.seats.iter_mut().find(|s| s.seat == ss.seat) {
                s.last = ss.sense.pos;
                if s.home.is_none() {
                    s.home = ss.sense.pos;
                }
            }
        }
        let start = *self.started.get_or_insert(tick);
        if self.finished || tick < start + u64::from(START_TICKS) {
            return senses
                .iter()
                .map(|s| SeatAct {
                    seat: s.seat,
                    act: Act::default(),
                })
                .collect();
        }
        let mut acts = Vec::with_capacity(senses.len());
        for ss in senses {
            let Some(i) = self.seats.iter().position(|s| s.seat == ss.seat) else {
                continue;
            };
            let act = self.step_seat(i, tick, &ss.sense);
            acts.push(SeatAct { seat: ss.seat, act });
        }
        let idle = self.seats.iter().all(|s| s.run.is_none());
        let left = !self.queue.is_empty() || !self.control_left.is_empty();
        self.stalled = if idle && left && self.gate != Gate::Running {
            self.stalled + 1
        } else {
            0
        };
        if idle && !left && self.gate != Gate::Running {
            self.finished = true;
        }
        if idle && self.gate == Gate::Running && self.control_left.is_empty() {
            // Every control drop's record is in: the gate has its verdict.
            let done: Vec<(Outcome, Option<String>)> = self
                .records
                .iter()
                .filter(|r| r.control)
                .map(|r| (r.outcome, r.why.clone()))
                .collect();
            self.gate = gate(&done, false);
            if matches!(self.gate, Gate::Failed(_)) {
                self.finished = true;
            }
        }
        acts
    }

    fn step_seat(&mut self, i: usize, tick: u64, sense: &Sense) -> Act {
        if let Some(run) = self.seats[i].run.as_mut() {
            let act = run.step(tick, sense);
            if run.done()
                && let Some(run) = self.seats[i].run.take()
            {
                self.finish_run(run);
            }
            return act;
        }
        if let Some(waited) = self.seats[i].homing {
            if sense.teleported.is_some() || waited + 1 >= TELEPORT_WAIT {
                self.seats[i].homing = None;
                self.seats[i].at_home = true;
            } else {
                self.seats[i].homing = Some(waited + 1);
            }
            return Act::default();
        }
        if sense.pos.is_none() {
            return Act::default();
        }
        if let Some(id) = self.next_drop(i) {
            let d = self.drops[id as usize];
            let seat = self.seats[i].seat;
            let mut run = DropRun::new(d, &self.targets[d.target as usize], &self.s, seat);
            let act = run.step(tick, sense);
            self.seats[i].run = Some(run);
            self.seats[i].at_home = false;
            return act;
        }
        // Nothing clear to take: off the targets, home to the spawn.
        if !self.seats[i].at_home
            && let Some(home) = self.seats[i].home
        {
            self.seats[i].homing = Some(0);
            return Act {
                teleport: Some((home, [1.0, 0.0])),
                ..Act::default()
            };
        }
        Act::default()
    }

    /// The run's report block.
    pub fn report(&self) -> DropsRun {
        DropsRun {
            format: FORMAT.into(),
            map: self.map.clone(),
            shard: self.shard,
            shards: self.shards,
            seats: self.seats.len() as u32,
            planned: self.drops.len() as u32,
            done: self.records.len() as u32,
            control: self.gate.name().into(),
            error: self.error().map(str::to_string),
            targets: self.targets.clone(),
            records: self.records.clone(),
            notes: self.notes.clone(),
        }
    }

    /// The targets the run proved, for the map's verified file.
    pub fn proven(&self) -> Vec<Proven> {
        proven(&self.targets, &self.records)
    }
}
