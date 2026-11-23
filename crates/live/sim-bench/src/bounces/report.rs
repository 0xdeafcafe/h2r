//! The bounce search's report (`bounces.json`, `bounces.md`,
//! `bounces.verified`; docs/runtime/bounce-search.md, "The command"):
//! every run's trials pooled, each site's bounce region, its window and
//! what the window is to a player, and the sites ranked.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use super::region::{self, Class, Window};
use super::{
    ACROSS, ALONG, AXES, BouncesRun, DEPTH, ForkStats, Kind, N, Site, SiteRun, Trial, V_DRIFT, VZ,
    Vector,
};
use crate::drops::{self, Outcome, Proven, dot2};

/// The report's schema tag.
pub const REPORT_FORMAT: &str = "h2-sim-bounces-report/1";
/// World units to centimetres.
pub const CM: f32 = 304.8;
/// Under this speed along the drift (units/s) a window has no timing: the
/// body is not running across it.
pub const RUNNING: f32 = 0.05;

fn is_false(b: &bool) -> bool {
    !*b
}

/// What the search made of a site.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// It has a window: a cube of inputs, with support, that bounced on
    /// every variant with no failure inside.
    Provable,
    /// Points bounced, but no window has the support.
    Fragile,
    /// Nothing bounced.
    None,
    /// No variant gave a base, so no trial ran.
    #[default]
    Untested,
}

impl Verdict {
    pub fn name(self) -> &'static str {
        match self {
            Verdict::Provable => "provable",
            Verdict::Fragile => "fragile",
            Verdict::None => "none",
            Verdict::Untested => "untested",
        }
    }
}

/// One variant of a site.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VariantLine {
    pub name: String,
    pub based: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// The full drop's outcome, and the anchor trial's: the same last row
    /// put back by teleport.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drop: Option<Outcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<Outcome>,
    /// Whether the two are the same outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agrees: Option<bool>,
    /// Sampled trials, and how they came out.
    pub trials: u32,
    pub bounced: u32,
    pub prime_lost: u32,
    pub setup_failed: u32,
}

/// The window in a player's terms.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Press {
    /// Where the window's centre lands, world X and Y.
    pub aim: [f32; 2],
    /// How far the landing can move along the drift (world units): the
    /// position window of the takeoff, since a jump's flight is fixed by
    /// where it leaves the floor.
    pub position: f32,
    /// How far it can move across the drift, along the edge for a drift
    /// square to it (world units).
    pub sideways: f32,
    /// The speeds along the drift that bounce, `[low, high]` units/s.
    pub speed: [f32; 2],
    /// The touchdown speeds down that bounce.
    pub down: [f32; 2],
    /// `position` over the ground a tick covers at the centre's speed
    /// along the drift: how many ticks in a row a press can fall on.
    /// `None` under [`RUNNING`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timing_ticks: Option<f32>,
    /// The share of depths that bounce at the window's aim: of tick
    /// phases, when the depth range is one tick of fall.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase_share: Option<f32>,
    /// The drop height whose touchdown speed is the centre's
    /// ([`drops::fall_model`]).
    pub height: f32,
}

/// One site's line.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SiteSummary {
    /// From 1, the easiest provable site first.
    pub rank: u32,
    pub id: String,
    pub target: String,
    pub shape: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub crouch: bool,
    pub point: [f32; 3],
    pub stand: [f32; 3],
    pub verdict: Verdict,
    /// The window's radius in human steps; 0 with no window.
    pub ease: f32,
    /// Of the first round's points with an outcome, the share that
    /// bounced on every variant, and on one at least: the region's volume
    /// as a share of the sampled space.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_any: Option<f32>,
    /// Of the points that bounced on a variant, the share that bounced on
    /// every one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reproducibility: Option<f32>,
    pub points: u32,
    /// Points by class.
    pub all: u32,
    pub some: u32,
    pub none: u32,
    pub unknown: u32,
    /// Sampled trials, and how they came out.
    pub trials: u32,
    pub bounced: u32,
    pub no_bounce: u32,
    pub prime_lost: u32,
    pub setup_failed: u32,
    pub best_launch: f32,
    /// Points run twice on a variant, and how many came out otherwise.
    pub replays: u32,
    pub replay_mismatches: u32,
    /// Points run with the word 0, and how many bounced anyway.
    pub unprimed: u32,
    pub unprimed_bounced: u32,
    pub variants: Vec<VariantLine>,
    /// The range of each axis over the points that bounced on every
    /// variant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extent: Option<[[f32; 2]; N]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<Window>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub press: Option<Press>,
}

