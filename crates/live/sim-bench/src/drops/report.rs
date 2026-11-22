//! The drops report (`drops.json`, `drops.md`; docs/runtime/sim.md, "Seam
//! drops"): every run's records pooled, each target's bounce rate by
//! height and across offset, and the three numbers the drops measure for
//! `superbounce`: the catch's half-width, the touchdown speed that
//! bounces (`BOUNCE_SPEED`) and the launch per unit of speed over it
//! (`EXCESS_GAIN`). Also the targets a run proved, for its verified file.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use super::{DropsRun, Outcome, Record, Target};

/// The report's schema tag.
pub const REPORT_FORMAT: &str = "h2-sim-drops-report/1";

/// The launch as a line in the touchdown speed: `launch = base + slope ×
/// (speed - at)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Gain {
    /// Units/s of launch per unit/s of touchdown speed: `EXCESS_GAIN`.
    pub slope: f32,
    /// The launch at `at`; `None` for a pooled fit over several targets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<f32>,
    /// The touchdown speed the base is at.
    pub at: f32,
    /// Bounces fitted.
    pub n: u32,
}

/// The catch: how far from the edge a landing still bounced.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Catch {
    /// The furthest touchdown from the edge that bounced (measured).
    pub half_width: f32,
    /// The furthest planned across offset that bounced.
    pub planned: f32,
    pub target: String,
    pub n: u32,
}

/// The touchdown speed that bounces.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Speed {
    /// The slowest touchdown that bounced: `BOUNCE_SPEED`.
    pub min_bounced: f32,
    /// The fastest primed touchdown within the catch that didn't: under
    /// `min_bounced`, the two bracket the threshold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_unbounced: Option<f32>,
    pub n: u32,
}

/// One height and across offset of a target.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    pub height: f32,
    pub across: f32,
    pub drops: u32,
    pub bounced: u32,
    pub no_bounce: u32,
    pub prime_lost: u32,
    pub setup_failed: u32,
    /// Bounced over measured (bounced and no_bounce); `None` with none
    /// measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<f32>,
}

/// One target's line.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TargetSummary {
    pub id: String,
    pub shape: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub control: bool,
    pub stand: [f32; 3],
    pub drops: u32,
    pub bounced: u32,
    pub no_bounce: u32,
    pub prime_lost: u32,
    pub setup_failed: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<f32>,
    /// The furthest touchdown from the edge that bounced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catch: Option<f32>,
    /// The slowest touchdown that bounced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_speed: Option<f32>,
    /// The best launch, units/s up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gain: Option<Gain>,
    pub cells: Vec<Cell>,
}

/// The first drop of an outcome.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FirstSeen {
    pub target: String,
    pub drop: u32,
    pub shard: u32,
    /// The game tick of its release.
    pub tick: u64,
}

/// One run's control.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ControlLine {
    pub shard: u32,
    pub state: String,
    /// Each control drop's outcome.
    pub outcomes: Vec<String>,
}

/// `drops.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DropsReport {
    pub format: String,
    pub map: String,
    pub runs: u32,
    pub planned: u32,
    pub drops: u32,
    pub outcomes: BTreeMap<String, u32>,
    /// Each outcome's first drop.
    pub first: BTreeMap<String, FirstSeen>,
    pub control: Vec<ControlLine>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catch: Option<Catch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounce_speed: Option<Speed>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gain: Option<Gain>,
    pub targets: Vec<TargetSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// A target a run proved: a verified file's `bounce proven` line.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Proven {
    pub target: String,
    /// The spot's `stand`, as the scan gave it.
    pub stand: [f32; 3],
    /// The best bounce's touchdown, from `stand` (X and Y).
    pub offset: [f32; 2],
    /// Its height.
    pub height: f32,
    /// Measured drops (bounced and no_bounce) and of them the bounces.
    pub trials: u32,
    pub ok: u32,
    /// The best launch, units/s up.
    pub best: f32,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn bounced(r: &Record) -> bool {
    r.outcome == Outcome::Bounced
}

