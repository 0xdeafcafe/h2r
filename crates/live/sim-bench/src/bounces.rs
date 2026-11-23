//! The forked-trial bounce search (docs/runtime/bounce-search.md): many
//! short superbounce trials from a few saved machine states, to map each
//! seam's bounce region in the inputs of a landing.
//!
//! A seam drop ([`crate::drops`]) spends about 300 ticks to learn one
//! outcome. Here a seat falls onto its site once per Havok variant, the
//! whole machine is saved a few rows before the touchdown (a fork), and
//! every trial starts from that state: the body is put at the trial's
//! position with its velocity, flies a row, touches down and is judged by
//! the drops' own verdict ([`crate::drops::BOUNCE_VZ`],
//! [`crate::drops::LAUNCH_TICKS`]), in about 7 ticks. Then the machine is
//! put back and the next trial runs. Every seat runs its own trial on its
//! own site in the same ticks.
//!
//! Pure, like the rest of the crate: the match's `[bounces]` table
//! ([`BouncesConfig`]), the sites and their batches ([`Site`],
//! [`batches`]), one trial row by row ([`TrialRun`]) and the run's whole
//! job ([`Job`]), which asks the game glue (`sim_bounces` in the game
//! crate) for each seat's [`Act`] and for a fork ([`ForkAsk`]) and is told
//! what came of it ([`ForkDone`]). The samplers are [`sample`]'s, the
//! region and its window [`region`]'s, the pooled report and the ranking
//! [`report`]'s.
//!
//! **The input space.** Seven axes ([`AXES`]), each the state of the last
//! airborne row before the touchdown tick: `across` and `along` the
//! site's edge (world units from its point), `depth` (the origin's height
//! from the site's anchor height, which is where the first variant's own
//! drop was on its last airborne row), `v_drift` and `v_side` (units/s
//! along the site's drift and to its left), `vz` (units/s down) and `yaw`
//! (degrees, the facing from the drift).
//!
//! **A trial.** Row 0 asks the teleport to the start: the last row moved
//! back `flight` rows along the fall ([`back`]). The teleport must put
//! the origin within [`PLACE_TOLERANCE`] of the start; a landing further
//! off is asked again once, aimed against the miss, and the aim that
//! lands is the site's for its next trials. The row the teleport
//! lands writes the start's velocity. Each row after writes the
//! horizontal velocity again, and the row `flight` after the landing the
//! whole planned velocity. The touchdown is the drops': the vertical
//! speed back over [`crate::drops::LANDED_VZ`]; the prime is checked over
//! the touchdown tick ([`crate::drops::prime_check`]); the launch is the
//! best of the touchdown row and the [`crate::drops::LAUNCH_TICKS`] after.

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::drops::{
    self, DropRun, FALL_GRAVITY, LANDED_VZ, LAUNCH_TICKS, Outcome, PRIMED, PairSense, SeatSense,
    Sense, Target, Targets, dot2, prime_check, sub2, unit2, xy,
};

pub mod region;
pub mod report;
pub mod sample;

#[cfg(test)]
mod tests;

pub use region::{Class, Window};
pub use report::{BouncesReport, markdown, proven, summarize, verified_text};
pub use sample::Axis;

/// The run's report block's schema tag.
pub const FORMAT: &str = "h2-sim-bounces/1";
/// The axes of the input space, in a [`Vector`]'s order.
pub const AXES: [&str; N] = ["across", "along", "depth", "v_drift", "v_side", "vz", "yaw"];
/// How many axes.
pub const N: usize = 7;
pub const ACROSS: usize = 0;
pub const ALONG: usize = 1;
pub const DEPTH: usize = 2;
pub const V_DRIFT: usize = 3;
pub const V_SIDE: usize = 4;
pub const VZ: usize = 5;
pub const YAW: usize = 6;
/// One point of the input space.
pub type Vector = [f32; N];

/// The default ranges: `[low, high, step]`. Positions are on a 0.0005
/// grid (1.5 mm; a world unit is 3.048 m). The depth covers a tick of
/// fall at 5.4 units/s (0.18), so every tick phase is in it.
pub const DOMAIN: [[f32; 3]; N] = [
    [-0.10, 0.10, 0.0005],
    [-0.05, 0.05, 0.0005],
    [-0.09, 0.09, 0.0005],
    [0.0, 0.8, 0.0],
    [-0.1, 0.1, 0.0],
    [5.0, 6.6, 0.0],
    [-45.0, 45.0, 0.0],
];
/// The widest a range may be, either way from 0 (`vz`: 0.5 to this).
pub const LIMITS: [f32; N] = [1.0, 1.0, 1.0, 3.0, 3.0, 20.0, 180.0];
/// The human steps: what a player's error in each axis is counted in. A
/// position is 0.01 units (3 cm), a speed 0.1 units/s, the facing 10°.
pub const HUMAN: [f32; N] = [0.01, 0.01, 0.01, 0.1, 0.1, 0.1, 10.0];
/// Defaults.
pub const POINTS: u32 = 400;
pub const ROUNDS: u32 = 3;
pub const REFINE: u32 = 200;
pub const REPLAY: u32 = 8;
pub const UNPRIMED: u32 = 16;
pub const LEAD: u32 = 4;
pub const FLIGHT: u32 = 1;
pub const MIN_SUPPORT: u32 = 8;
pub const SEED: u64 = 1;
/// The most points a site samples in its first round, refinement rounds
/// and points a round.
pub const MAX_POINTS: u32 = 20_000;
pub const MAX_ROUNDS: u32 = 12;
/// The most Havok variants: one saved machine each.
pub const MAX_VARIANTS: usize = 8;
/// A base drop's fall is at least this many rows longer than the lead.
pub const MIN_FALL_ROWS: u32 = 4;
/// Rows a trial waits for its teleport.
pub const TELEPORT_WAIT: u32 = 4;
/// A trial's teleport must put the origin this near its start. A landing
/// further off is asked again once, aimed against the miss
/// ([`TrialRun::aimed`]).
pub const PLACE_TOLERANCE: f32 = 0.005;
/// Rows past the planned last row a trial may still fall.
pub const EXTRA_ROWS: u32 = 6;
/// Hooks a fork is waited for before the run stops.
pub const FORK_WAIT: u32 = 3;
/// How often a busy restore is asked again.
pub const RESTORE_TRIES: u32 = 30;
/// Refinement leaves a pair of opposite outcomes alone once they are
/// this near, in human steps (0.05: half a millimetre of position).
pub const MIN_GAP: f32 = 0.05;

fn one() -> u32 {
    1
}

fn is_false(b: &bool) -> bool {
    !*b
}

// ---- the table ----

/// A match's `[bounces]` table (docs/runtime/bounce-search.md, "The
/// table"). Plain values first, tables last: TOML writes them in that
/// order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BouncesConfig {
    /// The map, when the match file leaves `map` out (the same if both).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<String>,
    /// `h2 sim bounces`'s results directory, relative to the file
    /// (default `results/<file stem>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out: Option<PathBuf>,
    /// Seats when the file has no `[[bot]]` (default 15, the most).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seats: Option<u32>,
    /// Sites along each edge (default 1, its middle).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub along: Option<u32>,
    /// `targets = "scan"`: at most this many of the scan's, in its order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_targets: Option<u32>,
    /// Points a site samples first (default 400).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub points: Option<u32>,
    /// Refinement rounds after them (default 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rounds: Option<u32>,
    /// Points a refinement round adds at most (default 200).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refine: Option<u32>,
    /// Points run a second time on each variant (default 8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay: Option<u32>,
    /// Points run once more with the word 0 (default 16).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unprimed: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// Rows before the base drop's touchdown tick the machine is saved
    /// (default 4, at least 2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead: Option<u32>,
    /// Rows a trial flies between its start and its last airborne row
    /// (default 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flight: Option<u32>,
    /// The launch (units/s up) that counts as a bounce (default 3.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounce_vz: Option<f32>,
    /// Setup failures of a base drop retried this many times (default 2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retries: Option<u32>,
    /// Points a window holds at least (default 8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_support: Option<u32>,
    /// `"off"` (default), `"on"` or `"both"`: crouch held through a trial.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crouch: Option<String>,
    /// `"lhs"` (default) or `"grid"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<String>,
    /// `"first"` (default), `"all"` or `"none"`: which batches replay a
    /// whole wave and compare guest RAM.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify: Option<String>,
    /// This run's share of a `h2 sim bounces` batch (set by the batch).
    #[serde(default)]
    pub shard: u32,
    #[serde(default = "one")]
    pub shards: u32,
    /// Each axis's range, `[low, high]` or `[low, high, step]`, by its
    /// name ([`AXES`]); `[v, v]` holds it at `v`. Default [`DOMAIN`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub axes: BTreeMap<String, Vec<f32>>,
    /// `sample = "grid"`: values on each axis (default 1, its middle).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub grid: BTreeMap<String, u32>,
    /// The human step of each axis (default [`HUMAN`]).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub human: BTreeMap<String, f32>,
    /// The Havok variants: `[[bounces.variant]]` (default
    /// [`Variant::defaults`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variant: Vec<Variant>,
    /// `"scan"`, every bounce candidate of the map's `.spots`, or a list.
    #[serde(default)]
    pub targets: Targets,
}