/// The pooled report.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BouncesReport {
    pub format: String,
    pub map: String,
    pub axes: Vec<String>,
    /// The human step of each axis.
    pub human: Vec<f32>,
    pub variants: Vec<String>,
    pub runs: u32,
    pub planned: u32,
    /// Every trial, anchors, replays and unprimed ones included.
    pub trials: u32,
    /// Trials by outcome.
    pub outcomes: BTreeMap<String, u32>,
    pub fork: ForkStats,
    /// Ranked.
    pub sites: Vec<SiteSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

/// The drop height whose touchdown speed first reaches `speed`, on a 0.05
/// grid from 0.2 to 20.
pub fn height_for(speed: f32) -> f32 {
    let mut h = 0.2f32;
    while h < 20.0 {
        if drops::fall_model(h).1 >= speed {
            return h;
        }
        h += 0.05;
    }
    20.0
}

fn press(site: &Site, anchor: f32, w: &Window, phase: Option<f32>) -> Press {
    let width = |a: usize| w.bounds[a][1] - w.bounds[a][0];
    let d = site.drift;
    let left = [-d[1], d[0]];
    // A step along a direction moves the landing across the edge by its
    // part along the normal and along the edge by its part along the
    // edge: the direction's room is the tighter of the two.
    let room = |dir: [f32; 2]| -> f32 {
        let mut best = f32::INFINITY;
        for (axis, part) in [
            (ACROSS, dot2(dir, site.target.normal).abs()),
            (ALONG, dot2(dir, site.edge).abs()),
        ] {
            if part > 1e-3 {
                best = best.min(width(axis) / part);
            }
        }
        if best.is_finite() { best } else { 0.0 }
    };
    let position = room(d);
    let v = w.centre[V_DRIFT].abs();
    let at = site.place(anchor, &w.centre);
    Press {
        aim: [at.pos[0], at.pos[1]],
        position,
        sideways: room(left),
        speed: w.bounds[V_DRIFT],
        down: w.bounds[VZ],
        timing_ticks: (v >= RUNNING).then(|| position / (v / 30.0)),
        phase_share: phase,
        height: height_for(w.centre[VZ]),
    }
}