fn speed_in(r: &Record) -> Option<f32> {
    r.touchdown.as_ref().map(|t| t.speed)
}

fn miss(r: &Record) -> Option<f32> {
    r.touchdown.as_ref().map(|t| t.miss)
}

fn launch(r: &Record) -> Option<f32> {
    r.launch.map(|l| l.speed)
}

fn rate(b: u32, nb: u32) -> Option<f32> {
    (b + nb > 0).then(|| b as f32 / (b + nb) as f32)
}

fn fmax(it: impl Iterator<Item = f32>) -> Option<f32> {
    it.fold(None, |m, v| Some(m.map_or(v, |m: f32| m.max(v))))
}

fn fmin(it: impl Iterator<Item = f32>) -> Option<f32> {
    it.fold(None, |m, v| Some(m.map_or(v, |m: f32| m.min(v))))
}

/// The least-squares line through `(speed, launch)` points, its base at
/// `at`: `None` under two points or with no spread in speed.
pub fn fit(points: &[(f32, f32)], at: f32) -> Option<Gain> {
    if points.len() < 2 {
        return None;
    }
    let n = points.len() as f64;
    let mx = points.iter().map(|p| f64::from(p.0)).sum::<f64>() / n;
    let my = points.iter().map(|p| f64::from(p.1)).sum::<f64>() / n;
    let sxx: f64 = points.iter().map(|p| (f64::from(p.0) - mx).powi(2)).sum();
    if sxx < 1e-9 {
        return None;
    }
    let sxy: f64 = points
        .iter()
        .map(|p| (f64::from(p.0) - mx) * (f64::from(p.1) - my))
        .sum();
    let slope = sxy / sxx;
    Some(Gain {
        slope: slope as f32,
        base: Some((my + slope * (f64::from(at) - mx)) as f32),
        at,
        n: points.len() as u32,
    })
}

/// The slope pooled over several targets, each about its own mean (a
/// seam's base launch is its own; the gain is the one thing shared):
/// `Σ(x − x̄ₜ)(y − ȳₜ) / Σ(x − x̄ₜ)²`. `None` without spread.
pub fn pooled(groups: &[Vec<(f32, f32)>], at: f32) -> Option<Gain> {
    let (mut sxy, mut sxx, mut n) = (0.0f64, 0.0f64, 0u32);
    for g in groups.iter().filter(|g| g.len() >= 2) {
        let k = g.len() as f64;
        let mx = g.iter().map(|p| f64::from(p.0)).sum::<f64>() / k;
        let my = g.iter().map(|p| f64::from(p.1)).sum::<f64>() / k;
        for p in g {
            sxx += (f64::from(p.0) - mx).powi(2);
            sxy += (f64::from(p.0) - mx) * (f64::from(p.1) - my);
        }
        n += g.len() as u32;
    }
    (sxx >= 1e-9).then(|| Gain {
        slope: (sxy / sxx) as f32,
        base: None,
        at,
        n,
    })
}

/// The catch over `records`: the furthest bounced touchdown from its edge.
pub fn catch(records: &[Record]) -> Option<Catch> {
    let b: Vec<&Record> = records.iter().filter(|r| bounced(r)).collect();
    let far = b
        .iter()
        .filter_map(|r| miss(r).map(|m| (m, *r)))
        .max_by(|a, b| a.0.total_cmp(&b.0))?;
    Some(Catch {
        half_width: far.0,
        planned: fmax(b.iter().map(|r| r.across.abs())).unwrap_or(0.0),
        target: far.1.target.clone(),
        n: b.len() as u32,
    })
}