impl Default for BouncesConfig {
    fn default() -> BouncesConfig {
        BouncesConfig {
            map: None,
            out: None,
            seats: None,
            along: None,
            max_targets: None,
            points: None,
            rounds: None,
            refine: None,
            replay: None,
            unprimed: None,
            seed: None,
            lead: None,
            flight: None,
            bounce_vz: None,
            retries: None,
            min_support: None,
            crouch: None,
            sample: None,
            verify: None,
            shard: 0,
            shards: 1,
            axes: BTreeMap::new(),
            grid: BTreeMap::new(),
            human: BTreeMap::new(),
            variant: Vec::new(),
            targets: Targets::default(),
        }
    }
}

/// One Havok variant: the base drop whose state the trials start from. A
/// different drop leaves the body's pairs, contact caches and sleep state
/// different a few rows before the touchdown.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Variant {
    pub name: String,
    /// The base drop's height over the site's floor.
    pub height: f32,
    /// Its speed along the site's drift (default 0, plumb). A site with
    /// no drift carries along its across direction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub carry_along: Option<f32>,
    /// Rows the settled body rests before the arm (default 0). Over 38
    /// the body sleeps first, and its pair is rebuilt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rest: Option<u32>,
}

impl Variant {
    /// A long fall, a short hop from the seam's own floor height, and a
    /// long fall by a body that slept first.
    pub fn defaults() -> Vec<Variant> {
        vec![
            Variant {
                name: "fall".into(),
                height: 3.65,
                carry_along: None,
                rest: None,
            },
            Variant {
                name: "hop".into(),
                height: 1.0,
                carry_along: None,
                rest: None,
            },
            Variant {
                name: "slept".into(),
                height: 3.65,
                carry_along: None,
                rest: Some(60),
            },
        ]
    }
}

/// Whether trials hold crouch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Crouch {
    #[default]
    Off,
    On,
    /// Every other point.
    Both,
}

/// Which batches check the fork by replaying a wave.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verify {
    #[default]
    First,
    All,
    None,
}

/// A table's settings, the defaults filled in.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub seats: u32,
    pub along: u32,
    pub points: u32,
    pub rounds: u32,
    pub refine: u32,
    pub replay: u32,
    pub unprimed: u32,
    pub seed: u64,
    pub lead: u32,
    pub flight: u32,
    pub bounce_vz: f32,
    pub retries: u32,
    pub min_support: u32,
    pub crouch: Crouch,
    /// `sample = "grid"`: the values on each axis.
    pub grid: Option<[u32; N]>,
    pub verify: Verify,
    pub axes: [Axis; N],
    pub human: [f32; N],
    pub variants: Vec<Variant>,
}

fn axis_index(name: &str) -> Option<usize> {
    AXES.iter().position(|a| *a == name)
}

impl BouncesConfig {
    /// The settings, or every problem with them.
    pub fn settings(&self) -> Result<Settings, Vec<String>> {
        let mut p = Vec::new();
        let seats = self.seats.unwrap_or(drops::SEATS);
        if seats == 0 || seats > drops::SEATS {
            p.push(format!("bounces: seats is 1 to {}", drops::SEATS));
        }
        let along = self.along.unwrap_or(1);
        if along == 0 || along > 64 {
            p.push("bounces: along is 1 to 64 sites an edge".into());
        }
        let points = self.points.unwrap_or(POINTS);
        if points == 0 || points > MAX_POINTS {
            p.push(format!("bounces: points is 1 to {MAX_POINTS}"));
        }
        let rounds = self.rounds.unwrap_or(ROUNDS);
        if rounds > MAX_ROUNDS {
            p.push(format!("bounces: rounds is at most {MAX_ROUNDS}"));
        }
        let refine = self.refine.unwrap_or(REFINE);
        if refine > MAX_POINTS {
            p.push(format!("bounces: refine is at most {MAX_POINTS}"));
        }
        let lead = self.lead.unwrap_or(LEAD);
        if !(2..=12).contains(&lead) {
            p.push("bounces: lead is 2 to 12 rows".into());
        }
        let flight = self.flight.unwrap_or(FLIGHT);
        if flight > 6 {
            p.push("bounces: flight is at most 6 rows".into());
        }
        let bounce_vz = self.bounce_vz.unwrap_or(drops::BOUNCE_VZ);
        if !(bounce_vz.is_finite() && bounce_vz > 0.5) {
            p.push("bounces: bounce_vz is over 0.5 units/s".into());
        }
        let retries = self.retries.unwrap_or(drops::RETRIES);
        if retries > 10 {
            p.push("bounces: retries is at most 10".into());
        }
        let crouch = match self.crouch.as_deref() {
            None | Some("off") => Crouch::Off,
            Some("on") => Crouch::On,
            Some("both") => Crouch::Both,
            Some(o) => {
                p.push(format!("bounces: crouch = \"{o}\"; off, on or both"));
                Crouch::Off
            }
        };
        let verify = match self.verify.as_deref() {
            None | Some("first") => Verify::First,
            Some("all") => Verify::All,
            Some("none") => Verify::None,
            Some(o) => {
                p.push(format!("bounces: verify = \"{o}\"; first, all or none"));
                Verify::First
            }
        };
        let mut axes = [Axis::default(); N];
        for (a, d) in DOMAIN.iter().enumerate() {
            axes[a] = Axis {
                lo: d[0],
                hi: d[1],
                step: d[2],
            };
        }
        for (name, v) in &self.axes {
            let Some(a) = axis_index(name) else {
                p.push(format!("bounces.axes: {name} is not an axis ({})", AXES.join(", ")));
                continue;
            };
            let ok = (v.len() == 2 || v.len() == 3)
                && v.iter().all(|x| x.is_finite())
                && v[0] <= v[1]
                && v.get(2).is_none_or(|s| *s >= 0.0);
            if !ok {
                p.push(format!("bounces.axes: {name} is [low, high] or [low, high, step]"));
                continue;
            }
            let floor = if a == VZ { 0.5 } else { -LIMITS[a] };
            if v[0] < floor || v[1] > LIMITS[a] {
                p.push(format!(
                    "bounces.axes: {name} is inside {floor}..{}",
                    LIMITS[a]
                ));
                continue;
            }
            axes[a] = Axis {
                lo: v[0],
                hi: v[1],
                step: v.get(2).copied().unwrap_or(0.0),
            };
        }
        let mut human = HUMAN;
        for (name, v) in &self.human {
            match axis_index(name) {
                Some(a) if v.is_finite() && *v > 0.0 => human[a] = *v,
                Some(_) => p.push(format!("bounces.human: {name} is over 0")),
                None => p.push(format!(
                    "bounces.human: {name} is not an axis ({})",
                    AXES.join(", ")
                )),
            }
        }
        let grid = match self.sample.as_deref() {
            None | Some("lhs") => {
                if !self.grid.is_empty() {
                    p.push("bounces.grid: only with sample = \"grid\"".into());
                }
                None
            }
            Some("grid") => {
                let mut counts = [1u32; N];
                for (name, c) in &self.grid {
                    match axis_index(name) {
                        Some(a) if (1..=1000).contains(c) => counts[a] = *c,
                        Some(_) => p.push(format!("bounces.grid: {name} is 1 to 1000 values")),
                        None => p.push(format!(
                            "bounces.grid: {name} is not an axis ({})",
                            AXES.join(", ")
                        )),
                    }
                }
                if let Err(e) = sample::grid(&axes, &counts) {
                    p.push(format!("bounces.{e}"));
                }
                Some(counts)
            }
            Some(o) => {
                p.push(format!("bounces: sample = \"{o}\"; lhs or grid"));
                None
            }
        };
        let variants = if self.variant.is_empty() {
            Variant::defaults()
        } else {
            self.variant.clone()
        };
        if variants.len() > MAX_VARIANTS {
            p.push(format!("bounces: at most {MAX_VARIANTS} variants"));
        }
        for (i, v) in variants.iter().enumerate() {
            if v.name.trim().is_empty() || variants[..i].iter().any(|o| o.name == v.name) {
                p.push(format!("bounces.variant {i}: a name of its own"));
            }
            if !(v.height.is_finite() && (0.2..=20.0).contains(&v.height)) {
                p.push(format!("bounces.variant {}: height is 0.2 to 20", v.name));
            }
            if v
                .carry_along
                .is_some_and(|c| !(c.is_finite() && c.abs() <= drops::MAX_CARRY))
            {
                p.push(format!(
                    "bounces.variant {}: carry_along is at most {} units/s either way",
                    v.name,
                    drops::MAX_CARRY
                ));
            }
            if v.rest.is_some_and(|r| r > 600) {
                p.push(format!("bounces.variant {}: rest is at most 600 rows", v.name));
            }
        }
        if !p.is_empty() {
            return Err(p);
        }
        Ok(Settings {
            seats,
            along,
            points,
            rounds,
            refine,
            replay: self.replay.unwrap_or(REPLAY).min(points),
            unprimed: self.unprimed.unwrap_or(UNPRIMED).min(points),
            seed: self.seed.unwrap_or(SEED),
            lead,
            flight,
            bounce_vz,
            retries,
            min_support: self.min_support.unwrap_or(MIN_SUPPORT).max(1),
            crouch,
            grid,
            verify,
            axes,
            human,
            variants,
        })
    }