fn summarize_site(
    run: &BouncesRun,
    index: usize,
    x: &SiteRun,
    crouch: bool,
    split: bool,
) -> SiteSummary {
    let mine = |t: &&Trial| t.site as usize == index;
    let of_group = |t: &Trial| {
        t.point
            .and_then(|p| x.points.get(p as usize))
            .is_some_and(|p| p.crouch == crouch)
    };
    let based: Vec<usize> = (0..x.bases.len()).filter(|&v| x.bases[v].based).collect();
    // Each point's outcome a variant, and its last row as it was.
    let mut outcomes: Vec<Vec<Option<Outcome>>> = vec![vec![None; x.bases.len()]; x.points.len()];
    let mut seen: Vec<Option<[f32; 4]>> = vec![None; x.points.len()];
    for t in run.trials.iter().filter(mine) {
        if t.kind != Kind::Sample {
            continue;
        }
        let Some(p) = t.point.map(|p| p as usize).filter(|&p| p < x.points.len()) else {
            continue;
        };
        if let Some(o) = outcomes[p].get_mut(t.variant as usize) {
            *o = Some(t.outcome);
        }
        if seen[p].is_none() {
            seen[p] = t.last;
        }
    }
    let mut s = SiteSummary {
        id: if split && crouch {
            format!("{} crouch", x.site.id)
        } else {
            x.site.id.clone()
        },
        target: x.site.target.id.clone(),
        shape: x.site.target.shape.clone(),
        crouch,
        point: x.site.point,
        stand: x.site.target.stand,
        ..SiteSummary::default()
    };
    let mut classed: Vec<(Vector, Class)> = Vec::new();
    let (mut first_known, mut first_all, mut first_any) = (0u32, 0u32, 0u32);
    for (p, point) in x.points.iter().enumerate() {
        if point.crouch != crouch {
            continue;
        }
        let class = region::class(based.iter().map(|&v| outcomes[p][v]));
        let mut at = point.x;
        if let Some(m) = seen[p] {
            at[ACROSS] = m[0];
            at[ALONG] = m[1];
            at[DEPTH] = m[2];
            at[VZ] = m[3];
        }
        s.points += 1;
        match class {
            Class::All => s.all += 1,
            Class::Some => s.some += 1,
            Class::None => s.none += 1,
            Class::Unknown => s.unknown += 1,
        }
        if point.round == 0 && class != Class::Unknown {
            first_known += 1;
            first_all += u32::from(class == Class::All);
            first_any += u32::from(class != Class::None);
        }
        classed.push((at, class));
    }
    if first_known > 0 {
        s.share = Some(first_all as f32 / first_known as f32);
        s.share_any = Some(first_any as f32 / first_known as f32);
    }
    if s.all + s.some > 0 {
        s.reproducibility = Some(s.all as f32 / (s.all + s.some) as f32);
    }
    let mut lines: Vec<VariantLine> = run
        .variants
        .iter()
        .zip(&x.bases)
        .map(|(v, b)| VariantLine {
            name: v.name.clone(),
            based: b.based,
            why: b.why.clone(),
            drop: b.outcome,
            ..VariantLine::default()
        })
        .collect();
    for t in run.trials.iter().filter(mine) {
        let Some(line) = lines.get_mut(t.variant as usize) else {
            continue;
        };
        match t.kind {
            Kind::Anchor => {
                if line.anchor.is_none() {
                    line.anchor = Some(t.outcome);
                    line.agrees = line.drop.map(|d| d == t.outcome);
                }
            }
            Kind::Sample if of_group(t) => {
                line.trials += 1;
                s.trials += 1;
                match t.outcome {
                    Outcome::Bounced => {
                        line.bounced += 1;
                        s.bounced += 1;
                        s.best_launch = s.best_launch.max(t.launch);
                    }
                    Outcome::NoBounce => s.no_bounce += 1,
                    Outcome::PrimeLost => {
                        line.prime_lost += 1;
                        s.prime_lost += 1;
                    }
                    Outcome::SetupFailed => {
                        line.setup_failed += 1;
                        s.setup_failed += 1;
                    }
                }
            }
            Kind::Replay if of_group(t) => {
                let first = t
                    .point
                    .and_then(|p| outcomes.get(p as usize))
                    .and_then(|o| o.get(t.variant as usize).copied().flatten());
                if let Some(first) = first {
                    s.replays += 1;
                    s.replay_mismatches += u32::from(first != t.outcome);
                }
            }
            Kind::Unprimed if of_group(t) => {
                s.unprimed += 1;
                s.unprimed_bounced += u32::from(t.outcome == Outcome::Bounced);
            }
            _ => {}
        }
    }
    s.variants = lines;
    let human: [f32; N] = std::array::from_fn(|a| run.human.get(a).copied().unwrap_or(0.0));
    s.extent = region::extent(&classed);
    s.window = region::window(&classed, &human, run.min_support);
    if let (Some(w), Some(anchor)) = (&s.window, x.anchor) {
        let phase = region::share_along(&classed, w, &human, DEPTH).map(|(share, _)| share);
        s.ease = w.radius;
        s.press = Some(press(&x.site, anchor, w, phase));
    }
    s.verdict = if based.is_empty() || s.points == 0 {
        Verdict::Untested
    } else if s.window.is_some() {
        Verdict::Provable
    } else if s.all + s.some > 0 {
        Verdict::Fragile
    } else {
        Verdict::None
    };
    s
}