/// The touchdown speed that bounces, over `records`; `within` is the
/// catch's half-width a primed miss must be inside to bracket it.
pub fn bounce_speed(records: &[Record], within: f32) -> Option<Speed> {
    let b: Vec<f32> = records.iter().filter(|r| bounced(r)).filter_map(speed_in).collect();
    let min_bounced = fmin(b.iter().copied())?;
    let max_unbounced = fmax(
        records
            .iter()
            .filter(|r| r.outcome == Outcome::NoBounce && miss(r).is_some_and(|m| m <= within))
            .filter_map(speed_in),
    );
    Some(Speed {
        min_bounced,
        max_unbounced,
        n: b.len() as u32,
    })
}

fn same(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

/// Every run's records pooled into `drops.json`.
pub fn summarize(runs: &[DropsRun]) -> DropsReport {
    let mut r = DropsReport {
        format: REPORT_FORMAT.into(),
        map: runs.first().map(|x| x.map.clone()).unwrap_or_default(),
        runs: runs.len() as u32,
        ..DropsReport::default()
    };
    for o in Outcome::ALL {
        r.outcomes.insert(o.name().into(), 0);
    }
    let mut targets: Vec<Target> = Vec::new();
    let mut records: Vec<(u32, &Record)> = Vec::new();
    for run in runs {
        r.planned += run.planned;
        if let Some(e) = &run.error {
            r.errors.push(format!("shard {}: {e}", run.shard));
        }
        for n in &run.notes {
            let n = format!("shard {}: {n}", run.shard);
            if !r.notes.contains(&n) {
                r.notes.push(n);
            }
        }
        r.control.push(ControlLine {
            shard: run.shard,
            state: run.control.clone(),
            outcomes: run
                .records
                .iter()
                .filter(|x| x.control)
                .map(|x| match &x.why {
                    Some(w) => format!("{} ({w})", x.outcome.name()),
                    None => x.outcome.name().into(),
                })
                .collect(),
        });
        for t in &run.targets {
            // The control is one target over every run.
            if !targets.iter().any(|x| x.id == t.id) {
                targets.push(t.clone());
            }
        }
        for rec in &run.records {
            records.push((run.shard, rec));
        }
    }
    r.drops = records.len() as u32;
    for (shard, rec) in &records {
        *r.outcomes.entry(rec.outcome.name().into()).or_default() += 1;
        let first = r
            .first
            .entry(rec.outcome.name().into())
            .or_insert_with(|| FirstSeen {
                target: rec.target.clone(),
                drop: rec.drop,
                shard: *shard,
                tick: rec.tick,
            });
        if (rec.tick, *shard) < (first.tick, first.shard) {
            *first = FirstSeen {
                target: rec.target.clone(),
                drop: rec.drop,
                shard: *shard,
                tick: rec.tick,
            };
        }
    }
    let all: Vec<Record> = records.iter().map(|(_, x)| (*x).clone()).collect();
    r.catch = catch(&all);
    let within = r.catch.as_ref().map_or(0.0, |c| c.half_width);
    r.bounce_speed = bounce_speed(&all, within);
    let at = r.bounce_speed.map_or(0.0, |s| s.min_bounced);
    let mut groups = Vec::new();
    for t in &targets {
        let mine: Vec<Record> = all.iter().filter(|x| x.target == t.id).cloned().collect();
        let points: Vec<(f32, f32)> = mine
            .iter()
            .filter(|x| bounced(x))
            .filter_map(|x| Some((speed_in(x)?, launch(x)?)))
            .collect();
        let count = |o: Outcome| mine.iter().filter(|x| x.outcome == o).count() as u32;
        let mut cells: Vec<Cell> = Vec::new();
        for x in &mine {
            let i = match cells
                .iter()
                .position(|c| same(c.height, x.height) && same(c.across, x.across))
            {
                Some(i) => i,
                None => {
                    cells.push(Cell {
                        height: x.height,
                        across: x.across,
                        ..Cell::default()
                    });
                    cells.len() - 1
                }
            };
            let c = &mut cells[i];
            c.drops += 1;
            match x.outcome {
                Outcome::Bounced => c.bounced += 1,
                Outcome::NoBounce => c.no_bounce += 1,
                Outcome::PrimeLost => c.prime_lost += 1,
                Outcome::SetupFailed => c.setup_failed += 1,
            }
        }
        for c in &mut cells {
            c.rate = rate(c.bounced, c.no_bounce);
        }
        cells.sort_by(|a, b| a.height.total_cmp(&b.height).then(a.across.total_cmp(&b.across)));
        let (b, nb) = (count(Outcome::Bounced), count(Outcome::NoBounce));
        r.targets.push(TargetSummary {
            id: t.id.clone(),
            shape: t.shape.clone(),
            control: t.control,
            stand: t.stand,
            drops: mine.len() as u32,
            bounced: b,
            no_bounce: nb,
            prime_lost: count(Outcome::PrimeLost),
            setup_failed: count(Outcome::SetupFailed),
            rate: rate(b, nb),
            catch: fmax(mine.iter().filter(|x| bounced(x)).filter_map(miss)),
            min_speed: fmin(mine.iter().filter(|x| bounced(x)).filter_map(speed_in)),
            best: fmax(mine.iter().filter(|x| bounced(x)).filter_map(launch)),
            gain: fit(&points, at),
            cells,
        });
        groups.push(points);
    }
    r.gain = pooled(&groups, at);
    // Best first: the most bounces, then the rate, then the list's order.
    r.targets.sort_by(|a, b| {
        b.control
            .cmp(&a.control)
            .then(b.bounced.cmp(&a.bounced))
            .then(b.rate.unwrap_or(0.0).total_cmp(&a.rate.unwrap_or(0.0)))
    });
    r
}

/// The targets `records` proved: each non-control target with a bounce,
/// at its best bounce (the biggest launch).
pub fn proven(targets: &[Target], records: &[Record]) -> Vec<Proven> {
    let mut out = Vec::new();
    for t in targets.iter().filter(|t| !t.control) {
        let mine: Vec<&Record> = records.iter().filter(|r| r.target == t.id).collect();
        let Some(best) = mine
            .iter()
            .filter(|r| bounced(r) && r.touchdown.is_some())
            .max_by(|a, b| launch(a).unwrap_or(0.0).total_cmp(&launch(b).unwrap_or(0.0)))
        else {
            continue;
        };
        let at = best.touchdown.as_ref().map_or(best.point, |d| d.pos);
        out.push(Proven {
            target: t.id.clone(),
            stand: t.stand,
            offset: [at[0] - t.stand[0], at[1] - t.stand[1]],
            height: best.height,
            trials: mine
                .iter()
                .filter(|r| matches!(r.outcome, Outcome::Bounced | Outcome::NoBounce))
                .count() as u32,
            ok: mine.iter().filter(|r| bounced(r)).count() as u32,
            best: launch(best).unwrap_or(0.0),
        });
    }
    out
}

fn opt(v: Option<f32>, digits: usize) -> String {
    v.map_or("-".into(), |v| format!("{v:.digits$}"))
}

/// `drops.md`: the totals, the measured numbers, a line a target, and a
/// height by across table for each target that bounced.
pub fn markdown(r: &DropsReport) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "# Seam drops: {}\n", r.map);
    let _ = writeln!(
        s,
        "{} runs, {} of {} planned drops: {}",
        r.runs,
        r.drops,
        r.planned,
        r.outcomes
            .iter()
            .map(|(k, v)| format!("{k} {v}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    for e in &r.errors {
        let _ = writeln!(s, "\n**{e}**");
    }
    let _ = writeln!(s, "\n## Control\n");
    for c in &r.control {
        let _ = writeln!(
            s,
            "- shard {}: {} ({})",
            c.shard,
            c.state,
            if c.outcomes.is_empty() {
                "no drop".into()
            } else {
                c.outcomes.join(", ")
            }
        );
    }
    let _ = writeln!(s, "\n## Measured\n");
    match &r.catch {
        Some(c) => {
            let _ = writeln!(
                s,
                "- catch half-width: {:.3} from the edge (planned across {:.3}; {}, {} bounces)",
                c.half_width, c.planned, c.target, c.n
            );
        }
        None => {
            let _ = writeln!(s, "- catch half-width: no bounce");
        }
    }
    match &r.bounce_speed {
        Some(b) => {
            let _ = writeln!(
                s,
                "- BOUNCE_SPEED: {:.2} units/s, the slowest touchdown that bounced; the fastest primed one in the catch that didn't: {}",
                b.min_bounced,
                opt(b.max_unbounced, 2)
            );
        }
        None => {
            let _ = writeln!(s, "- BOUNCE_SPEED: no bounce");
        }
    }
    match &r.gain {
        Some(g) => {
            let _ = writeln!(
                s,
                "- EXCESS_GAIN: {:.3} units/s of launch per unit/s over {:.2} ({} bounces)",
                g.slope, g.at, g.n
            );
        }
        None => {
            let _ = writeln!(s, "- EXCESS_GAIN: too few bounces at different speeds");
        }
    }
    if !r.first.is_empty() {
        let _ = writeln!(s, "\n## First of each\n");
        for (k, f) in &r.first {
            let _ = writeln!(
                s,
                "- {k}: {} drop {} (shard {}), tick {}",
                f.target, f.drop, f.shard, f.tick
            );
        }
    }
    let _ = writeln!(s, "\n## Targets\n");
    let _ = writeln!(
        s,
        "| target | shape | drops | bounced | no_bounce | prime_lost | setup_failed | rate | catch | min speed | best | gain |"
    );
    let _ = writeln!(s, "|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|");
    for t in &r.targets {
        let _ = writeln!(
            s,
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            t.id,
            t.shape,
            t.drops,
            t.bounced,
            t.no_bounce,
            t.prime_lost,
            t.setup_failed,
            opt(t.rate, 2),
            opt(t.catch, 3),
            opt(t.min_speed, 2),
            opt(t.best, 2),
            opt(t.gain.map(|g| g.slope), 2)
        );
    }
    for t in r.targets.iter().filter(|t| t.bounced > 0) {
        let mut heights: Vec<f32> = Vec::new();
        let mut acrosses: Vec<f32> = Vec::new();
        for c in &t.cells {
            if !heights.iter().any(|h| same(*h, c.height)) {
                heights.push(c.height);
            }
            if !acrosses.iter().any(|a| same(*a, c.across)) {
                acrosses.push(c.across);
            }
        }
        heights.sort_by(f32::total_cmp);
        acrosses.sort_by(f32::total_cmp);
        let _ = writeln!(
            s,
            "\n### {} ({}, stand {:.3} {:.3} {:.3})\n\nbounced of measured, by height (rows) and across (columns); `x` prime lost, `-` none measured\n",
            t.id, t.shape, t.stand[0], t.stand[1], t.stand[2]
        );
        let _ = write!(s, "| height |");
        for a in &acrosses {
            let _ = write!(s, " {a:+.2} |");
        }
        let _ = writeln!(s);
        let _ = write!(s, "|---:|");
        for _ in &acrosses {
            let _ = write!(s, "---:|");
        }
        let _ = writeln!(s);
        for h in &heights {
            let _ = write!(s, "| {h:.2} |");
            for a in &acrosses {
                let cell = t
                    .cells
                    .iter()
                    .find(|c| same(c.height, *h) && same(c.across, *a));
                let txt = match cell {
                    Some(c) if c.bounced + c.no_bounce > 0 => {
                        format!("{}/{}", c.bounced, c.bounced + c.no_bounce)
                    }
                    Some(c) if c.prime_lost > 0 => "x".into(),
                    _ => "-".into(),
                };
                let _ = write!(s, " {txt} |");
            }
            let _ = writeln!(s);
        }
    }
    if !r.notes.is_empty() {
        let _ = writeln!(s, "\n## Notes\n");
        for n in &r.notes {
            let _ = writeln!(s, "- {n}");
        }
    }
    s
}