    /// Every problem with the table in a match on `map`.
    pub fn problems(
        &self,
        map: &str,
        superbounce: bool,
        drill: bool,
        train: bool,
        drops: bool,
    ) -> Vec<String> {
        let mut p = self.settings().err().unwrap_or_default();
        if superbounce {
            p.push("bounces: leave superbounce out; the search drives every seat".into());
        }
        if drill || train || drops {
            p.push("bounces: a [drill], a [train] or a [drops] is another run; use one".into());
        }
        if let Some(m) = &self.map
            && !map.is_empty()
            && !m.eq_ignore_ascii_case(map)
        {
            p.push(format!("bounces: map = \"{m}\" but the match's map is \"{map}\""));
        }
        p
    }

    /// The seats a file with no `[[bot]]` gets.
    pub fn seat_bots(&self) -> Vec<crate::config::BotSpec> {
        let n = self.seats.unwrap_or(drops::SEATS).min(drops::SEATS);
        (0..n)
            .map(|i| crate::config::BotSpec {
                name: Some(format!("Bouncer {}", i + 1)),
                ..crate::config::BotSpec::default()
            })
            .collect()
    }
}

impl Settings {
    /// The points a site samples first.
    pub fn first_points(&self) -> usize {
        match &self.grid {
            Some(counts) => sample::grid(&self.axes, counts).map_or(0, |g| g.len()),
            None => self.points as usize,
        }
    }

    /// The most trials one site runs: its points and the refinement's on
    /// every variant, the replays, the anchors and the unprimed points.
    pub fn trials_per_site(&self) -> usize {
        let points = self.first_points();
        let v = self.variants.len();
        v * (points + (self.rounds * self.refine) as usize + self.replay as usize + 2)
            + self.unprimed as usize
    }

    /// The drops' settings a base drop runs under.
    fn drops(&self) -> drops::Settings {
        drops::Settings {
            seats: self.seats,
            heights: Vec::new(),
            across: vec![0.0],
            carries: vec![[0.0, 0.0]],
            carry_along: None,
            along: 1,
            repeats: 1,
            sides: vec![drops::Side::None],
            stick: 0.0,
            lead: 0.0,
            bounce_vz: self.bounce_vz,
            settle_ticks: drops::SETTLE_TICKS,
            fall_ticks: drops::FALL_TICKS,
            after_ticks: drops::AFTER_TICKS,
            retries: self.retries,
            control: None,
        }
    }
}

// ---- sites ----

/// A site: one point of a target's edge, and the frame its inputs are in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Site {
    pub id: String,
    pub target: Target,
    /// 0..1 along the edge.
    pub along: f32,
    /// The point on the edge, its floor.
    pub point: [f32; 3],
    /// The unit direction along the edge (a point target: the across
    /// direction's left).
    pub edge: [f32; 2],
    /// The unit drift (a target with none: its across direction).
    pub drift: [f32; 2],
}

/// A body's state on one row: where its origin is, its velocity and the
/// way it faces (X and Y).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Placed {
    pub pos: [f32; 3],
    pub vel: [f32; 3],
    pub facing: [f32; 2],
}

impl Site {
    /// The sites of `targets`: `along` points an edge (one: its middle),
    /// one for a point target.
    pub fn of(targets: &[Target], along: u32) -> Vec<Site> {
        let mut out = Vec::new();
        for t in targets {
            let alongs = t.alongs(along);
            let many = alongs.len() > 1;
            let normal = t.normal;
            let edge = unit2(sub2(xy(t.edge[1]), xy(t.edge[0]))).unwrap_or([-normal[1], normal[0]]);
            for a in alongs {
                out.push(Site {
                    id: if many {
                        format!("{}@{a:.2}", t.id)
                    } else {
                        t.id.clone()
                    },
                    target: t.clone(),
                    along: a,
                    point: t.at(a),
                    edge,
                    drift: t.drift.unwrap_or(normal),
                });
            }
        }
        out
    }

    /// The last airborne row `v` asks for, with the anchor height
    /// `anchor` over the site's floor.
    pub fn place(&self, anchor: f32, v: &Vector) -> Placed {
        let n = self.target.normal;
        let (e, d) = (self.edge, self.drift);
        let left = [-d[1], d[0]];
        let yaw = d[1].atan2(d[0]) + v[YAW].to_radians();
        Placed {
            pos: [
                self.point[0] + n[0] * v[ACROSS] + e[0] * v[ALONG],
                self.point[1] + n[1] * v[ACROSS] + e[1] * v[ALONG],
                self.point[2] + anchor + v[DEPTH],
            ],
            vel: [
                d[0] * v[V_DRIFT] + left[0] * v[V_SIDE],
                d[1] * v[V_DRIFT] + left[1] * v[V_SIDE],
                -v[VZ],
            ],
            facing: [yaw.cos(), yaw.sin()],
        }
    }

    /// What a body at `pos` falling at `vel` is on the measured axes:
    /// `[across, along, depth, vz]`.
    pub fn measured(&self, anchor: f32, pos: [f32; 3], vel: [f32; 3]) -> [f32; 4] {
        let rel = sub2(xy(pos), xy(self.point));
        [
            dot2(rel, self.target.normal),
            dot2(rel, self.edge),
            pos[2] - self.point[2] - anchor,
            -vel[2],
        ]
    }
}

/// `last` moved back `rows` rows along a free fall: each row the speed
/// down is [`FALL_GRAVITY`]/30 less and the body is where that row's tick
/// took it from (the drops' fall model: the tick adds the gravity, then
/// moves by the new speed). The horizontal velocity is kept.
pub fn back(last: &Placed, rows: u32) -> Placed {
    let mut p = *last;
    for _ in 0..rows {
        p.pos[0] -= p.vel[0] / 30.0;
        p.pos[1] -= p.vel[1] / 30.0;
        p.pos[2] -= p.vel[2] / 30.0;
        p.vel[2] += FALL_GRAVITY / 30.0;
    }
    p
}

/// Splits sites (their points, X and Y) into batches of at most `seats`
/// whose members are `clear` apart, so their bodies never meet: each
/// batch takes, in order, every site clear of the ones it already has.
pub fn batches(points: &[[f32; 2]], seats: usize, clear: f32) -> Vec<Vec<usize>> {
    let mut left: Vec<usize> = (0..points.len()).collect();
    let mut out = Vec::new();
    while !left.is_empty() {
        let mut batch: Vec<usize> = Vec::new();
        left.retain(|&i| {
            let fits = batch.len() < seats.max(1)
                && batch.iter().all(|&j| {
                    (points[i][0] - points[j][0]).hypot(points[i][1] - points[j][1]) >= clear
                });
            if fits {
                batch.push(i);
            }
            !fits
        });
        out.push(batch);
    }
    out
}

// ---- one trial ----

/// What the glue does for a seat this tick, before the tick runs: the
/// drops' act, and the bounce search's two more.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Act {
    pub base: drops::Act,
    /// Set the body's whole velocity (the object's and each Havok
    /// motion's).
    pub velocity: Option<[f32; 3]>,
    /// Hold crouch on the pad.
    pub crouch: bool,
}

/// One seat's act.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SeatAct {
    pub seat: u8,
    pub act: Act,
}

/// A trial as planned.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Plan {
    /// Where the teleport puts the body, and the velocity written there.
    pub start: Placed,
    /// The last airborne row asked for, `flight` rows later.
    pub last: Placed,
    pub flight: u32,
    /// The word held at -3 (else 0, the unprimed control).
    pub primed: bool,
    pub crouch: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum TrialPhase {
    Ask,
    Wait(u32),
    /// Rows since the teleport landed.
    Flight(u32),
    /// Rows after the touchdown row.
    After(u32),
    Done,
}

#[derive(Clone, Debug, PartialEq)]
struct Prev {
    pos: [f32; 3],
    vel: [f32; 3],
    pairs: Vec<PairSense>,
}

/// What came of a trial.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrialResult {
    pub outcome: Outcome,
    pub why: Option<String>,
    /// The best vertical speed of the touchdown row and the three after.
    pub launch: f32,
    /// The last airborne row: the origin, and the velocity its tick ran
    /// from.
    pub last: Option<([f32; 3], [f32; 3])>,
    /// Rows from the teleport's landing to the last airborne row.
    pub rows: u32,
    /// The aim (asked for minus the start) of the teleport that landed
    /// on the start, if one did.
    pub aim: Option<[f32; 3]>,
}

/// A teleport's miss, landed minus wanted, as a failure names it:
/// `0.405 off (x +0.001 y -0.002 z +0.405)`.
pub fn miss_text(off: [f32; 3]) -> String {
    format!(
        "{:.3} off (x {:+.3} y {:+.3} z {:+.3})",
        len3(off),
        off[0],
        off[1],
        off[2]
    )
}