/// Pools the runs' blocks: a line a site (two for a site sampled both
/// standing and crouched), ranked by verdict, then the ease, then the
/// share that bounces on every variant, then the id.
pub fn summarize(runs: &[BouncesRun]) -> BouncesReport {
    let mut r = BouncesReport {
        format: REPORT_FORMAT.into(),
        axes: AXES.iter().map(|a| a.to_string()).collect(),
        runs: runs.len() as u32,
        ..BouncesReport::default()
    };
    for run in runs {
        if r.map.is_empty() {
            r.map = run.map.clone();
        }
        if r.human.is_empty() {
            r.human = run.human.clone();
            r.variants = run.variants.iter().map(|v| v.name.clone()).collect();
        }
        r.planned += run.planned;
        r.trials += run.trials.len() as u32;
        r.fork.add(&run.fork);
        for t in &run.trials {
            *r.outcomes.entry(t.outcome.name().into()).or_default() += 1;
        }
        for n in &run.notes {
            if !r.notes.contains(n) {
                r.notes.push(n.clone());
            }
        }
        if let Some(e) = &run.error {
            r.errors.push(format!("run {}: {e}", run.shard));
        }
        for (i, x) in run.sites.iter().enumerate() {
            let crouched = x.points.iter().any(|p| p.crouch);
            let stood = x.points.iter().any(|p| !p.crouch) || x.points.is_empty();
            if stood {
                r.sites.push(summarize_site(run, i, x, false, crouched));
            }
            if crouched {
                r.sites.push(summarize_site(run, i, x, true, stood));
            }
        }
    }
    if r.fork.replay_mismatches > 0 {
        r.errors.push(format!(
            "fork: {} of {} replayed waves ended in other guest RAM: the trials are not exact replays",
            r.fork.replay_mismatches, r.fork.replay_checks
        ));
    }
    if r.fork.ram_mismatches > 0 {
        r.errors.push(format!(
            "fork: {} of {} checked restores left guest RAM other than the capture's",
            r.fork.ram_mismatches, r.fork.ram_checks
        ));
    }
    r.sites.sort_by(|a, b| {
        a.verdict
            .cmp(&b.verdict)
            .then(b.ease.partial_cmp(&a.ease).unwrap_or(std::cmp::Ordering::Equal))
            .then(
                b.share
                    .unwrap_or(0.0)
                    .partial_cmp(&a.share.unwrap_or(0.0))
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then(a.id.cmp(&b.id))
    });
    for (i, s) in r.sites.iter_mut().enumerate() {
        s.rank = i as u32 + 1;
    }
    r
}

/// The provable standing sites as verified entries, the best-ranked one a
/// stand: the landing point is the window's aim, the height the drop
/// whose touchdown speed is the window centre's.
pub fn proven(r: &BouncesReport) -> Vec<Proven> {
    let mut out: Vec<Proven> = Vec::new();
    for s in &r.sites {
        let (Verdict::Provable, false, Some(p)) = (s.verdict, s.crouch, &s.press) else {
            continue;
        };
        if out.iter().any(|o| o.stand == s.stand) {
            continue;
        }
        out.push(Proven {
            target: s.id.clone(),
            stand: s.stand,
            offset: [p.aim[0] - s.stand[0], p.aim[1] - s.stand[1]],
            height: p.height,
            trials: s.bounced + s.no_bounce,
            ok: s.bounced,
            best: s.best_launch,
        });
    }
    out
}

/// `proven` as a verified file's text (docs/game/superbounce.md,
/// "Verifying spots"; `superbounce::verified`, whose `parse` a game test
/// reads this with): the header, then a `bounce proven` line each.
pub fn verified_text(proven: &[Proven]) -> String {
    let mut o = String::from("# superbounce verified v1\n");
    for p in proven {
        let _ = write!(
            o,
            "bounce proven {:.3} {:.3} {:.3} dx={:.3} dy={:.3} height={:.3} trials={} ok={}",
            p.stand[0], p.stand[1], p.stand[2], p.offset[0], p.offset[1], p.height, p.trials, p.ok
        );
        if p.best > 0.0 {
            let _ = write!(o, " best={:.2}", p.best);
        }
        o.push('\n');
    }
    o
}

fn pct(v: Option<f32>) -> String {
    v.map_or("-".into(), |v| format!("{:.0}%", v * 100.0))
}

/// The report as markdown: the totals, the forks and the ranked table.
pub fn markdown(r: &BouncesReport) -> String {
    let mut o = String::new();
    let _ = writeln!(o, "# Bounce search: {}\n", r.map);
    let outcomes: Vec<String> = r.outcomes.iter().map(|(k, v)| format!("{k} {v}")).collect();
    let _ = writeln!(
        o,
        "{} sites, {} trials of {} planned in {} runs ({}). Variants: {}.\n",
        r.sites.len(),
        r.trials,
        r.planned,
        r.runs,
        outcomes.join(", "),
        r.variants.join(", ")
    );
    let per_wave = if r.fork.waves > 0 {
        r.fork.ticks as f32 / r.fork.waves as f32
    } else {
        0.0
    };
    let last_busy = r
        .fork
        .last_busy
        .as_ref()
        .map(|why| format!(" (the last: {why})"))
        .unwrap_or_default();
    let _ = writeln!(
        o,
        "Forks: {} captures, {} restores, {} busy{last_busy}; {} waves of {:.1} rows. Replayed waves that ended in the same guest RAM: {} of {}. Restores checked against the capture: {} of {} equal.\n",
        r.fork.captures,
        r.fork.restores,
        r.fork.busy,
        r.fork.waves,
        per_wave,
        r.fork.replay_checks - r.fork.replay_mismatches,
        r.fork.replay_checks,
        r.fork.ram_checks - r.fork.ram_mismatches,
        r.fork.ram_checks
    );
    for e in &r.errors {
        let _ = writeln!(o, "**Error:** {e}\n");
    }
    let _ = writeln!(
        o,
        "| # | site | verdict | ease | position cm | press ticks | speed along | phases | provable share | reproducible | drop = anchor | best launch | trials |"
    );
    let _ = writeln!(o, "|---|---|---|---|---|---|---|---|---|---|---|---|---|");
    for s in &r.sites {
        let (position, ticks, speed, phases) = match &s.press {
            Some(p) => (
                format!("{:.1}", p.position * CM),
                p.timing_ticks.map_or("-".into(), |t| format!("{t:.1}")),
                format!("{:.2} to {:.2}", p.speed[0], p.speed[1]),
                pct(p.phase_share),
            ),
            None => ("-".into(), "-".into(), "-".into(), "-".into()),
        };
        let checked = s.variants.iter().filter(|v| v.agrees.is_some()).count();
        let agree = s.variants.iter().filter(|v| v.agrees == Some(true)).count();
        let _ = writeln!(
            o,
            "| {} | {} | {} | {:.2} | {} | {} | {} | {} | {} | {} | {}/{} | {:.2} | {} |",
            s.rank,
            s.id,
            s.verdict.name(),
            s.ease,
            position,
            ticks,
            speed,
            phases,
            pct(s.share),
            pct(s.reproducibility),
            agree,
            checked,
            s.best_launch,
            s.trials
        );
    }
    if !r.notes.is_empty() {
        let _ = writeln!(o, "\nNotes:\n");
        for n in &r.notes {
            let _ = writeln!(o, "- {n}");
        }
    }
    o
}