fn len3(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

/// One trial, a row a tick: teleport, velocity, fall, touchdown, launch.
#[derive(Clone, Debug)]
pub struct TrialRun {
    plan: Plan,
    bounce_vz: f32,
    phase: TrialPhase,
    prev: Option<Prev>,
    last: Option<([f32; 3], [f32; 3])>,
    rows: u32,
    prime: Option<Result<(), String>>,
    launch: f32,
    failed: Option<String>,
    /// Added to the start in the teleport asked for.
    aim: [f32; 3],
    /// The teleport was asked again against a miss.
    corrected: bool,
    /// The aim that landed on the start.
    landed_aim: Option<[f32; 3]>,
}

impl TrialRun {
    pub fn new(plan: Plan, bounce_vz: f32) -> TrialRun {
        TrialRun {
            plan,
            bounce_vz,
            phase: TrialPhase::Ask,
            prev: None,
            last: None,
            rows: 0,
            prime: None,
            launch: 0.0,
            failed: None,
            aim: [0.0; 3],
            corrected: false,
            landed_aim: None,
        }
    }

    /// Asks the teleport for the start plus `aim`: the aim an earlier
    /// trial on the site landed with ([`TrialResult::aim`]), so a
    /// placement that is off by a steady amount costs no second teleport.
    pub fn aimed(mut self, aim: [f32; 3]) -> TrialRun {
        self.aim = aim;
        self
    }

    /// Where the teleport is asked to put the origin.
    fn asked(&self) -> [f32; 3] {
        let s = self.plan.start.pos;
        [s[0] + self.aim[0], s[1] + self.aim[1], s[2] + self.aim[2]]
    }

    pub fn done(&self) -> bool {
        self.phase == TrialPhase::Done
    }

    /// Its teleport was asked again against a miss.
    pub fn corrected(&self) -> bool {
        self.corrected
    }

    fn fail(&mut self, why: impl Into<String>) {
        self.failed.get_or_insert_with(|| why.into());
        self.phase = TrialPhase::Done;
    }

    /// One row.
    pub fn step(&mut self, sense: &Sense) -> Act {
        let word = if self.plan.primed { PRIMED } else { 0 };
        let mut act = Act {
            crouch: self.plan.crouch,
            ..Act::default()
        };
        match self.phase {
            TrialPhase::Done => {}
            TrialPhase::Ask => {
                act.base.teleport = Some((self.asked(), self.plan.start.facing));
                act.base.word = Some(word);
                self.phase = TrialPhase::Wait(0);
            }
            TrialPhase::Wait(n) => match &sense.teleported {
                Some(Err(e)) => self.fail(format!("teleport: {e}")),
                Some(Ok(())) => {
                    let Some(pos) = sense.pos else {
                        self.fail("died");
                        return act;
                    };
                    let want = self.plan.start.pos;
                    let off = [pos[0] - want[0], pos[1] - want[1], pos[2] - want[2]];
                    if len3(off) > PLACE_TOLERANCE {
                        if self.corrected {
                            self.fail(format!(
                                "teleport: landed {} with its aim corrected",
                                miss_text(off)
                            ));
                            return act;
                        }
                        // Asked again against the miss: a placement off
                        // by a steady amount lands on the start this time.
                        self.corrected = true;
                        for (aim, off) in self.aim.iter_mut().zip(off) {
                            *aim -= off;
                        }
                        act.base.teleport = Some((self.asked(), self.plan.start.facing));
                        act.base.word = Some(word);
                        self.phase = TrialPhase::Wait(0);
                        return act;
                    }
                    self.landed_aim = Some(self.aim);
                    act.velocity = Some(self.plan.start.vel);
                    act.base.word = Some(word);
                    self.prev = Some(Prev {
                        pos,
                        vel: self.plan.start.vel,
                        pairs: sense.pairs.clone(),
                    });
                    self.phase = TrialPhase::Flight(1);
                }
                None if n + 1 >= TELEPORT_WAIT => self.fail("teleport: never ran"),
                None => {
                    act.base.word = Some(word);
                    self.phase = TrialPhase::Wait(n + 1);
                }
            },
            TrialPhase::Flight(k) => {
                let (Some(pos), Some(vel)) = (sense.pos, sense.vel) else {
                    self.fail("died");
                    return act;
                };
                if vel[2] > LANDED_VZ {
                    // The capsule met the floor in the tick between the
                    // row before and this one.
                    if let Some(prev) = self.prev.take() {
                        if self.plan.primed {
                            self.prime = Some(prime_check(&prev.pairs, &sense.pairs));
                        }
                        self.last = Some((prev.pos, prev.vel));
                    }
                    self.rows = k - 1;
                    self.launch = vel[2];
                    self.phase = TrialPhase::After(0);
                    return act;
                }
                let planned = self.plan.last.vel;
                let ran = if k == self.plan.flight {
                    // The planned last row: its whole velocity.
                    act.velocity = Some(planned);
                    planned
                } else {
                    act.base.level = Some([planned[0], planned[1]]);
                    [planned[0], planned[1], vel[2]]
                };
                act.base.word = Some(word);
                self.prev = Some(Prev {
                    pos,
                    vel: ran,
                    pairs: sense.pairs.clone(),
                });
                if k >= self.plan.flight + EXTRA_ROWS {
                    self.fail(format!("no touchdown in {k} rows"));
                } else {
                    self.phase = TrialPhase::Flight(k + 1);
                }
            }
            TrialPhase::After(n) => {
                if let Some(vel) = sense.vel {
                    self.launch = self.launch.max(vel[2]);
                }
                self.phase = if n + 1 >= LAUNCH_TICKS || sense.vel.is_none() {
                    TrialPhase::Done
                } else {
                    TrialPhase::After(n + 1)
                };
            }
        }
        act
    }

    /// The outcome so far (final once [`TrialRun::done`]).
    pub fn result(&self) -> TrialResult {
        let (outcome, why) = if let Some(why) = &self.failed {
            (Outcome::SetupFailed, Some(why.clone()))
        } else if self.last.is_none() {
            (Outcome::SetupFailed, Some("no touchdown".into()))
        } else if self.launch >= self.bounce_vz {
            (Outcome::Bounced, None)
        } else {
            match &self.prime {
                Some(Err(why)) => (Outcome::PrimeLost, Some(why.clone())),
                _ => (Outcome::NoBounce, None),
            }
        };
        TrialResult {
            outcome,
            why,
            launch: self.launch,
            last: self.last,
            rows: self.rows,
            aim: self.landed_aim,
        }
    }
}

// ---- the fork ----

/// What the job asks of the machine, carried out between two slices just
/// after the tick that asked has returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForkAsk {
    /// Save the whole machine as it is, under `key`.
    Capture { key: u64 },
    /// Put the machine back as `key` saved it. `check`: first hash guest
    /// RAM as it is (the wave's end), and after the restore compare RAM
    /// with the hash the capture kept.
    Restore { key: u64, check: bool },
}

/// What came of a [`ForkAsk`], handed to the next tick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForkDone {
    Captured { key: u64 },
    Restored {
        key: u64,
        /// Guest RAM's hash before the restore (`check`).
        end_hash: Option<u64>,
        /// Whether RAM after the restore is the capture's (`check`).
        ram_equal: Option<bool>,
    },
    /// The machine could not be saved or loaded now: something is live.
    /// Nothing changed.
    Busy(String),
    /// The fork failed; after a failed restore the machine is not to run
    /// on.
    Failed(String),
}

/// One tick's answer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Step {
    pub acts: Vec<SeatAct>,
    pub fork: Option<ForkAsk>,
}

/// The forks a run made.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ForkStats {
    pub captures: u32,
    pub restores: u32,
    /// Forks answered busy.
    pub busy: u32,
    /// What the last busy answer named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_busy: Option<String>,
    /// Waves of trials, and the game ticks they ran.
    pub waves: u32,
    pub ticks: u64,
    /// Pairs of identical waves whose end RAM was compared, and how many
    /// differed.
    pub replay_checks: u32,
    pub replay_mismatches: u32,
    /// Restores whose RAM was compared with the capture's, and how many
    /// differed.
    pub ram_checks: u32,
    pub ram_mismatches: u32,
}

impl ForkStats {
    pub fn add(&mut self, o: &ForkStats) {
        self.captures += o.captures;
        self.restores += o.restores;
        self.busy += o.busy;
        if o.last_busy.is_some() {
            self.last_busy.clone_from(&o.last_busy);
        }
        self.waves += o.waves;
        self.ticks += o.ticks;
        self.replay_checks += o.replay_checks;
        self.replay_mismatches += o.replay_mismatches;
        self.ram_checks += o.ram_checks;
        self.ram_mismatches += o.ram_mismatches;
    }
}

// ---- the run's block ----

/// What a trial is for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A sampled point.
    #[default]
    Sample,
    /// The base drop's own last row, put back by the trial's means.
    Anchor,
    /// A sampled point a second time.
    Replay,
    /// A sampled point with the word 0.
    Unprimed,
}

/// One trial's record.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Trial {
    /// Its site's index in the block's `sites`.
    pub site: u32,
    /// Its variant's index in the block's `variants`.
    pub variant: u8,
    pub kind: Kind,
    /// Its point's index in the site's `points` (an anchor has none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub point: Option<u32>,
    pub outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    pub launch: f32,
    /// The last airborne row as it was: `[across, along, depth, vz]`
    /// ([`Site::measured`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<[f32; 4]>,
    /// Its teleport landed off its start and was asked again
    /// ([`TrialRun::corrected`]).
    #[serde(default, skip_serializing_if = "is_false")]
    pub reaimed: bool,
}

/// One sampled point of a site.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Point {
    /// As planned.
    pub x: Vector,
    #[serde(default, skip_serializing_if = "is_false")]
    pub crouch: bool,
    /// 0: the first sample; `n`: refinement round `n`.
    pub round: u8,
}

/// A variant's base drop on a site.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Base {
    /// The machine was saved from this drop: trials ran from it.
    pub based: bool,
    /// Why not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// The full drop that timed it: its outcome, touchdown speed, launch
    /// and the row of its last airborne row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row: Option<u32>,
}

/// One site of a run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SiteRun {
    pub site: Site,
    /// The anchor height: the origin over the site's floor on the last
    /// airborne row of the first variant's own drop that touched down.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<f32>,
    /// One a variant.
    pub bases: Vec<Base>,
    pub points: Vec<Point>,
}

/// A run's block of the sim report.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BouncesRun {
    pub format: String,
    pub map: String,
    pub shard: u32,
    pub shards: u32,
    pub seats: u32,
    /// The most trials planned, and the trials done.
    pub planned: u32,
    pub done: u32,
    /// The named error that stopped the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub axes: Vec<Axis>,
    pub human: Vec<f32>,
    pub variants: Vec<Variant>,
    pub lead: u32,
    pub flight: u32,
    pub bounce_vz: f32,
    pub min_support: u32,
    pub sites: Vec<SiteRun>,
    pub trials: Vec<Trial>,
    pub fork: ForkStats,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

// ---- the job ----

#[derive(Clone, Debug, PartialEq)]
struct Calib {
    /// The fall row of the last airborne row.
    row: u32,
    /// Rows from the arm to the release.
    latency: u32,
    pos: [f32; 3],
    vel: [f32; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Task {
    Anchor,
    Sample(u32),
    Replay(u32),
    Unprimed(u32),
}

struct SiteJob {
    site: Site,
    anchor: Option<f32>,
    bases: Vec<Base>,
    calib: Vec<Option<Calib>>,
    points: Vec<Point>,
    /// `[point][variant]`.
    outcomes: Vec<Vec<Option<Outcome>>>,
    /// One queue a variant.
    queue: Vec<VecDeque<Task>>,
    /// The aim the site's last trial landed its teleport with
    /// ([`TrialResult::aim`]); its next trials ask with it.
    aim: [f32; 3],
}

impl SiteJob {
    fn based(&self, v: usize) -> bool {
        self.bases[v].based
    }

    fn class(&self, p: usize) -> Class {
        region::class(
            (0..self.bases.len())
                .filter(|&v| self.based(v))
                .map(|v| self.outcomes[p][v]),
        )
    }
}

/// A base drop and when it armed and was released.
struct BaseDrop {
    run: DropRun,
    attempt: u32,
    arm: Option<u64>,
    release: Option<u64>,
    /// The lift's teleport as asked, until it lands.
    lift_asked: Option<[f32; 3]>,
    /// Where the lift's teleport put the origin, less where it was asked:
    /// the placement's own offset, in the air as a trial's start is.
    lift_miss: Option<[f32; 3]>,
}

impl BaseDrop {
    fn step(&mut self, tick: u64, sense: &Sense) -> drops::Act {
        if let (Some(asked), Some(Ok(())), Some(pos)) =
            (self.lift_asked, &sense.teleported, sense.pos)
        {
            self.lift_asked = None;
            self.lift_miss = Some([pos[0] - asked[0], pos[1] - asked[1], pos[2] - asked[2]]);
        }
        let act = self.run.step(tick, sense);
        if let Some((to, _)) = act.teleport
            && self.run.phase() == "lift"
        {
            self.lift_asked = Some(to);
        }
        if self.arm.is_none() && self.run.phase() == "lift" {
            self.arm = Some(tick);
        }
        if self.release.is_none() && self.run.phase() == "fall" {
            self.release = Some(tick);
        }
        act
    }
}

enum Phase {
    /// Rows waited after the match's start.
    Start(u32),
    /// Each seat's full drop, to time its fall.
    Calibrate { v: usize, runs: Vec<Option<BaseDrop>> },
    /// Each seat placed and settled again, held; rows each has been
    /// ready.
    Settle {
        v: usize,
        runs: Vec<Option<BaseDrop>>,
        ready: Vec<u32>,
    },
    /// Each seat let go on its own tick, so all are `lead` rows from
    /// their touchdowns at `fork_tick`.
    Launch {
        v: usize,
        runs: Vec<Option<BaseDrop>>,
        arm_at: Vec<u64>,
        fork_tick: u64,
    },
    /// The capture asked; `tries` asked again after a busy answer.
    Capture {
        v: usize,
        runs: Vec<Option<BaseDrop>>,
        tries: u32,
        waited: u32,
    },
    /// Choose the next wave, round or batch.
    Pick,
    /// A restore asked; `ended` is the wave it ends, `(variant, number)`.
    Restore {
        v: usize,
        ended: Option<(usize, u32)>,
        tries: u32,
        waited: u32,
    },
    /// One trial a seat.
    Wave {
        v: usize,
        runs: Vec<Option<(Task, TrialRun)>>,
    },
    Done,
}

impl Phase {
    fn name(&self) -> &'static str {
        match self {
            Phase::Start(_) => "start",
            Phase::Calibrate { .. } => "calibrate",
            Phase::Settle { .. } => "settle",
            Phase::Launch { .. } => "launch",
            Phase::Capture { .. } => "capture",
            Phase::Pick => "pick",
            Phase::Restore { .. } => "restore",
            Phase::Wave { .. } => "wave",
            Phase::Done => "done",
        }
    }
}

/// A run's whole job: every site of the run, batch by batch.
pub struct Job {
    map: String,
    shard: u32,
    shards: u32,
    s: Settings,
    drop_settings: drops::Settings,
    sites: Vec<SiteJob>,
    batches: Vec<Vec<usize>>,
    batch: usize,
    /// The current batch's site a slot (a seat), if it has one.
    slots: Vec<Option<usize>>,
    /// Each slot's seat, read on the first tick.
    seats: Vec<u8>,
    n_seats: usize,
    phase: Phase,
    round: u32,
    /// Waves run a variant in this batch, and the first one's end hash.
    waves_run: Vec<u32>,
    hash0: Vec<Option<u64>>,
    /// The wave that just ended: `(variant, number)`.
    last_wave: Option<(usize, u32)>,
    wave: (usize, u32),
    trials: Vec<Trial>,
    stats: ForkStats,
    notes: Vec<String>,
    error: Option<String>,
}

fn sense_of(senses: &[SeatSense], seat: u8) -> Sense {
    senses
        .iter()
        .find(|s| s.seat == seat)
        .map(|s| s.sense.clone())
        .unwrap_or_default()
}

fn plain(seat: u8, base: drops::Act) -> SeatAct {
    SeatAct {
        seat,
        act: Act {
            base,
            ..Act::default()
        },
    }
}

impl Job {
    /// The job for `seats` seats on `map`. The targets must be a list:
    /// `h2 sim bounces` resolves `"scan"` from the map's `.spots`.
    pub fn new(cfg: &BouncesConfig, map: &str, seats: u32) -> Result<Job, String> {
        let s = cfg.settings().map_err(|p| p.join("\n"))?;
        let Targets::List(list) = &cfg.targets else {
            return Err("bounces: targets = \"scan\" is resolved by `h2 sim bounces`, which writes each run's targets; a run on its own needs a list".into());
        };
        let targets = list
            .iter()
            .enumerate()
            .map(|(i, t)| Target::from_spec(i, t))
            .collect::<Result<Vec<Target>, String>>()?;
        let sites = Site::of(&targets, s.along);
        if sites.is_empty() {
            return Err("bounces: no targets".into());
        }
        let n = seats.clamp(1, drops::SEATS) as usize;
        let points: Vec<[f32; 2]> = sites.iter().map(|x| xy(x.point)).collect();
        let batches = batches(&points, n, drops::CLEAR);
        let variants = s.variants.len();
        Ok(Job {
            map: map.to_string(),
            shard: cfg.shard,
            shards: cfg.shards,
            drop_settings: s.drops(),
            sites: sites
                .into_iter()
                .map(|site| SiteJob {
                    site,
                    anchor: None,
                    bases: vec![Base::default(); variants],
                    calib: vec![None; variants],
                    points: Vec::new(),
                    outcomes: Vec::new(),
                    queue: vec![VecDeque::new(); variants],
                    aim: [0.0; 3],
                })
                .collect(),
            batches,
            batch: 0,
            slots: Vec::new(),
            seats: Vec::new(),
            n_seats: n,
            phase: Phase::Start(0),
            round: 0,
            waves_run: vec![0; variants],
            hash0: vec![None; variants],
            last_wave: None,
            wave: (0, 0),
            trials: Vec::new(),
            stats: ForkStats::default(),
            notes: Vec::new(),
            error: None,
            s,
        })
    }

    pub fn finished(&self) -> bool {
        matches!(self.phase, Phase::Done)
    }

    /// The named error that stopped the run, if one did.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The most trials the run can make.
    pub fn planned(&self) -> usize {
        self.sites.len() * self.s.trials_per_site()
    }

    pub fn done(&self) -> usize {
        self.trials.len()
    }

    pub fn sites(&self) -> usize {
        self.sites.len()
    }

    pub fn batches(&self) -> usize {
        self.batches.len()
    }

    /// Where the job is: its phase's name, its batch and its round.
    pub fn at(&self) -> (&'static str, usize, u32) {
        (self.phase.name(), self.batch, self.round)
    }

    pub fn stats(&self) -> &ForkStats {
        &self.stats
    }

    pub fn note(&mut self, n: impl Into<String>) {
        let n = n.into();
        if !self.notes.contains(&n) && self.notes.len() < 64 {
            self.notes.push(n);
        }
    }

    /// Stops the run with a named error (the glue's: a restore that left
    /// the machine part loaded).
    pub fn stop(&mut self, why: impl Into<String>) {
        self.fatal(why);
    }

    fn fatal(&mut self, why: impl Into<String>) {
        self.error.get_or_insert_with(|| why.into());
        self.phase = Phase::Done;
    }

    fn seat(&self, slot: usize) -> u8 {
        self.seats.get(slot).copied().unwrap_or(slot as u8)
    }

    fn verifying(&self) -> bool {
        match self.s.verify {
            Verify::None => false,
            Verify::First => self.batch == 0,
            Verify::All => true,
        }
    }

    fn base_drop(&self, site: usize, v: usize, hold: bool, attempt: u32) -> BaseDrop {
        let x = &self.sites[site].site;
        let var = &self.s.variants[v];
        let speed = var.carry_along.unwrap_or(0.0);
        let carry = [x.drift[0] * speed, x.drift[1] * speed];
        let drop = drops::single_drop(
            &self.drop_settings,
            &x.target,
            x.point,
            x.along,
            0.0,
            var.height,
            carry,
            var.carry_along,
        );
        let mut run = DropRun::new(drop, &x.target, &self.drop_settings, 0);
        run.set_hold(hold);
        BaseDrop {
            run,
            attempt,
            arm: None,
            release: None,
            lift_asked: None,
            lift_miss: None,
        }
    }

    fn begin_batch(&mut self) {
        let Some(batch) = self.batches.get(self.batch) else {
            self.phase = Phase::Done;
            return;
        };
        self.slots = (0..self.n_seats).map(|i| batch.get(i).copied()).collect();
        let variants = self.s.variants.len();
        self.waves_run = vec![0; variants];
        self.hash0 = vec![None; variants];
        self.last_wave = None;
        self.round = 0;
        self.begin_calibrate(0);
    }

    fn begin_calibrate(&mut self, v: usize) {
        let runs = self
            .slots
            .iter()
            .map(|s| s.map(|site| self.base_drop(site, v, false, 0)))
            .collect();
        self.phase = Phase::Calibrate { v, runs };
    }

    /// The full drop of `site` on variant `v` ended: its timing, and the
    /// site's anchor height if it has none.
    fn calibrated(&mut self, site: usize, v: usize, b: &BaseDrop) {
        let rec = b.run.record(false);
        let lead = self.s.lead;
        if let Some(miss) = b.lift_miss {
            // The trials' teleports ask against the placement's own
            // offset, so the first lands on its start.
            self.sites[site].aim = [-miss[0], -miss[1], -miss[2]];
            const PLACED: &str = "teleport: the placement put the origin";
            if len3(miss) > PLACE_TOLERANCE && !self.notes.iter().any(|n| n.starts_with(PLACED)) {
                self.note(format!(
                    "{PLACED} {} from where it was asked (site {}); trials ask against it",
                    miss_text(miss),
                    self.sites[site].site.id
                ));
            }
        }
        let x = &mut self.sites[site];
        let base = &mut x.bases[v];
        base.outcome = Some(rec.outcome);
        base.speed = rec.touchdown.as_ref().map(|t| t.speed);
        base.launch = rec.launch.map(|l| l.speed);
        base.row = rec.touchdown.as_ref().map(|t| t.t);
        let latency = match (b.arm, b.release) {
            (Some(a), Some(r)) if r >= a => Some((r - a) as u32),
            _ => None,
        };
        match (&rec.touchdown, latency) {
            (Some(t), Some(latency)) if t.t >= lead + MIN_FALL_ROWS => {
                if x.anchor.is_none() {
                    x.anchor = Some(t.pos[2] - x.site.point[2]);
                }
                x.calib[v] = Some(Calib {
                    row: t.t,
                    latency,
                    pos: t.pos,
                    vel: t.vel,
                });
            }
            (Some(t), Some(_)) => {
                base.why = Some(format!(
                    "calibration: the fall is {} rows, under the lead's {lead} and {MIN_FALL_ROWS}",
                    t.t
                ));
            }
            _ => {
                base.why = Some(format!(
                    "calibration: {}",
                    rec.why.as_deref().unwrap_or("no touchdown")
                ));
            }
        }
    }

    fn begin_base(&mut self, v: usize) {
        let runs: Vec<Option<BaseDrop>> = self
            .slots
            .iter()
            .map(|s| {
                s.filter(|&site| self.sites[site].calib[v].is_some())
                    .map(|site| self.base_drop(site, v, true, 0))
            })
            .collect();
        if runs.iter().all(Option::is_none) {
            self.next_variant(v);
        } else {
            let ready = vec![0; runs.len()];
            self.phase = Phase::Settle { v, runs, ready };
        }
    }

    fn next_variant(&mut self, v: usize) {
        if v + 1 < self.s.variants.len() {
            self.begin_calibrate(v + 1);
        } else {
            self.plan_first_round();
            self.phase = Phase::Pick;
        }
    }

    fn no_base(&mut self, site: usize, v: usize, why: String) {
        let id = self.sites[site].site.id.clone();
        let name = self.s.variants[v].name.clone();
        self.note(format!("{id}: no base on {name}: {why}"));
        let b = &mut self.sites[site].bases[v];
        b.based = false;
        b.why = Some(why);
    }

    /// The first round's points and tasks of every site of the batch
    /// that has a base.
    fn plan_first_round(&mut self) {
        let verifying = self.verifying();
        let variants = self.s.variants.len();
        let batch: Vec<usize> = self.slots.iter().flatten().copied().collect();
        for site in batch {
            let x = &self.sites[site];
            if x.anchor.is_none() || !(0..variants).any(|v| x.based(v)) {
                continue;
            }
            let xs = match &self.s.grid {
                Some(counts) => sample::grid(&self.s.axes, counts).unwrap_or_default(),
                None => {
                    let mut rng = sample::Rng::new(sample::seed_for(self.s.seed, &x.site.id));
                    sample::lhs(&self.s.axes, self.s.points as usize, &mut rng)
                }
            };
            let crouch = self.s.crouch;
            let points: Vec<Point> = xs
                .into_iter()
                .enumerate()
                .map(|(i, x)| Point {
                    x,
                    crouch: match crouch {
                        Crouch::Off => false,
                        Crouch::On => true,
                        Crouch::Both => i % 2 == 1,
                    },
                    round: 0,
                })
                .collect();
            let n = points.len() as u32;
            let (replay, unprimed) = (self.s.replay.min(n), self.s.unprimed.min(n));
            let x = &mut self.sites[site];
            x.outcomes = vec![vec![None; variants]; points.len()];
            x.points = points;
            let mut first = true;
            for v in 0..variants {
                if !x.based(v) {
                    continue;
                }
                let q = &mut x.queue[v];
                q.push_back(Task::Anchor);
                if verifying {
                    q.push_back(Task::Anchor);
                }
                q.extend((0..n).map(Task::Sample));
                q.extend((0..replay).map(Task::Replay));
                if first {
                    q.extend((0..unprimed).map(Task::Unprimed));
                    first = false;
                }
            }
        }
    }

    /// The next round's points: each site's refinement, by crouch. False
    /// when no site has any.
    fn plan_refinement(&mut self) -> bool {
        let round = self.round;
        let variants = self.s.variants.len();
        let batch: Vec<usize> = self.slots.iter().flatten().copied().collect();
        let mut any = false;
        for site in batch {
            let x = &self.sites[site];
            if x.points.is_empty() {
                continue;
            }
            let both = x.points.iter().any(|p| p.crouch) && x.points.iter().any(|p| !p.crouch);
            let want = (self.s.refine as usize) / if both { 2 } else { 1 };
            let mut fresh: Vec<Point> = Vec::new();
            for crouch in [false, true] {
                let idx: Vec<usize> = (0..x.points.len())
                    .filter(|&i| x.points[i].crouch == crouch)
                    .collect();
                if idx.is_empty() {
                    continue;
                }
                let xs: Vec<Vector> = idx.iter().map(|&i| x.points[i].x).collect();
                let labels: Vec<Option<bool>> = idx
                    .iter()
                    .map(|&i| match x.class(i) {
                        Class::All => Some(true),
                        Class::Some | Class::None => Some(false),
                        Class::Unknown => None,
                    })
                    .collect();
                for p in sample::refine(&xs, &labels, &self.s.axes, &self.s.human, want, MIN_GAP) {
                    fresh.push(Point {
                        x: p,
                        crouch,
                        round: round.min(255) as u8,
                    });
                }
            }
            if fresh.is_empty() {
                continue;
            }
            any = true;
            let x = &mut self.sites[site];
            let from = x.points.len() as u32;
            let to = from + fresh.len() as u32;
            x.points.extend(fresh);
            x.outcomes.resize(to as usize, vec![None; variants]);
            for v in 0..variants {
                if x.based(v) {
                    x.queue[v].extend((from..to).map(Task::Sample));
                }
            }
        }
        any
    }

    fn plan_of(&self, site: usize, v: usize, task: Task) -> Option<Plan> {
        let x = &self.sites[site];
        match task {
            Task::Anchor => {
                let c = x.calib[v].as_ref()?;
                let at = Placed {
                    pos: c.pos,
                    vel: c.vel,
                    facing: x.site.drift,
                };
                Some(Plan {
                    start: at,
                    last: at,
                    flight: 0,
                    primed: true,
                    crouch: false,
                })
            }
            Task::Sample(p) | Task::Replay(p) | Task::Unprimed(p) => {
                let point = x.points.get(p as usize)?;
                let last = x.site.place(x.anchor?, &point.x);
                Some(Plan {
                    start: back(&last, self.s.flight),
                    last,
                    flight: self.s.flight,
                    primed: !matches!(task, Task::Unprimed(_)),
                    crouch: point.crouch,
                })
            }
        }
    }

    fn record(&mut self, site: usize, v: usize, task: Task, run: &TrialRun) {
        let r = run.result();
        let x = &mut self.sites[site];
        let (kind, point) = match task {
            Task::Anchor => (Kind::Anchor, None),
            Task::Sample(p) => (Kind::Sample, Some(p)),
            Task::Replay(p) => (Kind::Replay, Some(p)),
            Task::Unprimed(p) => (Kind::Unprimed, Some(p)),
        };
        if let Task::Sample(p) = task
            && let Some(o) = x.outcomes.get_mut(p as usize)
        {
            o[v] = Some(r.outcome);
        }
        if let Some(aim) = r.aim {
            x.aim = aim;
        }
        let last = match (r.last, x.anchor) {
            (Some((pos, vel)), Some(anchor)) => Some(x.site.measured(anchor, pos, vel)),
            _ => None,
        };
        self.trials.push(Trial {
            site: site as u32,
            variant: v as u8,
            kind,
            point,
            outcome: r.outcome,
            why: r.why,
            launch: r.launch,
            last,
            reaimed: run.corrected(),
        });
    }

    /// One tick: every seat's sense, and what came of the fork the tick
    /// before asked for.
    pub fn tick(&mut self, tick: u64, senses: &[SeatSense], fork: Option<ForkDone>) -> Step {
        let mut fork = fork;
        // Each pass either answers the tick or moves to a phase that
        // will; a phase that moves on has given no act.
        for _ in 0..64 {
            let phase = std::mem::replace(&mut self.phase, Phase::Done);
            let step = match phase {
                Phase::Done => {
                    return Step::default();
                }
                Phase::Start(n) => {
                    if self.seats.is_empty() && !senses.is_empty() {
                        let mut seats: Vec<u8> = senses.iter().map(|s| s.seat).collect();
                        seats.sort_unstable();
                        seats.truncate(self.n_seats);
                        self.seats = seats;
                    }
                    if n < drops::START_TICKS || self.seats.is_empty() {
                        self.phase = Phase::Start(n + 1);
                        Some(Step::default())
                    } else {
                        self.begin_batch();
                        None
                    }
                }
                Phase::Calibrate { v, runs } => self.calibrate(tick, senses, v, runs),
                Phase::Settle { v, runs, ready } => self.settle(tick, senses, v, runs, ready),
                Phase::Launch {
                    v,
                    runs,
                    arm_at,
                    fork_tick,
                } => self.launch(tick, senses, v, runs, arm_at, fork_tick),
                Phase::Capture {
                    v,
                    runs,
                    tries,
                    waited,
                } => self.capture(tick, senses, v, runs, tries, waited, fork.take()),
                Phase::Pick => self.pick(),
                Phase::Restore {
                    v,
                    ended,
                    tries,
                    waited,
                } => self.restore(v, ended, tries, waited, fork.take()),
                Phase::Wave { v, runs } => self.wave(senses, v, runs),
            };
            if let Some(step) = step {
                return step;
            }
        }
        self.fatal("bounces: the job did not settle on a phase");
        Step::default()
    }

    fn calibrate(
        &mut self,
        tick: u64,
        senses: &[SeatSense],
        v: usize,
        mut runs: Vec<Option<BaseDrop>>,
    ) -> Option<Step> {
        let mut acts = Vec::new();
        let mut busy = false;
        for slot in 0..runs.len() {
            let (Some(site), Some(b)) = (self.slots[slot], runs[slot].as_mut()) else {
                continue;
            };
            let seat = self.seat(slot);
            acts.push(plain(seat, b.step(tick, &sense_of(senses, seat))));
            if !b.run.done() {
                busy = true;
                continue;
            }
            let (outcome, _) = b.run.outcome();
            if outcome == Outcome::SetupFailed && b.attempt < self.s.retries {
                let attempt = b.attempt + 1;
                runs[slot] = Some(self.base_drop(site, v, false, attempt));
                busy = true;
            } else {
                let b = runs[slot].take().expect("the run just stepped");
                self.calibrated(site, v, &b);
            }
        }
        if busy {
            self.phase = Phase::Calibrate { v, runs };
        } else {
            self.begin_base(v);
        }
        Some(Step { acts, fork: None })
    }

    fn settle(
        &mut self,
        tick: u64,
        senses: &[SeatSense],
        v: usize,
        mut runs: Vec<Option<BaseDrop>>,
        mut ready: Vec<u32>,
    ) -> Option<Step> {
        let rest = self.s.variants[v].rest.unwrap_or(0);
        let mut acts = Vec::new();
        for slot in 0..runs.len() {
            let (Some(site), Some(b)) = (self.slots[slot], runs[slot].as_mut()) else {
                continue;
            };
            let seat = self.seat(slot);
            acts.push(plain(seat, b.step(tick, &sense_of(senses, seat))));
            if b.run.done() {
                let (_, why) = b.run.outcome();
                if b.attempt < self.s.retries {
                    let attempt = b.attempt + 1;
                    runs[slot] = Some(self.base_drop(site, v, true, attempt));
                } else {
                    runs[slot] = None;
                    self.no_base(site, v, format!("base: {}", why.unwrap_or_default()));
                }
                ready[slot] = 0;
            } else if b.run.ready() {
                ready[slot] += 1;
            } else {
                ready[slot] = 0;
            }
        }
        if runs.iter().all(Option::is_none) {
            self.next_variant(v);
            return Some(Step { acts, fork: None });
        }
        let all = (0..runs.len()).all(|slot| runs[slot].is_none() || ready[slot] > rest);
        if !all {
            self.phase = Phase::Settle { v, runs, ready };
            return Some(Step { acts, fork: None });
        }
        // Every seat is still. Each is let go so that all reach the row
        // `lead` before their touchdown ticks on one tick.
        let lead = self.s.lead;
        let mut rows = vec![0u64; runs.len()];
        for slot in 0..runs.len() {
            if runs[slot].is_none() {
                continue;
            }
            let c = self.slots[slot]
                .and_then(|site| self.sites[site].calib[v].as_ref())
                .expect("a base has its calibration");
            rows[slot] = u64::from(c.latency + c.row - lead);
        }
        let fork_tick = tick + 1 + rows.iter().copied().max().unwrap_or(0);
        let arm_at: Vec<u64> = rows.iter().map(|r| fork_tick - r).collect();
        self.phase = Phase::Launch {
            v,
            runs,
            arm_at,
            fork_tick,
        };
        Some(Step { acts, fork: None })
    }

    fn launch(
        &mut self,
        tick: u64,
        senses: &[SeatSense],
        v: usize,
        mut runs: Vec<Option<BaseDrop>>,
        arm_at: Vec<u64>,
        fork_tick: u64,
    ) -> Option<Step> {
        let lead = self.s.lead;
        let mut acts = Vec::new();
        for slot in 0..runs.len() {
            let (Some(site), Some(b)) = (self.slots[slot], runs[slot].as_mut()) else {
                continue;
            };
            if tick >= arm_at[slot] {
                b.run.set_hold(false);
            }
            let seat = self.seat(slot);
            acts.push(plain(seat, b.step(tick, &sense_of(senses, seat))));
            let c = self.sites[site].calib[v].as_ref().expect("a base has its calibration");
            let why = if b.run.done() {
                let (_, why) = b.run.outcome();
                Some(format!("base: {}", why.unwrap_or_else(|| "ended early".into())))
            } else if tick >= fork_tick {
                let want = arm_at[slot] + u64::from(c.latency);
                let in_step = b.release == Some(want) && b.run.fall_row() == Some(c.row - lead + 1);
                (!in_step).then(|| match b.release {
                    Some(r) => format!("base: out of step: released at tick {r}, planned {want}"),
                    None => "base: out of step: not released".into(),
                })
            } else {
                None
            };
            if let Some(why) = why {
                runs[slot] = None;
                self.no_base(site, v, why);
            }
        }
        if runs.iter().all(Option::is_none) {
            self.next_variant(v);
            return Some(Step { acts, fork: None });
        }
        if tick < fork_tick {
            self.phase = Phase::Launch {
                v,
                runs,
                arm_at,
                fork_tick,
            };
            return Some(Step { acts, fork: None });
        }
        self.phase = Phase::Capture {
            v,
            runs,
            tries: 0,
            waited: 0,
        };
        Some(Step {
            acts,
            fork: Some(ForkAsk::Capture { key: v as u64 }),
        })
    }

    fn capture(
        &mut self,
        tick: u64,
        senses: &[SeatSense],
        v: usize,
        mut runs: Vec<Option<BaseDrop>>,
        tries: u32,
        waited: u32,
        fork: Option<ForkDone>,
    ) -> Option<Step> {
        let live: Vec<usize> = (0..runs.len())
            .filter(|&slot| runs[slot].is_some())
            .filter_map(|slot| self.slots[slot])
            .collect();
        match fork {
            Some(ForkDone::Captured { .. }) => {
                self.stats.captures += 1;
                for site in live {
                    self.sites[site].bases[v].based = true;
                }
                self.next_variant(v);
                None
            }
            Some(ForkDone::Busy(why)) => {
                self.stats.busy += 1;
                self.stats.last_busy = Some(why.clone());
                // Another row of the fall is spent. The trial's teleport
                // needs two more before the base's touchdown tick.
                if tries + 2 >= self.s.lead {
                    for site in live {
                        self.no_base(site, v, format!("fork: busy: {why}"));
                    }
                    self.next_variant(v);
                    return None;
                }
                let mut acts = Vec::new();
                for slot in 0..runs.len() {
                    if let Some(b) = runs[slot].as_mut() {
                        let seat = self.seat(slot);
                        acts.push(plain(seat, b.step(tick, &sense_of(senses, seat))));
                    }
                }
                self.phase = Phase::Capture {
                    v,
                    runs,
                    tries: tries + 1,
                    waited: 0,
                };
                Some(Step {
                    acts,
                    fork: Some(ForkAsk::Capture { key: v as u64 }),
                })
            }
            Some(ForkDone::Failed(e)) => {
                self.fatal(format!("fork: capture: {e}"));
                Some(Step::default())
            }
            Some(ForkDone::Restored { .. }) => {
                self.fatal("fork: a restore answered a capture");
                Some(Step::default())
            }
            None if waited + 1 >= FORK_WAIT => {
                self.fatal("fork: no answer to a capture: this host's loop runs no bounce fork");
                Some(Step::default())
            }
            None => {
                self.phase = Phase::Capture {
                    v,
                    runs,
                    tries,
                    waited: waited + 1,
                };
                Some(Step::default())
            }
        }
    }

    fn pick(&mut self) -> Option<Step> {
        let variants = self.s.variants.len();
        let pending = |job: &Job, v: usize| {
            job.slots
                .iter()
                .flatten()
                .any(|&site| job.sites[site].based(v) && !job.sites[site].queue[v].is_empty())
        };
        if let Some(v) = (0..variants).find(|&v| pending(self, v)) {
            let ended = self.last_wave.take();
            // The first restore of a batch and the ones that end a
            // variant's first two waves (the anchors, twice) are checked.
            let check = self.verifying() && ended.is_none_or(|(_, n)| n < 2);
            self.phase = Phase::Restore {
                v,
                ended,
                tries: 0,
                waited: 0,
            };
            return Some(Step {
                acts: Vec::new(),
                fork: Some(ForkAsk::Restore {
                    key: v as u64,
                    check,
                }),
            });
        }
        // The round is over.
        self.last_wave = None;
        self.round += 1;
        if self.round <= self.s.rounds && self.plan_refinement() {
            self.phase = Phase::Pick;
        } else {
            self.batch += 1;
            self.begin_batch();
        }
        None
    }

    fn restore(
        &mut self,
        v: usize,
        ended: Option<(usize, u32)>,
        tries: u32,
        waited: u32,
        fork: Option<ForkDone>,
    ) -> Option<Step> {
        let ask = |check: bool| ForkAsk::Restore {
            key: v as u64,
            check,
        };
        match fork {
            Some(ForkDone::Restored {
                end_hash, ram_equal, ..
            }) => {
                self.stats.restores += 1;
                match (ended, end_hash) {
                    (Some((ve, 0)), Some(h)) => self.hash0[ve] = Some(h),
                    (Some((ve, 1)), Some(h)) => {
                        if let Some(first) = self.hash0[ve] {
                            self.stats.replay_checks += 1;
                            if first != h {
                                self.stats.replay_mismatches += 1;
                                let name = self.s.variants[ve].name.clone();
                                self.note(format!(
                                    "fork: batch {}: {name}'s wave ended in other guest RAM the second time",
                                    self.batch
                                ));
                            }
                        }
                    }
                    _ => {}
                }
                if let Some(equal) = ram_equal {
                    self.stats.ram_checks += 1;
                    if !equal {
                        self.stats.ram_mismatches += 1;
                        let name = self.s.variants[v].name.clone();
                        self.note(format!(
                            "fork: batch {}: guest RAM after a restore of {name} is not the capture's",
                            self.batch
                        ));
                    }
                }
                let mut runs: Vec<Option<(Task, TrialRun)>> = Vec::with_capacity(self.slots.len());
                for slot in 0..self.slots.len() {
                    let mut run = None;
                    if let Some(site) = self.slots[slot]
                        && self.sites[site].based(v)
                    {
                        // A task with no plan is dropped; the next takes
                        // its place.
                        while let Some(task) = self.sites[site].queue[v].pop_front() {
                            if let Some(plan) = self.plan_of(site, v, task) {
                                let aim = self.sites[site].aim;
                                run = Some((
                                    task,
                                    TrialRun::new(plan, self.s.bounce_vz).aimed(aim),
                                ));
                                break;
                            }
                        }
                    }
                    runs.push(run);
                }
                let n = self.waves_run[v];
                self.waves_run[v] += 1;
                self.wave = (v, n);
                self.stats.waves += 1;
                self.phase = Phase::Wave { v, runs };
                None
            }
            Some(ForkDone::Busy(why)) => {
                self.stats.busy += 1;
                self.stats.last_busy = Some(why.clone());
                if tries + 1 >= RESTORE_TRIES {
                    self.fatal(format!("fork: restore: busy for {RESTORE_TRIES} ticks: {why}"));
                    return Some(Step::default());
                }
                self.phase = Phase::Restore {
                    v,
                    ended: None,
                    tries: tries + 1,
                    waited: 0,
                };
                Some(Step {
                    acts: Vec::new(),
                    fork: Some(ask(false)),
                })
            }
            Some(ForkDone::Failed(e)) => {
                self.fatal(format!("fork: restore: {e}"));
                Some(Step::default())
            }
            Some(ForkDone::Captured { .. }) => {
                self.fatal("fork: a capture answered a restore");
                Some(Step::default())
            }
            None if waited + 1 >= FORK_WAIT => {
                self.fatal("fork: no answer to a restore: this host's loop runs no bounce fork");
                Some(Step::default())
            }
            None => {
                self.phase = Phase::Restore {
                    v,
                    ended,
                    tries,
                    waited: waited + 1,
                };
                Some(Step::default())
            }
        }
    }

    fn wave(
        &mut self,
        senses: &[SeatSense],
        v: usize,
        mut runs: Vec<Option<(Task, TrialRun)>>,
    ) -> Option<Step> {
        self.stats.ticks += 1;
        let mut acts = Vec::new();
        let mut busy = false;
        for slot in 0..runs.len() {
            let Some((_, run)) = runs[slot].as_mut() else {
                continue;
            };
            if run.done() {
                continue;
            }
            let seat = self.seat(slot);
            let act = run.step(&sense_of(senses, seat));
            acts.push(SeatAct { seat, act });
            busy |= !run.done();
        }
        if busy {
            self.phase = Phase::Wave { v, runs };
            return Some(Step { acts, fork: None });
        }
        // Every trial is judged; a trial's last row gives no act.
        for slot in 0..runs.len() {
            if let (Some(site), Some((task, run))) = (self.slots[slot], runs[slot].as_ref()) {
                self.record(site, v, *task, run);
            }
        }
        // A wave whose teleport was asked again ran rows its replay may
        // not: its end is no replay check's.
        let corrected = runs.iter().flatten().any(|(_, run)| run.corrected());
        self.last_wave = (!corrected).then_some(self.wave);
        self.phase = Phase::Pick;
        None
    }

    /// The run's block of the report.
    pub fn report(&self) -> BouncesRun {
        BouncesRun {
            format: FORMAT.into(),
            map: self.map.clone(),
            shard: self.shard,
            shards: self.shards,
            seats: self.n_seats as u32,
            planned: self.planned() as u32,
            done: self.trials.len() as u32,
            error: self.error.clone(),
            axes: self.s.axes.to_vec(),
            human: self.s.human.to_vec(),
            variants: self.s.variants.clone(),
            lead: self.s.lead,
            flight: self.s.flight,
            bounce_vz: self.s.bounce_vz,
            min_support: self.s.min_support,
            sites: self
                .sites
                .iter()
                .map(|x| SiteRun {
                    site: x.site.clone(),
                    anchor: x.anchor,
                    bases: x.bases.clone(),
                    points: x.points.clone(),
                })
                .collect(),
            trials: self.trials.clone(),
            fork: self.stats.clone(),
            notes: self.notes.clone(),
        }
    }
}
